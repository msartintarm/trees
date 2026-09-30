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
pub const POST_WGSL: &str = include_str!("post.wgsl");

#[cfg(test)]
mod tests {
    fn validate(src: &str, name: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name} parse error:\n{e}"));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} validation error:\n{e:?}"));
        (module, info)
    }

    /// The WebGL2 fallback runs every shader as GLSL ES 3.00: translate
    /// each entry point natively so a construct WebGL can't express fails
    /// here instead of as a blank canvas in the browser.
    #[test]
    fn every_entry_point_translates_to_webgl_glsl() {
        for (src, name) in [(super::SCENE_WGSL, "scene.wgsl"), (super::POST_WGSL, "post.wgsl")] {
            let (module, info) = validate(src, name);
            for ep in &module.entry_points {
                let options = naga::back::glsl::Options {
                    version: naga::back::glsl::Version::Embedded { version: 300, is_webgl: true },
                    ..Default::default()
                };
                let pipeline = naga::back::glsl::PipelineOptions {
                    shader_stage: ep.stage,
                    entry_point: ep.name.clone(),
                    multiview: None,
                };
                let mut out = String::new();
                let mut w = naga::back::glsl::Writer::new(
                    &mut out,
                    &module,
                    &info,
                    &options,
                    &pipeline,
                    naga::proc::BoundsCheckPolicies::default(),
                )
                .unwrap_or_else(|e| panic!("{name}:{}: {e:?}", ep.name));
                w.write().unwrap_or_else(|e| panic!("{name}:{}: {e:?}", ep.name));
            }
        }
    }

    #[test]
    fn post_wgsl_type_checks() {
        validate(super::POST_WGSL, "post.wgsl");
    }

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
