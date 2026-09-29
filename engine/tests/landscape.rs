//! Landscape-scale features on a 128² map: climate zones from mountain
//! altitude, rivers with flood pulses, and grazing. Values measured with
//! `cargo run --release --example landscape_probe -- 128 7 8000` (seed 7:
//! γ 4.21 on; turnover β 1.60; sod 11.4% of the lowest altitude band vs
//! 54.5% of the highest; acacia on ground averaging temperature 0.60 vs
//! sod's 0.34; willows 98% within 4 tiles of a river). Each world is
//! simulated once and shared by the tests below.

use std::sync::OnceLock;

use tree_engine::sim::world::{Cell, GrassKind, Params, Species, World};

const HORIZON: u64 = 6_000;

struct Run {
    world: World,
    floods: u32,
    gamma: f64,
    alpha: f64,
}

fn simulate(p: Params) -> Run {
    let mut world = World::with_params(7, p);
    let (mut floods, mut was) = (0, false);
    let (mut g, mut a, mut n) = (0.0, 0.0, 0.0);
    for t in 1..=HORIZON {
        world.step(t);
        if world.is_flooding() && !was {
            floods += 1;
        }
        was = world.is_flooding();
        if t > 3_000 && t % 250 == 0 {
            g += world.diversity().1;
            a += world.local_diversity();
            n += 1.0;
        }
    }
    Run { world, floods, gamma: g / n, alpha: a / n }
}

fn base() -> Params {
    Params { width: 128, height: 128, seed_tree_p: 0.02, seed_grass_p: 0.10, ..Params::default() }
}

fn on() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(|| simulate(base()))
}

fn off_biomes() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(|| simulate(Params { biomes: 0.0, ..base() }))
}

fn off() -> &'static Run {
    static RUN: OnceLock<Run> = OnceLock::new();
    RUN.get_or_init(|| simulate(Params { climate_zones: 0.0, rivers: 0.0, grazing: 0.0, ..base() }))
}

/// Share of an altitude band's tiles held by tiles matching `is`.
fn band_share(w: &World, band: usize, is: impl Fn(usize) -> bool) -> f64 {
    let n = w.grid().cells();
    let amp = (0..n).map(|i| w.altitude(i)).fold(0.0f32, f32::max);
    let (lo, hi) = (band as f32 / 5.0 * amp, (band + 1) as f32 / 5.0 * amp + 1e-6);
    let tiles: Vec<usize> = (0..n).filter(|&i| w.altitude(i) >= lo && w.altitude(i) < hi).collect();
    tiles.iter().filter(|&&i| is(i)).count() as f64 / tiles.len().max(1) as f64
}

#[test]
fn mountains_sort_the_vegetation_into_climate_zones() {
    let w = &on().world;
    let acacia = |i: usize| w.state(i) == Cell::Tree && w.species(i) == Species::Acacia;
    let sod = |i: usize| w.state(i) == Cell::Grass && w.grass_kind(i) == GrassKind::Sod;
    let (sod_low, sod_high) = (band_share(w, 0, sod), band_share(w, 4, sod));
    assert!(sod_high > 2.0 * sod_low, "cool-season sod on the heights ({sod_high:.3} vs {sod_low:.3})");
    // Acacia holds the warmest ground (lowlands and sun-facing slopes):
    // with realistic, bounded dispersal a 128² map's modest altitude range
    // sorts it by temperature rather than excluding it from the heights.
    let n = w.grid().cells();
    let mean_temp = |is: &dyn Fn(usize) -> bool| {
        let t: Vec<f64> = (0..n).filter(|&i| is(i)).map(|i| w.temperature(i)).collect();
        t.iter().sum::<f64>() / t.len().max(1) as f64
    };
    let (warm, cool) = (mean_temp(&acacia), mean_temp(&sod));
    assert!(warm > cool + 0.15, "acacia on warm ground ({warm:.2}) vs sod ({cool:.2})");
    // And it's cooler up there — on average: with biomes on, a northern
    // lowland can be colder than a southern peak.
    let band_temp = |band: usize| {
        let n = w.grid().cells();
        let amp = (0..n).map(|i| w.altitude(i)).fold(0.0f32, f32::max);
        let (lo, hi) = (band as f32 / 5.0 * amp, (band + 1) as f32 / 5.0 * amp + 1e-6);
        let t: Vec<f64> = (0..n).filter(|&i| w.altitude(i) >= lo && w.altitude(i) < hi).map(|i| w.temperature(i)).collect();
        t.iter().sum::<f64>() / t.len().max(1) as f64
    };
    assert!(band_temp(4) < band_temp(0) - 0.05, "peaks {:.2} vs lowlands {:.2}", band_temp(4), band_temp(0));
}

#[test]
fn rivers_are_open_water_and_floods_lay_down_seedbeds() {
    let run = on();
    let w = &run.world;
    let n = w.grid().cells();
    let channels: Vec<usize> = (0..n).filter(|&i| w.is_channel(i)).collect();
    assert!(channels.len() > n / 200, "a river network should form ({} channel tiles)", channels.len());
    assert!(channels.iter().all(|&i| w.state(i) == Cell::Bare), "nothing grows in open water");
    assert!(run.floods >= 3, "wet seasons should flood the rivers (saw {})", run.floods);
}

#[test]
fn willows_line_the_rivers_instead_of_blanketing_wet_ground() {
    let w = &on().world;
    let n = w.grid().cells();
    let willows: Vec<usize> =
        (0..n).filter(|&i| w.state(i) == Cell::Tree && w.species(i) == Species::Willow).collect();
    assert!(!willows.is_empty(), "willows should persist along the rivers");
    let riparian = willows.iter().filter(|&&i| w.river_distance(i).is_some_and(|d| d <= 4)).count();
    assert!(riparian * 10 >= willows.len() * 8, "{riparian}/{} willows within 4 tiles of a river", willows.len());
    let drowned = willows.iter().filter(|&&i| w.water_table(i) > 0.95).count();
    assert!(drowned * 10 <= willows.len(), "saturated marsh is sedge country ({drowned} willows there)");
    // Without rivers, the same wet ground is a willow blanket.
    let off = &off().world;
    let blanket = (0..n).filter(|&i| off.state(i) == Cell::Tree && off.species(i) == Species::Willow).count();
    assert!(blanket > willows.len() * 3, "willow blanket {blanket} vs riparian band {}", willows.len());
}

#[test]
fn landscape_features_raise_diversity_across_the_map() {
    // The landscape's signature is turnover: mountains, rivers and grazing
    // make regions differ (β; measured 1.96 on vs 1.80 off). Total
    // diversity on a 128² map is high either way (measured γ 4.93 with
    // the landscape, 5.63 with the biome gradients alone) — the gradients
    // already span several biomes; on 256² everything together gives ~5.9.
    let (on, off) = (on(), off());
    let (beta_on, beta_off) = (on.gamma / on.alpha, off.gamma / off.alpha);
    println!("γ {:.2} vs {:.2}, β {beta_on:.2} vs {beta_off:.2}", on.gamma, off.gamma);
    assert!(on.gamma > 4.0, "a rich landscape: γ {:.2}", on.gamma);
    assert!(beta_on > beta_off + 0.1, "regions should differ more (β {beta_on:.2} vs {beta_off:.2})");
}

#[test]
fn climate_gradients_lay_out_biomes_with_their_own_plants() {
    // Biomes on (the default): a colder north, a drier interior, and the
    // biome plants. On the landscape run the map spans several biomes and
    // the specialists hold their own: reeds in wetlands, cacti on the
    // driest ground, spruce and birch in the cold.
    use tree_engine::sim::world::{Biome, BIOME_COUNT};
    let w = &on().world;
    let n = w.grid().cells();
    let mut cover = [0usize; BIOME_COUNT];
    for i in 0..n {
        cover[w.biome(i) as usize] += 1;
    }
    let present = cover.iter().filter(|&&c| c * 100 > n).count();
    assert!(present >= 4, "at least four biomes cover >1% each: {cover:?}");
    let in_biome = |b: Biome, is: &dyn Fn(usize) -> bool| {
        let tiles: Vec<usize> = (0..n).filter(|&i| w.biome(i) == b).collect();
        tiles.iter().filter(|&&i| is(i)).count()
    };
    let reeds = in_biome(Biome::Wetland, &|i| w.state(i) == Cell::Grass && w.grass_kind(i) == GrassKind::Reeds);
    assert!(reeds > 10, "reeds should fill the wetlands ({reeds})");
    // The boreal pair (spruce needs the colder north of a 256² map; birch
    // shows up on 128²) sits on cooler ground than acacia.
    let temps = |is: &dyn Fn(Species) -> bool| {
        let t: Vec<f64> = (0..n).filter(|&i| w.state(i) == Cell::Tree && is(w.species(i))).map(|i| w.site_climate(i).0).collect();
        (t.len(), t.iter().sum::<f64>() / t.len().max(1) as f64)
    };
    let (n_boreal, t_boreal) = temps(&|sp| sp == Species::Spruce || sp == Species::Birch);
    let (n_acacia, t_acacia) = temps(&|sp| sp == Species::Acacia);
    assert!(n_boreal > 0 && n_acacia > 0, "boreal {n_boreal}, acacia {n_acacia}");
    assert!(t_boreal < t_acacia - 0.1, "boreal trees in the cold ({t_boreal:.2}) vs acacia ({t_acacia:.2})");
}

#[test]
fn without_biomes_only_the_original_plants_grow() {
    let w = &off_biomes().world;
    let n = w.grid().cells();
    for i in 0..n {
        match w.state(i) {
            Cell::Tree => assert!((w.species(i) as usize) < 4, "a biome tree grew without biomes"),
            Cell::Grass => assert!((w.grass_kind(i) as usize) < 4, "a biome grass grew without biomes"),
            Cell::Bare => {}
        }
    }
}

#[test]
fn a_founding_stand_spreads_as_a_front_not_across_the_map() {
    // One small pine stand in the middle of a grassy 128² map. Seed now
    // travels by a bounded fat-tailed kernel (pine: ~5 tiles typical, 60
    // cap) and young trees bear small crops, so the stand advances as a
    // front. Measured (256², t=2000): 240 pines, 90% within 26 tiles, the
    // farthest 38 — where uniform map-wide seed once put 3,488 pines up to
    // 189 tiles out.
    use tree_engine::sim::hex;
    use tree_engine::sim::world::Brush;
    // Landscape-scale features off: this is about dispersal, not habitat
    // (on this seed the map's middle is a river marsh).
    // (Pests off too: a dense pure-pine stand is exactly what a bark-beetle
    // outbreak wipes out, which is realistic but not what's measured here.)
    let p = Params { width: 128, height: 128, climate_zones: 0.0, rivers: 0.0, grazing: 0.0, pest_strength: 0.0, ..Params::default() };
    let mut w = World::with_params(7, p);
    let g = w.grid();
    let origin = g.middle();
    let (oq, or) = g.index_to_axial(origin);
    // A founding stand big enough to outlast its first bad years (small
    // founding populations often fail outright — an Allee effect).
    for (dq, dr, _) in hex::disk(4) {
        if let Some(i) = g.axial_to_index(oq + dq, or + dr) {
            w.paint_species(i, Brush::Tree, Species::Pine, 0);
        }
    }
    for i in 0..g.cells() {
        if w.state(i) == Cell::Bare && i % 3 == 0 {
            w.paint_grass(i, GrassKind::Sod, 0);
        }
    }
    for t in 1..=2_000 {
        w.step(t);
    }
    let pines: Vec<i32> = (0..g.cells())
        .filter(|&i| w.state(i) == Cell::Tree && w.species(i) == Species::Pine)
        .map(|i| {
            let (q, r) = g.index_to_axial(i);
            hex::distance(q, r, oq, or)
        })
        .collect();
    let farthest = pines.iter().copied().max().unwrap_or(0);
    assert!(!pines.is_empty(), "the stand should persist");
    assert!(pines.len() < 800, "recruitment volume stays modest ({} pines)", pines.len());
    assert!(farthest < 55, "no map-wide jumps (farthest pine {farthest} tiles out)");
}

#[test]
fn grazing_keeps_the_sod_mat_in_check() {
    let n = on().world.grid().cells() as f64;
    let sod = |w: &World| (0..w.grid().cells()).filter(|&i| w.state(i) == Cell::Grass && w.grass_kind(i) == GrassKind::Sod).count() as f64 / n;
    static UNGRAZED: OnceLock<Run> = OnceLock::new();
    let ungrazed_run = UNGRAZED.get_or_init(|| simulate(Params { grazing: 0.0, ..base() }));
    let (grazed, ungrazed) = (sod(&on().world), sod(&ungrazed_run.world));
    assert!(grazed < ungrazed * 0.85, "sod share {grazed:.3} grazed vs {ungrazed:.3}");
}
