//! Hex-grid ecology engine. The simulation (`sim`) is a pure, dependency-free
//! core that runs and tests natively; render math (`render`) is equally pure,
//! with WGSL shaders validated under `cargo test`. Everything that touches the
//! browser — the wasm-bindgen `Simulation` bridge and the wgpu `Renderer` — is
//! gated behind `target_arch = "wasm32"`, mirroring the sibling `../traffic`
//! project.

pub mod render;
pub mod sim;

#[cfg(target_arch = "wasm32")]
mod bridge;

#[cfg(target_arch = "wasm32")]
pub use bridge::Simulation;
#[cfg(target_arch = "wasm32")]
pub use render::gpu::Renderer;
