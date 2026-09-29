//! Biomes: how much of the map each climate biome covers, and which plant
//! types live in each (share of the biome's plants).
//!
//!     cargo run --release --example biome_probe -- 256 7 6000
use tree_engine::sim::world::{Biome, Cell, Params, World, BIOME_COUNT, GRASS_KIND_COUNT, SPECIES_COUNT};

const TYPES: usize = SPECIES_COUNT + GRASS_KIND_COUNT;
const NAMES: [&str; TYPES] = ["acacia", "oak", "pine", "willow", "spruce", "birch", "creosote", "bunch", "sod", "sedge", "annual", "reeds", "cactus"];

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let side: u32 = a.get(1).and_then(|x| x.parse().ok()).unwrap_or(128);
    let seed: u64 = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(7);
    let horizon: u64 = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(6000);
    for (label, biomes) in [("biomes ON", 1.0), ("biomes OFF", 0.0)] {
        let p = Params { width: side, height: side, seed_tree_p: 0.02, seed_grass_p: 0.10, biomes, ..Params::default() };
        let mut w = World::with_params(seed, p);
        let (mut g, mut k) = (0.0, 0.0);
        for t in 1..=horizon {
            w.step(t);
            if t > horizon / 2 && t % 250 == 0 {
                g += w.diversity().1;
                k += 1.0;
            }
        }
        let n = w.grid().cells();
        println!("== {label} {side}² seed {seed}: mean γ {:.2}", g / k);
        let mut totals = [0usize; TYPES];
        for b in 0..BIOME_COUNT {
            let tiles: Vec<usize> = (0..n).filter(|&i| w.biome(i) as usize == b).collect();
            if tiles.is_empty() {
                continue;
            }
            let mut c = [0usize; TYPES];
            for &i in &tiles {
                match w.state(i) {
                    Cell::Tree => c[w.species(i) as usize] += 1,
                    Cell::Grass => c[SPECIES_COUNT + w.grass_kind(i) as usize] += 1,
                    Cell::Bare => {}
                }
            }
            for t in 0..TYPES {
                totals[t] += c[t];
            }
            let plants: usize = c.iter().sum();
            let mut top: Vec<(usize, usize)> = c.iter().copied().enumerate().filter(|&(_, x)| x > 0).collect();
            top.sort_by(|x, y| y.1.cmp(&x.1));
            println!(
                "   {:<16} {:>5.1}% of map | {}",
                Biome::from_u8(b as u8).name(),
                100.0 * tiles.len() as f64 / n as f64,
                top.iter().take(5).map(|&(t, x)| format!("{} {:.0}%", NAMES[t], 100.0 * x as f64 / plants.max(1) as f64)).collect::<Vec<_>>().join(", ")
            );
        }
        println!("   totals: {}", (0..TYPES).map(|t| format!("{} {}", NAMES[t], totals[t])).collect::<Vec<_>>().join(", "));
    }
}
