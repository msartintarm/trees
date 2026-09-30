//! The wanderer: a walking controller over the display surface. Pure
//! kinematics — the world answers height, water, and obstacle queries
//! through [`Ground`], so the controller is tested against synthetic ground.
//!
//! Scale: the wanderer stands a bit over a fifth of a mature tree's height
//! (eye at 0.55 world units; trees ~2.5), so grass brushes the knees and
//! the canopy towers overhead. A tile is ~1.7 units across.

pub const EYE_HEIGHT: f64 = 0.55;
/// Body radius for collisions with trunks and houses.
pub const RADIUS: f64 = 0.16;
/// Walking and sprinting speeds (world units / s).
pub const WALK_SPEED: f64 = 1.3;
pub const SPRINT_SPEED: f64 = 3.4;
/// Wading slows you down.
const WADE_SLOW: f64 = 0.5;
/// How deep the feet sink when wading (so the eye drops into the river).
const WADE_SINK: f64 = 0.12;
const GRAVITY: f64 = 9.0;
const JUMP_SPEED: f64 = 3.0;
/// Steepest walkable grade (rise over run); cliffs block.
const MAX_GRADE: f64 = 1.6;
/// A ledge this high is simply stepped up (hex-column terraces, rocks);
/// anything taller has to be jumped onto.
pub const STEP_UP: f64 = 0.2;
/// Downhill snap: stay glued to the ground below this drop per frame.
const SNAP: f64 = 0.35;
/// How far ahead actions reach.
pub const REACH: f64 = 3.0;

pub mod keys {
    pub const FORWARD: u32 = 1;
    pub const BACK: u32 = 2;
    pub const LEFT: u32 = 4;
    pub const RIGHT: u32 = 8;
    pub const SPRINT: u32 = 16;
    pub const JUMP: u32 = 32;
    /// Rest: let years stream by (held).
    pub const REST: u32 = 64;
}

/// What the controller asks of the world.
pub trait Ground {
    fn height(&self, x: f64, y: f64) -> f64;
    /// Water depth at a point (0 on dry land).
    fn water(&self, x: f64, y: f64) -> f64;
    /// Solid cylinders near a point: (x, y, radius).
    fn obstacles(&self, x: f64, y: f64, out: &mut Vec<[f64; 3]>);
    /// Walkable area: (min x, min y, max x, max y).
    fn bounds(&self) -> (f64, f64, f64, f64);
}

#[derive(Clone, Debug)]
pub struct Player {
    /// Feet position.
    pub pos: [f64; 3],
    pub vz: f64,
    pub yaw: f64,
    pub pitch: f64,
    pub grounded: bool,
    /// Camera behind the shoulder instead of the eyes.
    pub third_person: bool,
    /// Horizontal speed achieved last step, as a multiple of walking speed
    /// (what drives the time flow).
    pub motion: f64,
    /// Currently wading.
    pub wading: bool,
}

impl Player {
    pub fn spawn(ground: &impl Ground, x: f64, y: f64, yaw: f64) -> Player {
        let z = ground.height(x, y);
        Player {
            pos: [x, y, z],
            vz: 0.0,
            yaw,
            pitch: -0.08,
            grounded: true,
            third_person: false,
            motion: 0.0,
            wading: false,
        }
    }

    pub fn look(&mut self, dyaw: f64, dpitch: f64) {
        self.yaw = (self.yaw + dyaw).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch + dpitch).clamp(-1.45, 1.45);
    }

    pub fn eye(&self) -> [f64; 3] {
        [self.pos[0], self.pos[1], self.pos[2] + EYE_HEIGHT]
    }

    /// Unit view direction.
    pub fn forward(&self) -> [f64; 3] {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        [cp * cy, cp * sy, sp]
    }

    fn floor(&self, ground: &impl Ground, x: f64, y: f64) -> f64 {
        let h = ground.height(x, y);
        if ground.water(x, y) > 0.02 {
            h - WADE_SINK
        } else {
            h
        }
    }

    /// One controller step: `keys` is a [`keys`] bitmask.
    pub fn step(&mut self, dt: f64, keys: u32, ground: &impl Ground) {
        if dt <= 0.0 {
            return;
        }
        let dt = dt.min(0.1);
        let (sy, cy) = self.yaw.sin_cos();
        let fwd = [cy, sy];
        let right = [sy, -cy];
        let mut dir = [0.0f64; 2];
        let on = |k: u32| keys & k != 0;
        if on(keys::FORWARD) {
            dir[0] += fwd[0];
            dir[1] += fwd[1];
        }
        if on(keys::BACK) {
            dir[0] -= fwd[0];
            dir[1] -= fwd[1];
        }
        if on(keys::RIGHT) {
            dir[0] += right[0];
            dir[1] += right[1];
        }
        if on(keys::LEFT) {
            dir[0] -= right[0];
            dir[1] -= right[1];
        }
        let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        self.wading = ground.water(self.pos[0], self.pos[1]) > 0.02;
        let start = [self.pos[0], self.pos[1]];
        if len > 1e-9 && !on(keys::REST) {
            let mut speed = if on(keys::SPRINT) { SPRINT_SPEED } else { WALK_SPEED };
            if self.wading {
                speed *= WADE_SLOW;
            }
            let step = [dir[0] / len * speed * dt, dir[1] / len * speed * dt];
            // Try the full move, then each axis alone (sliding along a
            // cliff or trunk instead of sticking to it).
            for cand in [step, [step[0], 0.0], [0.0, step[1]]] {
                if self.try_move(cand, ground) {
                    break;
                }
            }
        }
        // Vertical: jumps and gravity, glued to the ground walking downhill.
        let floor = self.floor(ground, self.pos[0], self.pos[1]);
        if self.grounded && on(keys::JUMP) {
            self.vz = JUMP_SPEED;
            self.grounded = false;
        }
        if self.grounded && floor > self.pos[2] {
            // Stepped up onto a ledge.
            self.pos[2] = floor;
            self.vz = 0.0;
        }
        if self.grounded {
            if self.pos[2] - floor < SNAP {
                self.pos[2] = floor;
                self.vz = 0.0;
            } else {
                self.grounded = false;
            }
        }
        if !self.grounded {
            self.vz -= GRAVITY * dt;
            self.pos[2] += self.vz * dt;
            if self.pos[2] <= floor {
                self.pos[2] = floor;
                self.vz = 0.0;
                self.grounded = true;
            }
        }
        let moved = ((self.pos[0] - start[0]).powi(2) + (self.pos[1] - start[1]).powi(2)).sqrt();
        self.motion = moved / dt / WALK_SPEED;
    }

    fn try_move(&mut self, d: [f64; 2], ground: &impl Ground) -> bool {
        let run = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if run < 1e-12 {
            return false;
        }
        let (x0, y0, x1, y1) = ground.bounds();
        let nx = (self.pos[0] + d[0]).clamp(x0, x1);
        let ny = (self.pos[1] + d[1]).clamp(y0, y1);
        // Ledges: walk up small steps and gentle slopes; in the air, you
        // clear whatever your feet are above.
        let rise = self.floor(ground, nx, ny) - self.pos[2];
        if self.grounded {
            // A step (a discontinuity) is fine up to STEP_UP; a slope is
            // judged just beyond the step, so a steep face can't be
            // climbed in tiny per-frame increments.
            let probe = 0.25;
            let (ux, uy) = (d[0] / run, d[1] / run);
            let ahead = ground.height(nx + ux * probe, ny + uy * probe) - ground.height(nx, ny);
            if rise > STEP_UP + MAX_GRADE * run || ahead / probe > MAX_GRADE {
                return false;
            }
        } else if rise > 0.02 {
            return false;
        }
        let mut obs = Vec::new();
        ground.obstacles(nx, ny, &mut obs);
        for o in &obs {
            let (dx, dy) = (nx - o[0], ny - o[1]);
            let min = o[2] + RADIUS;
            if dx * dx + dy * dy < min * min {
                // Walking away from an obstacle you're already inside
                // (spawned in a thicket) is allowed.
                let (ox, oy) = (self.pos[0] - o[0], self.pos[1] - o[1]);
                if ox * ox + oy * oy >= dx * dx + dy * dy {
                    return false;
                }
            }
        }
        self.pos[0] = nx;
        self.pos[1] = ny;
        true
    }

    /// Camera eye and forward direction: at the eyes, or over the shoulder
    /// (kept above the ground) in third person.
    pub fn camera(&self, ground: &impl Ground) -> ([f64; 3], [f64; 3]) {
        let f = self.forward();
        let eye = self.eye();
        if !self.third_person {
            return (eye, f);
        }
        // Swing the boom out until it would enter a trunk or a house, so
        // the camera pulls in instead of clipping through them.
        let mut back = 0.3;
        let mut obs = Vec::new();
        while back < 2.6 {
            let next = back + 0.1;
            let p = [eye[0] - f[0] * next, eye[1] - f[1] * next];
            obs.clear();
            ground.obstacles(p[0], p[1], &mut obs);
            if obs.iter().any(|o| (p[0] - o[0]).powi(2) + (p[1] - o[1]).powi(2) < (o[2] + 0.15).powi(2)) {
                break;
            }
            back = next;
        }
        let mut cam = [eye[0] - f[0] * back, eye[1] - f[1] * back, eye[2] - f[2] * back + 0.7 * back / 2.6];
        let floor = ground.height(cam[0], cam[1]) + 0.25;
        if cam[2] < floor {
            cam[2] = floor;
        }
        let aim = [eye[0] + f[0] * 4.0, eye[1] + f[1] * 4.0, eye[2] + f[2] * 4.0];
        let d = [aim[0] - cam[0], aim[1] - cam[1], aim[2] - cam[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
        (cam, [d[0] / l, d[1] / l, d[2] / l])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tilted plane with a river strip and one trunk.
    struct Test {
        grade: f64,
    }

    impl Ground for Test {
        fn height(&self, x: f64, _y: f64) -> f64 {
            if x > 20.0 {
                // A cliff.
                self.grade * 20.0 + (x - 20.0) * 5.0
            } else {
                self.grade * x
            }
        }
        fn water(&self, _x: f64, y: f64) -> f64 {
            if (10.0..12.0).contains(&y) {
                0.3
            } else {
                0.0
            }
        }
        fn obstacles(&self, _x: f64, _y: f64, out: &mut Vec<[f64; 3]>) {
            out.push([5.0, 5.0, 0.2]);
        }
        fn bounds(&self) -> (f64, f64, f64, f64) {
            (0.0, 0.0, 100.0, 100.0)
        }
    }

    fn walk(p: &mut Player, g: &impl Ground, keys: u32, secs: f64) {
        for _ in 0..(secs * 60.0) as usize {
            p.step(1.0 / 60.0, keys, g);
        }
    }

    #[test]
    fn walks_at_walking_speed_and_follows_the_ground() {
        let g = Test { grade: 0.2 };
        let mut p = Player::spawn(&g, 1.0, 1.0, 0.0);
        walk(&mut p, &g, keys::FORWARD, 2.0);
        assert!((p.pos[0] - (1.0 + 2.0 * WALK_SPEED)).abs() < 0.05, "x {}", p.pos[0]);
        assert!((p.pos[2] - g.height(p.pos[0], p.pos[1])).abs() < 1e-6, "on the ground");
        assert!((p.motion - 1.0).abs() < 0.01);
        walk(&mut p, &g, keys::FORWARD | keys::SPRINT, 1.0);
        assert!(p.motion > 2.5);
        walk(&mut p, &g, 0, 0.5);
        assert_eq!(p.motion, 0.0);
    }

    #[test]
    fn cliffs_and_trunks_block_and_you_slide_along_them() {
        let g = Test { grade: 0.0 };
        let mut p = Player::spawn(&g, 18.0, 30.0, 0.0);
        walk(&mut p, &g, keys::FORWARD, 5.0);
        assert!(p.pos[0] <= 20.0 + 0.1, "the cliff stops you at x {}", p.pos[0]);
        // Walk diagonally into the cliff: slide north along it.
        let y0 = p.pos[1];
        p.yaw = std::f64::consts::FRAC_PI_4;
        walk(&mut p, &g, keys::FORWARD, 1.0);
        assert!(p.pos[1] > y0 + 0.5, "slid along the cliff");
        // A trunk.
        let mut p = Player::spawn(&g, 3.0, 5.0, 0.0);
        walk(&mut p, &g, keys::FORWARD, 3.0);
        assert!(p.pos[0] < 5.0 - 0.2 - RADIUS + 0.05, "stopped at the trunk: {}", p.pos[0]);
    }

    #[test]
    fn rivers_slow_you_and_you_wade_in_them() {
        let g = Test { grade: 0.0 };
        let mut p = Player::spawn(&g, 3.0, 10.5, 0.0);
        walk(&mut p, &g, keys::FORWARD, 1.0);
        assert!(p.wading);
        assert!((p.motion - WADE_SLOW).abs() < 0.01);
        assert!(p.pos[2] < -0.05, "feet in the water");
    }

    /// Hex terraces: flat steps of a given height every 2 units along x.
    struct Steps {
        rise: f64,
    }

    impl Ground for Steps {
        fn height(&self, x: f64, _y: f64) -> f64 {
            (x / 2.0).floor().max(0.0) * self.rise
        }
        fn water(&self, _x: f64, _y: f64) -> f64 {
            0.0
        }
        fn obstacles(&self, _x: f64, _y: f64, _out: &mut Vec<[f64; 3]>) {}
        fn bounds(&self) -> (f64, f64, f64, f64) {
            (0.0, 0.0, 100.0, 100.0)
        }
    }

    #[test]
    fn low_steps_are_walked_and_high_ledges_need_a_jump() {
        let low = Steps { rise: 0.12 };
        let mut p = Player::spawn(&low, 1.0, 1.0, 0.0);
        walk(&mut p, &low, keys::FORWARD, 3.0);
        assert!(p.pos[0] > 4.5, "walked up the low steps: x {}", p.pos[0]);
        assert!((p.pos[2] - low.height(p.pos[0], 1.0)).abs() < 1e-9);
        let high = Steps { rise: 0.4 };
        let mut p = Player::spawn(&high, 1.0, 1.0, 0.0);
        walk(&mut p, &high, keys::FORWARD, 2.0);
        assert!(p.pos[0] < 2.0, "a 0.4 ledge blocks walking: x {}", p.pos[0]);
        // A running jump clears it.
        for _ in 0..40 {
            p.step(1.0 / 60.0, keys::FORWARD | keys::JUMP, &high);
        }
        walk(&mut p, &high, keys::FORWARD, 0.5);
        assert!(p.pos[0] > 2.0 && p.pos[2] >= 0.4 - 1e-9, "jumped onto the ledge: {:?}", p.pos);
    }

    #[test]
    fn jumping_lands_back_on_the_ground() {
        let g = Test { grade: 0.0 };
        let mut p = Player::spawn(&g, 3.0, 3.0, 0.0);
        p.step(1.0 / 60.0, keys::JUMP, &g);
        assert!(!p.grounded);
        walk(&mut p, &g, 0, 0.3);
        assert!(p.pos[2] > 0.1, "in the air");
        walk(&mut p, &g, 0, 1.0);
        assert!(p.grounded && p.pos[2].abs() < 1e-9);
    }

    #[test]
    fn the_third_person_camera_pulls_in_before_a_trunk() {
        let g = Test { grade: 0.0 };
        // Facing +x with the trunk at (5, 5) one unit behind.
        let mut p = Player::spawn(&g, 6.0, 5.0, 0.0);
        p.third_person = true;
        let (cam, _) = p.camera(&g);
        assert!(cam[0] > 5.0 + 0.2, "camera stopped short of the trunk: {:?}", cam);
    }

    #[test]
    fn the_third_person_camera_stays_above_ground() {
        let g = Test { grade: 0.0 };
        let mut p = Player::spawn(&g, 30.0, 30.0, 0.0);
        p.third_person = true;
        p.pitch = 1.2; // looking steeply up swings the camera low
        let (cam, f) = p.camera(&g);
        assert!(cam[2] >= 0.25 - 1e-9);
        assert!((f[0] * f[0] + f[1] * f[1] + f[2] * f[2] - 1.0).abs() < 1e-9);
    }
}
