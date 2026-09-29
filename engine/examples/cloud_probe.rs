//! Dynamic clouds: genus populations, lifecycle events, and where the rain
//! falls (mountains vs lowlands) with cloud dynamics on vs off.
//!
//!     cargo run --release --example cloud_probe -- 128 4000
use tree_engine::sim::world::{CloudKind, Params, World, CLOUD_EVENT_COUNT};

const EVENTS: [&str; CLOUD_EVENT_COUNT] = ["formed", "towered", "collapsed", "front", "broke up", "evaporated", "daughter", "fire cloud"];

fn main() {
    let side: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(128);
    let horizon: u64 = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(4000);
    for (label, dynamics) in [("dynamic", 1.0), ("static", 0.0)] {
        let p = Params { width: side, height: side, cloud_dynamics: dynamics, ..Params::default() };
        let mut w = World::with_params(7, p);
        let n = w.grid().cells();
        let mut rain = vec![0u32; n];
        let mut genus = [0.0f64; 4];
        let mut samples = 0.0;
        for t in 1..=horizon {
            w.step(t);
            for (i, r) in rain.iter_mut().enumerate() {
                if w.wet_ratio(i) > 0.99 {
                    *r += 1;
                }
            }
            if t > 500 {
                for s in w.storms() {
                    genus[s.kind as usize] += 1.0;
                }
                samples += 1.0;
            }
        }
        let amp = (0..n).map(|i| w.altitude(i)).fold(0.0f32, f32::max).max(1e-6);
        let band = |lo: f32, hi: f32| {
            let t: Vec<usize> = (0..n).filter(|&i| w.altitude(i) >= lo * amp && w.altitude(i) < hi * amp && !w.is_channel(i)).collect();
            t.iter().map(|&i| rain[i] as f64).sum::<f64>() / t.len().max(1) as f64
        };
        let ev = w.cloud_events();
        println!(
            "{label}: clouds on map cu {:.1} cb {:.1} ns {:.1} ci {:.1} | rain ticks per tile: lowlands {:.1}, mid {:.1}, mountains {:.1}",
            genus[CloudKind::Cumulus as usize] / samples,
            genus[CloudKind::Cumulonimbus as usize] / samples,
            genus[CloudKind::Nimbostratus as usize] / samples,
            genus[CloudKind::Cirrus as usize] / samples,
            band(0.0, 0.33),
            band(0.33, 0.66),
            band(0.66, 1.01)
        );
        println!("   events: {}", (0..CLOUD_EVENT_COUNT).map(|k| format!("{} {}", EVENTS[k], ev[k])).collect::<Vec<_>>().join(", "));
    }
}
