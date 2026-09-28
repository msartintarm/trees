//! Axial hex math for a pointy-top grid stored as a width×height odd-r
//! offset rectangle. Cell index = `row * width + col`; axial coords `(q, r)`
//! are used for distance and neighborhood math; world positions put tile
//! centers on the z = 0 ground plane with hex size 1. The rectangle's size
//! is a runtime [`Grid`], chosen per world.

/// Center-to-corner hex radius in world units.
pub const SIZE: f64 = 1.0;

pub const SQRT3: f64 = 1.732_050_807_568_877_2;

/// Grid size limits (per side, in tiles).
pub const MIN_SIDE: u32 = 8;
pub const MAX_SIDE: u32 = 512;

/// The dimensions of a world's tile rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub width: i32,
    pub height: i32,
}

impl Grid {
    /// The default map: 256×256 (65,536 tiles).
    pub const DEFAULT: Grid = Grid { width: 256, height: 256 };
    /// The original 64×64 map the regime tests were calibrated on.
    pub const LEGACY: Grid = Grid { width: 64, height: 64 };

    pub fn new(width: u32, height: u32) -> Grid {
        Grid {
            width: width.clamp(MIN_SIDE, MAX_SIDE) as i32,
            height: height.clamp(MIN_SIDE, MAX_SIDE) as i32,
        }
    }

    #[inline]
    pub fn cells(&self) -> usize {
        (self.width * self.height) as usize
    }

    /// Axial coords of a cell index.
    #[inline]
    pub fn index_to_axial(&self, index: usize) -> (i32, i32) {
        let row = index as i32 / self.width;
        let col = index as i32 % self.width;
        offset_to_axial(col, row)
    }

    /// Cell index of axial coords, or None when outside the rectangle.
    #[inline]
    pub fn axial_to_index(&self, q: i32, r: i32) -> Option<usize> {
        let (col, row) = axial_to_offset(q, r);
        if col < 0 || col >= self.width || row < 0 || row >= self.height {
            return None;
        }
        Some((row * self.width + col) as usize)
    }

    /// Cell index of offset coords (col, row), or None outside.
    #[inline]
    pub fn offset_to_index(&self, col: i32, row: i32) -> Option<usize> {
        let (q, r) = offset_to_axial(col, row);
        self.axial_to_index(q, r)
    }

    /// World-plane center of a cell.
    #[inline]
    pub fn center(&self, index: usize) -> (f64, f64) {
        let (q, r) = self.index_to_axial(index);
        axial_to_world(q, r)
    }

    /// The middle tile (a handy anchor for tests and probes).
    pub fn middle(&self) -> usize {
        self.offset_to_index(self.width / 2, self.height / 2).unwrap()
    }

    /// World-plane point → containing cell index, or None outside the grid.
    pub fn pick(&self, x: f64, y: f64) -> Option<usize> {
        let qf = (SQRT3 / 3.0 * x - y / 3.0) / SIZE;
        let rf = (2.0 / 3.0 * y) / SIZE;
        let (q, r) = axial_round(qf, rf);
        self.axial_to_index(q, r)
    }

    /// Axis-aligned bounds of all tile centers: (min_x, min_y, max_x, max_y).
    /// Odd rows sit half a hex to the right (odd-r layout).
    pub fn world_bounds(&self) -> (f64, f64, f64, f64) {
        let shift = if self.height > 1 { 0.5 } else { 0.0 };
        (
            0.0,
            0.0,
            SIZE * SQRT3 * ((self.width - 1) as f64 + shift),
            SIZE * 1.5 * (self.height - 1) as f64,
        )
    }

    /// Indices of the cells whose centers could lie within `radius` of a
    /// world point: a row/column bounding box (callers still test the
    /// exact distance). Row-major order.
    pub fn cells_in_box(&self, center: [f64; 2], radius: f64) -> Vec<usize> {
        let row_lo = (((center[1] - radius) / (1.5 * SIZE)).floor() as i32).max(0);
        let row_hi = (((center[1] + radius) / (1.5 * SIZE)).ceil() as i32).min(self.height - 1);
        let col_w = SQRT3 * SIZE;
        let mut out = Vec::new();
        for row in row_lo..=row_hi {
            let shift = if row & 1 == 1 { 0.5 } else { 0.0 };
            let col_lo = (((center[0] - radius) / col_w - shift).floor() as i32).max(0);
            let col_hi = (((center[0] + radius) / col_w - shift).ceil() as i32).min(self.width - 1);
            for col in col_lo..=col_hi {
                out.push((row * self.width + col) as usize);
            }
        }
        out
    }

    /// Area relative to the legacy 64×64 map (rates that should scale with
    /// the map's linear size use its square root).
    pub fn area_ratio(&self) -> f64 {
        self.cells() as f64 / Grid::LEGACY.cells() as f64
    }
}

/// Offset (col, row) → axial (q, r), odd-r layout.
#[inline]
pub fn offset_to_axial(col: i32, row: i32) -> (i32, i32) {
    (col - (row - (row & 1)) / 2, row)
}

/// Axial (q, r) → offset (col, row), odd-r layout.
#[inline]
pub fn axial_to_offset(q: i32, r: i32) -> (i32, i32) {
    (q + (r - (r & 1)) / 2, r)
}

/// Hex (cube) distance between two axial coords.
#[inline]
pub fn distance(aq: i32, ar: i32, bq: i32, br: i32) -> i32 {
    let dq = aq - bq;
    let dr = ar - br;
    (dq.abs() + (dq + dr).abs() + dr.abs()) / 2
}

/// World-plane center of an axial coord (pointy-top, z up out of the ground).
#[inline]
pub fn axial_to_world(q: i32, r: i32) -> (f64, f64) {
    (SIZE * SQRT3 * (q as f64 + r as f64 / 2.0), SIZE * 1.5 * r as f64)
}

/// The six axial neighbor directions (hex distance exactly 1).
pub const NEIGHBORS: [(i32, i32); 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

/// All axial offsets `(dq, dr, dist)` with hex distance ≤ `radius`, center
/// included. Radius 1 yields 7 entries, radius 3 yields 37.
pub fn disk(radius: i32) -> Vec<(i32, i32, i32)> {
    let mut out = Vec::new();
    for dq in -radius..=radius {
        for dr in (-radius).max(-dq - radius)..=radius.min(-dq + radius) {
            out.push((dq, dr, distance(0, 0, dq, dr)));
        }
    }
    out
}

/// Round fractional axial coords to the containing hex (cube rounding).
pub fn axial_round(qf: f64, rf: f64) -> (i32, i32) {
    let sf = -qf - rf;
    let mut q = qf.round();
    let mut r = rf.round();
    let s = sf.round();
    let dq = (q - qf).abs();
    let dr = (r - rf).abs();
    let ds = (s - sf).abs();
    if dq > dr && dq > ds {
        q = -r - s;
    } else if dr > ds {
        r = -q - s;
    }
    (q as i32, r as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Grid = Grid::LEGACY;

    #[test]
    fn offset_axial_roundtrip_covers_the_grid() {
        for g in [G, Grid::new(37, 20), Grid::DEFAULT] {
            for i in 0..g.cells() {
                let (q, r) = g.index_to_axial(i);
                assert_eq!(g.axial_to_index(q, r), Some(i));
            }
        }
    }

    #[test]
    fn out_of_bounds_axial_is_rejected() {
        assert_eq!(G.axial_to_index(-1, 0), None);
        assert_eq!(G.axial_to_index(G.width, 0), None);
        let (q, r) = offset_to_axial(0, G.height);
        assert_eq!(G.axial_to_index(q, r), None);
    }

    #[test]
    fn disk_sizes_match_hex_arithmetic() {
        for (radius, n) in [(0, 1), (1, 7), (2, 19), (3, 37)] {
            assert_eq!(disk(radius).len(), n);
        }
    }

    #[test]
    fn distance_matches_neighbor_structure() {
        for (dq, dr) in NEIGHBORS {
            assert_eq!(distance(0, 0, dq, dr), 1);
        }
        assert_eq!(distance(0, 0, 3, -1), 3);
    }

    #[test]
    fn pick_recovers_every_tile_center() {
        for g in [G, Grid::new(50, 90)] {
            for i in 0..g.cells() {
                let (x, y) = g.center(i);
                assert_eq!(g.pick(x, y), Some(i));
            }
        }
    }

    #[test]
    fn pick_outside_the_grid_misses() {
        assert_eq!(G.pick(-5.0, -5.0), None);
        let (_, _, max_x, max_y) = G.world_bounds();
        assert_eq!(G.pick(max_x + 5.0, max_y + 5.0), None);
    }

    #[test]
    fn world_bounds_enclose_every_center_exactly() {
        for g in [G, Grid::new(9, 13), Grid::DEFAULT] {
            let (min_x, min_y, max_x, max_y) = g.world_bounds();
            let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for i in 0..g.cells() {
                let (x, y) = g.center(i);
                lo_x = lo_x.min(x);
                lo_y = lo_y.min(y);
                hi_x = hi_x.max(x);
                hi_y = hi_y.max(y);
            }
            let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
            assert!(close(min_x, lo_x) && close(min_y, lo_y) && close(max_x, hi_x) && close(max_y, hi_y));
        }
    }

    #[test]
    fn box_query_finds_every_cell_in_the_disk() {
        let g = Grid::new(40, 30);
        for (c, rad) in [([10.0, 12.0], 5.5), ([-3.0, 2.0], 7.0), ([60.0, 40.0], 9.0)] {
            let boxed = g.cells_in_box(c, rad);
            for i in 0..g.cells() {
                let (x, y) = g.center(i);
                if ((x - c[0]).powi(2) + (y - c[1]).powi(2)).sqrt() < rad {
                    assert!(boxed.contains(&i), "cell {i} inside the disk but missed");
                }
            }
        }
    }

    #[test]
    fn sides_are_clamped() {
        assert_eq!(Grid::new(1, 100_000), Grid { width: MIN_SIDE as i32, height: MAX_SIDE as i32 });
        assert_eq!(Grid::DEFAULT.area_ratio(), 16.0);
    }
}
