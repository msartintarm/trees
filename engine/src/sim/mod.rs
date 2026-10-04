//! Pure simulation core: no GPU, DOM, or wasm dependencies, so `cargo test`
//! runs it natively in well under a second.
//!
//! - `rng` — stateless counter-based randomness (SplitMix64 hash of coordinates)
//! - `clock` — fixed-timestep play/pause/speed clock
//! - `hex` — axial hex math for the 64×64 odd-r offset grid
//! - `world` — cell states and the per-tick ecology rules
//! - `places` — region and landmark names

pub mod clock;
pub mod fauna;
pub mod hex;
pub mod places;
pub mod rng;
pub mod terrain;
pub mod world;
