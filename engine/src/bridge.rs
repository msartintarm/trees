//! wasm-bindgen bridge: marshalling only — every rule lives in `sim`, every
//! matrix in `render`, every movement and time rule in `play`. Exposes one
//! `Simulation` object owning the world, the clock, the overview camera,
//! the display surface, and (walking) the wanderer; per-frame data crosses
//! the boundary as copied-out buffers, the same model as the sibling
//! traffic engine.

use wasm_bindgen::prelude::*;

use crate::play::player::{self, Ground, Player};
use crate::play::timeflow::{self, TimeFlow};
use crate::render::camera::{light_view_proj, march_heightfield, Camera, LookCamera, WALK_FOV_Y};
use crate::render::scene::{build_scene, tree_offset, trunk_radius, Focus, View};
use crate::render::sky::{self, LightMode, SunState};
use crate::render::surface::{self, Surface};
use crate::sim::clock::{PlayState, SimClock};
use crate::sim::world::{Brush, Cell, GrassKind, Params, Species, World, BIOME_COUNT};

/// Sim seconds per tick: 10 ticks/s at 1× speed.
const DT: f64 = 0.1;
/// Hard ceiling on catch-up ticks per frame.
const MAX_CATCHUP_TICKS: u32 = 64;
/// Real-time length of a lightning flash, and the minimum gap between
/// flashes shown.
const FLASH_MS: f64 = 140.0;
const FLASH_GAP_MS: f64 = 450.0;
/// Wall-clock milliseconds per frame the sim may spend on catch-up ticks.
const FRAME_BUDGET_MS: f64 = 12.0;
/// Walking: at most this many ecology ticks per frame (the rest carry to
/// the next frame, so a long rest streams years smoothly).
const WALK_TICKS_PER_FRAME: u32 = 4;
/// Walking: how far clouds race per game year of travel (they're weather:
/// years of it blow past a long-lived wanderer), capped per real second.
const CLOUD_DRIFT_PER_YEAR: f64 = 260.0;
const CLOUD_DRIFT_MAX: f64 = 30.0;
/// Timber a house takes.
pub const HOUSE_WOOD: f64 = 6.0;
/// Ground materials refresh (biomes shift with the climate) at most this
/// often in real time.
const MATERIAL_REFRESH_MS: f64 = 3000.0;

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

/// The world as the wanderer's controller sees it.
struct WorldGround<'a> {
    world: &'a World,
    surface: &'a Surface,
    tick: u64,
}

impl Ground for WorldGround<'_> {
    fn height(&self, x: f64, y: f64) -> f64 {
        self.surface.height_at(x, y)
    }

    fn water(&self, x: f64, y: f64) -> f64 {
        match self.world.grid().pick(x, y) {
            Some(i) if self.world.is_channel(i) => 0.35,
            Some(i) if self.world.inundated(i) => 0.2,
            _ => 0.0,
        }
    }

    fn obstacles(&self, x: f64, y: f64, out: &mut Vec<[f64; 3]>) {
        for i in self.world.grid().cells_in_box([x, y], 1.2) {
            let (cx, cy) = self.world.grid().center(i);
            if self.world.is_built(i) {
                out.push([cx, cy, 0.6]);
            } else if self.world.state(i) == Cell::Tree {
                let o = tree_offset(i);
                let sp = self.world.species(i);
                let mature = (self.world.params().tree_maturity_age as f64 * sp.traits().maturity).max(1.0);
                let grown = (self.world.age(i, self.tick) as f64 / mature).clamp(0.15, 1.0);
                // Saplings are pushed through; trunks stop you.
                if grown > 0.3 {
                    out.push([cx + o[0], cy + o[1], trunk_radius(sp as usize) * grown]);
                }
            }
        }
    }

    fn bounds(&self) -> (f64, f64, f64, f64) {
        self.world.grid().world_bounds()
    }
}

/// Walking state: the wanderer, their time, and their timber.
struct Walk {
    player: Player,
    time: TimeFlow,
    keys: u32,
    /// Game years accumulated toward the next ecology tick (0..1): the
    /// year's progress, which is also the season.
    year: f64,
    /// Ticks owed but not yet run (a rest streams them over frames).
    owed: u32,
    wood: f64,
    cloud_drift: [f64; 2],
    /// Last action's outcome, for the HUD.
    message: String,
}

#[derive(Clone, Copy, PartialEq)]
struct SurfaceKey {
    seed: u32,
    width: u32,
    height: u32,
    terrain: f64,
    climate_zones: f64,
    biomes: f64,
    rivers: f64,
    landforms: bool,
}

#[wasm_bindgen]
pub struct Simulation {
    world: World,
    clock: SimClock,
    camera: Camera,
    seed: u32,
    frame_bytes: Vec<u8>,
    frame_counts: Vec<u32>,
    roots_view: bool,
    biome_view: bool,
    flash_at: f64,
    flash_tick: u64,
    flash_pos: [f64; 2],
    flashes: bool,
    tick_ms: f64,
    actual_speed: f64,
    // Display surface and its GPU-ready terrain.
    surface: Surface,
    surface_key: Option<SurfaceKey>,
    terrain_version: u32,
    materials_at: f64,
    materials_tick: u64,
    terrain_vertices: Vec<u8>,
    terrain_indices: Vec<u32>,
    terrain_chunks: Vec<f32>,
    skirt_vertices: Vec<u8>,
    skirt_indices: Vec<u32>,
    // Display settings.
    hex_columns: bool,
    landforms: bool,
    light_mode: LightMode,
    bloom: bool,
    detail: f32,
    hex_overlay: bool,
    walk: Option<Walk>,
    /// This frame's uniform block (see `frame_uniforms`).
    uniforms: Vec<f32>,
    last_frame_ms: f64,
}

#[wasm_bindgen]
impl Simulation {
    #[wasm_bindgen(constructor)]
    pub fn new(seed: u32) -> Simulation {
        let world = World::new(seed as u64);
        let camera = Camera::fit_world(world.grid());
        let surface = Surface::flat_from(&world);
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
            surface,
            surface_key: None,
            terrain_version: 0,
            materials_at: 0.0,
            materials_tick: 0,
            terrain_vertices: Vec::new(),
            terrain_indices: Vec::new(),
            terrain_chunks: Vec::new(),
            skirt_vertices: Vec::new(),
            skirt_indices: Vec::new(),
            hex_columns: false,
            landforms: true,
            light_mode: LightMode::Clock,
            bloom: true,
            detail: 1.0,
            hex_overlay: false,
            walk: None,
            uniforms: Vec::new(),
            last_frame_ms: 0.0,
        }
    }

    /// Rebuild the world from a new seed; camera, clock speed, and tunables
    /// survive, the tick counter restarts. A walk ends.
    pub fn reseed(&mut self, seed: u32) {
        let speed = self.clock.speed();
        let playing = self.clock.state() == PlayState::Playing;
        let old_grid = self.world.grid();
        self.world = World::with_params(seed as u64, self.world.params());
        if self.world.grid() != old_grid {
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
        self.walk = None;
        self.refresh_surface(true);
    }

    /// Convert real elapsed seconds into sim ticks and run them. Overview:
    /// the clock at its speed. Walking: the wanderer moves, and time flows
    /// with their movement and actions (see `play::timeflow`). Returns the
    /// number of ticks executed this frame.
    pub fn advance(&mut self, real_elapsed_secs: f64) -> u32 {
        if self.walk.is_some() {
            return self.advance_walk(real_elapsed_secs);
        }
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

    fn advance_walk(&mut self, dt: f64) -> u32 {
        let tick = self.clock.tick();
        let Some(walk) = self.walk.as_mut() else { return 0 };
        {
            let ground = WorldGround { world: &self.world, surface: &self.surface, tick };
            walk.player.step(dt, walk.keys, &ground);
        }
        let resting = walk.keys & player::keys::REST != 0;
        let years = walk.time.step(dt, walk.player.motion, resting);
        walk.year += years;
        while walk.year >= 1.0 {
            walk.year -= 1.0;
            walk.owed += 1;
        }
        // Clouds race past in proportion to the time flying by.
        let (dir, _) = self.world.wind_at(tick);
        let d = (years * CLOUD_DRIFT_PER_YEAR).min(CLOUD_DRIFT_MAX * dt);
        walk.cloud_drift[0] += dir[0] * d;
        walk.cloud_drift[1] += dir[1] * d;
        let run = walk.owed.min(WALK_TICKS_PER_FRAME);
        walk.owed -= run;
        for _ in 0..run {
            let completed = self.clock.single_step();
            self.world.step(completed + 1);
        }
        if dt > 0.0 {
            // Years per second, as a speed readout (ticks are years).
            let speed = years / dt / DT;
            self.actual_speed = 0.9 * self.actual_speed + 0.1 * speed;
        }
        run
    }

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
        self.walk.is_some() || self.clock.state() == PlayState::Playing
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

    /// `[bare, grass, trees, burning, storms, infested, houses]`.
    pub fn counts(&self) -> Vec<u32> {
        let mut c = self.world.counts().to_vec();
        c.push(self.world.burning_count());
        c.push(self.world.storms().iter().filter(|s| s.kind.traits().rains).count() as u32);
        c.push(self.world.infested_count());
        c.push((0..self.world.grid().cells()).filter(|&i| self.world.is_built(i)).count() as u32);
        c
    }

    /// Swap the ecology tunables (values are sanitized engine-side).
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

    // ---- display settings ----

    /// Hex columns (the stepped platformer surface) or the smooth surface.
    /// The wanderer walks both with the same rules.
    pub fn set_hex_columns(&mut self, on: bool) {
        self.hex_columns = on;
        self.surface = self.surface.clone().with_hex(on);
    }

    /// Biome landforms (dunes, mesas, peaks, hummocks) exaggerated on top
    /// of the simulated relief.
    pub fn set_landforms(&mut self, on: bool) {
        self.landforms = on;
    }

    /// 0 = follow the clock, 1 golden hour, 2 dusk, 3 night.
    pub fn set_light_mode(&mut self, mode: u8) {
        self.light_mode = LightMode::from_u8(mode);
    }

    pub fn set_bloom(&mut self, on: bool) {
        self.bloom = on;
    }

    /// Close-up detail (parallax relief) 0..1 — the low setting for slow
    /// GPUs.
    pub fn set_detail(&mut self, detail: f32) {
        self.detail = detail.clamp(0.0, 1.0);
    }

    pub fn set_hex_overlay(&mut self, on: bool) {
        self.hex_overlay = on;
    }

    /// Renderer flags for this frame (gpu::FLAG_*).
    pub fn render_flags(&self) -> u32 {
        (self.roots_view as u32) | ((self.bloom as u32) << 1) | ((self.hex_columns as u32) << 2)
    }

    // ---- terrain for the renderer ----

    /// Bumped whenever the terrain buffers below change (re-upload then).
    pub fn terrain_version(&self) -> u32 {
        self.terrain_version
    }

    pub fn terrain_vertices(&self) -> Vec<u8> {
        self.terrain_vertices.clone()
    }

    pub fn terrain_indices(&self) -> Vec<u32> {
        self.terrain_indices.clone()
    }

    pub fn terrain_chunks(&self) -> Vec<f32> {
        self.terrain_chunks.clone()
    }

    pub fn skirt_vertices(&self) -> Vec<u8> {
        self.skirt_vertices.clone()
    }

    pub fn skirt_indices(&self) -> Vec<u32> {
        self.skirt_indices.clone()
    }

    fn surface_key(&self) -> SurfaceKey {
        let p = self.world.params();
        SurfaceKey {
            seed: self.seed,
            width: self.world.grid().width as u32,
            height: self.world.grid().height as u32,
            terrain: p.terrain,
            climate_zones: p.climate_zones,
            biomes: p.biomes,
            rivers: p.rivers,
            landforms: self.landforms,
        }
    }

    /// Rebuild the display surface when the landscape changed, and the
    /// ground materials now and then (biomes drift with the climate).
    fn refresh_surface(&mut self, force: bool) {
        let key = self.surface_key();
        let now = now_ms();
        if force || self.surface_key != Some(key) {
            self.surface_key = Some(key);
            self.surface =
                Surface::build(&self.world, if self.landforms { 1.0 } else { 0.0 }).with_hex(self.hex_columns);
            let v = surface::terrain_vertices(&self.world, &self.surface);
            let (idx, chunks) = surface::terrain_indices(self.world.grid(), &v);
            self.terrain_vertices = bytemuck::cast_slice(&v).to_vec();
            self.terrain_indices = idx;
            self.terrain_chunks = surface::chunk_table(&chunks);
            let skirt = surface::skirt_mesh(&self.surface);
            self.skirt_vertices = bytemuck::cast_slice(&skirt.vertices).to_vec();
            self.skirt_indices = skirt.indices;
            self.terrain_version += 1;
            self.materials_at = now;
            self.materials_tick = self.clock.tick();
        } else if self.clock.tick() >= self.materials_tick + 25 && now - self.materials_at > MATERIAL_REFRESH_MS {
            let v = surface::terrain_vertices(&self.world, &self.surface);
            self.terrain_vertices = bytemuck::cast_slice(&v).to_vec();
            self.terrain_version += 1;
            self.materials_at = now;
            self.materials_tick = self.clock.tick();
        }
    }

    // ---- overview camera ----

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

    pub fn grid_width(&self) -> u32 {
        self.world.grid().width as u32
    }

    pub fn grid_height(&self) -> u32 {
        self.world.grid().height as u32
    }

    pub fn mast_year(&self) -> bool {
        self.world.is_mast_year()
    }

    pub fn sun(&self) -> f32 {
        self.world.sun() as f32
    }

    pub fn moisture(&self) -> f32 {
        self.world.moisture() as f32
    }

    pub fn diversity(&self) -> f32 {
        self.world.diversity().1 as f32
    }

    pub fn local_diversity(&self) -> f32 {
        self.world.local_diversity() as f32
    }

    pub fn flooding(&self) -> bool {
        self.world.is_flooding()
    }

    fn light_level(&self) -> f32 {
        (0.6 + 0.8 * self.world.sun()) as f32
    }

    // ---- walking ----

    /// Step down into the world as the wanderer, at the tile under a
    /// screen point (or the camera's target on a miss), facing the way the
    /// camera looked.
    pub fn enter_walk(&mut self, bx: f32, by: f32) {
        let tick = self.clock.tick();
        let (x, y) = match self.pick_tile(bx, by) {
            i if i >= 0 => self.world.grid().center(i as usize),
            _ => (self.camera.target[0], self.camera.target[1]),
        };
        let yaw = self.camera.yaw + std::f64::consts::PI;
        let ground = WorldGround { world: &self.world, surface: &self.surface, tick };
        let player = Player::spawn(&ground, x, y, yaw);
        let season = if self.world.params().seasons > 0.0 { 0.12 } else { 0.3 };
        self.walk = Some(Walk {
            player,
            time: TimeFlow::new(season, 0.42),
            keys: 0,
            year: season,
            owed: 0,
            wood: 0.0,
            cloud_drift: [0.0, 0.0],
            message: String::new(),
        });
    }

    /// Back to the overview, looking down at where the wanderer stood.
    pub fn exit_walk(&mut self) {
        if let Some(w) = self.walk.take() {
            self.camera.target = [w.player.pos[0], w.player.pos[1]];
            self.camera.distance = 30.0;
            self.camera.pitch = 0.7;
            self.camera.yaw = w.player.yaw + std::f64::consts::PI;
        }
    }

    pub fn walking(&self) -> bool {
        self.walk.is_some()
    }

    /// Held movement keys (play::player::keys bitmask).
    pub fn set_keys(&mut self, keys: u32) {
        if let Some(w) = self.walk.as_mut() {
            w.keys = keys;
        }
    }

    /// Mouse-look (radians).
    pub fn look(&mut self, dyaw: f32, dpitch: f32) {
        if let Some(w) = self.walk.as_mut() {
            w.player.look(dyaw as f64, dpitch as f64);
        }
    }

    pub fn toggle_third_person(&mut self) {
        if let Some(w) = self.walk.as_mut() {
            w.player.third_person = !w.player.third_person;
        }
    }

    fn look_camera(&self, w: &Walk) -> LookCamera {
        let ground = WorldGround { world: &self.world, surface: &self.surface, tick: self.clock.tick() };
        let (eye, fwd) = w.player.camera(&ground);
        LookCamera { eye, fwd, viewport: self.camera.viewport, fov_y: WALK_FOV_Y, near: 0.04, far: 900.0 }
    }

    /// The tile under the crosshair within reach, if any.
    fn target_tile(&self) -> Option<usize> {
        let w = self.walk.as_ref()?;
        let eye = w.player.eye();
        let fwd = w.player.forward();
        let hit = march_heightfield(eye, fwd, player::REACH + 1.0, |x, y| Some(self.surface.height_at(x, y) + 0.05))
            .unwrap_or([eye[0] + fwd[0] * 1.2, eye[1] + fwd[1] * 1.2, 0.0]);
        let d = ((hit[0] - w.player.pos[0]).powi(2) + (hit[1] - w.player.pos[1]).powi(2)).sqrt();
        // Prefer a trunk within reach that the view ray passes close to.
        let mut best = None;
        let mut best_d = 0.5;
        for i in self.world.grid().cells_in_box([eye[0], eye[1]], player::REACH + 0.5) {
            if self.world.state(i) != Cell::Tree {
                continue;
            }
            let (cx, cy) = self.world.grid().center(i);
            let o = tree_offset(i);
            let (tx, ty) = (cx + o[0] - eye[0], cy + o[1] - eye[1]);
            let along = tx * fwd[0] + ty * fwd[1];
            if along <= 0.0 || along > player::REACH + 0.5 {
                continue;
            }
            let hl = (fwd[0] * fwd[0] + fwd[1] * fwd[1]).sqrt().max(1e-6);
            let perp = (tx * fwd[1] - ty * fwd[0]).abs() / hl;
            if perp < best_d {
                best_d = perp;
                best = Some(i);
            }
        }
        best.or_else(|| if d <= player::REACH { self.world.grid().pick(hit[0], hit[1]) } else { None })
    }

    /// Act on the tile under the crosshair: 0 inspect (see
    /// `inspect_target`), 1 fell a tree, 2 plant a tree (`species`), 3
    /// plant grass (`grass`), 4 build a house, 5 light a fire. Each action
    /// time-lapses its cost. Returns what happened, for the HUD.
    pub fn act(&mut self, action: u8, species: u8, grass: u8) -> String {
        let tick = self.clock.tick();
        let target = self.target_tile();
        let Some(w) = self.walk.as_mut() else { return String::new() };
        if let Some((label, _)) = w.time.lapse() {
            return format!("…still {label}");
        }
        let Some(i) = target else {
            w.message = "Nothing within reach".into();
            return w.message.clone();
        };
        let msg = match action {
            1 => match self.world.fell_tree(i, tick) {
                Some(wood) => {
                    w.wood += wood;
                    w.time.act(timeflow::FELL, "felling");
                    format!("Felled a tree: +{wood:.1} timber — a season passes")
                }
                None => "No tree there to fell".into(),
            },
            2 if self.world.is_channel(i) || self.world.is_built(i) => "Nothing roots there".into(),
            2 => {
                let sp = Species::from_u8(species);
                self.world.paint_species(i, Brush::Tree, sp, tick);
                w.time.act(timeflow::PLANT, "planting");
                format!("Planted a {} sapling", sp.traits().name.to_lowercase())
            }
            3 if self.world.is_channel(i) || self.world.is_built(i) => "Nothing roots there".into(),
            3 => {
                self.world.paint_grass(i, GrassKind::from_u8(grass), tick);
                w.time.act(timeflow::PLANT, "planting");
                "Sowed grass".into()
            }
            4 if w.wood < HOUSE_WOOD => format!("A house takes {HOUSE_WOOD:.0} timber (you have {:.1})", w.wood),
            4 if self.world.build_house(i, tick) => {
                w.wood -= HOUSE_WOOD;
                w.time.act(timeflow::BUILD, "building");
                "Raised a house — two years pass".into()
            }
            4 => "You can't build there".into(),
            5 => {
                self.world.paint(i, Brush::Fire, tick);
                w.time.act(timeflow::IGNITE, "lighting a fire");
                if self.world.burning(i) { "Lit a fire".into() } else { "Nothing there will burn".into() }
            }
            _ => String::new(),
        };
        w.message = msg.clone();
        msg
    }

    /// The inspector report for the tile under the crosshair.
    pub fn inspect_target(&self) -> String {
        self.target_tile().map_or(String::new(), |i| self.world.inspect(i, self.clock.tick()))
    }

    /// What the crosshair rests on, one line ("" when nothing in reach).
    pub fn target_label(&self) -> String {
        let Some(i) = self.target_tile() else { return String::new() };
        let tick = self.clock.tick();
        if self.world.is_built(i) {
            return "Your house".into();
        }
        if self.world.is_channel(i) {
            return "River water".into();
        }
        match self.world.state(i) {
            Cell::Tree => format!("{} · {} years", self.world.species(i).traits().name, self.world.age(i, tick)),
            Cell::Grass => format!("{} · {} years", crate::sim::world::GRASS_TABLE[self.world.grass_kind(i) as usize].name, self.world.age(i, tick)),
            Cell::Bare => format!("Bare ground · {}", self.world.biome(i).name()),
        }
    }

    /// Walking readout: [game years walked, time of day 0..1, days per
    /// real second, years of action time-lapse left, timber, wading 0/1,
    /// third person 0/1, year progress 0..1].
    pub fn walk_status(&self) -> Vec<f32> {
        let Some(w) = &self.walk else { return Vec::new() };
        vec![
            w.time.years() as f32,
            w.time.day_frac() as f32,
            w.time.days_per_sec() as f32,
            w.time.lapse().map_or(0.0, |l| l.1) as f32,
            w.wood as f32,
            w.player.wading as u8 as f32,
            w.player.third_person as u8 as f32,
            w.year as f32,
        ]
    }

    /// The current action being time-lapsed, or the last action's outcome.
    pub fn walk_message(&self) -> String {
        match &self.walk {
            Some(w) => match w.time.lapse() {
                Some((label, _)) => format!("⏩ {label}…"),
                None => w.message.clone(),
            },
            None => String::new(),
        }
    }

    // ---- overview interaction (bx/by are backing-store pixels) ----

    /// Tile index under a screen point, or -1 when the ray misses the grid.
    pub fn pick_tile(&self, bx: f32, by: f32) -> i32 {
        let top = self.surface.max_height().max(self.world.max_elevation()) as f64 + 0.01;
        self.camera
            .pick_heightfield(bx as f64, by as f64, top, |x, y| {
                self.world.grid().pick(x, y).map(|_| self.surface.height_at(x, y))
            })
            .and_then(|p| self.world.grid().pick(p[0], p[1]))
            .map_or(-1, |i| i as i32)
    }

    pub fn paint_at(&mut self, bx: f32, by: f32, brush: u8, species: u8, grass: u8) -> i32 {
        let index = self.pick_tile(bx, by);
        if index >= 0 {
            if let Some(Brush::Grass) = Brush::from_u8(brush) {
                self.world.paint_grass(index as usize, GrassKind::from_u8(grass), self.clock.tick());
            } else if let Some(b) = Brush::from_u8(brush) {
                self.world.paint_species(index as usize, b, Species::from_u8(species), self.clock.tick());
            }
        }
        index
    }

    // ---- per-frame data ----

    /// Build this frame's instance streams and uniform block once, then
    /// read them with `frame_bytes`/`frame_counts`/`frame_uniforms`.
    pub fn prepare_frame(&mut self) {
        self.refresh_surface(false);
        let now = now_ms();
        let tick = self.clock.tick();
        let walking = self.walk.is_some();
        let (alpha, season_phase, season_amp) = match &self.walk {
            Some(w) => (w.year as f32, w.year as f32, self.world.params().seasons as f32),
            None => {
                let speed = self.clock.speed();
                let amp = (((0.25 - speed) / 0.2).clamp(0.0, 1.0) * self.world.params().seasons) as f32;
                (self.clock.alpha() as f32, self.clock.alpha() as f32, amp)
            }
        };
        let sun = self.sun_state(season_phase as f64, season_amp as f64);
        let view = View {
            season_phase,
            season_amp,
            roots: self.roots_view,
            heat: self.heat(),
            biomes: self.biome_view,
            flashes: self.flashes,
        };
        // The viewer: the wanderer's eyes, or the overview camera.
        let (eye, vp, center, basis, focus) = match &self.walk {
            Some(w) => {
                let cam = self.look_camera(w);
                let f = Focus {
                    eye: cam.eye,
                    view_proj: cam.view_proj(),
                    center: [w.player.pos[0], w.player.pos[1]],
                    tree_full: 70.0,
                    grass_far: 55.0,
                    near: 20.0,
                    particles: 26.0,
                    cloud_drift: w.cloud_drift,
                    daylight: sun.daylight,
                };
                (cam.eye, f.view_proj, f.center, cam.sky_basis(), f)
            }
            None => {
                let d = self.camera.distance;
                let f = Focus {
                    eye: self.camera.eye(),
                    view_proj: self.camera.view_proj(),
                    center: self.camera.target,
                    tree_full: (d * 0.9).max(60.0),
                    grass_far: d * 1.6 + 60.0,
                    near: if d < 45.0 { 18.0 } else { 0.0 },
                    particles: if d < 60.0 { 24.0 } else { 0.0 },
                    cloud_drift: [0.0, 0.0],
                    daylight: sun.daylight,
                };
                (f.eye, f.view_proj, f.center, self.camera.sky_basis(), f)
            }
        };
        let frame = build_scene(&self.world, tick, alpha, &view, &self.surface, Some(&focus));
        let (bytes, counts) = frame.pack();
        self.frame_bytes = bytes;
        self.frame_counts = counts;

        // Shadow cascades: sharp near the viewer, broad over the view.
        let top = self.surface.max_height().max(self.world.max_elevation()) as f64;
        let key = sun.light_dir.map(|v| v as f32);
        let (bx0, by0, bx1, by1) = self.world.grid().world_bounds();
        let clip = |r: f64| ((center[0] - r).max(bx0), (center[1] - r).max(by0), (center[0] + r).min(bx1), (center[1] + r).min(by1));
        let near_r = if walking { 24.0 } else { (self.camera.distance * 0.35).clamp(10.0, 80.0) };
        let near = light_view_proj(key, clip(near_r), top);
        let far = if walking { light_view_proj(key, clip(160.0), top) } else { light_view_proj(key, self.camera.shadow_bounds(self.world.grid()), top) };
        let atmos = self.atmosphere(now);
        let heat = self.heat();
        let grade = self.grade(center);
        let mut u = Vec::with_capacity(crate::render::gpu::UNIFORM_FLOATS);
        u.extend_from_slice(&vp);
        u.extend_from_slice(&near);
        u.extend_from_slice(&near);
        u.extend_from_slice(&far);
        u.extend([key[0], key[1], key[2], atmos[0]]);
        u.extend([alpha, self.light_level(), heat, atmos[1]]);
        u.extend([eye[0] as f32, eye[1] as f32, eye[2] as f32, atmos[2]]);
        u.extend_from_slice(&atmos[3..7]);
        u.extend_from_slice(&atmos[7..11]);
        u.extend_from_slice(&atmos[11..15]);
        u.extend([sun.light_color[0] as f32, sun.light_color[1] as f32, sun.light_color[2] as f32, sun.daylight as f32]);
        u.extend([sun.sun_dir[0] as f32, sun.sun_dir[1] as f32, sun.sun_dir[2] as f32, sun.trail as f32]);
        u.extend([
            sun.declination as f32,
            sky::LATITUDE_DEG.to_radians() as f32,
            0.028 * (1.0 + 0.8 * heat),
            self.hex_overlay as u8 as f32,
        ]);
        u.extend([basis[0][0] as f32, basis[0][1] as f32, basis[0][2] as f32, self.detail]);
        u.extend([basis[1][0] as f32, basis[1][1] as f32, basis[1][2] as f32, walking as u8 as f32]);
        u.extend([basis[2][0] as f32, basis[2][1] as f32, basis[2][2] as f32, season_phase]);
        u.extend([grade[0], grade[1], grade[2], 1.0]);
        // Post: bloom strength, exposure (a little lift at night so the
        // moonlit world stays playable), vignette, bright threshold.
        let exposure = 1.0 + 0.35 * (1.0 - sun.daylight as f32);
        u.extend([0.55, exposure, if walking { 0.28 } else { 0.12 }, 0.95]);
        self.uniforms = u;
        self.last_frame_ms = now;
    }

    fn sun_state(&self, season_phase: f64, season_amp: f64) -> SunState {
        let year = if season_amp > 0.0 { season_phase } else { 0.15 };
        match &self.walk {
            Some(w) => sky::sun_state(w.time.day_frac(), year, sky::blur_for_rate(w.time.days_per_sec()), self.light_mode),
            // The overview runs years per second: days are always a blur
            // (the sun stands at its representative daytime position).
            None => sky::sun_state(0.5, year, 1.0, self.light_mode),
        }
    }

    /// Regional color grade from the biomes around the viewer: warm desert
    /// and savanna light, cool boreal and tundra air — kept subtle.
    fn grade(&self, center: [f64; 2]) -> [f32; 3] {
        const TINT: [[f32; 3]; BIOME_COUNT] = [
            [0.97, 1.0, 1.02],
            [0.95, 0.99, 1.06],
            [0.95, 1.0, 1.04],
            [1.0, 1.01, 0.98],
            [1.02, 1.01, 0.96],
            [1.05, 0.99, 0.92],
            [1.07, 1.0, 0.88],
        ];
        let w = surface::dominant_biome_near(&self.world, center, 30.0);
        let mut g = [0.0f32; 3];
        for (k, t) in TINT.iter().enumerate() {
            for c in 0..3 {
                g[c] += w[k] * t[c];
            }
        }
        if w.iter().sum::<f32>() <= 0.0 {
            return [1.0; 3];
        }
        g
    }

    pub fn frame_bytes(&self) -> Vec<u8> {
        self.frame_bytes.clone()
    }

    pub fn frame_counts(&self) -> Vec<u32> {
        self.frame_counts.clone()
    }

    /// The renderer's uniform block for this frame (see gpu::render).
    pub fn frame_uniforms(&self) -> Vec<f32> {
        self.uniforms.clone()
    }

    pub fn heat(&self) -> f32 {
        ((self.world.sun() - 0.2) / 0.6).clamp(0.0, 1.0) as f32
    }

    /// [flash, overcast, time, wind x, wind y, wind strength, gust, haze
    /// rgb, haze density, fog height, fog strength, flash x, flash y] — the
    /// flash is a local light around the strike, never a whole-scene
    /// brightening.
    fn atmosphere(&mut self, now: f64) -> [f32; 15] {
        let tick = self.clock.tick();
        if self.flashes && self.world.flash() >= 1.0 && tick != self.flash_tick && now - self.flash_at > FLASH_GAP_MS {
            self.flash_at = now;
            self.flash_tick = tick;
            if let Some(p) = self.world.last_strike() {
                self.flash_pos = p;
            }
        }
        let flash = if self.flashes { (1.0 - (now - self.flash_at) / FLASH_MS).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let w = &self.world;
        let overcast = w.overcast();
        let (dir, speed) = w.wind_at(tick);
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
        // Walking, the view is long and low: thicker aerial perspective.
        let scale = if self.walk.is_some() { 2.2 } else { 1.0 };
        let density = (0.0008 + 0.0028 * heat * dry + 0.0015 * overcast - 0.0006 * wet).max(0.0002) * scale;
        let fog = (((moisture - 0.5) * 2.5).clamp(0.0, 1.0) * ((0.55 - w.sun()) * 3.0).clamp(0.0, 1.0) * (1.0 - strength)
            + 0.4 * wet * (1.0 - strength))
            .min(1.0);
        [
            flash,
            overcast as f32,
            (now / 1000.0 % 10_000.0) as f32,
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
        ]
    }

    pub fn set_flashes(&mut self, on: bool) {
        self.flashes = on;
    }

    pub fn set_biome_view(&mut self, on: bool) {
        self.biome_view = on;
    }

    pub fn biome_shares(&self) -> Vec<f32> {
        let n = self.world.grid().cells();
        let mut c = vec![0.0f32; BIOME_COUNT];
        for i in 0..n {
            c[self.world.biome(i) as usize] += 1.0;
        }
        c.iter().map(|x| x / n as f32).collect()
    }

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

    pub fn set_roots_view(&mut self, on: bool) {
        self.roots_view = on;
    }

    pub fn roots_view(&self) -> bool {
        self.roots_view
    }

    /// The tile inspector for a screen point ("" on a miss).
    pub fn inspect_at(&self, bx: f32, by: f32) -> String {
        let index = self.pick_tile(bx, by);
        if index < 0 {
            return String::new();
        }
        self.world.inspect(index as usize, self.clock.tick())
    }

    pub fn cloud_counts(&self) -> Vec<u32> {
        let mut c = vec![0u32; 4];
        for s in self.world.storms() {
            c[s.kind as usize] += 1;
        }
        c
    }

    pub fn cloud_events(&self) -> Vec<u32> {
        self.world.cloud_events().to_vec()
    }

    pub fn deaths_recent(&self) -> Vec<f32> {
        self.world.deaths_recent().to_vec()
    }
}
