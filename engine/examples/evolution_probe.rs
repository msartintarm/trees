//! Does selection act on the heritable traits? Compare mean hardiness and
//! vigor after a long run in a calm vs a harsh (full-swing) climate.
use tree_engine::sim::hex;
use tree_engine::sim::world::{Cell, Params, World};
fn main() {
    let planted = |p: Params| Params { seed_tree_p: 0.02, seed_grass_p: 0.10, mutation_rate: 0.08, ..p };
    for (label, p) in [
        ("calm (swing 0)", planted(Params { climate_swing: 0.0, ..Params::legacy_map() })),
        ("harsh (swing 1)", planted(Params { climate_swing: 1.0, ..Params::legacy_map() })),
        ("fire (ign 5e-4)", planted(Params { fire_ignition_p: 0.0005, ..Params::legacy_map() })),
    ] {
        for seed in [7u64, 42, 1234] {
            let mut w = World::with_params(seed, p);
            print!("{label:<16} seed {seed:>4}:");
            for tick in 1..=30_000u64 {
                w.step(tick);
                if tick % 10_000 == 0 {
                    let (mut n, mut v, mut h) = (0.0f64, 0.0f64, 0.0f64);
                    for i in 0..hex::Grid::LEGACY.cells() {
                        if w.state(i) == Cell::Tree {
                            let g = w.genome(i);
                            n += 1.0; v += g[0] as f64; h += g[1] as f64;
                        }
                    }
                    print!("  t={:>5} n={:>4} vigor {:.3} hardy {:.3}", tick, n, v / n.max(1.0), h / n.max(1.0));
                }
            }
            println!();
        }
    }
}
