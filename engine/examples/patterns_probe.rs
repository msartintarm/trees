//! The order layers on vs off: how clumped the trees are (a join-count
//! aggregation index: tree–tree neighbor pairs over the random
//! expectation), veteran ages, diversity, the designed map's biome split
//! across the rain shadow, landmarks and region names, animals, and the
//! landscape events.
//!
//!     cargo run --release --example patterns_probe -- 128 7 4000
use tree_engine::sim::hex;
use tree_engine::sim::world::{Biome, BiomeEvent, Cell, Params, World, BIOME_COUNT};

fn aggregation(w: &World) -> f64 {
    // Stand-scale clumping: mean tree density within 3 tiles of a tree,
    // over the map's tree density (1 = random; > 1 = trees in stands).
    let g = w.grid();
    let n = g.cells();
    let trees = (0..n).filter(|&i| w.state(i) == Cell::Tree).count() as f64;
    let p = trees / n as f64;
    let disk = hex::disk(3);
    let (mut near, mut cnt) = (0.0f64, 0.0f64);
    for i in 0..n {
        if w.state(i) != Cell::Tree {
            continue;
        }
        let (q, r) = g.index_to_axial(i);
        let (mut t, mut m) = (0.0f64, 0.0f64);
        for &(dq, dr, d) in &disk {
            if d == 0 {
                continue;
            }
            if let Some(j) = g.axial_to_index(q + dq, r + dr) {
                m += 1.0;
                t += (w.state(j) == Cell::Tree) as u8 as f64;
            }
        }
        near += t / m.max(1.0);
        cnt += 1.0;
    }
    (near / cnt.max(1.0)) / p.max(1e-9)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let side: u32 = a.get(1).and_then(|x| x.parse().ok()).unwrap_or(128);
    let seed: u64 = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(7);
    let horizon: u64 = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(4000);
    let base = Params { width: side, height: side, seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() };
    let off = Params { patterns: 0.0, map_design: 0.0, events: 0.0, fauna: 0.0, ..base };
    let only = |f: fn(&mut Params)| {
        let mut p = off;
        f(&mut p);
        p
    };
    let variants = [
        ("order ON", base),
        ("order OFF", off),
        ("patterns only", only(|p| p.patterns = 1.0)),
        ("design only", only(|p| p.map_design = 1.0)),
        ("fauna only", only(|p| p.fauna = 1.0)),
    ];
    let pick = a.get(4).cloned().unwrap_or_default();
    for (label, p) in variants {
        if !pick.is_empty() && !label.starts_with(pick.as_str()) {
            continue;
        }
        let mut w = World::with_params(seed, p);
        let n = w.grid().cells();
        let (mut g, mut k, mut herds, mut packs, mut animals, mut wolves) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let mut oldest = 0u64;
        for t in 1..=horizon {
            w.step(t);
            if t > horizon / 2 && t % 100 == 0 {
                g += w.diversity().1;
                k += 1.0;
                let f = w.fauna();
                herds += f.herds.len() as f64;
                packs += f.packs.len() as f64;
                animals += f.herds.iter().map(|h| h.size).sum::<f64>();
                wolves += f.packs.iter().map(|p| p.size).sum::<f64>();
            }
        }
        for i in 0..n {
            if w.state(i) == Cell::Tree {
                oldest = oldest.max(w.age(i, horizon));
            }
        }
        let veterans = (0..n).filter(|&i| w.is_veteran(i, horizon)).count();
        let trees = (0..n).filter(|&i| w.state(i) == Cell::Tree).count();
        println!("== {label} ({side}², seed {seed}, {horizon} ticks)");
        println!(
            "  trees {trees} · aggregation {:.2} (1 = random) · oldest {oldest} · veterans {veterans} · γ {:.2}",
            aggregation(&w),
            g / k
        );
        // Biomes by fifth of the way inland (the designed map's coast is on
        // a seed-chosen side; read it from the landmarks' wind via biome
        // shares on each side).
        let mut by_fifth = [[0usize; BIOME_COUNT]; 5];
        let gw = w.grid().width;
        for i in 0..n {
            let col = (i as i32 % gw) as usize * 5 / gw as usize;
            by_fifth[col.min(4)][w.biome(i) as usize] += 1;
        }
        for (f, row) in by_fifth.iter().enumerate() {
            let tot: usize = row.iter().sum();
            let pct = |b: Biome| 100.0 * row[b as usize] as f64 / tot as f64;
            println!(
                "  fifth {f}: desert {:.0}% savanna {:.0}% grass {:.0}% temperate {:.0}% boreal {:.0}% tundra {:.0}% wetland {:.0}%",
                pct(Biome::Desert),
                pct(Biome::Savanna),
                pct(Biome::Grassland),
                pct(Biome::TemperateForest),
                pct(Biome::Boreal),
                pct(Biome::Tundra),
                pct(Biome::Wetland)
            );
        }
        for (kind, tile, name) in w.landmarks() {
            println!("  landmark {kind:?} at {tile}: {name}");
        }
        let names: Vec<&str> = w.regions().iter().map(|r| r.name.as_str()).take(8).collect();
        println!("  regions ({}): {}", w.regions().len(), names.join(", "));
        println!(
            "  fauna (mean, 2nd half): herds {:.1} ({:.0} animals) · packs {:.1} ({:.0} wolves)",
            herds / k,
            animals / k,
            packs / k,
            wolves / k
        );
        let e = w.biome_events();
        println!(
            "  events: superblooms {} · wildfires {} · beetle waves {} · floods {} (latest bloom {:?})",
            e[0],
            e[1],
            e[2],
            e[3],
            w.biome_event_at(BiomeEvent::Superbloom)
        );
    }
}
