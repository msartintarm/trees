//! Animals as ecology: grazing herds and the wolf packs that hunt them.
//!
//! - **Herds** each spend the year where forage is best within a few
//!   tiles' trek, near water (herbivores stay within reach of drinking
//!   water) and away from wolves. They grow with forage per head and
//!   shrink when it runs short; where they graze, they crop the grass and
//!   browse the saplings.
//! - **Packs** follow the nearest herd, take a share of it when close, and
//!   starve without prey — discrete predator–prey dynamics.
//! - **Fear**: herds avoid wolf country and spend less time browsing
//!   there, so saplings recover where packs roam (the "landscape of fear",
//!   Ripple & Beschta 2004 — debated in its strength, kept moderate here).
//!
//! The kind of grazer follows the biome it settles in (bison on the
//! prairie, antelope on the savanna, caribou on the tundra and taiga, elk
//! in the woods, oryx in the desert, moose in the wetlands). Herds arrive
//! once there is grass to eat, and wolves once there are herds — the world
//! still starts empty.

use super::hex::{self, Grid};
use super::rng::{self, Stream};
use super::world::{Biome, Cell, World, GRASS_TABLE};

/// Tiles a herd can trek in a year to new pasture (and a pack to prey).
const HERD_TREK: f64 = 5.0;
const PACK_TREK: f64 = 7.0;
/// Forage radius around a herd (tiles).
const GRAZE_RADIUS: i32 = 4;
/// Forage per head at which a herd holds steady.
const FORAGE_PER_HEAD: f64 = 0.6;
const HERD_GROWTH: f64 = 0.25;
const HERD_START: f64 = 12.0;
const HERD_MAX: f64 = 120.0;
/// Predation: share of a nearby herd a pack takes per year, capped by
/// what the pack can eat; wolves gain from kills and starve without.
const KILL_SHARE: f64 = 0.12;
const KILLS_PER_WOLF: f64 = 0.8;
const WOLF_GAIN: f64 = 0.3;
const WOLF_LOSS: f64 = 0.18;
const PACK_START: f64 = 4.0;
/// Reach of a pack's hunt and of the fear it casts (world units).
const HUNT_REACH: f64 = 8.0;
const FEAR_RADIUS: i32 = 7;
/// Arrival: grass tiles before herds immigrate, herd animals before
/// wolves do.
const GRASS_FOR_HERDS: usize = 250;
const ANIMALS_FOR_WOLVES: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Grazer {
    Bison = 0,
    Antelope = 1,
    Caribou = 2,
    Elk = 3,
    Oryx = 4,
    Moose = 5,
}

impl Grazer {
    pub fn for_biome(b: Biome) -> Grazer {
        match b {
            Biome::Grassland => Grazer::Bison,
            Biome::Savanna => Grazer::Antelope,
            Biome::Tundra | Biome::Boreal => Grazer::Caribou,
            Biome::TemperateForest => Grazer::Elk,
            Biome::Desert => Grazer::Oryx,
            Biome::Wetland => Grazer::Moose,
        }
    }

    pub fn name(self) -> &'static str {
        ["bison", "antelope", "caribou", "elk", "oryx", "moose"][self as usize]
    }
}

#[derive(Clone, Debug)]
pub struct Herd {
    pub kind: Grazer,
    pub pos: [f64; 2],
    /// Last year's position (the renderer glides between them).
    pub prev: [f64; 2],
    pub size: f64,
    /// Direction of travel (radians).
    pub heading: f64,
    /// Stable identity (for the renderer's per-animal offsets).
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct Pack {
    pub pos: [f64; 2],
    pub prev: [f64; 2],
    pub size: f64,
    pub heading: f64,
    /// Stable identity (for the renderer's per-animal offsets).
    pub id: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Fauna {
    pub herds: Vec<Herd>,
    pub packs: Vec<Pack>,
    /// Grazing/browsing pressure per tile, 0..1, from the herds this year.
    pub pressure: Vec<f32>,
    /// Wolf presence per tile, 0..1 (the landscape of fear).
    pub fear: Vec<f32>,
    next_id: u32,
}

/// Palatable grass within `radius` tiles of a point (the herd's pasture).
fn forage(w: &World, at: [f64; 2], radius: i32) -> f64 {
    let grid = w.grid();
    let Some(c) = grid.pick(at[0], at[1]) else { return 0.0 };
    let (q, r) = grid.index_to_axial(c);
    let mut f = 0.0;
    for (dq, dr, _) in hex::disk(radius) {
        if let Some(j) = grid.axial_to_index(q + dq, r + dr) {
            if w.state(j) == Cell::Grass {
                f += GRASS_TABLE[w.grass_kind(j) as usize].palatability;
            }
        }
    }
    f
}

fn add_disk(grid: Grid, field: &mut [f32], at: [f64; 2], radius: i32, peak: f32) {
    let Some(c) = grid.pick(at[0], at[1]) else { return };
    let (q, r) = grid.index_to_axial(c);
    for (dq, dr, d) in hex::disk(radius) {
        if let Some(j) = grid.axial_to_index(q + dq, r + dr) {
            let fall = 1.0 - d as f32 / (radius + 1) as f32;
            field[j] = (field[j] + peak * fall).min(1.0);
        }
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Fauna {
    pub fn new(cells: usize) -> Fauna {
        Fauna { pressure: vec![0.0; cells], fear: vec![0.0; cells], ..Default::default() }
    }

    /// One year: arrivals, movement, grazing, predation, and the pressure
    /// and fear fields the ecology reads.
    pub fn step(&mut self, w: &World, seed: u64, tick: u64) {
        let grid = w.grid();
        let n = grid.cells();
        if self.pressure.len() != n {
            *self = Fauna::new(n);
        }
        let step = hex::SQRT3 * hex::SIZE;
        let u = |key: u32, salt: u64| rng::uniform01(seed, key, tick * 16 + salt, Stream::Fauna);
        let grass = (0..n).step_by(4).filter(|&i| w.state(i) == Cell::Grass).count() * 4;
        // Arrivals.
        let herd_cap = (n / 5000).clamp(1, 12);
        if grass >= GRASS_FOR_HERDS && self.herds.len() < herd_cap && u(1, 0) < 0.08 {
            // Settle on the best of a few random grassy spots.
            let mut best: Option<(f64, usize)> = None;
            for k in 0..40 {
                let i = ((u(2, k) * n as f64) as usize).min(n - 1);
                if w.state(i) != Cell::Grass {
                    continue;
                }
                let (x, y) = grid.center(i);
                let f = forage(w, [x, y], 3);
                if best.is_none_or(|(b, _)| f > b) {
                    best = Some((f, i));
                }
            }
            if let Some((_, i)) = best {
                let (x, y) = grid.center(i);
                self.herds.push(Herd {
                    kind: Grazer::for_biome(w.biome(i)),
                    pos: [x, y],
                    prev: [x, y],
                    size: HERD_START,
                    heading: u(3, 0) * std::f64::consts::TAU,
                    id: self.next_id,
                });
                self.next_id += 1;
            }
        }
        let animals: f64 = self.herds.iter().map(|h| h.size).sum();
        if animals >= ANIMALS_FOR_WOLVES && self.packs.len() < self.herds.len().div_ceil(3) && u(4, 0) < 0.05 {
            let h = &self.herds[((u(5, 0) * self.herds.len() as f64) as usize).min(self.herds.len() - 1)];
            let a = u(6, 0) * std::f64::consts::TAU;
            let pos = [h.pos[0] + 20.0 * a.cos(), h.pos[1] + 20.0 * a.sin()];
            self.packs.push(Pack { pos, prev: pos, size: PACK_START, heading: a, id: self.next_id });
            self.next_id += 1;
        }
        let (bx0, by0, bx1, by1) = grid.world_bounds();
        let clamp = |p: [f64; 2]| [p[0].clamp(bx0, bx1), p[1].clamp(by0, by1)];
        // Herds move to the best pasture within a year's trek: forage, plus
        // water within reach, minus the fear of wolves.
        let packs: Vec<[f64; 2]> = self.packs.iter().map(|p| p.pos).collect();
        let rivers = w.params().rivers > 0.0;
        for h in self.herds.iter_mut() {
            h.prev = h.pos;
            let score = |p: [f64; 2]| {
                let Some(i) = grid.pick(p[0], p[1]) else { return f64::NEG_INFINITY };
                if w.is_channel(i) {
                    return f64::NEG_INFINITY;
                }
                let thirst = if rivers {
                    w.river_distance(i).map_or(12.0, |d| (d as f64 - 3.0).max(0.0)) * 0.6
                } else {
                    0.0
                };
                let fear: f64 = packs.iter().map(|&q| (1.0 - dist(p, q) / (HUNT_REACH * 2.5)).max(0.0)).sum();
                forage(w, p, 3) - thirst - 8.0 * fear
            };
            let mut best = (score(h.pos), h.pos, h.heading);
            for k in 0..8 {
                let a = std::f64::consts::TAU * (k as f64 + u(h.id + 10, k)) / 8.0;
                let p = clamp([h.pos[0] + HERD_TREK * step * a.cos(), h.pos[1] + HERD_TREK * step * a.sin()]);
                let s = score(p);
                if s > best.0 {
                    best = (s, p, a);
                }
            }
            h.pos = best.1;
            h.heading = best.2;
            // Forage per head sets growth or decline.
            let f = forage(w, h.pos, GRAZE_RADIUS);
            let per_head = f / h.size.max(1.0);
            h.size += HERD_GROWTH * h.size * (per_head / FORAGE_PER_HEAD - 1.0).clamp(-1.0, 1.0);
            h.size = h.size.min(HERD_MAX);
            if let Some(i) = grid.pick(h.pos[0], h.pos[1]) {
                h.kind = Grazer::for_biome(w.biome(i));
            }
        }
        // Packs chase the nearest herd and hunt it when close.
        for p in self.packs.iter_mut() {
            p.prev = p.pos;
            let target = self
                .herds
                .iter_mut()
                .min_by(|a, b| dist(a.pos, p.pos).partial_cmp(&dist(b.pos, p.pos)).unwrap());
            let mut kills = 0.0;
            if let Some(h) = target {
                let d = dist(h.pos, p.pos);
                let reach = PACK_TREK * step;
                let t = (reach / d.max(1e-9)).min(1.0);
                p.heading = (h.pos[1] - p.pos[1]).atan2(h.pos[0] - p.pos[0]);
                // Stop just short of the herd (wolves shadow it).
                let stop = (1.0 - 2.0 / d.max(2.0)).max(0.0);
                p.pos = clamp([p.pos[0] + (h.pos[0] - p.pos[0]) * t * stop, p.pos[1] + (h.pos[1] - p.pos[1]) * t * stop]);
                if dist(h.pos, p.pos) < HUNT_REACH {
                    kills = (KILL_SHARE * h.size).min(KILLS_PER_WOLF * p.size);
                    h.size -= kills;
                }
            }
            p.size += WOLF_GAIN * kills - WOLF_LOSS * p.size;
        }
        self.herds.retain(|h| h.size >= 3.0);
        self.packs.retain(|p| p.size >= 2.0);
        // The fields the ecology reads.
        self.pressure.fill(0.0);
        self.fear.fill(0.0);
        for h in &self.herds {
            add_disk(grid, &mut self.pressure, h.pos, GRAZE_RADIUS, (h.size / 40.0) as f32);
        }
        for p in &self.packs {
            add_disk(grid, &mut self.fear, p.pos, FEAR_RADIUS, 0.8);
        }
    }
}
