//! wasm-bindgen bridge: marshalling only — every rule lives in `sim`, every
//! matrix in `render`. Exposes one `Simulation` object owning the world, the
//! clock, and the camera; per-frame data crosses the boundary as copied-out
//! byte buffers (bytemuck-cast `Instance` arrays), the same model as the
//! sibling traffic engine.

use wasm_bindgen::prelude::*;

use crate::render::camera::{light_view_proj, Camera};
use crate::render::scene::{build_instances, FrameInstances};
use crate::sim::clock::{PlayState, SimClock};
use crate::sim::terrain::{RENDER_RELIEF, SUN_DIR};
use crate::sim::world::{Brush, GrassKind, Params, Species, World, GRASS_KIND_COUNT};

/// Sim seconds per tick: 10 ticks/s at 1× speed.
const DT: f64 = 0.1;
/// Hard ceiling on catch-up ticks per frame; a tick is cheap (~4k cells), so
/// running the full backlog is faster than traffic's wall-clock budget dance.
/// Past this the clock drops the backlog and the sim degrades to slow-motion.
const MAX_CATCHUP_TICKS: u32 = 64;

/// Wall-clock milliseconds per frame the sim may spend on catch-up ticks;
/// past it the clock drops the backlog (big maps at high speed degrade to
/// slow motion instead of freezing the page).
const FRAME_BUDGET_MS: f64 = 12.0;

/// Monotonic wall-clock milliseconds (worker or window `performance`);
/// 0.0 if unavailable, which disables the budget.
fn now_ms() -> f64 {
    use wasm_bindgen::JsCast;
    let global = js_sys::global();
    if let Some(perf) = global.dyn_ref::<web_sys::WorkerGlobalScope>().and_then(|s| s.performance()) {
        return perf.now();
    }
    if let Some(perf) = global.dyn_ref::<web_sys::Window>().and_then(|w| w.performance()) {
        return perf.now();
    }
    0.0
}

#[wasm_bindgen]
pub struct Simulation {
    world: World,
    clock: SimClock,
    camera: Camera,
    seed: u32,
    /// Instance streams built once per frame by `prepare_frame` (the
    /// per-stream getters only copy out of it).
    frame: FrameInstances,
    /// Smoothed wall-clock cost of one tick, ms.
    tick_ms: f64,
    /// Smoothed achieved speed (sim seconds per real second).
    actual_speed: f64,
}

#[wasm_bindgen]
impl Simulation {
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u32) -> Simulation {
        let world = World::new(seed as u64);
        let camera = Camera::fit_world(world.grid());
        Simulation {
            world,
            clock: SimClock::new(DT),
            camera,
            seed,
            frame: FrameInstances::default(),
            tick_ms: 1.0,
            actual_speed: 1.0,
        }
    }

    /// Rebuild the world from a new seed; camera, clock speed, and tunables
    /// survive, the tick counter restarts.
    pub fn reseed(&mut self, seed: u32) {
        let speed = self.clock.speed();
        let playing = self.clock.state() == PlayState::Playing;
        let old_grid = self.world.grid();
        self.world = World::with_params(seed as u64, self.world.params());
        if self.world.grid() != old_grid {
            // A new map size: re-frame the camera on it.
            let viewport = self.camera.viewport;
            self.camera = Camera::fit_world(self.world.grid());
            self.camera.viewport = viewport;
        }
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
        // As many ticks as fit the frame budget at the measured tick cost:
        // at least FRAME_BUDGET_MS, or 60% of a slow frame's wall time so
        // a slow renderer doesn't also starve the simulation.
        let budget = FRAME_BUDGET_MS.max(0.6 * real_elapsed_secs * 1000.0);
        let cap = ((budget / self.tick_ms.max(0.01)) as u32).clamp(1, MAX_CATCHUP_TICKS);
        let ran = self.clock.advance(real_elapsed_secs, cap);
        let start_tick = self.clock.tick() - ran as u64;
        let t0 = now_ms();
        for t in 1..=ran as u64 {
            self.world.step(start_tick + t);
        }
        if ran > 0 && t0 > 0.0 {
            let per = (now_ms() - t0) / ran as f64;
            self.tick_ms = 0.8 * self.tick_ms + 0.2 * per;
        }
        if real_elapsed_secs > 0.0 && self.is_playing() {
            let speed = ran as f64 * DT / real_elapsed_secs;
            self.actual_speed = 0.95 * self.actual_speed + 0.05 * speed;
        }
        ran
    }

    /// Achieved sim speed (1.0 = real time), smoothed — below the selected
    /// speed when a large map can't keep up.
    pub fn actual_speed(&self) -> f64 {
        self.actual_speed
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

    /// `[bare, grass, trees, burning, storms, infested]` counts (burning
    /// tiles are also counted under their current state).
    pub fn counts(&self) -> Vec<u32> {
        let mut c = self.world.counts().to_vec();
        c.push(self.world.burning_count());
        // ⛈ counts only the rain-bearing genera; fair-weather puffs and
        // cirrus don't warrant the badge.
        c.push(
            self.world
                .storms()
                .iter()
                .filter(|s| s.kind.traits().rains)
                .count() as u32,
        );
        c.push(self.world.infested_count());
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
        water_table: f64,
        mutation_rate: f64,
        terrain: f64,
        grass_niches: f64,
        pest_strength: f64,
        browse: f64,
        competition: f64,
        seed_tree_p: f64,
        seed_grass_p: f64,
        width: u32,
        height: u32,
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
            water_table,
            mutation_rate,
            terrain,
            grass_niches,
            pest_strength,
            browse,
            competition,
            seed_tree_p,
            seed_grass_p,
            width,
            height,
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
        self.camera = Camera::fit_world(self.world.grid());
        self.camera.viewport = viewport;
    }

    /// Column-major view-projection matrix, 16 floats.
    pub fn view_proj(&self) -> Vec<f32> {
        self.camera.view_proj().to_vec()
    }

    /// The sun's shadow-map view-projection, fitted to what the camera is
    /// looking at (16 floats, column-major).
    pub fn light_view_proj(&self) -> Vec<f32> {
        let sun = SUN_DIR.map(|v| v as f32);
        light_view_proj(sun, self.camera.shadow_bounds(self.world.grid())).to_vec()
    }

    /// Current map size in tiles (changes only on reseed).
    pub fn grid_width(&self) -> u32 {
        self.world.grid().width as u32
    }

    pub fn grid_height(&self) -> u32 {
        self.world.grid().height as u32
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

    /// Whether this is an oak mast year (HUD badge).
    pub fn mast_year(&self) -> bool {
        self.world.is_mast_year()
    }

    /// Climate readouts for the HUD, each 0..1.
    pub fn sun(&self) -> f32 {
        self.world.sun() as f32
    }

    pub fn moisture(&self) -> f32 {
        self.world.moisture() as f32
    }

    /// Biodiversity for the HUD: effective number of plant types, e^Shannon
    /// over the four tree species and four grass kinds.
    pub fn diversity(&self) -> f32 {
        self.world.diversity().1 as f32
    }

    /// Global illumination for the renderer: 1.0 at the neutral climate.
    pub fn light_level(&self) -> f32 {
        (0.7 + 0.6 * self.world.sun()) as f32
    }

    // ---- interaction (bx/by are backing-store pixels) ----

    /// Tile index under a screen point, or -1 when the ray misses the grid.
    /// Marches the ray over the raised tile tops, so a ridge in front of a
    /// valley is what gets picked.
    pub fn pick_tile(&self, bx: f32, by: f32) -> i32 {
        let top = RENDER_RELIEF * self.world.params().terrain + 0.01;
        self.camera
            .pick_heightfield(bx as f64, by as f64, top, |x, y| {
                self.world.grid().pick(x, y).map(|i| self.world.elevation(i) as f64)
            })
            .and_then(|p| self.world.grid().pick(p[0], p[1]))
            .map_or(-1, |i| i as i32)
    }

    /// Paint the tile under a screen point (brush: 0 clear, 1 grass,
    /// 2 tree, 3 fire; `species` selects the tree variety for brush 2,
    /// `grass` the grass kind for brush 1). Returns the painted tile index,
    /// or -1 on a miss.
    pub fn paint_at(&mut self, bx: f32, by: f32, brush: u8, species: u8, grass: u8) -> i32 {
        let index = self.pick_tile(bx, by);
        if index >= 0 {
            if let Some(Brush::Grass) = Brush::from_u8(brush) {
                self.world.paint_grass(index as usize, GrassKind::from_u8(grass), self.clock.tick());
            } else if let Some(b) = Brush::from_u8(brush) {
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
        bytemuck::cast_slice(&self.frame.ground).to_vec()
    }

    pub fn ground_instance_count(&self) -> u32 {
        self.frame.ground.len() as u32
    }

    /// Build this frame's instance streams once; call before the getters.
    pub fn prepare_frame(&mut self) {
        self.frame = build_instances(&self.world, self.clock.tick(), self.clock.alpha() as f32);
    }

    /// Per-species tree stream (0 acacia, 1 oak, 2 pine, 3 willow).
    pub fn tree_instances(&self, species: u8) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame.trees[(species as usize).min(3)]).to_vec()
    }

    /// Instance counts include standing-dead husks, so they can exceed the
    /// live-population counts the HUD shows.
    pub fn tree_instance_count(&self, species: u8) -> u32 {
        self.frame.trees[(species as usize).min(3)].len() as u32
    }

    /// Per-kind grass stream (0 bunchgrass, 1 sod, 2 sedge, 3 annual).
    pub fn grass_instances(&self, kind: u8) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame.grass[(kind as usize).min(GRASS_KIND_COUNT - 1)]).to_vec()
    }

    pub fn grass_instance_count(&self, kind: u8) -> u32 {
        self.frame.grass[(kind as usize).min(GRASS_KIND_COUNT - 1)].len() as u32
    }

    pub fn mushroom_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame.mushrooms).to_vec()
    }

    pub fn mushroom_instance_count(&self) -> u32 {
        self.frame.mushrooms.len() as u32
    }

    pub fn cloud_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame.clouds).to_vec()
    }

    pub fn cloud_instance_count(&self) -> u32 {
        self.frame.clouds.len() as u32
    }

    pub fn sheet_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame.sheets).to_vec()
    }

    pub fn sheet_instance_count(&self) -> u32 {
        self.frame.sheets.len() as u32
    }

    pub fn bolt_instances(&self) -> Vec<u8> {
        bytemuck::cast_slice(&self.frame.bolts).to_vec()
    }

    pub fn bolt_instance_count(&self) -> u32 {
        self.frame.bolts.len() as u32
    }
}
