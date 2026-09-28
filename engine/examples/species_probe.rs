//! Per-species behavior: solo niches and mixed-stand outcomes.
use tree_engine::sim::hex;
use tree_engine::sim::world::{Brush, Cell, Params, Species, World, SPECIES_COUNT};

const NAMES: [&str; 4] = ["Acacia", "Oak", "Pine", "Willow"];

struct Stat { n: u32, age_sum: u64, age_max: u64, old: u32, etiol: f64, fert: f64, vig: f64, hard: f64, wt: f64 }

fn census(w: &World, tick: u64) -> Vec<Stat> {
    let mut v: Vec<Stat> = (0..SPECIES_COUNT).map(|_| Stat { n: 0, age_sum: 0, age_max: 0, old: 0, etiol: 0.0, fert: 0.0, vig: 0.0, hard: 0.0, wt: 0.0 }).collect();
    for i in 0..hex::Grid::LEGACY.cells() {
        if w.state(i) != Cell::Tree { continue; }
        let s = &mut v[w.species(i) as usize];
        let a = w.age(i, tick);
        s.n += 1; s.age_sum += a; s.age_max = s.age_max.max(a);
        if a > 750 { s.old += 1; }
        s.etiol += w.etiolation(i) as f64;
        s.fert += w.nutrient_ratio(i) as f64;
        let g = w.genome(i);
        s.vig += g[0] as f64;
        s.hard += g[1] as f64;
        s.wt += w.water_table(i) as f64;
    }
    v
}

fn run(label: &str, p: Params, plant: &dyn Fn(&mut World), seed: u64) {
    let mut w = World::with_params(seed, p);
    for i in 0..hex::Grid::LEGACY.cells() { w.paint(i, Brush::Clear, 0); }
    plant(&mut w);
    let mut peak = [0u32; 4];
    let mut sums = [0u64; 4];
    let mut samples = 0u64;
    let horizon = 12_000u64;
    for t in 1..=horizon {
        w.step(t);
        if t % 100 == 0 {
            let c = census(&w, t);
            for k in 0..4 { peak[k] = peak[k].max(c[k].n); if t > 4000 { sums[k] += c[k].n as u64; } }
            if t > 4000 { samples += 1; }
        }
    }
    let c = census(&w, horizon);
    let [_, g, _] = w.counts();
    println!("== {label} (seed {seed}) grass={g}");
    for k in 0..4 {
        if peak[k] == 0 { continue; }
        let s = &c[k];
        let n = s.n.max(1) as f64;
        println!("  {:<7} mean {:>5} peak {:>5} final {:>5} | age {:>4.0} max {:>5} old:{:>3} | slim {:.2} soil {:.2} wt {:.2} | vigor {:.3} hardy {:.3}",
            NAMES[k], sums[k] / samples.max(1), peak[k], s.n, s.age_sum as f64 / n, s.age_max, s.old, s.etiol / n, s.fert / n, s.wt / n, s.vig / n, s.hard / n);
    }
}

fn main() {
    let d = Params::legacy_map();
    // Grass sward + a species' founders in the middle.
    let solo = |sp: Species| move |w: &mut World| {
        for i in 0..hex::Grid::LEGACY.cells() { if i % 7 == 0 { w.paint(i, Brush::Grass, 0); } }
        for k in 0..12 { w.paint_species(1500 + k * 97, Brush::Tree, sp, 0); }
    };
    let mixed = |w: &mut World| {
        for i in 0..hex::Grid::LEGACY.cells() { if i % 7 == 0 { w.paint(i, Brush::Grass, 0); } }
        for k in 0..48 { w.paint_species(300 + k * 79, Brush::Tree, Species::from_u8((k % 4) as u8), 0); }
    };
    for (env, p) in [
        ("savanna", d),
        ("wet forest", Params { tree_growth_p: 0.01, ..d }),
        ("fire", Params { fire_ignition_p: 0.0005, ..d }),
        ("harsh climate", Params { climate_swing: 1.0, ..d }),
    ] {
        for sp in [Species::Acacia, Species::Oak, Species::Pine, Species::Willow] {
            run(&format!("{env} / solo {:?}", sp), p, &solo(sp), 42);
        }
        run(&format!("{env} / mixed"), p, &mixed, 42);
        println!();
    }
}
