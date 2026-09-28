//! wasm-bindgen bridge: marshalling only — every rule lives in `sim`, every
//! matrix in `render`. Exposes one `Simulation` object owning the world, the
//! clock, and the camera; per-frame data crosses the boundary as copied-out
//! byte buffers (bytemuck-cast `Instance` arrays), the same model as the
//! sibling traffic engine.

use wasm_bindgen::prelude::*;

use crate::render::camera::Camera;
use crate::render::scene::{build_instances, FrameInstances};
use crate::sim::clock::{PlayState, SimClock};
use crate::sim::hex;
use crate::sim::world::{Brush, Params, Species, World};

/// Sim seconds per tick: 10 ticks/s at 1× speed.
const DT: f64 = 0.1;
/// Hard ceiling on catch-up ticks per frame; a tick is cheap (~4k cells), so
/// running the full backlog is faster than traffic's wall-clock budget dance.
/// Past this the clock drops the backlog and the sim degrades to slow-motion.
const MAX_CATCHUP_TICKS: u32 = 64;

#[wasm_bindgen]
pub struct Simulation {
    world: World,
    clock: SimClock,
    camera: Camera,
    seed: u32,
}

#[wasm_bindgen]
impl Simulation {
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u32) -> Simulation {
        Simulation {
            world: World::new(seed as u64),
            clock: SimClock::new(DT),
            camera: Camera::fit_world(),
            seed,
        }
    }

    /// Rebuild the world from a new seed; camera, clock speed, and tunables
    /// survive, the tick counter restarts.
    pub fn reseed(&mut self, seed: u32) {
        let speed = self.clock.speed();
        let playing = self.clock.state() == PlayState::Playing;
        self.world = World::with_params(seed as u64, self.world.params());
        self.clock = SimClock::new(DT);
        self.clock.set_speed(speed);
        if playing {
            self.clock.play();
        }
        self.seed = seed;
    }

    /// Convert real elapsed seconds into sim ticks and run them. Returns the
    /// number of ticks executed this frame.
    pub fn advance(&mut self, real_elapsed_secs: f64) -> u32 {
        let ran = self.clock.advance(real_elapsed_secs, MAX_CATCHUP_TICKS);
        let start_tick = self.clock.tick() - ran as u64;
        for t in 1..=ran as u64 {
            self.world.step(start_tick + t);
        }
        ran
    }

    pub fn play(&mut self) {
        self.clock.play();
    }

    pub fn pause(&mut self) {
        self.clock.pause();
    }

    pub fn is_playing(&self) -> bool {
        self.clock.state() == PlayState::Playing
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.clock.set_speed(speed);
    }

    pub fn selected_speed(&self) -> f64 {
        self.clock.speed()
    }

    pub fn single_step(&mut self) {
        let completed = self.clock.single_step();
        self.world.step(completed + 1);
    }

    pub fn tick(&self) -> f64 {
        self.clock.tick() as f64
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// `[bare, grass, trees, burning, storms]` counts (burning tiles are
    /// also counted under their current state).
    pub fn counts(&self) -> Vec<u32> {
        let mut c = self.world.counts().to_vec();
        c.push(self.world.burning_count());
        c.push(self.world.storms().len() as u32);
        c
    }

    /// Swap the ecology tunables (values are sanitized engine-side). Growth,
    /// shading, crowding, and fire fields apply from the next tick; the
    /// seeding fields apply on the next reseed.
    #[allow(clippy::too_many_arguments)]
    pub fn set_params(
        &mut self,
        grass_seed_p: f64,
        grass_clonal_p: f64,
        shade_strength: f64,
        tree_growth_p: f64,
        tree_range: i32,
        tree_maturity_age: u32,
        sod_factor: f64,
        crowding_p: f64,
        grass_mean_life: u32,
        tree_mean_life: u32,
        fire_ignition_p: f64,
        fire_spread_p: f64,
        nutrient_boost: f64,
        storm_rate: f64,
        storm_lightning_p: f64,
        climate_swing: f64,
        seed_tree_p: f64,
        seed_grass_p: f64,
    ) {
        self.world.set_params(Params {
            grass_seed_p,
            grass_clonal_p,
            shade_strength,
            tree_growth_p,
            tree_range,
            tree_maturity_age,
            sod_factor,
            crowding_p,
            grass_mean_life,
            tree_mean_life,
            fire_ignition_p,
            fire_spread_p,
            nutrient_boost,
            storm_rate,
            storm_lightning_p,
            climate_swing,
            seed_tree_p,
            seed_grass_p,
        });
    }

    // ---- camera ----

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.camera.set_viewport(w as f64, h as f64);
    }

    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.camera.orbit(dyaw as f64, dpitch as f64);
    }

    pub fn pan_pixels(&mut self, dx: f32, dy: f32) {
        self.camera.pan_pixels(dx as f64, dy as f64);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.camera.zoom(factor as f64);
    }

    pub fn reset_camera(&mut self) {
        let viewport = self.camera.viewport;
        self.camera = Camera::fit_world();
        self.camera.viewport = viewport;
    }

    /// Column-major view-projection matrix, 16 floats.
    pub fn view_proj(&self) -> Vec<f32> {
        self.camera.view_proj().to_vec()
    }

    /// Sub-tick blend factor for render interpolation.
    pub fn alpha(&self) -> f32 {
        self.clock.alpha() as f32
    }

    /// Camera eye position (for specular/rim lighting in the shader).
    pub fn eye(&self) -> Vec<f32> {
        let e = self.camera.eye();
        vec![e[0] as f32, e[1] as f32, e[2] as f32]
    }

    /// Climate readouts for the HUD, each 0..1.
    pub fn sun(&self) -> f32 {
        self.world.sun() as f32
    }

    pub fn moisture(&self) -> f32 {
        self.world.moisture() as f32
    }

    /// Global illumination for the renderer: 1.0 at the neutral climate.
    pub fn light_level(&self) -> f32 {
        (0.7 + 0.6 * self.world.sun()) as f32
    }

    // ---- interaction (bx/by are backing-store pixels) ----

    /// Tile index under a screen point, or -1 when the ray misses the grid.
    pub fn pick_tile(&self, bx: f32, by: f32) -> i32 {
        self.camera
            .pick_ground(bx as f64, by as f64)
            .and_then(|p| hex::pick(p[0], p[1]))
            .map_or(-1, |i| i as i32)
    }

    /// Paint the tile under a screen point (brush: 0 clear, 1 grass,
    /// 2 tree, 3 fire; `species` selects the tree variety for brush 2).
    /// Returns the painted tile index, or -1 on a miss.
    pub fn paint_at(&mut self, bx: f32, by: f32, brush: u8, species: u8) -> i32 {
        let index = self.pick_tile(bx, by);
        if index >= 0 {
            if let Some(b) = Brush::from_u8(brush) {
                self.world.paint_species(
                    index as usize,
                    b,
                    Species::from_u8(species),
                    self.clock.tick(),
                );
            }
        }
        index
    }

    // ---- per-frame instance buffers (copied out, bytemuck-cast) ----

    pub fn ground_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame().ground).to_vec()
    }

    pub fn ground_instance_count(&self) -> u32 {
        hex::CELLS as u32
    }

    /// Per-species tree stream (0 acacia, 1 oak, 2 pine, 3 willow).
    pub fn tree_instances(&self, species: u8) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame().trees[(species as usize).min(3)]).to_vec()
    }

    /// Instance counts include standing-dead husks, so they can exceed the
    /// live-population counts the HUD shows.
    pub fn tree_instance_count(&self, species: u8) -> u32 {
        self.frame().trees[(species as usize).min(3)].len() as u32
    }

    pub fn grass_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame().grass).to_vec()
    }

    pub fn grass_instance_count(&self) -> u32 {
        self.frame().grass.len() as u32
    }

    pub fn mushroom_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame().mushrooms).to_vec()
    }

    pub fn mushroom_instance_count(&self) -> u32 {
        self.frame().mushrooms.len() as u32
    }

    pub fn cloud_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame().clouds).to_vec()
    }

    pub fn cloud_instance_count(&self) -> u32 {
        self.frame().clouds.len() as u32
    }

    pub fn bolt_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame().bolts).to_vec()
    }

    pub fn bolt_instance_count(&self) -> u32 {
        self.frame().bolts.len() as u32
    }
}

impl Simulation {
    fn frame(&self) -> FrameInstances {
        build_instances(&self.world, self.clock.tick(), self.clock.alpha() as f32)
    }
}
