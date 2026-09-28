//! Whole-world integration tests: parameter constraints exercised through
//! the public API, and long-run regime bands for the configurations the
//! `equilibrium` example probe measured (re-derive with
//! `cargo run --release --example equilibrium`). Bands are deliberately wide
//! around the observed min/max so they assert the regime — coexistence,
//! closed forest, fire-swept grassland, bistability — not the trajectory.

use tree_engine::sim::hex;
use tree_engine::sim::world::{Brush, Cell, Params, Species, World, SPECIES_COUNT};

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
    for i in 0..hex::CELLS {
        w.paint(i, Brush::Clear, 0);
    }
    w
}

// ---- regimes (bands from the equilibrium example probe) ----

#[test]
fn default_savanna_holds_grass_and_trees_in_coexistence() {
    // Under full climate (hazard mortality + seasonal swings), measured over
    // 5 seeds, ticks 3000–14000: grass ~638 [273..1210] breathing with the
    // seasons, trees ~487 [335..645].
    for seed in [7u64, 42] {
        let (grass, trees) = run_and_sample(seed, planted(Params::default()), 3_000, 8_000, 100);
        assert_band(&grass, 180, 1_400, "grass");
        assert_band(&trees, 280, 720, "trees");
    }
}

#[test]
fn faster_trees_without_fire_close_into_moist_forest() {
    // tree_growth_p 1%, no lightning: shade-tolerant oak succeeds the
    // pioneers and packs the canopy denser than any single-species forest
    // before it — trees ~1900 [1443..2265]. Creeping grass dies beneath.
    let p = planted(Params { tree_growth_p: 0.01, ..Params::default() });
    let (grass, trees) = run_and_sample(42, p, 3_000, 8_000, 100);
    assert_band(&trees, 1_250, 2_450, "trees");
    assert_band(&grass, 0, 40, "grass");
}

#[test]
fn frequent_lightning_burns_the_world_down_to_grassland() {
    // Slow trees (0.1%) + ignition 5e-4: burns kill every sapling before
    // maturity — trees extinct on all 5 probed seeds; the sward surges in
    // wet seasons and burns back in droughts (grass ~875 [259..2152]).
    let p = planted(Params { tree_growth_p: 0.001, fire_ignition_p: 0.0005, ..Params::default() });
    let (grass, trees) = run_and_sample(1234, p, 3_000, 8_000, 100);
    assert_band(&trees, 0, 20, "trees");
    assert_band(&grass, 150, 2_300, "grass");
}

#[test]
fn strong_lightning_spreads_the_outcomes_across_a_spectrum() {
    // The savanna config + heavy lightning: alternative stable states under
    // real seasonal forcing. Measured, ticks 4000–14000: seed 1234 burns
    // down to pure grassland; seed 42 holds an embattled pioneer woodland
    // (trees mean ~353, swinging [23..756], acacia + willow — fire keeps
    // the oaks out); grass survives both fates.
    let p = planted(Params { fire_ignition_p: 0.0005, ..Params::default() });
    let (grass_wood, trees_wood) = run_and_sample(42, p, 4_000, 10_000, 100);
    let (grass_burn, trees_burn) = run_and_sample(1234, p, 5_000, 10_000, 100);
    let wood_mean = trees_wood.iter().map(|&v| v as f64).sum::<f64>() / trees_wood.len() as f64;
    assert!(wood_mean > 150.0, "woodland-fated seed should keep trees, mean {wood_mean:.0}");
    assert!(trees_wood.iter().any(|&t| t > 400), "and swing high between fires");
    assert_band(&trees_burn, 0, 30, "trees (grassland-fated seed)");
    assert!(grass_wood.iter().all(|&g| g > 30), "grass survives the woodland fate");
    assert!(grass_burn.iter().all(|&g| g > 150), "grass owns the grassland fate");
}

#[test]
fn the_fire_trap_selects_against_late_maturity() {
    // Under the same strong fire regime and seed, trees that stay flammable
    // until age 120 never escape the burn cycle (extinct on both probed
    // seeds), while age-40 trees close into forest (trees ~891 on seed 77).
    let fire = planted(Params { tree_growth_p: 0.005, fire_ignition_p: 0.0005, ..Params::default() });
    let (_, late) = run_and_sample(77, Params { tree_maturity_age: 120, ..fire }, 4_000, 8_000, 100);
    let (_, early) = run_and_sample(77, fire, 4_000, 8_000, 100);
    assert_band(&late, 0, 10, "late-maturity trees");
    assert_band(&early, 650, 1_100, "early-maturity trees");
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
        ..Params::default()
    };
    let (grass, trees) = run_and_sample(7, p, 2_000, 4_000, 100);
    assert_band(&trees, 0, 0, "trees");
    assert_band(&grass, 380, 600, "grass");
}

#[test]
fn succession_pioneers_first_then_the_oaks_take_the_forest() {
    // From an even planting in the fire-free forest regime: fast pioneers
    // dominate the early stand, then the shade-tolerant, long-lived oaks
    // close over them. Measured (seed 42): t=3000 already oak-led but with
    // hundreds of pioneers; by t=9000 the pioneers are gone.
    let p = planted(Params { tree_growth_p: 0.01, ..Params::default() });
    let mut w = World::with_params(42, p);
    let comp = |w: &World| -> [u32; SPECIES_COUNT] {
        let mut c = [0u32; SPECIES_COUNT];
        for i in 0..hex::CELLS {
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
    assert!(
        early_pioneers > 200,
        "the young stand should be full of pioneers, saw {early_pioneers}"
    );
    for tick in 1_001..=12_000u64 {
        w.step(tick);
    }
    let late = comp(&w);
    let late_pioneers = late[0] + late[2] + late[3];
    assert!(
        late[1] > 1_200,
        "oaks should have closed the canopy, saw {} oaks",
        late[1]
    );
    assert!(
        late_pioneers < late[1] / 10,
        "pioneers should be relegated to relics ({late_pioneers} vs {} oaks)",
        late[1]
    );
    // And the seed-source outside a burn: species breeding true is what
    // makes this succession rather than reshuffling.
    let _ = Species::Acacia;
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
            ..Params::default()
        };
        let mut w = bare_world(5, p);
        let center = hex::CELLS / 2 + hex::WIDTH as usize / 2;
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
        ..Params::default()
    };
    let mut w = bare_world(11, p);
    let center = hex::CELLS / 2;
    let (cq, cr) = hex::index_to_axial(center);
    let t0 = 200u64;
    w.paint(center, Brush::Tree, t0 - 100); // mature immediately
    w.step(t0);
    for i in 0..hex::CELLS {
        let (q, r) = hex::index_to_axial(i);
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
        ..Params::default()
    });
    let mut w = World::with_params(3, p);
    // Lifetimes are unbounded geometrics now: run a dozen mean-lives so the
    // survival tail (e⁻¹² per tree) is effectively zero.
    for tick in 1..=(12 * p.tree_mean_life as u64) {
        w.step(tick);
    }
    assert_eq!(w.counts(), [hex::CELLS as u32, 0, 0]);
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
        seed_tree_p: 2.0,
        seed_grass_p: -1.0,
    };
    let mut a = World::with_params(21, absurd);
    let mut b = World::with_params(21, absurd.sanitized());
    for tick in 1..=300 {
        a.step(tick);
        b.step(tick);
    }
    for i in 0..hex::CELLS {
        assert_eq!(a.state(i), b.state(i), "cell {i} diverged");
    }
    assert_eq!(a.counts().iter().sum::<u32>(), hex::CELLS as u32);
}

#[test]
fn runs_are_reproducible_from_seed_params_and_click_history() {
    let p = Params::default();
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
    for i in 0..hex::CELLS {
        assert_eq!(a.state(i), b.state(i), "cell {i} diverged");
    }

    // And a different tree probability must actually change the outcome.
    let mut c = World::with_params(99, Params { tree_growth_p: 0.006, ..p });
    for tick in 1..=800 {
        c.step(tick);
    }
    assert!((0..hex::CELLS).any(|i| a.state(i) != c.state(i)));
}

#[test]
fn changing_params_mid_run_shifts_the_regime() {
    // Start in the savanna balance, then speed tree recruitment to forest
    // levels: the canopy closes and — with no spontaneous grass seed —
    // the creeping sward is squeezed out for good.
    let mut w = World::with_params(7, planted(Params::default()));
    for tick in 1..=5_000u64 {
        w.step(tick);
    }
    let grass_savanna = w.counts()[1];
    assert!(grass_savanna > 250, "savanna should carry a sward, saw {grass_savanna}");

    w.set_params(Params { tree_growth_p: 0.01, ..Params::default() });
    for tick in 5_001..=14_000u64 {
        w.step(tick);
    }
    let [_, grass_after, trees_after] = w.counts();
    assert!(grass_after < 100, "the closing canopy should squeeze the sward out, saw {grass_after}");
    assert!(trees_after > 780, "trees should close into forest, saw {trees_after}");
}
