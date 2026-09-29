//! Landscape-scale features on a big map: where each plant type lives
//! (altitude, distance to river, groundwater), willow's place along the
//! channels, floods, and diversity at local (α) and whole-map (γ) scale —
//! with the features on vs off.
//!
//!     cargo run --release --example landscape_probe -- 256 7 8000
use tree_engine::sim::world::{Cell, DeathCause, Params, World, DEATH_CAUSE_COUNT, GRASS_KIND_COUNT, SPECIES_COUNT};

const TYPES: usize = SPECIES_COUNT + GRASS_KIND_COUNT;
const NAMES: [&str; TYPES] = ["acacia", "oak", "pine", "willow", "spruce", "birch", "creosote", "bunch", "sod", "sedge", "annual", "reeds", "cactus"];

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let side: u32 = a.get(1).and_then(|x| x.parse().ok()).unwrap_or(128);
    let seed: u64 = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(7);
    let horizon: u64 = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(8000);
    let base = Params { width: side, height: side, seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() };
    for (label, p) in [
        ("features ON", base),
        ("no physiology", Params { physiology: 0.0, seasons: 0.0, ..base }),
        ("features OFF", Params { climate_zones: 0.0, rivers: 0.0, grazing: 0.0, physiology: 0.0, seasons: 0.0, ..base }),
    ] {
        if a.get(4).is_some_and(|x| x == "on") && label.contains("OFF") {
            continue;
        }
        let mut w = World::with_params(seed, p);
        let n = w.grid().cells();
        let (mut g, mut al, mut k, mut floods, mut was) = (0.0, 0.0, 0.0, 0, false);
        for t in 1..=horizon {
            w.step(t);
            if w.is_flooding() && !was { floods += 1; }
            was = w.is_flooding();
            if t > 3000 && t % 250 == 0 {
                g += w.diversity().1;
                al += w.local_diversity();
                k += 1.0;
            }
        }
        let channels = (0..n).filter(|&i| w.is_channel(i)).count();
        let snow = (0..n).filter(|&i| w.temperature(i) < 0.13).count();
        println!("== {label} {side}² seed {seed}: γ {:.2} α {:.2} β {:.2} | channels {:.1}% | floods {floods} | snowy tiles {:.1}%",
            g / k, al / k, g / al, 100.0 * channels as f64 / n as f64, 100.0 * snow as f64 / n as f64);
        let deaths = w.deaths_total();
        let total: u32 = deaths.iter().sum();
        println!("   tree deaths by cause: {}", (0..DEATH_CAUSE_COUNT)
            .filter(|&k| deaths[k] > 0)
            .map(|k| format!("{} {:.0}%", DeathCause::from_u8(k as u8).name(), 100.0 * deaths[k] as f64 / total.max(1) as f64))
            .collect::<Vec<_>>().join(", "));
        for sp in 0..SPECIES_COUNT {
            let r: Vec<f32> = (0..n).filter(|&i| w.state(i) == Cell::Tree && w.species(i) as usize == sp).map(|i| w.reserve(i)).collect();
            if !r.is_empty() {
                print!("   {} reserve {:.2} (n {})", NAMES[sp], r.iter().sum::<f32>() / r.len() as f32, r.len());
            }
        }
        println!();
        // Standing husks by species and cause: who is dying of what.
        let mut husks = [[0u32; DEATH_CAUSE_COUNT]; SPECIES_COUNT];
        for i in 0..n {
            if let Some(r) = w.remains(i) {
                if r.tree {
                    husks[r.species as usize][r.cause as usize] += 1;
                }
            }
        }
        for sp in 0..SPECIES_COUNT {
            let t: u32 = husks[sp].iter().sum();
            if t > 0 {
                println!("   {} husks: {}", NAMES[sp], (0..DEATH_CAUSE_COUNT).filter(|&k| husks[sp][k] > 0)
                    .map(|k| format!("{} {}", DeathCause::from_u8(k as u8).name(), husks[sp][k])).collect::<Vec<_>>().join(", "));
            }
        }
        // Zonation: composition by altitude band (share of each band's tiles).
        let amp = (0..n).map(|i| w.altitude(i)).fold(0.0f32, f32::max).max(1e-6);
        println!("   altitude band  {}", NAMES.iter().map(|s| format!("{s:>7}")).collect::<String>());
        for band in 0..5 {
            let (lo, hi) = (band as f32 / 5.0 * amp, (band + 1) as f32 / 5.0 * amp + 1e-6);
            let tiles: Vec<usize> = (0..n).filter(|&i| w.altitude(i) >= lo && w.altitude(i) < hi).collect();
            let mut c = [0usize; TYPES];
            for &i in &tiles {
                match w.state(i) {
                    Cell::Tree => c[w.species(i) as usize] += 1,
                    Cell::Grass => c[SPECIES_COUNT + w.grass_kind(i) as usize] += 1,
                    Cell::Bare => {}
                }
            }
            let tm = tiles.iter().map(|&i| w.temperature(i)).sum::<f64>() / tiles.len().max(1) as f64;
            println!("   {:.2}-{:.2} t{:.2} {}", lo, hi, tm,
                c.iter().map(|&x| format!("{:>6.1}%", 100.0 * x as f64 / tiles.len().max(1) as f64)).collect::<String>());
        }
        for t in 0..TYPES {
            let tiles: Vec<usize> = (0..n).filter(|&i| match w.state(i) {
                Cell::Tree => t < SPECIES_COUNT && w.species(i) as usize == t,
                Cell::Grass => t >= SPECIES_COUNT && w.grass_kind(i) as usize == t - SPECIES_COUNT,
                Cell::Bare => false,
            }).collect();
            if tiles.is_empty() { println!("   {:<7} extinct", NAMES[t]); continue; }
            let m = |f: &dyn Fn(usize) -> f64| tiles.iter().map(|&i| f(i)).sum::<f64>() / tiles.len() as f64;
            let near = tiles.iter().filter(|&&i| w.river_distance(i).is_some_and(|d| d <= 4)).count();
            println!("   {:<7} {:>5.1}%  altitude {:.2}  temp {:.2}  groundwater {:.2}  ≤4 from river {:>3.0}%",
                NAMES[t], 100.0 * tiles.len() as f64 / n as f64,
                m(&|i| w.altitude(i) as f64), m(&|i| w.temperature(i)), m(&|i| w.water_table(i) as f64),
                100.0 * near as f64 / tiles.len() as f64);
        }
    }
}
