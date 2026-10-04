//! Stateless counter-based randomness: every draw is a pure hash of
//! `(seed, cell, tick, stream)`, so the run is reproducible from the seed and
//! the click history alone, independent of iteration order. Mixer is
//! SplitMix64. Copied from the sibling `traffic` engine.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Stream {
    /// Initial tree scatter at world creation.
    Seeding = 0,
    /// Initial grass scatter at world creation.
    SeedingGrass = 1,
    /// The per-tick 1% tree-growth Bernoulli draw.
    TreeGrowth = 2,
    /// The per-tick 4% grass-growth Bernoulli draw.
    GrassGrowth = 3,
    /// Per-tick mortality hazard draw (constant hazard ⇒ geometric
    /// lifetimes: unbounded, but long lives exponentially rare).
    Mortality = 4,
    /// Lightning: spontaneous ignition of a grass tile.
    FireIgnite = 5,
    /// Fire jumping from a burning tile to a flammable neighbor.
    FireSpread = 6,
    /// Self-thinning: extra mortality from crowding by mature neighbors.
    Crowding = 7,
    /// Thundercloud spawning: timing, heading, speed, and size draws.
    StormSpawn = 8,
    /// Lightning bolts thrown by an active storm.
    StormBolt = 9,
    /// Climate phase offsets (drawn once from the seed).
    Climate = 10,
    /// Which species wins a contested tree establishment.
    SpeciesChoice = 11,
    /// Wind field phases and cloud-kind selection.
    Wind = 12,
    /// Heritable-trait mutation on each new tree.
    Mutation = 13,
    /// Storm gusts toppling tall trees.
    Windthrow = 14,
    /// Jays caching acorns across the map.
    Jay = 15,
    /// Synchronized oak mast years.
    Mast = 16,
    /// Root resprouting after a willow dies.
    Resprout = 17,
    /// Terrain generation (the water-table map).
    Terrain = 18,
    /// Specialist pest/pathogen outbreaks (oak wilt, bark beetles).
    Pest = 19,
    /// Browsers (deer) eating saplings.
    Browse = 20,
    /// River flood pulses.
    Flood = 21,
    /// Which trees become long-lived veterans.
    Veteran = 22,
    /// Animal herds and packs.
    Fauna = 23,
    /// Place names and landmark siting.
    Places = 24,
    /// Auto-planting: where, and what.
    AutoPlant = 25,
}

#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[inline]
pub fn hash(seed: u64, cell: u32, tick: u64, stream: Stream) -> u64 {
    let mut key = seed;
    key = key.wrapping_mul(0xD1B5_4A32_D192_ED03).wrapping_add(cell as u64);
    key = key.wrapping_mul(0x00A0_761D_6478_BD64).wrapping_add(tick);
    key = key.wrapping_mul(0xE703_7ED1_A0B4_28DB).wrapping_add(stream as u64);
    mix64(key)
}

#[inline]
pub fn uniform01(seed: u64, cell: u32, tick: u64, stream: Stream) -> f64 {
    (hash(seed, cell, tick, stream) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform01_is_in_unit_interval() {
        for id in 0..10_000u32 {
            let u = uniform01(1234, id, 7, Stream::TreeGrowth);
            assert!((0.0..1.0).contains(&u), "u={u} out of range for id={id}");
        }
    }

    #[test]
    fn same_coordinates_reproduce_exactly() {
        let a = uniform01(42, 99, 3, Stream::GrassGrowth);
        let b = uniform01(42, 99, 3, Stream::GrassGrowth);
        assert_eq!(a.to_bits(), b.to_bits());
    }

    #[test]
    fn streams_are_decorrelated() {
        let a = uniform01(42, 99, 3, Stream::TreeGrowth);
        let b = uniform01(42, 99, 3, Stream::GrassGrowth);
        assert_ne!(a.to_bits(), b.to_bits());
    }

    #[test]
    fn different_cells_and_ticks_differ() {
        let base = uniform01(42, 99, 3, Stream::Mortality);
        assert_ne!(base, uniform01(42, 100, 3, Stream::Mortality));
        assert_ne!(base, uniform01(42, 99, 4, Stream::Mortality));
        assert_ne!(base, uniform01(43, 99, 3, Stream::Mortality));
    }

    #[test]
    fn mean_is_near_a_half() {
        let n = 200_000u32;
        let sum: f64 = (0..n).map(|id| uniform01(7, id, 0, Stream::Seeding)).sum();
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.01, "mean={mean}");
    }
}
