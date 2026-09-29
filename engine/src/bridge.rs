//! wasm-bindgen bridge: marshalling only — every rule lives in `sim`, every
//! matrix in `render`. Exposes one `Simulation` object owning the world, the
//! clock, and the camera; per-frame data crosses the boundary as copied-out
//! byte buffers (bytemuck-cast `Instance` arrays), the same model as the
//! sibling traffic engine.

use wasm_bindgen::prelude::*;

use crate::render::camera::{light_view_proj, Camera};
use crate::render::scene::{build_view, View};
use crate::sim::clock::{PlayState, SimClock};
use crate::sim::terrain::SUN_DIR;
use crate::sim::world::{Brush, GrassKind, Params, Species, World};

/// Sim seconds per tick: 10 ticks/s at 1× speed.
const DT: f64 = 0.1;
/// Hard ceiling on catch-up ticks per frame; a tick is cheap (~4k cells), so
/// running the full backlog is faster than traffic's wall-clock budget dance.
/// Past this the clock drops the backlog and the sim degrades to slow-motion.
const MAX_CATCHUP_TICKS: u32 = 64;

/// Real-time length of a lightning flash, and the minimum gap between
/// flashes shown.
const FLASH_MS: f64 = 140.0;
const FLASH_GAP_MS: f64 = 450.0;

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
    /// Instance streams packed once per frame by `prepare_frame`.
    frame_bytes: Vec<u8>,
    frame_counts: Vec<u32>,
    /// The roots view toggle.
    roots_view: bool,
    /// The biome overlay toggle.
    biome_view: bool,
    /// Wall-clock time (ms) of the last lightning flash shown: a flash
    /// lasts FLASH_MS of real time and at most one shows per FLASH_GAP_MS,
    /// so fast playback flickers instead of strobing white.
    flash_at: f64,
    /// The tick of the last strike shown (a paused world doesn't re-flash).
    flash_tick: u64,
    /// Where that strike hit (world x, y): the flash is a local light.
    flash_pos: [f64; 2],
    /// Lightning flashes on/off (a comfort setting).
    flashes: bool,
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
            frame_bytes: Vec::new(),
            frame_counts: Vec::new(),
            roots_view: false,
            biome_view: false,
            flash_at: -1e9,
            flash_tick: u64::MAX,
            flash_pos: [0.0, 0.0],
            flashes: true,
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
        climate_zones: f64,
        rivers: f64,
        grazing: f64,
        physiology: f64,
        seasons: f64,
        cloud_dynamics: f64,
        biomes: f64,
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
            climate_zones,
            rivers,
            grazing,
            physiology,
            seasons,
            cloud_dynamics,
            biomes,
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
        let top = self.world.max_elevation() as f64;
        light_view_proj(sun, self.camera.shadow_bounds(self.world.grid()), top).to_vec()
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

    /// Local (16×16-tile window) effective types, α diversity.
    pub fn local_diversity(&self) -> f32 {
        self.world.local_diversity() as f32
    }

    /// A river flood pulse is under way (HUD badge).
    pub fn flooding(&self) -> bool {
        self.world.is_flooding()
    }

    /// Global illumination for the renderer: 1.0 at the neutral climate.
    pub fn light_level(&self) -> f32 {
        (0.6 + 0.8 * self.world.sun()) as f32
    }

    // ---- interaction (bx/by are backing-store pixels) ----

    /// Tile index under a screen point, or -1 when the ray misses the grid.
    /// Marches the ray over the raised tile tops, so a ridge in front of a
    /// valley is what gets picked.
    pub fn pick_tile(&self, bx: f32, by: f32) -> i32 {
        let top = self.world.max_elevation() as f64 + 0.01;
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

    // ---- per-frame instance streams (copied out, bytemuck-cast) ----

    /// Build this frame's instance streams once, then read them with
    /// `frame_bytes`/`frame_counts`. The display view: seasons show at
    /// slow speeds (years flash by faster than that), the sun and light
    /// follow the heat, and the roots view is a toggle.
    pub fn prepare_frame(&mut self) {
        let speed = self.clock.speed();
        let season_amp = (((0.25 - speed) / 0.2).clamp(0.0, 1.0) * self.world.params().seasons) as f32;
        let view = View {
            season_phase: self.clock.alpha() as f32,
            season_amp,
            roots: self.roots_view,
            heat: self.heat(),
            biomes: self.biome_view,
            flashes: self.flashes,
        };
        let frame = build_view(&self.world, self.clock.tick(), self.clock.alpha() as f32, &view);
        let (bytes, counts) = frame.pack();
        self.frame_bytes = bytes;
        self.frame_counts = counts;
    }

    /// Every instance stream packed in `scene::stream` order.
    pub fn frame_bytes(&self) -> Vec<u8> {
        self.frame_bytes.clone()
    }

    /// Instance count per stream.
    pub fn frame_counts(&self) -> Vec<u32> {
        self.frame_counts.clone()
    }

    /// Heat, 0..1, from the climate's sun signal: brighter, warmer light
    /// and a bigger, gold sun when high.
    pub fn heat(&self) -> f32 {
        ((self.world.sun() - 0.2) / 0.6).clamp(0.0, 1.0) as f32
    }

    /// The atmosphere for the renderer, 16 floats: [flash, overcast, time,
    /// wind x, wind y, wind strength, gust, haze r, haze g, haze b, haze
    /// density, fog height, fog strength, flash x, flash y, 0] — the flash
    /// is a local light around the strike, not a whole-scene brightening. Haze follows the
    /// weather — clear blue after rain, milky and warm in heat and drought,
    /// grey under overcast — and valley fog pools after wet, cool, still
    /// spells.
    pub fn atmosphere(&mut self) -> Vec<f32> {
        let now = now_ms();
        let tick = self.clock.tick();
        if self.flashes
            && self.world.flash() >= 1.0
            && tick != self.flash_tick
            && now - self.flash_at > FLASH_GAP_MS
        {
            self.flash_at = now;
            self.flash_tick = tick;
            if let Some(p) = self.world.last_strike() {
                self.flash_pos = p;
            }
        }
        let flash = if self.flashes {
            (1.0 - (now - self.flash_at) / FLASH_MS).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let w = &self.world;
        let overcast = w.overcast();
        let (dir, speed) = w.wind_at(self.clock.tick());
        let strength = (speed / 0.45).min(1.0);
        let heat = self.heat() as f64;
        let moisture = w.moisture();
        let n = w.grid().cells();
        let wet = (0..n).step_by(17).filter(|&i| w.wet_ratio(i) > 0.0).count() as f64 / (n / 17).max(1) as f64;
        let dry = (1.0 - moisture).clamp(0.0, 1.0);
        let lerp = |a: [f64; 3], b: [f64; 3], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let clear = [0.60, 0.70, 0.80];
        let milky = [0.82, 0.76, 0.62];
        let grey = [0.52, 0.54, 0.58];
        let haze = lerp(lerp(clear, milky, heat * dry), grey, overcast);
        let density = (0.0008 + 0.0028 * heat * dry + 0.0015 * overcast - 0.0006 * wet).max(0.0002);
        let fog = (((moisture - 0.5) * 2.5).clamp(0.0, 1.0) * ((0.55 - w.sun()) * 3.0).clamp(0.0, 1.0) * (1.0 - strength)
            + 0.4 * wet * (1.0 - strength))
            .min(1.0);
        vec![
            flash,
            overcast as f32,
            (now_ms() / 1000.0 % 10_000.0) as f32,
            dir[0] as f32,
            dir[1] as f32,
            strength as f32,
            (0.6 * overcast) as f32,
            haze[0] as f32,
            haze[1] as f32,
            haze[2] as f32,
            density as f32,
            (1.2 + 2.5 * fog) as f32,
            (fog * w.params().climate_zones.max(w.params().terrain)) as f32,
            self.flash_pos[0] as f32,
            self.flash_pos[1] as f32,
            0.0,
        ]
    }

    /// Turn lightning flashes on or off (comfort / photosensitivity).
    pub fn set_flashes(&mut self, on: bool) {
        self.flashes = on;
    }

    /// Tint the ground by climate biome.
    pub fn set_biome_view(&mut self, on: bool) {
        self.biome_view = on;
    }

    /// Share of the map in each biome (Biome order: wetland, tundra,
    /// boreal forest, temperate forest, grassland, savanna, desert).
    pub fn biome_shares(&self) -> Vec<f32> {
        let n = self.world.grid().cells();
        let mut c = vec![0.0f32; crate::sim::world::BIOME_COUNT];
        for i in 0..n {
            c[self.world.biome(i) as usize] += 1.0;
        }
        c.iter().map(|x| x / n as f32).collect()
    }

    /// A sample of tiles for the Whittaker chart: flat triples of (site
    /// temperature index, site water, biome index).
    pub fn biome_samples(&self) -> Vec<f32> {
        let n = self.world.grid().cells();
        let step = (n / 600).max(1);
        let mut out = Vec::with_capacity(3 * (n / step + 1));
        for i in (0..n).step_by(step) {
            let (t, w) = self.world.site_climate(i);
            out.extend([t as f32, w as f32, self.world.biome(i) as u8 as f32]);
        }
        out
    }

    /// Show the root systems under glass ground.
    pub fn set_roots_view(&mut self, on: bool) {
        self.roots_view = on;
    }

    pub fn roots_view(&self) -> bool {
        self.roots_view
    }

    /// The tile inspector: every factor bearing on the tile under a screen
    /// point, as text lines ("" on a miss).
    pub fn inspect_at(&self, bx: f32, by: f32) -> String {
        let index = self.pick_tile(bx, by);
        if index < 0 {
            return String::new();
        }
        self.world.inspect(index as usize, self.clock.tick())
    }

    /// Clouds on the map by genus: [cumulus, cumulonimbus, nimbostratus,
    /// cirrus].
    pub fn cloud_counts(&self) -> Vec<u32> {
        let mut c = vec![0u32; 4];
        for s in self.world.storms() {
            c[s.kind as usize] += 1;
        }
        c
    }

    /// Cloud lifecycle events since the world began (CloudEvent order:
    /// formed, towered, collapsed, front, broke up, evaporated, daughter).
    pub fn cloud_events(&self) -> Vec<u32> {
        self.world.cloud_events().to_vec()
    }

    /// Tree deaths by cause over roughly the last 50 ticks (see
    /// `DeathCause` for the order).
    pub fn deaths_recent(&self) -> Vec<f32> {
        self.world.deaths_recent().to_vec()
    }
}
