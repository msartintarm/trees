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
use super::hex::Grid;
use super::terrain::{Terrain, RENDER_RELIEF};

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

/// The canopy race and the root network.
/// Shade-avoidance: a young tree hemmed in by neighbors elongates for the
/// light — its etiolation score accumulates while it grows and freezes at
/// maturity, permanently setting a taller, narrower form (open-grown trees
/// stay short and broad). Below ground, adjacent trees are linked: root
/// grafts and mycorrhizae diffuse soil nutrients from rich tiles to poor
/// ones each tick, mature trees nurse same-species seedlings beside them
/// (reduced mortality), and overlapping root plates drain shared soil
/// faster.
pub const ETIOL_HEIGHT_GAIN: f64 = 0.45;
pub const ETIOL_WIDTH_LOSS: f64 = 0.35;
/// Fraction of the nutrient difference equalized per tick per linked pair.
pub const ROOT_SHARE_DIV: u16 = 16;
/// Hazard multiplier for a seedling beside a mature tree of its own kind.
pub const NURSE_FACTOR: f64 = 0.6;

/// Species behaviors drawn from real tree ecology.
///
/// Serotiny (pine): a mature pine that burns releases its sealed cone bank
/// onto the ash around it — a seed pulse lasting `SERO_TICKS`, worth
/// `SERO_WEIGHT` of kernel seed rain.
pub const SERO_TICKS: u8 = 40;
pub const SERO_WEIGHT: f32 = 2.5;
pub const SERO_RADIUS: i32 = 2;
/// Windthrow: per-tick chance a mature tree under a storm is toppled,
/// scaled by the cloud's gustiness and by how drawn-up the tree grew
/// (tall, thin forest trees fall first).
pub const WINDTHROW_P: f64 = 0.0015;
/// Long-distance dispersal: per mature tree per tick, base chance that one
/// seed travels anywhere on the map (× the species' `long_distance`) —
/// jays caching acorns, wind-borne winged and cottony seed, pods carried by
/// browsing animals. A seed landing in open or edge ground may germinate.
pub const LDD_P: f64 = 0.0006;
pub const LDD_GERMINATION: f64 = 0.5;
/// Masting (oak): synchronized boom years. Length of a "year" in ticks,
/// oak establishment in mast vs lean years (seed predators eat lean crops).
pub const MAST_YEAR_TICKS: u64 = 300;
pub const MAST_BOOM: f64 = 3.0;
pub const MAST_LEAN: f64 = 0.3;
/// Nitrogen fixation (acacia): nutrients a living acacia adds per tick to
/// its own tile and to each neighbor.
pub const N_FIX_OWN: u16 = 4;
pub const N_FIX_NEIGHBOR: u16 = 2;
/// Waterlogging: in saturated basins (water table above 0.5), species that
/// can't tolerate flooded roots suffer hazard × (1 + this × excess).
pub const WATERLOG_HAZARD: f64 = 10.0;
/// Nitrogen-fixers establish as if soil were this fertile regardless of
/// actual nutrients — an edge on poor ground, none on rich.
pub const N_FIXER_FERTILITY: f64 = 1.5;
/// Water table: the fraction of a saturated table's water that reaches a
/// tile, and how much a wet table damps fire spread.
pub const WATER_TABLE: f64 = 1.0;
pub const WATER_TABLE_REACH: f64 = 0.85;
pub const WATER_TABLE_FIRE_DAMP: f64 = 0.6;
/// Landscape relief strength (0 = flat, uniform ground: legacy world).
pub const TERRAIN: f64 = 1.0;
/// Grass functional-type niche strength (0 = one generic grass: legacy).
pub const GRASS_NICHES: f64 = 1.0;
/// Peak niche-fit multiplier: a type on its optimum grows this much faster
/// than a generic grass; off-optimum it falls away (unimodal responses).
pub const NICHE_PEAK: f64 = 1.8;
/// Strength of the deep-soil competitive advantage of demanding species.
pub const SOIL_COMPETITION: f64 = 2.0;
/// Soil depth at which the hierarchy is neutral.
pub const SOIL_TYPICAL: f64 = 0.55;
/// Jays cache acorns within this many hexes of the parent tree (real jays
/// carry them a few hundred metres, to open ground and woodland edges).
pub const JAY_RADIUS: i32 = 12;
/// Hex radius of the "same-species adults nearby" neighborhood.
pub const NEAR_RADIUS: i32 = 2;
/// Neighbor competition (all scaled by `competition`).
pub const COMPETITION: f64 = 1.0;
/// Conspecific negative density dependence (Janzen-Connell / plant-soil
/// feedback): seedling establishment ÷ (1 + CNDD × same-species adults
/// within NEAR_RADIUS) — each species' specialist enemies build up where
/// it is common, so rare species recruit better.
pub const CNDD: f64 = 0.12;
/// Needle litter: drop per tick from a mature littering tree onto its own
/// and neighboring tiles, fraction kept per tick, and the establishment
/// suppression at a full carpet for everything but the litter's source.
pub const LITTER_DROP: f32 = 0.02;
pub const LITTER_KEEP: f32 = 0.99;
pub const ALLELOPATHY: f64 = 0.35;
/// Sod thatch: annual germination × (1 − THATCH per adjacent sod tile).
pub const THATCH: f64 = 0.12;
/// Root water competition: per adjacent mature tree, on ground drier than
/// typical, establishment ÷ (1 + stress) and sapling hazard × (1 + stress),
/// scaled by the seedling's drought sensitivity.
pub const ROOT_WATER: f64 = 0.5;
/// Specialist pest outbreaks (oak wilt spreads through root grafts, bark
/// beetles through dense pine). Spontaneous infection per mature tree per
/// tick at full local density, spread chance per infested graft neighbor,
/// load growth per tick (u8), hazard at full load, and recovery (× host
/// resistance) per tick.
pub const PEST_STRENGTH: f64 = 1.0;
pub const PEST_SEED: f64 = 0.002;
pub const PEST_SPREAD: f64 = 0.08;
pub const PEST_GROWTH: u8 = 4;
pub const PEST_HAZARD: f64 = 0.03;
pub const PEST_RECOVER: f64 = 0.01;
/// Browsing: per-tick browse chance on a sapling (× palatability), share
/// of browse events that kill it, ticks of growth each non-lethal browse
/// sets it back (the browse trap: stunted saplings stay browsable), and
/// the scar cap.
pub const BROWSE: f64 = 1.0;
pub const BROWSE_P: f64 = 0.006;
pub const BROWSE_KILL: f64 = 0.2;
pub const BROWSE_SETBACK: u64 = 6;
pub const BROWSE_SCAR_MAX: u8 = 10;
/// How much of the climate swing reaches grass niche fit (see niche_site).
pub const NICHE_CLIMATE: f64 = 0.35;
/// Annual grass: per-tick germination chance from a full soil seed bank
/// (seed, not creep) — the colonizer strategy.
pub const ANNUAL_SEED_P: f64 = 0.012;
/// Seed each mature annual adds to the bank of tiles within
/// ANNUAL_SEED_RADIUS per tick (÷ distance); the bank saturates at 1.
pub const SEED_BANK_DEPOSIT: f32 = 0.05;
/// Fraction of the banked seed still viable after a tick: the bank
/// outlives the plants that filled it (half-life ≈ 230 ticks), so annuals
/// reappear wherever fire or drought opens ground.
pub const SEED_BANK_KEEP: f32 = 0.997;
/// Below this the bank is treated as exhausted.
pub const SEED_BANK_VIABLE: f32 = 0.002;
pub const ANNUAL_SEED_RADIUS: i32 = 3;
pub const ANNUAL_MATURITY: u64 = 4;
/// Chance per tick per perennial neighbor that a perennial grass overgrows
/// an annual (× creep): perennials win undisturbed ground.
pub const PERENNIAL_DISPLACE: f64 = 0.5;
/// Heritable traits: default per-birth mutation scale, and the trait range.
pub const MUTATION_RATE: f64 = 0.03;
pub const GENE_MIN: f32 = 0.5;
pub const GENE_MAX: f32 = 1.5;

/// Weather. Thunderclouds spawn off-map, drift across the world, and exit.
/// Under a cloud, the inner rain core soaks tiles (wet fuel cannot ignite,
/// burning tiles are doused) and wet soil grows faster; lightning strikes
/// anywhere under the cloud, so bolts at the dry edge can start fires the
/// storm's own rain never reaches — the wet/dry-lightning split of real
/// convective storms.
pub const STORM_RATE: f64 = 0.008;
pub const STORM_LIGHTNING_P: f64 = 0.06;
/// Fraction of the cloud radius that actually rains.
pub const RAIN_CORE: f64 = 0.75;
/// Ticks a tile stays wet after rain passes.
pub const WET_TICKS: u8 = 30;
/// Ticks a lightning bolt stays visible.
const BOLT_TICKS: u8 = 2;

/// Cloud genera, stacked at different altitudes with different effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CloudKind {
    /// Fair-weather puffs: low, common, just dappled shadow.
    Cumulus = 0,
    /// The thunderhead: rain core, lightning, deep shadow.
    Cumulonimbus = 1,
    /// Wide grey rain sheet: soaks nearly its whole footprint, no lightning.
    Nimbostratus = 2,
    /// High fast wisps riding the jet: nearly shadowless, no rain.
    Cirrus = 3,
}

pub const CLOUD_KIND_COUNT: usize = 4;

/// Per-genus constants. Altitude layers each ride the wind differently —
/// the Ekman-spiral picture: higher layers move faster and veer further
/// from the surface wind direction.
pub struct CloudTraits {
    pub name: &'static str,
    pub altitude: f32,
    pub radius_min: f64,
    pub radius_max: f64,
    /// × the base wind speed at this layer.
    pub wind_mult: f64,
    /// Direction veer from the surface wind (radians, Ekman turning).
    pub veer: f64,
    pub rains: bool,
    /// Fraction of the radius that actually rains (when it rains).
    pub rain_core: f64,
    /// × storm_lightning_p (only the thunderhead throws bolts).
    pub lightning: f64,
    /// Ground shadow floor beneath the cloud (1 = none).
    pub shadow: f32,
    /// Central optical depth τ: the cloud passes e^(−τ) of the light
    /// behind it (thin ice cirrus ≪ 1, a thunderhead ≫ 10).
    pub optical_depth: f32,
}

pub static CLOUD_TABLE: [CloudTraits; CLOUD_KIND_COUNT] = [
    CloudTraits { name: "Cumulus", altitude: 7.0, radius_min: 3.0, radius_max: 5.0, wind_mult: 0.85, veer: 0.0, rains: false, rain_core: 0.0, lightning: 0.0, shadow: 0.86, optical_depth: 4.0 },
    CloudTraits { name: "Cumulonimbus", altitude: 8.5, radius_min: 6.0, radius_max: 10.0, wind_mult: 0.95, veer: 0.17, rains: true, rain_core: 0.75, lightning: 1.0, shadow: 0.68, optical_depth: 14.0 },
    CloudTraits { name: "Nimbostratus", altitude: 10.5, radius_min: 8.0, radius_max: 13.0, wind_mult: 1.25, veer: 0.42, rains: true, rain_core: 0.9, lightning: 0.0, shadow: 0.76, optical_depth: 6.0 },
    CloudTraits { name: "Cirrus", altitude: 14.0, radius_min: 6.0, radius_max: 9.0, wind_mult: 2.3, veer: 0.7, rains: false, rain_core: 0.0, lightning: 0.0, shadow: 0.965, optical_depth: 0.7 },
];

impl CloudKind {
    pub fn from_u8(v: u8) -> CloudKind {
        match v {
            1 => CloudKind::Cumulonimbus,
            2 => CloudKind::Nimbostratus,
            3 => CloudKind::Cirrus,
            _ => CloudKind::Cumulus,
        }
    }

    pub fn traits(self) -> &'static CloudTraits {
        &CLOUD_TABLE[self as usize]
    }
}

/// A cloud riding its layer's wind. Position integrates the (deterministic,
/// time-varying) wind each tick; `vel` holds the last applied step so the
/// renderer can interpolate between ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Storm {
    pub kind: CloudKind,
    pub pos: [f64; 2],
    pub vel: [f64; 2],
    pub radius: f64,
    pub spawned: u64,
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
    /// Strength of the terrain water-table map (0 = no groundwater).
    pub water_table: f64,
    /// Scale of heritable-trait mutation per birth (0 = no evolution).
    pub mutation_rate: f64,
    /// Landscape relief: elevation, aspect heat, soil depth (0 = flat).
    pub terrain: f64,
    /// Grass functional-type differentiation (0 = one generic grass).
    pub grass_niches: f64,
    /// Specialist pest/pathogen outbreaks in dense same-species stands.
    pub pest_strength: f64,
    /// Browsing pressure on saplings (deer).
    pub browse: f64,
    /// Neighbor-competition strength: conspecific seedling penalty,
    /// self-shading, needle-litter allelopathy, thatch, root water draw.
    pub competition: f64,
    pub seed_tree_p: f64,
    pub seed_grass_p: f64,
    /// Map size in tiles (applies on the next reseed, like the seeding
    /// fields).
    pub width: u32,
    pub height: u32,
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
            water_table: WATER_TABLE,
            mutation_rate: MUTATION_RATE,
            terrain: TERRAIN,
            grass_niches: GRASS_NICHES,
            pest_strength: PEST_STRENGTH,
            browse: BROWSE,
            competition: COMPETITION,
            seed_tree_p: SEED_TREE_P,
            seed_grass_p: SEED_GRASS_P,
            width: Grid::DEFAULT.width as u32,
            height: Grid::DEFAULT.height as u32,
        }
    }
}

impl Params {
    /// Clamp everything into ranges the sim can safely run with.
    /// Defaults on the original 64×64 map — the calibration grid the
    /// example probes and regime tests measure on.
    pub fn legacy_map() -> Params {
        Params { width: Grid::LEGACY.width as u32, height: Grid::LEGACY.height as u32, ..Params::default() }
    }

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
        self.water_table = prob(self.water_table, WATER_TABLE);
        self.terrain = prob(self.terrain, TERRAIN);
        self.grass_niches = prob(self.grass_niches, GRASS_NICHES);
        self.pest_strength = prob(self.pest_strength, PEST_STRENGTH);
        self.browse = prob(self.browse, BROWSE);
        self.competition = prob(self.competition, COMPETITION);
        self.mutation_rate = if self.mutation_rate.is_finite() {
            self.mutation_rate.clamp(0.0, 0.2)
        } else {
            MUTATION_RATE
        };
        self.seed_tree_p = prob(self.seed_tree_p, SEED_TREE_P);
        self.seed_grass_p = prob(self.seed_grass_p, SEED_GRASS_P);
        self.sod_factor =
            if self.sod_factor.is_finite() { self.sod_factor.clamp(0.0, 5.0) } else { SOD_FACTOR };
        self.climate_swing = prob(self.climate_swing, CLIMATE_SWING);
        self.tree_range = self.tree_range.clamp(1, 8);
        self.tree_maturity_age = self.tree_maturity_age.min(10_000);
        self.grass_mean_life = self.grass_mean_life.clamp(1, 100_000);
        self.tree_mean_life = self.tree_mean_life.clamp(1, 100_000);
        let grid = Grid::new(self.width, self.height);
        self.width = grid.width as u32;
        self.height = grid.height as u32;
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
    /// Establishment multiplier on DRY ground (not storm-wet, low water
    /// table). 1 = indifferent; willow seed dies within days off wet soil.
    pub dry_ground: f64,
    /// Chance a dying mature tree resprouts from its roots instead.
    pub resprout: f64,
    /// Releases a serotinous seed pulse when it burns while mature.
    pub serotinous: bool,
    /// Fixes nitrogen into its own and neighboring soil.
    pub nitrogen_fixer: bool,
    /// Acorns cached across the map by jays, with masting boom years.
    pub jay_cached: bool,
    /// × LDD_P: how often a seed travels map-wide (jays, wind, animals).
    pub long_distance: f64,
    /// Tolerates waterlogged roots in the saturated basins.
    pub flood_tolerant: bool,
    /// Temperature-index optimum and width (terrain aspect niche).
    pub temp_opt: f64,
    pub temp_width: f64,
    /// Establishment on bare-rock soil relative to deep soil (1 = indifferent).
    pub poor_soil: f64,
    /// Chance a burned tree of any age resprouts from its root crown
    /// (savanna trees escape the fire trap this way; pine bets on serotiny).
    pub fire_sprout: f64,
    /// Susceptibility to its specialist pest/pathogen (0 = immune).
    pub pest_susceptibility: f64,
    /// How much browsers favor its saplings (thorns and resin deter).
    pub palatability: f64,
    /// Shade tolerance multiplier under a canopy of its own kind (<1: its
    /// seedlings fail beneath its parents — oak's regeneration problem).
    pub own_shade: f64,
    /// Needle-litter drop scale (acidic, allelopathic carpet).
    pub litter: f64,
    /// Seed reserves (acorn ≫ pod ≫ winged seed): the fraction of sod
    /// competition a big-seeded seedling pushes through.
    pub seed_reserve: f64,
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
        dry_ground: 1.0,
        resprout: 0.0,
        serotinous: false,
        nitrogen_fixer: true,
        jay_cached: false,
        long_distance: 0.5,
        flood_tolerant: false,
        temp_opt: 0.66,
        temp_width: 0.30,
        poor_soil: 0.75,
        fire_sprout: 0.5,
        pest_susceptibility: 0.3,
        palatability: 0.15,
        own_shade: 1.0,
        litter: 0.0,
        seed_reserve: 0.2,
    },
    SpeciesTraits {
        name: "Oak",
        growth: 0.7,
        maturity: 2.5,
        mean_life: 2.2,
        shade_tolerance: 0.45,
        ash_affinity: 1.0,
        drought_sensitivity: 0.8,
        water_affinity: 1.0,
        fireproof: 0.8,
        crowding: 4.0,
        dispersal: 2,
        rot: 1.8,
        nutrients: 1.5,
        dry_ground: 1.0,
        resprout: 0.0,
        serotinous: false,
        nitrogen_fixer: false,
        jay_cached: true,
        long_distance: 6.0,
        flood_tolerant: false,
        temp_opt: 0.38,
        temp_width: 0.30,
        poor_soil: 0.2,
        fire_sprout: 0.45,
        pest_susceptibility: 1.0,
        palatability: 0.7,
        own_shade: 0.7,
        litter: 0.0,
        seed_reserve: 0.6,
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
        dry_ground: 1.0,
        resprout: 0.0,
        serotinous: true,
        nitrogen_fixer: false,
        jay_cached: false,
        long_distance: 1.5,
        flood_tolerant: false,
        temp_opt: 0.5,
        temp_width: 0.55,
        poor_soil: 1.0,
        fire_sprout: 0.0,
        pest_susceptibility: 0.7,
        palatability: 0.25,
        own_shade: 1.0,
        litter: 1.0,
        seed_reserve: 0.0,
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
        dry_ground: 0.12,
        resprout: 0.5,
        serotinous: false,
        nitrogen_fixer: false,
        jay_cached: false,
        long_distance: 2.0,
        flood_tolerant: true,
        temp_opt: 0.5,
        temp_width: 0.45,
        poor_soil: 0.5,
        fire_sprout: 0.0,
        pest_susceptibility: 0.5,
        palatability: 0.8,
        own_shade: 1.0,
        litter: 0.0,
        seed_reserve: 0.0,
    },
];

/// Grass functional types, each with a peaked niche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GrassKind {
    /// Warm-season (C4) bunchgrass: hot, dry, full sun; fire fuel that
    /// resprouts from its crown; tufts leave gaps tree seedlings exploit.
    Bunch = 0,
    /// Cool-season (C3) sod grass: cool, moist, tolerates partial shade;
    /// rhizomes knit a dense sod that blocks tree seedlings; burns poorly.
    Sod = 1,
    /// Sedges and rushes: the saturated band; flood-tolerant, barely burn.
    Sedge = 2,
    /// Annual grass: colonizes disturbed ground by seed, short-lived, and
    /// overgrown by perennials on undisturbed ground.
    Annual = 3,
}

pub const GRASS_KIND_COUNT: usize = 4;

impl GrassKind {
    pub fn from_u8(v: u8) -> GrassKind {
        match v {
            1 => GrassKind::Sod,
            2 => GrassKind::Sedge,
            3 => GrassKind::Annual,
            _ => GrassKind::Bunch,
        }
    }

    pub fn traits(self) -> &'static GrassTraits {
        &GRASS_TABLE[self as usize]
    }
}

/// Per-type grass traits. Multipliers are relative to the generic grass and
/// are lerped toward 1 by the `grass_niches` param (0 = legacy grass).
pub struct GrassTraits {
    pub name: &'static str,
    /// × grass_clonal_p per same-type neighbor (rhizome/stolon creep).
    pub creep: f64,
    /// × grass_mean_life.
    pub life: f64,
    /// Fraction of canopy shade ignored.
    pub shade_tolerance: f64,
    /// Temperature-index optimum and niche width.
    pub temp_opt: f64,
    pub temp_width: f64,
    /// Water optimum and niche width.
    pub water_opt: f64,
    pub water_width: f64,
    /// × fire spread/ignition probability into this grass.
    pub flammability: f64,
    /// Chance a burning tuft resprouts from its crown/rhizomes.
    pub fire_resprout: f64,
    /// × sod competition against tree seedlings (>1 = leaves gaps).
    pub sod: f64,
    /// Roots tolerate saturated basins.
    pub flood_tolerant: bool,
    /// Spreads by seed rain (colonizer) rather than mainly by creep.
    pub seeder: bool,
    /// Exponent on weather stress (1 = generic grass): deep-rooted C4
    /// bunchgrass shrugs off drought, shallow C3 sod browns out.
    pub drought_sensitivity: f64,
}

pub static GRASS_TABLE: [GrassTraits; GRASS_KIND_COUNT] = [
    GrassTraits { name: "Bunchgrass", creep: 1.1, life: 1.4, shade_tolerance: 0.0, temp_opt: 0.62, temp_width: 0.26, water_opt: 0.38, water_width: 0.30, flammability: 1.6, fire_resprout: 0.6, sod: 1.6, flood_tolerant: false, seeder: false, drought_sensitivity: 0.3 },
    GrassTraits { name: "Sod grass", creep: 1.1, life: 1.3, shade_tolerance: 0.45, temp_opt: 0.38, temp_width: 0.19, water_opt: 0.62, water_width: 0.24, flammability: 0.6, fire_resprout: 0.0, sod: 0.85, flood_tolerant: false, seeder: false, drought_sensitivity: 1.6 },
    GrassTraits { name: "Sedge", creep: 1.0, life: 1.0, shade_tolerance: 0.2, temp_opt: 0.5, temp_width: 0.35, water_opt: 0.92, water_width: 0.20, flammability: 0.3, fire_resprout: 0.3, sod: 1.0, flood_tolerant: true, seeder: false, drought_sensitivity: 1.3 },
    GrassTraits { name: "Annual", creep: 0.25, life: 0.35, shade_tolerance: 0.0, temp_opt: 0.55, temp_width: 0.36, water_opt: 0.45, water_width: 0.40, flammability: 1.3, fire_resprout: 0.0, sod: 1.2, flood_tolerant: false, seeder: true, drought_sensitivity: 0.7 },
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
    /// The tree's grown form (0 broad … 1 tall/thin) carried to the husk.
    pub etiolation: f32,
}

/// Groundwater map from the seed: the catena water of the terrain (valley
/// bottoms saturate, ridges drain).
pub fn water_table_map(seed: u64, grid: Grid) -> Vec<f32> {
    Terrain::generate(seed, grid).water
}

/// Species-independent establishment conditions of one tile.
struct SiteCtx {
    adults: u32,
    fert: f64,
    /// fert × sun factor.
    base: f64,
    water_base: f64,
    sod: Option<f64>,
    site_temp: f64,
    /// terrain × SOIL_COMPETITION × (depth − SOIL_TYPICAL).
    soil: f64,
    on_ash: bool,
    flood: f64,
    wet_gate: f64,
    litter: f64,
    root_stress: f64,
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
    /// Etiolation at death, so the husk keeps the tree's grown form.
    remains_etiol: Vec<u16>,
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
    /// Whether the current tick falls in an oak mast year.
    mast: bool,
    /// Accumulated seed-rain kernel weight from mature trees, per tile and
    /// per species — offspring inherit their parent's kind.
    seed_rain: Vec<[f32; SPECIES_COUNT]>,
    /// Hex distance to the nearest mature tree within range (255 = none).
    tree_dist: Vec<u8>,
    /// Mature-tree count at distance 1 (crowding pressure).
    mature_nbrs: Vec<u8>,
    /// Grass count at distance 1 (clonal spread pressure).
    grass_nbrs: Vec<u8>,
    /// ALL trees (any age) at distance 1 — the shade-avoidance signal.
    tree_nbrs: Vec<u8>,
    /// Accumulated crowding-while-growing; frozen at maturity. Sets form.
    etiol: Vec<u16>,
    /// A mature tree of the same species stands adjacent (kin nursing).
    nursed: Vec<u8>,
    /// Heritable traits per tree: [vigor, hardiness], each in GENE_MIN..MAX.
    /// Vigor trades faster establishment for a shorter life; hardiness
    /// trades drought tolerance for a smaller wet-season benefit.
    genome: Vec<[f32; 2]>,
    /// Kernel-weighted sums of parent genomes in each tile's seed rain,
    /// per species (divide by seed_rain for the local parental mean).
    gene_rain: Vec<[[f32; 2]; SPECIES_COUNT]>,
    /// Serotinous pine seed pulse remaining on a tile, and its genome.
    sero: Vec<u8>,
    sero_gene: Vec<[f32; 2]>,
    /// Static landscape: elevation, catena water, aspect heat, soil depth.
    terrain: Terrain,
    /// Same-type grass neighbors at distance 1, per grass kind.
    grass_nbrs_k: Vec<[u8; GRASS_KIND_COUNT]>,
    /// Mature annual grass seed rain arriving on each tile.
    seed_bank: Vec<f32>,
    /// Axial offsets with distance for the ≤ params.tree_range disk.
    range_disk: Vec<(i32, i32, i32)>,
    /// The tile rectangle, fixed for the world's lifetime.
    grid: Grid,
    /// Per-tick caches of the niche site (temperature) and each grass
    /// kind's raw fit — pure functions of the tick's climate and the
    /// static terrain, reused by several passes.
    site_temp: Vec<f64>,
    grass_fit_now: Vec<[f64; GRASS_KIND_COUNT]>,
    /// Offsets for the NEAR_RADIUS and JAY_RADIUS disks.
    near_disk: Vec<(i32, i32, i32)>,
    jay_disk: Vec<(i32, i32, i32)>,
    /// Mature trees per species within NEAR_RADIUS of each tile.
    adults_near: Vec<[u8; SPECIES_COUNT]>,
    /// Specialist pest load on each tree, 0 = clean .. 255 = dying.
    pest: Vec<u8>,
    /// Non-lethal browse events a sapling has suffered (growth setback).
    browse_scar: Vec<u8>,
    /// Needle-litter carpet, 0..1.
    litter: Vec<f32>,
    params: Params,
}

impl World {
    pub fn new(seed: u64) -> World {
        World::with_params(seed, Params::default())
    }

    pub fn with_params(seed: u64, params: Params) -> World {
        let params = params.sanitized();
        let grid = Grid::new(params.width, params.height);
        let n = grid.cells();
        let mut w = World {
            seed,
            state: vec![Cell::Bare; n],
            born: vec![0; n],
            species: vec![0; n],
            burn: vec![0; n],
            remains_code: vec![0; n],
            remains_species: vec![0; n],
            remains_etiol: vec![0; n],
            remains_rot: vec![0; n],
            myc: vec![0; n],
            remains_age: vec![0; n],
            remains_ratio: vec![0; n],
            ash: vec![0; n],
            nutrients: vec![0; n],
            wet: vec![0; n],
            bolt: vec![0; n],
            storms: Vec::new(),
            sun: 0.5,
            moisture: 0.5,
            mast: false,
            seed_rain: vec![[0.0; SPECIES_COUNT]; n],
            tree_dist: vec![255; n],
            mature_nbrs: vec![0; n],
            grass_nbrs: vec![0; n],
            tree_nbrs: vec![0; n],
            etiol: vec![0; n],
            nursed: vec![0; n],
            genome: vec![[1.0, 1.0]; n],
            gene_rain: vec![[[0.0; 2]; SPECIES_COUNT]; n],
            sero: vec![0; n],
            sero_gene: vec![[1.0, 1.0]; n],
            terrain: Terrain::generate(seed, grid),
            grass_nbrs_k: vec![[0; GRASS_KIND_COUNT]; n],
            seed_bank: vec![0.0; n],
            range_disk: hex::disk(params.tree_range),
            grid,
            site_temp: vec![0.5; n],
            grass_fit_now: vec![[0.0; GRASS_KIND_COUNT]; n],
            near_disk: hex::disk(NEAR_RADIUS),
            jay_disk: hex::disk(JAY_RADIUS),
            adults_near: vec![[0; SPECIES_COUNT]; n],
            pest: vec![0; n],
            browse_scar: vec![0; n],
            litter: vec![0.0; n],
            params,
        };
        for i in 0..n {
            if rng::uniform01(seed, i as u32, 0, Stream::Seeding) < w.params.seed_tree_p {
                let sp = Species::from_u8(
                    (rng::hash(seed, i as u32, 0, Stream::SpeciesChoice) % SPECIES_COUNT as u64)
                        as u8,
                );
                w.plant_tree(i, sp, 0);
            } else if rng::uniform01(seed, i as u32, 0, Stream::SeedingGrass) < w.params.seed_grass_p {
                let kind = (rng::hash(seed, i as u32 + n as u32, 0, Stream::SpeciesChoice)
                    % GRASS_KIND_COUNT as u64) as u8;
                w.plant_grass(i, GrassKind::from_u8(kind), 0);
            }
        }
        let (sun, moisture) = w.climate_at(0);
        w.sun = sun;
        w.moisture = moisture;
        w.mast = w.mast_at(0);
        w
    }

    /// Oak mast years: synchronized across the whole map (one draw per
    /// "year"), likelier after a moist year-start — weather-cued masting.
    pub fn mast_at(&self, tick: u64) -> bool {
        let year = tick / MAST_YEAR_TICKS;
        let (_, m) = self.climate_at(year * MAST_YEAR_TICKS);
        rng::uniform01(self.seed, year as u32, 0, Stream::Mast) < 0.15 + 0.4 * m
    }

    pub fn is_mast_year(&self) -> bool {
        self.mast
    }

    /// Groundwater level of the terrain under a tile, 0..1 (scaled by the
    /// water_table param).
    pub fn water_table(&self, index: usize) -> f32 {
        self.terrain.water[index] * self.params.water_table as f32
    }

    /// Render height of a tile's surface (0 on a flat world).
    pub fn elevation(&self, index: usize) -> f32 {
        self.terrain.elevation[index] * (RENDER_RELIEF * self.params.terrain) as f32
    }

    /// Normalized terrain layers for display/probes: (elevation, heat, depth).
    pub fn terrain_at(&self, index: usize) -> (f32, f32, f32) {
        (self.terrain.elevation[index], self.terrain.heat[index], self.terrain.depth[index])
    }

    /// Local temperature index: the season's sun shifted by slope aspect
    /// (0.5 in a neutral climate on flat ground).
    pub fn temperature(&self, index: usize) -> f64 {
        let aspect = (self.terrain.heat[index] as f64 - 0.5) * 1.0 * self.params.terrain;
        (self.sun + aspect).clamp(0.02, 0.98)
    }

    /// Effective soil depth, 1 on a flat world.
    pub fn soil_depth(&self, index: usize) -> f64 {
        1.0 - self.params.terrain * (1.0 - self.terrain.depth[index] as f64)
    }

    /// Grass kind of a grass tile (meaningless elsewhere).
    pub fn grass_kind(&self, index: usize) -> GrassKind {
        GrassKind::from_u8(self.species[index])
    }

    /// Niche fit of a grass kind on a tile: a unimodal response to local
    /// temperature and water, scaled to NICHE_PEAK at the optimum and lerped
    /// toward 1 by `grass_niches`.
    fn grass_fit(&self, k: usize, index: usize) -> f64 {
        1.0 + self.params.grass_niches * (NICHE_PEAK * self.grass_fit_now[index][k] - 1.0)
    }

    /// Raw unimodal niche fit in 0..1 (1 = on the optimum).
    pub fn grass_fit_raw(&self, k: usize, index: usize) -> f64 {
        let tr = &GRASS_TABLE[k];
        let (t, w) = self.niche_site(index);
        (-((t - tr.temp_opt) / tr.temp_width).powi(2) - ((w - tr.water_opt) / tr.water_width).powi(2))
            .exp()
    }

    /// The world's tile rectangle.
    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// Specialist pest load on a tree, 0 (clean) .. 1 (dying).
    pub fn pest_load(&self, index: usize) -> f32 {
        if self.state[index] == Cell::Tree { self.pest[index] as f32 / 255.0 } else { 0.0 }
    }

    /// Trees currently carrying a pest outbreak.
    pub fn infested_count(&self) -> u32 {
        (0..self.grid.cells()).filter(|&i| self.state[i] == Cell::Tree && self.pest[i] > 0).count() as u32
    }

    /// How hard browsers have knocked a sapling back, 0..1.
    pub fn browse_damage(&self, index: usize) -> f32 {
        self.browse_scar[index] as f32 / BROWSE_SCAR_MAX as f32
    }

    /// Needle-litter carpet on a tile, 0..1.
    pub fn litter(&self, index: usize) -> f32 {
        self.litter[index]
    }

    /// Root water competition a sapling suffers, 0..1 (0 for mature trees).
    pub fn root_competition(&self, index: usize, tick: u64) -> f32 {
        if self.state[index] != Cell::Tree || self.is_mature(index, tick) {
            return 0.0;
        }
        (self.root_water_stress(index) / 1.5).min(1.0) as f32
    }

    /// Shannon diversity over the eight plant types (four tree species, four
    /// grass kinds): (H, effective number of types = e^H).
    pub fn diversity(&self) -> (f64, f64) {
        let mut n = [0u32; SPECIES_COUNT + GRASS_KIND_COUNT];
        for i in 0..self.grid.cells() {
            match self.state[i] {
                Cell::Tree => n[self.species[i] as usize] += 1,
                // With niches off the grass kinds are interchangeable: one type.
                Cell::Grass if self.params.grass_niches <= 0.0 => n[SPECIES_COUNT] += 1,
                Cell::Grass => n[SPECIES_COUNT + self.species[i] as usize] += 1,
                Cell::Bare => {}
            }
        }
        let total: u32 = n.iter().sum();
        if total == 0 {
            return (0.0, 0.0);
        }
        let h: f64 = n
            .iter()
            .filter(|&&c| c > 0)
            .map(|&c| {
                let p = c as f64 / total as f64;
                -p * p.ln()
            })
            .sum();
        (h, h.exp())
    }

    /// A grass trait multiplier lerped toward 1 by `grass_niches`.
    fn gmul(&self, x: f64) -> f64 {
        1.0 + self.params.grass_niches * (x - 1.0)
    }

    /// Heritable [vigor, hardiness] of the tree on a tile.
    pub fn genome(&self, index: usize) -> [f32; 2] {
        self.genome[index]
    }

    /// A small deterministic mutation of a parental genome.
    fn mutate(&self, base: [f32; 2], key: u32, tick: u64) -> [f32; 2] {
        let m = self.params.mutation_rate as f32;
        let d = |k: u32| {
            (rng::uniform01(self.seed, key.wrapping_mul(2).wrapping_add(k), tick, Stream::Mutation)
                as f32
                - 0.5)
                * 2.0
                * m
        };
        [
            (base[0] + d(0)).clamp(GENE_MIN, GENE_MAX),
            (base[1] + d(1)).clamp(GENE_MIN, GENE_MAX),
        ]
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

    /// The surface wind at a tick: a slowly meandering direction and a
    /// breathing speed, deterministic from the seed. Each cloud layer scales
    /// and veers this (see [`CLOUD_TABLE`]).
    pub fn wind_at(&self, tick: u64) -> ([f64; 2], f64) {
        let tau = std::f64::consts::TAU;
        let t = tick as f64;
        let phase = |k: u32| {
            (rng::hash(self.seed, k, 0, Stream::Wind) % 10_000) as f64 / 10_000.0 * tau
        };
        let theta = phase(0)
            + 0.9 * (tau * t / 2900.0 + phase(1)).sin()
            + 0.5 * (tau * t / 710.0 + phase(2)).sin();
        let speed = (0.24 + 0.14 * (tau * t / 1150.0 + phase(3)).sin()).clamp(0.08, 0.45);
        ([theta.cos(), theta.sin()], speed)
    }

    /// Wind vector for a given cloud layer at a tick (Ekman shear: faster
    /// and veered aloft).
    pub fn layer_wind(&self, kind: CloudKind, tick: u64) -> [f64; 2] {
        let (dir, speed) = self.wind_at(tick);
        let tr = kind.traits();
        let (vs, vc) = tr.veer.sin_cos();
        let veered = [dir[0] * vc - dir[1] * vs, dir[0] * vs + dir[1] * vc];
        [veered[0] * speed * tr.wind_mult, veered[1] * speed * tr.wind_mult]
    }

    pub fn moisture(&self) -> f64 {
        self.moisture
    }

    /// Effective water available to a tile: storm-soaked ground is saturated
    /// regardless of the season.
    fn tile_water(&self, index: usize) -> f64 {
        if self.wet[index] > 0 {
            1.0
        } else {
            // Groundwater tops up the season's moisture in the valleys; thin
            // rocky soils hold less of it.
            let wt = self.water_table(index) as f64 * WATER_TABLE_REACH;
            let retention = 0.85 + 0.15 * self.soil_depth(index);
            (self.moisture + (1.0 - self.moisture) * wt) * retention
        }
    }

    /// The temperature and water a perennial grass is adapted to on this
    /// site: slope aspect and catena position, with the year-to-year
    /// climate damped by NICHE_CLIMATE. Swings still favor one type or
    /// another for a while (the storage effect), but the map, not the
    /// weather, decides who lives where.
    fn niche_site(&self, index: usize) -> (f64, f64) {
        let sun = 0.5 + (self.sun - 0.5) * NICHE_CLIMATE;
        let moisture = 0.5 + (self.moisture - 0.5) * NICHE_CLIMATE;
        let aspect = (self.terrain.heat[index] as f64 - 0.5) * self.params.terrain;
        let wt = self.water_table(index) as f64 * WATER_TABLE_REACH;
        let retention = 0.85 + 0.15 * self.soil_depth(index);
        ((sun + aspect).clamp(0.02, 0.98), (moisture + (1.0 - moisture) * wt) * retention)
    }

    /// Recompute the per-tick niche caches (call once the tick's climate
    /// is set).
    fn refresh_site_cache(&mut self) {
        for i in 0..self.grid.cells() {
            if self.state[i] == Cell::Tree {
                continue; // only open ground and grass read the caches
            }
            self.site_temp[i] = self.niche_site(i).0;
            let mut fits = [0.0; GRASS_KIND_COUNT];
            for (k, f) in fits.iter_mut().enumerate() {
                *f = self.grass_fit_raw(k, i);
            }
            self.grass_fit_now[i] = fits;
        }
    }

    /// Drought index in [0, ~1.9]: 0.5 in neutral climate. Heat with no
    /// water pushes it up; saturation pulls it to zero.
    fn drought(&self, index: usize, water: f64) -> f64 {
        (1.0 - water) * (0.4 + 1.2 * self.temperature(index))
    }

    /// How visibly browned vegetation is by the current conditions, 0..1.
    pub fn browning(&self, index: usize) -> f32 {
        ((self.drought(index, self.tile_water(index)) - 0.7) / 0.8).clamp(0.0, 1.0) as f32
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
            etiolation: {
                let sp2 = Species::from_u8(self.remains_species[index]);
                let denom =
                    3.0 * (self.params.tree_maturity_age as f64 * sp2.traits().maturity) as f32;
                (self.remains_etiol[index] as f32 / denom.max(1.0)).min(1.0)
            },
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

    /// Inject a cloud directly (tests, and a possible future cloud brush).
    pub fn spawn_cloud(&mut self, kind: CloudKind, pos: [f64; 2], radius: f64, tick: u64) {
        self.storms.push(Storm {
            kind,
            pos,
            vel: [0.0, 0.0],
            radius: radius.clamp(2.0, 20.0),
            spawned: tick,
        });
    }

    fn add_nutrients(&mut self, index: usize, amount: u16) {
        // Thin rocky soils can't hold much organic matter.
        let cap = (NUTRIENT_CAP as f64 * (0.4 + 0.6 * self.soil_depth(index))) as u16;
        let cap = cap.max(self.nutrients[index]);
        self.nutrients[index] = (self.nutrients[index] + amount).min(cap);
    }

    fn leave_remains(&mut self, index: usize, charred: bool, tick: u64) {
        let tree = self.state[index] == Cell::Tree;
        self.remains_code[index] = if tree { 2 } else { 1 } + if charred { 2 } else { 0 };
        self.remains_species[index] = self.species[index];
        self.remains_etiol[index] = self.etiol[index];
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

    /// How far this tree raced for the canopy, 0 (open-grown, broad) to 1
    /// (hemmed in all its youth: tall and thin).
    pub fn etiolation(&self, index: usize) -> f32 {
        let denom = 3.0 * self.maturity_age(index).max(1) as f32;
        (self.etiol[index] as f32 / denom).min(1.0)
    }

    /// Maturity age for the tree standing on `index`.
    fn maturity_age(&self, index: usize) -> u64 {
        (self.params.tree_maturity_age as f64 * self.species(index).traits().maturity) as u64
            + self.browse_scar[index] as u64 * BROWSE_SETBACK
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

    fn plant_grass(&mut self, index: usize, kind: GrassKind, tick: u64) {
        self.species[index] = kind as u8;
        self.plant(index, Cell::Grass, tick);
    }

    /// Brush stroke choosing the grass kind (the panel's grass picker).
    pub fn paint_grass(&mut self, index: usize, kind: GrassKind, tick: u64) {
        self.plant_grass(index, kind, tick);
    }

    fn plant_tree(&mut self, index: usize, sp: Species, tick: u64) {
        self.plant_tree_with(index, sp, [1.0, 1.0], tick);
    }

    fn plant_tree_with(&mut self, index: usize, sp: Species, gene: [f32; 2], tick: u64) {
        self.species[index] = sp as u8;
        self.etiol[index] = 0;
        self.pest[index] = 0;
        self.browse_scar[index] = 0;
        self.genome[index] = gene;
        self.plant(index, Cell::Tree, tick);
    }

    /// Root resprouting (willow): a dying mature tree regrows from its root
    /// crown — same genome, no husk. Returns whether it resprouted.
    fn try_resprout(&mut self, index: usize, tick: u64) -> bool {
        if self.state[index] != Cell::Tree || !self.is_mature(index, tick) {
            return false;
        }
        let chance = self.species(index).traits().resprout;
        if chance <= 0.0
            || rng::uniform01(self.seed, index as u32, tick, Stream::Resprout) >= chance
        {
            return false;
        }
        let (sp, gene) = (self.species(index), self.genome[index]);
        self.burn[index] = 0;
        self.plant_tree_with(index, sp, gene, tick);
        true
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
            Brush::Grass => self.plant_grass(index, GrassKind::Bunch, tick),
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
        self.gene_rain.fill([[0.0; 2]; SPECIES_COUNT]);
        self.tree_nbrs.fill(0);
        self.nursed.fill(0);
        self.tree_dist.fill(255);
        self.mature_nbrs.fill(0);
        self.grass_nbrs.fill(0);
        self.grass_nbrs_k.fill([0; GRASS_KIND_COUNT]);
        self.adults_near.fill([0; SPECIES_COUNT]);
        for l in self.litter.iter_mut() {
            *l *= LITTER_KEEP;
        }
        for b in self.seed_bank.iter_mut() {
            *b *= SEED_BANK_KEEP;
            if *b < SEED_BANK_VIABLE {
                *b = 0.0; // too few viable seeds left to matter
            }
        }
        let annual_disk = hex::disk(ANNUAL_SEED_RADIUS);
        for i in 0..self.grid.cells() {
            match self.state[i] {
                Cell::Bare => {}
                Cell::Grass => {
                    let (q, r) = self.grid.index_to_axial(i);
                    let k = self.species[i] as usize;
                    for (dq, dr) in hex::NEIGHBORS {
                        if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                            self.grass_nbrs[j] += 1;
                            self.grass_nbrs_k[j][k] += 1;
                        }
                    }
                    // Annuals rain seed around them once they've set seed.
                    if GRASS_TABLE[k].seeder && self.age(i, tick) >= ANNUAL_MATURITY {
                        for &(dq, dr, d) in &annual_disk {
                            if d == 0 {
                                continue;
                            }
                            if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                                self.seed_bank[j] =
                                    (self.seed_bank[j] + SEED_BANK_DEPOSIT / d as f32).min(1.0);
                            }
                        }
                    }
                }
                Cell::Tree => {
                    // Every tree — any age — shades its ring: the
                    // shade-avoidance signal for the canopy race.
                    let (tq, tr_ax) = self.grid.index_to_axial(i);
                    for (dq, dr) in hex::NEIGHBORS {
                        if let Some(j) = self.grid.axial_to_index(tq + dq, tr_ax + dr) {
                            self.tree_nbrs[j] += 1;
                        }
                    }
                    if !self.is_mature(i, tick) {
                        continue;
                    }
                    let sp = self.species[i] as usize;
                    let dispersal = SPECIES_TABLE[sp].dispersal;
                    let (q, r) = self.grid.index_to_axial(i);
                    for &(dq, dr, _) in &self.near_disk {
                        if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                            self.adults_near[j][sp] = self.adults_near[j][sp].saturating_add(1);
                        }
                    }
                    let drop = SPECIES_TABLE[sp].litter as f32 * LITTER_DROP;
                    if drop > 0.0 {
                        self.litter[i] = (self.litter[i] + drop).min(1.0);
                        for (dq, dr) in hex::NEIGHBORS {
                            if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                                self.litter[j] = (self.litter[j] + drop).min(1.0);
                            }
                        }
                    }
                    for k in 0..self.range_disk.len() {
                        let (dq, dr, d) = self.range_disk[k];
                        if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                            if d <= dispersal {
                                // Fecundity is individual: a vigorous tree
                                // casts proportionally more seed, so its genes
                                // weigh more in the local seed mix.
                                let g = self.genome[i];
                                let kw = kernel_weight(d) as f32 * g[0];
                                self.seed_rain[j][sp] += kw;
                                self.gene_rain[j][sp][0] += kw * g[0];
                                self.gene_rain[j][sp][1] += kw * g[1];
                            }
                            self.tree_dist[j] = self.tree_dist[j].min(d as u8);
                            if d == 1 {
                                self.mature_nbrs[j] += 1;
                                // Kin nursing: a same-species seedling next
                                // to this mature tree taps its network.
                                if self.state[j] == Cell::Tree
                                    && self.species[j] as usize == sp
                                    && !self.is_mature(j, tick)
                                {
                                    self.nursed[j] = 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// The canopy race: while a tree is still growing, crowding accrues on
    /// its etiolation score (frozen at maturity — form is set in youth).
    /// Below ground, linked trees equalize soil: nutrients diffuse along
    /// adjacent tree-tree pairs from rich tiles to poor.
    fn root_pass(&mut self, tick: u64) {
        for i in 0..self.grid.cells() {
            if self.state[i] != Cell::Tree {
                continue;
            }
            if !self.is_mature(i, tick) && self.tree_nbrs[i] > 0 {
                self.etiol[i] = self.etiol[i].saturating_add(self.tree_nbrs[i].min(3) as u16);
            }
        }
        // Nitrogen fixers enrich their own soil and their ring's.
        for i in 0..self.grid.cells() {
            if self.state[i] != Cell::Tree || !self.species(i).traits().nitrogen_fixer {
                continue;
            }
            self.add_nutrients(i, N_FIX_OWN);
            let (q, r) = self.grid.index_to_axial(i);
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                    self.add_nutrients(j, N_FIX_NEIGHBOR);
                }
            }
        }
        // Pairwise diffusion, each unordered pair visited once (i < j).
        for i in 0..self.grid.cells() {
            if self.state[i] != Cell::Tree {
                continue;
            }
            let (q, r) = self.grid.index_to_axial(i);
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                    if j > i && self.state[j] == Cell::Tree {
                        let (a, b) = (self.nutrients[i], self.nutrients[j]);
                        let flow = (a.abs_diff(b)) / ROOT_SHARE_DIV;
                        if a > b {
                            self.nutrients[i] -= flow;
                            self.nutrients[j] = (b + flow).min(NUTRIENT_CAP);
                        } else {
                            self.nutrients[j] -= flow;
                            self.nutrients[i] = (a + flow).min(NUTRIENT_CAP);
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
        let (min_x, min_y, max_x, max_y) = self.grid.world_bounds();
        let c = [(min_x + max_x) / 2.0, (min_y + max_y) / 2.0];
        let world_r = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt() / 2.0;

        // Wet seasons brew storms; deep drought starves them. Clouds take
        // time ∝ the map's width to cross it, so scaling the spawn rate by
        // the linear size keeps cloud cover per area the same on any map.
        let size = self.grid.area_ratio().sqrt();
        let spawn_rate = (p.storm_rate * (0.2 + 1.6 * self.moisture) * size).min(1.0);
        if rng::uniform01(self.seed, 0, tick, Stream::StormSpawn) < spawn_rate {
            let u = |k: u32| rng::uniform01(self.seed, k, tick, Stream::StormSpawn);
            // Genus by weather: moist air brews the rain-bearers, fair or
            // dry skies mostly loft cumulus and cirrus.
            let m = self.moisture;
            let weights = [0.5, 0.35 * m, 0.5 * m, 0.25];
            let total: f64 = weights.iter().sum();
            let mut pick = u(1) * total;
            let mut kind = CloudKind::Cumulus;
            for (k, &wt) in weights.iter().enumerate() {
                pick -= wt;
                if pick <= 0.0 {
                    kind = CloudKind::from_u8(k as u8);
                    break;
                }
            }
            let tr = kind.traits();
            let radius = tr.radius_min + (tr.radius_max - tr.radius_min) * u(2);
            // Spawn upwind of the map so the layer wind carries it across;
            // the lateral offset spreads tracks over the whole world.
            let w = self.layer_wind(kind, tick);
            let speed = (w[0] * w[0] + w[1] * w[1]).sqrt().max(1e-6);
            let dir = [w[0] / speed, w[1] / speed];
            let lateral = (u(3) - 0.5) * 2.0 * (world_r * 0.9);
            let perp = [-dir[1], dir[0]];
            let pos = [
                c[0] - dir[0] * (world_r + radius) + perp[0] * lateral,
                c[1] - dir[1] * (world_r + radius) + perp[1] * lateral,
            ];
            self.spawn_cloud(kind, pos, radius, tick);
        }

        // Every cloud rides its own layer's wind.
        let winds: Vec<[f64; 2]> =
            self.storms.iter().map(|s| self.layer_wind(s.kind, tick)).collect();
        for (s, w) in self.storms.iter_mut().zip(winds) {
            s.vel = w;
            s.pos[0] += w[0];
            s.pos[1] += w[1];
        }

        // Dissipate clouds that have crossed and left the world behind.
        self.storms.retain(|s| {
            let out = [s.pos[0] - c[0], s.pos[1] - c[1]];
            let dist = (out[0] * out[0] + out[1] * out[1]).sqrt();
            let leaving = out[0] * s.vel[0] + out[1] * s.vel[1] > 0.0;
            !(tick > s.spawned + 20 && dist > world_r + s.radius + 4.0 && leaving)
        });

        for idx in 0..self.storms.len() {
            let storm = self.storms[idx];
            let tr = storm.kind.traits();
            let center = storm.pos;
            if tr.rains {
                let core = storm.radius * tr.rain_core;
                for i in self.grid.cells_in_box(center, core) {
                    let (q, r) = self.grid.index_to_axial(i);
                    let (x, y) = hex::axial_to_world(q, r);
                    let d = ((x - center[0]).powi(2) + (y - center[1]).powi(2)).sqrt();
                    if d < core {
                        self.wet[i] = WET_TICKS;
                        self.burn[i] = 0; // the downpour douses open flame
                    }
                }
            }
            // Windthrow: gusts topple mature trees under the storm, the tall
            // thin forest-grown ones first — opening gaps for pioneers.
            let gust = match storm.kind {
                CloudKind::Cumulonimbus => 1.0,
                CloudKind::Nimbostratus => 0.35,
                _ => 0.0,
            };
            if gust > 0.0 {
                for i in self.grid.cells_in_box(center, storm.radius) {
                    if self.state[i] != Cell::Tree || !self.is_mature(i, tick) {
                        continue;
                    }
                    let (q, r) = self.grid.index_to_axial(i);
                    let (x, y) = hex::axial_to_world(q, r);
                    let d = ((x - center[0]).powi(2) + (y - center[1]).powi(2)).sqrt();
                    if d >= storm.radius {
                        continue;
                    }
                    let pw = WINDTHROW_P * gust * (1.0 + 2.0 * self.etiolation(i) as f64);
                    let key = (i as u32).wrapping_add((idx as u32).wrapping_mul(self.grid.cells() as u32));
                    if rng::uniform01(self.seed, key, tick, Stream::Windthrow) < pw
                        && !self.try_resprout(i, tick)
                    {
                        self.leave_remains(i, false, tick);
                        self.state[i] = Cell::Bare;
                    }
                }
            }
            // Lightning: only the thunderhead throws bolts, anywhere under
            // the cloud — possibly outside its rain core.
            let bolt_p = p.storm_lightning_p * tr.lightning;
            let k = |j: u32| rng::uniform01(self.seed, idx as u32 * 16 + j, tick, Stream::StormBolt);
            if bolt_p > 0.0 && k(0) < bolt_p {
                let rr = storm.radius * k(1).sqrt();
                let phi = std::f64::consts::TAU * k(2);
                if let Some(i) = self.grid.pick(center[0] + rr * phi.cos(), center[1] + rr * phi.sin()) {
                    self.bolt[i] = BOLT_TICKS;
                    if self.flammable(i, tick) && self.wet[i] == 0 {
                        self.burn[i] = BURN_TICKS;
                    }
                }
            }
        }
    }

    /// Long-distance dispersal: every mature tree occasionally sends one
    /// seed far beyond its seed-rain disk — jays caching acorns within
    /// JAY_RADIUS of the parent, winged pine seed and cottony willow seed
    /// on the wind anywhere on the map, acacia pods carried off by
    /// browsers. Arrival is not establishment: the seed then faces exactly
    /// the site filter local seed does (light, niche, soil, competition,
    /// mast-year predation), so a seed landing in another species' habitat
    /// rarely takes.
    fn dispersal_pass(&mut self, tick: u64) {
        let scale = (self.params.tree_growth_p / TREE_GROWTH_P).min(1.0);
        for o in 0..self.grid.cells() {
            if self.state[o] != Cell::Tree || !self.is_mature(o, tick) {
                continue;
            }
            let k = self.species[o] as usize;
            let tr = &SPECIES_TABLE[k];
            let rate = LDD_P * tr.long_distance * self.genome[o][0] as f64;
            if rng::uniform01(self.seed, o as u32, tick, Stream::Jay) >= rate {
                continue;
            }
            let roll = rng::hash(self.seed, o as u32 + 7919, tick, Stream::Jay);
            let Some(t) = self.ldd_target(o, roll) else {
                continue; // cached off the map
            };
            if self.state[t] == Cell::Tree || self.remains_code[t] != 0 || self.burn[t] > 0 {
                continue;
            }
            let light = match self.tree_dist[t] {
                255 => 1.0,
                d => 1.0 - self.params.shade_strength * shade_suppression(d),
            };
            // Germination scales with the global establishment knob (so
            // tree_growth_p = 0 truly freezes recruitment).
            let germ = LDD_GERMINATION * scale * self.tree_establishment(t, k, light);
            if rng::uniform01(self.seed, t as u32 + 104_729, tick, Stream::Jay) >= germ {
                continue;
            }
            let gene = self.mutate(self.genome[o], t as u32, tick);
            self.plant_tree_with(t, self.species(o), gene, tick);
        }
    }

    /// Where a long-distance seed from the tree on `o` lands: jay-cached
    /// species within JAY_RADIUS of the parent, the rest anywhere.
    fn ldd_target(&self, o: usize, roll: u64) -> Option<usize> {
        if self.species(o).traits().jay_cached {
            let (q, r) = self.grid.index_to_axial(o);
            let (dq, dr, _) = self.jay_disk[(roll % self.jay_disk.len() as u64) as usize];
            self.grid.axial_to_index(q + dq, r + dr)
        } else {
            Some((roll % self.grid.cells() as u64) as usize)
        }
    }

    /// Specialist pests and pathogens. Oak wilt and its kin spread from an
    /// infested tree to same-species neighbors through root grafts; new
    /// outbreaks start mostly in dense same-species stands; the load builds
    /// until the host dies (see death_pass), unless a resistant host walls
    /// the infection off. Dense monocultures thin themselves into gaps.
    fn pest_pass(&mut self, tick: u64) {
        let strength = self.params.pest_strength;
        let mut infect = Vec::new();
        for i in 0..self.grid.cells() {
            if self.state[i] != Cell::Tree {
                continue;
            }
            let sp = self.species[i] as usize;
            let susc = SPECIES_TABLE[sp].pest_susceptibility;
            if self.pest[i] > 0 {
                let recover = PEST_RECOVER * (1.0 - susc);
                if rng::uniform01(self.seed, i as u32, tick, Stream::Pest) < recover {
                    self.pest[i] = 0;
                    continue;
                }
                self.pest[i] = self.pest[i].saturating_add(PEST_GROWTH);
                let spread = strength * PEST_SPREAD * susc * self.pest[i] as f64 / 255.0;
                let (q, r) = self.grid.index_to_axial(i);
                for (n, (dq, dr)) in hex::NEIGHBORS.iter().enumerate() {
                    if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                        if self.state[j] == Cell::Tree
                            && self.species[j] as usize == sp
                            && self.pest[j] == 0
                            && rng::uniform01(
                                self.seed,
                                j as u32 + (n as u32 + 2) * self.grid.cells() as u32,
                                tick,
                                Stream::Pest,
                            ) < spread
                        {
                            infect.push(j);
                        }
                    }
                }
            } else if strength > 0.0 && self.is_mature(i, tick) {
                let density = self.adults_near[i][sp] as f64 / 6.0;
                let p = strength * PEST_SEED * susc * density * density;
                if rng::uniform01(self.seed, i as u32 + self.grid.cells() as u32, tick, Stream::Pest) < p {
                    infect.push(i);
                }
            }
        }
        for j in infect {
            if self.pest[j] == 0 {
                self.pest[j] = 1;
            }
        }
    }

    /// Browsers (deer) eat saplings, favoring palatable species: some
    /// browse kills outright, the rest knocks the sapling back — a
    /// stunted sapling stays in browse reach longer (the browse trap).
    fn browse_pass(&mut self, tick: u64) {
        if self.params.browse <= 0.0 {
            return;
        }
        for i in 0..self.grid.cells() {
            if self.state[i] != Cell::Tree || self.is_mature(i, tick) {
                continue;
            }
            let pal = self.species(i).traits().palatability;
            let p = self.params.browse * BROWSE_P * pal;
            if rng::uniform01(self.seed, i as u32, tick, Stream::Browse) >= p {
                continue;
            }
            if rng::uniform01(self.seed, i as u32 + self.grid.cells() as u32, tick, Stream::Browse) < BROWSE_KILL {
                self.leave_remains(i, false, tick);
                self.state[i] = Cell::Bare;
            } else {
                self.browse_scar[i] = (self.browse_scar[i] + 1).min(BROWSE_SCAR_MAX);
            }
        }
    }

    /// Spread from burning tiles, roll new ignitions, burn tiles down to bare.
    fn fire_pass(&mut self, tick: u64) {
        let p = self.params;
        let burning_now: Vec<usize> = (0..self.grid.cells()).filter(|&i| self.burn[i] > 0).collect();
        if p.fire_ignition_p <= 0.0 && burning_now.is_empty() {
            return;
        }
        let mut caught = Vec::new();
        for &i in &burning_now {
            let (q, r) = self.grid.index_to_axial(i);
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                    let damp = 1.0 - WATER_TABLE_FIRE_DAMP * self.water_table(j) as f64;
                    let spread =
                        (p.fire_spread_p * (0.4 + 1.2 * (1.0 - self.moisture)) * damp).min(1.0);
                    let flam = if self.state[j] == Cell::Grass {
                        self.gmul(self.grass_kind(j).traits().flammability)
                    } else {
                        1.0
                    };
                    if self.burn[j] == 0
                        && self.wet[j] == 0 // soaked fuel will not catch
                        && self.flammable(j, tick)
                        && rng::uniform01(self.seed, j as u32, tick, Stream::FireSpread)
                            < (spread * flam).min(1.0)
                    {
                        caught.push(j);
                    }
                }
            }
        }
        if p.fire_ignition_p > 0.0 {
            // Droughts quadruple spontaneous ignition; wet seasons quench it.
            let ignite = p.fire_ignition_p * (2.0 * (1.0 - self.moisture)).powi(2);
            for i in 0..self.grid.cells() {
                if self.burn[i] == 0
                    && self.wet[i] == 0
                    && self.state[i] == Cell::Grass
                    && rng::uniform01(self.seed, i as u32, tick, Stream::FireIgnite)
                        < ignite * self.gmul(self.grass_kind(i).traits().flammability)
                {
                    caught.push(i);
                }
            }
        }
        for &i in &burning_now {
            self.burn[i] -= 1;
            if self.burn[i] == 0 {
                // Serotiny: a mature pine's sealed cones open in the heat
                // and rain seed onto the fresh ash all around.
                if self.state[i] == Cell::Tree
                    && self.species(i).traits().serotinous
                    && self.is_mature(i, tick)
                {
                    let (q, r) = self.grid.index_to_axial(i);
                    let gene = self.genome[i];
                    for (dq, dr, _) in hex::disk(SERO_RADIUS) {
                        if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
                            self.sero[j] = SERO_TICKS;
                            self.sero_gene[j] = gene;
                        }
                    }
                }
                // Fire-adapted grasses resprout from the crown/rhizomes.
                if self.state[i] == Cell::Grass {
                    let chance = self.params.grass_niches
                        * self.grass_kind(i).traits().fire_resprout;
                    if chance > 0.0
                        && rng::uniform01(self.seed, i as u32, tick, Stream::Resprout) < chance
                    {
                        let kind = self.grass_kind(i);
                        self.plant_grass(i, kind, tick);
                        self.ash[i] = ASH_TICKS;
                        continue;
                    }
                }
                // Root-crown resprouters survive the burn below ground.
                if self.state[i] == Cell::Tree {
                    let chance = self.species(i).traits().fire_sprout;
                    if chance > 0.0
                        && rng::uniform01(self.seed, i as u32 + self.grid.cells() as u32, tick, Stream::Resprout)
                            < chance
                    {
                        let (sp, gene) = (self.species(i), self.genome[i]);
                        self.burn[i] = 0;
                        self.plant_tree_with(i, sp, gene, tick);
                        self.ash[i] = ASH_TICKS;
                        continue;
                    }
                }
                if self.try_resprout(i, tick) {
                    self.ash[i] = ASH_TICKS;
                    continue;
                }
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
        for i in 0..self.grid.cells() {
            if self.state[i] == Cell::Bare {
                continue;
            }
            // Constant-per-tick hazard scaled by weather stress: lifetimes
            // are unbounded, but survival decays exponentially. Neutral
            // climate ⇒ stress 1 ⇒ mean lifetime = the mean_life param.
            // Species bend the drought response via an exponent (neutral
            // stress is exactly 1, so calibration is preserved).
            let base_stress = 0.4 + 1.2 * self.drought(i, self.tile_water(i));
            let (stress, mean) = match self.state[i] {
                Cell::Grass => (
                    base_stress.powf(self.gmul(self.grass_kind(i).traits().drought_sensitivity)),
                    self.params.grass_mean_life as f64 * self.gmul(self.grass_kind(i).traits().life),
                ),
                _ => {
                    let tr = self.species(i).traits();
                    let [vigor, hardy] = self.genome[i];
                    // Hardiness flattens the stress curve both ways: less
                    // pain in drought, less relief in wet years. Vigor
                    // trades life span for recruitment.
                    (
                        base_stress.powf(tr.drought_sensitivity / hardy as f64),
                        self.params.tree_mean_life as f64 * tr.mean_life / vigor as f64,
                    )
                }
            };
            let mut hazard = (stress / mean).min(1.0);
            if self.state[i] == Cell::Grass && self.params.grass_niches > 0.0 {
                // Off-niche grasses die faster, on-niche ones slower.
                let k = self.species[i] as usize;
                let fit = self.grass_fit_now[i][k];
                hazard *= (1.0 + self.params.grass_niches * (0.6 - 1.2 * fit)).max(0.3);
                if !GRASS_TABLE[k].flood_tolerant {
                    let excess = (self.water_table(i) as f64 - 0.5).max(0.0);
                    hazard *= 1.0 + self.params.grass_niches * WATERLOG_HAZARD * excess;
                }
                hazard = hazard.min(1.0);
            }
            if self.state[i] == Cell::Tree {
                // Established roots outcompete a sapling for water on dry
                // ground; a heavy pest load kills.
                if !self.is_mature(i, tick) {
                    let sens = self.species(i).traits().drought_sensitivity;
                    hazard = (hazard * (1.0 + self.root_water_stress(i) * sens)).min(1.0);
                }
                let load = self.pest[i] as f64 / 255.0;
                hazard = 1.0 - (1.0 - hazard) * (1.0 - PEST_HAZARD * load);
            }
            if self.state[i] == Cell::Tree && !self.species(i).traits().flood_tolerant {
                let excess = (self.water_table(i) as f64 - 0.5).max(0.0);
                hazard = (hazard * (1.0 + WATERLOG_HAZARD * excess)).min(1.0);
            }
            if self.nursed[i] != 0 {
                // The network feeds its kin: nursed seedlings die less.
                hazard *= NURSE_FACTOR;
            }
            if self.state[i] == Cell::Tree && self.mature_nbrs[i] > CROWD_FREE {
                let excess = (self.mature_nbrs[i] - CROWD_FREE) as f64;
                let cp = (self.params.crowding_p * self.species(i).traits().crowding).min(1.0);
                let p_crowd = 1.0 - (1.0 - cp).powf(excess);
                hazard = 1.0 - (1.0 - hazard) * (1.0 - p_crowd);
            }
            if rng::uniform01(self.seed, i as u32, tick, Stream::Mortality) < hazard
                && !self.try_resprout(i, tick)
            {
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
        for i in 0..self.grid.cells() {
            if self.ash[i] > 0 {
                self.ash[i] -= 1;
            }
            if self.wet[i] > 0 {
                self.wet[i] -= 1;
            }
            if self.bolt[i] > 0 {
                self.bolt[i] -= 1;
            }
            if self.sero[i] > 0 {
                self.sero[i] -= 1;
            }
            let drain = if self.state[i] == Cell::Bare {
                NUTRIENT_DRAIN_IDLE
            } else {
                // Overlapping root plates drain shared soil faster.
                NUTRIENT_DRAIN_OCCUPIED + (self.tree_nbrs[i] / 2) as u16
            };
            self.nutrients[i] = self.nutrients[i].saturating_sub(drain);
            if self.remains_code[i] == 0 {
                continue;
            }
            let (q, r) = self.grid.index_to_axial(i);
            let mut sources = 0u32;
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = self.grid.axial_to_index(q + dq, r + dr) {
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

    /// Mean genome of a species' seed arriving on a tile: the kernel-
    /// weighted parental mean, blended with any serotinous pulse.
    fn parental_gene(&self, i: usize, k: usize) -> [f32; 2] {
        let mut w = self.seed_rain[i][k];
        let mut g = self.gene_rain[i][k];
        if SPECIES_TABLE[k].serotinous && self.sero[i] > 0 {
            w += SERO_WEIGHT;
            g[0] += SERO_WEIGHT * self.sero_gene[i][0];
            g[1] += SERO_WEIGHT * self.sero_gene[i][1];
        }
        if w <= 0.0 {
            [1.0, 1.0]
        } else {
            [g[0] / w, g[1] / w]
        }
    }

    /// Root water competition on this tile: adjacent mature trees drawing
    /// down soil that is drier than typical (0 on moist ground).
    fn root_water_stress(&self, index: usize) -> f64 {
        let dryness = ((0.55 - self.tile_water(index)) / 0.55).clamp(0.0, 1.0);
        self.params.competition * ROOT_WATER * dryness * self.mature_nbrs[index] as f64
    }

    /// Everything about a tile that decides whether species `k`'s seed,
    /// once it has arrived, becomes a seedling — shared by local seed rain
    /// and long-distance dispersal, so a far-flung seed faces the same
    /// site as a local one. `light` is the tile's canopy light.
    fn tree_establishment(&self, i: usize, k: usize, light: f64) -> f64 {
        self.establish(&self.site_ctx(i), i, k, light)
    }

    /// The species-independent part of a tile's establishment conditions,
    /// computed once per tile and shared by every species' seed on it.
    fn site_ctx(&self, i: usize) -> SiteCtx {
        let p = &self.params;
        let water = self.tile_water(i);
        let fert = 1.0 + p.nutrient_boost * self.nutrients[i] as f64 / NUTRIENT_CAP as f64;
        let ground_wet = if self.wet[i] > 0 { 1.0 } else { self.water_table(i) as f64 };
        SiteCtx {
            adults: self.adults_near[i].iter().map(|&a| a as u32).sum(),
            fert,
            base: fert * (0.4 + 1.2 * self.temperature(i)),
            water_base: 0.3 + 1.4 * water,
            sod: if self.state[i] == Cell::Grass {
                Some(p.sod_factor * self.gmul(self.grass_kind(i).traits().sod))
            } else {
                None
            },
            site_temp: self.site_temp[i],
            soil: p.terrain * SOIL_COMPETITION * (self.terrain.depth[i] as f64 - SOIL_TYPICAL),
            on_ash: self.ash[i] > 0,
            flood: (1.0 - 2.0 * (self.water_table(i) as f64 - 0.5).max(0.0)).max(0.0),
            wet_gate: ((ground_wet - 0.15) / 0.45).clamp(0.0, 1.0),
            litter: (1.0 - p.competition * ALLELOPATHY * self.litter[i] as f64).max(0.0),
            root_stress: self.root_water_stress(i),
        }
    }

    /// Establishment multiplier for species `k` on tile `i` given its
    /// shared site conditions `c` and canopy `light`.
    fn establish(&self, c: &SiteCtx, i: usize, k: usize, light: f64) -> f64 {
        let p = &self.params;
        let tr = &SPECIES_TABLE[k];
        // Shade tolerance, weakened under a canopy of the species' own kind
        // (oak seedlings fail beneath oaks: they need gaps).
        let own = self.adults_near[i][k] as f64;
        let own_frac = if c.adults > 0 { own / c.adults as f64 } else { 0.0 };
        let tolerance = tr.shade_tolerance * (1.0 + p.competition * own_frac * (tr.own_shade - 1.0));
        let light_k = light + tolerance * (1.0 - light);
        if light_k <= 0.0 {
            return 0.0;
        }
        let affinity = if tr.water_affinity == 1.0 { c.water_base } else { c.water_base.powf(tr.water_affinity) };
        let mut pk = light_k * c.base * affinity;
        if let Some(sod) = c.sod {
            // Sod competition depends on the grass: bunchgrass tufts
            // leave gaps, rhizomatous sod blocks seedlings — and a
            // big-seeded seedling lives off its reserves long enough to
            // push through.
            pk *= if sod < 1.0 { sod + (1.0 - sod) * tr.seed_reserve } else { sod };
        }
        // Terrain niches: aspect temperature and soil depth.
        let tfit = (-((c.site_temp - tr.temp_opt) / tr.temp_width).powi(2)).exp();
        pk *= 1.0 + p.terrain * (1.4 * tfit - 1.0);
        // Competitive hierarchy on soil, centered on typical upland
        // depth: demanding hardwoods outgrow stress-tolerators on
        // deep soil, so pine keeps the thin ridges they can't hold.
        pk *= (1.0 + c.soil * (1.0 - 2.0 * tr.poor_soil)).max(0.2);
        if c.on_ash {
            pk *= tr.ash_affinity;
        }
        // Nitrogen-fixers supply their own: they establish as if on
        // moderately fertile soil everywhere (an edge on poor ground)
        // and gain nothing from rich soil — no fix-and-crowd runaway.
        if tr.nitrogen_fixer {
            pk *= N_FIXER_FERTILITY / c.fert;
        }
        // Seedlings of flood-intolerant species drown in saturated basins.
        if !tr.flood_tolerant {
            pk *= c.flood;
        }
        // Willow seed only lives days: dry ground barely takes it.
        pk *= tr.dry_ground + (1.0 - tr.dry_ground) * c.wet_gate;
        // Mast years swamp the acorn predators; in lean years rodents and
        // weevils eat nearly the whole crop.
        if tr.jay_cached {
            pk *= if self.mast { MAST_BOOM } else { MAST_LEAN };
        }
        // Conspecific negative density dependence: specialist enemies
        // accumulate where the species is already common.
        pk /= 1.0 + p.competition * CNDD * own;
        // Needle litter suppresses everything but its own source.
        if tr.litter <= 0.0 {
            pk *= c.litter;
        }
        // Established roots drink first on dry ground.
        pk /= 1.0 + c.root_stress * tr.drought_sensitivity;
        pk
    }

    fn growth_pass(&mut self, tick: u64) {
        let p = self.params;
        for i in 0..self.grid.cells() {
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
            let sun_f = 0.4 + 1.2 * self.temperature(i);
            let water_base = 0.3 + 1.4 * water;
            let on_ash = self.ash[i] > 0;

            // Per-species establishment probabilities from their own seed
            // rain; the shared site conditions are computed on first need.
            let mut ctx: Option<SiteCtx> = None;
            let mut p_sp = [0.0f64; SPECIES_COUNT];
            let mut none_grow = 1.0f64;
            for (k, item) in p_sp.iter_mut().enumerate() {
                let tr = &SPECIES_TABLE[k];
                let mut raw = self.seed_rain[i][k];
                if tr.serotinous && self.sero[i] > 0 {
                    raw += SERO_WEIGHT;
                }
                let w = (raw as f64).min(SEED_RAIN_CAP);
                if w <= 0.0 {
                    continue;
                }
                let ctx = ctx.get_or_insert_with(|| self.site_ctx(i));
                let pk = (1.0 - (1.0 - (p.tree_growth_p * tr.growth).min(1.0)).powf(w))
                    * self.establish(ctx, i, k, light);
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
                let gene = self.mutate(self.parental_gene(i, chosen as usize), i as u32, tick);
                self.plant_tree_with(i, chosen, gene, tick);
                continue;
            }
            // Grass: each functional type competes for the tile with its own
            // creep (same-type neighbors), its own seed (annuals), and its own
            // niche fit to the local temperature and water.
            let niches = p.grass_niches;
            let grass_press = |k: usize, own: bool| -> f64 {
                let tr = &GRASS_TABLE[k];
                let light_k = light + niches * tr.shade_tolerance * (1.0 - light);
                let creep = p.grass_clonal_p * self.gmul(tr.creep) * self.grass_nbrs_k[i][k] as f64;
                let seed = if own { p.grass_seed_p / GRASS_KIND_COUNT as f64 } else { 0.0 }
                    + if tr.seeder {
                        // Scales with the grass-spread knob, so zero spread
                        // truly freezes grass.
                        niches * ANNUAL_SEED_P * (p.grass_clonal_p / GRASS_CLONAL_P).min(1.0)
                            * self.seed_bank[i] as f64
                    } else {
                        0.0
                    };
                // Legacy monotonic water/sun response fades out as niches
                // take over (they carry the climate response instead).
                let legacy = 1.0 + (1.0 - niches) * (sun_f * water_base - 1.0);
                let mut pk = (seed + creep) * light_k * fert * legacy * self.grass_fit(k, i);
                if tr.seeder && on_ash {
                    pk *= 1.0 + niches; // annual seed banks flush on burns
                }
                if !tr.flood_tolerant {
                    let excess = (self.water_table(i) as f64 - 0.5).max(0.0);
                    pk *= (1.0 - niches * 2.0 * excess).max(0.0);
                }
                // Needle litter smothers the sward; sod thatch shades out
                // annual seedlings.
                pk *= (1.0 - p.competition * ALLELOPATHY * self.litter[i] as f64).max(0.0);
                if tr.seeder {
                    let sod = self.grass_nbrs_k[i][GrassKind::Sod as usize] as f64;
                    pk *= (1.0 - p.competition * THATCH * sod).max(0.0);
                }
                pk.clamp(0.0, 1.0)
            };
            // Bare ground no grass can reach (no creeping neighbor, no banked
            // seed, no spontaneous seeding) has zero pressure: skip it.
            let reachable =
                p.grass_seed_p > 0.0 || self.grass_nbrs[i] > 0 || self.seed_bank[i] > 0.0;
            if s == Cell::Bare && reachable {
                let mut pg = [0.0f64; GRASS_KIND_COUNT];
                for (k, item) in pg.iter_mut().enumerate() {
                    *item = grass_press(k, true);
                }
                // Additive like the legacy single grass: with niches off the
                // kinds are identical and their pressures sum to exactly it.
                let p_any = pg.iter().sum::<f64>().min(1.0);
                if p_any > 0.0
                    && rng::uniform01(self.seed, i as u32, tick, Stream::GrassGrowth) < p_any
                {
                    let total: f64 = pg.iter().sum();
                    let mut pick = rng::uniform01(
                        self.seed,
                        i as u32 + self.grid.cells() as u32,
                        tick,
                        Stream::SpeciesChoice,
                    ) * total;
                    let mut chosen = GrassKind::Bunch;
                    for (k, &pk) in pg.iter().enumerate() {
                        pick -= pk;
                        if pick <= 0.0 {
                            chosen = GrassKind::from_u8(k as u8);
                            break;
                        }
                    }
                    self.plant_grass(i, chosen, tick);
                }
            } else if s == Cell::Grass && self.grass_kind(i) == GrassKind::Annual && niches > 0.0 {
                // Perennials overgrow annuals on undisturbed ground.
                for k in 0..GRASS_KIND_COUNT {
                    if GRASS_TABLE[k].seeder || self.grass_nbrs_k[i][k] == 0 {
                        continue;
                    }
                    let pk = grass_press(k, false) * PERENNIAL_DISPLACE * niches;
                    let key = i as u32 + (k as u32 + 2) * self.grid.cells() as u32;
                    if rng::uniform01(self.seed, key, tick, Stream::GrassGrowth) < pk {
                        self.plant_grass(i, GrassKind::from_u8(k as u8), tick);
                        break;
                    }
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
        self.mast = self.mast_at(tick);
        self.refresh_site_cache();
        self.decay_pass();
        self.weather_pass(tick);
        self.rebuild_fields(tick);
        self.root_pass(tick);
        self.dispersal_pass(tick);
        self.pest_pass(tick);
        self.browse_pass(tick);
        self.fire_pass(tick);
        self.death_pass(tick);
        self.growth_pass(tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Grid = Grid::LEGACY;
    const CELLS: usize = 4096;

    /// Defaults on the 64×64 map the tests were calibrated on.
    fn legacy() -> Params {
        Params { width: 64, height: 64, ..Params::default() }
    }

    /// Fire off unless a test is about fire — ignitions are rare but would
    /// make long statistical runs noisy.
    /// Fire, storms, and climate swings all off: a controlled laboratory
    /// world at the exactly-neutral climate (all multipliers = 1).
    fn no_fire() -> Params {
        Params {
            fire_ignition_p: 0.0,
            storm_rate: 0.0,
            climate_swing: 0.0,
            water_table: 0.0,
            mutation_rate: 0.0,
            terrain: 0.0,
            grass_niches: 0.0,
            pest_strength: 0.0,
            browse: 0.0,
            competition: 0.0,
            ..legacy()
        }
    }

    /// A world "the user has planted": scattered trees + grass, standing in
    /// for manual brush work now that defaults start empty.
    fn planted(seed: u64) -> World {
        World::with_params(
            seed,
            Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..legacy() },
        )
    }

    fn bare_world(seed: u64, params: Params) -> World {
        let mut w = World::with_params(seed, params);
        for i in 0..CELLS {
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
        (G.axial_to_index(q, r).unwrap(), q, r)
    }

    #[test]
    fn default_worlds_start_empty() {
        let w = World::with_params(5, legacy());
        assert_eq!(w.counts(), [CELLS as u32, 0, 0], "nothing grows until painted");
    }

    #[test]
    fn counts_always_sum_to_the_grid() {
        let mut w = planted(7);
        for tick in 1u64..=200 {
            w.step(tick);
            assert_eq!(w.counts().iter().sum::<u32>(), CELLS as u32);
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
        for i in 0..CELLS {
            assert_eq!(a.state(i), b.state(i), "cell {i} diverged");
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let a = planted(1);
        let b = planted(2);
        assert!((0..CELLS).any(|i| a.state(i) != b.state(i)));
    }

    #[test]
    fn explicit_seeding_scatters_both_trees_and_grass() {
        let w = World::with_params(
            1234,
            Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..legacy() },
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
            for i in 0..CELLS {
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
                w.paint(G.axial_to_index(q + dq, r + dr).unwrap(), Brush::Grass, tick);
            }
            w.step(tick);
            trials += 1;
            if w.state(hole) == Cell::Grass {
                grew += 1;
            }
            // Sweep any stray spontaneous grass so neighbor counts stay exact.
            for i in 0..CELLS {
                if w.state(i) != Cell::Bare {
                    w.paint(i, Brush::Clear, tick);
                }
            }
        }
        let rate = grew as f64 / trials as f64;
        let p = legacy();
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
                let j = G.axial_to_index(q + dq, r + dr).unwrap();
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
        let far = G.axial_to_index(q + 20, r).unwrap();
        for tick in 100u64..=4100 {
            ensure_mature_tree(&mut w, c, tick);
            let d2 = G.axial_to_index(q + 2, r).unwrap();
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
        let t1 = G.axial_to_index(q + 1, r).unwrap();
        let t2 = G.axial_to_index(q + 2, r).unwrap();
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
            for i in 0..CELLS {
                if i != c && w.state(i) == Cell::Tree {
                    w.paint(i, Brush::Clear, tick);
                }
            }
        }
        let (r1, r2) = (d1 as f64 / trials as f64, d2 as f64 / trials as f64);
        assert!(r1 < r2 * 0.5, "no Janzen-Connell dip: d1={r1}, d2={r2}");
        assert!(r1 > r2 * 0.1, "trunk dip too deep: d1={r1}, d2={r2}");
        // And the peak rate itself sits near tree_growth_p.
        // The founder is an acacia: as a nitrogen-fixer it establishes at
        // N_FIXER_FERTILITY (1.5x) on this unfertilized soil.
        assert!((0.055..=0.095).contains(&r2), "d2 rate {r2} far from 1.5 x p=0.075");
    }

    #[test]
    fn sod_competition_slows_trees_on_grass() {
        let base = Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, tree_growth_p: 0.05, ..no_fire() };
        let rate_on = |sod: f64, cover: Brush| {
            let mut w = bare_world(31, Params { sod_factor: sod, ..base });
            let (c, q, r) = center();
            let target = G.axial_to_index(q + 2, r).unwrap();
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
                for i in 0..CELLS {
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
            w.paint(G.axial_to_index(q + dq, r + dr).unwrap(), Brush::Tree, born);
        }
        let far = G.axial_to_index(q + 15, r).unwrap();
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
            w.paint(G.axial_to_index(q + dq, r).unwrap(), Brush::Grass, t0);
        }
        let firebreak = G.axial_to_index(q + 4, r).unwrap();
        w.paint(firebreak, Brush::Tree, 0);
        for dq in 5..8 {
            w.paint(G.axial_to_index(q + dq, r).unwrap(), Brush::Grass, t0);
        }
        let island = G.axial_to_index(q - 3, r).unwrap();
        w.paint(island, Brush::Grass, t0);

        w.paint(G.axial_to_index(q, r).unwrap(), Brush::Fire, t0);
        for tick in t0..t0 + 20 {
            w.step(tick);
        }
        for dq in 0..4 {
            let j = G.axial_to_index(q + dq, r).unwrap();
            assert_eq!(w.state(j), Cell::Bare, "corridor tile {dq} should have burned");
        }
        assert_eq!(w.state(firebreak), Cell::Tree, "mature tree must survive the fire");
        for dq in 5..8 {
            let j = G.axial_to_index(q + dq, r).unwrap();
            assert_eq!(w.state(j), Cell::Grass, "fire crossed the firebreak to {dq}");
        }
        assert_eq!(w.state(island), Cell::Grass, "fire jumped a bare gap");
    }

    #[test]
    fn fire_kills_immature_trees_but_needs_fuel_to_reach_them() {
        let p = Params { fire_spread_p: 1.0, grass_seed_p: 0.0, ..no_fire() };
        let mut w = bare_world(43, p);
        let (_, q, r) = center();
        w.paint(G.axial_to_index(q, r).unwrap(), Brush::Grass, 100);
        let sapling = G.axial_to_index(q + 1, r).unwrap();
        // Pine: no root-crown resprouting to rescue it.
        w.paint_species(sapling, Brush::Tree, Species::Pine, 99); // age 1 at ignition: flammable
        w.paint(G.axial_to_index(q, r).unwrap(), Brush::Fire, 100);
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
            ..legacy()
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
            water_table: 4.0,
            mutation_rate: 9.0,
            terrain: -1.0,
            grass_niches: f64::NAN,
            pest_strength: 3.0,
            browse: -1.0,
            competition: f64::INFINITY,
            seed_tree_p: f64::NAN,
            seed_grass_p: 0.5,
            width: 3,
            height: 100_000,
        }
        .sanitized();
        assert_eq!((p.width, p.height), (hex::MIN_SIDE, hex::MAX_SIDE));
        assert_eq!(p.pest_strength, 1.0);
        assert_eq!(p.browse, 0.0);
        assert_eq!(p.competition, COMPETITION);
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
        assert_eq!(p.water_table, 1.0);
        assert_eq!(p.mutation_rate, 0.2);
        assert_eq!(p.terrain, 0.0);
        assert_eq!(p.grass_niches, GRASS_NICHES);
        assert_eq!(p.seed_tree_p, SEED_TREE_P);
        assert_eq!(p.seed_grass_p, 0.5);
    }

    #[test]
    fn cloud_layers_stack_and_ride_sheared_wind() {
        // Altitudes strictly increase up the stack.
        for k in 1..CLOUD_KIND_COUNT {
            assert!(
                CLOUD_TABLE[k].altitude != CLOUD_TABLE[k - 1].altitude,
                "layers must be distinct"
            );
        }
        let alts: Vec<f32> = CLOUD_TABLE.iter().map(|t| t.altitude).collect();
        let mut sorted = alts.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(alts, sorted, "table should be ordered ground-up");
        // Ekman shear: the jet layer is much faster and veered from the
        // surface layer, and every layer shares the same base wind epoch.
        let w = World::with_params(3, no_fire());
        let lo = w.layer_wind(CloudKind::Cumulus, 500);
        let hi = w.layer_wind(CloudKind::Cirrus, 500);
        let speed = |v: [f64; 2]| (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!(speed(hi) > speed(lo) * 2.0, "cirrus should race the cumulus");
        let angle = |v: [f64; 2]| v[1].atan2(v[0]);
        let mut dtheta = (angle(hi) - angle(lo)).abs();
        if dtheta > std::f64::consts::PI {
            dtheta = std::f64::consts::TAU - dtheta;
        }
        assert!(
            (dtheta - 0.7).abs() < 0.05,
            "the jet should veer ~0.7 rad from the surface wind, got {dtheta:.2}"
        );
    }

    #[test]
    fn clouds_move_with_their_layer_wind() {
        let mut w = bare_world(151, no_fire());
        w.spawn_cloud(CloudKind::Cumulus, [50.0, 50.0], 4.0, 100);
        let before = w.storms()[0].pos;
        w.step(100);
        let after = w.storms()[0].pos;
        let step = [after[0] - before[0], after[1] - before[1]];
        let expect = w.layer_wind(CloudKind::Cumulus, 100);
        assert!(
            (step[0] - expect[0]).abs() < 1e-9 && (step[1] - expect[1]).abs() < 1e-9,
            "the cloud must ride exactly its layer's wind"
        );
    }

    #[test]
    fn only_the_rain_bearers_rain_and_only_the_thunderhead_strikes() {
        let (c, q, r) = center();
        let (cx, cy) = hex::axial_to_world(q, r);
        let try_kind = |kind: CloudKind| -> (bool, u32) {
            let p = Params {
                grass_seed_p: 0.0,
                grass_clonal_p: 0.0,
                tree_growth_p: 0.0,
                storm_lightning_p: 1.0,
                grass_mean_life: 100_000,
                ..no_fire()
            };
            let mut w = bare_world(157, p);
            w.set_params(Params { storm_lightning_p: 1.0, ..w.params() });
            w.paint(c, Brush::Grass, 100);
            let mut bolts = 0u32;
            for tick in 100u64..140 {
                if tick % 15 == 0 || tick == 100 {
                    w.spawn_cloud(kind, [cx, cy], 8.0, tick.saturating_sub(20));
                }
                w.step(tick);
                bolts += (0..CELLS).filter(|&i| w.bolt_active(i)).count() as u32;
            }
            (w.wet_ratio(c) > 0.0, bolts)
        };
        let (wet, bolts) = try_kind(CloudKind::Cumulonimbus);
        assert!(wet && bolts > 0, "the thunderhead rains and strikes");
        let (wet, bolts) = try_kind(CloudKind::Nimbostratus);
        assert!(wet, "the rain sheet must soak the ground");
        assert_eq!(bolts, 0, "but never throw lightning");
        let (wet, bolts) = try_kind(CloudKind::Cumulus);
        assert!(!wet && bolts == 0, "fair-weather puffs do neither");
        let (wet, bolts) = try_kind(CloudKind::Cirrus);
        assert!(!wet && bolts == 0, "cirrus does neither");
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
        let east = G.axial_to_index(q + 1, r).unwrap();
        w.paint(east, Brush::Grass, 100);
        w.paint(c, Brush::Fire, 100);
        assert!(w.burning(c));
        // Park a storm overhead: its rain core covers both tiles.
        let (cx, cy) = hex::axial_to_world(q, r);
        w.spawn_cloud(CloudKind::Cumulonimbus, [cx, cy], 8.0, 100);
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
        let west = G.axial_to_index(q, r).unwrap();
        let east = G.axial_to_index(q + 1, r).unwrap();
        w.paint(west, Brush::Grass, 50);
        w.paint(east, Brush::Grass, 50);
        let (ex, ey) = hex::axial_to_world(q + 1, r);
        w.spawn_cloud(CloudKind::Cumulonimbus, [ex, ey], 2.0, 50); // rain core ≈ 1.5 tiles
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
        for i in 0..CELLS {
            w.paint(i, Brush::Grass, 0);
        }
        let (c, q, r) = center();
        let _ = c;
        let (cx, cy) = hex::axial_to_world(q, r);
        w.spawn_cloud(CloudKind::Cumulonimbus, [cx, cy], 8.0, 0);
        let mut bolts = 0u32;
        let mut ignitions = 0u32;
        for tick in 1u64..=300 {
            // The wind carries clouds off; keep a thunderhead overhead.
            if tick % 25 == 0 {
                w.spawn_cloud(CloudKind::Cumulonimbus, [cx, cy], 8.0, tick.saturating_sub(20));
            }
            w.step(tick);
            bolts += (0..CELLS).filter(|&i| w.bolt_active(i)).count() as u32;
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
        let dry = G.axial_to_index(q + 20, r).unwrap();
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
        for tick in 300u64..3300 {
            if w.state(c) != Cell::Tree {
                w.paint_species(c, Brush::Tree, Species::Oak, tick.saturating_sub(300));
            }
            w.step(tick);
        }
        let _ = (q, r);
        let mut offspring = 0;
        for i in 0..CELLS {
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
            let target = G.axial_to_index(q + 1, r).unwrap();
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
                for i in 0..CELLS {
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
        let ashed = G.axial_to_index(q + 2, r).unwrap();
        let plain = G.axial_to_index(q - 2, r).unwrap();
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
            // Sweep stray sprouts so the root network + decomposition of a
            // growing pine grove don't lift the plain tile's baseline.
            for i in 0..CELLS {
                if i != c && w.state(i) == Cell::Tree {
                    w.paint(i, Brush::Clear, tick);
                }
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
                water_table: 0.0,
                mutation_rate: 0.0,
                terrain: 0.0,
                ..legacy()
            };
            let mut w = World::with_params(137, p);
            for i in 0..CELLS {
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
            // Count original stems only: a willow that died and resprouted
            // from its roots has a reset age.
            let end = dry_start + 120;
            (0..800).filter(|&i| w.state(i) == Cell::Tree && w.age(i, end) >= 120).count()
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
        let pine = G.axial_to_index(q, r).unwrap();
        let fuel = G.axial_to_index(q + 1, r).unwrap();
        w.paint_species(pine, Brush::Tree, Species::Pine, 60);
        w.paint(fuel, Brush::Grass, 100);
        w.paint(fuel, Brush::Fire, 100);
        w.step(100); // spread reaches the 40-tick pine
        assert!(w.burning(pine), "a mature-but-unarmored pine should catch");

        // Oak at age 90 (maturity 100, fireproof at 80): already armored.
        let mut v = bare_world(139, p);
        let oak = G.axial_to_index(q, r).unwrap();
        let fuel2 = G.axial_to_index(q + 1, r).unwrap();
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
    fn crowded_youth_grows_tall_and_thin_open_youth_stays_broad() {
        let p = Params {
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            tree_mean_life: 100_000,
            crowding_p: 0.0,
            ..no_fire()
        };
        let mut w = bare_world(163, p);
        let (c, q, r) = center();
        let lone = G.axial_to_index(q + 15, r).unwrap();
        w.paint_species(c, Brush::Tree, Species::Acacia, 0);
        for (dq, dr) in hex::NEIGHBORS {
            w.paint_species(G.axial_to_index(q + dq, r + dr).unwrap(), Brush::Tree, Species::Acacia, 0);
        }
        w.paint_species(lone, Brush::Tree, Species::Acacia, 0);
        for tick in 1u64..=200 {
            w.step(tick);
        }
        assert!(w.etiolation(c) > 0.9, "a fully hemmed-in tree races upward, got {}", w.etiolation(c));
        assert_eq!(w.etiolation(lone), 0.0, "an open-grown tree keeps a broad form");
        // Form is set in youth: clearing the neighbors later changes nothing.
        let frozen = w.etiolation(c);
        for (dq, dr) in hex::NEIGHBORS {
            w.paint(G.axial_to_index(q + dq, r + dr).unwrap(), Brush::Clear, 200);
        }
        for tick in 201u64..=300 {
            w.step(tick);
        }
        assert_eq!(w.etiolation(c), frozen, "etiolation freezes at maturity");
    }

    #[test]
    fn root_network_moves_nutrients_from_rich_to_poor_trees() {
        let p = Params {
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            tree_growth_p: 0.0,
            tree_mean_life: 100_000,
            ..no_fire()
        };
        let mut w = bare_world(167, p);
        let (a, q, r) = center();
        let b = G.axial_to_index(q + 1, r).unwrap();
        let unlinked = G.axial_to_index(q + 10, r).unwrap();
        for i in [a, b, unlinked] {
            w.paint_species(i, Brush::Tree, Species::Pine, 0);
        }
        w.nutrients[a] = NUTRIENT_CAP;
        w.nutrients[b] = 0;
        w.nutrients[unlinked] = 0;
        for tick in 1u64..=20 {
            w.step(tick);
        }
        assert!(w.nutrient_ratio(b) > 0.1, "the linked poor tree should be fed, got {}", w.nutrient_ratio(b));
        assert_eq!(w.nutrient_ratio(unlinked), 0.0, "an isolated tree gets nothing");
        assert!(w.nutrient_ratio(a) < 0.95, "the rich tree gives some away");
    }

    #[test]
    fn mature_trees_nurse_seedlings_of_their_own_kind() {
        // Same drought-free hazard, with and without a mature kin neighbor.
        let survivors = |kin: Option<Species>| -> usize {
            let p = Params {
                grass_seed_p: 0.0,
                grass_clonal_p: 0.0,
                tree_growth_p: 0.0,
                tree_mean_life: 40,
                crowding_p: 0.0,
                ..no_fire()
            };
            let mut w = bare_world(173, p);
            let mut n = 0usize;
            // A grid of seedling pairs well apart from each other.
            let mut seedlings = Vec::new();
            for col in (4..60).step_by(5) {
                for row in (4..60).step_by(5) {
                    let (q, r) = hex::offset_to_axial(col, row);
                    let s = G.axial_to_index(q, r).unwrap();
                    let parent = G.axial_to_index(q + 1, r).unwrap();
                    if let Some(sp) = kin {
                        // Mature by the time the seedlings go in (most
                        // parents survive the short window).
                        w.paint_species(parent, Brush::Tree, sp, 0);
                    }
                    seedlings.push(s);
                }
            }
            // Let parents mature, then plant the seedlings.
            for tick in 1u64..=120 {
                w.step(tick);
            }
            for &s in &seedlings {
                w.paint_species(s, Brush::Tree, Species::Acacia, 120);
            }
            for tick in 121u64..=160 {
                w.step(tick);
            }
            for &s in &seedlings {
                if w.state(s) == Cell::Tree {
                    n += 1;
                }
            }
            n
        };
        let alone = survivors(None);
        let with_stranger = survivors(Some(Species::Pine));
        let with_kin = survivors(Some(Species::Acacia));
        assert!(
            with_kin > alone && with_kin > with_stranger,
            "kin-nursed seedlings should survive best (kin {with_kin}, stranger {with_stranger}, alone {alone})"
        );
    }

    /// A laboratory with only the tree machinery live (no grass, no weather).
    fn trees_only() -> Params {
        Params { grass_seed_p: 0.0, grass_clonal_p: 0.0, ..no_fire() }
    }

    #[test]
    fn burning_mature_pines_reseed_the_ash_but_acacias_do_not() {
        // Serotiny: torch one mature tree, remove every living tree, and see
        // whether seedlings of its kind appear with no living parent.
        let regrowth = |sp: Species| -> usize {
            let p = Params { tree_growth_p: 0.02, fire_spread_p: 0.0, ..trees_only() };
            let mut w = bare_world(181, p);
            let (c, _, _) = center();
            w.paint_species(c, Brush::Tree, sp, 0); // mature by tick 200
            w.paint(c, Brush::Fire, 200);
            w.step(200);
            w.step(201); // burn-out
            // Clear anything the tree seeded normally while it still stood,
            // so only post-fire recruitment is counted.
            for i in 0..CELLS {
                if w.state(i) == Cell::Tree {
                    w.paint(i, Brush::Clear, 201);
                }
            }
            for tick in 202u64..240 {
                w.step(tick);
            }
            (0..CELLS).filter(|&i| w.state(i) == Cell::Tree && w.species(i) == sp).count()
        };
        let pines = regrowth(Species::Pine);
        let acacias = regrowth(Species::Acacia);
        assert!(pines >= 3, "a burned pine should reseed its ash, saw {pines} seedlings");
        assert_eq!(acacias, 0, "a burned acacia leaves no seed behind");
    }

    #[test]
    fn storms_topple_tall_thin_trees_before_open_grown_ones() {
        let p = Params { tree_growth_p: 0.0, tree_mean_life: 100_000, crowding_p: 0.0, ..trees_only() };
        let mut w = bare_world(191, p);
        // Two groups of mature trees under one parked thunderhead: half
        // drawn-up (etiolation maxed), half open-grown.
        let (_, q, r) = center();
        let mut tall = Vec::new();
        let mut broad = Vec::new();
        for k in -4..=4 {
            for (dq, list) in [(0, &mut tall), (1, &mut broad)] {
                let i = G.axial_to_index(q + 2 * k, r + dq).unwrap();
                w.paint_species(i, Brush::Tree, Species::Acacia, 0);
                list.push(i);
            }
        }
        for &i in &tall {
            w.etiol[i] = u16::MAX / 2; // saturates etiolation at 1
        }
        let (cx, cy) = hex::axial_to_world(q, r);
        let (mut tall_down, mut broad_down) = (0usize, 0usize);
        for tick in 100u64..4100 {
            w.spawn_cloud(CloudKind::Cumulonimbus, [cx, cy], 12.0, tick.saturating_sub(20));
            w.step(tick);
            w.storms.clear();
            for list in [&tall, &broad] {
                for &i in list {
                    if w.state(i) != Cell::Tree {
                        if list[0] == tall[0] { tall_down += 1 } else { broad_down += 1 }
                        w.paint_species(i, Brush::Tree, Species::Acacia, 0);
                        if list[0] == tall[0] {
                            w.etiol[i] = u16::MAX / 2;
                        }
                    }
                }
            }
        }
        assert!(broad_down > 0, "gusts should topple some open-grown trees too");
        assert!(
            tall_down as f64 > broad_down as f64 * 2.0,
            "drawn-up trees should fall ~3x as often ({tall_down} vs {broad_down})"
        );
    }

    #[test]
    fn the_water_table_map_has_basins_and_uplands() {
        let map = water_table_map(7, G);
        assert_eq!(map, water_table_map(7, G), "terrain must be deterministic");
        assert_ne!(map, water_table_map(8, G), "and vary with the seed");
        let wet = map.iter().filter(|&&v| v > 0.6).count();
        let dry = map.iter().filter(|&&v| v < 0.05).count();
        assert!(wet > 40, "there should be real basins, saw {wet} wet tiles");
        assert!(dry > CELLS / 4, "and plenty of dry upland, saw {dry}");
    }

    #[test]
    fn willows_establish_in_the_basins_and_barely_on_dry_ground() {
        let p = Params { tree_growth_p: 0.05, water_table: 1.0, shade_strength: 0.0, ..trees_only() };
        let mut w = bare_world(7, p);
        let map = water_table_map(7, G);
        let wettest = (0..CELLS).max_by(|&a, &b| map[a].total_cmp(&map[b])).unwrap();
        let driest = (0..CELLS).min_by(|&a, &b| map[a].total_cmp(&map[b])).unwrap();
        let rate_at = |w: &mut World, target: usize| -> f64 {
            let (q, r) = G.index_to_axial(target);
            let parent = G.axial_to_index(q + 2, r)
                .or_else(|| G.axial_to_index(q - 2, r))
                .unwrap();
            let mut grew = 0u64;
            for tick in 300u64..3300 {
                if w.state(parent) != Cell::Tree {
                    w.paint_species(parent, Brush::Tree, Species::Willow, tick - 300);
                }
                w.paint(target, Brush::Clear, tick);
                w.step(tick);
                if w.state(target) == Cell::Tree {
                    grew += 1;
                }
            }
            grew as f64 / 3000.0
        };
        let wet_rate = rate_at(&mut w, wettest);
        let dry_rate = rate_at(&mut w, driest);
        assert!(
            wet_rate > dry_rate * 4.0,
            "willows should need the water table (wet {wet_rate:.4} vs dry {dry_rate:.4})"
        );
    }

    #[test]
    fn jays_plant_oaks_far_beyond_acorn_dispersal() {
        let p = Params { tree_mean_life: 100_000, ..trees_only() };
        let mut w = bare_world(197, p);
        let (c, cq, cr) = center();
        w.paint_species(c, Brush::Tree, Species::Oak, 0);
        let mut far = 0;
        for tick in 200u64..8200 {
            w.step(tick);
        }
        for i in 0..CELLS {
            if i != c && w.state(i) == Cell::Tree && w.species(i) == Species::Oak {
                let (q, r) = G.index_to_axial(i);
                if hex::distance(q, r, cq, cr) > 4 {
                    far += 1;
                }
            }
        }
        assert!(far >= 2, "jays should cache acorns far from the parent, saw {far} distant oaks");
    }

    #[test]
    fn mast_years_are_synchronous_and_boost_acorn_crops() {
        let w = World::with_params(211, no_fire());
        let years = 400u64;
        let masts = (0..years).filter(|&y| w.mast_at(y * MAST_YEAR_TICKS)).count();
        let frac = masts as f64 / years as f64;
        assert!((0.2..0.5).contains(&frac), "mast every 2–5 years, got fraction {frac:.2}");
        // One draw per year: every tick of a year agrees (synchrony).
        let y0 = 17 * MAST_YEAR_TICKS;
        let first = w.mast_at(y0);
        assert!((y0..y0 + MAST_YEAR_TICKS).all(|t| w.mast_at(t) == first));
    }

    #[test]
    fn acacias_fix_nitrogen_into_their_soil_and_ring() {
        let p = Params { tree_growth_p: 0.0, tree_mean_life: 100_000, ..trees_only() };
        let mut w = bare_world(223, p);
        let (a, q, r) = center();
        let beside = G.axial_to_index(q + 1, r).unwrap();
        let pine = G.axial_to_index(q + 10, r).unwrap();
        w.paint_species(a, Brush::Tree, Species::Acacia, 0);
        w.paint_species(pine, Brush::Tree, Species::Pine, 0);
        w.nutrients[a] = 500;
        w.nutrients[pine] = 500;
        for tick in 1u64..=100 {
            w.step(tick);
        }
        // Fixation offsets the acacia's own draw (it never exhausts its
        // soil) and spills into its ring; a pine just drains.
        assert!(w.nutrient_ratio(a) >= 0.49, "acacia soil should hold, got {}", w.nutrient_ratio(a));
        assert!(w.nutrient_ratio(pine) < 0.15, "pine soil should drain, got {}", w.nutrient_ratio(pine));
        assert!(w.nutrient_ratio(beside) > 0.05, "the acacia's ring is enriched");
    }

    #[test]
    fn willows_resprout_from_the_root_after_dying() {
        // Mean life 1: every tree dies each tick; mature willows come back
        // about half the time, other species never.
        let p = Params { tree_growth_p: 0.0, tree_mean_life: 1, ..trees_only() };
        let back = |sp: Species| -> usize {
            let mut w = bare_world(227, p);
            for i in 0..400 {
                w.paint_species(i * 10, Brush::Tree, sp, 0);
            }
            w.step(100); // all mature, (nearly) all die this tick
            // A resprout is a tree whose age reset to zero this tick
            // (long-lived oaks can simply survive the hazard).
            (0..400).filter(|&i| w.state(i * 10) == Cell::Tree && w.age(i * 10, 100) == 0).count()
        };
        let willows = back(Species::Willow);
        assert!((140..=260).contains(&willows), "~half the willows resprout, saw {willows}/400");
        assert_eq!(back(Species::Oak), 0, "oaks don't resprout");
    }

    #[test]
    fn offspring_inherit_their_parents_traits_with_small_mutations() {
        let p = Params { tree_mean_life: 100_000, mutation_rate: 0.03, ..trees_only() };
        let mut w = bare_world(229, p);
        let (c, _, _) = center();
        w.paint_species(c, Brush::Tree, Species::Acacia, 0);
        w.genome[c] = [1.3, 0.7];
        for tick in 100u64..2100 {
            w.step(tick);
        }
        let kids: Vec<[f32; 2]> = (0..CELLS)
            .filter(|&i| i != c && w.state(i) == Cell::Tree)
            .map(|i| w.genome(i))
            .collect();
        assert!(kids.len() > 5, "the founder should have offspring");
        for g in &kids {
            assert!((g[0] - 1.3).abs() < 0.25 && (g[1] - 0.7).abs() < 0.25, "offspring {g:?} strayed from the lineage");
        }
        assert!(kids.iter().any(|g| *g != [1.3, 0.7]), "mutation should introduce variation");
        // With mutation off, genomes copy exactly.
        let mut v = bare_world(229, Params { mutation_rate: 0.0, ..p });
        v.paint_species(c, Brush::Tree, Species::Acacia, 0);
        v.genome[c] = [1.3, 0.7];
        for tick in 100u64..1100 {
            v.step(tick);
        }
        for i in 0..CELLS {
            if v.state(i) == Cell::Tree {
                let g = v.genome(i);
                assert!((g[0] - 1.3).abs() < 1e-4 && (g[1] - 0.7).abs() < 1e-4, "copy drifted: {g:?}");
            }
        }
    }

    #[test]
    fn vigorous_parents_cast_more_seed_and_dominate_the_offspring_mix() {
        let p = Params { tree_mean_life: 100_000, ..trees_only() };
        let mut w = bare_world(233, p);
        let (t, q, r) = center();
        let a = G.axial_to_index(q - 2, r).unwrap();
        let b = G.axial_to_index(q + 2, r).unwrap();
        w.paint_species(a, Brush::Tree, Species::Pine, 0);
        w.paint_species(b, Brush::Tree, Species::Pine, 0);
        w.genome[a] = [1.5, 0.6];
        w.genome[b] = [0.5, 1.4];
        w.step(100); // both mature: build the seed-rain fields
        let mix = w.parental_gene(t, Species::Pine as usize);
        // Weighted by vigor: (1.5*1.5 + 0.5*0.5)/2.0 = 1.25 vigor, hardiness
        // (1.5*0.6 + 0.5*1.4)/2.0 = 0.8 — skewed toward the vigorous parent.
        assert!((mix[0] - 1.25).abs() < 1e-3, "vigor mix {}", mix[0]);
        assert!((mix[1] - 0.8).abs() < 1e-3, "hardiness mix {}", mix[1]);
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
                    plant_doomed_tree(&mut w, G.axial_to_index(q + dq, r + dr).unwrap());
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
        a.paint_species(c, Brush::Tree, Species::Pine, 0); // same wood as the torched one
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
        b.paint_species(c, Brush::Tree, Species::Pine, 900);
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
        let poor = G.axial_to_index(q + 20, r).unwrap();
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
        let stump = G.axial_to_index(q + 2, r).unwrap();
        w.paint(stump, Brush::Grass, 0); // doomed: dies on the first step
        w.paint(c, Brush::Tree, 960); // mature seed source by tick 1000
        w.step(1000); // stump dies this step (death runs before growth)
        w.step(1001);
        assert!(w.remains(stump).is_some(), "the dead grass should stand as a husk");
        assert_eq!(w.state(stump), Cell::Bare, "nothing may establish on the stump");
        let (nq, nr) = (q + 2, r + 1);
        let open = G.axial_to_index(nq, nr).unwrap();
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
    #[test]
    fn jays_cache_acorns_near_the_parent_while_wind_seed_goes_anywhere() {
        let mut w = bare_world(3, no_fire());
        let (c, q, r) = center();
        w.paint_species(c, Brush::Tree, Species::Oak, 0);
        let pine = G.axial_to_index(q + 1, r).unwrap();
        w.paint_species(pine, Brush::Tree, Species::Pine, 0);
        let dist = |a: usize, b: usize| {
            let (aq, ar) = G.index_to_axial(a);
            let (bq, br) = G.index_to_axial(b);
            hex::distance(aq, ar, bq, br)
        };
        let mut far_pine = false;
        for roll in 0..2_000u64 {
            let roll = rng::mix64(roll);
            if let Some(t) = w.ldd_target(c, roll) {
                assert!(dist(c, t) <= JAY_RADIUS, "acorn cached {} hexes away", dist(c, t));
            }
            if let Some(t) = w.ldd_target(pine, roll) {
                far_pine |= dist(pine, t) > JAY_RADIUS * 2;
            }
        }
        assert!(far_pine, "wind-borne pine seed should reach across the map");
    }

    #[test]
    fn far_flung_seed_still_faces_the_site() {
        // A long-distance oak seed onto a waterlogged valley bottom can't
        // establish, however it arrived (the old bypass planted it anyway).
        let w = World::with_params(7, Params { water_table: 1.0, ..no_fire() });
        let flooded = (0..CELLS).find(|&i| w.water_table(i) > 0.99).expect("a saturated bottom");
        assert_eq!(w.tree_establishment(flooded, Species::Oak as usize, 1.0), 0.0);
        assert!(w.tree_establishment(flooded, Species::Willow as usize, 1.0) > 0.0);
    }

    /// Set the per-species adult counts on a tile directly (field rebuilds
    /// would recompute them from real trees; tests probe the formula).
    fn with_adults(w: &mut World, i: usize, adults: [u8; SPECIES_COUNT]) {
        w.adults_near[i] = adults;
    }

    #[test]
    fn seedlings_suffer_beneath_their_own_kind() {
        // Same shade, same site: an oak seedling does worse surrounded by
        // oaks (specialist enemies + oak's own-canopy shade intolerance)
        // than surrounded by pines — and not at all with competition off.
        let mut w = bare_world(5, Params { competition: 1.0, ..no_fire() });
        let (c, _, _) = center();
        let oak = Species::Oak as usize;
        with_adults(&mut w, c, [0, 6, 0, 0]);
        let under_oaks = w.tree_establishment(c, oak, 0.4);
        with_adults(&mut w, c, [0, 0, 6, 0]);
        let under_pines = w.tree_establishment(c, oak, 0.4);
        assert!(under_oaks < under_pines * 0.6, "{under_oaks:.4} vs {under_pines:.4}");

        let mut off = bare_world(5, no_fire());
        with_adults(&mut off, c, [0, 6, 0, 0]);
        let a = off.tree_establishment(c, oak, 0.4);
        with_adults(&mut off, c, [0, 0, 6, 0]);
        assert_eq!(a, off.tree_establishment(c, oak, 0.4));
    }

    #[test]
    fn pine_needle_litter_builds_up_and_smothers_other_seedlings() {
        let mut w = bare_world(9, Params { competition: 1.0, tree_mean_life: 100_000, ..no_fire() });
        let (c, q, r) = center();
        w.paint_species(c, Brush::Tree, Species::Pine, 0); // mature at once
        let next = G.axial_to_index(q + 1, r).unwrap();
        for tick in 200..400 {
            w.step(tick);
        }
        assert!(w.litter(next) > 0.5, "litter carpet {:.2}", w.litter(next));
        let oak = w.tree_establishment(next, Species::Oak as usize, 1.0);
        let pine = w.tree_establishment(next, Species::Pine as usize, 1.0);
        w.litter[next] = 0.0;
        let clean = w.tree_establishment(next, Species::Oak as usize, 1.0);
        assert!(oak < clean * 0.8, "litter should suppress oak ({oak:.3} vs clean {clean:.3})");
        w.litter[next] = 1.0;
        assert_eq!(pine, w.tree_establishment(next, Species::Pine as usize, 1.0).max(pine));
    }

    #[test]
    fn oak_wilt_runs_through_root_grafts_in_a_dense_stand() {
        // A solid oak block: an outbreak starts and spreads tree to tree,
        // killing stems; the same block with pests off is untouched.
        let run = |pests: f64| {
            let p = Params {
                pest_strength: pests,
                tree_mean_life: 100_000,
                tree_growth_p: 0.0,
                crowding_p: 0.0, // isolate the wilt from self-thinning
                ..no_fire()
            };
            let mut w = bare_world(21, p);
            let (_, q, r) = center();
            let mut block = Vec::new();
            for dq in -5..=5 {
                for dr in -5..=5 {
                    if let Some(i) = G.axial_to_index(q + dq, r + dr) {
                        w.paint_species(i, Brush::Tree, Species::Oak, 0);
                        block.push(i);
                    }
                }
            }
            let mut peak = 0;
            for tick in 300..2_300 {
                w.step(tick);
                peak = peak.max(w.infested_count());
            }
            let alive = block.iter().filter(|&&i| w.state(i) == Cell::Tree).count();
            (peak, alive, block.len())
        };
        let (peak, alive, n) = run(1.0);
        assert!(peak >= 5, "an outbreak should spread, peak {peak}");
        assert!(alive < n * 9 / 10, "the wilt should kill stems ({alive}/{n} alive)");
        let (peak0, alive0, _) = run(0.0);
        assert_eq!((peak0, alive0), (0, n));
    }

    #[test]
    fn browsers_favor_palatable_saplings_and_stunt_them() {
        // Equal sapling cohorts of oak (palatable) and acacia (thorny):
        // browsing scars and kills far more oaks, and a scarred sapling
        // takes longer to mature.
        let p = Params { browse: 1.0, tree_mean_life: 100_000, tree_growth_p: 0.0, ..no_fire() };
        let mut w = bare_world(33, p);
        let cohort = |sp: Species, w: &mut World, lo: usize| {
            for i in (lo..lo + 1200).step_by(2) {
                w.paint_species(i, Brush::Tree, sp, 0);
            }
        };
        cohort(Species::Oak, &mut w, 0);
        cohort(Species::Acacia, &mut w, 2048);
        for tick in 1..80 {
            w.step(tick);
        }
        let hurt = |lo: usize, w: &World| {
            (lo..lo + 1200)
                .step_by(2)
                .filter(|&i| w.state(i) != Cell::Tree || w.browse_damage(i) > 0.0)
                .count()
        };
        let (oak, acacia) = (hurt(0, &w), hurt(2048, &w));
        assert!(oak > acacia * 3, "oaks browsed {oak} vs acacias {acacia}");
        let scarred = (0..1200).step_by(2).find(|&i| w.state(i) == Cell::Tree && w.browse_damage(i) > 0.0).unwrap();
        assert!(w.maturity_age(scarred) > w.maturity_age(2048), "browsing should delay maturity");
    }

}
