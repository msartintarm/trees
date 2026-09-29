//! Render layer: pure math (camera, geometry, scene) tested natively, plus
//! the wasm32-only wgpu `Renderer` in `gpu`.

pub mod camera;
pub mod geometry;
pub mod scene;
pub mod sky;
pub mod surface;

#[cfg(target_arch = "wasm32")]
pub mod gpu;

pub const SCENE_WGSL: &str = include_str!("scene.wgsl");

#[cfg(test)]
mod tests {
    #[test]
    fn scene_wgsl_type_checks() {
        let module = naga::front::wgsl::parse_str(super::SCENE_WGSL)
            .unwrap_or_else(|e| panic!("scene.wgsl parse error:\n{e}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("scene.wgsl validation error:\n{e:?}"));
    }
}
