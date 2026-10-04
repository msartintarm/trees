//! Place names: the land divided into named regions — contiguous stretches
//! of one biome, found on a coarse grid so a region is a landscape, not a
//! patch — and names for the landmarks. Names are drawn deterministically
//! from the seed (stream `Places`), so a seed always names its world the
//! same way and naming never perturbs the simulation's random streams.

use super::hex::Grid;
use super::rng::{self, Stream};
use super::terrain::LandmarkKind;
use super::world::{Biome, BIOME_COUNT};

/// Coarse cell side (tiles) for region finding.
const BLOCK: i32 = 8;
/// Regions smaller than this many coarse cells go unnamed.
const MIN_BLOCKS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: String,
    pub biome: Biome,
    /// The tile nearest the region's centroid (where its label sits).
    pub anchor: usize,
    /// Size in tiles.
    pub tiles: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Places {
    /// Region index per tile (u16::MAX = unnamed ground).
    region: Vec<u16>,
    pub regions: Vec<Region>,
}

const SYLLABLES: [&str; 28] = [
    "al", "bar", "cor", "dun", "el", "fen", "gor", "hal", "is", "kel", "lor", "mar", "nor", "os", "pel", "ra", "sel",
    "tor", "ul", "ven", "wen", "yr", "ash", "bri", "cal", "dra", "eth", "mor",
];

/// Adjectives and nouns per biome (Biome order: wetland, tundra, boreal,
/// temperate forest, grassland, savanna, desert).
const ADJECTIVES: [[&str; 6]; BIOME_COUNT] = [
    ["Greywater", "Reed", "Heron", "Mist", "Willow", "Black Pool"],
    ["White", "Frost", "Bare", "Windward", "Lichen", "Long Night"],
    ["Black", "Pine", "Snow", "Grey", "Wolf", "Silent"],
    ["Oak", "Deep", "Green", "Mossy", "Old", "Hollow"],
    ["Wind", "Long", "Silver", "Bluestem", "Lark", "Wide"],
    ["Golden", "Lion", "Red", "Thorn", "Dry", "Ember"],
    ["Amber", "Bone", "Saffron", "Glass", "Copper", "Burning"],
];
const NOUNS: [[&str; 4]; BIOME_COUNT] = [
    ["Fen", "Marsh", "Mire", "Sloughs"],
    ["Fell", "Heights", "Tundra", "Barrens"],
    ["Taiga", "Woods", "Pinewood", "Wilds"],
    ["Wood", "Weald", "Forest", "Holt"],
    ["Prairie", "Downs", "Meadows", "Steppe"],
    ["Plains", "Veld", "Range", "Bushland"],
    ["Erg", "Sands", "Barrens", "Flats"],
];

fn draw(seed: u64, key: u32, salt: u64, n: usize) -> usize {
    (rng::hash(seed, key, salt, Stream::Places) % n as u64) as usize
}

/// A proper name of two or three syllables, e.g. "Kelmar", "Osvenor".
pub fn proper_name(seed: u64, key: u32) -> String {
    let count = 2 + draw(seed, key, 900, 2);
    let mut s = String::new();
    for k in 0..count {
        s.push_str(SYLLABLES[draw(seed, key, 901 + k as u64, SYLLABLES.len())]);
    }
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// A region's name: "the Amber Erg", or "the Kelmar Downs".
fn region_name(seed: u64, key: u32, biome: Biome) -> String {
    let b = biome as usize;
    let noun = NOUNS[b][draw(seed, key, 2, NOUNS[b].len())];
    if draw(seed, key, 3, 3) == 0 {
        format!("the {} {}", proper_name(seed, key), noun)
    } else {
        format!("the {} {}", ADJECTIVES[b][draw(seed, key, 1, ADJECTIVES[b].len())], noun)
    }
}

/// A landmark's name, e.g. "Mount Kelmar's Glacier", "Lake Osvenor".
pub fn landmark_name(seed: u64, kind: LandmarkKind, tile: usize, tree: &str) -> String {
    let p = proper_name(seed, tile as u32 ^ 0x5151);
    match kind {
        LandmarkKind::Glacier => format!("the {p} Glacier"),
        LandmarkKind::CraterLake => format!("Lake {p}"),
        LandmarkKind::Oasis => format!("the {p} Oasis"),
        LandmarkKind::Waterfall => format!("{p} Falls"),
        LandmarkKind::GreatTree => format!("the Elder {tree} of {p}"),
    }
}

impl Places {
    /// Regions from a per-tile biome map.
    pub fn find(seed: u64, grid: Grid, biome: &[Biome]) -> Places {
        let bw = (grid.width + BLOCK - 1) / BLOCK;
        let bh = (grid.height + BLOCK - 1) / BLOCK;
        let nb = (bw * bh) as usize;
        // Majority biome per coarse block.
        let mut block_biome = vec![Biome::Grassland; nb];
        for by in 0..bh {
            for bx in 0..bw {
                let mut votes = [0u32; BIOME_COUNT];
                for row in by * BLOCK..((by + 1) * BLOCK).min(grid.height) {
                    for col in bx * BLOCK..((bx + 1) * BLOCK).min(grid.width) {
                        votes[biome[(row * grid.width + col) as usize] as usize] += 1;
                    }
                }
                let best = (0..BIOME_COUNT).max_by_key(|&k| votes[k]).unwrap();
                block_biome[(by * bw + bx) as usize] = Biome::from_u8(best as u8);
            }
        }
        // Connected components of same-biome blocks (4-neighborhood).
        let mut comp = vec![usize::MAX; nb];
        let mut comps: Vec<Vec<usize>> = Vec::new();
        for start in 0..nb {
            if comp[start] != usize::MAX {
                continue;
            }
            let id = comps.len();
            let mut members = vec![start];
            comp[start] = id;
            let mut k = 0;
            while k < members.len() {
                let b = members[k];
                k += 1;
                let (bx, by) = ((b as i32) % bw, (b as i32) / bw);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (bx + dx, by + dy);
                    if nx < 0 || ny < 0 || nx >= bw || ny >= bh {
                        continue;
                    }
                    let nbk = (ny * bw + nx) as usize;
                    if comp[nbk] == usize::MAX && block_biome[nbk] == block_biome[start] {
                        comp[nbk] = id;
                        members.push(nbk);
                    }
                }
            }
            comps.push(members);
        }
        let mut region_of_comp = vec![u16::MAX; comps.len()];
        let mut regions = Vec::new();
        for (id, members) in comps.iter().enumerate() {
            if members.len() < MIN_BLOCKS {
                continue;
            }
            let b = block_biome[members[0]];
            // Centroid in block space, then the nearest member block's
            // center tile (a crescent's centroid may lie outside it).
            let (sx, sy) = members.iter().fold((0.0, 0.0), |(x, y), &m| (x + (m as i32 % bw) as f64, y + (m as i32 / bw) as f64));
            let (cx, cy) = (sx / members.len() as f64, sy / members.len() as f64);
            let &near = members
                .iter()
                .min_by(|&&a, &&c| {
                    let d = |m: usize| ((m as i32 % bw) as f64 - cx).powi(2) + ((m as i32 / bw) as f64 - cy).powi(2);
                    d(a).partial_cmp(&d(c)).unwrap()
                })
                .unwrap();
            let col = ((near as i32 % bw) * BLOCK + BLOCK / 2).min(grid.width - 1);
            let row = ((near as i32 / bw) * BLOCK + BLOCK / 2).min(grid.height - 1);
            region_of_comp[id] = regions.len() as u16;
            // Names are unique on a map: redraw until unused.
            let mut salt = 0u32;
            let name = loop {
                let n = region_name(seed, id as u32 * 7 + b as u32 + salt * 7919, b);
                if !regions.iter().any(|r: &Region| r.name == n) || salt > 20 {
                    break n;
                }
                salt += 1;
            };
            regions.push(Region {
                name,
                biome: b,
                anchor: (row * grid.width + col) as usize,
                tiles: 0,
            });
        }
        let mut region = vec![u16::MAX; grid.cells()];
        for (i, slot) in region.iter_mut().enumerate() {
            let (col, row) = (i as i32 % grid.width, i as i32 / grid.width);
            let b = ((row / BLOCK) * bw + col / BLOCK) as usize;
            *slot = region_of_comp[comp[b]];
            if *slot != u16::MAX {
                regions[*slot as usize].tiles += 1;
            }
        }
        Places { region, regions }
    }

    /// The named region a tile lies in.
    pub fn region_of(&self, i: usize) -> Option<&Region> {
        self.region.get(i).filter(|&&r| r != u16::MAX).map(|&r| &self.regions[r as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_contiguous_named_and_stable() {
        let g = Grid::new(64, 48);
        // Desert west, wetland east.
        let biome: Vec<Biome> = (0..g.cells())
            .map(|i| if (i as i32 % g.width) < 32 { Biome::Desert } else { Biome::Wetland })
            .collect();
        let a = Places::find(3, g, &biome);
        let b = Places::find(3, g, &biome);
        assert_eq!(a.regions, b.regions, "same seed, same names");
        assert_eq!(a.regions.len(), 2);
        let west = a.region_of(5).unwrap();
        let east = a.region_of(60).unwrap();
        assert_eq!(west.biome, Biome::Desert);
        assert_eq!(east.biome, Biome::Wetland);
        assert!(west.name.starts_with("the "), "{}", west.name);
        assert!(NOUNS[Biome::Desert as usize].iter().any(|n| west.name.ends_with(n)), "{}", west.name);
        assert_eq!(west.tiles + east.tiles, g.cells());
        assert_ne!(Places::find(4, g, &biome).regions[0].name, "", "names exist for any seed");
        // A patchwork of many regions still gets distinct names.
        let quilt: Vec<Biome> = (0..g.cells())
            .map(|i| if ((i as i32 % g.width) / 16 + (i as i32 / g.width) / 16) % 2 == 0 { Biome::Desert } else { Biome::Tundra })
            .collect();
        let q = Places::find(5, g, &quilt);
        let mut names: Vec<&str> = q.regions.iter().map(|r| r.name.as_str()).collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate region names");
    }

    #[test]
    fn landmark_names_read_naturally() {
        let n = landmark_name(1, LandmarkKind::CraterLake, 40, "Oak");
        assert!(n.starts_with("Lake ") && n.len() > 6, "{n}");
        assert!(landmark_name(1, LandmarkKind::GreatTree, 40, "Oak").starts_with("the Elder Oak of "));
    }
}
