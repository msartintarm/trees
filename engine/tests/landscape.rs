//! Landscape-scale features on a 128² map: climate zones from mountain
//! altitude, rivers with flood pulses, and grazing. Values measured with
//! `cargo run --release --example landscape_probe -- 128 7 8000` (seed 7:
//! γ 4.63 on vs 4.05 off; turnover β 1.56 vs 1.32; acacia 4.1% of the
//! lowest altitude band vs 0% of the highest; sod 14.5% vs 45.9%; willows
//! 100% within 4 tiles of a river). Each world is simulated once and
//! shared by the tests below.

use std::sync::OnceLock;

use tree_engine::sim::world::{Cell, GrassKind, Params, Species, World};

const HORIZON: u64 = 6_000;

struct Run {
    world: World,
    floods: u32,
    gamma: f64,
    alpha: f64,
}

fn simulate(p: Params) -> Run {
    let mut world = World::with_params(7, p);
    let (mut floods, mut was) = (0, false);
    let (mut g, mut a, mut n) = (0.0, 0.0, 0.0);
    for t in 1..=HORIZON {
        world.step(t);
        if world.is_flooding() && !was {
            floods += 1;
        }
        was = world.is_flooding();
        if t > 3_000 && t % 250 == 0 {
            g += world.diversity().1;
            a += world.local_diversity();
            n += 1.0;
        }
    }
    Run { world, floods, gamma: g / n, alpha: a / n }
}

fn base() -> Params {
    Params { width: 128, height: 128, seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() }
}

fn on() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(|| simulate(base()))
}

fn off() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(|| simulate(Params { climate_zones: 0.0, rivers: 0.0, grazing: 0.0, ..base() }))
}

/// Share of an altitude band's tiles held by tiles matching `is`.
fn band_share(w: &World, band: usize, is: impl Fn(usize) -> bool) -> f64 {
    let n = w.grid().cells();
    let amp = (0..n).map(|i| w.altitude(i)).fold(0.0f32, f32::max);
    let (lo, hi) = (band as f32 / 5.0 * amp, (band + 1) as f32 / 5.0 * amp + 1e-6);
    let tiles: Vec<usize> = (0..n).filter(|&i| w.altitude(i) >= lo && w.altitude(i) < hi).collect();
    tiles.iter().filter(|&&i| is(i)).count() as f64 / tiles.len().max(1) as f64
}

#[test]
fn mountains_sort_the_vegetation_into_climate_zones() {
    let w = &on().world;
    let acacia = |i: usize| w.state(i) == Cell::Tree && w.species(i) == Species::Acacia;
    let sod = |i: usize| w.state(i) == Cell::Grass && w.grass_kind(i) == GrassKind::Sod;
    let (acacia_low, acacia_high) = (band_share(w, 0, acacia), band_share(w, 4, acacia));
    let (sod_low, sod_high) = (band_share(w, 0, sod), band_share(w, 4, sod));
    assert!(acacia_low > 2.0 * acacia_high + 0.005, "acacia in the warm lowlands ({acacia_low:.3} vs {acacia_high:.3})");
    assert!(sod_high > 2.0 * sod_low, "cool-season sod on the heights ({sod_high:.3} vs {sod_low:.3})");
    // And it's cooler up there.
    let n = w.grid().cells();
    let high = (0..n).max_by(|&a, &b| w.altitude(a).total_cmp(&w.altitude(b))).unwrap();
    let low = (0..n).min_by(|&a, &b| w.altitude(a).total_cmp(&w.altitude(b))).unwrap();
    assert!(w.temperature(high) < w.temperature(low) - 0.15);
}

#[test]
fn rivers_are_open_water_and_floods_lay_down_seedbeds() {
    let run = on();
    let w = &run.world;
    let n = w.grid().cells();
    let channels: Vec<usize> = (0..n).filter(|&i| w.is_channel(i)).collect();
    assert!(channels.len() > n / 200, "a river network should form ({} channel tiles)", channels.len());
    assert!(channels.iter().all(|&i| w.state(i) == Cell::Bare), "nothing grows in open water");
    assert!(run.floods >= 3, "wet seasons should flood the rivers (saw {})", run.floods);
}

#[test]
fn willows_line_the_rivers_instead_of_blanketing_wet_ground() {
    let w = &on().world;
    let n = w.grid().cells();
    let willows: Vec<usize> =
        (0..n).filter(|&i| w.state(i) == Cell::Tree && w.species(i) == Species::Willow).collect();
    assert!(!willows.is_empty(), "willows should persist along the rivers");
    let riparian = willows.iter().filter(|&&i| w.river_distance(i).is_some_and(|d| d <= 4)).count();
    assert!(riparian * 10 >= willows.len() * 8, "{riparian}/{} willows within 4 tiles of a river", willows.len());
    let drowned = willows.iter().filter(|&&i| w.water_table(i) > 0.95).count();
    assert!(drowned * 10 <= willows.len(), "saturated marsh is sedge country ({drowned} willows there)");
    // Without rivers, the same wet ground is a willow blanket.
    let off = &off().world;
    let blanket = (0..n).filter(|&i| off.state(i) == Cell::Tree && off.species(i) == Species::Willow).count();
    assert!(blanket > willows.len() * 3, "willow blanket {blanket} vs riparian band {}", willows.len());
}

#[test]
fn landscape_features_raise_diversity_across_the_map() {
    let (on, off) = (on(), off());
    let (beta_on, beta_off) = (on.gamma / on.alpha, off.gamma / off.alpha);
    assert!(on.gamma > off.gamma + 0.2, "whole-map diversity {:.2} vs {:.2}", on.gamma, off.gamma);
    assert!(beta_on > beta_off + 0.1, "regions should differ more (β {beta_on:.2} vs {beta_off:.2})");
}

#[test]
fn grazing_keeps_the_sod_mat_in_check() {
    let n = on().world.grid().cells() as f64;
    let sod = |w: &World| (0..w.grid().cells()).filter(|&i| w.state(i) == Cell::Grass && w.grass_kind(i) == GrassKind::Sod).count() as f64 / n;
    let (grazed, ungrazed) = (sod(&on().world), sod(&off().world));
    assert!(grazed < ungrazed * 0.85, "sod share {grazed:.3} grazed vs {ungrazed:.3}");
}
