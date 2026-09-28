//! wasm32-only wgpu renderer: one pipeline drawing three instanced meshes
//! (ground tiles, trees, grass) with a depth buffer. WebGPU when the browser
//! has it, WebGL2 otherwise — the same `BROWSER_WEBGPU | GL` routing as the
//! sibling traffic engine, which is why there is no separate 2D fallback.

use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::sim::hex::Grid;
use super::geometry::{base_mesh, bolt_mesh, cloud_mesh, grass_mesh_for, mushroom_mesh, sheet_mesh, tile_mesh, tree_mesh_for, MeshData, MeshVertex};
use super::scene::Instance;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_SIZE: u32 = 2048;
const SKY: wgpu::Color = wgpu::Color { r: 0.336, g: 0.540, b: 0.629, a: 1.0 };
/// Sun direction (normalized, toward the light).
const SUN: [f32; 3] = [0.36, -0.42, 0.83];

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

struct Mesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

impl Mesh {
    fn upload(device: &wgpu::Device, queue: &wgpu::Queue, data: &MeshData, label: &str) -> Mesh {
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (data.vertices.len() * std::mem::size_of::<MeshVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertices, 0, bytemuck::cast_slice(&data.vertices));
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (data.indices.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&indices, 0, bytemuck::cast_slice(&data.indices));
        Mesh { vertices, indices, index_count: data.indices.len() as u32 }
    }
}

/// A growable per-frame instance buffer.
struct InstanceBuffer {
    buf: wgpu::Buffer,
    capacity: u64,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, capacity: u64, label: &'static str) -> InstanceBuffer {
        InstanceBuffer {
            buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity.max(64),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: capacity.max(64),
        }
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8], label: &'static str) {
        if bytes.len() as u64 > self.capacity {
            *self = InstanceBuffer::new(device, (bytes.len() as u64).next_power_of_two(), label);
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buf, 0, bytes);
        }
    }
}

#[wasm_bindgen]
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    cloud_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    depth: wgpu::TextureView,
    shadow: wgpu::TextureView,
    base: Mesh,
    tile: Mesh,
    trees: [Mesh; 4],
    grass: [Mesh; 4],
    mushroom: Mesh,
    cloud: Mesh,
    sheet: Mesh,
    bolt: Mesh,
    base_instances: InstanceBuffer,
    ground_instances: InstanceBuffer,
    tree_instances: [InstanceBuffer; 4],
    grass_instances: [InstanceBuffer; 4],
    mushroom_instances: InstanceBuffer,
    cloud_instances: InstanceBuffer,
    sheet_instances: InstanceBuffer,
    bolt_instances: InstanceBuffer,
    backend: &'static str,
}

#[wasm_bindgen]
impl Renderer {
    pub async fn create(canvas: HtmlCanvasElement) -> Result<Renderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let instance = make_instance().await;
        let surface =
            instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(err)?;
        from_surface(instance, surface, width, height).await
    }

    /// The same renderer on an `OffscreenCanvas`, usable from a Web Worker
    /// after `transferControlToOffscreen()`.
    pub async fn create_offscreen(canvas: web_sys::OffscreenCanvas) -> Result<Renderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let instance = make_instance().await;
        let surface =
            instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).map_err(err)?;
        from_surface(instance, surface, width, height).await
    }

    pub fn backend(&self) -> String {
        self.backend.to_string()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = make_depth(&self.device, width, height);
    }

    /// Re-fit the soil slab under a world of a new size (after a reseed
    /// that changed the map dimensions).
    pub fn set_grid(&mut self, width: u32, height: u32) {
        self.base = Mesh::upload(&self.device, &self.queue, &base_mesh(Grid::new(width, height)), "base-mesh");
    }

    /// Draw one frame. Instance buffers arrive as raw bytes (bytemuck-cast
    /// `Instance` arrays copied out of the sim) with their instance counts.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        view_proj: Vec<f32>,
        alpha: f32,
        light: f32,
        eye: Vec<f32>,
        light_vp: Vec<f32>,
        ground: Vec<u8>,
        ground_n: u32,
        acacia: Vec<u8>,
        acacia_n: u32,
        oak: Vec<u8>,
        oak_n: u32,
        pine: Vec<u8>,
        pine_n: u32,
        willow: Vec<u8>,
        willow_n: u32,
        bunch: Vec<u8>,
        bunch_n: u32,
        sod: Vec<u8>,
        sod_n: u32,
        sedge: Vec<u8>,
        sedge_n: u32,
        annual: Vec<u8>,
        annual_n: u32,
        mushrooms: Vec<u8>,
        mushrooms_n: u32,
        clouds: Vec<u8>,
        clouds_n: u32,
        sheets: Vec<u8>,
        sheets_n: u32,
        bolts: Vec<u8>,
        bolts_n: u32,
    ) {
        let mut globals = [0f32; 44];
        globals[..16].copy_from_slice(&view_proj[..16]);
        globals[16..32].copy_from_slice(&light_vp[..16]);
        globals[32..35].copy_from_slice(&SUN);
        globals[36] = alpha;
        globals[37] = light;
        globals[40..43].copy_from_slice(&eye[..3]);
        self.queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&globals));

        self.ground_instances.write(&self.device, &self.queue, &ground, "ground-instances");
        for (buf, bytes) in self.tree_instances.iter_mut().zip([&acacia, &oak, &pine, &willow]) {
            buf.write(&self.device, &self.queue, bytes, "tree-instances");
        }
        for (buf, bytes) in self.grass_instances.iter_mut().zip([&bunch, &sod, &sedge, &annual]) {
            buf.write(&self.device, &self.queue, bytes, "grass-instances");
        }
        self.mushroom_instances.write(&self.device, &self.queue, &mushrooms, "mushroom-instances");
        self.cloud_instances.write(&self.device, &self.queue, &clouds, "cloud-instances");
        self.sheet_instances.write(&self.device, &self.queue, &sheets, "sheet-instances");
        self.bolt_instances.write(&self.device, &self.queue, &bolts, "bolt-instances");

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    Ok(f) => f,
                    Err(_) => return,
                }
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            // Pass 1: depth-only shadow map from the sun's view. Terrain
            // columns and vegetation cast — clouds keep their soft analytic
            // shadows.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            for (mesh, inst, n) in [
                (&self.tile, &self.ground_instances, ground_n),
                (&self.trees[0], &self.tree_instances[0], acacia_n),
                (&self.trees[1], &self.tree_instances[1], oak_n),
                (&self.trees[2], &self.tree_instances[2], pine_n),
                (&self.trees[3], &self.tree_instances[3], willow_n),
                (&self.grass[0], &self.grass_instances[0], bunch_n),
                (&self.grass[1], &self.grass_instances[1], sod_n),
                (&self.grass[2], &self.grass_instances[2], sedge_n),
                (&self.grass[3], &self.grass_instances[3], annual_n),
                (&self.mushroom, &self.mushroom_instances, mushrooms_n),
            ] {
                if n == 0 {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, inst.buf.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..n);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            // The sky dims and brightens with the season.
                            r: SKY.r * (0.5 + 0.5 * light as f64),
                            g: SKY.g * (0.5 + 0.5 * light as f64),
                            b: SKY.b * (0.55 + 0.45 * light as f64),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_bind_group(1, &self.shadow_bind_group, &[]);
            for (mesh, inst, n) in [
                (&self.base, &self.base_instances, 1),
                (&self.tile, &self.ground_instances, ground_n),
                (&self.trees[0], &self.tree_instances[0], acacia_n),
                (&self.trees[1], &self.tree_instances[1], oak_n),
                (&self.trees[2], &self.tree_instances[2], pine_n),
                (&self.trees[3], &self.tree_instances[3], willow_n),
                (&self.grass[0], &self.grass_instances[0], bunch_n),
                (&self.grass[1], &self.grass_instances[1], sod_n),
                (&self.grass[2], &self.grass_instances[2], sedge_n),
                (&self.grass[3], &self.grass_instances[3], annual_n),
                (&self.mushroom, &self.mushroom_instances, mushrooms_n),
                (&self.bolt, &self.bolt_instances, bolts_n),
            ] {
                if n == 0 {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, inst.buf.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..n);
            }
            // Translucent clouds last, back to front as seen from above:
            // the low puffy decks, then the high sheets over them.
            pass.set_pipeline(&self.cloud_pipeline);
            for (mesh, inst, n) in [
                (&self.cloud, &self.cloud_instances, clouds_n),
                (&self.sheet, &self.sheet_instances, sheets_n),
            ] {
                if n == 0 {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, inst.buf.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..n);
            }
        }
        self.queue.submit([encoder.finish()]);
        frame.present();
    }
}

/// A `navigator.gpu` object can exist while being unable to create any
/// adapter (headless, blocklisted GPU); plain `Instance::new` would then lock
/// to WebGPU and never try WebGL. This detection variant probes for a real
/// adapter before committing to a backend.
async fn make_instance() -> wgpu::Instance {
    wgpu::util::new_instance_with_webgpu_detection(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        ..Default::default()
    })
    .await
}

fn make_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

async fn from_surface(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Result<Renderer, JsValue> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        })
        .await
        .map_err(err)?;
    let backend = match adapter.get_info().backend {
        wgpu::Backend::BrowserWebGpu => "webgpu",
        wgpu::Backend::Gl => "webgl",
        _ => "other",
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("tree-simulator"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(err)?;

    let caps = surface.get_capabilities(&adapter);
    // Prefer a non-sRGB surface: colors are authored in display space, and
    // an sRGB target would re-encode (and visibly brighten) them.
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene.wgsl"),
        source: wgpu::ShaderSource::Wgsl(super::SCENE_WGSL.into()),
    });

    let globals = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("globals"),
        size: 176,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("globals-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals-bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
    });
    // Shadow map resources: depth texture + comparison sampler (group 1).
    let shadow = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow-map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow-sampler"),
        compare: Some(wgpu::CompareFunction::LessEqual),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadow-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    });
    let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadow-bg"),
        layout: &shadow_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&shadow) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&bgl, &shadow_bgl],
        push_constant_ranges: &[],
    });
    let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });

    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32],
    };
    let instance_layout = wgpu::VertexBufferLayout {
        array_stride: 36,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![4 => Float32x3, 5 => Float32, 6 => Float32, 7 => Float32x3, 8 => Float32],
    };

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[vertex_layout.clone(), instance_layout.clone()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });

    // Clouds are translucent (Beer–Lambert opacity from the fragment):
    // alpha-blended over the opaque scene, depth-tested but not written so
    // the layers behind still show through.
    let cloud_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("clouds"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_cloud"),
            compilation_options: Default::default(),
            buffers: &[vertex_layout.clone(), instance_layout.clone()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_cloud"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });

    let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow"),
        layout: Some(&shadow_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_shadow"),
            compilation_options: Default::default(),
            buffers: &[vertex_layout.clone(), instance_layout.clone()],
        },
        fragment: None,
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: SHADOW_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });

    let depth = make_depth(&device, width, height);
    let base = Mesh::upload(&device, &queue, &base_mesh(Grid::DEFAULT), "base-mesh");
    let tile = Mesh::upload(&device, &queue, &tile_mesh(), "tile-mesh");
    let trees = [
        Mesh::upload(&device, &queue, &tree_mesh_for(0), "acacia-mesh"),
        Mesh::upload(&device, &queue, &tree_mesh_for(1), "oak-mesh"),
        Mesh::upload(&device, &queue, &tree_mesh_for(2), "pine-mesh"),
        Mesh::upload(&device, &queue, &tree_mesh_for(3), "willow-mesh"),
    ];
    let grass = [
        Mesh::upload(&device, &queue, &grass_mesh_for(0), "bunch-mesh"),
        Mesh::upload(&device, &queue, &grass_mesh_for(1), "sod-mesh"),
        Mesh::upload(&device, &queue, &grass_mesh_for(2), "sedge-mesh"),
        Mesh::upload(&device, &queue, &grass_mesh_for(3), "annual-mesh"),
    ];
    let mushroom = Mesh::upload(&device, &queue, &mushroom_mesh(), "mushroom-mesh");
    let cloud = Mesh::upload(&device, &queue, &cloud_mesh(), "cloud-mesh");
    let sheet = Mesh::upload(&device, &queue, &sheet_mesh(), "sheet-mesh");
    let bolt = Mesh::upload(&device, &queue, &bolt_mesh(), "bolt-mesh");
    // The base slab never changes: one identity instance uploaded once.
    let mut base_instances = InstanceBuffer::new(&device, 32, "base-instance");
    let slab = [Instance {
        pos: [0.0; 3],
        scale: 1.0,
        prev_scale: 1.0,
        color: [1.0; 3],
        slim: 0.0,
    }];
    base_instances.write(&device, &queue, bytemuck::cast_slice(&slab), "base-instance");
    let ground_instances = InstanceBuffer::new(&device, 4096 * 32, "ground-instances");
    let tree_instances = [
        InstanceBuffer::new(&device, 512 * 32, "tree-instances"),
        InstanceBuffer::new(&device, 512 * 32, "tree-instances"),
        InstanceBuffer::new(&device, 512 * 32, "tree-instances"),
        InstanceBuffer::new(&device, 512 * 32, "tree-instances"),
    ];
    let grass_instances = [
        InstanceBuffer::new(&device, 1024 * 36, "grass-instances"),
        InstanceBuffer::new(&device, 1024 * 36, "grass-instances"),
        InstanceBuffer::new(&device, 1024 * 36, "grass-instances"),
        InstanceBuffer::new(&device, 1024 * 36, "grass-instances"),
    ];
    let mushroom_instances = InstanceBuffer::new(&device, 512 * 32, "mushroom-instances");
    let cloud_instances = InstanceBuffer::new(&device, 16 * 32, "cloud-instances");
    let sheet_instances = InstanceBuffer::new(&device, 16 * 32, "sheet-instances");
    let bolt_instances = InstanceBuffer::new(&device, 64 * 32, "bolt-instances");

    Ok(Renderer {
        surface,
        device,
        queue,
        config,
        pipeline,
        cloud_pipeline,
        shadow_pipeline,
        globals,
        bind_group,
        shadow_bind_group,
        depth,
        shadow,
        base,
        tile,
        trees,
        grass,
        mushroom,
        cloud,
        sheet,
        bolt,
        base_instances,
        ground_instances,
        tree_instances,
        grass_instances,
        mushroom_instances,
        cloud_instances,
        sheet_instances,
        bolt_instances,
        backend,
    })
}
