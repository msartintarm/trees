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
//!
//! Two map-scale layers sit above the local hills:
//!
//! - **Altitude**: mountain ranges at the scale of the whole map. Its range
//!   grows with the map (bigger landscapes span more elevation), and the
//!   world cools with height (a lapse rate) — climate zones.
//! - **Rain**: a regional precipitation field (wet vs dry regions), a bit
//!   wetter on the mountains (orographic lift).
//!
//! And water finds its way down: **drainage** routes every tile's runoff to
//! the map edge (priority-flood depression filling), accumulating upstream
//! area; tiles draining enough area become river channels.

use super::hex::Grid;
use super::rng::{self, Stream};

/// Physical height of the highest ridge (hexes are 1 unit across): sets the
/// slopes that drive aspect heat and soil depth.
pub const RELIEF: f64 = 3.0;
/// Rendered height of the highest ridge: the physical relief with a 2.5×
/// vertical exaggeration, as terrain maps use, so gentle hills read.
pub const RENDER_RELIEF: f64 = RELIEF * 2.5;
/// Rendered height of the tallest mountain at full altitude amplitude.
pub const MACRO_RENDER: f64 = 30.0;
/// Physical height of the mountains, for drainage routing.
pub const MACRO_PHYS: f64 = 12.0;
/// Map span (world units) at which altitude reaches its full 0..1 range;
/// smaller maps get proportionally gentler mountains.
pub const ALTITUDE_SPAN: f64 = 400.0;

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
    /// Map-scale altitude, 0 .. `altitude_amp` (≤ 1).
    pub altitude: Vec<f32>,
    /// Altitude range of this map (grows with map size, capped at 1).
    pub altitude_amp: f32,
    /// Regional rain anomaly, about −amp/2 .. +amp/2 (0 = average).
    pub rain: Vec<f32>,
    /// Upstream drainage area in tiles (rain-weighted), ≥ 1.
    pub flow: Vec<f32>,
    /// Map-scale climate gradients (biomes): a north–south position, 0
    /// (warm south) .. 1 (cold north), and a coast-to-interior position,
    /// 0 (wet coast) .. 1 (dry interior); `gradient_amp` scales them with
    /// map size like the altitude range.
    pub latitude: Vec<f32>,
    pub interior: Vec<f32>,
    pub gradient_amp: f32,
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

/// Two-octave map-scale noise at wavelength `wl` (world units), 0..1-ish.
fn macro_noise(seed: u64, salt: u32, x: f64, y: f64, wl: f64) -> f64 {
    0.72 * value_noise(seed ^ 0x5A5A_0000, salt, x / wl, y / wl)
        + 0.28 * value_noise(seed ^ 0x5A5A_0000, salt + 1, x / (wl / 2.3), y / (wl / 2.3))
}

fn normalize(v: &[f64]) -> Vec<f64> {
    let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    v.iter().map(|&x| ((x - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0)).collect()
}

/// Priority-flood drainage (Barnes et al. 2014): grow inward from the map
/// edge in order of (depression-filled) height; each tile drains to the
/// tile it was reached from, so every tile has a path to the edge and
/// pits fill into flat outlets. Returns the rain-weighted upstream area.
pub fn drainage(grid: Grid, height: &[f64], rain: &[f64]) -> Vec<f32> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let n = grid.cells();
    let mut visited = vec![false; n];
    let mut receiver = vec![usize::MAX; n];
    let mut order = Vec::with_capacity(n);
    let mut filled = vec![0.0f64; n];
    let mut heap = BinaryHeap::new();
    // Heights are ≥ 0, so their bit patterns order like the floats.
    let key = |h: f64| h.max(0.0).to_bits();
    for i in 0..n {
        let (q, r) = grid.index_to_axial(i);
        let edge = super::hex::NEIGHBORS.iter().any(|&(dq, dr)| grid.axial_to_index(q + dq, r + dr).is_none());
        if edge {
            visited[i] = true;
            filled[i] = height[i];
            heap.push(Reverse((key(height[i]), i)));
        }
    }
    while let Some(Reverse((_, c))) = heap.pop() {
        order.push(c);
        let (q, r) = grid.index_to_axial(c);
        for (dq, dr) in super::hex::NEIGHBORS {
            if let Some(j) = grid.axial_to_index(q + dq, r + dr) {
                if !visited[j] {
                    visited[j] = true;
                    filled[j] = height[j].max(filled[c] + 1e-6);
                    receiver[j] = c;
                    heap.push(Reverse((key(filled[j]), j)));
                }
            }
        }
    }
    let mut acc: Vec<f64> = (0..n).map(|i| (1.0 + 0.8 * rain[i]).max(0.2)).collect();
    for &c in order.iter().rev() {
        if receiver[c] != usize::MAX {
            acc[receiver[c]] += acc[c];
        }
    }
    acc.iter().map(|&a| a as f32).collect()
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
        // Map-scale layers: ranges ~1–2 per map at any size, with an
        // altitude range that grows with the landscape.
        let (_, _, max_x, max_y) = grid.world_bounds();
        let span = max_x.max(max_y);
        let wl = (span * 0.8).max(120.0);
        let amp = (span / ALTITUDE_SPAN).min(1.0);
        let alt_raw: Vec<f64> = pos.iter().map(|&(x, y)| macro_noise(seed, 40, x, y, wl)).collect();
        let altitude: Vec<f64> = normalize(&alt_raw).iter().map(|a| a * amp).collect();
        let rain_raw: Vec<f64> = pos.iter().map(|&(x, y)| macro_noise(seed, 50, x, y, wl * 0.9)).collect();
        let rain_n = normalize(&rain_raw);
        // Orographic lift: mountains wring out a little more rain.
        let rain: Vec<f64> = (0..pos.len())
            .map(|i| amp * (0.8 * (rain_n[i] - 0.5) + 0.2 * (altitude[i] / amp.max(1e-9) - 0.5)))
            .collect();
        let height: Vec<f64> =
            (0..pos.len()).map(|i| elevation[i] as f64 * RELIEF + altitude[i] * MACRO_PHYS).collect();
        let flow = drainage(grid, &height, &rain);
        // The coast lies on a seed-chosen side (east or west).
        let coast_east = rng::uniform01(seed, 7, 0, Stream::Terrain) < 0.5;
        let latitude: Vec<f32> = pos.iter().map(|&(_, y)| (y / max_y.max(1e-9)) as f32).collect();
        let interior: Vec<f32> = pos
            .iter()
            .map(|&(x, _)| {
                let f = (x / max_x.max(1e-9)) as f32;
                if coast_east { 1.0 - f } else { f }
            })
            .collect();
        Terrain {
            latitude,
            interior,
            gradient_amp: amp as f32,
            elevation,
            water,
            heat,
            depth,
            altitude: altitude.iter().map(|&a| a as f32).collect(),
            altitude_amp: amp as f32,
            rain: rain.iter().map(|&r| r as f32).collect(),
            flow,
        }
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
    fn every_tile_drains_and_flow_accumulates_downstream() {
        let g = Grid::new(96, 96);
        let t = Terrain::generate(3, g);
        // Total rain-weighted area arrives at the edge: the largest flows
        // are big rivers, and no tile has less than its own rain.
        assert!(t.flow.iter().all(|&f| f >= 0.2));
        let max = t.flow.iter().cloned().fold(0.0f32, f32::max);
        assert!(max > 300.0, "some tile should drain a real catchment, max {max}");
        // A flat field drains too (depression filling gives it outlets).
        let flat = drainage(g, &vec![1.0; g.cells()], &vec![0.0; g.cells()]);
        let total: f64 = flat.iter().map(|&f| f as f64).sum();
        assert!(total >= g.cells() as f64);
    }

    #[test]
    fn mountains_grow_with_the_map() {
        let small = Terrain::generate(4, Grid::LEGACY);
        let big = Terrain::generate(4, Grid::DEFAULT);
        assert!(small.altitude_amp < 0.4 && big.altitude_amp >= 0.99);
        let hi = big.altitude.iter().cloned().fold(0.0f32, f32::max);
        assert!(hi > 0.95);
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
