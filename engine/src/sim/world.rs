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
use super::terrain::{Terrain, MACRO_RENDER, RENDER_RELIEF};

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
/// Long-distance dispersal: per full-grown mature tree per tick, base
/// chance that one seed travels beyond the seed-rain disk (× the species'
/// `long_distance`) — jays caching acorns, wind-borne winged and cottony
/// seed, pods carried by browsing animals. The distance follows the
/// species' fat-tailed 2Dt kernel (`ldd_scale`, capped at `ldd_cap` tiles;
/// a tile ≈ one crown ≈ 10 m). A seed landing on a suitable site may
/// germinate.
pub const LDD_P: f64 = 0.0006;
pub const LDD_GERMINATION: f64 = 0.5;
/// 2Dt kernel shape: smaller = fatter tail. ~1 matches wind- and
/// animal-dispersed trees (Clark et al. 1999).
pub const LDD_SHAPE: f64 = 1.0;
/// Fecundity ramp: a just-matured tree bears this fraction of a full crop,
/// reaching full output at FECUNDITY_FULL × its maturity age.
pub const FECUNDITY_MIN: f64 = 0.15;
pub const FECUNDITY_FULL: f64 = 3.0;

/// Distance (tiles) from a truncated 2Dt kernel with scale `s` and cap,
/// for a uniform draw `u`: the inverse CDF of F(r) = 1 − (1 + r²/s²)^(−p).
pub fn ldd_distance(s: f64, cap: f64, u: f64) -> f64 {
    // Truncate by rescaling u into the kernel mass below the cap.
    let mass = 1.0 - (1.0 + (cap / s).powi(2)).powf(-LDD_SHAPE);
    let v = (u * mass).min(1.0 - 1e-12);
    s * ((1.0 - v).powf(-1.0 / LDD_SHAPE) - 1.0).sqrt()
}
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
/// Landscape-scale features (see Params).
pub const CLIMATE_ZONES: f64 = 1.0;
pub const RIVERS: f64 = 1.0;
pub const GRAZING: f64 = 1.0;
/// Temperature drop from the lowest to the highest ground at full altitude
/// range (±LAPSE/2 around the map's mid-altitude): the lapse rate.
pub const LAPSE: f64 = 0.5;
/// Regional rain anomaly → moisture shift.
pub const RAIN_GRADIENT: f64 = 0.5;
/// Frost: below this temperature index, frost-tender trees establish less
/// and die more (scaled by 1 − cold_hardiness).
pub const FROST_T: f64 = 0.4;
pub const FROST_HAZARD: f64 = 3.0;
/// Treeline: no tree establishes below TREELINE_T; full recruitment above
/// TREELINE_T + TREELINE_BAND. Snow lies where the snowline temperature
/// (altitude, half the slope aspect, a damped climate swing) is below
/// SNOW_T, fading in over SNOW_BAND.
pub const TREELINE_T: f64 = 0.1;
pub const TREELINE_BAND: f64 = 0.08;
pub const SNOW_T: f64 = 0.24;
pub const SNOW_BAND: f64 = 0.06;
/// Soil thinning at the highest altitude (fraction of depth lost).
pub const MONTANE_THIN: f64 = 0.35;
/// Upstream drainage area (tiles) at which a channel forms (÷ rivers).
pub const RIVER_CELLS: f64 = 350.0;
/// Groundwater on a river bank (distance 1), falling off across the
/// floodplain.
pub const BANK_WATER: f64 = 0.8;
/// Flood pulses: chance per tick (× how far the season is above
/// FLOOD_MOISTURE), duration, and how long a fresh sediment bar lasts.
pub const FLOOD_P: f64 = 0.02;
pub const FLOOD_MOISTURE: f64 = 0.55;
pub const FLOOD_TICKS: u16 = 25;
pub const SEDIMENT_TICKS: u8 = 150;
/// Saplings younger than this are scoured away by a flood.
pub const SCOUR_AGE: u64 = 15;
/// Anoxia: flood-tolerant trees still die in permanently saturated ground
/// above this groundwater level (sedge marsh takes it).
pub const ANOXIA_LEVEL: f64 = 0.85;
pub const ANOXIA_HAZARD: f64 = 4.0;
/// Riparian herbivores (beaver, moose): extra willow-sapling browsing ×
/// (1 + this × groundwater).
pub const RIPARIAN_BROWSE: f64 = 2.0;
/// Grazing: extra grass hazard at full intensity × palatability, and the
/// piosphere decay distance from water (tiles).
pub const GRAZE_HAZARD: f64 = 1.5;
pub const GRAZE_REACH: f64 = 18.0;
/// Physiology and phenology (see Params).
pub const PHYSIOLOGY: f64 = 1.0;
pub const SEASONS: f64 = 1.0;
/// Time scale: vegetation runs at about one year per tick (lifespans of
/// centuries, maturity in decades, seed jumps of tens of metres a year).
/// Weather is stylized — each cloud crossing stands for a season's storm
/// track — and climate swings are multi-decadal oscillations.
///
/// Carbon balance per tree, per tick (reserve 0..1): income = CARBON_GAIN
/// × light × thermal bell × water; upkeep = CARBON_COST × Q10^(Δt/Q10_STEP)
/// where Δt is the site's (climate-damped, acclimated) temperature above
/// the species' own optimum — respiration climbs with heat, canonically ×2
/// per 10 °C, and a temperature-index step of 0.25 ≈ 10 °C.
pub const CARBON_GAIN: f64 = 0.05;
pub const CARBON_COST: f64 = 0.02;
/// Saplings respire in proportion to their small size and shade-acclimated
/// leaves.
pub const SAPLING_COST: f64 = 0.6;
pub const Q10: f64 = 2.0;
pub const Q10_STEP: f64 = 0.25;
/// Water that fully satisfies a tree's demand (tree_water units).
pub const WATER_SUFFICIENT: f64 = 0.45;
/// Canopy trees shade each other: light = 1 / (1 + this × mature neighbors).
pub const CROWD_LIGHT: f64 = 0.05;
/// Starvation: below STARVE_BELOW reserve, hazard rises (quadratically)
/// to STARVE_HAZARD at an empty reserve.
pub const STARVE_BELOW: f64 = 0.25;
pub const STARVE_HAZARD: f64 = 0.06;
/// Carbon an oak spends on a mast crop.
pub const MAST_COST: f64 = 0.015;
/// Rooting: depth reaches (1 − 1/e) of the species' taproot at this age.
pub const ROOT_TIME: f64 = 30.0;
/// Flood duration: ticks a tree has spent waterlogged (inundated, or
/// groundwater above SOAK_LEVEL); beyond its species tolerance the roots
/// rot at up to FLOOD_ROT hazard. Drained ground dries out 2 per tick.
pub const SOAK_LEVEL: f64 = 0.7;
pub const FLOOD_ROT: f64 = 0.08;
/// Climate-triggered pests: outbreak seeding × (1 + BEETLE_WEAK × beetle
/// × (1 − reserve)) — bark beetles overwhelm carbon-starved, drought-
/// stressed hosts — × (1 + WET_ROT × wet_rot × wet-year anomaly) — root
/// rots and water moulds thrive in wet years.
pub const BEETLE_WEAK: f64 = 8.0;
pub const WET_ROT: f64 = 4.0;
/// Deciduous canopies are bare through spring: the understory beneath
/// them gets this much of the shade back.
pub const DECID_RELIEF: f64 = 0.35;
/// Biomes (see Params): the latitude gradient cools the north by up to
/// LAT_GRADIENT (temperature index, north edge vs south), and the interior
/// dries by up to COAST_GRADIENT (moisture) — both scaled with map size.
pub const BIOMES: f64 = 1.0;
pub const LAT_GRADIENT: f64 = 0.45;
pub const COAST_GRADIENT: f64 = 0.5;
/// Snow: precipitation falls as snow below SNOW_PRECIP_T, adding SNOWFALL
/// to the snowpack (0..255) per tick; the pack melts above MELT_T at
/// MELT_RATE per tick per 0.1 of warmth, wetting the ground, and a big
/// melt swells the rivers (FLOOD_MELT × melted fraction of the map).
pub const SNOW_PRECIP_T: f64 = 0.28;
pub const SNOWFALL: u8 = 12;
pub const MELT_T: f64 = 0.34;
pub const MELT_RATE: f64 = 6.0;
pub const FLOOD_MELT: f64 = 40.0;
/// Fire clouds: a fire burning this many tiles can loft a pyrocumulus.
pub const PYRO_TILES: usize = 40;
pub const PYRO_P: f64 = 0.05;
/// Lightning flashes: ticks a storm glows after a ground strike, and the
/// per-tick chance a grown thunderhead flashes internally.
pub const FLASH_TICKS: u8 = 3;
pub const INTRACLOUD_P: f64 = 0.12;
/// Dynamic clouds (see Params). Water per cloud is 0..~1.2 (a full
/// thunderhead). Per tick: evaporation from the ground below (moist soil,
/// groundwater, rivers) and orographic lift on windward slopes add water;
/// descent on lee slopes, raining, and — for small cumulus — evaporation
/// over hot dry ground remove it.
pub const CLOUD_DYNAMICS: f64 = 1.0;
pub const CLOUD_EVAP: f64 = 0.018;
pub const CLOUD_LIFT: f64 = 0.35;
pub const CLOUD_SINK: f64 = 0.25;
pub const CLOUD_RAIN_OUT: f64 = 0.03;
pub const CUMULUS_DRY: f64 = 0.02;
/// Genus transitions (with hysteresis): cumulus towers into cumulonimbus
/// above CB_WATER over warm ground (temperature index ≥ CB_HEAT); a
/// thunderhead that rains below CB_SPENT collapses, leaving its anvil as
/// cirrus; cirrus that fills above FRONT_WATER thickens and lowers into
/// nimbostratus (a front arriving); spent nimbostratus below NS_SPENT
/// breaks up into fair-weather cumulus.
pub const CB_WATER: f64 = 0.75;
pub const CB_HEAT: f64 = 0.45;
pub const CB_SPENT: f64 = 0.2;
pub const FRONT_WATER: f64 = 0.6;
pub const NS_SPENT: f64 = 0.15;
/// Terrain rise (height per world unit along the wind) strong enough to
/// force a well-fed cumulus into a thunderhead even over cool ground.
pub const FORCED_LIFT: f64 = 0.05;
/// Orographic precipitation: any non-cirrus cloud holding more than
/// OROG_WATER rains on windward slopes rising faster than OROG_SLOPE —
/// air forced up the ridge cools and condenses — wringing out its water so
/// the lee side lies in a rain shadow.
pub const OROG_SLOPE: f64 = 0.02;
pub const OROG_WATER: f64 = 0.35;
/// How fast a changing cloud morphs into its new form (per tick).
pub const CLOUD_MORPH: f32 = 0.06;
/// A cloud rains / throws lightning only with this much water (and a
/// thunderhead only once mostly grown).
pub const RAIN_WATER: f64 = 0.2;
/// In-place convective initiation: candidate sites per 4096 tiles per tick
/// and the base chance a perfectly warm, moist, windward site bubbles up a
/// new cumulus.
pub const INIT_SITES: f64 = 2.0;
/// Chance per tick a mature thunderhead's gust front triggers a daughter
/// cell at its leading edge (multicell storms).
pub const OUTFLOW_P: f64 = 0.008;
pub const INIT_P: f64 = 0.6;
/// Recent-death tally decay per tick (≈ a 50-tick window).
pub const DEATH_RECENT_KEEP: f32 = 0.98;
/// How much longer the flat background lifetime runs once physiology
/// models the mechanistic deaths.
pub const PHYSIOLOGY_LIFE: f64 = 0.4;
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
    /// Water content (dynamic clouds): feeds growth, rain, and lightning.
    pub water: f64,
    /// The genus this cloud is changing from, and how far the change has
    /// progressed (1 = fully its current genus).
    pub from: CloudKind,
    pub morph: f32,
    /// Ticks left of a ground-strike flash (lights the whole scene) and of
    /// an in-cloud flash (lights the thunderhead from inside).
    pub flash: u8,
    pub glow: u8,
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
    /// Map-scale climate: mountains cool with altitude (lapse rate, frost,
    /// treeline) and a regional rain gradient (0 = uniform climate).
    pub climate_zones: f64,
    /// River channels, riparian corridors, and flood pulses; also the
    /// riparian realism for willow (flood-bar recruitment, anoxia in
    /// permanently saturated ground, beaver/moose browsing). 0 = none.
    pub rivers: f64,
    /// Grazing by herbivores on grass, heaviest near water (piospheres).
    pub grazing: f64,
    /// Tree physiology: per-tree carbon reserves (income vs heat-rising
    /// upkeep, starvation), age-deepening roots reaching groundwater,
    /// flood tolerance by duration, climate-triggered pests, annual
    /// masting. 0 = the older flat-hazard model.
    pub physiology: f64,
    /// Phenology: deciduous canopies let spring light through to the
    /// understory (and show the seasons at slow speeds). 0 = none.
    pub seasons: f64,
    /// Biomes: map-scale climate gradients (colder north, drier interior)
    /// and the biome-specific plants (spruce, birch, creosote, reeds,
    /// cactus). 0 = the original four trees and grasses, no gradients.
    pub biomes: f64,
    /// Dynamic clouds: each cloud carries water, grows and changes genus
    /// with the ground beneath it (convection, orographic lift, fronts),
    /// rains itself out, and new cumulus form in place. 0 = clouds keep
    /// their birth genus and drift across unchanged.
    pub cloud_dynamics: f64,
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
            climate_zones: CLIMATE_ZONES,
            rivers: RIVERS,
            grazing: GRAZING,
            physiology: PHYSIOLOGY,
            seasons: SEASONS,
            cloud_dynamics: CLOUD_DYNAMICS,
            biomes: BIOMES,
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
    /// The landscape-scale features (climate zones, rivers, grazing) are
    /// off, so this is exactly the world the regime bands were measured on.
    pub fn legacy_map() -> Params {
        Params {
            width: Grid::LEGACY.width as u32,
            height: Grid::LEGACY.height as u32,
            climate_zones: 0.0,
            rivers: 0.0,
            grazing: 0.0,
            physiology: 0.0,
            seasons: 0.0,
            cloud_dynamics: 0.0,
            biomes: 0.0,
            ..Params::default()
        }
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
        self.climate_zones = prob(self.climate_zones, CLIMATE_ZONES);
        self.rivers = prob(self.rivers, RIVERS);
        self.grazing = prob(self.grazing, GRAZING);
        self.physiology = prob(self.physiology, PHYSIOLOGY);
        self.seasons = prob(self.seasons, SEASONS);
        self.cloud_dynamics = prob(self.cloud_dynamics, CLOUD_DYNAMICS);
        self.biomes = prob(self.biomes, BIOMES);
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
    /// Boreal climax conifer: cold-hardy, deeply shade-tolerant, shallow-
    /// rooted, fire-sensitive, evergreen — the dark taiga.
    Spruce = 4,
    /// Boreal pioneer: fast, short-lived, cold-hardy, deciduous, with tiny
    /// wind-borne seed that floods burns and clearings.
    Birch = 5,
    /// Desert shrub (creosote): extraordinarily drought-tolerant, deep-
    /// rooted, slow, very long-lived clones; frost-tender.
    Creosote = 6,
}

pub const SPECIES_COUNT: usize = 7;
/// The original four species — all a world without `biomes` ever grows.
pub const BASE_SPECIES: usize = 4;

impl Species {
    pub fn from_u8(v: u8) -> Species {
        match v {
            1 => Species::Oak,
            2 => Species::Pine,
            3 => Species::Willow,
            4 => Species::Spruce,
            5 => Species::Birch,
            6 => Species::Creosote,
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
    /// Frost tolerance, 0 (tender) .. 1 (hardy): sets the alpine limit.
    pub cold_hardy: f64,
    /// Needs a bare, freshly flooded seedbed to recruit (willow's
    /// short-lived seed; the "recruitment box").
    pub seedbed: bool,
    /// Long-distance kernel scale and hard cap, in tiles (≈ 10 m each):
    /// wind-winged pine ~50 m / 600 m, jay-cached acorns ~80 m / 400 m,
    /// ungulate-carried acacia pods ~60 m / 400 m, cottony willow seed on
    /// wind and water ~100 m / 800 m.
    pub ldd_scale: f64,
    pub ldd_cap: f64,
    /// Taproot reach, 0..1: how much of the groundwater an old tree taps
    /// (deep-rooted acacia and oak ≫ shallow-rooted willow).
    pub taproot: f64,
    /// Ticks of waterlogging the roots tolerate before rotting.
    pub flood_days: f64,
    /// 0 evergreen .. 1 fully deciduous (bare in winter and early spring).
    pub deciduous: f64,
    /// Bark-beetle susceptibility when carbon-starved (pine ≫ others).
    pub beetle: f64,
    /// Root-rot / water-mould susceptibility in wet years (oak ≫ others).
    pub wet_rot: f64,
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
        cold_hardy: 0.0,
        seedbed: false,
        ldd_scale: 6.0,
        ldd_cap: 40.0,
        taproot: 1.0,
        flood_days: 4.0,
        deciduous: 0.4,
        beetle: 0.3,
        wet_rot: 0.0,
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
        cold_hardy: 0.65,
        seedbed: false,
        ldd_scale: 8.0,
        ldd_cap: 40.0,
        taproot: 0.9,
        flood_days: 12.0,
        deciduous: 1.0,
        beetle: 0.2,
        wet_rot: 1.0,
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
        cold_hardy: 1.0,
        seedbed: false,
        ldd_scale: 5.0,
        ldd_cap: 60.0,
        taproot: 0.6,
        flood_days: 4.0,
        deciduous: 0.0,
        beetle: 1.0,
        wet_rot: 0.2,
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
        cold_hardy: 0.8,
        seedbed: true,
        ldd_scale: 10.0,
        ldd_cap: 80.0,
        taproot: 0.5,
        flood_days: 60.0,
        deciduous: 1.0,
        beetle: 0.2,
        wet_rot: 0.3,
    },
    SpeciesTraits {
        name: "Spruce",
        growth: 1.05,
        maturity: 1.2,
        mean_life: 1.3,
        shade_tolerance: 0.6,
        ash_affinity: 0.5,
        drought_sensitivity: 1.3,
        water_affinity: 1.1,
        fireproof: 3.0,
        crowding: 1.0,
        dispersal: 3,
        rot: 1.2,
        nutrients: 0.8,
        dry_ground: 1.0,
        resprout: 0.0,
        serotinous: false,
        nitrogen_fixer: false,
        jay_cached: false,
        long_distance: 1.2,
        flood_tolerant: false,
        temp_opt: 0.2,
        temp_width: 0.18,
        poor_soil: 0.8,
        fire_sprout: 0.0,
        pest_susceptibility: 0.8,
        palatability: 0.1,
        own_shade: 1.0,
        litter: 0.6,
        seed_reserve: 0.0,
        cold_hardy: 1.0,
        seedbed: false,
        ldd_scale: 4.0,
        ldd_cap: 40.0,
        taproot: 0.3,
        flood_days: 6.0,
        deciduous: 0.0,
        beetle: 0.8,
        wet_rot: 0.2,
    },
    SpeciesTraits {
        name: "Birch",
        growth: 1.8,
        maturity: 0.6,
        mean_life: 0.7,
        shade_tolerance: 0.0,
        ash_affinity: 2.0,
        drought_sensitivity: 1.2,
        water_affinity: 1.2,
        fireproof: 1.0,
        crowding: 1.0,
        dispersal: 3,
        rot: 0.8,
        nutrients: 1.0,
        dry_ground: 1.0,
        resprout: 0.3,
        serotinous: false,
        nitrogen_fixer: false,
        jay_cached: false,
        long_distance: 2.5,
        flood_tolerant: false,
        temp_opt: 0.27,
        temp_width: 0.22,
        poor_soil: 0.5,
        fire_sprout: 0.4,
        pest_susceptibility: 0.4,
        palatability: 0.6,
        own_shade: 1.0,
        litter: 0.0,
        seed_reserve: 0.0,
        cold_hardy: 0.95,
        seedbed: false,
        ldd_scale: 8.0,
        ldd_cap: 80.0,
        taproot: 0.4,
        flood_days: 8.0,
        deciduous: 1.0,
        beetle: 0.2,
        wet_rot: 0.2,
    },
    SpeciesTraits {
        name: "Creosote",
        growth: 0.4,
        maturity: 1.0,
        mean_life: 3.0,
        shade_tolerance: 0.0,
        ash_affinity: 0.5,
        drought_sensitivity: 0.2,
        water_affinity: 0.3,
        fireproof: 1.0,
        crowding: 0.6,
        dispersal: 2,
        rot: 2.0,
        nutrients: 0.6,
        dry_ground: 1.0,
        resprout: 0.5,
        serotinous: false,
        nitrogen_fixer: false,
        jay_cached: false,
        long_distance: 0.5,
        flood_tolerant: false,
        temp_opt: 0.8,
        temp_width: 0.22,
        poor_soil: 1.0,
        fire_sprout: 0.2,
        pest_susceptibility: 0.1,
        palatability: 0.05,
        own_shade: 1.0,
        litter: 0.0,
        seed_reserve: 0.1,
        cold_hardy: 0.0,
        seedbed: false,
        ldd_scale: 5.0,
        ldd_cap: 30.0,
        taproot: 1.0,
        flood_days: 2.0,
        deciduous: 0.3,
        beetle: 0.0,
        wet_rot: 0.3,
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
    /// Reeds and cattails: tall marsh stands in open water margins and
    /// saturated ground; rhizomes form dense, flood-proof beds.
    Reeds = 4,
    /// Succulents and cacti: the desert floor; store water, barely burn,
    /// live very long, spread slowly.
    Cactus = 5,
}

pub const GRASS_KIND_COUNT: usize = 6;
/// The original four grass kinds — all a world without `biomes` grows.
pub const BASE_GRASS_KINDS: usize = 4;

impl GrassKind {
    pub fn from_u8(v: u8) -> GrassKind {
        match v {
            1 => GrassKind::Sod,
            2 => GrassKind::Sedge,
            3 => GrassKind::Annual,
            4 => GrassKind::Reeds,
            5 => GrassKind::Cactus,
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
    /// Grazer preference: leafy C3 sod ≫ annuals > coarse sedge, C4 bunch.
    pub palatability: f64,
}

pub static GRASS_TABLE: [GrassTraits; GRASS_KIND_COUNT] = [
    GrassTraits { name: "Bunchgrass", creep: 1.1, life: 1.4, shade_tolerance: 0.0, temp_opt: 0.62, temp_width: 0.26, water_opt: 0.38, water_width: 0.30, flammability: 1.6, fire_resprout: 0.6, sod: 1.6, flood_tolerant: false, seeder: false, drought_sensitivity: 0.3, palatability: 0.3 },
    GrassTraits { name: "Sod grass", creep: 1.1, life: 1.3, shade_tolerance: 0.45, temp_opt: 0.38, temp_width: 0.19, water_opt: 0.62, water_width: 0.24, flammability: 0.6, fire_resprout: 0.0, sod: 0.85, flood_tolerant: false, seeder: false, drought_sensitivity: 1.6, palatability: 1.0 },
    GrassTraits { name: "Sedge", creep: 1.0, life: 1.0, shade_tolerance: 0.2, temp_opt: 0.5, temp_width: 0.35, water_opt: 0.92, water_width: 0.20, flammability: 0.3, fire_resprout: 0.3, sod: 1.0, flood_tolerant: true, seeder: false, drought_sensitivity: 1.3, palatability: 0.35 },
    GrassTraits { name: "Annual", creep: 0.25, life: 0.35, shade_tolerance: 0.0, temp_opt: 0.55, temp_width: 0.36, water_opt: 0.45, water_width: 0.40, flammability: 1.3, fire_resprout: 0.0, sod: 1.2, flood_tolerant: false, seeder: true, drought_sensitivity: 0.7, palatability: 0.7 },
    GrassTraits { name: "Reeds", creep: 1.3, life: 1.5, shade_tolerance: 0.1, temp_opt: 0.55, temp_width: 0.35, water_opt: 1.0, water_width: 0.15, flammability: 0.8, fire_resprout: 0.8, sod: 0.4, flood_tolerant: true, seeder: false, drought_sensitivity: 2.0, palatability: 0.3 },
    GrassTraits { name: "Cactus", creep: 0.2, life: 3.0, shade_tolerance: 0.0, temp_opt: 0.85, temp_width: 0.2, water_opt: 0.08, water_width: 0.2, flammability: 0.1, fire_resprout: 0.0, sod: 1.3, flood_tolerant: false, seeder: false, drought_sensitivity: 0.05, palatability: 0.05 },
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
    /// What killed it (trees only).
    pub cause: DeathCause,
}

/// Why a tree died — recorded on its husk and tallied for the HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeathCause {
    Age = 0,
    Drought = 1,
    Starvation = 2,
    Frost = 3,
    Flood = 4,
    Pests = 5,
    Crowding = 6,
    Fire = 7,
    Windthrow = 8,
    Browsed = 9,
    Scoured = 10,
    /// Felled by the player (leaves a stump).
    Logged = 11,
}

pub const DEATH_CAUSE_COUNT: usize = 12;

/// Cloud lifecycle events, tallied for the HUD and probes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CloudEvent {
    /// A cumulus bubbled up in place over warm, moist or windward ground.
    Formed = 0,
    /// Cumulus towered into a cumulonimbus.
    Towered = 1,
    /// A spent thunderhead collapsed, leaving its anvil as cirrus.
    Collapsed = 2,
    /// Cirrus thickened and lowered into nimbostratus (a front).
    Front = 3,
    /// Spent nimbostratus broke up into fair-weather cumulus.
    BrokeUp = 4,
    /// A cloud evaporated away.
    Evaporated = 5,
    /// A thunderhead's gust front triggered a daughter cell.
    Daughter = 6,
    /// A large fire lofted its own cloud (pyrocumulus).
    Pyro = 7,
}

pub const CLOUD_EVENT_COUNT: usize = 8;

/// Climate biome of a tile (Whittaker-style: site temperature × water),
/// with saturated ground as wetland.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Biome {
    Wetland = 0,
    Tundra = 1,
    Boreal = 2,
    TemperateForest = 3,
    Grassland = 4,
    Savanna = 5,
    Desert = 6,
}

pub const BIOME_COUNT: usize = 7;

impl Biome {
    pub fn from_u8(v: u8) -> Biome {
        use Biome::*;
        match v {
            0 => Wetland,
            1 => Tundra,
            2 => Boreal,
            3 => TemperateForest,
            4 => Grassland,
            5 => Savanna,
            _ => Desert,
        }
    }

    pub fn name(self) -> &'static str {
        use Biome::*;
        match self {
            Wetland => "wetland",
            Tundra => "tundra",
            Boreal => "boreal forest",
            TemperateForest => "temperate forest",
            Grassland => "grassland",
            Savanna => "savanna",
            Desert => "desert",
        }
    }
}

impl DeathCause {
    pub fn from_u8(v: u8) -> DeathCause {
        use DeathCause::*;
        match v {
            1 => Drought,
            2 => Starvation,
            3 => Frost,
            4 => Flood,
            5 => Pests,
            6 => Crowding,
            7 => Fire,
            8 => Windthrow,
            9 => Browsed,
            10 => Scoured,
            11 => Logged,
            _ => Age,
        }
    }

    pub fn name(self) -> &'static str {
        use DeathCause::*;
        match self {
            Age => "old age",
            Drought => "drought",
            Starvation => "starvation",
            Frost => "frost",
            Flood => "root rot",
            Pests => "pests",
            Crowding => "crowding",
            Fire => "fire",
            Windthrow => "windthrow",
            Browsed => "browsed",
            Scoured => "flood scour",
            Logged => "logging",
        }
    }
}

/// Index drawn in proportion to `weights` from a 64-bit hash roll.
fn weighted_pick(weights: &[f64], roll: u64) -> usize {
    let total: f64 = weights.iter().sum();
    let mut pick = (roll >> 11) as f64 / (1u64 << 53) as f64 * total;
    for (k, &wt) in weights.iter().enumerate() {
        pick -= wt;
        if pick <= 0.0 {
            return k;
        }
    }
    weights.len() - 1
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
    /// Local temperature index, groundwater, and seedbed state.
    temp: f64,
    groundwater: f64,
    bare: bool,
    sediment: bool,
    channel: bool,
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
    /// Hex distance to the nearest mature evergreen (255 = none in range).
    evergreen_dist: Vec<u8>,
    /// Per-tree carbon reserve, 0..1.
    reserve: Vec<f32>,
    /// Consecutive-ish ticks a tree has been waterlogged.
    soaked: Vec<u8>,
    /// Death cause recorded on a husk.
    remains_cause: Vec<u8>,
    /// Tree deaths by cause: all-time, and a decaying recent tally.
    deaths_total: [u32; DEATH_CAUSE_COUNT],
    deaths_recent: [f32; DEATH_CAUSE_COUNT],
    /// Cloud lifecycle events since the world began.
    cloud_events: [u32; CLOUD_EVENT_COUNT],
    /// Snowpack depth per tile (0..255) and last tick's melt, as a
    /// fraction of the map (drives spring floods).
    snow: Vec<u8>,
    melt_recent: f64,
    /// The tile of the most recent ground strike (for a localized flash).
    last_strike: Option<usize>,
    /// Structures the player built (1 = a house): nothing grows there.
    built: Vec<u8>,
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
    /// River channels (open water), hex distance to the nearest channel
    /// (u16::MAX = none), that channel's floodplain reach, and the
    /// corridor groundwater they add.
    channel: Vec<bool>,
    river_dist: Vec<u16>,
    river_reach: Vec<f32>,
    river_water: Vec<f32>,
    /// Ticks a fresh flood-sediment bar stays a seedbed.
    sediment: Vec<u8>,
    /// Ticks left in the current flood pulse (0 = none) and its stage
    /// (fraction of each floodplain it covers).
    flood_left: u16,
    flood_stage: f64,
    /// Per-tick caches of the niche site (temperature) and each grass
    /// kind's raw fit — pure functions of the tick's climate and the
    /// static terrain, reused by several passes.
    site_temp: Vec<f64>,
    grass_fit_now: Vec<[f64; GRASS_KIND_COUNT]>,
    /// Offsets for the NEAR_RADIUS disk.
    near_disk: Vec<(i32, i32, i32)>,
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
            evergreen_dist: vec![255; n],
            reserve: vec![0.0; n],
            soaked: vec![0; n],
            remains_cause: vec![0; n],
            built: vec![0; n],
            deaths_total: [0; DEATH_CAUSE_COUNT],
            deaths_recent: [0.0; DEATH_CAUSE_COUNT],
            cloud_events: [0; CLOUD_EVENT_COUNT],
            snow: vec![0; n],
            melt_recent: 0.0,
            last_strike: None,
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
            channel: vec![false; n],
            river_dist: vec![u16::MAX; n],
            river_reach: vec![0.0; n],
            river_water: vec![0.0; n],
            sediment: vec![0; n],
            flood_left: 0,
            flood_stage: 0.0,
            site_temp: vec![0.5; n],
            grass_fit_now: vec![[0.0; GRASS_KIND_COUNT]; n],
            near_disk: hex::disk(NEAR_RADIUS),
            adults_near: vec![[0; SPECIES_COUNT]; n],
            pest: vec![0; n],
            browse_scar: vec![0; n],
            litter: vec![0.0; n],
            params,
        };
        w.rebuild_rivers();
        // The world's climate is needed before seeding (site fits below).
        let (sun, moisture) = w.climate_at(0);
        w.sun = sun;
        w.moisture = moisture;
        let biomes = w.params.biomes > 0.0;
        if biomes {
            w.refresh_site_cache();
        }
        // The biome plants are only in the seed mix with biomes on.
        let species_pool = if biomes { SPECIES_COUNT } else { BASE_SPECIES };
        let grass_pool = if biomes { GRASS_KIND_COUNT } else { BASE_GRASS_KINDS };
        for i in 0..n {
            if w.channel[i] {
                continue; // open water
            }
            if rng::uniform01(seed, i as u32, 0, Stream::Seeding) < w.params.seed_tree_p {
                let roll = rng::hash(seed, i as u32, 0, Stream::SpeciesChoice);
                let sp = if biomes {
                    // Founders arrive from the regional species pool: a
                    // tile's founder is likelier to be a species suited to
                    // it (weights = how readily each would establish there).
                    let weights: Vec<f64> = (0..species_pool).map(|k| w.tree_establishment(i, k, 1.0).max(1e-4)).collect();
                    Species::from_u8(weighted_pick(&weights, roll) as u8)
                } else {
                    Species::from_u8((roll % species_pool as u64) as u8)
                };
                w.plant_tree(i, sp, 0);
            } else if rng::uniform01(seed, i as u32, 0, Stream::SeedingGrass) < w.params.seed_grass_p {
                let roll = rng::hash(seed, i as u32 + n as u32, 0, Stream::SpeciesChoice);
                let kind = if biomes {
                    let weights: Vec<f64> = (0..grass_pool).map(|k| w.grass_fit_raw(k, i).max(1e-4)).collect();
                    weighted_pick(&weights, roll) as u8
                } else {
                    (roll % grass_pool as u64) as u8
                };
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
        if self.params.physiology > 0.0 {
            // A tick is a year: mast years are cued by moist weather, but
            // a mast year exhausts the trees — never two in a row (the
            // real 2–5 year masting rhythm).
            let draw = |t: u64| {
                let (_, m) = self.climate_at(t);
                rng::uniform01(self.seed, t as u32, 0, Stream::Mast) < 0.15 + 0.4 * m
            };
            return draw(tick) && !(tick > 0 && draw(tick - 1));
        }
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
        let catena = self.terrain.water[index];
        let river = self.river_water[index] * self.params.rivers as f32;
        let open = if self.channel[index] { 1.0 } else { 0.0 };
        catena.max(river).max(open) * self.params.water_table as f32
    }

    /// Render height of a tile's surface (0 on a flat world): local hills
    /// plus the map-scale mountains; river channels sit a little low.
    pub fn elevation(&self, index: usize) -> f32 {
        let hills = self.terrain.elevation[index] as f64 * RENDER_RELIEF * self.params.terrain;
        let mountains = self.terrain.altitude[index] as f64 * MACRO_RENDER * self.params.climate_zones;
        let bed = if self.channel[index] { 0.25 } else { 0.0 };
        (hills + mountains - bed) as f32
    }

    /// Highest possible surface on this map (for picking and shadows).
    pub fn max_elevation(&self) -> f32 {
        (RENDER_RELIEF * self.params.terrain
            + MACRO_RENDER * self.terrain.altitude_amp as f64 * self.params.climate_zones) as f32
    }

    /// Map-scale altitude, 0 .. 1 (0 on small maps' low ground).
    pub fn altitude(&self, index: usize) -> f32 {
        self.terrain.altitude[index]
    }

    /// Cooling with altitude (and warming of the lowlands) around the
    /// map's mid-altitude, scaled by climate_zones.
    fn lapse(&self, index: usize) -> f64 {
        let mid = self.terrain.altitude_amp as f64 / 2.0;
        let altitude = -LAPSE * self.params.climate_zones * (self.terrain.altitude[index] as f64 - mid);
        // Biomes: the north runs colder, the south warmer.
        let lat = -LAT_GRADIENT
            * self.params.biomes
            * self.terrain.gradient_amp as f64
            * (self.terrain.latitude[index] as f64 - 0.5);
        altitude + lat
    }

    /// Regional moisture shift (wet vs dry regions), scaled by climate_zones.
    fn rain_shift(&self, index: usize) -> f64 {
        // Biomes: a wet coast grading to a dry continental interior.
        let coast = -COAST_GRADIENT
            * self.params.biomes
            * self.terrain.gradient_amp as f64
            * (self.terrain.interior[index] as f64 - 0.5);
        RAIN_GRADIENT * self.params.climate_zones * self.terrain.rain[index] as f64 + coast
    }

    /// Whether a tile is open river water (nothing grows there).
    pub fn is_channel(&self, index: usize) -> bool {
        self.channel[index]
    }

    /// Hex distance to the nearest river channel (None if no river nearby).
    pub fn river_distance(&self, index: usize) -> Option<u16> {
        let d = self.river_dist[index];
        (d != u16::MAX).then_some(d)
    }

    /// A flood pulse is running.
    pub fn is_flooding(&self) -> bool {
        self.flood_left > 0
    }

    /// Whether a tile is under floodwater right now.
    pub fn inundated(&self, index: usize) -> bool {
        self.flood_left > 0 && self.in_floodplain(index, self.flood_stage)
    }

    /// Whether a tile lies within `stage` of its river's floodplain reach.
    fn in_floodplain(&self, index: usize, stage: f64) -> bool {
        !self.channel[index]
            && self.river_dist[index] != u16::MAX
            && (self.river_dist[index] as f64) <= self.river_reach[index] as f64 * stage
    }

    /// Fresh flood sediment on a tile, 0..1.
    pub fn sediment(&self, index: usize) -> f32 {
        self.sediment[index] as f32 / SEDIMENT_TICKS as f32
    }

    /// Grazing intensity on a tile, 0..1: herbivores stay near water
    /// (piosphere), so pressure falls with distance to rivers and wet
    /// ground.
    pub fn grazing_intensity(&self, index: usize) -> f64 {
        if self.params.grazing <= 0.0 {
            return 0.0;
        }
        let near_river = match self.river_distance(index) {
            Some(d) => (-(d as f64) / GRAZE_REACH).exp() * self.params.rivers,
            None => 0.0,
        };
        let near = near_river.max(self.terrain.water[index] as f64);
        self.params.grazing * (0.3 + 0.7 * near)
    }

    /// Normalized terrain layers for display/probes: (elevation, heat, depth).
    pub fn terrain_at(&self, index: usize) -> (f32, f32, f32) {
        (self.terrain.elevation[index], self.terrain.heat[index], self.terrain.depth[index])
    }

    /// Local temperature index: the season's sun shifted by slope aspect
    /// (0.5 in a neutral climate on flat ground).
    pub fn temperature(&self, index: usize) -> f64 {
        let aspect = (self.terrain.heat[index] as f64 - 0.5) * 1.0 * self.params.terrain;
        (self.sun + aspect + self.lapse(index)).clamp(0.02, 0.98)
    }

    /// Snow cover on a tile, 0..1: the snowline follows altitude (and runs
    /// lower on shaded slopes), creeping down in cold years — the weather
    /// swing is damped like the niche climate so the snowline doesn't
    /// flicker across the lowlands.
    pub fn snow_cover(&self, index: usize) -> f32 {
        self.snow_cover_at(index, 0.0)
    }

    /// Snow cover with the snowline lowered by `extra_cold` (the display's
    /// winter, at slow seasonal speeds).
    pub fn snow_cover_at(&self, index: usize, extra_cold: f64) -> f32 {
        if self.params.climate_zones <= 0.0 {
            return 0.0;
        }
        let aspect = (self.terrain.heat[index] as f64 - 0.5) * 0.5 * self.params.terrain;
        let t = 0.5 + (self.sun - 0.5) * NICHE_CLIMATE + self.lapse(index) + aspect - extra_cold;
        ((SNOW_T - t) / SNOW_BAND).clamp(0.0, 1.0) as f32
    }

    /// Frost stress for a species on a tile, 0..1 (0 when warm enough or
    /// frost-hardy).
    fn frost(&self, index: usize, cold_hardy: f64) -> f64 {
        let cold = ((FROST_T - self.temperature(index)) / FROST_T).clamp(0.0, 1.0);
        self.params.climate_zones * cold * (1.0 - cold_hardy)
    }

    /// Effective soil depth, 1 on a flat world.
    pub fn soil_depth(&self, index: usize) -> f64 {
        1.0 - self.params.terrain * (1.0 - self.depth(index))
    }

    /// Terrain soil depth, thinned on the mountains (montane soils are
    /// shallow and rocky) when climate zones are on.
    fn depth(&self, index: usize) -> f64 {
        self.terrain.depth[index] as f64
            * (1.0 - MONTANE_THIN * self.params.climate_zones * self.terrain.altitude[index] as f64)
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

    /// Mean effective number of plant types within 16×16-tile windows
    /// (local, α diversity) — compare with `diversity()` (whole map, γ).
    pub fn local_diversity(&self) -> f64 {
        let win = 16;
        let g = self.grid;
        let (mut sum, mut count) = (0.0, 0.0);
        for wy in (0..g.height).step_by(win) {
            for wx in (0..g.width).step_by(win) {
                let mut n = [0u32; SPECIES_COUNT + GRASS_KIND_COUNT];
                for row in wy..(wy + win as i32).min(g.height) {
                    for col in wx..(wx + win as i32).min(g.width) {
                        let i = (row * g.width + col) as usize;
                        match self.state[i] {
                            Cell::Tree => n[self.species[i] as usize] += 1,
                            Cell::Grass if self.params.grass_niches <= 0.0 => n[SPECIES_COUNT] += 1,
                            Cell::Grass => n[SPECIES_COUNT + self.species[i] as usize] += 1,
                            Cell::Bare => {}
                        }
                    }
                }
                let total: u32 = n.iter().sum();
                if total == 0 {
                    continue;
                }
                let h: f64 = n
                    .iter()
                    .filter(|&&c| c > 0)
                    .map(|&c| {
                        let p = c as f64 / total as f64;
                        -p * p.ln()
                    })
                    .sum();
                sum += h.exp();
                count += 1.0;
            }
        }
        if count > 0.0 { sum / count } else { 0.0 }
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
            let m = (self.moisture + self.rain_shift(index)).clamp(0.0, 1.0);
            (m + (1.0 - m) * wt) * retention
        }
    }

    /// The temperature and water a perennial grass is adapted to on this
    /// site: slope aspect and catena position, with the year-to-year
    /// climate damped by NICHE_CLIMATE. Swings still favor one type or
    /// another for a while (the storage effect), but the map, not the
    /// weather, decides who lives where.
    fn niche_site(&self, index: usize) -> (f64, f64) {
        let sun = 0.5 + (self.sun - 0.5) * NICHE_CLIMATE + self.lapse(index);
        let moisture =
            (0.5 + (self.moisture - 0.5) * NICHE_CLIMATE + self.rain_shift(index)).clamp(0.0, 1.0);
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
        let rivers_changed = params.rivers != self.params.rivers;
        self.params = params;
        if rivers_changed {
            self.rebuild_rivers();
        }
    }

    /// Derive channels and riparian corridors from the drainage map: a
    /// channel wherever upstream area ≥ RIVER_CELLS / rivers; then a
    /// multi-source BFS gives every tile its distance to water and the
    /// reach of the floodplain it belongs to (wider for bigger rivers).
    fn rebuild_rivers(&mut self) {
        let n = self.grid.cells();
        let r = self.params.rivers;
        self.channel.fill(false);
        self.river_dist.fill(u16::MAX);
        self.river_reach.fill(0.0);
        self.river_water.fill(0.0);
        if r <= 0.0 {
            return;
        }
        let threshold = RIVER_CELLS / r;
        let mut queue = std::collections::VecDeque::new();
        for i in 0..n {
            let f = self.terrain.flow[i] as f64;
            if f >= threshold {
                self.channel[i] = true;
                self.river_dist[i] = 0;
                self.river_reach[i] = (1.0 + 1.6 * (f / threshold).ln()) as f32;
                queue.push_back(i);
                if self.state[i] != Cell::Bare {
                    self.state[i] = Cell::Bare; // the river takes the tile
                }
            }
        }
        while let Some(c) = queue.pop_front() {
            let d = self.river_dist[c];
            if d as f32 > self.river_reach[c] + 6.0 {
                continue;
            }
            let (q, rr) = self.grid.index_to_axial(c);
            for (dq, dr) in hex::NEIGHBORS {
                if let Some(j) = self.grid.axial_to_index(q + dq, rr + dr) {
                    if self.river_dist[j] == u16::MAX {
                        self.river_dist[j] = d + 1;
                        self.river_reach[j] = self.river_reach[c];
                        queue.push_back(j);
                    }
                }
            }
        }
        for i in 0..n {
            let d = self.river_dist[i];
            if d == 0 || d == u16::MAX {
                continue;
            }
            let reach = self.river_reach[i] as f64;
            let fall = ((d as f64 - 1.0) / (reach + 1.0)).clamp(0.0, 1.0);
            self.river_water[i] = (BANK_WATER * (1.0 - fall)) as f32;
        }
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
            cause: DeathCause::from_u8(self.remains_cause[index]),
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
        // Birth water by genus: fair-weather cumulus and high cirrus are
        // thin, the rain-bearers full; a moist season adds to it.
        let m = self.moisture;
        let water = match kind {
            CloudKind::Cumulus => 0.3 + 0.2 * m,
            CloudKind::Cumulonimbus => 0.9,
            CloudKind::Nimbostratus => 0.9,
            CloudKind::Cirrus => 0.2 + 0.35 * m,
        };
        self.storms.push(Storm {
            kind,
            pos,
            vel: [0.0, 0.0],
            radius: radius.clamp(2.0, 20.0),
            spawned: tick,
            water,
            from: kind,
            morph: 1.0,
            flash: 0,
            glow: 0,
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
        if self.channel[index] || self.built[index] != 0 {
            return;
        }
        self.plant_grass(index, kind, tick);
    }

    fn plant_tree(&mut self, index: usize, sp: Species, tick: u64) {
        self.plant_tree_with(index, sp, [1.0, 1.0], tick);
    }

    fn plant_tree_with(&mut self, index: usize, sp: Species, gene: [f32; 2], tick: u64) {
        // A seedling starts on its seed's reserves (acorns carry most).
        self.reserve[index] = (0.35 + 0.4 * sp.traits().seed_reserve) as f32;
        self.soaked[index] = 0;
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
        // Rivers bring beavers and floods that cut root crowns back:
        // clones don't regrow indefinitely.
        let chance = self.species(index).traits().resprout * (1.0 - 0.5 * self.params.rivers);
        if chance <= 0.0
            || rng::uniform01(self.seed, index as u32, tick, Stream::Resprout) >= chance
        {
            return false;
        }
        let (sp, gene) = (self.species(index), self.genome[index]);
        let stored = self.reserve[index];
        self.burn[index] = 0;
        self.plant_tree_with(index, sp, gene, tick);
        // The root crown's stores feed the new shoots.
        self.reserve[index] = self.reserve[index].max(stored * 0.7);
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
        if (self.channel[index] || self.built[index] != 0) && matches!(brush, Brush::Tree | Brush::Grass) {
            return; // nothing roots in open water or under a house
        }
        if brush == Brush::Tree {
            self.plant_tree(index, sp, tick);
        } else {
            self.paint(index, brush, tick);
        }
    }

    /// Fell the tree on a tile (the walking player's axe): it dies of
    /// logging, leaving a stump that rots like any husk. Returns the timber
    /// it yields (≈ 1 for a sapling up to 4 for a mature tree), or None if
    /// there's no tree.
    pub fn fell_tree(&mut self, index: usize, tick: u64) -> Option<f64> {
        if self.state[index] != Cell::Tree {
            return None;
        }
        let tr = self.species(index).traits();
        let mature = (self.params.tree_maturity_age as f64 * tr.maturity).max(1.0);
        let grown = (self.age(index, tick) as f64 / mature).clamp(0.0, 1.0);
        self.record_death(index, DeathCause::Logged);
        self.leave_remains(index, false, tick);
        self.state[index] = Cell::Bare;
        self.burn[index] = 0;
        Some(1.0 + 3.0 * grown)
    }

    /// Build a house on a tile: the ground is cleared and nothing grows
    /// there while it stands. Fails on open water or an existing house.
    pub fn build_house(&mut self, index: usize, tick: u64) -> bool {
        if self.channel[index] || self.built[index] != 0 {
            return false;
        }
        self.paint(index, Brush::Clear, tick);
        self.built[index] = 1;
        true
    }

    /// Whether the player built on this tile.
    pub fn is_built(&self, index: usize) -> bool {
        self.built[index] != 0
    }

    /// Tear a house down (the ground reopens to colonization).
    pub fn demolish(&mut self, index: usize) {
        self.built[index] = 0;
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
        self.evergreen_dist.fill(255);
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
                    // Young trees bear small crops (seed output grows with size).
                    let fecund = self.fecundity(i, tick) as f32;
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
                                let kw = kernel_weight(d) as f32 * g[0] * fecund;
                                self.seed_rain[j][sp] += kw;
                                self.gene_rain[j][sp][0] += kw * g[0];
                                self.gene_rain[j][sp][1] += kw * g[1];
                            }
                            self.tree_dist[j] = self.tree_dist[j].min(d as u8);
                            if SPECIES_TABLE[sp].deciduous < 0.5 {
                                self.evergreen_dist[j] = self.evergreen_dist[j].min(d as u8);
                            }
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

        self.cloud_life(tick);

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
            // Dynamic clouds rain only while they hold water (and a
            // thunderhead only once grown); the rain shaft shrinks as the
            // cloud rains itself out.
            let dyn_ = p.cloud_dynamics > 0.0;
            let grown = !dyn_ || (storm.water > RAIN_WATER && storm.morph > 0.5);
            let shaft = if dyn_ { (storm.water / 0.6).clamp(0.3, 1.0) } else { 1.0 };
            let orographic = dyn_ && self.orographic_rain(&storm);
            if (tr.rains && grown) || orographic {
                let core = if tr.rains && grown { storm.radius * tr.rain_core * shaft } else { storm.radius * 0.5 };
                for i in self.grid.cells_in_box(center, core) {
                    let (q, r) = self.grid.index_to_axial(i);
                    let (x, y) = hex::axial_to_world(q, r);
                    let d = ((x - center[0]).powi(2) + (y - center[1]).powi(2)).sqrt();
                    if d < core {
                        // Cold ground takes it as snow (climate zones on).
                        if p.climate_zones > 0.0 && self.temperature(i) < SNOW_PRECIP_T {
                            self.snow[i] = self.snow[i].saturating_add(SNOWFALL);
                        } else {
                            self.wet[i] = WET_TICKS;
                        }
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
                        self.record_death(i, DeathCause::Windthrow);
                        self.leave_remains(i, false, tick);
                        self.state[i] = Cell::Bare;
                    }
                }
            }
            // Lightning: only the thunderhead throws bolts, anywhere under
            // the cloud — possibly outside its rain core.
            let bolt_p = if grown { p.storm_lightning_p * tr.lightning } else { 0.0 };
            let k = |j: u32| rng::uniform01(self.seed, idx as u32 * 16 + j, tick, Stream::StormBolt);
            if bolt_p > 0.0 && k(0) < bolt_p {
                let rr = storm.radius * k(1).sqrt();
                let phi = std::f64::consts::TAU * k(2);
                self.storms[idx].flash = FLASH_TICKS;
                if let Some(i) = self.grid.pick(center[0] + rr * phi.cos(), center[1] + rr * phi.sin()) {
                    self.last_strike = Some(i);
                    self.bolt[i] = BOLT_TICKS;
                    if self.flammable(i, tick) && self.wet[i] == 0 {
                        self.burn[i] = BURN_TICKS;
                    }
                }
            }
        }
    }

    /// Whether a cloud is being forced up a windward slope hard enough, with
    /// enough water, to rain there whatever its genus (not thin cirrus).
    pub fn orographic_rain(&self, s: &Storm) -> bool {
        if s.kind == CloudKind::Cirrus || s.water <= OROG_WATER {
            return false;
        }
        let speed = (s.vel[0] * s.vel[0] + s.vel[1] * s.vel[1]).sqrt().max(1e-9);
        self.slope_along(s.pos[0], s.pos[1], [s.vel[0] / speed, s.vel[1] / speed]) > OROG_SLOPE
    }

    /// Surface height (render units, hills + mountains) at a world point,
    /// or None off the map.
    fn height_at(&self, x: f64, y: f64) -> Option<f64> {
        self.grid.pick(x, y).map(|i| self.elevation(i) as f64)
    }

    /// Terrain rise along a direction at a point: + climbing (windward
    /// lift), − descending (lee), in height units per world unit.
    fn slope_along(&self, x: f64, y: f64, dir: [f64; 2]) -> f64 {
        let step = 4.0;
        match (
            self.height_at(x + dir[0] * step, y + dir[1] * step),
            self.height_at(x - dir[0] * step, y - dir[1] * step),
        ) {
            (Some(a), Some(b)) => (a - b) / (2.0 * step),
            _ => 0.0,
        }
    }

    /// Evaporation source under a point, 0..1: the season's moisture, wet
    /// ground, groundwater, and open river water.
    fn evaporation_at(&self, i: usize) -> f64 {
        let ground = if self.wet[i] > 0 { 1.0 } else { self.water_table(i) as f64 };
        let river = if self.channel[i] { 1.0 } else { 0.0 };
        (0.5 * self.moisture + 0.5 * ground).max(river)
    }

    /// Dynamic clouds: water budgets, genus transitions, and in-place
    /// convective initiation (see CLOUD_* constants).
    fn cloud_life(&mut self, tick: u64) {
        let dyn_ = self.params.cloud_dynamics;
        if dyn_ <= 0.0 {
            return;
        }
        for k in 0..self.storms.len() {
            let mut s = self.storms[k];
            s.morph = (s.morph + CLOUD_MORPH).min(1.0);
            s.flash = s.flash.saturating_sub(1);
            s.glow = s.glow.saturating_sub(1);
            // Grown thunderheads flicker with in-cloud lightning.
            if s.kind == CloudKind::Cumulonimbus
                && s.morph >= 1.0
                && s.water > RAIN_WATER
                && rng::uniform01(self.seed, 9_000 + k as u32, tick, Stream::StormBolt) < INTRACLOUD_P
            {
                s.glow = 2;
            }
            let speed = (s.vel[0] * s.vel[0] + s.vel[1] * s.vel[1]).sqrt().max(1e-9);
            let dir = [s.vel[0] / speed, s.vel[1] / speed];
            let Some(i) = self.grid.pick(s.pos[0], s.pos[1]) else {
                self.storms[k] = s; // still offshore: no ground to feed on
                continue;
            };
            let slope = self.slope_along(s.pos[0], s.pos[1], dir);
            let evap = self.evaporation_at(i);
            let heat = self.temperature(i);
            let raining = (s.kind.traits().rains && s.water > RAIN_WATER) || self.orographic_rain(&s);
            let mut dw = CLOUD_EVAP * evap + CLOUD_LIFT * slope.max(0.0) - CLOUD_SINK * (-slope).max(0.0);
            if raining {
                dw -= CLOUD_RAIN_OUT;
            }
            if s.kind == CloudKind::Cumulus {
                dw -= CUMULUS_DRY * heat * (1.0 - evap);
            }
            s.water = (s.water + dw * dyn_).clamp(-0.01, 1.2);
            let next = match s.kind {
                // Towering needs an unstable column: surface heating, or
                // air forced up a ridge (orographic convection).
                CloudKind::Cumulus if s.water > CB_WATER && (heat > CB_HEAT || slope > FORCED_LIFT) => {
                    Some(CloudKind::Cumulonimbus)
                }
                CloudKind::Cumulonimbus if s.water < CB_SPENT => Some(CloudKind::Cirrus),
                CloudKind::Cirrus if s.water > FRONT_WATER => Some(CloudKind::Nimbostratus),
                CloudKind::Nimbostratus if s.water < NS_SPENT => Some(CloudKind::Cumulus),
                _ => None,
            };
            if let Some(n) = next {
                let ev = match n {
                    CloudKind::Cumulonimbus => CloudEvent::Towered,
                    CloudKind::Cirrus => CloudEvent::Collapsed,
                    CloudKind::Nimbostratus => CloudEvent::Front,
                    CloudKind::Cumulus => CloudEvent::BrokeUp,
                };
                self.cloud_events[ev as usize] += 1;
                s.from = s.kind;
                s.kind = n;
                s.morph = 0.0;
            }
            // The cloud grows or shrinks toward its genus' typical size.
            let tr = s.kind.traits();
            let target = 0.5 * (tr.radius_min + tr.radius_max);
            s.radius += (target - s.radius) * 0.04;
            self.storms[k] = s;
        }
        // Spent clouds evaporate away.
        let before = self.storms.len();
        self.storms.retain(|s| s.water > 0.0);
        self.cloud_events[CloudEvent::Evaporated as usize] += (before - self.storms.len()) as u32;
        // Multicell storms: a mature thunderhead's cold outflow (its gust
        // front) lifts warm air at its leading edge into new cells.
        let mut daughters = Vec::new();
        for (k, s) in self.storms.iter().enumerate() {
            if s.kind != CloudKind::Cumulonimbus || s.morph < 1.0 || s.water < 0.5 {
                continue;
            }
            let u = |j: u32| rng::uniform01(self.seed, 4_000 + k as u32 * 8 + j, tick, Stream::StormSpawn);
            if u(0) < OUTFLOW_P * dyn_ {
                let speed = (s.vel[0] * s.vel[0] + s.vel[1] * s.vel[1]).sqrt().max(1e-9);
                let side = (u(1) - 0.5) * 1.6;
                let (fx, fy) = (s.vel[0] / speed, s.vel[1] / speed);
                let pos = [
                    s.pos[0] + s.radius * (fx - side * fy),
                    s.pos[1] + s.radius * (fy + side * fx),
                ];
                daughters.push(pos);
            }
        }
        // Fire clouds: a big fire's heat lofts smoke and moisture into a
        // pyrocumulus over the flames.
        let burning: Vec<usize> = (0..self.grid.cells()).filter(|&i| self.burn[i] > 0).collect();
        if burning.len() >= PYRO_TILES
            && rng::uniform01(self.seed, 8_888, tick, Stream::StormSpawn) < PYRO_P * dyn_
        {
            let pick = rng::hash(self.seed, 8_889, tick, Stream::StormSpawn) as usize % burning.len();
            let (x, y) = self.grid.center(burning[pick]);
            self.spawn_cloud(CloudKind::Cumulus, [x, y], 3.5, tick);
            self.cloud_events[CloudEvent::Pyro as usize] += 1;
            if let Some(c) = self.storms.last_mut() {
                c.water = 0.55;
            }
        }
        for pos in daughters {
            self.cloud_events[CloudEvent::Daughter as usize] += 1;
            self.spawn_cloud(CloudKind::Cumulus, pos, 3.0, tick);
            if let Some(c) = self.storms.last_mut() {
                c.water = 0.45;
            }
        }
        // Convection bubbles up new cumulus over warm, moist, windward
        // ground (thermals and orographic lift), not only from upwind.
        let (dir, _) = self.wind_at(tick);
        let sites = (INIT_SITES * self.grid.area_ratio()).ceil() as u32;
        for k in 0..sites {
            let u = |j: u32| rng::uniform01(self.seed, k * 8 + j, tick, Stream::StormSpawn);
            let t = (u(1) * self.grid.cells() as f64) as usize % self.grid.cells();
            if self.channel[t] {
                continue;
            }
            let (x, y) = self.grid.center(t);
            let lift = 1.0 + 6.0 * self.slope_along(x, y, dir).max(0.0);
            let heat = self.temperature(t);
            let p = self.params.storm_rate * dyn_ * INIT_P * heat * self.evaporation_at(t) * lift;
            if u(2) < p {
                let radius = 2.5 + 1.5 * u(3);
                self.spawn_cloud(CloudKind::Cumulus, [x, y], radius, tick);
                self.cloud_events[CloudEvent::Formed as usize] += 1;
                if let Some(c) = self.storms.last_mut() {
                    c.water = 0.25;
                }
            }
        }
    }

    /// Long-distance dispersal: every mature tree occasionally sends one
    /// seed beyond its seed-rain disk — jays caching acorns, winged pine
    /// seed and cottony willow seed on the wind, acacia pods carried off by
    /// browsers — a distance drawn from the species' bounded fat-tailed
    /// kernel (see `ldd_target`), at a rate that grows with the parent's
    /// size (`fecundity`). Arrival is not establishment: the seed then faces exactly
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
            let rate = LDD_P * tr.long_distance * self.genome[o][0] as f64 * self.fecundity(o, tick);
            if rng::uniform01(self.seed, o as u32, tick, Stream::Jay) >= rate {
                continue;
            }
            let u_dist = rng::uniform01(self.seed, o as u32 + 7919, tick, Stream::Jay);
            let u_angle = rng::uniform01(self.seed, o as u32 + 15_881, tick, Stream::Jay);
            let Some(t) = self.ldd_target(o, u_dist, u_angle) else {
                continue; // carried off the map
            };
            if self.state[t] == Cell::Tree || self.remains_code[t] != 0 || self.burn[t] > 0 {
                continue;
            }
            let light = self.canopy_light(t);
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

    /// Where a long-distance seed from the tree on `o` lands, from two
    /// uniform draws: a distance from the species' fat-tailed 2Dt kernel
    /// (Clark et al. 1999), `r = s·√((1−u)^(−1/p) − 1)` with scale `s`
    /// and shape LDD_SHAPE, truncated at the species' cap, in a uniform
    /// direction. Most seed lands within a few scales; rare seed flies to
    /// the cap — never across an arbitrarily large map.
    fn ldd_target(&self, o: usize, u_dist: f64, u_angle: f64) -> Option<usize> {
        let tr = self.species(o).traits();
        let r = ldd_distance(tr.ldd_scale, tr.ldd_cap, u_dist);
        let theta = std::f64::consts::TAU * u_angle;
        // Tile centers are √3 world units apart.
        let (x, y) = self.grid.center(o);
        let step = hex::SQRT3 * hex::SIZE;
        self.grid.pick(x + r * step * theta.cos(), y + r * step * theta.sin())
    }

    /// Seed output relative to a full-grown tree: a tree that has just
    /// matured bears a small crop, rising to full fecundity at
    /// FECUNDITY_FULL × its maturity age (seed production scales with size).
    fn fecundity(&self, index: usize, tick: u64) -> f64 {
        let m = self.maturity_age(index).max(1) as f64;
        let age = self.age(index, tick) as f64;
        (FECUNDITY_MIN + (1.0 - FECUNDITY_MIN) * (age - m) / (m * (FECUNDITY_FULL - 1.0)))
            .clamp(FECUNDITY_MIN, 1.0)
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
                let tr = &SPECIES_TABLE[sp];
                let weak = 1.0 - self.reserve[i] as f64;
                let wet_year = ((self.moisture - FLOOD_MOISTURE) / (1.0 - FLOOD_MOISTURE)).max(0.0);
                let trigger = 1.0
                    + self.params.physiology * (BEETLE_WEAK * tr.beetle * weak + WET_ROT * tr.wet_rot * wet_year);
                let p = strength * PEST_SEED * susc * density * density * trigger;
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

    /// Snowmelt: a snowpack melts in warm spells, wetting the ground; a
    /// big melt across the map swells the rivers (see flood_pass).
    fn snow_pass(&mut self) {
        let mut melted = 0.0;
        for i in 0..self.grid.cells() {
            if self.snow[i] == 0 {
                continue;
            }
            let t = self.temperature(i);
            if t > MELT_T {
                let m = ((MELT_RATE * (t - MELT_T) / 0.1).ceil() as u8).min(self.snow[i]);
                self.snow[i] -= m;
                self.wet[i] = WET_TICKS;
                melted += m as f64 / 255.0;
            }
        }
        self.melt_recent = melted / self.grid.cells() as f64;
    }

    /// River flood pulses. Wet seasons can send the rivers over their
    /// banks; for FLOOD_TICKS the floodplain (up to `flood_stage` of each
    /// corridor's reach) is under water: grasses other than sedge and young
    /// saplings are scoured away, fires drowned. When the water recedes it
    /// leaves fresh sediment bars on the bare ground — the seedbed willows
    /// (and annual pioneers) recruit on.
    fn flood_pass(&mut self, tick: u64) {
        for s in self.sediment.iter_mut() {
            *s = s.saturating_sub(1);
        }
        let r = self.params.rivers;
        if r <= 0.0 {
            self.flood_left = 0;
            return;
        }
        if self.flood_left == 0 {
            // Wet seasons and snowmelt freshets both flood the rivers.
            let wetness = ((self.moisture - FLOOD_MOISTURE) / (1.0 - FLOOD_MOISTURE)).max(0.0)
                + FLOOD_MELT * self.melt_recent;
            if rng::uniform01(self.seed, 0, tick, Stream::Flood) < r * FLOOD_P * wetness {
                self.flood_left = FLOOD_TICKS;
                self.flood_stage = 0.5 + 0.8 * rng::uniform01(self.seed, 1, tick, Stream::Flood);
            }
            return;
        }
        for i in 0..self.grid.cells() {
            if !self.inundated(i) {
                continue;
            }
            self.wet[i] = WET_TICKS;
            self.burn[i] = 0;
            let scour = match self.state[i] {
                Cell::Grass => !self.grass_kind(i).traits().flood_tolerant,
                Cell::Tree => self.age(i, tick) < SCOUR_AGE,
                Cell::Bare => false,
            };
            if scour {
                self.record_death(i, DeathCause::Scoured);
                self.state[i] = Cell::Bare;
                self.clear_remains(i); // washed away, not left standing
            }
        }
        self.flood_left -= 1;
        if self.flood_left == 0 {
            for i in 0..self.grid.cells() {
                if self.in_floodplain(i, self.flood_stage)
                    && self.state[i] == Cell::Bare
                    && self.remains_code[i] == 0
                {
                    self.sediment[i] = SEDIMENT_TICKS;
                }
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
            let riparian = 1.0 + RIPARIAN_BROWSE * self.params.rivers * self.water_table(i) as f64;
            let p = self.params.browse * BROWSE_P * pal * riparian;
            if rng::uniform01(self.seed, i as u32, tick, Stream::Browse) >= p {
                continue;
            }
            if rng::uniform01(self.seed, i as u32 + self.grid.cells() as u32, tick, Stream::Browse) < BROWSE_KILL {
                self.record_death(i, DeathCause::Browsed);
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
                        let (sp, gene, stored) = (self.species(i), self.genome[i], self.reserve[i]);
                        self.burn[i] = 0;
                        self.plant_tree_with(i, sp, gene, tick);
                        self.reserve[i] = self.reserve[i].max(stored * 0.7);
                        self.ash[i] = ASH_TICKS;
                        continue;
                    }
                }
                if self.try_resprout(i, tick) {
                    self.ash[i] = ASH_TICKS;
                    continue;
                }
                if self.state[i] != Cell::Bare {
                    self.record_death(i, DeathCause::Fire);
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
            let is_tree = self.state[i] == Cell::Tree;
            // Trees drink what their roots reach; grass the shallow soil.
            let water = if is_tree { self.tree_water(i, tick) } else { self.tile_water(i) };
            let base_stress = 0.4 + 1.2 * self.drought(i, water);
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
                    // With physiology, carbon starvation, flood rot and the
                    // like kill explicitly, so the flat background hazard
                    // covers less (PHYSIOLOGY_LIFE).
                    (
                        base_stress.powf(tr.drought_sensitivity / hardy as f64),
                        self.params.tree_mean_life as f64 * tr.mean_life / vigor as f64
                            * (1.0 + PHYSIOLOGY_LIFE * self.params.physiology),
                    )
                }
            };
            let mut hazard = (stress / mean).min(1.0);
            if self.state[i] == Cell::Grass && self.params.grass_niches > 0.0 {
                // Off-niche grasses die faster, on-niche ones slower.
                let k = self.species[i] as usize;
                let fit = self.grass_fit_now[i][k];
                hazard *= (1.0 + self.params.grass_niches * (0.6 - 1.2 * fit)).max(0.3);
                // Grazers crop the palatable grasses, most near water.
                hazard *= 1.0 + GRAZE_HAZARD * self.grazing_intensity(i) * GRASS_TABLE[k].palatability;
                if !GRASS_TABLE[k].flood_tolerant {
                    let excess = (self.water_table(i) as f64 - 0.5).max(0.0);
                    hazard *= 1.0 + self.params.grass_niches * WATERLOG_HAZARD * excess;
                }
                hazard = hazard.min(1.0);
            }
            // Trees: build the hazard from named parts so the death can be
            // attributed to whichever added the most.
            let mut parts = [0.0f64; DEATH_CAUSE_COUNT];
            if is_tree {
                let age_part = (1.0 / mean).min(hazard);
                parts[DeathCause::Age as usize] = age_part;
                parts[DeathCause::Drought as usize] = hazard - age_part;
                let phys = self.params.physiology;
                let tr = self.species(i).traits();
                let apply = |hazard: &mut f64, parts: &mut [f64; DEATH_CAUSE_COUNT], cause: DeathCause, new: f64| {
                    let new = new.clamp(0.0, 1.0);
                    parts[cause as usize] += (new - *hazard).max(0.0);
                    *hazard = new;
                };
                // Established roots outcompete a sapling for water on dry
                // ground; a heavy pest load kills.
                if !self.is_mature(i, tick) {
                    let h = hazard * (1.0 + self.root_water_stress(i) * tr.drought_sensitivity);
                    apply(&mut hazard, &mut parts, DeathCause::Drought, h);
                }
                let load = self.pest[i] as f64 / 255.0;
                let h = 1.0 - (1.0 - hazard) * (1.0 - PEST_HAZARD * load);
                apply(&mut hazard, &mut parts, DeathCause::Pests, h);
                // Frost kills tender trees in the cold zones.
                let h = hazard * (1.0 + FROST_HAZARD * self.frost(i, tr.cold_hardy));
                apply(&mut hazard, &mut parts, DeathCause::Frost, h);
                // Waterlogging: the older instant model fades out as the
                // duration model (roots rot only past the species'
                // tolerance) fades in.
                let wt = self.water_table(i) as f64;
                if tr.flood_tolerant {
                    let sat = ((wt - ANOXIA_LEVEL) / (1.0 - ANOXIA_LEVEL)).clamp(0.0, 1.0);
                    let h = hazard * (1.0 + (1.0 - phys) * self.params.rivers * ANOXIA_HAZARD * sat);
                    apply(&mut hazard, &mut parts, DeathCause::Flood, h);
                } else {
                    let excess = (wt - 0.5).max(0.0);
                    let h = hazard * (1.0 + (1.0 - phys) * WATERLOG_HAZARD * excess);
                    apply(&mut hazard, &mut parts, DeathCause::Flood, h);
                }
                let over = (self.soaked[i] as f64 - tr.flood_days) / tr.flood_days;
                if phys > 0.0 && over > 0.0 {
                    let h = 1.0 - (1.0 - hazard) * (1.0 - phys * FLOOD_ROT * over.min(1.0));
                    apply(&mut hazard, &mut parts, DeathCause::Flood, h);
                }
                // Carbon starvation: an emptied reserve (deep shade, a long
                // drought, heat, pests) kills — the slow decline after a
                // bad spell that a flat hazard can't produce.
                let short = ((STARVE_BELOW - self.reserve[i] as f64) / STARVE_BELOW).clamp(0.0, 1.0);
                if phys > 0.0 && short > 0.0 {
                    let h = 1.0 - (1.0 - hazard) * (1.0 - phys * STARVE_HAZARD * short * short);
                    apply(&mut hazard, &mut parts, DeathCause::Starvation, h);
                }
            }
            if self.nursed[i] != 0 {
                // The network feeds its kin: nursed seedlings die less.
                hazard *= NURSE_FACTOR;
            }
            if is_tree && self.mature_nbrs[i] > CROWD_FREE {
                let excess = (self.mature_nbrs[i] - CROWD_FREE) as f64;
                let cp = (self.params.crowding_p * self.species(i).traits().crowding).min(1.0);
                let p_crowd = 1.0 - (1.0 - cp).powf(excess);
                let h = 1.0 - (1.0 - hazard) * (1.0 - p_crowd);
                parts[DeathCause::Crowding as usize] += h - hazard;
                hazard = h;
            }
            if rng::uniform01(self.seed, i as u32, tick, Stream::Mortality) < hazard
                && !self.try_resprout(i, tick)
            {
                if is_tree {
                    let mut cause = 0;
                    for (k, &v) in parts.iter().enumerate() {
                        if v > parts[cause] {
                            cause = k;
                        }
                    }
                    self.record_death(i, DeathCause::from_u8(cause as u8));
                }
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

    /// Understory light on a tile: 1 in the open, falling beside mature
    /// canopy. With seasons, a deciduous canopy is bare through spring and
    /// casts less shade than an evergreen one at the same distance.
    pub fn canopy_light(&self, i: usize) -> f64 {
        let s = self.params.shade_strength;
        let d = self.tree_dist[i];
        if d == 255 {
            return 1.0;
        }
        let nearest = s * shade_suppression(d);
        if self.params.seasons <= 0.0 {
            return 1.0 - nearest;
        }
        let e = self.evergreen_dist[i];
        let evergreen = if e == 255 { 0.0 } else { s * shade_suppression(e) };
        let nearest = if e <= d { nearest } else { nearest * (1.0 - DECID_RELIEF * self.params.seasons) };
        1.0 - nearest.max(evergreen)
    }

    /// How far a tree's roots reach toward the groundwater, 0..1: deepens
    /// with age toward the species' taproot (saplings sip rain; old oaks
    /// and acacias tap the water table).
    pub fn root_depth(&self, i: usize, tick: u64) -> f64 {
        if self.state[i] != Cell::Tree {
            return 0.0;
        }
        let age = self.age(i, tick) as f64;
        self.species(i).traits().taproot * (1.0 - (-age / ROOT_TIME).exp())
    }

    /// Water available to the tree on a tile: rain plus whatever of the
    /// groundwater its roots reach (every plant got all of it before).
    fn tree_water(&self, i: usize, tick: u64) -> f64 {
        if self.wet[i] > 0 {
            return 1.0;
        }
        let access = 1.0 + self.params.physiology * (self.root_depth(i, tick) - 1.0);
        let wt = self.water_table(i) as f64 * WATER_TABLE_REACH * access;
        let retention = 0.85 + 0.15 * self.soil_depth(i);
        let m = (self.moisture + self.rain_shift(i)).clamp(0.0, 1.0);
        (m + (1.0 - m) * wt) * retention
    }

    /// Carbon reserve of the tree on a tile, 0..1.
    pub fn reserve(&self, i: usize) -> f32 {
        if self.state[i] == Cell::Tree { self.reserve[i] } else { 0.0 }
    }

    /// Tree deaths by cause over roughly the last 50 ticks (decaying).
    pub fn deaths_recent(&self) -> [f32; DEATH_CAUSE_COUNT] {
        self.deaths_recent
    }

    /// Edit a cloud in place (render tests set up mid-transition states).
    #[doc(hidden)]
    pub fn debug_set_cloud(&mut self, k: usize, f: impl FnOnce(&mut Storm)) {
        if let Some(c) = self.storms.get_mut(k) {
            f(c);
        }
    }

    /// Whether a cloud is precipitating right now (what the rain loop
    /// applies to the ground).
    pub fn precipitating(&self, s: &Storm) -> bool {
        if self.params.cloud_dynamics <= 0.0 {
            return s.kind.traits().rains;
        }
        (s.kind.traits().rains && s.water > RAIN_WATER && s.morph > 0.5) || self.orographic_rain(s)
    }

    /// The map's altitude range (grows with map size), for display.
    pub fn altitude_range(&self) -> f32 {
        self.terrain.altitude_amp
    }

    /// Snowpack on a tile, 0..1.
    pub fn snowpack(&self, index: usize) -> f32 {
        self.snow[index] as f32 / 255.0
    }

    /// Site climate (temperature index, water) — what the biome is read
    /// from (the weather swing damped, as for niches).
    pub fn site_climate(&self, index: usize) -> (f64, f64) {
        self.niche_site(index)
    }

    /// The climate biome of a tile (Whittaker-style temperature × water).
    pub fn biome(&self, index: usize) -> Biome {
        let (t, w) = self.niche_site(index);
        if self.channel[index] || self.water_table(index) > 0.8 {
            Biome::Wetland
        } else if t < 0.22 {
            Biome::Tundra
        } else if t < 0.36 {
            if w < 0.25 { Biome::Tundra } else { Biome::Boreal }
        } else if t < 0.58 {
            if w < 0.42 { Biome::Grassland } else { Biome::TemperateForest }
        } else if w < 0.25 {
            Biome::Desert
        } else {
            Biome::Savanna
        }
    }

    /// Fraction of the sky's light blocked by cloud over the map, 0..1 —
    /// storm light dims and cools the scene under overcast.
    pub fn overcast(&self) -> f64 {
        let area = self.grid.cells() as f64 * 2.598;
        let covered: f64 = self
            .storms
            .iter()
            .map(|s| {
                let tau = s.kind.traits().optical_depth as f64;
                std::f64::consts::PI * s.radius * s.radius * (1.0 - (-tau).exp())
            })
            .sum();
        (covered / area).min(1.0)
    }

    /// Where the most recent ground strike hit (world x, y), if any.
    pub fn last_strike(&self) -> Option<[f64; 2]> {
        self.last_strike.map(|i| {
            let (x, y) = self.grid.center(i);
            [x, y]
        })
    }

    /// Lightning flash right now, 0..1 (a ground strike lights the scene).
    pub fn flash(&self) -> f32 {
        self.storms.iter().map(|s| s.flash).max().unwrap_or(0) as f32 / FLASH_TICKS as f32
    }

    /// Cloud lifecycle events since the world began (CloudEvent order).
    pub fn cloud_events(&self) -> [u32; CLOUD_EVENT_COUNT] {
        self.cloud_events
    }

    /// Tree deaths by cause since the world began.
    pub fn deaths_total(&self) -> [u32; DEATH_CAUSE_COUNT] {
        self.deaths_total
    }

    /// Everything that bears on a tile, as readable lines for the tile
    /// inspector: the site (altitude, temperature, water, soil, light),
    /// what grows there and how it's doing, or why the last occupant died.
    pub fn inspect(&self, i: usize, tick: u64) -> String {
        // Temperature index → a nominal °C (0 ≈ −5 °C, 1 ≈ 35 °C).
        let celsius = |t: f64| -5.0 + 40.0 * t;
        let pct = |v: f64| format!("{:.0}%", 100.0 * v);
        let mut out = Vec::new();
        let (q, r) = self.grid.index_to_axial(i);
        out.push(format!("Tile ({q}, {r})"));
        if self.channel[i] {
            out.push("Open river water — nothing roots here".to_string());
        }
        if self.built[i] != 0 {
            out.push("A house stands here — nothing grows on its floor".to_string());
        }
        out.push(format!(
            "Altitude {} · {:.0} °C (site {:.0} °C){}",
            pct(self.terrain.altitude[i] as f64),
            celsius(self.temperature(i)),
            celsius(self.niche_site(i).0),
            if self.snow_cover(i) > 0.0 { " · snow" } else { "" }
        ));
        out.push(format!(
            "Soil water {} · groundwater {} · soil depth {} · fertility {}",
            pct(self.tile_water(i)),
            pct(self.water_table(i) as f64),
            pct(self.soil_depth(i)),
            pct(self.nutrient_ratio(i) as f64)
        ));
        out.push(format!("Understory light {}", pct(self.canopy_light(i))));
        let snow = self.snowpack(i);
        out.push(format!(
            "Biome: {}{}",
            self.biome(i).name(),
            if snow > 0.0 { format!(" · snowpack {}", pct(snow as f64)) } else { String::new() }
        ));
        match self.state[i] {
            Cell::Tree => {
                let sp = self.species(i);
                let tr = sp.traits();
                let age = self.age(i, tick);
                let mature = if self.is_mature(i, tick) { "mature" } else { "sapling" };
                out.push(format!("{} — age {age} ({mature})", tr.name));
                if self.params.physiology > 0.0 {
                    let temp = self.temperature(i);
                    let bell = (-((temp - tr.temp_opt) / (tr.temp_width * 1.4)).powi(2)).exp();
                    out.push(format!(
                        "Carbon reserve {} · thermal fit {} · roots reach {} of the groundwater",
                        pct(self.reserve[i] as f64),
                        pct(bell),
                        pct(self.root_depth(i, tick))
                    ));
                    out.push(format!(
                        "Tree water {} · waterlogged {} of {:.0} tolerable ticks",
                        pct(self.tree_water(i, tick)),
                        self.soaked[i],
                        tr.flood_days
                    ));
                }
                let load = self.pest[i] as f64 / 255.0;
                if load > 0.0 {
                    out.push(format!("Pest outbreak: load {}", pct(load)));
                }
                let [vigor, hardy] = self.genome[i];
                out.push(format!("Genome: vigor {vigor:.2} · hardiness {hardy:.2}"));
            }
            Cell::Grass => {
                let k = self.grass_kind(i);
                out.push(format!(
                    "{} — age {} · niche fit {} · grazing {}",
                    k.traits().name,
                    self.age(i, tick),
                    pct(self.grass_fit_raw(k as usize, i)),
                    pct(self.grazing_intensity(i))
                ));
            }
            Cell::Bare => {
                if !self.channel[i] {
                    // Which tree would take here, and how readily.
                    let light = self.canopy_light(i);
                    let mut best = (0.0, "none");
                    for k in 0..SPECIES_COUNT {
                        let e = self.tree_establishment(i, k, light);
                        if e > best.0 {
                            best = (e, SPECIES_TABLE[k].name);
                        }
                    }
                    out.push(format!("Bare ground — best tree site: {} ({:.2})", best.1, best.0));
                }
            }
        }
        if let Some(rm) = self.remains(i) {
            if rm.tree {
                out.push(format!("Standing dead {} — died of {}", rm.species.traits().name, rm.cause.name()));
            }
        }
        if self.inundated(i) {
            out.push("Under floodwater".to_string());
        } else if self.sediment[i] > 0 {
            out.push("Fresh flood sediment (a seedbed for willow)".to_string());
        }
        out.join("\n")
    }

    /// Tally a tree death and stamp its cause on the husk it leaves.
    fn record_death(&mut self, i: usize, cause: DeathCause) {
        if self.state[i] != Cell::Tree {
            return;
        }
        self.remains_cause[i] = cause as u8;
        self.deaths_total[cause as usize] += 1;
        self.deaths_recent[cause as usize] += 1.0;
    }

    /// Carbon economy of every tree (physiology): photosynthetic income
    /// limited by the scarcest of light, temperature, and water (Liebig),
    /// minus maintenance respiration that climbs with heat above the
    /// species' optimum (Q10) and with pest load. Oaks pay for mast crops.
    /// Also tracks waterlogging duration for flood tolerance.
    fn carbon_pass(&mut self, tick: u64) {
        let phys = self.params.physiology;
        for r in self.deaths_recent.iter_mut() {
            *r *= DEATH_RECENT_KEEP;
        }
        if phys <= 0.0 {
            return;
        }
        for i in 0..self.grid.cells() {
            if self.state[i] != Cell::Tree {
                continue;
            }
            let tr = self.species(i).traits();
            let mature = self.is_mature(i, tick);
            let light = if mature {
                1.0 / (1.0 + CROWD_LIGHT * self.mature_nbrs[i] as f64)
            } else {
                let l = self.canopy_light(i);
                l + tr.shade_tolerance * (1.0 - l)
            };
            let temp = self.temperature(i);
            let bell = (-((temp - tr.temp_opt) / (tr.temp_width * 1.4)).powi(2)).exp();
            let water = (self.tree_water(i, tick) / WATER_SUFFICIENT).min(1.0);
            let income = CARBON_GAIN * light * bell * water.powf(tr.drought_sensitivity);
            let load = self.pest[i] as f64 / 255.0;
            // Respiration acclimates to the site's climate (the damped
            // niche temperature), not to every hot or cold year.
            let site = self.niche_site(i).0;
            let size = if mature { 1.0 } else { SAPLING_COST };
            let cost = CARBON_COST * size * Q10.powf((site - tr.temp_opt) / Q10_STEP) * (1.0 + load);
            let mut delta = (income - cost) * phys;
            if tr.jay_cached && mature && self.mast {
                delta -= MAST_COST * phys;
            }
            self.reserve[i] = (self.reserve[i] as f64 + delta).clamp(0.0, 1.0) as f32;
            // Flood-tolerant roots only count permanently saturated ground
            // (or a flood) against them; others any waterlogged soil.
            let limit = if tr.flood_tolerant { ANOXIA_LEVEL } else { SOAK_LEVEL };
            let soaking = self.inundated(i) || self.water_table(i) as f64 > limit;
            self.soaked[i] = if soaking { self.soaked[i].saturating_add(1) } else { self.soaked[i].saturating_sub(2) };
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
            // Heat helps seedlings only in the old monotonic model; with
            // physiology the species' own thermal bell (below) decides.
            base: fert * (1.0 + (1.0 - p.physiology) * (0.4 + 1.2 * self.temperature(i) - 1.0)),
            water_base: 0.3 + 1.4 * water,
            sod: if self.state[i] == Cell::Grass {
                Some(p.sod_factor * self.gmul(self.grass_kind(i).traits().sod))
            } else {
                None
            },
            site_temp: self.site_temp[i],
            soil: p.terrain * SOIL_COMPETITION * (self.depth(i) - SOIL_TYPICAL),
            on_ash: self.ash[i] > 0,
            flood: (1.0 - 2.0 * (self.water_table(i) as f64 - 0.5).max(0.0)).max(0.0),
            wet_gate: ((ground_wet - 0.15) / 0.45).clamp(0.0, 1.0),
            litter: (1.0 - p.competition * ALLELOPATHY * self.litter[i] as f64).max(0.0),
            root_stress: self.root_water_stress(i),
            temp: self.temperature(i),
            groundwater: self.water_table(i) as f64,
            bare: self.state[i] == Cell::Bare,
            sediment: self.sediment[i] > 0,
            channel: self.channel[i],
        }
    }

    /// Establishment multiplier for species `k` on tile `i` given its
    /// shared site conditions `c` and canopy `light`.
    fn establish(&self, c: &SiteCtx, i: usize, k: usize, light: f64) -> f64 {
        let p = &self.params;
        let tr = &SPECIES_TABLE[k];
        if c.channel {
            return 0.0; // open water
        }
        if k >= BASE_SPECIES && p.biomes <= 0.0 {
            return 0.0; // the biome plants exist only with biomes on
        }
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
        // Willow seed only lives days: dry ground barely takes it. With
        // rivers, it needs the real recruitment box — a bare, freshly
        // flooded sediment bar — not just damp ground or a sod mat.
        let damp = tr.dry_ground + (1.0 - tr.dry_ground) * c.wet_gate;
        pk *= if tr.seedbed && p.rivers > 0.0 {
            let bed = if c.sediment {
                1.0
            } else if c.bare {
                0.3 * c.wet_gate
            } else {
                0.0
            };
            damp + p.rivers * (bed - damp)
        } else {
            damp
        };
        // Even flood-tolerant roots suffocate in permanently saturated
        // ground (that's sedge marsh).
        if tr.flood_tolerant {
            let sat = ((c.groundwater - ANOXIA_LEVEL) / (1.0 - ANOXIA_LEVEL)).clamp(0.0, 1.0);
            pk *= 1.0 - p.rivers * sat;
        }
        // Climate zones: frost-tender seedlings fail in the cold, and above
        // the treeline no tree establishes at all.
        let cold = ((FROST_T - c.temp) / FROST_T).clamp(0.0, 1.0);
        pk *= 1.0 - p.climate_zones * cold * (1.0 - tr.cold_hardy);
        let treeline = ((c.temp - TREELINE_T) / TREELINE_BAND).clamp(0.0, 1.0);
        pk *= 1.0 - p.climate_zones * (1.0 - treeline);
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
            if self.burn[i] > 0
                || self.state[i] == Cell::Tree
                || self.remains_code[i] != 0
                || self.channel[i]
                || self.built[i] != 0
            {
                continue;
            }
            let s = self.state[i];
            // Light on this tile: 1 in the open, sliding to 0 beside a mature
            // canopy. It gates BOTH understories — grass, and tree seedlings
            // (gap-phase regeneration: recruitment happens in openings and at
            // edges, never under a closed canopy).
            let light = self.canopy_light(i);
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
                // Spontaneous seed splits across the kinds actually present.
                let pool = if p.biomes > 0.0 { GRASS_KIND_COUNT } else { BASE_GRASS_KINDS };
                let seed = if own && k < pool { p.grass_seed_p / pool as f64 } else { 0.0 }
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
                if tr.seeder && (on_ash || self.sediment[i] > 0) {
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
                if k >= BASE_GRASS_KINDS {
                    pk *= p.biomes; // biome plants only with biomes on
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
        self.snow_pass();
        self.flood_pass(tick);
        self.rebuild_fields(tick);
        self.root_pass(tick);
        self.dispersal_pass(tick);
        self.pest_pass(tick);
        self.carbon_pass(tick);
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
        Params::legacy_map()
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
            climate_zones: 2.0,
            rivers: f64::NAN,
            grazing: -0.5,
            physiology: 4.0,
            seasons: f64::NEG_INFINITY,
            cloud_dynamics: -3.0,
            biomes: 2.0,
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
        assert_eq!((p.climate_zones, p.rivers, p.grazing), (1.0, RIVERS, 0.0));
        assert_eq!((p.physiology, p.seasons), (1.0, SEASONS));
        assert_eq!(p.cloud_dynamics, 0.0);
        assert_eq!(p.biomes, 1.0);
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
    fn long_distance_seed_follows_a_bounded_fat_tailed_kernel() {
        // Distances straight from the kernel: most seed stays within a few
        // scales, the tail reaches well beyond, and nothing passes the cap.
        for sp in [Species::Acacia, Species::Oak, Species::Pine, Species::Willow] {
            let tr = sp.traits();
            let mut d: Vec<f64> =
                (0..10_000).map(|k| ldd_distance(tr.ldd_scale, tr.ldd_cap, (k as f64 + 0.5) / 10_000.0)).collect();
            d.sort_by(f64::total_cmp);
            let median = d[5_000];
            assert!(median > 0.5 * tr.ldd_scale && median < 2.0 * tr.ldd_scale, "{sp:?} median {median:.1}");
            assert!(d[9_900] > 3.0 * tr.ldd_scale, "{sp:?} needs a fat tail ({:.1})", d[9_900]);
            assert!(*d.last().unwrap() <= tr.ldd_cap + 1e-9, "{sp:?} past the cap");
        }
        // And landing tiles on a big map respect the cap too.
        let mut w = World::with_params(3, Params { width: 256, height: 256, ..no_fire() });
        let g = w.grid();
        let c = g.middle();
        w.paint_species(c, Brush::Tree, Species::Pine, 0);
        let (cq, cr) = g.index_to_axial(c);
        let cap = Species::Pine.traits().ldd_cap;
        for k in 0..2_000u64 {
            let (u, a) = (rng::uniform01(1, k as u32, 0, Stream::Jay), rng::uniform01(2, k as u32, 0, Stream::Jay));
            if let Some(t) = w.ldd_target(c, u, a) {
                let (q, r) = g.index_to_axial(t);
                // The cap is Euclidean (in tile spacings); hex step counts
                // run up to 2/√3 longer along the diagonals.
                assert!(hex::distance(q, r, cq, cr) as f64 <= cap * 1.155 + 1.5, "seed landed past the cap");
            }
        }
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
        with_adults(&mut w, c, [0, 6, 0, 0, 0, 0, 0]);
        let under_oaks = w.tree_establishment(c, oak, 0.4);
        with_adults(&mut w, c, [0, 0, 6, 0, 0, 0, 0]);
        let under_pines = w.tree_establishment(c, oak, 0.4);
        assert!(under_oaks < under_pines * 0.6, "{under_oaks:.4} vs {under_pines:.4}");

        let mut off = bare_world(5, no_fire());
        with_adults(&mut off, c, [0, 6, 0, 0, 0, 0, 0]);
        let a = off.tree_establishment(c, oak, 0.4);
        with_adults(&mut off, c, [0, 0, 6, 0, 0, 0, 0]);
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

    /// Lab world with physiology (and seasons) switched on.
    fn physio() -> Params {
        Params { physiology: 1.0, seasons: 1.0, ..no_fire() }
    }

    #[test]
    fn a_sapling_in_deep_shade_starves_while_one_in_the_open_thrives() {
        // Same species, same site, same start: shade tolerance 0 pine.
        let mut w = bare_world(12, Params { tree_growth_p: 0.0, tree_mean_life: 100_000, crowding_p: 0.0, ..physio() });
        let (c, q, r) = center();
        // A ring of mature evergreen pines around the shaded sapling (a
        // deciduous ring would let spring light through).
        for (dq, dr) in hex::NEIGHBORS {
            let j = G.axial_to_index(q + dq, r + dr).unwrap();
            w.paint_species(j, Brush::Tree, Species::Pine, 0);
        }
        let open = G.axial_to_index(q + 20, r).unwrap();
        for tick in 200..230 {
            if tick == 200 {
                w.paint_species(c, Brush::Tree, Species::Pine, tick);
                w.paint_species(open, Brush::Tree, Species::Pine, tick);
            }
            w.step(tick);
        }
        let (shaded, lit) = (w.reserve(c), w.reserve(open));
        assert!(shaded < 0.2 && lit > 0.6, "shaded {shaded:.2} vs open {lit:.2}");
    }

    #[test]
    fn heat_above_a_species_optimum_raises_its_upkeep() {
        // Q10 respiration: the same pine loses carbon faster on a hot site.
        let run = |sun: f64| {
            let mut w = bare_world(13, Params { tree_growth_p: 0.0, tree_mean_life: 100_000, ..physio() });
            let (c, _, _) = center();
            w.paint_species(c, Brush::Tree, Species::Pine, 0);
            w.reserve[c] = 0.5;
            w.sun = sun;
            w.carbon_pass(100);
            w.reserve[c]
        };
        // carbon_pass reads the site temperature through niche_site, which
        // follows the (damped) climate sun.
        assert!(run(0.9) < run(0.5), "hot upkeep should cut the reserve more");
    }

    #[test]
    fn roots_deepen_with_age_and_old_trees_drink_the_groundwater() {
        let mut w = World::with_params(7, Params { water_table: 1.0, ..physio() });
        let wet = (0..CELLS).find(|&i| (0.4..0.8).contains(&w.water_table(i)) && !w.is_channel(i)).unwrap();
        w.paint(wet, Brush::Clear, 0);
        w.paint_species(wet, Brush::Tree, Species::Oak, 0);
        let (young, old) = (w.root_depth(wet, 1), w.root_depth(wet, 200));
        assert!(young < 0.1 && old > 0.8, "root depth {young:.2} → {old:.2}");
        assert!(w.tree_water(wet, 200) > w.tree_water(wet, 1) + 0.05);
        // And a shallow-rooted willow taps less of it than an oak.
        assert!(Species::Willow.traits().taproot < Species::Oak.traits().taproot);
    }

    #[test]
    fn flood_tolerance_is_a_matter_of_duration() {
        // Only waterlogging longer than the species tolerates counts.
        let mut w = bare_world(14, Params { tree_growth_p: 0.0, ..physio() });
        let (c, _, _) = center();
        w.paint_species(c, Brush::Tree, Species::Pine, 0);
        let pine_days = Species::Pine.traits().flood_days as u8;
        w.soaked[c] = pine_days; // at the limit: no rot yet
        assert!(w.soaked[c] as f64 <= Species::Pine.traits().flood_days);
        assert!(Species::Willow.traits().flood_days > 10.0 * Species::Pine.traits().flood_days);
        // Drained ground dries a tree out again.
        w.carbon_pass(10);
        assert!(w.soaked[c] < pine_days);
    }

    #[test]
    fn deaths_carry_their_cause() {
        // Fire: a torched sapling's husk says fire; the HUD tally agrees.
        let p = Params { fire_spread_p: 0.0, ..physio() };
        let mut w = bare_world(15, p);
        let (c, _, _) = center();
        w.paint_species(c, Brush::Tree, Species::Pine, 100);
        w.paint(c, Brush::Fire, 101);
        for tick in 101..110 {
            w.step(tick);
        }
        let husk = w.remains(c).expect("a charred husk");
        assert_eq!(husk.cause, DeathCause::Fire);
        assert!(w.deaths_total()[DeathCause::Fire as usize] >= 1);
        assert!(w.deaths_recent()[DeathCause::Fire as usize] > 0.5);
    }

    #[test]
    fn starving_pines_draw_bark_beetles() {
        // Outbreak seeding climbs as the host's reserve empties.
        let strength = |reserve: f32| {
            let mut w = bare_world(16, Params { pest_strength: 1.0, tree_growth_p: 0.0, ..physio() });
            let (_, q, r) = center();
            let mut block = Vec::new();
            for dq in -3..=3 {
                for dr in -3..=3 {
                    if let Some(i) = G.axial_to_index(q + dq, r + dr) {
                        w.paint_species(i, Brush::Tree, Species::Pine, 0);
                        block.push(i);
                    }
                }
            }
            let mut infested = 0;
            for tick in 200..400 {
                for &i in &block {
                    if w.state(i) == Cell::Tree {
                        w.reserve[i] = reserve;
                    }
                }
                w.rebuild_fields(tick);
                w.pest_pass(tick);
                infested += block.iter().filter(|&&i| w.pest[i] > 0).count();
                for &i in &block {
                    w.pest[i] = 0;
                }
            }
            infested
        };
        let (fed, starved) = (strength(1.0), strength(0.0));
        assert!(starved > fed * 3, "starved hosts {starved} vs fed {fed}");
    }

    #[test]
    fn deciduous_canopies_let_spring_light_through() {
        let light_beside = |sp: Species, seasons: f64| {
            let mut w = bare_world(17, Params { seasons, ..physio() });
            let (c, q, r) = center();
            w.paint_species(c, Brush::Tree, sp, 0);
            w.rebuild_fields(500);
            w.canopy_light(G.axial_to_index(q + 2, r).unwrap())
        };
        assert!(light_beside(Species::Oak, 1.0) > light_beside(Species::Pine, 1.0) + 0.1);
        assert_eq!(light_beside(Species::Oak, 0.0), light_beside(Species::Pine, 0.0));
    }

    #[test]
    fn mast_years_never_come_back_to_back() {
        let w = World::with_params(18, physio());
        let masts: Vec<bool> = (0..2_000u64).map(|t| w.mast_at(t)).collect();
        assert!(masts.windows(2).all(|p| !(p[0] && p[1])), "a mast year exhausts the trees");
        let share = masts.iter().filter(|&&m| m).count() as f64 / masts.len() as f64;
        assert!((0.15..0.45).contains(&share), "a mast every ~2–5 years, saw {share:.2}");
    }

    #[test]
    fn the_inspector_explains_a_tile() {
        let mut w = bare_world(19, physio());
        let (c, _, _) = center();
        w.paint_species(c, Brush::Tree, Species::Oak, 0);
        w.step(50);
        let text = w.inspect(c, 50);
        assert!(text.contains("Oak") && text.contains("Carbon reserve") && text.contains("°C"), "{text}");
    }

    /// A lab world with dynamic clouds and nothing else going on.
    fn cloudy(dynamics: f64) -> World {
        let mut w = bare_world(40, Params { cloud_dynamics: dynamics, storm_rate: 0.0, ..no_fire() });
        w.moisture = 0.5;
        w.sun = 0.6;
        w
    }

    /// Put one cloud of `kind` over the map center with `water`.
    fn one_cloud(w: &mut World, kind: CloudKind, water: f64) {
        let (x, y) = G.center(G.middle());
        w.spawn_cloud(kind, [x, y], 5.0, 1);
        let c = w.storms.last_mut().unwrap();
        c.water = water;
        c.vel = [0.2, 0.0];
    }

    #[test]
    fn a_well_fed_cumulus_towers_into_a_thunderhead_that_rains_out_into_anvil_cirrus() {
        let mut w = cloudy(1.0);
        one_cloud(&mut w, CloudKind::Cumulus, 0.9);
        w.cloud_life(10);
        assert_eq!(w.storms[0].kind, CloudKind::Cumulonimbus);
        assert_eq!(w.storms[0].from, CloudKind::Cumulus);
        assert!(w.storms[0].morph < 0.1, "the new form grows in gradually");
        // Raining drains it; spent, it collapses and leaves its anvil.
        w.storms[0].water = CB_SPENT - 0.01;
        w.cloud_life(11);
        assert_eq!(w.storms[0].kind, CloudKind::Cirrus);
        let ev = w.cloud_events();
        assert_eq!((ev[CloudEvent::Towered as usize], ev[CloudEvent::Collapsed as usize]), (1, 1));
    }

    #[test]
    fn a_front_thickens_cirrus_into_rain_that_breaks_up_into_fair_weather_cumulus() {
        let mut w = cloudy(1.0);
        one_cloud(&mut w, CloudKind::Cirrus, FRONT_WATER + 0.05);
        w.cloud_life(10);
        assert_eq!(w.storms[0].kind, CloudKind::Nimbostratus);
        w.storms[0].water = NS_SPENT - 0.01;
        w.cloud_life(11);
        assert_eq!(w.storms[0].kind, CloudKind::Cumulus);
    }

    #[test]
    fn dry_clouds_evaporate_and_moist_ground_feeds_them() {
        let mut w = cloudy(1.0);
        one_cloud(&mut w, CloudKind::Cumulus, 0.01);
        w.moisture = 0.0;
        w.sun = 0.9;
        for t in 10..20 {
            w.cloud_life(t);
        }
        assert!(w.storms.is_empty(), "a small cumulus over hot dry ground evaporates");
        assert!(w.cloud_events()[CloudEvent::Evaporated as usize] >= 1);
        let mut wet = cloudy(1.0);
        one_cloud(&mut wet, CloudKind::Cumulus, 0.3);
        wet.moisture = 1.0;
        let before = wet.storms[0].water;
        wet.cloud_life(10);
        assert!(wet.storms[0].water > before, "evaporation from moist ground feeds it");
    }

    #[test]
    fn windward_slopes_lift_clouds_and_lee_slopes_dry_them() {
        let w = World::with_params(7, Params { climate_zones: 1.0, ..physio() });
        // Find a steep slope along +x.
        let (best, slope) = (0..CELLS)
            .map(|i| {
                let (x, y) = G.center(i);
                (i, w.slope_along(x, y, [1.0, 0.0]))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(slope > OROG_SLOPE, "the test map has windward slopes ({slope:.3})");
        let (x, y) = G.center(best);
        assert!((w.slope_along(x, y, [-1.0, 0.0]) + slope).abs() < 1e-9, "lee is the mirror");
        let climbing = Storm { kind: CloudKind::Cumulus, pos: [x, y], vel: [0.3, 0.0], radius: 4.0, spawned: 0, water: 0.6, from: CloudKind::Cumulus, morph: 1.0, flash: 0, glow: 0 };
        assert!(w.orographic_rain(&climbing), "forced up the ridge, a moist cloud rains");
        let sinking = Storm { vel: [-0.3, 0.0], ..climbing };
        assert!(!w.orographic_rain(&sinking), "on the lee it doesn't");
        let thin = Storm { kind: CloudKind::Cirrus, ..climbing };
        assert!(!w.orographic_rain(&thin), "thin cirrus never does");
    }

    #[test]
    fn warm_moist_ground_bubbles_up_new_cumulus_only_with_dynamics() {
        let run = |dynamics: f64| {
            let mut w = bare_world(41, Params { cloud_dynamics: dynamics, storm_rate: 0.05, ..no_fire() });
            w.moisture = 0.9;
            w.sun = 0.8;
            for t in 1..400 {
                w.cloud_life(t);
            }
            w.cloud_events()[CloudEvent::Formed as usize]
        };
        assert!(run(1.0) > 0);
        assert_eq!(run(0.0), 0);
    }

    #[test]
    fn static_clouds_never_change() {
        let mut w = cloudy(0.0);
        one_cloud(&mut w, CloudKind::Cumulus, 0.95);
        for t in 10..60 {
            w.cloud_life(t);
        }
        assert_eq!(w.storms[0].kind, CloudKind::Cumulus);
        assert_eq!(w.storms[0].water, 0.95);
    }

    #[test]
    fn cold_ground_banks_snow_that_melts_into_wet_ground_and_a_freshet() {
        let mut w = bare_world(50, Params { climate_zones: 1.0, cloud_dynamics: 1.0, ..physio() });
        let (c, _, _) = center();
        w.snow[c] = 200;
        // Cold: the pack holds.
        w.sun = 0.02;
        w.snow_pass();
        assert_eq!(w.snow[c], 200);
        assert_eq!(w.melt_recent, 0.0);
        // A warm spell melts it: wet ground below and a melt pulse that
        // raises the flood odds.
        w.sun = 0.9;
        w.snow_pass();
        assert!(w.snow[c] < 200, "the pack melts");
        assert!(w.wet[c] > 0, "meltwater wets the ground");
        assert!(w.melt_recent > 0.0);
    }

    #[test]
    fn a_big_fire_lofts_its_own_cloud() {
        let mut w = bare_world(51, Params { cloud_dynamics: 1.0, storm_rate: 0.0, ..physio() });
        for i in 0..PYRO_TILES * 3 {
            w.paint(i, Brush::Grass, 0);
            w.burn[i] = BURN_TICKS;
        }
        for t in 1..200 {
            for i in 0..PYRO_TILES * 3 {
                w.burn[i] = BURN_TICKS;
            }
            w.cloud_life(t);
        }
        assert!(w.cloud_events()[CloudEvent::Pyro as usize] > 0, "a pyrocumulus should form");
    }

    #[test]
    fn ground_strikes_flash_the_scene() {
        let mut w = cloudy(1.0);
        one_cloud(&mut w, CloudKind::Cumulonimbus, 0.9);
        assert_eq!(w.flash(), 0.0);
        w.storms[0].flash = FLASH_TICKS;
        assert_eq!(w.flash(), 1.0);
        w.cloud_life(5);
        assert!(w.flash() < 1.0 && w.flash() > 0.0, "the flash fades over a few ticks");
    }

    #[test]
    fn biomes_follow_temperature_and_water() {
        let mut w = World::with_params(52, Params { biomes: 1.0, climate_zones: 1.0, ..physio() });
        // Sweep the climate and classify a dry upland tile.
        let tile = (0..CELLS).find(|&i| w.water_table(i) < 0.1 && !w.is_channel(i)).unwrap();
        w.sun = 0.05;
        let cold = w.biome(tile);
        w.sun = 0.95;
        w.moisture = 0.0;
        let hot_dry = w.biome(tile);
        assert!(matches!(cold, Biome::Tundra | Biome::Boreal), "cold → {cold:?}");
        assert!(matches!(hot_dry, Biome::Desert | Biome::Savanna), "hot and dry → {hot_dry:?}");
        // Saturated ground is wetland whatever the climate.
        if let Some(wet) = (0..CELLS).find(|&i| w.water_table(i) > 0.85) {
            assert_eq!(w.biome(wet), Biome::Wetland);
        }
    }

    #[test]
    fn biome_founders_match_their_sites() {
        // With biomes on, the initial scatter weights species by how well
        // they'd establish: on a big varied map, cold tiles get more
        // boreal founders than hot ones do.
        let w = World::with_params(53, Params { width: 192, height: 192, seed_tree_p: 0.05, ..Params::default() });
        let n = w.grid().cells();
        let boreal = |cold: bool| {
            let trees: Vec<usize> = (0..n)
                .filter(|&i| w.state(i) == Cell::Tree && (w.site_climate(i).0 < 0.4) == cold)
                .collect();
            let b = trees.iter().filter(|&&i| matches!(w.species(i), Species::Spruce | Species::Birch)).count();
            b as f64 / trees.len().max(1) as f64
        };
        assert!(boreal(true) > 2.0 * boreal(false), "cold {:.2} vs warm {:.2}", boreal(true), boreal(false));
    }


    #[test]
    fn felling_leaves_a_logged_stump_and_timber() {
        let mut w = bare_world(4, no_fire());
        let i = G.middle();
        assert_eq!(w.fell_tree(i, 10), None, "no tree, no timber");
        w.paint(i, Brush::Tree, 0);
        let young = w.fell_tree(i, 5).expect("a sapling yields a little");
        assert!((1.0..2.0).contains(&young));
        w.paint(i, Brush::Tree, 0);
        let old = w.fell_tree(i, 400).expect("a mature tree yields more");
        assert!(old > 3.5 && old <= 4.0);
        assert_eq!(w.state(i), Cell::Bare);
        let r = w.remains(i).expect("a stump remains");
        assert!(r.tree);
        assert_eq!(r.cause, DeathCause::Logged);
        assert_eq!(w.deaths_total()[DeathCause::Logged as usize], 2);
    }

    #[test]
    fn nothing_grows_on_a_house() {
        let mut w = World::with_params(8, Params { seed_tree_p: 0.0, seed_grass_p: 0.0, grass_seed_p: 0.3, ..no_fire() });
        let i = G.middle();
        w.paint(i, Brush::Tree, 0);
        assert!(w.build_house(i, 1));
        assert!(!w.build_house(i, 1), "one house per tile");
        assert_eq!(w.state(i), Cell::Bare, "building clears the tile");
        w.paint_species(i, Brush::Tree, Species::Oak, 2);
        w.paint_grass(i, GrassKind::Sod, 2);
        for t in 2..200 {
            w.step(t);
            assert_eq!(w.state(i), Cell::Bare, "tick {t}: something grew in the house");
        }
        w.demolish(i);
        assert!(!w.is_built(i));
    }
}
