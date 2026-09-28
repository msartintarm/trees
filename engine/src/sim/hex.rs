//! Axial hex math for a pointy-top grid stored as a WIDTH×HEIGHT odd-r offset
//! rectangle. Cell index = `row * WIDTH + col`; axial coords `(q, r)` are used
//! for distance and neighborhood math; world positions put tile centers on the
//! z = 0 ground plane with hex size 1.

pub const WIDTH: i32 = 64;
pub const HEIGHT: i32 = 64;
pub const CELLS: usize = (WIDTH * HEIGHT) as usize;

/// Center-to-corner hex radius in world units.
pub const SIZE: f64 = 1.0;

pub const SQRT3: f64 = 1.732_050_807_568_877_2;

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

/// Axial coords of a cell index.
#[inline]
pub fn index_to_axial(index: usize) -> (i32, i32) {
    let row = index as i32 / WIDTH;
    let col = index as i32 % WIDTH;
    offset_to_axial(col, row)
}

/// Cell index of axial coords, or None when outside the world rectangle.
#[inline]
pub fn axial_to_index(q: i32, r: i32) -> Option<usize> {
    let (col, row) = axial_to_offset(q, r);
    if col < 0 || col >= WIDTH || row < 0 || row >= HEIGHT {
        return None;
    }
    Some((row * WIDTH + col) as usize)
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

/// World-plane point → containing cell index, or None outside the grid.
pub fn pick(x: f64, y: f64) -> Option<usize> {
    let qf = (SQRT3 / 3.0 * x - y / 3.0) / SIZE;
    let rf = (2.0 / 3.0 * y) / SIZE;
    let (q, r) = axial_round(qf, rf);
    axial_to_index(q, r)
}

/// Axis-aligned bounds of all tile centers: (min_x, min_y, max_x, max_y).
pub fn world_bounds() -> (f64, f64, f64, f64) {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..CELLS {
        let (q, r) = index_to_axial(i);
        let (x, y) = axial_to_world(q, r);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_axial_roundtrip_covers_the_grid() {
        for i in 0..CELLS {
            let (q, r) = index_to_axial(i);
            assert_eq!(axial_to_index(q, r), Some(i));
        }
    }

    #[test]
    fn out_of_bounds_axial_is_rejected() {
        assert_eq!(axial_to_index(-1, 0), None);
        assert_eq!(axial_to_index(WIDTH, 0), None);
        let (q, r) = offset_to_axial(0, HEIGHT);
        assert_eq!(axial_to_index(q, r), None);
    }

    #[test]
    fn disk_sizes_match_hex_arithmetic() {
        // 1 + 3·radius·(radius+1) cells in a hex disk.
        assert_eq!(disk(1).len(), 7);
        assert_eq!(disk(3).len(), 37);
        assert!(disk(3).iter().all(|&(dq, dr, d)| d == distance(0, 0, dq, dr) && d <= 3));
    }

    #[test]
    fn distance_matches_neighbor_structure() {
        let neighbors = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];
        for (dq, dr) in neighbors {
            assert_eq!(distance(0, 0, dq, dr), 1);
        }
        assert_eq!(distance(0, 0, 3, -1), 3);
        assert_eq!(distance(2, -1, 2, -1), 0);
    }

    #[test]
    fn pick_recovers_every_tile_center() {
        for i in 0..CELLS {
            let (q, r) = index_to_axial(i);
            let (x, y) = axial_to_world(q, r);
            assert_eq!(pick(x, y), Some(i), "center of cell {i} mispicked");
            // A point nudged well inside the hex still picks the same cell.
            assert_eq!(pick(x + 0.4 * SIZE, y + 0.4 * SIZE), Some(i));
        }
    }

    #[test]
    fn pick_outside_the_grid_misses() {
        let (min_x, min_y, max_x, max_y) = world_bounds();
        assert_eq!(pick(min_x - 5.0, min_y), None);
        assert_eq!(pick(max_x + 5.0, max_y + 5.0), None);
    }

    #[test]
    fn world_bounds_are_sane() {
        let (min_x, min_y, max_x, max_y) = world_bounds();
        assert!(min_x < 1.0 && min_y <= 0.0);
        assert!(max_x > 100.0 && max_y > 90.0, "64×64 grid should span ~110×95 units");
    }
}
