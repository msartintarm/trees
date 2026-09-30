//! The continuous terrain surface: a smooth heightfield through the tile
//! centers (the hex grid's dual — tile centers form a triangular lattice, so
//! triangulating them gives equilateral faces with one vertex per tile), with
//! biome landforms exaggerated on top of the simulated relief (dune fields,
//! mesa terraces, jagged alpine ridges, tundra hummocks), per-vertex ground
//! material weights for the terrain shader, static ambient occlusion, and
//! the chunked index buffers the renderer frustum-culls.
//!
//! Pure data — built natively, tested natively. Everything that stands on
//! the ground (plants, houses, the player) reads its height from here, so
//! nothing floats above or sinks into the exaggerated landforms.

use bytemuck::{Pod, Zeroable};

use crate::sim::hex::{self, Grid, SQRT3};
use crate::sim::rng::mix64;
use crate::sim::world::{Biome, World, BIOME_COUNT};

/// Ground materials the terrain shader knows (the biomes plus bare rock).
pub const MATERIAL_COUNT: usize = 8;
pub const MAT_ROCK: usize = 7;

/// Tiles per chunk side for frustum culling.
pub const CHUNK: i32 = 32;

/// Landform amplitudes (world units; a tree is ~2.5 tall).
const DUNE_AMP: f64 = 1.1;
/// Dune wavelength across the ridges.
const DUNE_WAVE: f64 = 7.5;
const TERRACE_STEP: f64 = 1.6;
const PEAK_AMP: f64 = 5.0;
const HUMMOCK_AMP: f64 = 0.35;

/// One terrain vertex (one per tile): 72 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TerrainVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Material weights: wetland, tundra, boreal, temperate forest.
    pub mat_a: [f32; 4],
    /// Grassland, savanna, desert, rock.
    pub mat_b: [f32; 4],
    /// [static ambient occlusion 0..1 (1 = open), downslope x, downslope y,
    /// column drop: how far this tile's hex column must reach down to meet
    /// its lowest neighbor (or the floor, at the map edge)].
    pub extra: [f32; 4],
}

/// A culling chunk: a contiguous index range and its world bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chunk {
    pub first: u32,
    pub count: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Cheap hashed value noise (render-side; independent of the sim's RNG
/// streams so display landforms never perturb the simulation).
fn hash(ix: i64, iy: i64, salt: u64) -> f64 {
    let k = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ salt;
    (mix64(k) >> 11) as f64 / (1u64 << 53) as f64
}

pub fn noise(x: f64, y: f64, salt: u64) -> f64 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (ix, iy) = (xi as i64, yi as i64);
    let a = hash(ix, iy, salt);
    let b = hash(ix + 1, iy, salt);
    let c = hash(ix, iy + 1, salt);
    let d = hash(ix + 1, iy + 1, salt);
    let (u, v) = (s(fx), s(fy));
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

fn fbm(x: f64, y: f64, salt: u64) -> f64 {
    0.55 * noise(x, y, salt) + 0.3 * noise(x * 2.1, y * 2.1, salt + 1) + 0.15 * noise(x * 4.3, y * 4.3, salt + 2)
}

/// Asymmetric dune profile along the wind: a long gentle windward slope and
/// a short steep slip face, 0..1.
fn dune(phase: f64) -> f64 {
    let f = phase.rem_euclid(1.0);
    let p = if f < 0.78 { f / 0.78 } else { (1.0 - f) / 0.22 };
    p * p * (3.0 - 2.0 * p)
}

/// Soft staircase: flat mesa treads joined by steep risers.
fn terrace(h: f64, step: f64) -> f64 {
    let k = h / step;
    let base = k.floor();
    let f = k - base;
    let riser = ((f - 0.72) / 0.28).clamp(0.0, 1.0);
    (base + riser * riser * (3.0 - 2.0 * riser)) * step
}

/// World position of lattice vertex (col, row).
fn lattice_xy(col: i32, row: i32) -> (f64, f64) {
    let shift = if row & 1 == 1 { 0.5 } else { 0.0 };
    (SQRT3 * (col as f64 + shift), 1.5 * row as f64)
}

/// The display surface of a world: per-tile heights with landforms, and
/// interpolation anywhere on the map.
#[derive(Clone, Debug)]
pub struct Surface {
    grid: Grid,
    heights: Vec<f32>,
    /// Landform relief per tile, 0..1 (for the shader's detail).
    relief: Vec<f32>,
    max: f32,
    /// Hex columns: every tile is a flat-topped step (the platformer
    /// look) instead of a smooth surface through the tile centers.
    hex: bool,
}

impl Surface {
    /// A surface that is exactly the simulation's tile elevations (no
    /// landforms) — the legacy look, and what tests calibrate against.
    pub fn flat_from(world: &World) -> Surface {
        let n = world.grid().cells();
        let heights: Vec<f32> = (0..n).map(|i| world.elevation(i)).collect();
        let max = heights.iter().cloned().fold(0.0f32, f32::max).max(world.max_elevation());
        Surface { grid: world.grid(), heights, relief: vec![0.0; n], max, hex: false }
    }

    /// The same heights as flat-topped hex columns (or back to smooth).
    pub fn with_hex(mut self, hex: bool) -> Surface {
        self.hex = hex;
        self
    }

    pub fn is_hex(&self) -> bool {
        self.hex
    }

    /// The display surface: the simulated relief plus biome landforms,
    /// scaled by `landforms` (0 = none).
    pub fn build(world: &World, landforms: f64) -> Surface {
        let grid = world.grid();
        let n = grid.cells();
        let weights = biome_weights(world);
        let amp = world.altitude_range().max(1e-3) as f64;
        let mut heights = Vec::with_capacity(n);
        let mut relief = Vec::with_capacity(n);
        // Prevailing dune-building wind (fixed per map, from the seed-free
        // map geometry so dunes don't reorient on reseed of the same size).
        let wind = (0.93f64, 0.37f64);
        for i in 0..n {
            let base = world.elevation(i) as f64;
            let (x, y) = grid.center(i);
            if landforms <= 0.0 || world.is_channel(i) {
                heights.push(base as f32);
                relief.push(0.0);
                continue;
            }
            let w = &weights[i];
            let desert = w[Biome::Desert as usize];
            let savanna = w[Biome::Savanna as usize];
            let tundra = w[Biome::Tundra as usize];
            let wet = w[Biome::Wetland as usize];
            let rock = w[MAT_ROCK];
            let mut h = base;
            let mut r = 0.0f64;
            // Dune fields: ridges across the wind, the phase warped by noise
            // so crests meander and fork like real transverse dunes.
            if desert > 0.05 {
                let along = x * wind.0 + y * wind.1;
                let warp = 1.4 * fbm(x / 23.0, y / 23.0, 11);
                let crest = dune(along / DUNE_WAVE + warp);
                let field = (0.35 + 0.65 * noise(x / 40.0, y / 40.0, 12)) * (1.0 - rock);
                h += desert * DUNE_AMP * crest * field;
                r = r.max(desert * field);
            }
            // Mesas and buttes: the dry uplands step into flat treads and
            // cliffed risers where a noise mask says the caprock survives.
            let dry = (desert + savanna).min(1.0);
            let caprock = ((noise(x / 55.0, y / 55.0, 21) - 0.45) / 0.2).clamp(0.0, 1.0);
            if dry > 0.2 && caprock > 0.0 && base > 1.0 {
                let t = dry * caprock;
                h = h + (terrace(h, TERRACE_STEP) + 0.6 * TERRACE_STEP - h) * t;
                r = r.max(t);
            }
            // Jagged peaks: ridged noise on the high rock, strongest at the
            // top of the altitude range.
            let high = (world.altitude(i) as f64 / amp).clamp(0.0, 1.0);
            if rock > 0.05 && high > 0.4 {
                let ridged = 1.0 - (2.0 * fbm(x / 9.0, y / 9.0, 31) - 1.0).abs();
                let t = rock * ((high - 0.4) / 0.6).powf(1.5);
                h += PEAK_AMP * t * ridged * ridged;
                r = r.max(t);
            }
            // Frost hummocks on the tundra.
            if tundra > 0.05 {
                h += HUMMOCK_AMP * tundra * (noise(x / 2.2, y / 2.2, 41) - 0.3);
                r = r.max(0.4 * tundra);
            }
            // Wetlands stay flat (standing water finds its level).
            h = base + (h - base) * (1.0 - wet) * landforms;
            heights.push(h as f32);
            relief.push((r * landforms).clamp(0.0, 1.0) as f32);
        }
        let max = heights.iter().cloned().fold(0.0f32, f32::max);
        Surface { grid, heights, relief, max, hex: false }
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// Display height of a tile's center.
    pub fn tile_height(&self, i: usize) -> f32 {
        self.heights[i]
    }

    pub fn relief(&self, i: usize) -> f32 {
        self.relief[i]
    }

    /// The highest point on the surface.
    pub fn max_height(&self) -> f32 {
        self.max
    }

    fn h_at(&self, col: i32, row: i32) -> f64 {
        let c = col.clamp(0, self.grid.width - 1);
        let r = row.clamp(0, self.grid.height - 1);
        self.heights[(r * self.grid.width + c) as usize] as f64
    }

    /// Surface height anywhere (barycentric over the lattice triangle that
    /// contains the point; clamped to the map's edge outside it).
    pub fn height_at(&self, x: f64, y: f64) -> f64 {
        let (_, _, max_x, max_y) = self.grid.world_bounds();
        let x = x.clamp(0.0, max_x);
        let y = y.clamp(0.0, max_y);
        if self.hex {
            // The containing tile's flat top.
            return self.grid.pick(x, y).map_or_else(
                || self.h_at((x / SQRT3).round() as i32, (y / 1.5).round() as i32),
                |i| self.heights[i] as f64,
            );
        }
        let row = ((y / 1.5).floor() as i32).clamp(0, (self.grid.height - 2).max(0));
        if self.grid.height < 2 {
            return self.h_at(((x / SQRT3).round()) as i32, 0);
        }
        // Candidate triangles between rows `row` and `row + 1`.
        let c0 = (x / SQRT3).floor() as i32;
        let mut best = None;
        let mut best_err = f64::INFINITY;
        for col in (c0 - 1)..=(c0 + 1) {
            for tri in triangles_at(col, row) {
                let [a, b, c] = tri;
                let pa = lattice_xy(a.0, a.1);
                let pb = lattice_xy(b.0, b.1);
                let pc = lattice_xy(c.0, c.1);
                let det = (pb.1 - pc.1) * (pa.0 - pc.0) + (pc.0 - pb.0) * (pa.1 - pc.1);
                if det.abs() < 1e-12 {
                    continue;
                }
                let l1 = ((pb.1 - pc.1) * (x - pc.0) + (pc.0 - pb.0) * (y - pc.1)) / det;
                let l2 = ((pc.1 - pa.1) * (x - pc.0) + (pa.0 - pc.0) * (y - pc.1)) / det;
                let l3 = 1.0 - l1 - l2;
                let err = (-l1).max(0.0) + (-l2).max(0.0) + (-l3).max(0.0);
                if err < best_err {
                    best_err = err;
                    let (l1, l2, l3) = (l1.max(0.0), l2.max(0.0), l3.max(0.0));
                    let s = (l1 + l2 + l3).max(1e-9);
                    best = Some((l1 * self.h_at(a.0, a.1) + l2 * self.h_at(b.0, b.1) + l3 * self.h_at(c.0, c.1)) / s);
                }
                if err == 0.0 {
                    return best.unwrap();
                }
            }
        }
        best.unwrap_or(0.0)
    }

    /// Surface normal anywhere (central differences).
    pub fn normal_at(&self, x: f64, y: f64) -> [f64; 3] {
        if self.hex {
            return [0.0, 0.0, 1.0];
        }
        let e = 0.5;
        let dx = (self.height_at(x + e, y) - self.height_at(x - e, y)) / (2.0 * e);
        let dy = (self.height_at(x, y + e) - self.height_at(x, y - e)) / (2.0 * e);
        let len = (dx * dx + dy * dy + 1.0).sqrt();
        [-dx / len, -dy / len, 1.0 / len]
    }
}

/// The two lattice triangles whose lower-left vertex column is `col`
/// between rows `row` and `row + 1`, counter-clockwise from above, as
/// (col, row) vertex triples.
fn triangles_at(col: i32, row: i32) -> [[(i32, i32); 3]; 2] {
    if row & 1 == 0 {
        // Even row below, shifted odd row above.
        [[(col, row), (col + 1, row), (col, row + 1)], [(col + 1, row), (col + 1, row + 1), (col, row + 1)]]
    } else {
        // Shifted odd row below, even row above.
        [[(col, row), (col + 1, row + 1), (col, row + 1)], [(col, row), (col + 1, row), (col + 1, row + 1)]]
    }
}

/// Per-tile material weights (biome one-hot plus rock), softened over the
/// neighbors so biome edges blend into ecotones.
fn biome_weights(world: &World) -> Vec<[f64; MATERIAL_COUNT]> {
    let grid = world.grid();
    let n = grid.cells();
    let raw: Vec<[f64; MATERIAL_COUNT]> = (0..n)
        .map(|i| {
            let mut w = [0.0; MATERIAL_COUNT];
            w[world.biome(i) as usize] = 1.0;
            // Thin, stony soils (ridges, the high mountains) show bare rock.
            let rock = ((0.5 - world.soil_depth(i)) / 0.4).clamp(0.0, 1.0);
            for v in w.iter_mut() {
                *v *= 1.0 - rock;
            }
            w[MAT_ROCK] += rock;
            w
        })
        .collect();
    (0..n)
        .map(|i| {
            let (q, r) = grid.index_to_axial(i);
            let mut acc = raw[i].map(|v| v * 2.0);
            let mut total = 2.0;
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = grid.axial_to_index(q + dq, r + dr) {
                    for (a, b) in acc.iter_mut().zip(raw[j].iter()) {
                        *a += b;
                    }
                    total += 1.0;
                }
            }
            acc.map(|v| v / total)
        })
        .collect()
}

/// The terrain vertex buffer: one vertex per tile, in tile-index order (so
/// the per-frame ground stream lines up with it vertex for vertex).
pub fn terrain_vertices(world: &World, surface: &Surface) -> Vec<TerrainVertex> {
    let grid = world.grid();
    let n = grid.cells();
    let weights = biome_weights(world);
    // Mean height within ~3 tiles, for valley occlusion.
    let ring = hex::disk(3);
    let smooth = surface.clone().with_hex(false);
    let surface = &smooth;
    (0..n)
        .map(|i| {
            let (x, y) = grid.center(i);
            let h = surface.tile_height(i) as f64;
            let nrm = surface.normal_at(x, y);
            let (q, r) = grid.index_to_axial(i);
            let mut sum = 0.0;
            let mut cnt = 0.0f64;
            for &(dq, dr, _) in &ring {
                if let Some(j) = grid.axial_to_index(q + dq, r + dr) {
                    sum += surface.tile_height(j) as f64;
                    cnt += 1.0;
                }
            }
            let hollow = sum / cnt.max(1.0) - h;
            let ao = (1.0 - 0.12 * hollow).clamp(0.55, 1.08);
            // Downslope direction (river ripples run along it).
            let gl = (nrm[0] * nrm[0] + nrm[1] * nrm[1]).sqrt();
            let flow = if gl > 1e-4 { [nrm[0] / gl, nrm[1] / gl] } else { [1.0, 0.0] };
            let w = weights[i];
            // Hex columns only need walls down to the lowest neighbor.
            let mut low = h;
            let mut edge = false;
            for (dq, dr) in hex::NEIGHBORS {
                match grid.axial_to_index(q + dq, r + dr) {
                    Some(j) => low = low.min(surface.tile_height(j) as f64),
                    None => edge = true,
                }
            }
            let drop = if edge { h + super::geometry::TILE_HEIGHT as f64 } else { h - low + 0.15 };
            TerrainVertex {
                pos: [x as f32, y as f32, h as f32],
                normal: [nrm[0] as f32, nrm[1] as f32, nrm[2] as f32],
                mat_a: [w[0] as f32, w[1] as f32, w[2] as f32, w[3] as f32],
                mat_b: [w[4] as f32, w[5] as f32, w[6] as f32, w[7] as f32],
                extra: [ao as f32, flow[0] as f32, flow[1] as f32, drop as f32],
            }
        })
        .collect()
}

/// Chunked triangle indices over the tile-center lattice, with each chunk's
/// world bounds (from the vertex heights).
pub fn terrain_indices(grid: Grid, vertices: &[TerrainVertex]) -> (Vec<u32>, Vec<Chunk>) {
    let (w, h) = (grid.width, grid.height);
    let mut indices = Vec::with_capacity(((w - 1) * (h - 1) * 6).max(0) as usize);
    let mut chunks = Vec::new();
    let idx = |c: i32, r: i32| (r * w + c) as u32;
    let mut cy = 0;
    while cy < h - 1 {
        let mut cx = 0;
        while cx < w - 1 {
            let first = indices.len() as u32;
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for row in cy..(cy + CHUNK).min(h - 1) {
                for col in cx..(cx + CHUNK).min(w - 1) {
                    for tri in triangles_at(col, row) {
                        for &(c, r) in &tri {
                            let c = c.min(w - 1);
                            let v = idx(c, r);
                            indices.push(v);
                            let p = vertices[v as usize].pos;
                            for k in 0..3 {
                                min[k] = min[k].min(p[k]);
                                max[k] = max[k].max(p[k]);
                            }
                        }
                    }
                }
            }
            let count = indices.len() as u32 - first;
            if count > 0 {
                chunks.push(Chunk { first, count, min, max });
            }
            cx += CHUNK;
        }
        cy += CHUNK;
    }
    (indices, chunks)
}

/// Chunk table as flat floats for the renderer: [first, count, min xyz,
/// max xyz] per chunk.
pub fn chunk_table(chunks: &[Chunk]) -> Vec<f32> {
    chunks
        .iter()
        .flat_map(|c| [c.first as f32, c.count as f32, c.min[0], c.min[1], c.min[2], c.max[0], c.max[1], c.max[2]])
        .collect()
}

/// The walls around the map's edge, dropping from the surface to a floor,
/// plus the floor itself (soil-colored, baked material), as a static mesh.
pub fn skirt_mesh(surface: &Surface) -> super::geometry::MeshData {
    use super::geometry::{MeshData, MeshVertex};
    let grid = surface.grid;
    let (w, h) = (grid.width, grid.height);
    let floor = -super::geometry::TILE_HEIGHT;
    let soil = [0.30, 0.24, 0.17];
    let mut m = MeshData { vertices: Vec::new(), indices: Vec::new() };
    let mut push = |p: [f32; 3], n: [f32; 3]| {
        let k = m.vertices.len() as u32;
        m.vertices.push(MeshVertex { pos: p, normal: n, color: soil, color_weight: 1.0 });
        k
    };
    // Perimeter in order: bottom row, right column, top row, left column.
    let mut ring: Vec<(i32, i32)> = Vec::new();
    ring.extend((0..w).map(|c| (c, 0)));
    ring.extend((1..h).map(|r| (w - 1, r)));
    ring.extend((0..w - 1).rev().map(|c| (c, h - 1)));
    ring.extend((1..h - 1).rev().map(|r| (0, r)));
    let (cx, cy) = {
        let (a, b, c, d) = grid.world_bounds();
        ((a + c) / 2.0, (b + d) / 2.0)
    };
    let mut quads = Vec::new();
    for k in 0..ring.len() {
        let (c0, r0) = ring[k];
        let (c1, r1) = ring[(k + 1) % ring.len()];
        let (x0, y0) = lattice_xy(c0, r0);
        let (x1, y1) = lattice_xy(c1, r1);
        let z0 = surface.h_at(c0, r0) as f32;
        let z1 = surface.h_at(c1, r1) as f32;
        let mx = ((x0 + x1) / 2.0 - cx) as f32;
        let my = ((y0 + y1) / 2.0 - cy) as f32;
        let l = (mx * mx + my * my).sqrt().max(1e-6);
        let nrm = [mx / l, my / l, 0.0];
        quads.push([
            push([x0 as f32, y0 as f32, floor], nrm),
            push([x1 as f32, y1 as f32, floor], nrm),
            push([x1 as f32, y1 as f32, z1], nrm),
            push([x0 as f32, y0 as f32, z0], nrm),
        ]);
    }
    for q in quads {
        m.indices.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
    let (a, b, c, d) = grid.world_bounds();
    let pad = 0.5;
    let f = [
        push([(a - pad) as f32, (b - pad) as f32, floor], [0.0, 0.0, 1.0]),
        push([(c + pad) as f32, (b - pad) as f32, floor], [0.0, 0.0, 1.0]),
        push([(c + pad) as f32, (d + pad) as f32, floor], [0.0, 0.0, 1.0]),
        push([(a - pad) as f32, (d + pad) as f32, floor], [0.0, 0.0, 1.0]),
    ];
    m.indices.extend([f[0], f[1], f[2], f[0], f[2], f[3]]);
    m
}

/// Biome weights at a tile for the renderer's regional color grade (the
/// dominant biome nearby tints the light).
pub fn dominant_biome_near(world: &World, center: [f64; 2], radius: f64) -> [f32; BIOME_COUNT] {
    let grid = world.grid();
    let mut acc = [0.0f32; BIOME_COUNT];
    let cells = grid.cells_in_box(center, radius);
    let step = (cells.len() / 200).max(1);
    let mut total = 0.0;
    for &i in cells.iter().step_by(step) {
        acc[world.biome(i) as usize] += 1.0;
        total += 1.0;
    }
    if total > 0.0 {
        for a in acc.iter_mut() {
            *a /= total;
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::world::{Params, World};

    fn world() -> World {
        World::with_params(3, Params { width: 48, height: 40, ..Params::default() })
    }

    #[test]
    fn heights_interpolate_through_tile_centers() {
        let w = world();
        let s = Surface::build(&w, 1.0);
        for i in [0, 17, 500, w.grid().cells() - 1] {
            let (x, y) = w.grid().center(i);
            assert!((s.height_at(x, y) - s.tile_height(i) as f64).abs() < 1e-4, "tile {i}");
        }
        // Between two neighbors the surface lies between their heights.
        let a = w.grid().offset_to_index(10, 10).unwrap();
        let b = w.grid().offset_to_index(11, 10).unwrap();
        let (ax, ay) = w.grid().center(a);
        let (bx, by) = w.grid().center(b);
        let mid = s.height_at((ax + bx) / 2.0, (ay + by) / 2.0);
        let (lo, hi) = (s.tile_height(a).min(s.tile_height(b)), s.tile_height(a).max(s.tile_height(b)));
        assert!(mid >= lo as f64 - 1e-4 && mid <= hi as f64 + 1e-4);
    }

    #[test]
    fn the_flat_surface_is_the_simulated_relief() {
        let w = world();
        let s = Surface::flat_from(&w);
        for i in 0..w.grid().cells() {
            assert_eq!(s.tile_height(i), w.elevation(i));
        }
        let none = Surface::build(&w, 0.0);
        for i in 0..w.grid().cells() {
            assert_eq!(none.tile_height(i), w.elevation(i));
        }
    }

    #[test]
    fn the_mesh_covers_the_lattice_in_chunks() {
        let w = world();
        let s = Surface::build(&w, 1.0);
        let v = terrain_vertices(&w, &s);
        assert_eq!(v.len(), w.grid().cells());
        let (idx, chunks) = terrain_indices(w.grid(), &v);
        let g = w.grid();
        assert_eq!(idx.len() as i32, (g.width - 1) * (g.height - 1) * 6);
        assert_eq!(chunks.iter().map(|c| c.count).sum::<u32>() as usize, idx.len());
        assert_eq!(chunks.len(), 4, "48×40 tiles → 2×2 chunks of 32");
        assert!(idx.iter().all(|&k| (k as usize) < v.len()));
        // Every triangle is counter-clockwise from above and non-degenerate.
        for t in idx.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|k| v[k as usize].pos);
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(cross > 0.1, "triangle {t:?} winds clockwise or is degenerate");
        }
        // Normals point up and material weights sum to one.
        for vx in &v {
            assert!(vx.normal[2] > 0.0);
            let total: f32 = vx.mat_a.iter().chain(vx.mat_b.iter()).sum();
            assert!((total - 1.0).abs() < 1e-3, "weights sum {total}");
        }
    }

    #[test]
    fn landforms_raise_dunes_in_the_desert_only() {
        // A hot, dry coast-to-desert map on the default size has deserts.
        let w = World::with_params(7, Params { width: 128, height: 128, climate_zones: 0.4, ..Params::default() });
        let s = Surface::build(&w, 1.0);
        let mut desert_lift = 0.0;
        let mut desert_n = 0.0;
        for i in 0..w.grid().cells() {
            let d = s.tile_height(i) - w.elevation(i);
            if w.is_channel(i) {
                assert_eq!(d, 0.0, "rivers keep their bed");
            }
            if w.biome(i) == Biome::Wetland {
                assert!(d.abs() < 0.2, "wetlands stay level");
            }
            if w.biome(i) == Biome::Desert {
                desert_lift += d.abs() as f64;
                desert_n += 1.0;
            }
        }
        if desert_n > 20.0 {
            assert!(desert_lift / desert_n > 0.15, "dunes should sculpt the desert");
        }
    }

    #[test]
    fn hex_mode_makes_every_tile_a_flat_step() {
        let w = world();
        let s = Surface::build(&w, 1.0).with_hex(true);
        let i = w.grid().offset_to_index(12, 9).unwrap();
        let (x, y) = w.grid().center(i);
        for (dx, dy) in [(0.0, 0.0), (0.5, 0.2), (-0.3, -0.5)] {
            assert_eq!(s.height_at(x + dx, y + dy), s.tile_height(i) as f64);
        }
        assert_eq!(s.normal_at(x, y), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn the_skirt_closes_the_map_edge() {
        let w = world();
        let s = Surface::build(&w, 1.0);
        let m = skirt_mesh(&s);
        let g = w.grid();
        let perimeter = 2 * (g.width + g.height) - 4;
        assert_eq!(m.indices.len() as i32, perimeter * 6 + 6);
    }
}
