//! The order layers on a 128² map: vegetation self-organization
//! (`patterns`), the designed map (`map_design`), biome events, and fauna.
//! Values measured with
//! `cargo run --release --example patterns_probe -- 128 7 4000` (seed 7:
//! stand aggregation 1.97 on vs 1.48 off; oldest tree 3828 vs 2433; γ 4.70
//! vs 4.82; temperate forest 34–37% of the windward fifths vs 8–9% in the
//! lee; 3 herds and a pack; 39 superblooms). Each world runs once.

use std::sync::OnceLock;

use tree_engine::sim::hex;
use tree_engine::sim::world::{Biome, Cell, Params, World};

const HORIZON: u64 = 4_000;

struct Run {
    world: World,
    gamma: f64,
    herds: f64,
    packs: f64,
}

fn simulate(p: Params) -> Run {
    let mut world = World::with_params(7, p);
    let (mut g, mut herds, mut packs, mut k) = (0.0, 0.0, 0.0, 0.0);
    for t in 1..=HORIZON {
        world.step(t);
        if t > HORIZON / 2 && t % 100 == 0 {
            g += world.diversity().1;
            herds += world.fauna().herds.len() as f64;
            packs += world.fauna().packs.len() as f64;
            k += 1.0;
        }
    }
    Run { world, gamma: g / k, herds: herds / k, packs: packs / k }
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
    RUN.get_or_init(|| simulate(Params { patterns: 0.0, map_design: 0.0, events: 0.0, fauna: 0.0, ..base() }))
}

/// Mean tree density within 3 tiles of a tree over the map's density
/// (1 = random placement; stands with edges score higher).
fn aggregation(w: &World) -> f64 {
    let g = w.grid();
    let n = g.cells();
    let p = (0..n).filter(|&i| w.state(i) == Cell::Tree).count() as f64 / n as f64;
    let disk = hex::disk(3);
    let (mut sum, mut cnt) = (0.0, 0.0);
    for i in (0..n).filter(|&i| w.state(i) == Cell::Tree) {
        let (q, r) = g.index_to_axial(i);
        let (mut t, mut m) = (0.0, 0.0);
        for &(dq, dr, d) in &disk {
            if let (true, Some(j)) = (d > 0, g.axial_to_index(q + dq, r + dr)) {
                m += 1.0;
                t += (w.state(j) == Cell::Tree) as u8 as f64;
            }
        }
        sum += t / f64::max(m, 1.0);
        cnt += 1.0;
    }
    sum / f64::max(cnt, 1.0) / p.max(1e-9)
}

#[test]
fn forests_grow_as_stands_with_edges() {
    let (a_on, a_off) = (aggregation(&on().world), aggregation(&off().world));
    assert!(a_on > 1.2 * a_off, "stand aggregation {a_on:.2} on vs {a_off:.2} off");
}

#[test]
fn veterans_reach_great_age() {
    let oldest = |w: &World| (0..w.grid().cells()).map(|i| w.age(i, HORIZON)).max().unwrap();
    let (o_on, o_off) = (oldest(&on().world), oldest(&off().world));
    assert!(o_on > o_off + 500, "oldest tree {o_on} on vs {o_off} off");
}

#[test]
fn the_order_layers_keep_the_diversity() {
    assert!(on().gamma > 0.9 * off().gamma, "γ {:.2} on vs {:.2} off", on().gamma, off().gamma);
}

#[test]
fn the_windward_side_is_forested_and_the_lee_lies_in_a_rain_shadow() {
    let w = &on().world;
    let g = w.grid();
    let east = w.prevailing_wind()[0] < 0.0; // the wind blows from the east coast
    let inland = |i: usize| {
        let f = (i as i32 % g.width) as f64 / (g.width - 1) as f64;
        if east { 1.0 - f } else { f }
    };
    let share = |b: Biome, lo: f64, hi: f64| {
        let tiles: Vec<usize> = (0..g.cells()).filter(|&i| (lo..hi).contains(&inland(i))).collect();
        tiles.iter().filter(|&&i| w.biome(i) == b).count() as f64 / tiles.len() as f64
    };
    let (wind_forest, lee_forest) = (share(Biome::TemperateForest, 0.0, 0.4), share(Biome::TemperateForest, 0.6, 1.0));
    assert!(wind_forest > 2.0 * lee_forest, "forest windward {wind_forest:.2} vs lee {lee_forest:.2}");
    let dry = |lo, hi| share(Biome::Desert, lo, hi) + share(Biome::Savanna, lo, hi) + share(Biome::Grassland, lo, hi);
    assert!(dry(0.5, 1.0) > dry(0.0, 0.4), "the lee is drier");
}

#[test]
fn the_designed_map_is_named_and_has_its_landmarks() {
    let w = &on().world;
    assert!(w.landmarks().len() >= 3, "{:?}", w.landmarks());
    assert!(w.regions().len() >= 8, "{} regions", w.regions().len());
    assert!(off().world.landmarks().is_empty());
}

#[test]
fn herds_and_wolves_live_on_the_land() {
    assert!(on().herds >= 1.0, "herds {:.1}", on().herds);
    assert!(on().packs >= 0.3, "packs {:.1}", on().packs);
    assert_eq!(off().herds, 0.0);
}

#[test]
fn deserts_bloom_after_rain() {
    assert!(on().world.biome_events()[0] >= 1, "superblooms {:?}", on().world.biome_events());
    assert_eq!(off().world.biome_events()[0], 0);
}
