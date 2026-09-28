//! Parameter-space probe: runs the ecology long enough to see its long-run
//! behaviour under different tunables and reports population statistics, so
//! equilibrium bands asserted in `tests/ecology.rs` come from measurement
//! rather than guesswork. Mirrors the sibling traffic engine's `examples/`
//! probe convention.
//!
//!     cargo run --release --example equilibrium            # curated sweep
//!     cargo run --release --example equilibrium -- 12000   # custom horizon

use tree_engine::sim::hex;
use tree_engine::sim::world::{Params, World};

const WARMUP: u64 = 3_000;
const SAMPLE_EVERY: u64 = 50;

struct Stats {
    mean: f64,
    min: u32,
    max: u32,
}

fn stats(samples: &[u32]) -> Stats {
    let mean = samples.iter().map(|&v| v as f64).sum::<f64>() / samples.len() as f64;
    Stats {
        mean,
        min: *samples.iter().min().unwrap(),
        max: *samples.iter().max().unwrap(),
    }
}

/// Coexistence verdict: both populations alive at every sample after warmup,
/// and trees not saturating the grid (a frozen monoculture).
fn verdict(grass: &Stats, trees: &Stats) -> &'static str {
    let cells = hex::Grid::LEGACY.cells() as u32;
    if trees.min == 0 && grass.min == 0 {
        "both die out"
    } else if trees.min == 0 {
        "trees die out"
    } else if grass.min == 0 {
        "grass dies out"
    } else if trees.max > cells * 9 / 10 {
        "tree saturation"
    } else if grass.min >= 100 && trees.min >= 100 {
        "COEXISTS"
    } else {
        "marginal"
    }
}

fn run(label: &str, params: Params, horizon: u64, seeds: &[u64]) {
    for &seed in seeds {
        let mut w = World::with_params(seed, params);
        let mut grass = Vec::new();
        let mut trees = Vec::new();
        for tick in 1..=horizon {
            w.step(tick);
            if tick > WARMUP && tick % SAMPLE_EVERY == 0 {
                let c = w.counts();
                grass.push(c[1]);
                trees.push(c[2]);
            }
        }
        let (g, t) = (stats(&grass), stats(&trees));
        println!(
            "{label:<28} seed {seed:>4}  grass {:>6.0} [{:>4}..{:>4}]  trees {:>6.0} [{:>4}..{:>4}]  {}",
            g.mean, g.min, g.max, t.mean, t.min, t.max,
            verdict(&g, &t),
        );
    }
}

fn main() {
    let horizon: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(12_000);
    let seeds = [7u64, 42, 1234];
    println!("horizon {horizon} ticks, warmup {WARMUP}, sampling every {SAMPLE_EVERY}\n");

    let d = Params::legacy_map();
    let planted = |p: Params| Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..p };
    for (label, p) in [
        ("savanna (defaults)", planted(d)),
        ("savanna, calm climate", planted(Params { climate_swing: 0.0, ..d })),
        ("savanna, no storms", planted(Params { storm_rate: 0.0, ..d })),
        ("moist forest 1%", planted(Params { tree_growth_p: 0.01, ..d })),
        ("grassland .1% + ign 5e-4", planted(Params { tree_growth_p: 0.001, fire_ignition_p: 0.0005, ..d })),
        ("lottery: defaults + ign", planted(Params { fire_ignition_p: 0.0005, ..d })),
        ("long-lived trees (1000)", planted(Params { tree_mean_life: 1000, ..d })),
        ("harsh climate (swing 1)", planted(Params { climate_swing: 1.0, ..d })),
    ] {

        run(label, p, horizon, &seeds);
    }
}
