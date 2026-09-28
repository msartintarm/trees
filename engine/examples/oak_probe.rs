//! Tree composition per regime (end of run, mean over two seeds), with each
//! neighbor-competition mechanism knocked out in turn — the check for one
//! species taking over (or being wiped off) the map.
use tree_engine::sim::hex;
use tree_engine::sim::world::{Cell, Params, World, SPECIES_COUNT};

fn main() {
    let planted = |p: Params| Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..p };
    let only = std::env::args().nth(1);
    for (mech, m) in [
        ("all on", Params::legacy_map()),
        ("no pests", Params { pest_strength: 0.0, ..Params::legacy_map() }),
        ("no browse", Params { browse: 0.0, ..Params::legacy_map() }),
        ("no competition", Params { competition: 0.0, ..Params::legacy_map() }),
        ("all off", Params { pest_strength: 0.0, browse: 0.0, competition: 0.0, ..Params::legacy_map() }),
    ] {
        if only.as_deref().is_some_and(|o| o != "all" && o != mech) {
            continue;
        }
        for (label, p) in [
            ("default", planted(m)),
            ("moist forest", planted(Params { tree_growth_p: 0.01, ..m })),
            ("fire savanna", planted(Params { fire_ignition_p: 0.0005, ..m })),
        ] {
            let mut comp = [0u32; SPECIES_COUNT];
            let (mut eff, mut pests) = (0.0, 0u32);
            for seed in [7u64, 42] {
                let mut w = World::with_params(seed, p);
                for t in 1..=8_000u64 {
                    w.step(t);
                }
                for i in 0..hex::Grid::LEGACY.cells() {
                    if w.state(i) == Cell::Tree {
                        comp[w.species(i) as usize] += 1;
                    }
                }
                eff += w.diversity().1 / 2.0;
                pests += w.infested_count() / 2;
            }
            let c: Vec<u32> = comp.iter().map(|&c| c / 2).collect();
            println!("{mech:<15} {label:<13} acacia/oak/pine/willow {c:?}  eff {eff:.2}  infested {pests}");
        }
    }
}
