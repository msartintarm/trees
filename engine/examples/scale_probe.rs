//! Map-size scaling: tick cost, and whether plant and cloud densities per
//! area hold as the map grows (they should — nothing depends on size).
use std::time::Instant;
use tree_engine::render::scene::build_instances;
use tree_engine::sim::world::{Params, World};

fn main() {
    for (w, h) in [(64u32, 64u32), (128, 128), (256, 256), (512, 512)] {
        let p = Params { width: w, height: h, seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() };
        let mut world = World::with_params(7, p);
        let cells = world.grid().cells() as f64;
        let (mut trees, mut grass, mut clouds, mut n) = (0.0, 0.0, 0.0, 0.0);
        let horizon = 3_000u64;
        let t0 = Instant::now();
        for t in 1..=horizon {
            world.step(t);
            if t > 1_500 && t % 50 == 0 {
                let c = world.counts();
                grass += c[1] as f64 / cells;
                trees += c[2] as f64 / cells;
                clouds += world.storms().len() as f64 / (cells / 4096.0);
                n += 1.0;
            }
        }
        let per_tick = t0.elapsed().as_secs_f64() * 1e3 / horizon as f64;
        let t1 = Instant::now();
        for _ in 0..20 {
            std::hint::black_box(build_instances(&world, horizon, 0.5));
        }
        let per_frame = t1.elapsed().as_secs_f64() * 1e3 / 20.0;
        println!(
            "{w}×{h}: {per_tick:.2} ms/tick, {per_frame:.2} ms/frame build | trees {:.1}% grass {:.1}% clouds per 4096 tiles {:.2} | eff {:.2}",
            100.0 * trees / n, 100.0 * grass / n, clouds / n, world.diversity().1
        );
    }
}
