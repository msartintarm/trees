//! Static meshes baked once at Renderer creation: a hex ground tile, a
//! low-poly tree (trunk + conical canopy), and a grass tuft. Vertices carry a
//! color + weight so multi-material meshes (trunk vs canopy) work with a
//! single instance color: `final = mix(instance_color, vertex_color, weight)`.

use bytemuck::{Pod, Zeroable};

use crate::sim::hex::SIZE;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    /// 0 = take the instance color, 1 = keep this vertex color.
    pub color_weight: f32,
}

#[derive(Clone)]
pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    fn new() -> MeshData {
        MeshData { vertices: Vec::new(), indices: Vec::new() }
    }

    fn push(&mut self, pos: [f32; 3], normal: [f32; 3], color: [f32; 3], weight: f32) -> u32 {
        let i = self.vertices.len() as u32;
        self.vertices.push(MeshVertex { pos, normal, color, color_weight: weight });
        i
    }
}

/// Corner angles of a pointy-top hex (corner toward +y).
fn hex_corner(k: usize, radius: f32) -> (f32, f32) {
    let a = (60.0 * k as f32 + 30.0).to_radians();
    (radius * a.cos(), radius * a.sin())
}

/// Hexagonal prism from `z_bottom` to `z_top`: lit top face + outward side
/// walls (no bottom face — the camera never goes below the ground plane).
fn hex_prism(mesh: &mut MeshData, radius: f32, z_bottom: f32, z_top: f32, color: [f32; 3], weight: f32) {
    let mut top = [0u32; 6];
    for (k, slot) in top.iter_mut().enumerate() {
        let (x, y) = hex_corner(k, radius);
        *slot = mesh.push([x, y, z_top], [0.0, 0.0, 1.0], color, weight);
    }
    for k in 1..5 {
        mesh.indices.extend([top[0], top[k], top[k + 1]]);
    }
    for k in 0..6 {
        let (x0, y0) = hex_corner(k, radius);
        let (x1, y1) = hex_corner((k + 1) % 6, radius);
        let n = {
            let (nx, ny) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            let len = (nx * nx + ny * ny).sqrt();
            [nx / len, ny / len, 0.0]
        };
        let a = mesh.push([x0, y0, z_bottom], n, color, weight);
        let b = mesh.push([x1, y1, z_bottom], n, color, weight);
        let c = mesh.push([x1, y1, z_top], n, color, weight);
        let d = mesh.push([x0, y0, z_top], n, color, weight);
        mesh.indices.extend([a, b, c, a, c, d]);
    }
}

/// Open-ended cylinder with radial normals, centered at (cx, cy).
#[allow(clippy::too_many_arguments)]
fn cylinder(mesh: &mut MeshData, sides: usize, cx: f32, cy: f32, radius: f32, z0: f32, z1: f32, color: [f32; 3], weight: f32) {
    for k in 0..sides {
        let a0 = std::f32::consts::TAU * k as f32 / sides as f32;
        let a1 = std::f32::consts::TAU * (k + 1) as f32 / sides as f32;
        let (x0, y0) = (cx + radius * a0.cos(), cy + radius * a0.sin());
        let (x1, y1) = (cx + radius * a1.cos(), cy + radius * a1.sin());
        let n0 = [a0.cos(), a0.sin(), 0.0];
        let n1 = [a1.cos(), a1.sin(), 0.0];
        let a = mesh.push([x0, y0, z0], n0, color, weight);
        let b = mesh.push([x1, y1, z0], n1, color, weight);
        let c = mesh.push([x1, y1, z1], n1, color, weight);
        let d = mesh.push([x0, y0, z1], n0, color, weight);
        mesh.indices.extend([a, b, c, a, c, d]);
    }
}

/// Cone with smooth side normals, centered at (cx, cy).
#[allow(clippy::too_many_arguments)]
fn cone(mesh: &mut MeshData, sides: usize, cx: f32, cy: f32, radius: f32, z_base: f32, z_apex: f32, color: [f32; 3], weight: f32) {
    let h = z_apex - z_base;
    for k in 0..sides {
        let a0 = std::f32::consts::TAU * k as f32 / sides as f32;
        let a1 = std::f32::consts::TAU * (k + 1) as f32 / sides as f32;
        let am = (a0 + a1) / 2.0;
        let norm = |a: f32| {
            let len = (h * h + radius * radius).sqrt();
            [h * a.cos() / len, h * a.sin() / len, radius / len]
        };
        let b0 = mesh.push([cx + radius * a0.cos(), cy + radius * a0.sin(), z_base], norm(a0), color, weight);
        let b1 = mesh.push([cx + radius * a1.cos(), cy + radius * a1.sin(), z_base], norm(a1), color, weight);
        let apex = mesh.push([cx, cy, z_apex], norm(am), color, weight);
        mesh.indices.extend([b0, b1, apex]);
    }
}

/// Tiles are hex columns reaching below the lowest valley (hills plus
/// mountains), so relief reads as stepped terrain with no gaps between
/// neighbors and the map edge shows as a solid block of land.
const TILE_HEIGHT: f32 =
    (crate::sim::terrain::RENDER_RELIEF + crate::sim::terrain::MACRO_RENDER) as f32 + 0.3;
/// Slight inset leaves visible seams between tiles.
const TILE_INSET: f32 = 0.96;

/// Just the hex top face (the roots view's glass ground).
pub fn tile_top_mesh() -> MeshData {
    let mut m = MeshData::new();
    hex_prism(&mut m, SIZE as f32 * TILE_INSET, -0.02, 0.0, [1.0; 3], 0.0);
    m
}

pub fn tile_mesh() -> MeshData {
    let mut m = MeshData::new();
    hex_prism(&mut m, SIZE as f32 * TILE_INSET, -TILE_HEIGHT, 0.0, [1.0; 3], 0.0);
    m
}

const TRUNK_COLOR: [f32; 3] = [0.36, 0.25, 0.15];
/// Cattail seed heads (brown).
const CATTAIL: [f32; 3] = [0.40, 0.26, 0.14];
/// Birch's chalk-white bark.
const BIRCH_BARK: [f32; 3] = [0.88, 0.86, 0.82];

/// One silhouette per species (canopies take the instance color, trunks are
/// baked brown): flat-top acacia, broad oak, spired pine, drooping willow.
pub fn tree_mesh_for(species: usize) -> MeshData {
    let mut m = MeshData::new();
    match species {
        1 => {
            // Oak: thick trunk, broad double crown.
            cylinder(&mut m, 12, 0.0, 0.0, 0.19, 0.0, 0.85, TRUNK_COLOR, 1.0);
            cone(&mut m, 18, 0.0, 0.0, 0.92, 0.55, 1.75, [1.0; 3], 0.0);
            cone(&mut m, 18, 0.0, 0.0, 0.68, 1.15, 2.25, [1.0; 3], 0.0);
        }
        2 => {
            // Pine: narrow stacked spire.
            cylinder(&mut m, 10, 0.0, 0.0, 0.10, 0.0, 0.55, TRUNK_COLOR, 1.0);
            cone(&mut m, 16, 0.0, 0.0, 0.52, 0.35, 1.35, [1.0; 3], 0.0);
            cone(&mut m, 16, 0.0, 0.0, 0.42, 1.05, 1.95, [1.0; 3], 0.0);
            cone(&mut m, 16, 0.0, 0.0, 0.30, 1.65, 2.65, [1.0; 3], 0.0);
        }
        3 => {
            // Willow: curtain skirt under a low dome.
            cylinder(&mut m, 10, 0.0, 0.0, 0.13, 0.0, 0.95, TRUNK_COLOR, 1.0);
            cylinder(&mut m, 18, 0.0, 0.0, 0.78, 0.55, 1.45, [1.0; 3], 0.0);
            cone(&mut m, 18, 0.0, 0.0, 0.85, 1.40, 2.05, [1.0; 3], 0.0);
        }
        4 => {
            // Spruce: a dense, narrow, dark spire of many tiers to the ground.
            cylinder(&mut m, 8, 0.0, 0.0, 0.09, 0.0, 0.35, TRUNK_COLOR, 1.0);
            for (k, r) in [0.62f32, 0.54, 0.46, 0.38, 0.30, 0.22].iter().enumerate() {
                let z = 0.2 + 0.42 * k as f32;
                cone(&mut m, 14, 0.0, 0.0, *r, z, z + 0.72, [1.0; 3], 0.0);
            }
        }
        5 => {
            // Birch: a slender white trunk and a small, airy, open crown.
            cylinder(&mut m, 8, 0.0, 0.0, 0.08, 0.0, 1.7, BIRCH_BARK, 1.0);
            for (x, y, z, r) in [(0.0f32, 0.0f32, 1.5f32, 0.42f32), (0.25, 0.1, 1.2, 0.3), (-0.22, -0.12, 1.3, 0.3), (0.05, -0.05, 1.9, 0.3)] {
                cone(&mut m, 12, x, y, r, z, z + 0.55, [1.0; 3], 0.0);
                cone(&mut m, 12, x, y, r, z, z - 0.25, [1.0; 3], 0.0);
            }
        }
        6 => {
            // Creosote: a low, rounded, multi-stemmed desert shrub.
            for (x, y) in [(0.0f32, 0.0f32), (0.28, 0.12), (-0.24, 0.18), (0.05, -0.28)] {
                cylinder(&mut m, 5, x, y, 0.03, 0.0, 0.35, TRUNK_COLOR, 1.0);
            }
            for (x, y, r) in [(0.0f32, 0.0f32, 0.42f32), (0.3, 0.15, 0.3), (-0.26, 0.2, 0.3), (0.06, -0.3, 0.28)] {
                cone(&mut m, 10, x, y, r, 0.25, 0.7, [1.0; 3], 0.0);
                cone(&mut m, 10, x, y, r, 0.25, 0.1, [1.0; 3], 0.0);
            }
        }
        _ => {
            // Acacia: tall bare trunk, flat umbrella crown.
            cylinder(&mut m, 10, 0.0, 0.0, 0.12, 0.0, 1.15, TRUNK_COLOR, 1.0);
            cone(&mut m, 18, 0.0, 0.0, 0.98, 1.05, 1.55, [1.0; 3], 0.0);
            cone(&mut m, 18, 0.0, 0.0, 0.45, 1.45, 1.85, [1.0; 3], 0.0);
        }
    }
    m
}

const STEM_COLOR: [f32; 3] = [0.87, 0.82, 0.70];

/// A cluster of three toadstools (cream stems, instance-colored caps) that
/// fruits on well-colonized dead wood.
pub fn mushroom_mesh() -> MeshData {
    let mut m = MeshData::new();
    for (cx, cy, s) in [(0.20f32, 0.10, 1.0f32), (-0.18, 0.18, 0.72), (-0.04, -0.22, 0.55)] {
        cylinder(&mut m, 8, cx, cy, 0.055 * s, 0.0, 0.20 * s, STEM_COLOR, 1.0);
        cone(&mut m, 10, cx, cy, 0.17 * s, 0.14 * s, 0.30 * s, [1.0; 3], 0.0);
    }
    m
}

/// One silhouette per grass functional type, colored per instance:
/// tall spiky bunchgrass tussocks, a low even sod mat, dark upright sedge
/// spears, and a small scatter of annual seedheads.
pub fn grass_mesh_for(kind: usize) -> MeshData {
    let mut m = MeshData::new();
    match kind {
        1 => {
            // Sod: a low continuous mat almost covering the tile.
            hex_prism(&mut m, 0.66, 0.0, 0.10, [1.0; 3], 0.0);
        }
        2 => {
            // Sedge: a clump of stiff upright spears.
            for (cx, cy, h) in [(0.0f32, 0.0f32, 0.46f32), (0.2, 0.12, 0.36), (-0.18, 0.14, 0.40), (0.02, -0.22, 0.34)] {
                cone(&mut m, 5, cx, cy, 0.10, 0.0, h, [1.0; 3], 0.0);
            }
        }
        4 => {
            // Reeds: a dense bed of tall thin spears with cattail heads.
            for (cx, cy, h) in [(0.0f32, 0.0f32, 0.95f32), (0.22, 0.1, 0.8), (-0.2, 0.16, 0.85), (0.1, -0.24, 0.75), (-0.14, -0.18, 0.9), (0.28, -0.1, 0.7)] {
                cone(&mut m, 4, cx, cy, 0.05, 0.0, h, [1.0; 3], 0.0);
                cylinder(&mut m, 5, cx, cy, 0.05, h * 0.62, h * 0.8, CATTAIL, 1.0);
            }
        }
        5 => {
            // Cactus: a columnar stem with a pair of upturned arms.
            cylinder(&mut m, 8, 0.0, 0.0, 0.12, 0.0, 0.75, [1.0; 3], 0.0);
            cone(&mut m, 8, 0.0, 0.0, 0.12, 0.75, 0.85, [1.0; 3], 0.0);
            for (x, z0, z1) in [(0.22f32, 0.3f32, 0.6f32), (-0.2, 0.4, 0.66)] {
                cylinder(&mut m, 6, x, 0.0, 0.07, z0, z1, [1.0; 3], 0.0);
                cone(&mut m, 6, x, 0.0, 0.07, z1, z1 + 0.06, [1.0; 3], 0.0);
            }
        }
        3 => {
            // Annual: a sparse scatter of small seedheads.
            for (cx, cy) in [(0.22f32, 0.05f32), (-0.2, 0.2), (-0.05, -0.25), (0.1, 0.3)] {
                cone(&mut m, 6, cx, cy, 0.09, 0.0, 0.22, [1.0; 3], 0.0);
            }
        }
        _ => {
            // Bunchgrass: separate tall tussocks with bare gaps between.
            for (cx, cy) in [(0.24f32, 0.0f32), (-0.14, 0.22), (-0.12, -0.22)] {
                cone(&mut m, 7, cx, cy, 0.20, 0.0, 0.42, [1.0; 3], 0.0);
            }
        }
    }
    m
}

/// Dark soil slab spanning the whole grid, sitting just under the tile tops
/// so the inset seams between tiles read as earth rather than sky.
pub fn base_mesh(grid: crate::sim::hex::Grid) -> MeshData {
    let (min_x, min_y, max_x, max_y) = grid.world_bounds();
    let pad = 1.5 * SIZE as f32;
    let (x0, y0) = (min_x as f32 - pad, min_y as f32 - pad);
    let (x1, y1) = (max_x as f32 + pad, max_y as f32 + pad);
    let z = -0.03;
    let color = [0.30, 0.24, 0.17];
    let mut m = MeshData::new();
    let a = m.push([x0, y0, z], [0.0, 0.0, 1.0], color, 1.0);
    let b = m.push([x1, y0, z], [0.0, 0.0, 1.0], color, 1.0);
    let c = m.push([x1, y1, z], [0.0, 0.0, 1.0], color, 1.0);
    let d = m.push([x0, y1, z], [0.0, 0.0, 1.0], color, 1.0);
    m.indices.extend([a, b, c, a, c, d]);
    m
}

const BOLT_GLOW: [f32; 3] = [2.1, 2.1, 1.6]; // over-bright: stays white under any shading

/// Ellipsoid (or its upper part, from latitude `lat_min`) centered at `c`
/// with radii `r`, colored by height from `low` (its base) to `high` (its
/// top) — how sunlit cloud tops and shadowed bases read.
#[allow(clippy::too_many_arguments)]
fn ellipsoid(mesh: &mut MeshData, c: [f32; 3], r: [f32; 3], lat_min: f32, low: [f32; 3], high: [f32; 3], weight: f32) {
    let (bands, sides) = (6usize, 12usize);
    let lat = |b: usize| lat_min + (std::f32::consts::FRAC_PI_2 - lat_min) * b as f32 / bands as f32;
    let mut ring = |phi: f32| -> Vec<u32> {
        (0..sides)
            .map(|k| {
                let th = std::f32::consts::TAU * k as f32 / sides as f32;
                let (cp, sp) = (phi.cos(), phi.sin());
                let p = [c[0] + r[0] * cp * th.cos(), c[1] + r[1] * cp * th.sin(), c[2] + r[2] * sp];
                let n = [cp * th.cos() / r[0], cp * th.sin() / r[1], sp / r[2]];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                let t = ((sp + 1.0) / 2.0).clamp(0.0, 1.0);
                let col = [low[0] + (high[0] - low[0]) * t, low[1] + (high[1] - low[1]) * t, low[2] + (high[2] - low[2]) * t];
                mesh.push(p, [n[0] / len, n[1] / len, n[2] / len], col, weight)
            })
            .collect()
    };
    let rings: Vec<Vec<u32>> = (0..=bands).map(|b| ring(lat(b))).collect();
    for b in 0..bands {
        for k in 0..sides {
            let k1 = (k + 1) % sides;
            let (a0, a1, b0, b1) = (rings[b][k], rings[b][k1], rings[b + 1][k], rings[b + 1][k1]);
            mesh.indices.extend([a0, a1, b1, a0, b1, b0]);
        }
    }
    // Close the base when the ellipsoid is cut off (flat cloud bases).
    if lat_min > -std::f32::consts::FRAC_PI_2 + 1e-3 {
        let z = c[2] + r[2] * lat_min.sin();
        let center = mesh.push([c[0], c[1], z], [0.0, 0.0, -1.0], low, weight);
        let base = &rings[0];
        for k in 0..sides {
            // Wound like the sides: counter-clockwise seen from outside.
            mesh.indices.extend([center, base[(k + 1) % sides], base[k]]);
        }
    }
}

/// Cloud palettes (baked, weight ≈ 1): sunlit tops, shadowed bases.
const CU_TOP: [f32; 3] = [1.0, 1.0, 1.0];
const CU_BASE: [f32; 3] = [0.66, 0.68, 0.73];
const CB_BASE: [f32; 3] = [0.26, 0.27, 0.33];
const CB_MID: [f32; 3] = [0.56, 0.57, 0.63];
const CB_ANVIL: [f32; 3] = [0.93, 0.93, 0.97];
const NS_TOP: [f32; 3] = [0.56, 0.57, 0.61];
const NS_BASE: [f32; 3] = [0.34, 0.35, 0.39];
const CI_WISP: [f32; 3] = [0.97, 0.98, 1.0];
const CLOUD_WEIGHT: f32 = 0.85;

/// Fair-weather cumulus: a field of separate small heaps, each as tall as
/// it is wide, flat-based at the condensation level with cauliflower
/// domes on top (~8 units across at scale 1; heaps ~2 across).
pub fn cumulus_mesh() -> MeshData {
    let mut m = MeshData::new();
    let heaps = [(0.0f32, 0.0f32, 1.25f32), (3.6, 1.4, 1.0), (-3.2, 2.2, 0.9), (1.4, -3.6, 1.1), (-2.4, -2.8, 0.8), (4.6, -2.4, 0.7)];
    for (x, y, s) in heaps {
        let flat = -0.35f32; // cut the lower half: flat bases
        ellipsoid(&mut m, [x, y, 0.4 * s], [1.3 * s, 1.3 * s, 0.9 * s], flat, CU_BASE, CU_TOP, CLOUD_WEIGHT);
        ellipsoid(&mut m, [x + 0.5 * s, y + 0.2 * s, 1.1 * s], [0.8 * s, 0.8 * s, 0.8 * s], flat, CU_BASE, CU_TOP, CLOUD_WEIGHT);
        ellipsoid(&mut m, [x - 0.45 * s, y - 0.3 * s, 1.0 * s], [0.7 * s, 0.7 * s, 0.7 * s], flat, CU_BASE, CU_TOP, CLOUD_WEIGHT);
        ellipsoid(&mut m, [x, y, 1.7 * s], [0.55 * s, 0.55 * s, 0.55 * s], flat, CU_BASE, CU_TOP, CLOUD_WEIGHT);
    }
    m
}

/// Cumulonimbus: the thunderstorm tower — a dark, wide, flat base, a
/// towering column bulging with updraft cells, and a flat anvil spread
/// out downwind (+x, turned to the wind) at the top of the troposphere.
pub fn cumulonimbus_mesh() -> MeshData {
    let mut m = MeshData::new();
    let flat = -0.3f32;
    ellipsoid(&mut m, [0.0, 0.0, 0.6], [4.6, 4.6, 1.3], flat, CB_BASE, CB_MID, CLOUD_WEIGHT);
    for (z, r) in [(2.4f32, 3.6f32), (4.4, 3.3), (6.4, 3.0), (8.2, 2.7)] {
        ellipsoid(&mut m, [0.2, 0.0, z], [r, r, 1.5], -1.2, CB_MID, CB_ANVIL, CLOUD_WEIGHT);
    }
    // The anvil: wide, flat, blown downwind, with an overshooting top.
    ellipsoid(&mut m, [2.2, 0.0, 10.0], [6.0, 4.4, 0.7], -1.3, CB_MID, CB_ANVIL, CLOUD_WEIGHT);
    ellipsoid(&mut m, [0.3, 0.0, 10.6], [1.6, 1.6, 0.9], -0.5, CB_ANVIL, CB_ANVIL, CLOUD_WEIGHT);
    m
}

/// Nimbostratus: a broad, featureless, dark rain layer — one thick slab
/// with a soft top and ragged scud (pannus) hanging beneath.
pub fn nimbostratus_mesh() -> MeshData {
    let mut m = MeshData::new();
    ellipsoid(&mut m, [0.0, 0.0, 1.0], [7.6, 7.6, 1.1], -0.6, NS_BASE, NS_TOP, CLOUD_WEIGHT);
    ellipsoid(&mut m, [2.4, 1.6, 1.4], [4.4, 4.0, 0.8], -0.6, NS_BASE, NS_TOP, CLOUD_WEIGHT);
    for (x, y) in [(-3.8f32, 1.2f32), (-1.0, -3.9), (2.6, -2.2), (3.9, 3.1), (-2.8, 4.2), (0.8, 1.0)] {
        ellipsoid(&mut m, [x, y, 0.15], [1.3, 0.9, 0.4], -1.4, NS_BASE, NS_BASE, CLOUD_WEIGHT);
    }
    m
}

/// Cirrus: high, thin, fibrous streaks ("mares' tails") drawn out along
/// the jet (+x, turned to the wind), each ending in a small hook where
/// falling ice crystals trail off.
pub fn cirrus_mesh() -> MeshData {
    let mut m = MeshData::new();
    for (y, x0, len) in [(-2.4f32, -5.0f32, 9.0f32), (-1.2, -4.0, 8.0), (0.0, -6.0, 11.0), (1.2, -3.4, 8.4), (2.4, -5.4, 9.6)] {
        let cx = x0 + len / 2.0;
        ellipsoid(&mut m, [cx, y, 0.0], [len / 2.0, 0.28, 0.1], -1.5, CI_WISP, CI_WISP, CLOUD_WEIGHT);
        // The hook: a short fallstreak curling back and down.
        ellipsoid(&mut m, [x0 + len - 0.2, y + 0.45, -0.25], [0.7, 0.2, 0.12], -1.5, CI_WISP, CI_WISP, CLOUD_WEIGHT);
    }
    m
}

fn quad(mesh: &mut MeshData, p: [[f32; 3]; 4], n: [f32; 3]) {
    let v: Vec<u32> = p.iter().map(|&q| mesh.push(q, n, [1.0; 3], 0.0)).collect();
    mesh.indices.extend([v[0], v[1], v[2], v[0], v[2], v[3]]);
}

/// A precipitation shaft: a unit-radius curtain from the cloud base
/// (z = 0) to z = −1 — an open cylinder plus three crossing sheets so it
/// reads as a volume of falling streaks from any side. The instance sets
/// its radius and length (see scene.wgsl vs_rain).
pub fn rain_mesh() -> MeshData {
    let mut m = MeshData::new();
    cylinder(&mut m, 16, 0.0, 0.0, 1.0, -1.0, 0.0, [1.0; 3], 0.0);
    for k in 0..3 {
        let a = std::f32::consts::PI * k as f32 / 3.0;
        let (c, s) = (a.cos(), a.sin());
        quad(&mut m, [[-c, -s, -1.0], [c, s, -1.0], [c, s, 0.0], [-c, -s, 0.0]], [-s, c, 0.0]);
    }
    m
}

/// A smoke plume rising from a fire and leaning downwind (+x, turned to
/// the wind): puffs growing and spreading as they rise.
pub fn smoke_mesh() -> MeshData {
    let mut m = MeshData::new();
    let grey_lo = [0.30, 0.29, 0.28];
    let grey_hi = [0.62, 0.60, 0.58];
    for (k, (x, z, r)) in [(0.0f32, 0.6f32, 0.45f32), (0.5, 1.6, 0.7), (1.3, 2.7, 1.0), (2.4, 3.8, 1.35), (3.8, 4.8, 1.7)].iter().enumerate() {
        let _ = k;
        ellipsoid(&mut m, [*x, 0.0, *z], [*r, *r, *r * 0.8], -1.5, grey_lo, grey_hi, 0.9);
    }
    m
}

/// An orographic cap cloud: one smooth, flattened lens sitting on a peak.
pub fn cap_mesh() -> MeshData {
    let mut m = MeshData::new();
    ellipsoid(&mut m, [0.0, 0.0, 0.0], [6.0, 4.5, 1.1], -1.5, [0.78, 0.79, 0.83], [1.0, 1.0, 1.0], 0.85);
    m
}

/// The sun, drawn over the landscape along the light direction: a sphere
/// whose size and glow the instance sets from the heat.
pub fn sun_mesh() -> MeshData {
    let mut m = MeshData::new();
    ellipsoid(&mut m, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], -std::f32::consts::FRAC_PI_2, [1.0; 3], [1.0; 3], 0.0);
    m
}

/// A tree's root system, below the tile surface (z ≤ 0): a taproot
/// plunging down (reach set by the instance scale), a shallow fibrous
/// root plate, and a few sinker roots. Instance-colored.
pub fn root_mesh() -> MeshData {
    let mut m = MeshData::new();
    cone(&mut m, 8, 0.0, 0.0, 0.16, 0.0, -3.2, [1.0; 3], 0.0);
    cone(&mut m, 12, 0.0, 0.0, 0.85, -0.05, -0.55, [1.0; 3], 0.0);
    for k in 0..5 {
        let a = std::f32::consts::TAU * k as f32 / 5.0 + 0.4;
        cone(&mut m, 6, 0.5 * a.cos(), 0.5 * a.sin(), 0.08, -0.2, -1.3, [1.0; 3], 0.0);
    }
    m
}

/// A lightning bolt: two thin offset segments (a kink) from ground to cloud
/// base, baked over-bright so shading can't dim it.
pub fn bolt_mesh() -> MeshData {
    // A jagged main channel (offset vertical segments) with forked
    // side branches, like a real cloud-to-ground stroke.
    let mut m = MeshData::new();
    let main = [(0.0f32, 0.0f32, 7.6f32, 6.2f32), (0.3, 0.2, 6.3, 4.6), (-0.1, 0.45, 4.7, 3.1), (0.35, 0.3, 3.2, 1.5), (0.1, 0.05, 1.6, 0.0)];
    for (x, y, z1, z0) in main {
        cylinder(&mut m, 4, x, y, 0.1, z0, z1 + 0.05, BOLT_GLOW, 1.0);
    }
    for (x, y, z1, z0) in [(-0.45f32, 0.1f32, 6.0f32, 5.0f32), (0.8, 0.5, 4.5, 3.6), (-0.5, 0.7, 3.0, 2.2), (0.9, 0.1, 2.4, 1.8)] {
        cylinder(&mut m, 4, x, y, 0.05, z0, z1, BOLT_GLOW, 1.0);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &MeshData) {
        assert!(!m.indices.is_empty());
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        for v in &m.vertices {
            let n = v.normal;
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "unnormalized normal {n:?}");
        }
    }

    #[test]
    fn meshes_are_well_formed() {
        check(&tile_mesh());
        for sp in 0..crate::sim::world::SPECIES_COUNT {
            check(&tree_mesh_for(sp));
        }
        for k in 0..crate::sim::world::GRASS_KIND_COUNT {
            check(&grass_mesh_for(k));
        }
        for m in [rain_mesh(), smoke_mesh(), cap_mesh()] {
            check(&m);
        }
        check(&base_mesh(crate::sim::hex::Grid::LEGACY));
        check(&mushroom_mesh());
        for m in [cumulus_mesh(), cumulonimbus_mesh(), nimbostratus_mesh(), cirrus_mesh(), sun_mesh(), root_mesh(), tile_top_mesh()] {
            check(&m);
        }
    }

    fn extent(m: &MeshData) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &m.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.pos[k]);
                hi[k] = hi[k].max(v.pos[k]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn cloud_genera_have_their_real_shapes() {
        let (cu_lo, cu_hi) = extent(&cumulus_mesh());
        let (cb_lo, cb_hi) = extent(&cumulonimbus_mesh());
        let (ns_lo, ns_hi) = extent(&nimbostratus_mesh());
        let (ci_lo, ci_hi) = extent(&cirrus_mesh());
        let height = |lo: [f32; 3], hi: [f32; 3]| hi[2] - lo[2];
        // The thunderhead towers over everything; cirrus and the rain
        // layer are thin; cumulus heaps are modest.
        assert!(height(cb_lo, cb_hi) > 3.0 * height(cu_lo, cu_hi));
        assert!(height(ns_lo, ns_hi) < 3.0 && height(ci_lo, ci_hi) < 1.0);
        // Cirrus streaks are long and thin along the wind (x) axis.
        let ci = extent(&cirrus_mesh());
        assert!((ci.1[0] - ci.0[0]) > 1.4 * (ci.1[1] - ci.0[1]));
        // Everything stays within the cloud shader's fade radius (8).
        for (lo, hi) in [(cu_lo, cu_hi), (cb_lo, cb_hi), (ns_lo, ns_hi), (ci_lo, ci_hi)] {
            assert!(lo[0] > -8.5 && hi[0] < 8.5 && lo[1] > -8.5 && hi[1] < 8.5);
        }
        // Tops lighter than bases (baked gradient).
        let cb = cumulonimbus_mesh();
        let top = cb.vertices.iter().max_by(|a, b| a.pos[2].total_cmp(&b.pos[2])).unwrap();
        let bottom = cb.vertices.iter().min_by(|a, b| a.pos[2].total_cmp(&b.pos[2])).unwrap();
        assert!(top.color[0] > bottom.color[0] + 0.3);
    }

    #[test]
    fn cloud_skins_wind_counter_clockwise_from_outside() {
        // The cloud pipeline culls back faces (counter-clockwise front):
        // every triangle's right-hand normal must point out of the volume,
        // along the vertices' outward normals.
        for m in [cumulus_mesh(), cumulonimbus_mesh(), nimbostratus_mesh(), cirrus_mesh()] {
            for t in m.indices.chunks(3) {
                let [a, b, c] = [m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]];
                let u = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1], b.pos[2] - a.pos[2]];
                let v = [c.pos[0] - a.pos[0], c.pos[1] - a.pos[1], c.pos[2] - a.pos[2]];
                let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if len < 1e-6 {
                    continue; // degenerate sliver at a pole
                }
                let out = [
                    a.normal[0] + b.normal[0] + c.normal[0],
                    a.normal[1] + b.normal[1] + c.normal[1],
                    a.normal[2] + b.normal[2] + c.normal[2],
                ];
                let d = n[0] * out[0] + n[1] * out[1] + n[2] * out[2];
                assert!(d > 0.0, "a cloud triangle faces inward");
            }
        }
    }

    #[test]
    fn roots_grow_down_from_the_surface() {
        let (lo, hi) = extent(&root_mesh());
        assert!(hi[2] <= 0.0 && lo[2] < -3.0);
    }

    #[test]
    fn silhouettes_are_actually_distinct() {
        let heights: Vec<f32> = (0..4)
            .map(|sp| {
                tree_mesh_for(sp).vertices.iter().map(|v| v.pos[2]).fold(0.0f32, f32::max)
            })
            .collect();
        assert!(heights[2] > heights[0], "the pine spire should top the acacia umbrella");
        let counts: Vec<usize> = (0..4).map(|sp| tree_mesh_for(sp).vertices.len()).collect();
        assert!(counts.windows(2).any(|w| w[0] != w[1]), "meshes must differ");
    }

    #[test]
    fn bolts_reach_from_ground_to_cloud_base() {
        let b = bolt_mesh();
        assert!(b.vertices.iter().any(|v| v.pos[2] <= 0.1));
        assert!(b.vertices.iter().any(|v| v.pos[2] >= 7.0));
        // Deliberately over-bright; excluded from the unit-normal mesh check
        // above because glow color, not normals, is what sells it.
        assert!(b.vertices.iter().all(|v| v.color[0] > 1.5 && v.color_weight == 1.0));
    }

    #[test]
    fn mushrooms_sit_above_the_ground() {
        assert!(mushroom_mesh().vertices.iter().all(|v| v.pos[2] >= 0.0));
    }

    #[test]
    fn vertex_layout_is_ten_floats() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 40);
        assert_eq!(std::mem::offset_of!(MeshVertex, normal), 12);
        assert_eq!(std::mem::offset_of!(MeshVertex, color), 24);
        assert_eq!(std::mem::offset_of!(MeshVertex, color_weight), 36);
    }

    #[test]
    fn tile_top_sits_on_the_ground_plane() {
        let m = tile_mesh();
        assert!(m.vertices.iter().all(|v| v.pos[2] <= 0.0));
        assert!(m.vertices.iter().any(|v| v.pos[2] == 0.0));
    }

    #[test]
    fn tree_and_grass_grow_upward() {
        for sp in 0..4 {
            assert!(tree_mesh_for(sp).vertices.iter().all(|v| v.pos[2] >= 0.0));
        }
        for k in 0..4 {
            assert!(grass_mesh_for(k).vertices.iter().all(|v| v.pos[2] >= 0.0));
        }
    }
}
