//! Orbit perspective camera over the z = 0 ground plane (z-up world). Pure
//! math — shared by the wasm bridge (which owns the camera) and native tests.
//! Produces a WebGPU-clip-space (z ∈ [0, 1]) column-major view-projection.

use crate::sim::hex::Grid;

pub const FOV_Y: f64 = 45.0 * std::f64::consts::PI / 180.0;
pub const NEAR: f64 = 0.5;
pub const MIN_PITCH: f64 = 0.25;
pub const MAX_PITCH: f64 = 1.45;
pub const MIN_DISTANCE: f64 = 4.0;
pub const MAX_DISTANCE: f64 = 1500.0;

#[derive(Clone, Debug)]
pub struct Camera {
    pub target: [f64; 2],
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    /// Viewport size in backing-store pixels.
    pub viewport: [f64; 2],
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Camera {
    /// Camera fitted to the world grid, looking north from the south edge.
    pub fn fit_world(grid: Grid) -> Camera {
        let (min_x, min_y, max_x, max_y) = grid.world_bounds();
        let span = (max_x - min_x).max(max_y - min_y);
        let mut cam = Camera {
            target: [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0],
            yaw: -std::f64::consts::FRAC_PI_2,
            pitch: 0.9,
            distance: span * 0.9,
            viewport: [1280.0, 720.0],
        };
        cam.clamp();
        cam
    }

    fn clamp(&mut self) {
        self.pitch = self.pitch.clamp(MIN_PITCH, MAX_PITCH);
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    pub fn set_viewport(&mut self, w: f64, h: f64) {
        self.viewport = [w.max(1.0), h.max(1.0)];
    }

    pub fn orbit(&mut self, dyaw: f64, dpitch: f64) {
        self.yaw += dyaw;
        self.pitch += dpitch;
        self.clamp();
    }

    pub fn zoom(&mut self, factor: f64) {
        self.distance *= factor.abs().max(1e-9);
        self.clamp();
    }

    /// World units per backing pixel on the ground plane at the target depth.
    pub fn units_per_pixel(&self) -> f64 {
        2.0 * self.distance * (FOV_Y / 2.0).tan() / self.viewport[1]
    }

    /// Drag the scene with the cursor: `dx`/`dy` are backing-pixel deltas
    /// (screen y grows downward).
    pub fn pan_pixels(&mut self, dx: f64, dy: f64) {
        let k = self.units_per_pixel();
        let (sy, cy) = self.yaw.sin_cos();
        let right = [-sy, cy];
        let up_ground = [-cy, -sy];
        self.target[0] += -right[0] * dx * k + up_ground[0] * dy * k;
        self.target[1] += -right[1] * dx * k + up_ground[1] * dy * k;
    }

    pub fn eye(&self) -> [f64; 3] {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        [
            self.target[0] + self.distance * cp * cy,
            self.target[1] + self.distance * cp * sy,
            self.distance * sp,
        ]
    }

    fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let eye = self.eye();
        let fwd = normalize([self.target[0] - eye[0], self.target[1] - eye[1], -eye[2]]);
        let s = normalize(cross(fwd, [0.0, 0.0, 1.0]));
        let u = cross(s, fwd);
        (fwd, s, u)
    }

    pub fn far(&self) -> f64 {
        (self.distance * 4.0).max(100.0)
    }

    /// Column-major view-projection matrix (WebGPU clip space, z ∈ [0, 1]).
    pub fn view_proj(&self) -> [f32; 16] {
        let eye = self.eye();
        let (fwd, s, u) = self.basis();
        // Column-major look-at.
        #[rustfmt::skip]
        let view = [
            s[0], u[0], -fwd[0], 0.0,
            s[1], u[1], -fwd[1], 0.0,
            s[2], u[2], -fwd[2], 0.0,
            -dot(s, eye), -dot(u, eye), dot(fwd, eye), 1.0,
        ];
        let aspect = self.viewport[0] / self.viewport[1];
        let f = 1.0 / (FOV_Y / 2.0).tan();
        let (near, far) = (NEAR, self.far());
        #[rustfmt::skip]
        let proj = [
            f / aspect, 0.0, 0.0, 0.0,
            0.0, f, 0.0, 0.0,
            0.0, 0.0, far / (near - far), -1.0,
            0.0, 0.0, near * far / (near - far), 0.0,
        ];
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut v = 0.0;
                for k in 0..4 {
                    v += proj[k * 4 + row] * view[col * 4 + k];
                }
                out[col * 4 + row] = v as f32;
            }
        }
        out
    }

    /// Ray from the eye through a backing-pixel screen point.
    pub fn screen_ray(&self, bx: f64, by: f64) -> ([f64; 3], [f64; 3]) {
        let eye = self.eye();
        let (fwd, s, u) = self.basis();
        let aspect = self.viewport[0] / self.viewport[1];
        let tan = (FOV_Y / 2.0).tan();
        let ndc_x = 2.0 * bx / self.viewport[0] - 1.0;
        let ndc_y = 1.0 - 2.0 * by / self.viewport[1];
        let vx = ndc_x * tan * aspect;
        let vy = ndc_y * tan;
        let dir = normalize([
            s[0] * vx + u[0] * vy + fwd[0],
            s[1] * vx + u[1] * vy + fwd[1],
            s[2] * vx + u[2] * vy + fwd[2],
        ]);
        (eye, dir)
    }

    /// Intersection of a screen ray with the ground plane (z = 0), if the ray
    /// points toward it.
    pub fn pick_ground(&self, bx: f64, by: f64) -> Option<[f64; 2]> {
        let (eye, dir) = self.screen_ray(bx, by);
        if dir[2] >= -1e-9 {
            return None;
        }
        let t = -eye[2] / dir[2];
        Some([eye[0] + t * dir[0], eye[1] + t * dir[1]])
    }

    /// First point where a screen ray meets a height field (tile tops on
    /// raised terrain): march down from `max_height` in small steps and
    /// return the ground point under the first sample at or below the
    /// surface. `height` answers None off the map.
    pub fn pick_heightfield(
        &self,
        bx: f64,
        by: f64,
        max_height: f64,
        height: impl Fn(f64, f64) -> Option<f64>,
    ) -> Option<[f64; 2]> {
        let (eye, dir) = self.screen_ray(bx, by);
        if dir[2] >= -1e-9 {
            return None;
        }
        // Enter the relief slab at its top, leave it at z = 0.
        let t0 = ((eye[2] - max_height) / -dir[2]).max(0.0);
        let t1 = eye[2] / -dir[2];
        let step = 0.05;
        let mut t = t0;
        while t <= t1 + step {
            let p = [eye[0] + t * dir[0], eye[1] + t * dir[1], eye[2] + t * dir[2]];
            if let Some(h) = height(p[0], p[1]) {
                if p[2] <= h {
                    return Some([p[0], p[1]]);
                }
            }
            t += step;
        }
        None
    }
}

impl Camera {
    /// Ground rectangle the shadow map should cover: the region around the
    /// camera target that is in view (generously), clipped to the world.
    /// Zoomed out it is the whole map; zoomed in, the shadow map's texels
    /// concentrate where the viewer is looking, at any map size.
    pub fn shadow_bounds(&self, grid: Grid) -> (f64, f64, f64, f64) {
        let (min_x, min_y, max_x, max_y) = grid.world_bounds();
        let reach = self.distance * (1.2 + 1.5 * (MAX_PITCH - self.pitch));
        (
            (self.target[0] - reach).max(min_x),
            (self.target[1] - reach).max(min_y),
            (self.target[0] + reach).min(max_x),
            (self.target[1] + reach).min(max_y),
        )
    }
}

/// Orthographic sun view-projection covering a ground rectangle from z = 0
/// up to `top` (highest terrain) plus canopy height, for the shadow map.
/// `sun` points from the surface toward the light. Column-major, WebGPU
/// clip space (z in [0, 1]).
pub fn light_view_proj(sun: [f32; 3], bounds: (f64, f64, f64, f64), top: f64) -> [f32; 16] {
    let (min_x, min_y, max_x, max_y) = bounds;
    let c = [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0, 1.5 + top / 2.0];
    let sun64 = normalize([sun[0] as f64, sun[1] as f64, sun[2] as f64]);
    let dist = 90.0 + (max_x - min_x).max(max_y - min_y);
    let eye = [c[0] + sun64[0] * dist, c[1] + sun64[1] * dist, c[2] + sun64[2] * dist];
    let fwd = normalize([c[0] - eye[0], c[1] - eye[1], c[2] - eye[2]]);
    let s = normalize(cross(fwd, [0.0, 0.0, 1.0]));
    let u = cross(s, fwd);
    // View-space extents of the world box (ground to treetop height).
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for &x in &[min_x - 2.0, max_x + 2.0] {
        for &y in &[min_y - 2.0, max_y + 2.0] {
            for &z in &[-0.3, top + 5.0] {
                let d = [x - eye[0], y - eye[1], z - eye[2]];
                let v = [dot(s, d), dot(u, d), -dot(fwd, d)]; // v[2] = distance along -fwd
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
        }
    }
    let (l, r, b, t) = (lo[0], hi[0], lo[1], hi[1]);
    let (near, far) = (lo[2] - 1.0, hi[2] + 1.0);
    // Column-major ortho: view x→[-1,1], y→[-1,1], depth→[0,1].
    let mut m = [0.0f64; 16];
    m[0] = 2.0 / (r - l);
    m[5] = 2.0 / (t - b);
    m[10] = -1.0 / (far - near);
    m[12] = -(r + l) / (r - l);
    m[13] = -(t + b) / (t - b);
    m[14] = far / (far - near);
    m[15] = 1.0;
    // proj * view (view built from the same basis as Camera::view_proj).
    #[rustfmt::skip]
    let view = [
        s[0], u[0], -fwd[0], 0.0,
        s[1], u[1], -fwd[1], 0.0,
        s[2], u[2], -fwd[2], 0.0,
        -dot(s, eye), -dot(u, eye), dot(fwd, eye), 1.0,
    ];
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut v = 0.0;
            for k in 0..4 {
                v += m[k * 4 + row] * view[col * 4 + k];
            }
            out[col * 4 + row] = v as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(cam: &Camera, world: [f64; 3]) -> (f64, f64) {
        let m = cam.view_proj();
        let mut clip = [0.0f64; 4];
        for row in 0..4 {
            clip[row] = m[row] as f64 * world[0]
                + m[4 + row] as f64 * world[1]
                + m[8 + row] as f64 * world[2]
                + m[12 + row] as f64;
        }
        let (x, y) = (clip[0] / clip[3], clip[1] / clip[3]);
        ((x + 1.0) / 2.0 * cam.viewport[0], (1.0 - y) / 2.0 * cam.viewport[1])
    }

    #[test]
    fn heightfield_pick_hits_the_raised_tile_in_front() {
        let cam = Camera::fit_world(Grid::LEGACY);
        let (min_x, min_y, max_x, max_y) = Grid::LEGACY.world_bounds();
        let c = [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0];
        let target = Grid::LEGACY.pick(c[0], c[1]).unwrap();
        // Flat field: agrees with the plane pick.
        let (bx, by) = project(&cam, [c[0], c[1], 0.0]);
        let flat = cam.pick_heightfield(bx, by, 3.0, |x, y| Grid::LEGACY.pick(x, y).map(|_| 0.0)).unwrap();
        assert_eq!(Grid::LEGACY.pick(flat[0], flat[1]), Some(target));
        // Raise the target 2 units: clicking its TOP picks it, although the
        // same screen point projects onto a different tile at z = 0.
        let (bx, by) = project(&cam, [c[0], c[1], 2.0]);
        let plane = cam.pick_ground(bx, by).unwrap();
        assert_ne!(Grid::LEGACY.pick(plane[0], plane[1]), Some(target));
        let hit = cam
            .pick_heightfield(bx, by, 3.0, |x, y| {
                Grid::LEGACY.pick(x, y).map(|i| if i == target { 2.0 } else { 0.0 })
            })
            .unwrap();
        assert_eq!(Grid::LEGACY.pick(hit[0], hit[1]), Some(target));
    }

    #[test]
    fn fit_world_centers_the_grid() {
        let cam = Camera::fit_world(Grid::LEGACY);
        let (min_x, min_y, max_x, max_y) = Grid::LEGACY.world_bounds();
        let (bx, by) = project(&cam, [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0, 0.0]);
        assert!((bx - cam.viewport[0] / 2.0).abs() < 1.0);
        assert!((by - cam.viewport[1] / 2.0).abs() < 1.0);
    }

    #[test]
    fn pitch_and_distance_are_clamped() {
        let mut cam = Camera::fit_world(Grid::LEGACY);
        cam.orbit(0.0, 10.0);
        assert!((cam.pitch - MAX_PITCH).abs() < 1e-12);
        cam.orbit(0.0, -20.0);
        assert!((cam.pitch - MIN_PITCH).abs() < 1e-12);
        cam.zoom(1e-9);
        assert!((cam.distance - MIN_DISTANCE).abs() < 1e-9);
        cam.zoom(1e9);
        assert!((cam.distance - MAX_DISTANCE).abs() < 1e-9);
    }

    #[test]
    fn pick_ground_inverts_projection() {
        let mut cam = Camera::fit_world(Grid::LEGACY);
        cam.orbit(0.7, -0.2);
        cam.pan_pixels(37.0, -12.0);
        for &(wx, wy) in &[(0.0, 0.0), (30.0, 40.0), (110.0, 95.0), (55.0, 10.0)] {
            let (bx, by) = project(&cam, [wx, wy, 0.0]);
            let picked = cam.pick_ground(bx, by).expect("ray should hit the ground");
            // The projection runs through f32, so allow a small tolerance —
            // hexes are 1 unit across, so 1e-3 is far below a mispick.
            assert!((picked[0] - wx).abs() < 1e-3, "x: {} vs {wx}", picked[0]);
            assert!((picked[1] - wy).abs() < 1e-3, "y: {} vs {wy}", picked[1]);
        }
    }

    #[test]
    fn ray_pointing_at_the_sky_misses() {
        let cam = Camera::fit_world(Grid::LEGACY);
        assert_eq!(cam.pick_ground(cam.viewport[0] / 2.0, -1e6), None);
    }

    #[test]
    fn the_sun_matrix_covers_the_whole_world() {
        // Mountains up to 33 units: the box must still hold the treetops.
        let m = light_view_proj([0.36, -0.42, 0.83], Grid::LEGACY.world_bounds(), 33.0);
        let (min_x, min_y, max_x, max_y) = Grid::LEGACY.world_bounds();
        for &x in &[min_x, max_x] {
            for &y in &[min_y, max_y] {
                for &z in &[0.0, 4.5, 35.0] {
                    let mut clip = [0.0f64; 4];
                    for row in 0..4 {
                        clip[row] = m[row] as f64 * x
                            + m[4 + row] as f64 * y
                            + m[8 + row] as f64 * z
                            + m[12 + row] as f64;
                    }
                    assert!((clip[3] - 1.0).abs() < 1e-3, "ortho w must be 1");
                    assert!(clip[0].abs() <= 1.0 && clip[1].abs() <= 1.0, "xy outside the map");
                    assert!((0.0..=1.0).contains(&clip[2]), "depth {z} out of [0,1]: {}", clip[2]);
                }
            }
        }
    }

    #[test]
    fn pick_ground_hits_tile_centers() {
        let cam = Camera::fit_world(Grid::LEGACY);
        for i in [0usize, 100, 2048, Grid::LEGACY.cells() - 1] {
            let (q, r) = Grid::LEGACY.index_to_axial(i);
            let (x, y) = crate::sim::hex::axial_to_world(q, r);
            let (bx, by) = project(&cam, [x, y, 0.0]);
            let p = cam.pick_ground(bx, by).unwrap();
            assert_eq!(Grid::LEGACY.pick(p[0], p[1]), Some(i));
        }
    }
}
