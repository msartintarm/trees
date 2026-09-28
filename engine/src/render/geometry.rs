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

/// Tiles are hex columns reaching below the lowest valley, so relief
/// reads as stepped terrain with no gaps between neighbors.
const TILE_HEIGHT: f32 = crate::sim::terrain::RENDER_RELIEF as f32 + 0.2;
/// Slight inset leaves visible seams between tiles.
const TILE_INSET: f32 = 0.96;

pub fn tile_mesh() -> MeshData {
    let mut m = MeshData::new();
    hex_prism(&mut m, SIZE as f32 * TILE_INSET, -TILE_HEIGHT, 0.0, [1.0; 3], 0.0);
    m
}

const TRUNK_COLOR: [f32; 3] = [0.36, 0.25, 0.15];

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

const CLOUD_GREY: [f32; 3] = [1.0, 1.0, 1.0]; // instance-colored
const BOLT_GLOW: [f32; 3] = [2.1, 2.1, 1.6]; // over-bright: stays white under any shading

/// A thundercloud: overlapping squashed hex puffs, ~8 world units across at
/// scale 1. Altitude comes from the instance position.
pub fn cloud_mesh() -> MeshData {
    let mut m = MeshData::new();
    for (cx, cy, rad, z0, z1) in [
        (0.0f32, 0.0f32, 5.0f32, 0.0f32, 1.7f32),
        (3.4, 1.6, 3.4, 0.4, 2.3),
        (-3.7, -0.9, 3.0, 0.3, 2.0),
        (0.6, -3.1, 2.3, 0.2, 1.5),
    ] {
        // Reuse the cylinder builder as a hex "puff" drum plus a cone cap.
        cylinder(&mut m, 6, cx, cy, rad, z0, z1, CLOUD_GREY, 0.0);
        cone(&mut m, 6, cx, cy, rad, z1, z1 + 0.8, CLOUD_GREY, 0.0);
        // Underside so the cloud is opaque from below.
        cone(&mut m, 6, cx, cy, rad, z0, z0 - 0.3, CLOUD_GREY, 0.0);
    }
    m
}

/// A flat cloud sheet (~8 units radius at scale 1): a wide thin slab with a
/// couple of offset panels so nimbostratus reads layered and cirrus wispy.
pub fn sheet_mesh() -> MeshData {
    let mut m = MeshData::new();
    for (cx, cy, rad, z0, z1) in [
        (0.0f32, 0.0f32, 7.6f32, 0.0f32, 0.45f32),
        (2.8, 1.2, 4.6, 0.35, 0.7),
        (-3.4, -1.6, 4.0, 0.25, 0.6),
    ] {
        cylinder(&mut m, 6, cx, cy, rad, z0, z1, [1.0; 3], 0.0);
        cone(&mut m, 6, cx, cy, rad, z1, z1 + 0.25, [1.0; 3], 0.0);
        cone(&mut m, 6, cx, cy, rad, z0, z0 - 0.15, [1.0; 3], 0.0);
    }
    m
}

/// A lightning bolt: two thin offset segments (a kink) from ground to cloud
/// base, baked over-bright so shading can't dim it.
pub fn bolt_mesh() -> MeshData {
    let mut m = MeshData::new();
    cylinder(&mut m, 4, 0.35, 0.22, 0.09, 0.0, 3.6, BOLT_GLOW, 1.0);
    cylinder(&mut m, 4, 0.0, 0.0, 0.11, 3.3, 7.6, BOLT_GLOW, 1.0);
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
        for sp in 0..4 {
            check(&tree_mesh_for(sp));
        }
        for k in 0..4 {
            check(&grass_mesh_for(k));
        }
        check(&base_mesh(crate::sim::hex::Grid::LEGACY));
        check(&mushroom_mesh());
        check(&cloud_mesh());
        check(&sheet_mesh());
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
