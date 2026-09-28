//! Whole-world integration tests: parameter constraints exercised through
//! the public API, and long-run regime bands for the configurations the
//! `equilibrium` example probe measured (re-derive with
//! `cargo run --release --example equilibrium`). Bands are deliberately wide
//! around the observed min/max so they assert the regime — coexistence,
//! closed forest, fire-swept grassland, bistability — not the trajectory.

use tree_engine::sim::hex::{self, Grid};
use tree_engine::sim::world::{Brush, Cell, GrassKind, Params, Species, World, SPECIES_COUNT};

const G: Grid = Grid::LEGACY;
const CELLS: usize = 4096;

/// Defaults on the 64×64 map the regime bands were measured on.
fn legacy() -> Params {
    Params::legacy_map()
}

fn run_and_sample(
    seed: u64,
    params: Params,
    warmup: u64,
    horizon: u64,
    every: u64,
) -> (Vec<u32>, Vec<u32>) {
    let mut w = World::with_params(seed, params);
    let mut grass = Vec::new();
    let mut trees = Vec::new();
    for tick in 1..=horizon {
        w.step(tick);
        if tick > warmup && tick % every == 0 {
            let c = w.counts();
            grass.push(c[1]);
            trees.push(c[2]);
        }
    }
    (grass, trees)
}

fn assert_band(samples: &[u32], lo: u32, hi: u32, what: &str) {
    assert!(!samples.is_empty());
    for (i, &v) in samples.iter().enumerate() {
        assert!(
            (lo..=hi).contains(&v),
            "{what} sample {i} = {v}, outside the expected band [{lo}, {hi}]"
        );
    }
}

/// Defaults start the world empty (the player plants everything); regime
/// tests simulate "the user painted 2% trees / 10% grass" via the one-shot
/// seeding fields.
fn planted(p: Params) -> Params {
    Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..p }
}

fn bare_world(seed: u64, params: Params) -> World {
    let mut w = World::with_params(seed, params);
    for i in 0..CELLS {
        w.paint(i, Brush::Clear, 0);
    }
    w
}

// ---- regimes (bands from the equilibrium example probe) ----

#[test]
fn default_savanna_holds_grass_and_trees_in_coexistence() {
    // On the default relief with grass niches, measured over 4 seeds, ticks
    // 3000–8000: grass ~970–1520 [272..2016] breathing with the seasons,
    // trees ~550–900 [440..1433].
    for seed in [7u64, 42] {
        let (grass, trees) = run_and_sample(seed, planted(legacy()), 3_000, 8_000, 100);
        assert_band(&grass, 150, 2_400, "grass");
        assert!(grass.iter().any(|&g| g > 600), "grass must recover between droughts");
        assert_band(&trees, 350, 1_600, "trees");
    }
}

#[test]
fn faster_trees_without_fire_close_into_moist_forest() {
    // tree_growth_p 1%, no lightning: a mixed forest — oak can't replace
    // itself under its own canopy, oak-wilt outbreaks open gaps, pioneers
    // refill them. Measured on seeds 42/7/9, ticks 3000–8000: trees
    // ~1180–1310 [948..1939], grass ~270–550 [66..1293] in the gaps.
    let p = planted(Params { tree_growth_p: 0.01, ..legacy() });
    let (grass, trees) = run_and_sample(42, p, 3_000, 8_000, 100);
    assert_band(&trees, 800, 2_300, "trees");
    assert_band(&grass, 0, 1_400, "grass");
}

#[test]
fn frequent_lightning_burns_the_world_down_to_grassland() {
    // Slow trees (0.1%) + ignition 5e-4: burns kill every sapling before
    // maturity — trees extinct on all 5 probed seeds; the sward surges in
    // wet seasons and burns back in droughts (grass ~875 [259..2152]).
    let p = planted(Params { tree_growth_p: 0.001, fire_ignition_p: 0.0005, ..legacy() });
    let (grass, trees) = run_and_sample(1234, p, 3_000, 8_000, 100);
    assert_band(&trees, 0, 20, "trees");
    assert_band(&grass, 150, 2_300, "grass");
}

/// Standing trees per species at the end of a run.
fn composition(seed: u64, params: Params, horizon: u64) -> [u32; SPECIES_COUNT] {
    let mut w = World::with_params(seed, params);
    for tick in 1..=horizon {
        w.step(tick);
    }
    let mut c = [0u32; SPECIES_COUNT];
    for i in 0..CELLS {
        if w.state(i) == Cell::Tree {
            c[w.species(i) as usize] += 1;
        }
    }
    c
}

#[test]
fn strong_lightning_leaves_willow_refuges_in_the_valleys() {
    // The savanna config + heavy lightning. The fires sweep the dry
    // uplands every few decades, but the wet valley bottoms (catena) don't
    // carry fire and willow resprouts from its roots: every probed seed
    // ends as open fire-grassland with riparian willow stands. Measured,
    // ticks 4000–10000 on seeds 7/9/42/77: trees ~270–400 [234..567],
    // grass ~950–1180 [516..1829]; willows 50–100% of the survivors.
    let p = planted(Params { fire_ignition_p: 0.0005, ..legacy() });
    for seed in [7u64, 77] {
        let (grass, trees) = run_and_sample(seed, p, 4_000, 10_000, 100);
        assert_band(&trees, 150, 650, "trees");
        assert_band(&grass, 350, 2_300, "grass");
        let c = composition(seed, p, 10_000);
        let total: u32 = c.iter().sum();
        assert!(c[Species::Willow as usize] * 10 > total * 8,
            "seed {seed}: the survivors should be valley willows, saw {c:?}");
    }
}

#[test]
fn the_fire_trap_selects_against_late_maturity() {
    // Under the same strong fire regime and seed, upland trees that stay
    // flammable until age 120 never escape the burn cycle (only the valley
    // willow refuge survives, ~330), while age-40 trees close into a mixed
    // forest (~960 [718..1324], oak-led).
    let fire = planted(Params { tree_growth_p: 0.005, fire_ignition_p: 0.0005, ..legacy() });
    let late = composition(77, Params { tree_maturity_age: 120, ..fire }, 8_000);
    let early = composition(77, fire, 8_000);
    let uplanders = |c: &[u32; SPECIES_COUNT]| c.iter().sum::<u32>() - c[Species::Willow as usize];
    assert!(uplanders(&late) <= 10, "late-maturity uplanders should be trapped, saw {late:?}");
    assert!(uplanders(&early) >= 400, "early-maturity uplanders should escape, saw {early:?}");
    let (_, trees) = run_and_sample(77, fire, 4_000, 8_000, 100);
    assert_band(&trees, 600, 1_800, "early-maturity trees");
}

#[test]
fn treeless_grassland_matches_the_birth_death_balance() {
    // Clonal creep off so the analytic steady state applies. The tile
    // cycle is now life (E=30.5) + husk decomposition (~27 for isolated
    // thatch) + sprout wait (1/p = 200), so
    //   g ≈ cells · 30.5 / 257.5 ≈ 485.
    let p = Params {
        seed_tree_p: 0.0,
        seed_grass_p: 0.10,
        tree_growth_p: 0.0,
        grass_seed_p: 0.005,
        grass_clonal_p: 0.0,
        climate_swing: 0.0, // neutral climate: all multipliers exactly 1
        storm_rate: 0.0,
        water_table: 0.0, // no groundwater bonus
        mutation_rate: 0.0,
        ..legacy()
    };
    let (grass, trees) = run_and_sample(7, p, 2_000, 4_000, 100);
    assert_band(&trees, 0, 0, "trees");
    assert_band(&grass, 380, 600, "grass");
}

#[test]
fn succession_leads_to_oak_but_never_to_an_oak_monoculture() {
    // From an even planting in the fire-free forest regime: fast pioneers
    // dominate the young stand; over the long run the shade-tolerant oak
    // becomes the commonest tree, but its seedlings fail beneath their own
    // parents and oak wilt sweeps its dense stands, so the pioneers keep a
    // real share. Measured (seed 42): t=1000 ~860 pioneers; t=12000
    // ~1420 oaks with ~470 pioneers (an outbreak then cuts oak back).
    let p = planted(Params { tree_growth_p: 0.01, ..legacy() });
    let mut w = World::with_params(42, p);
    let comp = |w: &World| -> [u32; SPECIES_COUNT] {
        let mut c = [0u32; SPECIES_COUNT];
        for i in 0..CELLS {
            if w.state(i) == Cell::Tree {
                c[w.species(i) as usize] += 1;
            }
        }
        c
    };
    for tick in 1..=1_000u64 {
        w.step(tick);
    }
    let early = comp(&w);
    let early_pioneers = early[0] + early[2] + early[3];
    assert!(early_pioneers > 200, "the young stand should be full of pioneers, saw {early_pioneers}");
    for tick in 1_001..=12_000u64 {
        w.step(tick);
    }
    let late = comp(&w);
    let oak = late[Species::Oak as usize];
    let pioneers = late[0] + late[2] + late[3];
    assert!(late.iter().all(|&c| c <= oak), "oak should lead the late forest, saw {late:?}");
    assert!(pioneers * 5 > oak, "pioneers must keep a real share, saw {late:?}");
}

#[test]
fn neighbor_competition_keeps_the_forest_mixed() {
    // The same forest with pests, browsing, and neighbor competition all
    // off collapses toward an oak monoculture (~95% oak, ~1.5 effective
    // types); with them on it stays mixed (~4.4 types, no species > 50%).
    let forest = planted(Params { tree_growth_p: 0.01, ..legacy() });
    let eff_at = |p: Params| {
        let mut w = World::with_params(42, p);
        for tick in 1..=8_000u64 {
            w.step(tick);
        }
        let mut c = [0u32; SPECIES_COUNT];
        for i in 0..CELLS {
            if w.state(i) == Cell::Tree {
                c[w.species(i) as usize] += 1;
            }
        }
        (w.diversity().1, c)
    };
    let (mixed, c) = eff_at(forest);
    let (mono, m) = eff_at(Params { pest_strength: 0.0, browse: 0.0, competition: 0.0, ..forest });
    let total: u32 = c.iter().sum();
    assert!(c.iter().all(|&n| n * 10 < total * 6), "no species should hold 60% of the forest, saw {c:?}");
    assert!(mixed > mono + 1.0, "competition should add diversity ({mono:.2} {m:?} → {mixed:.2} {c:?})");
}

#[test]
fn closed_forests_grow_tall_and_thin_while_savanna_trees_spread_wide() {
    // The canopy race as an emergent population statistic: mean etiolation
    // of the standing trees is far higher in the dense moist forest than in
    // the open savanna.
    let mean_etiol = |p: Params| -> f64 {
        let mut w = World::with_params(42, planted(p));
        for tick in 1..=6_000u64 {
            w.step(tick);
        }
        let (mut sum, mut n) = (0.0f64, 0u32);
        for i in 0..CELLS {
            if w.state(i) == Cell::Tree {
                sum += w.etiolation(i) as f64;
                n += 1;
            }
        }
        sum / n.max(1) as f64
    };
    let forest = mean_etiol(Params { tree_growth_p: 0.01, ..legacy() });
    let savanna = mean_etiol(legacy());
    // Measured (seed 42): forest ~0.34, savanna ~0.10 (the mixed forest
    // has gaps, so fewer trees are fully drawn up than in a closed canopy).
    assert!(forest > 0.25, "forest trees should be markedly drawn-up, mean {forest:.2}");
    assert!(
        forest > savanna * 2.0,
        "forest trees must be much slimmer than savanna trees ({forest:.2} vs {savanna:.2})"
    );
}

// ---- parameter constraints through the public API ----

#[test]
fn larger_seed_rain_range_colonizes_faster() {
    // Shade off so range-1 dispersal isn't extinguished by the light gate,
    // grass off so only trees move.
    let time_to_150 = |range: i32| -> u64 {
        let p = Params {
            tree_range: range,
            tree_growth_p: 0.02,
            shade_strength: 0.0,
            grass_seed_p: 0.0,
            grass_clonal_p: 0.0,
            seed_grass_p: 0.0,
            climate_swing: 0.0,
            storm_rate: 0.0,
            tree_mean_life: 100_000, // isolate colonization from mortality
            ..legacy()
        };
        let mut w = bare_world(5, p);
        let center = CELLS / 2 + G.width as usize / 2;
        for tick in 1u64..=40_000 {
            if w.state(center) != Cell::Tree {
                w.paint(center, Brush::Tree, tick.saturating_sub(100));
            }
            w.step(tick);
            if w.counts()[2] >= 150 {
                return tick;
            }
        }
        panic!("range {range} never reached 150 trees");
    };
    let fast = time_to_150(3);
    let slow = time_to_150(1);
    assert!(
        fast * 2 < slow,
        "range 3 ({fast} ticks) should colonize at least 2× faster than range 1 ({slow} ticks)"
    );
}

#[test]
fn certain_growth_saturates_exactly_the_seed_rain_disk_in_one_tick() {
    // p = 1 with shade off makes the growth pass deterministic: one tick
    // after a mature tree appears, every tile inside its range is a tree and
    // every tile beyond it is grass.
    let p = Params {
        grass_seed_p: 1.0,
        tree_growth_p: 1.0,
        shade_strength: 0.0,
        climate_swing: 0.0, // vigor exactly 1: the saturation is deterministic
        storm_rate: 0.0,
        terrain: 0.0, // flat and uniform: no niche multipliers
        grass_niches: 0.0,
        ..legacy()
    };
    let mut w = bare_world(11, p);
    let center = CELLS / 2;
    let (cq, cr) = G.index_to_axial(center);
    let t0 = 200u64;
    w.paint(center, Brush::Tree, t0 - 100); // mature immediately
    w.step(t0);
    for i in 0..CELLS {
        let (q, r) = G.index_to_axial(i);
        let want = if hex::distance(q, r, cq, cr) <= p.tree_range { Cell::Tree } else { Cell::Grass };
        assert_eq!(w.state(i), want, "cell {i} at distance {}", hex::distance(q, r, cq, cr));
    }
}

#[test]
fn zero_growth_worlds_go_fully_bare_once_lifespans_expire() {
    let p = planted(Params {
        grass_seed_p: 0.0,
        grass_clonal_p: 0.0,
        tree_growth_p: 0.0,
        // Root-crown fire resprouting is survival, not growth: without
        // lightning every stem eventually dies for good.
        storm_rate: 0.0,
        ..legacy()
    });
    let mut w = World::with_params(3, p);
    // Lifetimes are unbounded geometrics: run many mean-lives so the
    // survival tail is effectively zero — longer than a dozen, since a
    // valley willow in low drought stress also resprouts from its roots.
    for tick in 1..=(30 * p.tree_mean_life as u64) {
        w.step(tick);
    }
    assert_eq!(w.counts(), [CELLS as u32, 0, 0]);
}

#[test]
fn absurd_parameters_behave_exactly_like_their_sanitized_form() {
    let absurd = Params {
        grass_seed_p: 42.0,
        grass_clonal_p: -1.0,
        shade_strength: 9.0,
        tree_growth_p: -3.0,
        tree_range: 999,
        tree_maturity_age: u32::MAX,
        sod_factor: -2.0,
        crowding_p: 7.0,
        grass_mean_life: 0,
        tree_mean_life: u32::MAX,
        fire_ignition_p: 5.0,
        fire_spread_p: -1.0,
        nutrient_boost: f64::INFINITY,
        storm_rate: -3.0,
        storm_lightning_p: 42.0,
        climate_swing: f64::NAN,
        water_table: -1.0,
        mutation_rate: f64::INFINITY,
        terrain: 7.0,
        grass_niches: -3.0,
        pest_strength: 9.0,
        browse: f64::NAN,
        competition: -2.0,
        climate_zones: 5.0,
        rivers: -1.0,
        grazing: f64::NAN,
        seed_tree_p: 2.0,
        seed_grass_p: -1.0,
        width: 64,
        height: 64,
    };
    let mut a = World::with_params(21, absurd);
    let mut b = World::with_params(21, absurd.sanitized());
    for tick in 1..=300 {
        a.step(tick);
        b.step(tick);
    }
    for i in 0..CELLS {
        assert_eq!(a.state(i), b.state(i), "cell {i} diverged");
    }
    assert_eq!(a.counts().iter().sum::<u32>(), CELLS as u32);
}

#[test]
fn runs_are_reproducible_from_seed_params_and_click_history() {
    let p = legacy();
    let clicks: [(u64, usize, Brush); 5] = [
        (50, 1000, Brush::Tree),
        (200, 2000, Brush::Clear),
        (350, 3000, Brush::Grass),
        (351, 3000, Brush::Fire),
        (500, 64, Brush::Tree),
    ];
    let run = || {
        let mut w = World::with_params(99, p);
        for tick in 1u64..=800 {
            for &(at, index, brush) in &clicks {
                if at == tick {
                    w.paint(index, brush, tick);
                }
            }
            w.step(tick);
        }
        w
    };
    let (a, b) = (run(), run());
    for i in 0..CELLS {
        assert_eq!(a.state(i), b.state(i), "cell {i} diverged");
    }

    // And a different tree probability must actually change the outcome.
    let mut c = World::with_params(99, Params { tree_growth_p: 0.006, ..p });
    for tick in 1..=800 {
        c.step(tick);
    }
    assert!((0..CELLS).any(|i| a.state(i) != c.state(i)));
}

#[test]
fn changing_params_mid_run_shifts_the_regime() {
    // Start in the savanna balance, then speed tree recruitment to forest
    // levels: the canopy closes and — with no spontaneous grass seed —
    // the creeping sward is squeezed out for good.
    let mut w = World::with_params(7, planted(legacy()));
    for tick in 1..=5_000u64 {
        w.step(tick);
    }
    let grass_savanna = w.counts()[1];
    assert!(grass_savanna > 250, "savanna should carry a sward, saw {grass_savanna}");

    w.set_params(Params { tree_growth_p: 0.01, ..legacy() });
    for tick in 5_001..=14_000u64 {
        w.step(tick);
    }
    let [_, grass_after, trees_after] = w.counts();
    // Shade-tolerant sod lingers in the understory, but the sward collapses.
    assert!(
        grass_after * 5 < grass_savanna,
        "the closing canopy should squeeze the sward ({grass_savanna} → {grass_after})"
    );
    assert!(trees_after > 780, "trees should close into forest, saw {trees_after}");
}

// ---- landscape niches and biodiversity ----

/// Mean (heat, groundwater, soil depth) of the tiles one plant type holds.
fn habitat(w: &World, is: impl Fn(usize) -> bool) -> (f64, f64, f64, u32) {
    let (mut h, mut g, mut s, mut n) = (0.0, 0.0, 0.0, 0u32);
    for i in 0..CELLS {
        if is(i) {
            let (_, heat, depth) = w.terrain_at(i);
            h += heat as f64;
            g += w.water_table(i) as f64;
            s += depth as f64;
            n += 1;
        }
    }
    let d = n.max(1) as f64;
    (h / d, g / d, s / d, n)
}

#[test]
fn every_plant_type_finds_its_own_corner_of_the_landscape() {
    // Measured with `cargo run --release --example niche_probe` (seed 42,
    // t=10000): bunchgrass on hot slopes (heat ~0.60) and sod on cool ones
    // (~0.39); sedge in the saturated bottoms (groundwater ~0.85); willow
    // in the valleys (~0.62) vs pine on the dry thin-soiled ridges (~0.01,
    // soil ~0.49) and oak on the deeper mid-slopes (soil ~0.60).
    let mut w = World::with_params(42, planted(legacy()));
    for tick in 1..=10_000u64 {
        w.step(tick);
    }
    let grass = |k: GrassKind| {
        let w = &w;
        move |i: usize| w.state(i) == Cell::Grass && w.grass_kind(i) == k
    };
    let tree = |sp: Species| {
        let w = &w;
        move |i: usize| w.state(i) == Cell::Tree && w.species(i) == sp
    };
    let bunch = habitat(&w, grass(GrassKind::Bunch));
    let sod = habitat(&w, grass(GrassKind::Sod));
    let sedge = habitat(&w, grass(GrassKind::Sedge));
    let willow = habitat(&w, tree(Species::Willow));
    let pine = habitat(&w, tree(Species::Pine));
    let oak = habitat(&w, tree(Species::Oak));
    for (name, h) in [("bunch", bunch), ("sod", sod), ("sedge", sedge), ("willow", willow), ("pine", pine), ("oak", oak)] {
        assert!(h.3 >= 10, "{name} should persist, saw {} tiles", h.3);
    }
    assert!(bunch.0 > sod.0 + 0.1, "bunchgrass hot, sod cool ({:.2} vs {:.2})", bunch.0, sod.0);
    assert!(sedge.1 > 0.6, "sedge in the wet bottoms (groundwater {:.2})", sedge.1);
    assert!(willow.1 > pine.1 + 0.4, "willow valleys vs pine ridges ({:.2} vs {:.2})", willow.1, pine.1);
    assert!(oak.2 > pine.2 + 0.05, "oak on deeper soil than pine ({:.2} vs {:.2})", oak.2, pine.2);
}

#[test]
fn relief_and_grass_niches_raise_biodiversity() {
    // Effective number of plant types (e^Shannon), averaged over ticks
    // 4000–8000. Flat ground with one generic grass sorts nothing: ~2–3
    // types. Relief + grass functional types: ~4.6–4.8.
    let mean_eff = |p: Params| {
        let mut w = World::with_params(42, planted(p));
        let (mut sum, mut n) = (0.0, 0);
        for tick in 1..=8_000u64 {
            w.step(tick);
            if tick > 4_000 && tick % 200 == 0 {
                sum += w.diversity().1;
                n += 1;
            }
        }
        sum / n as f64
    };
    let flat = mean_eff(Params { terrain: 0.0, grass_niches: 0.0, ..legacy() });
    let rich = mean_eff(legacy());
    println!("effective types: flat {flat:.2}, relief + niches {rich:.2}");
    assert!(rich > 4.0, "the default landscape should hold ≥4 effective types, saw {rich:.2}");
    assert!(rich > flat + 1.0, "niches should add at least one effective type ({flat:.2} → {rich:.2})");
}
