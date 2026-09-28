//! Per-frame instance building: turns the world's cell states into three
//! instance streams (ground tiles, trees, grass). Growth and death animate by
//! shipping the previous tick's scale alongside the current one; the shader
//! lerps them with the clock's alpha, the same prev/current idiom the sibling
//! traffic engine uses for vehicle poses.

use bytemuck::{Pod, Zeroable};

use crate::sim::hex;
use crate::sim::rng::mix64;
use crate::sim::world::{Cell, World, SPECIES_COUNT};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    pub pos: [f32; 3],
    pub scale: f32,
    pub prev_scale: f32,
    pub color: [f32; 3],
}

/// Ticks a plant takes to reach full size.
const GRASS_GROW_TICKS: f64 = 5.0;
const TREE_GROW_TICKS: f64 = 40.0;

/// Drought wilt: vegetation shrinks toward this fraction of full size (and
/// browns) as the climate dries out. Mortality is a hazard now, so deaths
/// aren't individually telegraphed — instead whole dry SEASONS brown the
/// map, which is when most of the dying happens.
const WILT_MIN_SCALE: f64 = 0.62;

fn wilt_mult(wilt: f64) -> f64 {
    1.0 - (1.0 - WILT_MIN_SCALE) * wilt.clamp(0.0, 1.0)
}

fn smoothstep(x: f64) -> f64 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Fraction of mature size a plant shows the instant it exists — so a
/// just-painted (or just-sprouted) plant gives immediate visual feedback
/// instead of being invisible at age 0.
const SPROUT_SCALE: f64 = 0.15;

fn grow_scale(age: u64, grow_ticks: f64, mature: f64) -> f32 {
    (mature * smoothstep(age as f64 / grow_ticks).max(SPROUT_SCALE)) as f32
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Deterministic per-cell unit float for visual variation (independent of the
/// world seed so reseeding keeps the same ground texture).
fn cell_noise(index: usize, salt: u64) -> f32 {
    ((mix64(index as u64 ^ salt.wrapping_mul(0x9E37_79B9)) >> 40) as f32) / (1u64 << 24) as f32
}

const SOIL: [f32; 3] = [0.43, 0.34, 0.24];
const SOIL_UNDER_PLANT: [f32; 3] = [0.36, 0.30, 0.20];
const EMBER: [f32; 3] = [0.95, 0.38, 0.10];
const SCORCH: [f32; 3] = [0.16, 0.12, 0.10];
const ASH: [f32; 3] = [0.33, 0.32, 0.30];
/// Rich decomposed soil; the ground lerps toward this with fertility.
const LOAM: [f32; 3] = [0.24, 0.17, 0.10];
/// Standing-dead colors: grey snags, straw thatch, charcoal for burn kills.
const SNAG: [f32; 3] = [0.44, 0.40, 0.33];
const STRAW: [f32; 3] = [0.61, 0.54, 0.33];
const CHARRED: [f32; 3] = [0.15, 0.13, 0.11];
/// Wilt targets that senescing plants brown toward.
const GRASS_WILT: [f32; 3] = [0.44, 0.37, 0.21];
const TREE_WILT: [f32; 3] = [0.47, 0.44, 0.30];
const GRASS_YOUNG: [f32; 3] = [0.26, 0.44, 0.18];
const GRASS_OLD: [f32; 3] = [0.50, 0.45, 0.21];
/// Young canopy color per species: acacia, oak, pine, willow.
const CANOPY_YOUNG: [[f32; 3]; SPECIES_COUNT] = [
    [0.40, 0.53, 0.21],
    [0.13, 0.38, 0.16],
    [0.10, 0.33, 0.24],
    [0.47, 0.62, 0.36],
];
const CANOPY_OLD: [f32; 3] = [0.42, 0.44, 0.16];

const MUSHROOM: [f32; 3] = [0.78, 0.52, 0.34];
const MUSHROOM_ON_CHAR: [f32; 3] = [0.62, 0.58, 0.48];
const CLOUD: [f32; 3] = [0.33, 0.34, 0.40];
const BOLT_WHITE: [f32; 3] = [1.0, 1.0, 0.9];
/// Cloud altitude above the ground plane.
const CLOUD_ALTITUDE: f32 = 8.5;
/// The cloud mesh is ~this many world units in radius at scale 1.
const CLOUD_MESH_RADIUS: f64 = 8.0;

pub struct FrameInstances {
    pub ground: Vec<Instance>,
    /// One stream per species (each has its own silhouette mesh).
    pub trees: [Vec<Instance>; SPECIES_COUNT],
    pub grass: Vec<Instance>,
    pub mushrooms: Vec<Instance>,
    pub clouds: Vec<Instance>,
    pub bolts: Vec<Instance>,
}

/// `alpha` is the clock's sub-tick blend factor: storms drift on exact
/// linear paths, so clouds (and their shadows) interpolate smoothly between
/// ticks without any stored previous position.
pub fn build_instances(world: &World, tick: u64, alpha: f32) -> FrameInstances {
    let mut out = FrameInstances {
        ground: Vec::with_capacity(hex::CELLS),
        trees: Default::default(),
        grass: Vec::new(),
        mushrooms: Vec::new(),
        clouds: Vec::new(),
        bolts: Vec::new(),
    };
    let t = tick as f64 + alpha as f64;
    let storms: Vec<([f64; 2], f64)> =
        world.storms().iter().map(|s| (s.center(t), s.radius)).collect();
    for (i, storm) in world.storms().iter().enumerate() {
        let c = storm.center(t);
        let age = t - storm.spawned as f64;
        let ramp = (age / 10.0).clamp(0.0, 1.0);
        let s = (storm.radius / CLOUD_MESH_RADIUS * ramp) as f32;
        let jitter = 0.9 + 0.2 * cell_noise(i * 37 + 5, 6);
        out.clouds.push(Instance {
            pos: [c[0] as f32, c[1] as f32, CLOUD_ALTITUDE],
            scale: s,
            prev_scale: s,
            color: [CLOUD[0] * jitter, CLOUD[1] * jitter, CLOUD[2] * jitter],
        });
    }
    for i in 0..hex::CELLS {
        let (q, r) = hex::index_to_axial(i);
        let (x, y) = hex::axial_to_world(q, r);
        let pos = [x as f32, y as f32, 0.0];
        let state = world.state(i);

        let burning = world.burning(i);
        let shade = 0.92 + 0.16 * cell_noise(i, 1);
        let soil = if state == Cell::Bare { SOIL } else { SOIL_UNDER_PLANT };
        // Fertility reads as the soil deepening toward dark loam.
        let fert = world.nutrient_ratio(i);
        let base = lerp3(
            [soil[0] * shade, soil[1] * shade, soil[2] * shade],
            LOAM,
            fert * 0.85,
        );
        let ash = world.ash_ratio(i);
        let mut ground = if burning {
            EMBER
        } else if ash > 0.0 {
            // Fresh ash reads grey, then washes out to reveal the soil —
            // darker than before the burn, thanks to the ash flush.
            lerp3(base, ASH, ash)
        } else {
            base
        };
        // Rain-slick ground reads darker and cooler.
        let wet = world.wet_ratio(i);
        if wet > 0.0 && !burning {
            let w = 0.5 + 0.5 * wet;
            ground = [
                ground[0] * (1.0 - 0.16 * w),
                ground[1] * (1.0 - 0.13 * w),
                ground[2] * (1.0 - 0.02 * w) + 0.015 * w,
            ];
        }
        // Cloud shadow, interpolated with the drift.
        for &(c, radius) in &storms {
            let d = ((x - c[0]).powi(2) + (y - c[1]).powi(2)).sqrt();
            if d < radius {
                let edge = ((d / radius - 0.75) / 0.25).clamp(0.0, 1.0) as f32;
                let f = 0.68 + 0.32 * edge;
                ground = [ground[0] * f, ground[1] * f, ground[2] * f];
            }
        }
        out.ground.push(Instance { pos, scale: 1.0, prev_scale: 1.0, color: ground });
        if world.bolt_active(i) {
            out.bolts.push(Instance {
                pos,
                scale: 1.0,
                prev_scale: 1.0,
                color: BOLT_WHITE,
            });
        }

        // Standing dead: a husk shrinks from the exact size the plant died
        // at (growth curve + wilt, same per-cell noise) down to nothing, so
        // the prev/current lerp carries it out smoothly.
        if let Some(r) = world.remains(i) {
            let (grow_ticks, mature_base, mature_span, salt) = if r.tree {
                (TREE_GROW_TICKS * r.species.traits().maturity, 0.8, 0.4, 2)
            } else {
                (GRASS_GROW_TICKS, 0.7, 0.5, 3)
            };
            let mature = mature_base + mature_span * cell_noise(i, salt) as f64;
            let died_scale = grow_scale(r.age_at_death as u64, grow_ticks, mature) as f64
                * wilt_mult(r.life_ratio as f64);
            let inst = Instance {
                pos,
                scale: (died_scale * r.remaining as f64) as f32,
                prev_scale: (died_scale * r.remaining_prev as f64) as f32,
                color: if r.charred { CHARRED } else if r.tree { SNAG } else { STRAW },
            };
            if r.tree {
                out.trees[r.species as usize].push(inst);
            } else {
                out.grass.push(inst);
            }
            // Fruiting bodies once the mycelium is established; they grow in
            // with colonization and only sink away in the husk's last stretch
            // (isolated wood rots out before full colonization, so the ramp
            // starts low or the fruit would never be visible).
            let fruit = ((r.myc as f64 - 0.25) / 0.3).clamp(0.0, 1.0);
            if fruit > 0.0 {
                let s = (fruit
                    * (r.remaining as f64 * 1.6).min(1.0)
                    * (0.8 + 0.4 * cell_noise(i, 4) as f64)) as f32;
                out.mushrooms.push(Instance {
                    pos,
                    scale: s,
                    prev_scale: s,
                    color: if r.charred { MUSHROOM_ON_CHAR } else { MUSHROOM },
                });
            }
        }

        if state == Cell::Bare {
            continue;
        }
        let age = world.age(i, tick);
        let params = world.params();
        // Age tint runs against the MEAN lifetime (lifetimes themselves are
        // open-ended); drought browning wilts everything in dry seasons.
        let brown = world.browning(i) as f64;
        match state {
            Cell::Tree => {
                let sp = world.species(i);
                let tr = sp.traits();
                let mature = 0.8 + 0.4 * cell_noise(i, 2) as f64;
                let mean_life = params.tree_mean_life.max(1) as f64 * tr.mean_life;
                let tint = (age as f64 / mean_life).min(1.0) as f32;
                let wilt = brown * 0.6 * tr.drought_sensitivity.min(1.5); // willows visibly suffer
                let grow_ticks = TREE_GROW_TICKS * tr.maturity;
                let scaled = |a: u64| {
                    (grow_scale(a, grow_ticks, mature) as f64 * wilt_mult(wilt)) as f32
                };
                out.trees[sp as usize].push(Instance {
                    pos,
                    scale: scaled(age),
                    prev_scale: scaled(age.saturating_sub(1)),
                    color: if burning {
                        SCORCH
                    } else {
                        lerp3(
                            lerp3(CANOPY_YOUNG[sp as usize], CANOPY_OLD, tint),
                            TREE_WILT,
                            wilt as f32,
                        )
                    },
                });
            }
            Cell::Grass => {
                let mature = 0.7 + 0.5 * cell_noise(i, 3) as f64;
                let tint = (age as f64 / params.grass_mean_life.max(1) as f64).min(1.0) as f32;
                let scaled = |a: u64| {
                    (grow_scale(a, GRASS_GROW_TICKS, mature) as f64 * wilt_mult(brown)) as f32
                };
                out.grass.push(Instance {
                    pos,
                    scale: scaled(age),
                    prev_scale: scaled(age.saturating_sub(1)),
                    color: if burning {
                        SCORCH
                    } else {
                        lerp3(lerp3(GRASS_YOUNG, GRASS_OLD, tint), GRASS_WILT, brown as f32)
                    },
                });
            }
            Cell::Bare => unreachable!(),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_layout_matches_the_shader_stride() {
        assert_eq!(std::mem::size_of::<Instance>(), 32);
        assert_eq!(std::mem::offset_of!(Instance, pos), 0);
        assert_eq!(std::mem::offset_of!(Instance, scale), 12);
        assert_eq!(std::mem::offset_of!(Instance, prev_scale), 16);
        assert_eq!(std::mem::offset_of!(Instance, color), 20);
    }

    #[test]
    fn instance_counts_cover_live_plants_plus_husks() {
        use crate::sim::world::Params;
        let seeded = || {
            World::with_params(
                5,
                Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() },
            )
        };
        // A fresh world has no husks: streams match the live counts exactly.
        let w = seeded();
        let [_, grass, trees] = w.counts();
        let f = build_instances(&w, 0, 0.0);
        assert_eq!(f.ground.len(), hex::CELLS);
        let tree_total: usize = f.trees.iter().map(|v| v.len()).sum();
        assert_eq!(tree_total, trees as usize);
        assert_eq!(f.grass.len(), grass as usize);

        // After some churn, standing dead pad the streams — never shrink them.
        let mut w = seeded();
        for tick in 1..=80 {
            w.step(tick);
        }
        let [_, grass, trees] = w.counts();
        let f = build_instances(&w, 80, 0.0);
        let tree_total: usize = f.trees.iter().map(|v| v.len()).sum();
        assert!(tree_total >= trees as usize);
        assert!(f.grass.len() > grass as usize, "80 ticks of grass churn should leave husks");
    }

    /// A world where nothing grows or ignites on its own.
    fn inert_world(seed: u64) -> World {
        use crate::sim::world::{Brush, Params};
        let mut w = World::with_params(
            seed,
            Params {
                grass_seed_p: 0.0,
                grass_clonal_p: 0.0,
                tree_growth_p: 0.0,
                fire_ignition_p: 0.0,
                ..Params::default()
            },
        );
        for i in 0..hex::CELLS {
            w.paint(i, Brush::Clear, 0);
        }
        w
    }

    #[test]
    fn deaths_leave_shrinking_husks_instead_of_popping() {
        use crate::sim::world::Brush;
        let mut w = inert_world(9);
        w.paint(0, Brush::Grass, 1000);
        // Step until the hazard takes it, tracking the last living scale.
        let mut live_scale = 0.0f32;
        let mut died_at = 0u64;
        for t in 1001..=4000u64 {
            let f = build_instances(&w, t - 1, 0.0);
            if let Some(g) = f.grass.first() {
                live_scale = g.scale;
            }
            w.step(t);
            if w.counts()[1] == 0 {
                died_at = t;
                break;
            }
        }
        assert!(died_at > 0, "the grass should die within a few mean-lives");
        w.step(died_at + 1);
        let f = build_instances(&w, died_at + 1, 0.0);
        assert_eq!(f.grass.len(), 1, "a husk should remain");
        let husk = f.grass[0];
        assert_eq!(husk.color, STRAW);
        assert!(husk.scale > 0.0 && husk.scale <= live_scale + 1e-6);
        assert!(husk.prev_scale >= husk.scale, "husks shrink, never grow");
        // And it rots away completely once the mycelium finishes.
        for t in (died_at + 2)..(died_at + 200) {
            w.step(t);
        }
        assert!(
            build_instances(&w, died_at + 200, 0.0).grass.is_empty(),
            "husk should have rotted away"
        );
    }

    #[test]
    fn mushrooms_fruit_on_colonized_wood_and_not_before() {
        use crate::sim::world::{Brush, Params};
        let mut w = inert_world(15);
        w.set_params(Params { tree_mean_life: 1, ..w.params() }); // dies at once
        w.paint(0, Brush::Tree, 0);
        w.step(1000); // dies → fresh, uncolonized husk
        assert!(build_instances(&w, 1000, 0.0).mushrooms.is_empty(), "fresh dead wood has no fruit");
        let mut fruited = false;
        for t in 1001u64..1100 {
            w.step(t);
            let f = build_instances(&w, t, 0.0);
            if !f.mushrooms.is_empty() {
                assert!(w.remains(0).map(|r| r.myc).unwrap_or(0.0) >= 0.25);
                fruited = true;
                break;
            }
        }
        assert!(fruited, "colonization should eventually fruit mushrooms");
    }

    #[test]
    fn burns_leave_charred_husks_and_ash_that_fades() {
        use crate::sim::world::Brush;
        let mut w = inert_world(11);
        w.paint(0, Brush::Grass, 100);
        w.paint(0, Brush::Fire, 100);
        w.step(100);
        w.step(101); // burn-out: charred husk + fresh ash
        let f = build_instances(&w, 101, 0.0);
        assert_eq!(f.grass.len(), 1);
        assert_eq!(f.grass[0].color, CHARRED);
        let fresh_ratio = w.ash_ratio(0);
        assert!(fresh_ratio > 0.5, "ash should be fresh right after the burn");
        for t in 102..140 {
            w.step(t);
        }
        assert!(
            w.ash_ratio(0) < fresh_ratio - 0.3,
            "ash should fade over time ({} → {})",
            fresh_ratio,
            w.ash_ratio(0)
        );
    }

    #[test]
    fn fertile_ground_renders_darker() {
        use crate::sim::world::{Brush, Params};
        let mut w = inert_world(17);
        w.set_params(Params { tree_mean_life: 1, ..w.params() }); // dies at once
        w.paint(0, Brush::Tree, 0); // doomed: rots into a nutrient pulse
        for t in 1000u64..1300 {
            w.step(t);
            if w.remains(0).is_none() {
                break;
            }
        }
        assert!(w.nutrient_ratio(0) > 0.4, "rot should have fertilized tile 0");
        let f = build_instances(&w, 1300, 0.0);
        let (rich, plain) = (f.ground[0].color, f.ground[10].color);
        assert!(
            rich[0] < plain[0] && rich[1] < plain[1] && rich[2] < plain[2],
            "fertile soil should read darker: {rich:?} vs {plain:?}"
        );
    }

    #[test]
    fn storms_render_as_drifting_clouds_with_shadows_and_bolts() {
        use crate::sim::world::{Brush, Params};
        let mut w = inert_world(21);
        let f = build_instances(&w, 0, 0.0);
        assert!(f.clouds.is_empty() && f.bolts.is_empty(), "calm world, clear sky");

        w.spawn_storm([50.0, 50.0], [0.3, 0.0], 8.0, 100);
        let a = build_instances(&w, 200, 0.0); // well past the spawn ramp
        assert_eq!(a.clouds.len(), 1);
        let b = build_instances(&w, 200, 0.5);
        assert!(
            b.clouds[0].pos[0] > a.clouds[0].pos[0],
            "the cloud should drift smoothly with alpha"
        );
        assert!(a.clouds[0].pos[2] > 5.0, "clouds float above the canopy");

        // The ground under the cloud is shadowed relative to far ground.
        let under = hex::pick(50.0 + 30.0 * 0.3, 50.0).unwrap_or(0);
        let _ = under;
        // (Shadow is easiest to assert via a directly-covered tile.)
        let covered = hex::pick(80.0, 50.0); // center at t=200: x = 50+0.3*100 = 80
        if let Some(ci) = covered {
            let far = build_instances(&w, 200, 0.0).ground[0].color;
            let shadowed = build_instances(&w, 200, 0.0).ground[ci].color;
            assert!(shadowed[0] < far[0], "covered soil should be darker than open soil");
        }

        // A lightning strike renders a bolt on its tile.
        let p = Params { storm_lightning_p: 1.0, ..w.params() };
        w.set_params(p);
        w.paint(hex::pick(80.0, 50.0).unwrap(), Brush::Grass, 200);
        w.step(200);
        let f = build_instances(&w, 200, 0.0);
        assert!(!f.bolts.is_empty(), "certain lightning should render a bolt");
    }

    #[test]
    fn drought_browns_and_shrinks_the_vegetation() {
        use crate::sim::world::{Brush, Params};
        // Full climate swings; find a wet tick and a drought tick, then
        // compare a fresh mature plant's look under each.
        let mk = || {
            let mut w = World::with_params(
                13,
                Params {
                    climate_swing: 1.0,
                    storm_rate: 0.0,
                    fire_ignition_p: 0.0,
                    grass_seed_p: 0.0,
                    grass_clonal_p: 0.0,
                    tree_growth_p: 0.0,
                    grass_mean_life: 100_000,
                    ..Params::default()
                },
            );
            for i in 0..hex::CELLS {
                w.paint(i, Brush::Clear, 0);
            }
            w
        };
        let probe = mk();
        let mut wet_tick = 0u64;
        let mut dry_tick = 0u64;
        for t in 50..3000u64 {
            let (sun, m) = probe.climate_at(t);
            if wet_tick == 0 && m > 0.85 {
                wet_tick = t;
            }
            if dry_tick == 0 && m < 0.15 && sun > 0.6 {
                dry_tick = t;
            }
        }
        assert!(wet_tick > 0 && dry_tick > 0, "full swings must reach both extremes");
        let sample = |target: u64| {
            let mut w = mk();
            w.paint(0, Brush::Grass, target.saturating_sub(30)); // mature by target
            for t in (target - 20)..=target {
                w.step(t);
            }
            if w.state(0) != Cell::Grass {
                w.paint(0, Brush::Grass, target - 30);
            }
            build_instances(&w, target, 0.0).grass[0]
        };
        let lush = sample(wet_tick);
        let parched = sample(dry_tick);
        assert!(
            parched.color[1] < lush.color[1] - 0.02,
            "drought should brown the sward (green {:.2} vs {:.2})",
            parched.color[1],
            lush.color[1]
        );
        assert!(parched.scale < lush.scale, "drought should visibly wilt it");
    }

    #[test]
    fn newborn_plants_start_small_and_grow() {
        use crate::sim::world::Brush;
        let mut w = World::new(6);
        for i in 0..hex::CELLS {
            w.paint(i, Brush::Clear, 0);
        }
        w.paint(0, Brush::Tree, 100);
        let f = build_instances(&w, 100, 0.0);
        assert_eq!(f.trees[0].len(), 1);
        let sprout = f.trees[0][0].scale;
        assert!(sprout > 0.0, "age 0 tree should be visible as a sprout");
        assert!(sprout < 0.3, "but still clearly smaller than mature");
        let f = build_instances(&w, 120, 0.0);
        assert!(f.trees[0][0].scale > 0.2);
        assert!(f.trees[0][0].prev_scale < f.trees[0][0].scale, "still growing at age 20");
        let f = build_instances(&w, 100 + TREE_GROW_TICKS as u64 + 5, 0.0);
        assert!((f.trees[0][0].scale - f.trees[0][0].prev_scale).abs() < 1e-6, "mature tree stops growing");
    }

    #[test]
    fn burning_tiles_render_as_embers_and_scorch() {
        use crate::sim::world::Brush;
        let mut w = World::new(12);
        for i in 0..hex::CELLS {
            w.paint(i, Brush::Clear, 0);
        }
        w.paint(0, Brush::Grass, 100);
        w.paint(0, Brush::Fire, 100);
        assert!(w.burning(0));
        let f = build_instances(&w, 100, 0.0);
        assert_eq!(f.ground[0].color, EMBER);
        assert_eq!(f.grass[0].color, SCORCH);
        let calm = f.ground[1].color;
        assert_ne!(calm, EMBER, "unburnt neighbors keep soil colors");
    }

    #[test]
    fn colors_shift_with_age() {
        use crate::sim::world::Brush;
        let mut w = World::new(8);
        for i in 0..hex::CELLS {
            w.paint(i, Brush::Clear, 0);
        }
        w.paint(0, Brush::Grass, 0);
        let young = build_instances(&w, 1, 0.0).grass[0].color;
        // Twice the mean lifetime: fully age-tinted (if it were still alive).
        let old = build_instances(&w, 60, 0.0).grass[0].color;
        assert!(old[0] > young[0], "grass should yellow (red channel rises) with age");
    }
}
