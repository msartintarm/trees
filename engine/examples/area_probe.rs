//! Diversity across scales and map sizes: whole-map (γ) vs local 16×16
//! window (α) effective types, turnover β = γ/α, and per-type persistence
//! (minimum count after warmup; 0 = went extinct).
use tree_engine::sim::world::{Cell, Params, World, GRASS_KIND_COUNT, SPECIES_COUNT};

const TYPES: usize = SPECIES_COUNT + GRASS_KIND_COUNT;
const NAMES: [&str; TYPES] = ["acacia", "oak", "pine", "willow", "bunch", "sod", "sedge", "annual"];

fn eff(n: &[u32]) -> f64 {
    let t: u32 = n.iter().sum();
    if t == 0 {
        return 0.0;
    }
    n.iter().filter(|&&c| c > 0).map(|&c| { let p = c as f64 / t as f64; -p * p.ln() }).sum::<f64>().exp()
}

fn census(w: &World) -> (Vec<u32>, f64) {
    let g = w.grid();
    let mut total = vec![0u32; TYPES];
    let win = 16;
    let (mut alpha, mut nwin) = (0.0, 0.0);
    for wy in (0..g.height).step_by(win) {
        for wx in (0..g.width).step_by(win) {
            let mut n = vec![0u32; TYPES];
            for row in wy..(wy + win as i32).min(g.height) {
                for col in wx..(wx + win as i32).min(g.width) {
                    let i = (row * g.width + col) as usize;
                    match w.state(i) {
                        Cell::Tree => n[w.species(i) as usize] += 1,
                        Cell::Grass => n[SPECIES_COUNT + w.grass_kind(i) as usize] += 1,
                        Cell::Bare => {}
                    }
                }
            }
            if n.iter().sum::<u32>() > 0 {
                alpha += eff(&n);
                nwin += 1.0;
            }
            for k in 0..TYPES { total[k] += n[k]; }
        }
    }
    (total, alpha / nwin)
}

fn main() {
    let side: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(64);
    let seeds: Vec<u64> = std::env::args().nth(2).map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or(vec![7, 42, 9, 1234]);
    let horizon: u64 = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    for seed in seeds {
        let p = Params { width: side, height: side, seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() };
        let mut w = World::with_params(seed, p);
        let mut mins = vec![u32::MAX; TYPES];
        let (mut g_sum, mut a_sum, mut n) = (0.0, 0.0, 0.0);
        for t in 1..=horizon {
            w.step(t);
            if t > 3_000 && t % 250 == 0 {
                let (tot, alpha) = census(&w);
                for k in 0..TYPES { mins[k] = mins[k].min(tot[k]); }
                g_sum += eff(&tot);
                a_sum += alpha;
                n += 1.0;
            }
        }
        let (tot, _) = census(&w);
        let cells = w.grid().cells() as f64;
        let lost: Vec<&str> = (0..TYPES).filter(|&k| mins[k] == 0).map(|k| NAMES[k]).collect();
        let rarest = (0..TYPES).min_by_key(|&k| mins[k]).unwrap();
        println!(
            "{side}² seed {seed:>4}: γ {:.2}  α {:.2}  β {:.2} | extinct-at-some-point {:?} | rarest min {} ({}, {:.2}‰ of map) | end %: {}",
            g_sum / n, a_sum / n, (g_sum / n) / (a_sum / n), lost, mins[rarest], NAMES[rarest], 1000.0 * mins[rarest] as f64 / cells,
            (0..TYPES).map(|k| format!("{}={:.1}", NAMES[k], 100.0 * tot[k] as f64 / cells)).collect::<Vec<_>>().join(" ")
        );
    }
}
