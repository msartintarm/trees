//! How fast does a species spread from a single founding patch? Plants a
//! small stand in the middle of an empty map and tracks the invasion:
//! the front (90th-percentile distance of trees from the origin, in tiles),
//! the farthest tree, and the trees in far outposts (> 30 tiles out).
//!
//!     cargo run --release --example spread_probe -- 256 4000
use tree_engine::sim::hex::{self, Grid};
use tree_engine::sim::world::{Brush, Cell, Params, Species, World};

fn main() {
    let side: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(256);
    let horizon: u64 = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(4000);
    let sps = [Species::Acacia, Species::Oak, Species::Pine, Species::Willow];
    let lines: Vec<String> = std::thread::scope(|s| {
        let hs: Vec<_> = sps
            .iter()
            .map(|&sp| {
                s.spawn(move || {
                    let p = Params { width: side, height: side, ..Params::default() };
                    let mut w = World::with_params(7, p);
                    let g: Grid = w.grid();
                    let origin = g.middle();
                    let (oq, or) = g.index_to_axial(origin);
                    // A founding stand: every tile within 2 of the middle.
                    for (dq, dr, _) in hex::disk(2) {
                        if let Some(i) = g.axial_to_index(oq + dq, or + dr) {
                            w.paint_species(i, Brush::Tree, sp, 0);
                        }
                    }
                    // A lawn of grass everywhere else for a realistic seedbed.
                    for i in 0..g.cells() {
                        if w.state(i) == Cell::Bare && i % 3 == 0 {
                            w.paint_grass(i, tree_engine::sim::world::GrassKind::Sod, 0);
                        }
                    }
                    let mut out = format!("{sp:?}:");
                    for t in 1..=horizon {
                        w.step(t);
                        if t % (horizon / 4) == 0 {
                            let mut d: Vec<i32> = (0..g.cells())
                                .filter(|&i| w.state(i) == Cell::Tree && w.species(i) == sp)
                                .map(|i| {
                                    let (q, r) = g.index_to_axial(i);
                                    hex::distance(q, r, oq, or)
                                })
                                .collect();
                            d.sort();
                            if d.is_empty() {
                                out += &format!(" t={t}: extinct |");
                                continue;
                            }
                            let p90 = d[(d.len() * 9) / 10];
                            let far = d.iter().filter(|&&x| x > 30).count();
                            out += &format!(" t={t}: n={} front {} max {} outposts>30 {} |", d.len(), p90, d.last().unwrap(), far);
                        }
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for l in lines {
        println!("{l}");
    }
}
