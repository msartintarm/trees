//! Cell states and the per-tick ecology rules, modeled on real grass–tree
//! spatial dynamics:
//!
//! - **Seed rain, not a flat radius**: each *mature* tree deposits a
//!   distance kernel over its disk — low right at the trunk (the
//!   Janzen-Connell effect: pests and pathogens concentrate under the
//!   parent), peaking one ring out, decaying beyond — and contributions from
//!   multiple trees compound: `p = 1 − (1 − tree_growth_p)^W`.
//! - **Maturity**: trees younger than `tree_maturity_age` produce no seed,
//!   cast no shade, and exert no crowding — so colonization fronts advance
//!   at generation speed, not sapling speed.
//! - **Graded shading**: canopy proximity scales understory growth — grass
//!   AND tree seedlings (gap-phase regeneration: shade-intolerant
//!   recruitment happens in openings and at edges, never under a closed
//!   canopy). Fully suppressive adjacent to a mature tree at default
//!   strength, partial at distance 2–3, and seed rain saturates at
//!   [`SEED_RAIN_CAP`] so a ring of parents can't force-fill a gap.
//! - **Clonal spread**: grass mostly creeps — a small spontaneous seed rate
//!   plus a bonus per adjacent grass tile, so swards advance as fronts.
//! - **Sod competition**: established grass resists tree seedlings
//!   (`sod_factor < 1`); values above 1 model nurse-plant facilitation.
//! - **Self-thinning**: a tree with more than [`CROWD_FREE`] mature
//!   neighbors suffers extra per-tick mortality per excess neighbor.
//! - **Fire**: grass is fuel. Rare ignitions spread tile-to-tile through
//!   contiguous grass and immature trees; mature trees survive and act as
//!   firebreaks. Fire is the classic mechanism that keeps real savannas
//!   from succeeding to closed forest.
//!
//! Tick order: decay → weather → field rebuild (from start-of-tick state)
//! → fire → death → growth.
//!
//! **Climate**: two smooth deterministic signals — sun and moisture — drift
//! through wet years and droughts (period ~15–60 s at 1×). They scale
//! growth, mortality, flammability, and storm frequency, all calibrated so
//! the neutral climate (sun = moisture = 0.5) multiplies by exactly 1.
//!
//! **Mortality is a hazard, not a lifespan**: every plant rolls a per-tick
//! death chance of `stress / mean_life` (drought and heat raise stress, wet
//! spells lower it; crowding adds on for trees). Lifetimes are therefore
//! geometric — a tree can in principle live forever, but the survival curve
//! decays exponentially, so in equilibrium very few ever do.

use super::hex;
use super::rng::{self, Stream};

pub const GRASS_SEED_P: f64 = 0.0;
pub const GRASS_CLONAL_P: f64 = 0.08;
pub const SHADE_STRENGTH: f64 = 1.0;
pub const TREE_GROWTH_P: f64 = 0.002;
/// A mature tree's seed rain reaches this hex distance.
pub const TREE_RANGE: i32 = 3;
pub const TREE_MATURITY_AGE: u32 = 40;
pub const SOD_FACTOR: f64 = 0.4;
pub const CROWDING_P: f64 = 0.002;
/// Mean lifetime under neutral climate; the per-tick hazard is
/// `stress / mean_life`, so lifetimes are unbounded geometrics.
pub const GRASS_MEAN_LIFE: u32 = 30;
pub const TREE_MEAN_LIFE: u32 = 250;
/// Amplitude of the climate oscillation (0 = eternal neutral weather).
pub const CLIMATE_SWING: f64 = 0.7;
pub const FIRE_IGNITION_P: f64 = 0.0;
pub const FIRE_SPREAD_P: f64 = 0.85;
pub const NUTRIENT_BOOST: f64 = 1.0;
/// Initial scatter probabilities at world creation.
pub const SEED_TREE_P: f64 = 0.0;
pub const SEED_GRASS_P: f64 = 0.0;

/// A tree tolerates this many mature neighbors before crowding mortality.
pub const CROWD_FREE: u8 = 2;
/// Accumulated seed rain saturates here — a gap only fits so many seedlings
/// however many parents ring it (recruitment saturation).
pub const SEED_RAIN_CAP: f64 = 4.0;
/// Ticks a tile stays alight once ignited (it spreads each of them).
const BURN_TICKS: u8 = 2;
/// Decomposition. Dead plants stand as husks that saprotrophic mycelium
/// breaks down: colonization builds per tick (faster beside other colonized
/// wood — inoculum proximity), rot advances at a rate that scales with it,
/// and the husk crumbles when rot reaches the wood's total. Charred wood
/// resists colonization (charcoal is nearly inert), so burn snags outlast
/// natural deadfall unless surrounded by rot. Husks BLOCK regrowth on their
/// tile until fully decomposed — the one sim-visible effect of this layer.
/// Ash is still a purely visual ground stain.
pub const TREE_ROT_UNITS: u16 = 600;
pub const GRASS_ROT_UNITS: u16 = 150;
/// Abiotic weathering: rot per tick with zero mycelium.
pub const ROT_BASE: u16 = 2;
pub const MYC_MAX: u8 = 96;
/// Colonization level at which a husk inoculates its neighbors.
pub const MYC_ESTABLISHED: u8 = 24;
/// Colonization level at which fruiting bodies (mushrooms) appear.
pub const MYC_FRUITING: u8 = 48;
/// Charred wood colonizes this many times slower.
pub const CHAR_RESISTANCE: u32 = 4;
pub const ASH_TICKS: u8 = 60;

/// Nutrient cycling. Each tile holds a fertility store (0..=NUTRIENT_CAP)
/// that decomposition returns to the soil — a rotted tree is a rich pulse,
/// thatch a small one, charcoal keeps most of its carbon locked — and fire
/// mineralizes standing biomass into an immediate ash flush. Fertility
/// multiplies establishment (both grass and trees) by up to
/// `1 + nutrient_boost`, living plants draw the store down as they hold the
/// tile, and idle soil leaches slowly back to baseline. Rendered as the
/// soil darkening toward loam.
pub const NUTRIENT_CAP: u16 = 1000;
pub const NUTRIENTS_FROM_TREE: u16 = 800;
pub const NUTRIENTS_FROM_GRASS: u16 = 150;
/// Charred wood returns only a fraction of its nutrients when it rots.
pub const NUTRIENTS_FROM_CHARRED_TREE: u16 = 200;
pub const NUTRIENTS_FROM_CHARRED_GRASS: u16 = 40;
/// Immediate mineralization when a burning plant burns out.
pub const ASH_FLUSH_TREE: u16 = 250;
pub const ASH_FLUSH_GRASS: u16 = 100;
/// Per-tick drawdown: occupied tiles feed their plant, idle soil leaches.
pub const NUTRIENT_DRAIN_OCCUPIED: u16 = 4;
pub const NUTRIENT_DRAIN_IDLE: u16 = 1;

/// Weather. Thunderclouds spawn off-map, drift across the world, and exit.
/// Under a cloud, the inner rain core soaks tiles (wet fuel cannot ignite,
/// burning tiles are doused) and wet soil grows faster; lightning strikes
/// anywhere under the cloud, so bolts at the dry edge can start fires the
/// storm's own rain never reaches — the wet/dry-lightning split of real
/// convective storms.
pub const STORM_RATE: f64 = 0.003;
pub const STORM_LIGHTNING_P: f64 = 0.06;
/// Fraction of the cloud radius that actually rains.
pub const RAIN_CORE: f64 = 0.75;
/// Ticks a tile stays wet after rain passes.
pub const WET_TICKS: u8 = 30;
/// Ticks a lightning bolt stays visible.
const BOLT_TICKS: u8 = 2;

/// A thundercloud crossing the map: pure linear drift from its spawn state,
/// so its center at any (fractional) tick is exactly reconstructible — the
/// renderer interpolates cloud motion from this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Storm {
    pub origin: [f64; 2],
    pub vel: [f64; 2],
    pub radius: f64,
    pub spawned: u64,
}

impl Storm {
    pub fn center(&self, t: f64) -> [f64; 2] {
        let dt = t - self.spawned as f64;
        [self.origin[0] + self.vel[0] * dt, self.origin[1] + self.vel[1] * dt]
    }
}

/// Seed-rain kernel weight by hex distance from a mature tree: the
/// Janzen-Connell dip at distance 1, recruitment peak at 2, exponential
/// decay beyond.
fn kernel_weight(d: i32) -> f64 {
    match d {
        0 => 0.0,
        1 => 0.25,
        2 => 1.0,
        d => (-0.9 * (d as f64 - 2.0)).exp(),
    }
}

/// Fraction of grass growth removed at full `shade_strength`, by distance to
/// the nearest mature tree. Distance 1 is fully suppressed at strength 1 —
/// the original "no grass adjacent to a tree" rule survives as the default.
fn shade_suppression(d: u8) -> f64 {
    match d {
        0 | 1 => 1.0,
        2 => 0.55,
        3 => 0.2,
        _ => 0.0,
    }
}

/// Everything tunable about the ecology, adjustable at runtime from the UI.
/// Growth/lifespan fields apply from the next tick (lifespans only to plants
/// born after the change — a plant keeps the lifespan it was dealt at
/// birth); the seeding fields apply on the next reseed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Spontaneous grass sprout probability per bare tile per tick.
    pub grass_seed_p: f64,
    /// Extra grass probability per adjacent grass tile (clonal creep).
    pub grass_clonal_p: f64,
    /// 0 = canopy casts no shade, 1 = full suppression curve.
    pub shade_strength: f64,
    /// Tree establishment probability at the kernel peak from one mature tree.
    pub tree_growth_p: f64,
    pub tree_range: i32,
    /// Age below which a tree neither seeds, shades, crowds, nor resists fire.
    pub tree_maturity_age: u32,
    /// Multiplier on tree establishment where grass already grows
    /// (<1 = sod competition, >1 = nurse-plant facilitation).
    pub sod_factor: f64,
    /// Extra per-tick death probability per mature neighbor above CROWD_FREE.
    pub crowding_p: f64,
    /// Mean lifetime in ticks under neutral climate (hazard = stress/mean).
    pub grass_mean_life: u32,
    pub tree_mean_life: u32,
    /// 0 = constant mild climate, 1 = full wet-year/drought swings.
    pub climate_swing: f64,
    /// Lightning: per grass tile per tick ignition probability.
    pub fire_ignition_p: f64,
    /// Chance per tick that a burning tile ignites a flammable neighbor.
    pub fire_spread_p: f64,
    /// Establishment multiplier at full soil fertility is `1 + this`.
    pub nutrient_boost: f64,
    /// Per-tick chance a new thundercloud spawns at the map edge.
    pub storm_rate: f64,
    /// Per-tick chance each active storm throws a lightning bolt.
    pub storm_lightning_p: f64,
    pub seed_tree_p: f64,
    pub seed_grass_p: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            grass_seed_p: GRASS_SEED_P,
            grass_clonal_p: GRASS_CLONAL_P,
            shade_strength: SHADE_STRENGTH,
            tree_growth_p: TREE_GROWTH_P,
            tree_range: TREE_RANGE,
            tree_maturity_age: TREE_MATURITY_AGE,
            sod_factor: SOD_FACTOR,
            crowding_p: CROWDING_P,
            grass_mean_life: GRASS_MEAN_LIFE,
            tree_mean_life: TREE_MEAN_LIFE,
            climate_swing: CLIMATE_SWING,
            fire_ignition_p: FIRE_IGNITION_P,
            fire_spread_p: FIRE_SPREAD_P,
            nutrient_boost: NUTRIENT_BOOST,
            storm_rate: STORM_RATE,
            storm_lightning_p: STORM_LIGHTNING_P,
            seed_tree_p: SEED_TREE_P,
            seed_grass_p: SEED_GRASS_P,
        }
    }
}

impl Params {
    /// Clamp everything into ranges the sim can safely run with.
    pub fn sanitized(mut self) -> Params {
        let prob = |v: f64, fallback: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { fallback };
        self.grass_seed_p = prob(self.grass_seed_p, GRASS_SEED_P);
        self.grass_clonal_p = prob(self.grass_clonal_p, GRASS_CLONAL_P);
        self.shade_strength = prob(self.shade_strength, SHADE_STRENGTH);
        self.tree_growth_p = prob(self.tree_growth_p, TREE_GROWTH_P);
        self.crowding_p = prob(self.crowding_p, CROWDING_P);
        self.fire_ignition_p = prob(self.fire_ignition_p, FIRE_IGNITION_P);
        self.fire_spread_p = prob(self.fire_spread_p, FIRE_SPREAD_P);
        self.nutrient_boost = if self.nutrient_boost.is_finite() {
            self.nutrient_boost.clamp(0.0, 5.0)
        } else {
            NUTRIENT_BOOST
        };
        self.storm_rate = prob(self.storm_rate, STORM_RATE);
        self.storm_lightning_p = prob(self.storm_lightning_p, STORM_LIGHTNING_P);
        self.seed_tree_p = prob(self.seed_tree_p, SEED_TREE_P);
        self.seed_grass_p = prob(self.seed_grass_p, SEED_GRASS_P);
        self.sod_factor =
            if self.sod_factor.is_finite() { self.sod_factor.clamp(0.0, 5.0) } else { SOD_FACTOR };
        self.climate_swing = prob(self.climate_swing, CLIMATE_SWING);
        self.tree_range = self.tree_range.clamp(1, 8);
        self.tree_maturity_age = self.tree_maturity_age.min(10_000);
        self.grass_mean_life = self.grass_mean_life.clamp(1, 100_000);
        self.tree_mean_life = self.tree_mean_life.clamp(1, 100_000);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cell {
    Bare = 0,
    Grass = 1,
    Tree = 2,
}

/// Tree varieties, each occupying a distinct strategy corner. Traits are a
/// fixed archetype table (multipliers on the global tree params), NOT panel
/// knobs — the panel keeps tuning the whole forest, species stay curated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Species {
    /// Savanna pioneer: the baseline — quick, drought-hardy, short-lived.
    Acacia = 0,
    /// Climax hardwood: slow, late-maturing, very long-lived; its seedlings
    /// tolerate shade, so it establishes under canopies pioneers can't.
    Oak = 1,
    /// Fire-embracer: fast, stays flammable well past maturity, but its
    /// seeds germinate on ash (serotiny) — burns feed its own recruitment.
    Pine = 2,
    /// Water-lover: booms on wet ground and wet seasons, suffers brutally in
    /// drought; lives fast.
    Willow = 3,
}

pub const SPECIES_COUNT: usize = 4;

impl Species {
    pub fn from_u8(v: u8) -> Species {
        match v {
            1 => Species::Oak,
            2 => Species::Pine,
            3 => Species::Willow,
            _ => Species::Acacia,
        }
    }

    pub fn traits(self) -> &'static SpeciesTraits {
        &SPECIES_TABLE[self as usize]
    }
}

/// Per-species multipliers on the global tree parameters. All 1.0 = exactly
/// the pre-species tree.
pub struct SpeciesTraits {
    pub name: &'static str,
    /// × tree_growth_p (establishment at the kernel peak).
    pub growth: f64,
    /// × tree_maturity_age (seeding/shading/crowding begin here).
    pub maturity: f64,
    /// × tree_mean_life.
    pub mean_life: f64,
    /// Fraction of shade its seedlings ignore (0 = full light gate).
    pub shade_tolerance: f64,
    /// × establishment on ash-bearing tiles (serotiny when > 1).
    pub ash_affinity: f64,
    /// Exponent on drought stress (>1 = suffers, <1 = hardy). Neutral
    /// climate stress is exactly 1, so this preserves calibration.
    pub drought_sensitivity: f64,
    /// Exponent on the water growth factor (>1 = booms when wet).
    pub water_affinity: f64,
    /// × maturity age until which the tree stays flammable (fireproofing).
    pub fireproof: f64,
    /// × crowding_p: big canopies need space (climax trees self-thin hard).
    pub crowding: f64,
    /// Max hex distance its seeds travel (capped by the tree_range param).
    /// Heavy acorns barely move; winged and fluffy seeds ride the wind.
    pub dispersal: i32,
    /// × TREE_ROT_UNITS for its dead wood (hardwood rots slowly).
    pub rot: f64,
    /// × NUTRIENTS_FROM_TREE returned on decomposition.
    pub nutrients: f64,
}

pub static SPECIES_TABLE: [SpeciesTraits; SPECIES_COUNT] = [
    SpeciesTraits {
        name: "Acacia",
        growth: 1.0,
        maturity: 1.0,
        mean_life: 1.0,
        shade_tolerance: 0.0,
        ash_affinity: 1.0,
        drought_sensitivity: 0.7,
        water_affinity: 0.8,
        fireproof: 1.0,
        crowding: 1.0,
        dispersal: 3,
        rot: 1.0,
        nutrients: 1.0,
    },
    SpeciesTraits {
        name: "Oak",
        growth: 0.45,
        maturity: 2.5,
        mean_life: 2.2,
        shade_tolerance: 0.15,
        ash_affinity: 1.0,
        drought_sensitivity: 1.25,
        water_affinity: 1.0,
        fireproof: 0.8,
        crowding: 4.0,
        dispersal: 2,
        rot: 1.8,
        nutrients: 1.5,
    },
    SpeciesTraits {
        name: "Pine",
        growth: 1.3,
        maturity: 0.8,
        mean_life: 0.8,
        shade_tolerance: 0.0,
        ash_affinity: 3.0,
        drought_sensitivity: 1.0,
        water_affinity: 1.0,
        fireproof: 2.5,
        crowding: 1.0,
        dispersal: 3,
        rot: 1.2,
        nutrients: 0.8,
    },
    SpeciesTraits {
        name: "Willow",
        growth: 1.4,
        maturity: 0.75,
        mean_life: 0.7,
        shade_tolerance: 0.0,
        ash_affinity: 1.0,
        drought_sensitivity: 1.6,
        water_affinity: 2.0,
        fireproof: 1.0,
        crowding: 1.0,
        dispersal: 3,
        rot: 0.7,
        nutrients: 1.0,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Brush {
    Clear = 0,
    Grass = 1,
    Tree = 2,
    /// Ignite the tile if it holds fuel (grass or an immature tree).
    Fire = 3,
}

impl Brush {
    pub fn from_u8(v: u8) -> Option<Brush> {
        match v {
            0 => Some(Brush::Clear),
            1 => Some(Brush::Grass),
            2 => Some(Brush::Tree),
            3 => Some(Brush::Fire),
            _ => None,
        }
    }
}

/// A dead plant still standing on a tile, purely for rendering: what it
/// was, how burnt, how far through crumbling, and how big/wilted it was
/// when it died (so the husk starts at exactly the size the plant rendered).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Remains {
    pub tree: bool,
    pub charred: bool,
    /// Species of a tree husk (meaningless for thatch).
    pub species: Species,
    /// Fraction of the husk still standing: 1 fresh → 0 gone.
    pub remaining: f32,
    /// Last tick's `remaining` (for smooth render interpolation).
    pub remaining_prev: f32,
    /// Mycelium colonization, 0..1 (mushrooms fruit above ~0.5).
    pub myc: f32,
    pub age_at_death: u32,
    /// `age / lifespan` when it died, in [0, 1] — 1 means it wilted out.
    pub life_ratio: f32,
}

pub struct World {
    seed: u64,
    state: Vec<Cell>,
    /// Tick each plant was born (meaningless for bare tiles).
    born: Vec<u64>,
    /// Tree species per tile (valid only where state == Tree).
    species: Vec<u8>,
    /// Ticks of burning remaining (0 = not on fire).
    burn: Vec<u8>,
    /// Husks: 0 none, 1 grass, 2 tree, +2 for charred variants.
    remains_code: Vec<u8>,
    /// Species of a tree husk (drives its silhouette and rot rate).
    remains_species: Vec<u8>,
    /// Accumulated rot toward the wood's *_ROT_UNITS total.
    remains_rot: Vec<u16>,
    /// Mycelium colonization level (0..=MYC_MAX), nonzero only on husks.
    myc: Vec<u8>,
    remains_age: Vec<u32>,
    /// Quantized life_ratio at death (0..=255).
    remains_ratio: Vec<u8>,
    /// Ash ticks remaining on the ground after a burn-out.
    ash: Vec<u8>,
    /// Soil fertility store, 0..=NUTRIENT_CAP.
    nutrients: Vec<u16>,
    /// Ticks of rain-wetness remaining per tile.
    wet: Vec<u8>,
    /// Ticks a lightning bolt remains visible per tile.
    bolt: Vec<u8>,
    storms: Vec<Storm>,
    /// Climate cache for the current tick (updated at the top of `step`).
    sun: f64,
    moisture: f64,
    /// Accumulated seed-rain kernel weight from mature trees, per tile and
    /// per species — offspring inherit their parent's kind.
    seed_rain: Vec<[f32; SPECIES_COUNT]>,
    /// Hex distance to the nearest mature tree within range (255 = none).
    tree_dist: Vec<u8>,
    /// Mature-tree count at distance 1 (crowding pressure).
    mature_nbrs: Vec<u8>,
    /// Grass count at distance 1 (clonal spread pressure).
    grass_nbrs: Vec<u8>,
    /// Axial offsets with distance for the ≤ params.tree_range disk.
    range_disk: Vec<(i32, i32, i32)>,
    params: Params,
}

impl World {
    pub fn new(seed: u64) -> World {
        World::with_params(seed, Params::default())
    }

    pub fn with_params(seed: u64, params: Params) -> World {
        let params = params.sanitized();
        let mut w = World {
            seed,
            state: vec![Cell::Bare; hex::CELLS],
            born: vec![0; hex::CELLS],
            species: vec![0; hex::CELLS],
            burn: vec![0; hex::CELLS],
            remains_code: vec![0; hex::CELLS],
            remains_species: vec![0; hex::CELLS],
            remains_rot: vec![0; hex::CELLS],
            myc: vec![0; hex::CELLS],
            remains_age: vec![0; hex::CELLS],
            remains_ratio: vec![0; hex::CELLS],
            ash: vec![0; hex::CELLS],
            nutrients: vec![0; hex::CELLS],
            wet: vec![0; hex::CELLS],
            bolt: vec![0; hex::CELLS],
            storms: Vec::new(),
            sun: 0.5,
            moisture: 0.5,
            seed_rain: vec![[0.0; SPECIES_COUNT]; hex::CELLS],
            tree_dist: vec![255; hex::CELLS],
            mature_nbrs: vec![0; hex::CELLS],
            grass_nbrs: vec![0; hex::CELLS],
            range_disk: hex::disk(params.tree_range),
            params,
        };
        for i in 0..hex::CELLS {
            if rng::uniform01(seed, i as u32, 0, Stream::Seeding) < w.params.seed_tree_p {
                let sp = Species::from_u8(
                    (rng::hash(seed, i as u32, 0, Stream::SpeciesChoice) % SPECIES_COUNT as u64)
                        as u8,
                );
                w.plant_tree(i, sp, 0);
            } else if rng::uniform01(seed, i as u32, 0, Stream::SeedingGrass) < w.params.seed_grass_p {
                w.plant(i, Cell::Grass, 0);
            }
        }
        let (sun, moisture) = w.climate_at(0);
        w.sun = sun;
        w.moisture = moisture;
        w
    }

    /// The deterministic climate signals at a tick: (sun, moisture), each in
    /// [0.02, 0.98], drifting as two incommensurate sinusoids whose phases
    /// come from the seed. `climate_swing` scales the amplitude.
    pub fn climate_at(&self, tick: u64) -> (f64, f64) {
        let tau = std::f64::consts::TAU;
        let t = tick as f64;
        let phase = |k: u32| {
            (rng::hash(self.seed, k, 0, Stream::Climate) % 10_000) as f64 / 10_000.0 * tau
        };
        let a = 0.5 * self.params.climate_swing;
        let sun = 0.5
            + a * (0.6 * (tau * t / 613.0 + phase(0)).sin() + 0.4 * (tau * t / 149.0 + phase(1)).sin());
        let moisture = 0.5
            + a * (0.6 * (tau * t / 431.0 + phase(2)).sin() + 0.4 * (tau * t / 113.0 + phase(3)).sin());
        (sun.clamp(0.02, 0.98), moisture.clamp(0.02, 0.98))
    }

    pub fn sun(&self) -> f64 {
        self.sun
    }

    pub fn moisture(&self) -> f64 {
        self.moisture
    }

    /// Effective water available to a tile: storm-soaked ground is saturated
    /// regardless of the season.
    fn tile_water(&self, index: usize) -> f64 {
        if self.wet[index] > 0 { 1.0 } else { self.moisture }
    }

    /// Drought index in [0, ~1.9]: 0.5 in neutral climate. Heat with no
    /// water pushes it up; saturation pulls it to zero.
    fn drought(&self, water: f64) -> f64 {
        (1.0 - water) * (0.4 + 1.2 * self.sun)
    }

    /// How visibly browned vegetation is by the current conditions, 0..1.
    pub fn browning(&self, index: usize) -> f32 {
        ((self.drought(self.tile_water(index)) - 0.7) / 0.8).clamp(0.0, 1.0) as f32
    }

    pub fn params(&self) -> Params {
        self.params
    }

    /// Swap the tunables in place; takes effect from the next tick.
    pub fn set_params(&mut self, params: Params) {
        let params = params.sanitized();
        if params.tree_range != self.params.tree_range {
            self.range_disk = hex::disk(params.tree_range);
        }
        self.params = params;
    }

    pub fn state(&self, index: usize) -> Cell {
        self.state[index]
    }

    pub fn burning(&self, index: usize) -> bool {
        self.burn[index] > 0
    }

    pub fn burning_count(&self) -> u32 {
        self.burn.iter().filter(|&&b| b > 0).count() as u32
    }

    /// The husk standing on a tile, if any. Blocks regrowth until it rots
    /// away; everything else about it is rendering data.
    pub fn remains(&self, index: usize) -> Option<Remains> {
        if self.remains_code[index] == 0 {
            return None;
        }
        let code = self.remains_code[index];
        let tree = code == 2 || code == 4;
        let sp = Species::from_u8(self.remains_species[index]);
        let total = if tree {
            (TREE_ROT_UNITS as f64 * sp.traits().rot) as f32
        } else {
            GRASS_ROT_UNITS as f32
        };
        let rot = self.remains_rot[index];
        // Reconstruct last tick's rot from the current rate — close enough
        // for a smooth shader lerp without storing history.
        let rate = ROT_BASE + (self.myc[index] / 8) as u16;
        let prev_rot = rot.saturating_sub(rate);
        Some(Remains {
            tree,
            charred: code > 2,
            species: sp,
            remaining: (1.0 - rot as f32 / total).max(0.0),
            remaining_prev: (1.0 - prev_rot as f32 / total).max(0.0),
            myc: self.myc[index] as f32 / MYC_MAX as f32,
            age_at_death: self.remains_age[index],
            life_ratio: self.remains_ratio[index] as f32 / 255.0,
        })
    }

    /// How fresh the ash on this tile is: 1 = just burned, 0 = none.
    pub fn ash_ratio(&self, index: usize) -> f32 {
        self.ash[index] as f32 / ASH_TICKS as f32
    }

    /// Soil fertility, 0..1 of the cap.
    pub fn nutrient_ratio(&self, index: usize) -> f32 {
        self.nutrients[index] as f32 / NUTRIENT_CAP as f32
    }

    /// Rain wetness, 1 = just soaked, 0 = dry.
    pub fn wet_ratio(&self, index: usize) -> f32 {
        self.wet[index] as f32 / WET_TICKS as f32
    }

    /// A lightning bolt is striking (or just struck) this tile.
    pub fn bolt_active(&self, index: usize) -> bool {
        self.bolt[index] > 0
    }

    pub fn storms(&self) -> &[Storm] {
        &self.storms
    }

    /// Inject a storm directly (tests, and a possible future storm brush).
    pub fn spawn_storm(&mut self, origin: [f64; 2], vel: [f64; 2], radius: f64, tick: u64) {
        self.storms.push(Storm { origin, vel, radius: radius.clamp(2.0, 20.0), spawned: tick });
    }

    fn add_nutrients(&mut self, index: usize, amount: u16) {
        self.nutrients[index] = (self.nutrients[index] + amount).min(NUTRIENT_CAP);
    }

    fn leave_remains(&mut self, index: usize, charred: bool, tick: u64) {
        let tree = self.state[index] == Cell::Tree;
        self.remains_code[index] = if tree { 2 } else { 1 } + if charred { 2 } else { 0 };
        self.remains_species[index] = self.species[index];
        self.remains_rot[index] = 0;
        self.myc[index] = 0; // fresh dead wood starts uncolonized
        self.remains_age[index] = self.age(index, tick).min(u32::MAX as u64) as u32;
        // How wilted it looked when it died (drought browning).
        self.remains_ratio[index] = (self.browning(index) * 255.0) as u8;
    }

    fn clear_remains(&mut self, index: usize) {
        self.remains_code[index] = 0;
        self.remains_rot[index] = 0;
        self.myc[index] = 0;
    }

    /// Age of the plant at `index` as of `tick` (0 for bare tiles).
    pub fn age(&self, index: usize, tick: u64) -> u64 {
        match self.state[index] {
            Cell::Bare => 0,
            _ => tick.saturating_sub(self.born[index]),
        }
    }

    pub fn species(&self, index: usize) -> Species {
        Species::from_u8(self.species[index])
    }

    /// Maturity age for the tree standing on `index`.
    fn maturity_age(&self, index: usize) -> u64 {
        (self.params.tree_maturity_age as f64 * self.species(index).traits().maturity) as u64
    }

    fn is_mature(&self, index: usize, tick: u64) -> bool {
        self.age(index, tick) >= self.maturity_age(index)
    }

    /// [bare, grass, tree] tile counts.
    pub fn counts(&self) -> [u32; 3] {
        let mut c = [0u32; 3];
        for &s in &self.state {
            c[s as usize] += 1;
        }
        c
    }

    fn plant(&mut self, index: usize, kind: Cell, tick: u64) {
        debug_assert!(kind != Cell::Bare, "plant() only grows things");
        self.state[index] = kind;
        self.born[index] = tick;
        self.clear_remains(index);
    }

    fn plant_tree(&mut self, index: usize, sp: Species, tick: u64) {
        self.species[index] = sp as u8;
        self.plant(index, Cell::Tree, tick);
    }

    /// Fuel = grass, or a tree that hasn't yet grown fireproof bark. Pines
    /// stay flammable well past maturity — that's their gamble.
    fn flammable(&self, index: usize, tick: u64) -> bool {
        match self.state[index] {
            Cell::Grass => true,
            Cell::Tree => {
                let tr = self.species(index).traits();
                let fireproof_age =
                    (self.params.tree_maturity_age as f64 * tr.maturity * tr.fireproof) as u64;
                self.age(index, tick) < fireproof_age
            }
            Cell::Bare => false,
        }
    }

    /// Brush stroke choosing the tree species (the panel's species picker).
    pub fn paint_species(&mut self, index: usize, brush: Brush, sp: Species, tick: u64) {
        if brush == Brush::Tree {
            self.plant_tree(index, sp, tick);
        } else {
            self.paint(index, brush, tick);
        }
    }

    /// Apply a user brush stroke. Deterministic from seed + click history.
    /// Tree strokes default to Acacia; see [`World::paint_species`].
    pub fn paint(&mut self, index: usize, brush: Brush, tick: u64) {
        match brush {
            Brush::Clear => {
                self.state[index] = Cell::Bare;
                self.burn[index] = 0;
                self.clear_remains(index);
                self.ash[index] = 0;
            }
            Brush::Grass => self.plant(index, Cell::Grass, tick),
            Brush::Tree => self.plant_tree(index, Species::Acacia, tick),
            Brush::Fire => {
                // A deliberate torch lights ANY plant, mature trees included
                // — bark resistance only protects against spreading grass
                // fire, not the player. Bare ground holds no fuel.
                if self.state[index] != Cell::Bare {
                    self.burn[index] = BURN_TICKS;
                }
            }
        }
    }

    /// Rebuild the neighborhood fields from start-of-tick state.
    fn rebuild_fields(&mut self, tick: u64) {
        self.seed_rain.fill([0.0; SPECIES_COUNT]);
        self.tree_dist.fill(255);
        self.mature_nbrs.fill(0);
        self.grass_nbrs.fill(0);
        for i in 0..hex::CELLS {
            match self.state[i] {
                Cell::Bare => {}
                Cell::Grass => {
                    let (q, r) = hex::index_to_axial(i);
                    for (dq, dr) in hex::NEIGHBORS {
                        if let Some(j) = hex::axial_to_index(q + dq, r + dr) {
                            self.grass_nbrs[j] += 1;
                        }
                    }
                }
                Cell::Tree => {
                    if !self.is_mature(i, tick) {
                        continue;
                    }
                    let sp = self.species[i] as usize;
                    let dispersal = SPECIES_TABLE[sp].dispersal;
                    let (q, r) = hex::index_to_axial(i);
                    for k in 0..self.range_disk.len() {
                        let (dq, dr, d) = self.range_disk[k];
                        if let Some(j) = hex::axial_to_index(q + dq, r + dr) {
                            if d <= dispersal {
                                self.seed_rain[j][sp] += kernel_weight(d) as f32;
                            }
                            self.tree_dist[j] = self.tree_dist[j].min(d as u8);
                            if d == 1 {
                                self.mature_nbrs[j] += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Spawn, drift, and apply thunderclouds: rain soaks and douses, bolts
    /// strike, exited storms dissipate.
    fn weather_pass(&mut self, tick: u64) {
        let p = self.params;
        let (min_x, min_y, max_x, max_y) = hex::world_bounds();
        let c = [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0];
        let world_r = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt() / 2.0;

        // Wet seasons brew storms; deep drought starves them.
        let spawn_rate = p.storm_rate * (0.2 + 1.6 * self.moisture);
        if rng::uniform01(self.seed, 0, tick, Stream::StormSpawn) < spawn_rate {
            let u = |k: u32| rng::uniform01(self.seed, k, tick, Stream::StormSpawn);
            let theta = std::f64::consts::TAU * u(1);
            let radius = 6.0 + 4.0 * u(2);
            let dir = [theta.cos(), theta.sin()];
            let origin = [c[0] + dir[0] * (world_r + radius), c[1] + dir[1] * (world_r + radius)];
            let skew = (u(3) - 0.5) * 0.8;
            let (sk_s, sk_c) = skew.sin_cos();
            let inward = [-(dir[0] * sk_c - dir[1] * sk_s), -(dir[0] * sk_s + dir[1] * sk_c)];
            let speed = 0.2 + 0.2 * u(4);
            self.spawn_storm(origin, [inward[0] * speed, inward[1] * speed], radius, tick);
        }

        // Dissipate storms that have crossed and left the world behind.
        self.storms.retain(|s| {
            let pos = s.center(tick as f64);
            let out = [pos[0] - c[0], pos[1] - c[1]];
            let dist = (out[0] * out[0] + out[1] * out[1]).sqrt();
            let leaving = out[0] * s.vel[0] + out[1] * s.vel[1] > 0.0;
            !(tick > s.spawned + 20 && dist > world_r + s.radius + 2.0 && leaving)
        });

        for idx in 0..self.storms.len() {
            let storm = self.storms[idx];
            let center = storm.center(tick as f64);
            let core = storm.radius * RAIN_CORE;
            for i in 0..hex::CELLS {
                let (q, r) = hex::index_to_axial(i);
                let (x, y) = hex::axial_to_world(q, r);
                let d = ((x - center[0]).powi(2) + (y - center[1]).powi(2)).sqrt();
                if d < core {
                    self.wet[i] = WET_TICKS;
                    self.burn[i] = 0; // the downpour douses open flame
                }
            }
            // Lightning: one bolt roll per storm per tick, striking a random
            // point under the cloud — possibly outside the rain core.
            let k = |j: u32| rng::uniform01(self.seed, idx as u32 * 16 + j, tick, Stream::StormBolt);
            if k(0) < p.storm_lightning_p {
                let rr = storm.radius * k(1).sqrt();
                let phi = std::f64::consts::TAU * k(2);
                if let Some(i) = hex::pick(center[0] + rr * phi.cos(), center[1] + rr * phi.sin()) {
                    self.bolt[i] = BOLT_TICKS;
                    if self.flammable(i, tick) && self.wet[i] == 0 {
                        self.burn[i] = BURN_TICKS;
                    }
                }
            }
        }
    }

    /// Spread from burning tiles, roll new ignitions, burn tiles down to bare.
    fn fire_pass(&mut self, tick: u64) {
        let p = self.params;
        let burning_now: Vec<usize> = (0..hex::CELLS).filter(|&i| self.burn[i] > 0).collect();
        if p.fire_ignition_p <= 0.0 && burning_now.is_empty() {
            return;
        }
        let mut caught = Vec::new();
        for &i in &burning_now {
            let (q, r) = hex::index_to_axial(i);
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = hex::axial_to_index(q + dq, r + dr) {
                    let spread = (p.fire_spread_p * (0.4 + 1.2 * (1.0 - self.moisture))).min(1.0);
                    if self.burn[j] == 0
                        && self.wet[j] == 0 // soaked fuel will not catch
                        && self.flammable(j, tick)
                        && rng::uniform01(self.seed, j as u32, tick, Stream::FireSpread) < spread
                    {
                        caught.push(j);
                    }
                }
            }
        }
        if p.fire_ignition_p > 0.0 {
            // Droughts quadruple spontaneous ignition; wet seasons quench it.
            let ignite = p.fire_ignition_p * (2.0 * (1.0 - self.moisture)).powi(2);
            for i in 0..hex::CELLS {
                if self.burn[i] == 0
                    && self.wet[i] == 0
                    && self.state[i] == Cell::Grass
                    && rng::uniform01(self.seed, i as u32, tick, Stream::FireIgnite) < ignite
                {
                    caught.push(i);
                }
            }
        }
        for &i in &burning_now {
            self.burn[i] -= 1;
            if self.burn[i] == 0 {
                if self.state[i] != Cell::Bare {
                    self.leave_remains(i, true, tick);
                    // Fire mineralizes much of the biomass on the spot.
                    self.add_nutrients(
                        i,
                        if self.state[i] == Cell::Tree { ASH_FLUSH_TREE } else { ASH_FLUSH_GRASS },
                    );
                }
                self.state[i] = Cell::Bare;
                self.ash[i] = ASH_TICKS;
            }
        }
        for j in caught {
            if self.state[j] != Cell::Bare {
                self.burn[j] = BURN_TICKS;
            }
        }
    }

    fn death_pass(&mut self, tick: u64) {
        for i in 0..hex::CELLS {
            if self.state[i] == Cell::Bare {
                continue;
            }
            // Constant-per-tick hazard scaled by weather stress: lifetimes
            // are unbounded, but survival decays exponentially. Neutral
            // climate ⇒ stress 1 ⇒ mean lifetime = the mean_life param.
            // Species bend the drought response via an exponent (neutral
            // stress is exactly 1, so calibration is preserved).
            let base_stress = 0.4 + 1.2 * self.drought(self.tile_water(i));
            let (stress, mean) = match self.state[i] {
                Cell::Grass => (base_stress, self.params.grass_mean_life as f64),
                _ => {
                    let tr = self.species(i).traits();
                    (
                        base_stress.powf(tr.drought_sensitivity),
                        self.params.tree_mean_life as f64 * tr.mean_life,
                    )
                }
            };
            let mut hazard = (stress / mean).min(1.0);
            if self.state[i] == Cell::Tree && self.mature_nbrs[i] > CROWD_FREE {
                let excess = (self.mature_nbrs[i] - CROWD_FREE) as f64;
                let cp = (self.params.crowding_p * self.species(i).traits().crowding).min(1.0);
                let p_crowd = 1.0 - (1.0 - cp).powf(excess);
                hazard = 1.0 - (1.0 - hazard) * (1.0 - p_crowd);
            }
            if rng::uniform01(self.seed, i as u32, tick, Stream::Mortality) < hazard {
                self.leave_remains(i, false, tick);
                self.state[i] = Cell::Bare;
                self.burn[i] = 0;
            }
        }
    }

    /// Decomposition: mycelium colonizes each husk (faster beside other
    /// colonized wood, much slower on charcoal), rot advances with
    /// colonization, and fully rotted husks crumble away. Deterministic —
    /// pure integer arithmetic over the grid, no RNG draws.
    fn decay_pass(&mut self) {
        for i in 0..hex::CELLS {
            if self.ash[i] > 0 {
                self.ash[i] -= 1;
            }
            if self.wet[i] > 0 {
                self.wet[i] -= 1;
            }
            if self.bolt[i] > 0 {
                self.bolt[i] -= 1;
            }
            let drain = if self.state[i] == Cell::Bare {
                NUTRIENT_DRAIN_IDLE
            } else {
                NUTRIENT_DRAIN_OCCUPIED
            };
            self.nutrients[i] = self.nutrients[i].saturating_sub(drain);
            if self.remains_code[i] == 0 {
                continue;
            }
            let (q, r) = hex::index_to_axial(i);
            let mut sources = 0u32;
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = hex::axial_to_index(q + dq, r + dr) {
                    if self.remains_code[j] != 0 && self.myc[j] >= MYC_ESTABLISHED {
                        sources += 1;
                    }
                }
            }
            let mut growth = 1 + 2 * sources;
            if self.remains_code[i] > 2 {
                growth /= CHAR_RESISTANCE;
            }
            self.myc[i] = ((self.myc[i] as u32 + growth).min(MYC_MAX as u32)) as u8;
            let rate = ROT_BASE + (self.myc[i] / 8) as u16;
            self.remains_rot[i] = self.remains_rot[i].saturating_add(rate);
            let total = if self.remains_code[i] == 2 || self.remains_code[i] == 4 {
                (TREE_ROT_UNITS as f64
                    * Species::from_u8(self.remains_species[i]).traits().rot) as u16
            } else {
                GRASS_ROT_UNITS
            };
            if self.remains_rot[i] >= total {
                // Decomposition complete: the biomass returns to the soil
                // (hardwoods hold more of it).
                let n_mult = Species::from_u8(self.remains_species[i]).traits().nutrients;
                self.add_nutrients(
                    i,
                    match self.remains_code[i] {
                        2 => (NUTRIENTS_FROM_TREE as f64 * n_mult) as u16,
                        1 => NUTRIENTS_FROM_GRASS,
                        4 => (NUTRIENTS_FROM_CHARRED_TREE as f64 * n_mult) as u16,
                        _ => NUTRIENTS_FROM_CHARRED_GRASS,
                    },
                );
                self.clear_remains(i);
            }
        }
    }

    fn growth_pass(&mut self, tick: u64) {
        let p = self.params;
        for i in 0..hex::CELLS {
            // A standing husk occupies the tile: nothing establishes on a
            // stump until the mycelium has finished with it.
            if self.burn[i] > 0 || self.state[i] == Cell::Tree || self.remains_code[i] != 0 {
                continue;
            }
            let s = self.state[i];
            // Light on this tile: 1 in the open, sliding to 0 beside a mature
            // canopy. It gates BOTH understories — grass, and tree seedlings
            // (gap-phase regeneration: recruitment happens in openings and at
            // edges, never under a closed canopy).
            let light = match self.tree_dist[i] {
                255 => 1.0,
                d => 1.0 - p.shade_strength * shade_suppression(d),
            };
            // Vigor components, each calibrated to 1 at the neutral climate
            // (0.5/0.5) with baseline soil. Species bend the water response
            // via an exponent, and the light gate via shade tolerance.
            let water = self.tile_water(i);
            let fert = 1.0 + p.nutrient_boost * self.nutrients[i] as f64 / NUTRIENT_CAP as f64;
            let sun_f = 0.4 + 1.2 * self.sun;
            let water_base = 0.3 + 1.4 * water;
            let on_ash = self.ash[i] > 0;

            // Per-species establishment probabilities from their own seed rain.
            let mut p_sp = [0.0f64; SPECIES_COUNT];
            let mut none_grow = 1.0f64;
            for (k, item) in p_sp.iter_mut().enumerate() {
                let w = (self.seed_rain[i][k] as f64).min(SEED_RAIN_CAP);
                if w <= 0.0 {
                    continue;
                }
                let tr = &SPECIES_TABLE[k];
                let light_k = light + tr.shade_tolerance * (1.0 - light);
                if light_k <= 0.0 {
                    continue;
                }
                let mut pk = (1.0 - (1.0 - (p.tree_growth_p * tr.growth).min(1.0)).powf(w))
                    * light_k
                    * fert
                    * sun_f
                    * water_base.powf(tr.water_affinity);
                if s == Cell::Grass {
                    pk *= p.sod_factor;
                }
                if on_ash {
                    pk *= tr.ash_affinity;
                }
                *item = pk.clamp(0.0, 1.0);
                none_grow *= 1.0 - *item;
            }
            let p_any = 1.0 - none_grow;
            if p_any > 0.0
                && rng::uniform01(self.seed, i as u32, tick, Stream::TreeGrowth) < p_any
            {
                // Winner drawn proportional to each species' pressure.
                let total: f64 = p_sp.iter().sum();
                let mut pick = rng::uniform01(self.seed, i as u32, tick, Stream::SpeciesChoice)
                    * total;
                let mut chosen = Species::Acacia;
                for (k, &pk) in p_sp.iter().enumerate() {
                    pick -= pk;
                    if pick <= 0.0 {
                        chosen = Species::from_u8(k as u8);
                        break;
                    }
                }
                self.plant_tree(i, chosen, tick);
                continue;
            }
            if s == Cell::Bare {
                let p_grass = ((p.grass_seed_p + p.grass_clonal_p * self.grass_nbrs[i] as f64)
                    * light
                    * fert
                    * sun_f
                    * water_base)
                    .clamp(0.0, 1.0);
                if rng::uniform01(self.seed, i as u32, tick, Stream::GrassGrowth) < p_grass {
                    self.plant(i, Cell::Grass, tick);
                }
            }
        }
    }

    /// Run one tick of the ecology; `tick` is the committed tick counter from
    /// the clock (used as the RNG time coordinate).
    pub fn step(&mut self, tick: u64) {
        let (sun, moisture) = self.climate_at(tick);
        self.sun = sun;
        self.moisture = moisture;
        self.decay_pass();
        self.weather_pass(tick);
        self.rebuild_fields(tick);
        self.fire_pass(tick);
        self.death_pass(tick);
        self.growth_pass(tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fire off unless a test is about fire — ignitions are rare but would
    /// make long statistical runs noisy.
    /// Fire, storms, and climate swings all off: a controlled laboratory
    /// world at the exactly-neutral climate (all multipliers = 1).
    fn no_fire() -> Params {
        Params {
            fire_ignition_p: 0.0,
            storm_rate: 0.0,
            climate_swing: 0.0,
            ..Params::default()
        }
    }

    /// A world "the user has planted": scattered trees + grass, standing in
    /// for manual brush work now that defaults start empty.
    fn planted(seed: u64) -> World {
        World::with_params(
            seed,
            Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() },
        )
    }

    fn bare_world(seed: u64, params: Params) -> World {
        let mut w = World::with_params(seed, params);
        for i in 0..hex::CELLS {
            w.paint(i, Brush::Clear, 0);
        }
        w
    }

    /// Keep a mature tree standing at `index`: if it died, re-plant it with a
    /// birth tick far enough back to be mature immediately.
    fn ensure_mature_tree(w: &mut World, index: usize, tick: u64) {
        if w.state(index) != Cell::Tree {
            let born = tick.saturating_sub(w.params().tree_maturity_age as u64 + 1);
            w.paint(index, Brush::Tree, born);
        }
    }

    fn center() -> (usize, i32, i32) {
        let (q, r) = hex::offset_to_axial(32, 32);
        (hex::axial_to_index(q, r).unwrap(), q, r)
    }

    #[test]
    fn default_worlds_start_empty() {
        let w = World::new(5);
        assert_eq!(w.counts(), [hex::CELLS as u32, 0, 0], "nothing grows until painted");
    }

    #[test]
    fn counts_always_sum_to_the_grid() {
        let mut w = planted(7);
        for tick in 1u64..=200 {
            w.step(tick);
            assert_eq!(w.counts().iter().sum::<u32>(), hex::CELLS as u32);
        }
    }

    #[test]
    fn same_seed_reproduces_the_same_run() {
        let mut a = planted(42);
        let mut b = planted(42);
        for tick in 1..=1000 {
            a.step(tick);
            b.step(tick);
        }
        for i in 0..hex::CELLS {
            assert_eq!(a.state(i), b.state(i), "cell {i} diverged");
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let a = planted(1);
        let b = planted(2);
        assert!((0..hex::CELLS).any(|i| a.state(i) != b.state(i)));
    }

    #[test]
    fn explicit_seeding_scatters_both_trees_and_grass() {
        let w = World::with_params(
            1234,
            Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() },
        );
        let [_, grass, trees] = w.counts();
        assert!((30..=140).contains(&trees), "trees={trees}");
        assert!((250..=600).contains(&grass), "grass={grass}");
    }

    #[test]
    fn spontaneous_grass_appears_at_the_seed_rate() {
        // Spontaneous seeding is off by default; switch it on explicitly and
        // re-clear each tick so every draw sees zero neighbors and no shade.
        let mut w = bare_world(9, Params { grass_seed_p: 0.005, ..no_fire() });
        let mut grew = 0u64;
        let mut eligible = 0u64;
        for tick in 1u64..=400 {
            w.step(tick);
            for i in 0..hex::CELLS {
                eligible += 1;
                if w.state(i) == Cell::Grass {
                    grew += 1;
                    w.paint(i, Brush::Clear, tick);
                }
            }
        }
        let rate = grew as f64 / eligible as f64;
        let want = 0.005;
        assert!(
            (want * 0.85..=want * 1.15).contains(&rate),
            "rate={rate} vs seed_p={want}"
        );
    }

    #[test]
    fn clonal_spread_compounds_per_grass_neighbor() {
        // A bare hole ringed by six grass tiles sprouts at seed + 6·clonal.
        let mut w = bare_world(13, no_fire());
        let (hole, q, r) = center();
        let mut grew = 0u64;
        let mut trials = 0u64;
        for tick in 1u64..=4000 {
            w.paint(hole, Brush::Clear, tick);
            for (dq, dr) in hex::NEIGHBORS {
                w.paint(hex::axial_to_index(q + dq, r + dr).unwrap(), Brush::Grass, tick);
            }
            w.step(tick);
            trials += 1;
            if w.state(hole) == Cell::Grass {
                grew += 1;
            }
            // Sweep any stray spontaneous grass so neighbor counts stay exact.
            for i in 0..hex::CELLS {
                if w.state(i) != Cell::Bare {
                    w.paint(i, Brush::Clear, tick);
                }
            }
        }
        let rate = grew as f64 / trials as f64;
        let p = Params::default();
        let want = p.grass_seed_p + 6.0 * p.grass_clonal_p;
        assert!(
            (want * 0.85..=want * 1.15).contains(&rate),
            "rate={rate} vs seed+6·clonal={want}"
        );
    }

    #[test]
    fn full_shade_still_bans_grass_adjacent_to_mature_trees() {
        // shade_strength 1 (default) zeroes the distance-1 multiplier, so the
        // original "not adjacent to a tree" rule holds exactly at defaults.
        let mut w =
            bare_world(17, Params { grass_seed_p: 0.5, tree_growth_p: 0.0, ..no_fire() });
        let (c, q, r) = center();
        // Start beyond maturity age so ensure_mature_tree can backdate births.
        for tick in 100u64..=400 {
            ensure_mature_tree(&mut w, c, tick);
            w.step(tick);
            for (dq, dr) in hex::NEIGHBORS {
                let j = hex::axial_to_index(q + dq, r + dr).unwrap();
                assert_ne!(w.state(j), Cell::Grass, "grass under full shade at tick {tick}");
            }
        }
    }

    #[test]
    fn partial_shade_thins_grass_by_distance() {
        // With a high base rate, distance-2 tiles should sprout measurably
        // less often than unshaded tiles, and more often than distance-1.
        let mut w = bare_world(
            19,
            Params { grass_seed_p: 0.2, grass_clonal_p: 0.0, tree_growth_p: 0.0, ..no_fire() },
        );
        let (c, q, r) = center();
        let mut hits = [0u64; 2]; // [d2, far]
        let mut trials = 0u64;
        let far = hex::axial_to_index(q + 20, r).unwrap();
        for tick in 100u64..=4100 {
            ensure_mature_tree(&mut w, c, tick);
            let d2 = hex::axial_to_index(q + 2, r).unwrap();
            w.paint(d2, Brush::Clear, tick);
            w.paint(far, Brush::Clear, tick);
            w.step(tick);
            trials += 1;
            if w.state(d2) == Cell::Grass {
                hits[0] += 1;
            }
            if w.state(far) == Cell::Grass {
                hits[1] += 1;
            }
        }
        let (rate_d2, rate_far) = (hits[0] as f64 / trials as f64, hits[1] as f64 / trials as f64);
        // d2 multiplier at full strength is 1 − 0.55 = 0.45.
        assert!(rate_d2 < rate_far * 0.65, "d2={rate_d2} not thinned vs far={rate_far}");
        assert!(rate_d2 > rate_far * 0.25, "d2={rate_d2} over-suppressed vs far={rate_far}");
    }

    #[test]
    fn immature_trees_produce_no_seed_rain() {
        // tree_growth_p = 1 would convert every tile with any seed rain, so
        // zero sprouts proves an always-immature tree emits none.
        let p = Params { tree_growth_p: 1.0, grass_seed_p: 0.0, ..no_fire() };
        let mut w = bare_world(23, p);
        let (c, _, _) = center();
        for tick in 1u64..=100 {
            w.paint(c, Brush::Tree, tick); // born now: age 0 forever
            w.step(tick);
            assert_eq!(w.counts()[2], 1, "an immature tree spawned offspring at tick {tick}");
        }
    }

    #[test]
    fn recruitment_dips_at_the_trunk_and_peaks_one_ring_out() {
        // Janzen-Connell: distance-1 establishment ≈ 0.25 of the distance-2
        // rate. Shade off so the kernel is measured alone (at default
        // strength the light gate zeroes distance 1 entirely).
        let p = Params {
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.05,
            shade_strength: 0.0,
            ..no_fire()
        };
        let mut w = bare_world(29, p);
        let (c, q, r) = center();
        let mut d1 = 0u64;
        let mut d2 = 0u64;
        let mut trials = 0u64;
        let t1 = hex::axial_to_index(q + 1, r).unwrap();
        let t2 = hex::axial_to_index(q + 2, r).unwrap();
        for tick in 1u64..=8000 {
            ensure_mature_tree(&mut w, c, tick);
            w.paint(t1, Brush::Clear, tick);
            w.paint(t2, Brush::Clear, tick);
            w.step(tick);
            trials += 1;
            if w.state(t1) == Cell::Tree {
                d1 += 1;
            }
            if w.state(t2) == Cell::Tree {
                d2 += 1;
            }
            // Remove other sprouts so the center stays the only seed source.
            for i in 0..hex::CELLS {
                if i != c && w.state(i) == Cell::Tree {
                    w.paint(i, Brush::Clear, tick);
                }
            }
        }
        let (r1, r2) = (d1 as f64 / trials as f64, d2 as f64 / trials as f64);
        assert!(r1 < r2 * 0.5, "no Janzen-Connell dip: d1={r1}, d2={r2}");
        assert!(r1 > r2 * 0.1, "trunk dip too deep: d1={r1}, d2={r2}");
        // And the peak rate itself sits near tree_growth_p.
        assert!((0.035..=0.065).contains(&r2), "d2 rate {r2} far from p=0.05");
    }

    #[test]
    fn sod_competition_slows_trees_on_grass() {
        let base = Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.05, ..no_fire() };
        let rate_on = |sod: f64, cover: Brush| {
            let mut w = bare_world(31, Params { sod_factor: sod, ..base });
            let (c, q, r) = center();
            let target = hex::axial_to_index(q + 2, r).unwrap();
            let mut grew = 0u64;
            let mut trials = 0u64;
            for tick in 1u64..=6000 {
                ensure_mature_tree(&mut w, c, tick);
                w.paint(target, Brush::Clear, tick);
                if cover == Brush::Grass {
                    w.paint(target, Brush::Grass, tick);
                }
                w.step(tick);
                trials += 1;
                if w.state(target) == Cell::Tree {
                    grew += 1;
                }
                // Sweep every tree but the source so nothing new shades the
                // target or adds seed rain.
                for i in 0..hex::CELLS {
                    if i != c && w.state(i) == Cell::Tree {
                        w.paint(i, Brush::Clear, tick);
                    }
                }
            }
            grew as f64 / trials as f64
        };
        let on_bare = rate_on(0.4, Brush::Clear);
        let on_sod = rate_on(0.4, Brush::Grass);
        assert!(
            on_sod < on_bare * 0.6 && on_sod > on_bare * 0.2,
            "sod 0.4 should cut establishment to ~40%: bare={on_bare}, sod={on_sod}"
        );
    }

    #[test]
    fn crowded_trees_self_thin() {
        // Center of a 7-tree clump with aggressive crowding dies quickly; an
        // isolated tree with the same settings stands for hundreds of ticks.
        let p = Params {
            crowding_p: 0.2,
            tree_mean_life: 100_000,
            grass_seed_p: 0.0,
            tree_growth_p: 0.0,
            ..no_fire()
        };
        let mut w = bare_world(37, p);
        let (c, q, r) = center();
        let born = 0u64;
        w.paint(c, Brush::Tree, born);
        for (dq, dr) in hex::NEIGHBORS {
            w.paint(hex::axial_to_index(q + dq, r + dr).unwrap(), Brush::Tree, born);
        }
        let far = hex::axial_to_index(q + 15, r).unwrap();
        w.paint(far, Brush::Tree, born);
        let mut center_died_at = None;
        for tick in 1u64..=300 {
            w.step(tick);
            if center_died_at.is_none() && w.state(c) != Cell::Tree {
                center_died_at = Some(tick);
            }
        }
        let died = center_died_at.expect("crowded center never thinned");
        assert!(died >= 40, "crowding applied before the clump matured (tick {died})");
        assert!(died < 150, "crowded center lived too long (tick {died})");
        assert_eq!(w.state(far), Cell::Tree, "isolated tree should not self-thin");
    }

    #[test]
    fn fire_spreads_along_grass_and_stops_at_mature_trees_and_gaps() {
        // Tree growth off: the firebreak's own (flammable) saplings would
        // otherwise bridge the fire around it.
        let p = Params {
            fire_spread_p: 1.0,
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            // Long-lived grass so the corridor doesn't die of old age mid-test.
            grass_mean_life: 10_000,
            ..no_fire()
        };
        let mut w = bare_world(41, p);
        let (_, q, r) = center();
        let t0 = 60u64; // past maturity for the firebreak tree painted at born 0
        // Grass corridor with a mature-tree firebreak, then more grass; and a
        // disconnected patch across a bare gap. Grass painted fresh at t0.
        for dq in 0..4 {
            w.paint(hex::axial_to_index(q + dq, r).unwrap(), Brush::Grass, t0);
        }
        let firebreak = hex::axial_to_index(q + 4, r).unwrap();
        w.paint(firebreak, Brush::Tree, 0);
        for dq in 5..8 {
            w.paint(hex::axial_to_index(q + dq, r).unwrap(), Brush::Grass, t0);
        }
        let island = hex::axial_to_index(q - 3, r).unwrap();
        w.paint(island, Brush::Grass, t0);

        w.paint(hex::axial_to_index(q, r).unwrap(), Brush::Fire, t0);
        for tick in t0..t0 + 20 {
            w.step(tick);
        }
        for dq in 0..4 {
            let j = hex::axial_to_index(q + dq, r).unwrap();
            assert_eq!(w.state(j), Cell::Bare, "corridor tile {dq} should have burned");
        }
        assert_eq!(w.state(firebreak), Cell::Tree, "mature tree must survive the fire");
        for dq in 5..8 {
            let j = hex::axial_to_index(q + dq, r).unwrap();
            assert_eq!(w.state(j), Cell::Grass, "fire crossed the firebreak to {dq}");
        }
        assert_eq!(w.state(island), Cell::Grass, "fire jumped a bare gap");
    }

    #[test]
    fn fire_kills_immature_trees_but_needs_fuel_to_reach_them() {
        let p = Params { fire_spread_p: 1.0, grass_seed_p: 0.0, ..no_fire() };
        let mut w = bare_world(43, p);
        let (_, q, r) = center();
        w.paint(hex::axial_to_index(q, r).unwrap(), Brush::Grass, 100);
        let sapling = hex::axial_to_index(q + 1, r).unwrap();
        w.paint(sapling, Brush::Tree, 99); // age 1 at ignition: flammable
        w.paint(hex::axial_to_index(q, r).unwrap(), Brush::Fire, 100);
        for tick in 100..110 {
            w.step(tick);
        }
        assert_eq!(w.state(sapling), Cell::Bare, "sapling should burn");
    }

    #[test]
    fn no_ignition_means_no_fire() {
        let mut w = World::with_params(47, no_fire());
        for tick in 1..=500 {
            w.step(tick);
            assert_eq!(w.burning_count(), 0);
        }
    }

    #[test]
    fn tree_lifetimes_are_unbounded_but_long_lives_are_rare() {
        // Constant hazard ⇒ geometric lifetimes: some trees outlive the old
        // 500-tick cap (indefinite lifecycle), yet survival decays
        // exponentially, so in equilibrium very few ever get old.
        let p = Params {
            tree_growth_p: 0.0,
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            crowding_p: 0.0,
            ..no_fire()
        };
        let mut w = bare_world(107, p);
        let n = 2000usize;
        for i in 0..n {
            w.paint(i, Brush::Tree, 0);
        }
        let (mut at_600, mut at_1500) = (0u32, 0u32);
        let mut death_sum = 0u64;
        let mut alive = n as u64;
        for tick in 1u64..=3000 {
            w.step(tick);
            let now = (0..n).filter(|&i| w.state(i) == Cell::Tree).count() as u64;
            death_sum += (alive - now) * tick;
            alive = now;
            if tick == 600 {
                at_600 = now as u32;
            }
            if tick == 1500 {
                at_1500 = now as u32;
            }
        }
        death_sum += alive * 3000; // survivors censored at the horizon
        let mean = death_sum as f64 / n as f64;
        assert!(at_600 > 50, "many trees should outlive the old 500-tick cap, saw {at_600}");
        assert!(
            at_1500 < 40,
            "six mean-lives out, long-lived trees must be rare, saw {at_1500}"
        );
        assert!(
            (200.0..=300.0).contains(&mean),
            "mean lifetime should track tree_mean_life=250, got {mean:.0}"
        );
    }

    #[test]
    fn zero_growth_probabilities_freeze_all_growth() {
        let mut w = planted(37);
        w.set_params(Params {
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            fire_ignition_p: 0.0,
            ..Params::default()
        });
        let mut prev = w.counts();
        for tick in 1u64..=200 {
            w.step(tick);
            let cur = w.counts();
            assert!(cur[1] <= prev[1] && cur[2] <= prev[2], "something grew at tick {tick}");
            prev = cur;
        }
    }

    #[test]
    fn mean_life_param_sets_the_death_rate() {
        let survivors = |mean: u32| -> usize {
            let p = Params {
                grass_mean_life: mean,
                grass_seed_p: 0.0,
                grass_clonal_p: 0.0,
                tree_growth_p: 0.0,
                ..no_fire()
            };
            let mut w = bare_world(109, p);
            for i in 0..1000 {
                w.paint(i, Brush::Grass, 0);
            }
            for tick in 1u64..=40 {
                w.step(tick);
            }
            (0..1000).filter(|&i| w.state(i) == Cell::Grass).count()
        };
        let short = survivors(20); // e^-2 ≈ 13.5%
        let long = survivors(80); // e^-0.5 ≈ 61%
        assert!((80..=200).contains(&short), "mean 20 → ~135 of 1000 after 40 ticks, saw {short}");
        assert!((520..=700).contains(&long), "mean 80 → ~607 of 1000 after 40 ticks, saw {long}");
    }

    #[test]
    fn params_are_sanitized() {
        let p = Params {
            grass_seed_p: 7.0,
            grass_clonal_p: -2.0,
            shade_strength: 3.0,
            tree_growth_p: -1.0,
            tree_range: 99,
            tree_maturity_age: 1_000_000,
            sod_factor: 40.0,
            crowding_p: f64::NAN,
            grass_mean_life: 0,
            tree_mean_life: u32::MAX,
            fire_ignition_p: 2.0,
            fire_spread_p: -0.5,
            nutrient_boost: 99.0,
            storm_rate: 7.0,
            storm_lightning_p: f64::NEG_INFINITY,
            climate_swing: -2.0,
            seed_tree_p: f64::NAN,
            seed_grass_p: 0.5,
        }
        .sanitized();
        assert_eq!(p.grass_seed_p, 1.0);
        assert_eq!(p.grass_clonal_p, 0.0);
        assert_eq!(p.shade_strength, 1.0);
        assert_eq!(p.tree_growth_p, 0.0);
        assert_eq!(p.tree_range, 8);
        assert_eq!(p.tree_maturity_age, 10_000);
        assert_eq!(p.sod_factor, 5.0);
        assert_eq!(p.crowding_p, CROWDING_P, "non-finite falls back to the default");
        assert_eq!(p.grass_mean_life, 1);
        assert_eq!(p.tree_mean_life, 100_000);
        assert_eq!(p.fire_ignition_p, 1.0);
        assert_eq!(p.fire_spread_p, 0.0);
        assert_eq!(p.nutrient_boost, 5.0);
        assert_eq!(p.storm_rate, 1.0);
        assert_eq!(p.storm_lightning_p, STORM_LIGHTNING_P, "non-finite falls back");
        assert_eq!(p.climate_swing, 0.0);
        assert_eq!(p.seed_tree_p, SEED_TREE_P);
        assert_eq!(p.seed_grass_p, 0.5);
    }

    #[test]
    fn storms_spawn_drift_across_and_dissipate() {
        let mut w = bare_world(91, Params { storm_rate: 0.05, storm_lightning_p: 0.0, ..no_fire() });
        w.set_params(Params { storm_rate: 0.05, ..w.params() });
        let mut seen = 0usize;
        for tick in 1u64..=600 {
            w.step(tick);
            seen = seen.max(w.storms().len());
        }
        assert!(seen >= 1, "storms should spawn at rate 5%");
        // Cut spawning: every active storm must eventually cross and leave.
        w.set_params(Params { storm_rate: 0.0, ..w.params() });
        for tick in 601u64..=2600 {
            w.step(tick);
        }
        assert!(w.storms().is_empty(), "storms should exit the map and dissipate");
    }

    #[test]
    fn rain_soaks_douses_and_protects() {
        let p = Params { grass_clonal_p: 0.0, tree_growth_p: 0.0, fire_spread_p: 1.0, ..no_fire() };
        let mut w = bare_world(93, p);
        let (c, q, r) = center();
        w.paint(c, Brush::Grass, 100);
        let east = hex::axial_to_index(q + 1, r).unwrap();
        w.paint(east, Brush::Grass, 100);
        w.paint(c, Brush::Fire, 100);
        assert!(w.burning(c));
        // Park a storm overhead: its rain core covers both tiles.
        let (cx, cy) = hex::axial_to_world(q, r);
        w.spawn_storm([cx, cy], [0.0, 0.0], 8.0, 100);
        w.step(100);
        assert!(!w.burning(c), "the downpour should douse the flame");
        assert_eq!(w.state(c), Cell::Grass, "a doused plant survives");
        assert!(w.wet_ratio(c) > 0.9 && w.wet_ratio(east) > 0.9, "the core soaks tiles");
        // A torch lights instantly even in the rain — but while the storm
        // sits overhead, the next downpour tick puts it straight back out.
        w.paint(east, Brush::Fire, 101);
        assert!(w.burning(east), "the torch lights on contact");
        w.step(101);
        assert!(!w.burning(east), "the ongoing downpour douses it next tick");
    }

    #[test]
    fn wet_fuel_resists_natural_fire_spread() {
        let p = Params { grass_clonal_p: 0.0, tree_growth_p: 0.0, fire_spread_p: 1.0, ..no_fire() };
        let mut w = bare_world(97, p);
        let (_, q, r) = center();
        // Grass pair: west burns, east is soaked by hand via a tiny parked storm.
        let west = hex::axial_to_index(q, r).unwrap();
        let east = hex::axial_to_index(q + 1, r).unwrap();
        w.paint(west, Brush::Grass, 50);
        w.paint(east, Brush::Grass, 50);
        let (ex, ey) = hex::axial_to_world(q + 1, r);
        w.spawn_storm([ex, ey], [0.0, 0.0], 2.0, 50); // rain core ≈ 1.5 tiles
        w.step(50); // soaks east (and west, both within core… use distance check)
        // Move the storm away conceptually: clear storms by exhausting them is
        // hard; instead just verify spread respects wetness directly.
        w.paint(west, Brush::Fire, 51);
        let east_wet = w.wet_ratio(east) > 0.0;
        w.step(51);
        w.step(52);
        if east_wet {
            assert_eq!(w.state(east), Cell::Grass, "wet grass must not catch from its neighbor");
        }
    }

    #[test]
    fn storm_lightning_strikes_and_can_start_edge_fires() {
        // A parked storm with certain lightning over a dry sward: bolts land
        // every tick; strikes in the dry ring (outside the rain core) ignite.
        let p = Params {
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            grass_mean_life: 100_000,
            fire_spread_p: 0.0,
            storm_lightning_p: 1.0,
            ..no_fire()
        };
        let mut w = bare_world(101, p);
        w.set_params(Params { storm_lightning_p: 1.0, ..w.params() });
        for i in 0..hex::CELLS {
            w.paint(i, Brush::Grass, 0);
        }
        let (c, q, r) = center();
        let _ = c;
        let (cx, cy) = hex::axial_to_world(q, r);
        w.spawn_storm([cx, cy], [0.0, 0.0], 8.0, 0);
        let mut bolts = 0u32;
        let mut ignitions = 0u32;
        for tick in 1u64..=300 {
            w.step(tick);
            bolts += (0..hex::CELLS).filter(|&i| w.bolt_active(i)).count() as u32;
            ignitions += w.burning_count();
        }
        assert!(bolts > 100, "a certain-lightning storm should strike constantly, saw {bolts}");
        assert!(ignitions > 0, "some edge strikes must land on dry fuel and ignite");
    }

    #[test]
    fn rain_speeds_growth_while_the_ground_is_wet() {
        let p = Params {
            grass_seed_p: 0.02,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            nutrient_boost: 0.0,
            ..no_fire()
        };
        let mut w = bare_world(103, p);
        let (rich, q, r) = center();
        let dry = hex::axial_to_index(q + 20, r).unwrap();
        let (mut wet_hits, mut dry_hits, mut trials) = (0u64, 0u64, 0u64);
        for tick in 1u64..=8000 {
            w.paint(rich, Brush::Clear, tick);
            w.paint(dry, Brush::Clear, tick);
            w.wet[rich] = WET_TICKS;
            w.wet[dry] = 0;
            w.step(tick);
            trials += 1;
            if w.state(rich) == Cell::Grass {
                wet_hits += 1;
            }
            if w.state(dry) == Cell::Grass {
                dry_hits += 1;
            }
        }
        let ratio = wet_hits as f64 / dry_hits.max(1) as f64;
        assert!(
            ratio > 1.25 && ratio < 1.8,
            "wet soil should sprout ~1.5× faster, ratio {ratio:.2} ({wet_hits}/{dry_hits}/{trials})"
        );
    }

    #[test]
    fn offspring_inherit_their_parents_species() {
        let p = Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, ..no_fire() };
        let mut w = bare_world(113, p);
        let (c, q, r) = center();
        w.paint_species(c, Brush::Tree, Species::Oak, 0);
        // Keep the founder alive; sweep in fresh oaks as they seed.
        for tick in 300u64..1500 {
            if w.state(c) != Cell::Tree {
                w.paint_species(c, Brush::Tree, Species::Oak, tick.saturating_sub(300));
            }
            w.step(tick);
        }
        let _ = (q, r);
        let mut offspring = 0;
        for i in 0..hex::CELLS {
            if i != c && w.state(i) == Cell::Tree {
                offspring += 1;
                assert_eq!(w.species(i), Species::Oak, "cell {i} sprouted the wrong species");
            }
        }
        assert!(offspring > 3, "the oak should have reproduced, saw {offspring}");
    }

    #[test]
    fn oak_seedlings_tolerate_shade_that_stops_pioneers() {
        // A target tile fully shaded (distance 1 from a mature tree): only
        // the shade-tolerant species can establish there.
        let rate_for = |sp: Species| -> f64 {
            // High growth so oak's thin shade edge (0.15 light × 0.45 growth
            // × the d1 kernel dip) yields a clear statistical signal.
            let p = Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.3, ..no_fire() };
            let mut w = bare_world(127, p);
            let (c, q, r) = center();
            let target = hex::axial_to_index(q + 1, r).unwrap();
            let mut grew = 0u64;
            for tick in 300u64..4300 {
                if w.state(c) != Cell::Tree {
                    w.paint_species(c, Brush::Tree, sp, tick.saturating_sub(300));
                }
                w.paint(target, Brush::Clear, tick);
                w.step(tick);
                if w.state(target) == Cell::Tree {
                    grew += 1;
                }
                // Remove strays so the founder is the only seed source.
                for i in 0..hex::CELLS {
                    if i != c && i != target && w.state(i) == Cell::Tree {
                        w.paint(i, Brush::Clear, tick);
                    }
                }
            }
            grew as f64 / 4000.0
        };
        let oak = rate_for(Species::Oak);
        let acacia = rate_for(Species::Acacia);
        assert_eq!(acacia, 0.0, "pioneers must not establish in full shade");
        assert!(oak > 0.001, "oak seedlings should take root under the canopy, rate {oak}");
    }

    #[test]
    fn pine_seeds_flourish_on_ash() {
        // Serotiny: hold ash on one target, none on the other; pine
        // establishment should run ~3× higher on the ash.
        let p = Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.02, ..no_fire() };
        let mut w = bare_world(131, p);
        let (c, q, r) = center();
        let ashed = hex::axial_to_index(q + 2, r).unwrap();
        let plain = hex::axial_to_index(q - 2, r).unwrap();
        let (mut on_ash, mut off_ash) = (0u64, 0u64);
        for tick in 300u64..6300 {
            if w.state(c) != Cell::Tree {
                w.paint_species(c, Brush::Tree, Species::Pine, tick.saturating_sub(300));
            }
            w.paint(ashed, Brush::Clear, tick);
            w.paint(plain, Brush::Clear, tick);
            w.ash[ashed] = ASH_TICKS; // held fresh (test-only direct access)
            w.ash[plain] = 0;
            w.step(tick);
            if w.state(ashed) == Cell::Tree {
                on_ash += 1;
            }
            if w.state(plain) == Cell::Tree {
                off_ash += 1;
            }
        }
        let ratio = on_ash as f64 / off_ash.max(1) as f64;
        assert!(
            ratio > 2.0 && ratio < 4.5,
            "ash should triple pine recruitment, ratio {ratio:.2} ({on_ash}/{off_ash})"
        );
    }

    #[test]
    fn willows_die_in_droughts_that_acacias_shrug_off() {
        // Drought sensitivity: hold the ground dry under a hot sun (via a
        // swingless climate is neutral, so force dryness by clearing wet and
        // relying on stress exponents at a HARSH tick found from swings).
        let mk = |sp: Species| -> usize {
            let p = Params {
                climate_swing: 1.0,
                grass_seed_p: 0.0,
                grass_clonal_p: 0.0,
                tree_growth_p: 0.0,
                crowding_p: 0.0,
                fire_ignition_p: 0.0,
                storm_rate: 0.0,
                ..Params::default()
            };
            let mut w = World::with_params(137, p);
            for i in 0..hex::CELLS {
                w.paint(i, Brush::Clear, 0);
            }
            // Find a sustained dry-hot stretch.
            let mut dry_start = 0u64;
            for t in 100..4000u64 {
                let (sun, m) = w.climate_at(t);
                if m < 0.2 && sun > 0.6 {
                    dry_start = t;
                    break;
                }
            }
            assert!(dry_start > 0);
            for i in 0..800 {
                w.paint_species(i, Brush::Tree, sp, dry_start.saturating_sub(500));
            }
            for t in dry_start..(dry_start + 120) {
                w.step(t);
            }
            (0..800).filter(|&i| w.state(i) == Cell::Tree).count()
        };
        let willows = mk(Species::Willow);
        let acacias = mk(Species::Acacia);
        assert!(
            willows * 2 < acacias,
            "willows should thin far faster in drought ({willows} vs {acacias} survivors)"
        );
        assert!(acacias > 250, "acacias should weather it comparatively well, saw {acacias}");
    }

    #[test]
    fn fire_immunity_follows_the_species_bark() {
        let p = Params { fire_spread_p: 1.0, grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.0, ..no_fire() };
        // Pine at age 40 (mature at 32, fireproof only at 80): still fuel.
        let mut w = bare_world(139, p);
        let (_, q, r) = center();
        let pine = hex::axial_to_index(q, r).unwrap();
        let fuel = hex::axial_to_index(q + 1, r).unwrap();
        w.paint_species(pine, Brush::Tree, Species::Pine, 60);
        w.paint(fuel, Brush::Grass, 100);
        w.paint(fuel, Brush::Fire, 100);
        w.step(100); // spread reaches the 40-tick pine
        assert!(w.burning(pine), "a mature-but-unarmored pine should catch");

        // Oak at age 90 (maturity 100, fireproof at 80): already armored.
        let mut v = bare_world(139, p);
        let oak = hex::axial_to_index(q, r).unwrap();
        let fuel2 = hex::axial_to_index(q + 1, r).unwrap();
        v.paint_species(oak, Brush::Tree, Species::Oak, 10);
        v.paint(fuel2, Brush::Grass, 100);
        v.paint(fuel2, Brush::Fire, 100);
        v.step(100);
        assert!(!v.burning(oak), "an armored young oak should shrug the fire off");
    }

    #[test]
    fn oak_snags_outlast_acacia_snags() {
        let clear_time = |sp: Species| -> u64 {
            let p = Params {
                grass_seed_p: 0.0,
                grass_clonal_p: 0.0,
                tree_growth_p: 0.0,
                tree_mean_life: 1,
                ..no_fire()
            };
            let mut w = bare_world(149, p);
            let (c, _, _) = center();
            w.paint_species(c, Brush::Tree, sp, 0);
            // Oak's mean-life multiplier keeps it alive a few ticks even at
            // param 1, so time the husk from death, not from a fixed tick.
            let mut died = 0u64;
            for tick in 1000u64..5000 {
                w.step(tick);
                if died == 0 && w.state(c) == Cell::Bare {
                    died = tick;
                }
                if died > 0 && w.remains(c).is_none() {
                    return tick - died;
                }
            }
            panic!("husk never cleared");
        };
        let oak = clear_time(Species::Oak);
        let acacia = clear_time(Species::Acacia);
        assert!(
            oak as f64 > acacia as f64 * 1.4,
            "hardwood should stand much longer ({oak} vs {acacia} ticks)"
        );
    }

    #[test]
    fn paint_sets_clears_and_ignites() {
        let mut w = bare_world(31, no_fire());
        w.paint(5, Brush::Tree, 3);
        assert_eq!(w.state(5), Cell::Tree);
        w.paint(5, Brush::Grass, 4);
        assert_eq!(w.state(5), Cell::Grass);
        w.paint(5, Brush::Fire, 5);
        assert!(w.burning(5), "grass should ignite");
        w.paint(5, Brush::Clear, 5);
        assert_eq!(w.state(5), Cell::Bare);
        assert!(!w.burning(5), "clearing extinguishes");
        w.paint(6, Brush::Fire, 5);
        assert!(!w.burning(6), "bare ground holds no fire");
        // The torch overrides bark: a mature tree ignites under the brush
        // even though spreading fire can't touch it.
        w.paint(7, Brush::Tree, 0);
        w.paint(7, Brush::Fire, 100);
        assert!(w.burning(7), "the fire brush should light mature trees too");
    }

    #[test]
    fn deaths_leave_remains_and_burns_leave_ash() {
        // Growth off: fresh churn would otherwise re-husk the tiles under test.
        let mut w = bare_world(
            53,
            Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.0, ..no_fire() },
        );
        w.paint(0, Brush::Grass, 10);
        // Hazard mortality: step until the stochastic death lands.
        let mut died_at = 0u64;
        for t in 11..=2000u64 {
            w.step(t);
            if w.state(0) == Cell::Bare {
                died_at = t;
                break;
            }
        }
        assert!(died_at > 0, "grass should die within a few mean-lives");
        let r = w.remains(0).expect("natural death should leave a husk");
        assert!(!r.tree && !r.charred);
        assert_eq!(w.ash_ratio(0), 0.0, "no fire, no ash");

        // Regrowth (or a paint) clears the husk immediately.
        w.paint(0, Brush::Tree, died_at);
        assert!(w.remains(0).is_none());

        // A burn leaves a charred husk plus ash that decays to zero.
        w.paint(1, Brush::Grass, 500);
        w.paint(1, Brush::Fire, 500);
        w.step(500);
        w.step(501);
        let r = w.remains(1).expect("burn-out should leave a husk");
        assert!(r.charred && !r.tree);
        assert!(w.ash_ratio(1) > 0.9);
        // Charred thatch rots at the slow abiotic rate; give it plenty.
        for t in 502..=(502 + 2 * (GRASS_ROT_UNITS / ROT_BASE) as u64) {
            w.step(t);
        }
        assert!(w.remains(1).is_none(), "husk should crumble");
        assert_eq!(w.ash_ratio(1), 0.0, "ash should wash out");
    }

    /// Kill a tree on demand by planting it already far past any lifespan.
    fn plant_doomed_tree(w: &mut World, index: usize) {
        w.paint(index, Brush::Tree, 0);
    }

    #[test]
    fn mycelium_spreads_between_husks_and_speeds_decay() {
        let p = Params {
            tree_growth_p: 0.0,
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_mean_life: 1, // all doomed trees die on the first step
            ..no_fire()
        };
        let ticks_to_clear = |cluster: bool| -> u64 {
            let mut w = bare_world(61, p);
            let (c, q, r) = center();
            plant_doomed_tree(&mut w, c);
            if cluster {
                for (dq, dr) in hex::NEIGHBORS {
                    plant_doomed_tree(&mut w, hex::axial_to_index(q + dq, r + dr).unwrap());
                }
            }
            // At tick 1000 every planted tree is far beyond its lifespan:
            // they all die in the first step and decompose from there.
            for tick in 1000u64..2000 {
                w.step(tick);
                if w.remains(c).is_none() && w.state(c) == Cell::Bare {
                    return tick - 1000;
                }
            }
            panic!("husk never cleared");
        };
        let clustered = ticks_to_clear(true);
        let isolated = ticks_to_clear(false);
        assert!(
            clustered + 8 < isolated,
            "shared inoculum should rot a clustered husk faster ({clustered} vs {isolated} ticks)"
        );
    }

    #[test]
    fn charred_wood_resists_decomposition() {
        // tree_mean_life 1: the natural-death tree dies on the first step,
        // so its decomposition clock starts deterministically.
        let p = Params {
            tree_growth_p: 0.0,
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_mean_life: 1,
            ..no_fire()
        };
        // Natural death.
        let mut a = bare_world(67, p);
        let (c, _, _) = center();
        plant_doomed_tree(&mut a, c);
        let mut natural = 0u64;
        for tick in 1000u64..3000 {
            a.step(tick);
            if a.remains(c).is_none() {
                natural = tick - 1000;
                break;
            }
        }
        // Torched mature tree → charred snag (this tree must LIVE till the
        // torch, so it gets an effectively infinite mean life).
        let mut b = bare_world(67, Params { fire_spread_p: 0.0, tree_mean_life: 100_000, ..p });
        b.paint(c, Brush::Tree, 900);
        b.paint(c, Brush::Fire, 1000);
        let mut charred = 0u64;
        for tick in 1000u64..3000 {
            b.step(tick);
            if b.state(c) == Cell::Bare && b.remains(c).is_none() && tick > 1002 {
                charred = tick - 1000;
                break;
            }
        }
        assert!(natural > 0 && charred > 0, "both husks must eventually clear");
        assert!(
            charred > natural * 2,
            "charcoal should far outlast natural deadfall ({charred} vs {natural} ticks)"
        );
    }

    #[test]
    fn decomposition_returns_nutrients_and_fire_flushes_immediately() {
        // tree_mean_life 1: the planted tree dies on the first step, so the
        // decomposition clock starts deterministically.
        let p = Params {
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            tree_mean_life: 1,
            ..no_fire()
        };
        let mut w = bare_world(73, p);
        let (c, _, _) = center();
        assert_eq!(w.nutrient_ratio(c), 0.0, "fresh soil starts at baseline");
        plant_doomed_tree(&mut w, c);
        let mut cleared_at = None;
        for tick in 1000u64..1400 {
            w.step(tick);
            if cleared_at.is_none() && w.remains(c).is_none() {
                cleared_at = Some(tick);
                break;
            }
        }
        cleared_at.expect("husk should rot");
        let after_rot = w.nutrient_ratio(c);
        assert!(
            after_rot > 0.5,
            "a rotted tree should leave rich soil, got {after_rot}"
        );

        // Fire mineralizes on the spot: nutrients jump the tick the burn ends.
        let mut b = bare_world(73, Params { fire_spread_p: 0.0, ..p });
        b.paint(0, Brush::Grass, 100);
        b.paint(0, Brush::Fire, 100);
        b.step(100);
        let before = b.nutrient_ratio(0);
        b.step(101); // burn-out
        assert!(
            b.nutrient_ratio(0) > before + 0.05,
            "ash flush should land immediately ({} → {})",
            before,
            b.nutrient_ratio(0)
        );
    }

    #[test]
    fn fertile_soil_speeds_establishment() {
        let p = Params {
            grass_seed_p: 0.01,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            nutrient_boost: 5.0,
            ..no_fire()
        };
        let mut w = bare_world(79, p);
        let (rich, q, r) = center();
        let poor = hex::axial_to_index(q + 20, r).unwrap();
        let (mut rich_hits, mut poor_hits, mut trials) = (0u64, 0u64, 0u64);
        for tick in 1u64..=6000 {
            w.paint(rich, Brush::Clear, tick);
            w.paint(poor, Brush::Clear, tick);
            w.nutrients[rich] = NUTRIENT_CAP; // held at full fertility
            w.nutrients[poor] = 0;
            w.step(tick);
            trials += 1;
            if w.state(rich) == Cell::Grass {
                rich_hits += 1;
            }
            if w.state(poor) == Cell::Grass {
                poor_hits += 1;
            }
        }
        let (rr, pr) = (rich_hits as f64 / trials as f64, poor_hits as f64 / trials as f64);
        // boost 5 at full fertility → 6× the establishment rate.
        assert!(rr > pr * 3.5, "fertile soil should sprout much faster ({rr} vs {pr})");
        assert!(rr < pr * 9.0, "but not absurdly so ({rr} vs {pr})");
    }

    #[test]
    fn plants_drain_soil_faster_than_leaching() {
        let p = Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.0, ..no_fire() };
        let mut w = bare_world(83, Params { grass_mean_life: 100_000, ..p });
        w.nutrients[0] = NUTRIENT_CAP;
        w.nutrients[1] = NUTRIENT_CAP;
        w.paint(0, Brush::Grass, 1000);
        for tick in 1000u64..1100 {
            w.step(tick);
        }
        let occupied = w.nutrient_ratio(0);
        let idle = w.nutrient_ratio(1);
        assert!(occupied < idle - 0.2, "a living plant should feed on the store ({occupied} vs {idle})");
        assert!(idle < 1.0, "idle soil still leaches");
    }

    #[test]
    fn husks_block_regrowth_until_rotted() {
        // Certain growth everywhere — except the tile with a standing husk.
        // Grass mean life 1 makes the stump die on the first step; trees are
        // effectively immortal so the seed source survives the test.
        let p = Params {
            tree_growth_p: 1.0,
            grass_seed_p: 1.0,
            shade_strength: 0.0,
            grass_mean_life: 1,
            tree_mean_life: 100_000,
            ..no_fire()
        };
        let mut w = bare_world(71, p);
        let (c, q, r) = center();
        let stump = hex::axial_to_index(q + 2, r).unwrap();
        w.paint(stump, Brush::Grass, 0); // doomed: dies on the first step
        w.paint(c, Brush::Tree, 960); // mature seed source by tick 1000
        w.step(1000); // stump dies this step (death runs before growth)
        w.step(1001);
        assert!(w.remains(stump).is_some(), "the dead grass should stand as a husk");
        assert_eq!(w.state(stump), Cell::Bare, "nothing may establish on the stump");
        let (nq, nr) = (q + 2, r + 1);
        let open = hex::axial_to_index(nq, nr).unwrap();
        assert_ne!(w.state(open), Cell::Bare, "open tiles in range must have grown");
        // Once the mycelium finishes, the tile becomes plantable again.
        for tick in 1002u64..2000 {
            w.step(tick);
            if w.state(stump) != Cell::Bare {
                return;
            }
        }
        panic!("the stump tile never regrew after decomposition");
    }
}
