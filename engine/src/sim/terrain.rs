//! Static landscape generated from the seed. One fractal elevation field is
//! the source of truth; everything else is derived from it so the land stays
//! physically coherent:
//!
//! - **Catena water**: water collects downslope, so valley bottoms are wet
//!   and ridges dry (the old free-floating basins become emergent valleys).
//! - **Aspect heat**: slopes tilted toward the sun run hot and dry, slopes
//!   tilted away run cool and moist (0.5 = flat / neutral).
//! - **Soil depth**: thin and rocky on ridges and steep slopes, deep and rich
//!   in the flat bottoms (1 = deep).

use super::hex::Grid;
use super::rng::{self, Stream};

/// Physical height of the highest ridge (hexes are 1 unit across): sets the
/// slopes that drive aspect heat and soil depth.
pub const RELIEF: f64 = 3.0;
/// Rendered height of the highest ridge: the physical relief with a 2.5×
/// vertical exaggeration, as terrain maps use, so gentle hills read.
pub const RENDER_RELIEF: f64 = RELIEF * 2.5;
/// Sun direction shared with the renderer (normalized, toward the light).
pub const SUN_DIR: [f64; 3] = [0.36, -0.42, 0.83];

pub struct Terrain {
    /// Normalized elevation, 0 (lowest valley) .. 1 (highest ridge).
    pub elevation: Vec<f32>,
    /// Groundwater from the catena, 0 (dry ridge) .. 1 (saturated bottom).
    pub water: Vec<f32>,
    /// Insolation from slope aspect, 0 (cool shaded) .. 1 (hot sun-facing).
    pub heat: Vec<f32>,
    /// Soil depth, 0 (bare rock) .. 1 (deep valley soil).
    pub depth: Vec<f32>,
}

/// Smooth hash-lattice value noise in world coordinates.
fn value_noise(seed: u64, octave: u32, x: f64, y: f64) -> f64 {
    let xi = x.floor();
    let yi = y.floor();
    let (fx, fy) = (x - xi, y - yi);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let corner = |dx: f64, dy: f64| {
        let cx = (xi + dx) as i64;
        let cy = (yi + dy) as i64;
        let key = (cx.wrapping_mul(73_856_093) ^ cy.wrapping_mul(19_349_663)) as u64;
        rng::uniform01(seed ^ (octave as u64).wrapping_mul(0x9E37_79B9), key as u32, key >> 32, Stream::Terrain)
    };
    let a = corner(0.0, 0.0);
    let b = corner(1.0, 0.0);
    let c = corner(0.0, 1.0);
    let d = corner(1.0, 1.0);
    let (u, v) = (s(fx), s(fy));
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Raw (unnormalized) elevation at a world point: three octaves.
fn raw_elevation(seed: u64, x: f64, y: f64) -> f64 {
    let base = 34.0;
    0.60 * value_noise(seed, 0, x / base, y / base)
        + 0.28 * value_noise(seed, 1, x / (base / 2.2), y / (base / 2.2))
        + 0.12 * value_noise(seed, 2, x / (base / 5.0), y / (base / 5.0))
}

impl Terrain {
    pub fn generate(seed: u64, grid: Grid) -> Terrain {
        let pos: Vec<(f64, f64)> = (0..grid.cells()).map(|i| grid.center(i)).collect();
        let raw: Vec<f64> = pos.iter().map(|&(x, y)| raw_elevation(seed, x, y)).collect();
        let lo = raw.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = raw.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let norm = |v: f64| ((v - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
        let elevation: Vec<f32> = raw.iter().map(|&v| norm(v) as f32).collect();

        let mut water = Vec::with_capacity(grid.cells());
        let mut heat = Vec::with_capacity(grid.cells());
        let mut depth = Vec::with_capacity(grid.cells());
        let h = 0.75; // finite-difference step, world units
        for (i, &(x, y)) in pos.iter().enumerate() {
            let e = elevation[i] as f64;
            // Catena: bottoms saturate, ridges drain.
            water.push(((0.42 - e) / 0.30).clamp(0.0, 1.0) as f32);
            // Surface gradient in world units (height = norm × RELIEF).
            let ex = (norm(raw_elevation(seed, x + h, y)) - norm(raw_elevation(seed, x - h, y)))
                * RELIEF
                / (2.0 * h);
            let ey = (norm(raw_elevation(seed, x, y + h)) - norm(raw_elevation(seed, x, y - h)))
                * RELIEF
                / (2.0 * h);
            let nlen = (ex * ex + ey * ey + 1.0).sqrt();
            let n = [-ex / nlen, -ey / nlen, 1.0 / nlen];
            let facing = n[0] * SUN_DIR[0] + n[1] * SUN_DIR[1] + n[2] * SUN_DIR[2];
            // Flat ground faces the sun at SUN_DIR.z: that's neutral heat.
            heat.push((0.5 + 7.5 * (facing - SUN_DIR[2])).clamp(0.0, 1.0) as f32);
            let slope = (ex * ex + ey * ey).sqrt();
            let d = (1.0 - 1.4 * slope).clamp(0.0, 1.0) * (0.35 + 0.65 * (1.0 - e));
            depth.push(d.clamp(0.0, 1.0) as f32);
        }
        Terrain { elevation, water, heat, depth }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_is_deterministic_and_seeded() {
        let a = Terrain::generate(5, Grid::LEGACY);
        let b = Terrain::generate(5, Grid::LEGACY);
        let c = Terrain::generate(6, Grid::LEGACY);
        assert_eq!(a.elevation, b.elevation);
        assert_ne!(a.elevation, c.elevation);
    }

    #[test]
    fn layers_are_coherent_with_the_catena() {
        let t = Terrain::generate(11, Grid::LEGACY);
        let mean = |v: &[f32], pick: &dyn Fn(usize) -> bool| {
            let sel: Vec<f32> = (0..Grid::LEGACY.cells()).filter(|&i| pick(i)).map(|i| v[i]).collect();
            sel.iter().sum::<f32>() / sel.len().max(1) as f32
        };
        let low = |i: usize| t.elevation[i] < 0.25;
        let high = |i: usize| t.elevation[i] > 0.75;
        assert!(mean(&t.water, &low) > 0.3 && mean(&t.water, &high) == 0.0, "valleys wet, ridges dry");
        assert!(mean(&t.depth, &low) > mean(&t.depth, &high) + 0.2, "valley soils deeper than ridge soils");
        // Aspect: both hot and cool slopes exist, and flat-ish ground sits near neutral.
        assert!(t.heat.iter().any(|&h| h > 0.7) && t.heat.iter().any(|&h| h < 0.3));
    }
}
