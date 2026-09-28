//! Search for the conditions where oak thrives: alone at the top of the
//! canopy ("oak woodland"), and alongside the most other types ("mixed oak
//! mosaic"). Sweeps the ecology knobs oak's traits respond to — recruitment
//! speed (succession), pest and browse pressure, neighbor competition, and
//! grazing (wood pasture: grazers hold the sod mat open for acorns) — on a
//! 128² map with the landscape features on.
//!
//!     cargo run --release --example oak_presets -- 6000
use tree_engine::sim::hex::Grid;
use tree_engine::sim::world::{Cell, Params, Species, World};

fn main() {
    let horizon: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(6000);
    let base = Params {
        width: 128,
        height: 128,
        seed_tree_p: 0.02,
        seed_grass_p: 0.10,
        ..Params::default()
    };
    let mut combos = Vec::new();
    for growth in [0.002, 0.005, 0.01] {
        for pests in [0.3, 1.0] {
            for browse in [0.3, 1.0] {
                for competition in [0.5, 1.0] {
                    for grazing in [0.0, 1.0] {
                        combos.push((growth, pests, browse, competition, grazing));
                    }
                }
            }
        }
    }
    // One thread per combination (worlds are independent).
    let rows: Vec<(f64, f64, f64, f64, f64, f64, f64)> = std::thread::scope(|scope| {
        let handles: Vec<_> = combos
            .iter()
            .map(|&(growth, pests, browse, competition, grazing)| {
                scope.spawn(move || {
                    let p = Params { tree_growth_p: growth, pest_strength: pests, browse, competition, grazing, ..base };
                    let (mut oak, mut trees, mut g) = (0.0, 0.0, 0.0);
                    for seed in [7u64, 42] {
                        let mut w = World::with_params(seed, p);
                        let cells = Grid::new(128, 128).cells();
                        let (mut o, mut t, mut e, mut n) = (0.0, 0.0, 0.0, 0.0);
                        for tick in 1..=horizon {
                            w.step(tick);
                            if tick > horizon / 2 && tick % 250 == 0 {
                                let (mut oc, mut tc) = (0, 0);
                                for i in 0..cells {
                                    if w.state(i) == Cell::Tree {
                                        tc += 1;
                                        if w.species(i) == Species::Oak {
                                            oc += 1;
                                        }
                                    }
                                }
                                o += oc as f64 / cells as f64;
                                t += tc as f64 / cells as f64;
                                e += w.diversity().1;
                                n += 1.0;
                            }
                        }
                        oak += o / n / 2.0;
                        trees += t / n / 2.0;
                        g += e / n / 2.0;
                    }
                    (oak, g, growth, pests, browse, competition, grazing, trees)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                let (oak, g, growth, pests, browse, competition, grazing, trees) = h.join().unwrap();
                println!(
                    "growth {growth:.3} pests {pests:.1} browse {browse:.1} competition {competition:.1} grazing {grazing:.1}: oak {:.1}% of map ({:.0}% of trees)  γ {g:.2}",
                    100.0 * oak,
                    100.0 * oak / trees.max(1e-9)
                );
                (oak, g, growth, pests, browse, competition, grazing)
            })
            .collect()
    });
    let best = rows.iter().cloned().fold(rows[0], |a, b| if b.0 > a.0 { b } else { a });
    println!("most oak: {best:?}");
    // Mixed: oak at least half its best share, then the most diverse.
    let mixed = rows
        .iter()
        .filter(|r| r.0 >= best.0 * 0.5)
        .cloned()
        .fold(best, |a, b| if b.1 > a.1 { b } else { a });
    println!("most diverse with strong oak: {mixed:?}");
}
