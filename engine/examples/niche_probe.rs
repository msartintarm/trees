//! Where does each plant type end up living? Mean local conditions per type
//! and landscape biodiversity, flat/legacy vs full terrain + grass niches.
use tree_engine::sim::hex;
use tree_engine::sim::world::{Cell, Params, World, GRASS_KIND_COUNT, SPECIES_COUNT};

const T: [&str; 4] = ["Acacia", "Oak", "Pine", "Willow"];
const G: [&str; 4] = ["Bunch", "Sod", "Sedge", "Annual"];

fn main() {
    let planted = |p: Params| Params { seed_tree_p: 0.02, seed_grass_p: 0.10, ..p };
    let d = Params::legacy_map();
    for (label, p) in [
        ("flat + generic grass", planted(Params { terrain: 0.0, grass_niches: 0.0, ..d })),
        ("terrain only", planted(Params { grass_niches: 0.0, ..d })),
        ("terrain + grass niches", planted(d)),
        ("  ... with fire", planted(Params { fire_ignition_p: 0.0005, ..d })),
    ] {
        for seed in [7u64, 42] {
            let mut w = World::with_params(seed, p);
            let mut h_sum = 0.0;
            let mut n_h = 0;
            for t in 1..=10_000u64 {
                w.step(t);
                if t > 4_000 && t % 200 == 0 {
                    h_sum += w.diversity().1;
                    n_h += 1;
                }
            }
            let (h, eff) = w.diversity();
            println!("== {label} (seed {seed})  final H={h:.2} eff={eff:.2}  mean eff={:.2}", h_sum / n_h as f64);
            let mut rows: Vec<(String, u32, f64, f64, f64, f64)> = Vec::new();
            for k in 0..SPECIES_COUNT + GRASS_KIND_COUNT {
                let (mut n, mut e, mut ht, mut wa, mut so) = (0u32, 0.0, 0.0, 0.0, 0.0);
                for i in 0..hex::Grid::LEGACY.cells() {
                    let hit = match w.state(i) {
                        Cell::Tree => k < SPECIES_COUNT && w.species(i) as usize == k,
                        Cell::Grass => k >= SPECIES_COUNT && w.grass_kind(i) as usize == k - SPECIES_COUNT,
                        Cell::Bare => false,
                    };
                    if hit {
                        let (el, heat, depth) = w.terrain_at(i);
                        n += 1; e += el as f64; ht += heat as f64; so += depth as f64;
                        wa += w.water_table(i) as f64;
                    }
                }
                let name = if k < SPECIES_COUNT { T[k].to_string() } else { format!("~{}", G[k - SPECIES_COUNT]) };
                let nn = n.max(1) as f64;
                rows.push((name, n, e / nn, ht / nn, wa / nn, so / nn));
            }
            for (name, n, e, ht, wa, so) in rows {
                if n > 0 {
                    println!("   {name:<8} {n:>5}  elev {e:.2}  heat {ht:.2}  groundwater {wa:.2}  soil {so:.2}");
                }
            }
        }
    }
}
