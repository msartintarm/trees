//! wasm32-only wgpu renderer. WebGPU when the browser has it, WebGL2
//! otherwise — the same `BROWSER_WEBGPU | GL` routing as the sibling traffic
//! engine, which is why there is no separate 2D fallback.
//!
//! A frame: two shadow cascades from the key light (near: sharp, around the
//! viewer; far: the broad view) → the scene into an HDR target (sky dome,
//! chunk-culled continuous terrain, instanced vegetation and props,
//! translucent precipitation, particles and clouds) → bloom (bright pass +
//! separable blur at half resolution) → tone map and grade to the screen.

use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use super::geometry::{
    bolt_mesh, cap_mesh, column_mesh, flame_mesh, grazer_mesh, wolf_mesh, cirrus_mesh, cumulonimbus_mesh, cumulus_mesh, flower_mesh, grass_mesh_for, house_mesh,
    mushroom_mesh, nimbostratus_mesh, particle_mesh, rain_mesh, root_mesh, shrub_mesh, smoke_mesh, stump_mesh,
    sun_mesh, tree_lod_mesh_for, tree_mesh_for, MeshData, MeshVertex,
};
use super::scene::{stream, Instance, STREAM_COUNT};
use super::surface::TerrainVertex;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_SIZE: u32 = 2048;
/// Floats in the scene uniform block (see `Globals` in scene.wgsl).
pub const GLOBAL_FLOATS: usize = 120;
/// Floats the bridge sends per frame: the globals plus post parameters
/// [bloom strength, exposure, vignette, bright threshold].
pub const UNIFORM_FLOATS: usize = GLOBAL_FLOATS + 4;
/// `render` flags.
pub const FLAG_ROOTS: u32 = 1;
pub const FLAG_BLOOM: u32 = 2;
/// Draw the ground as hex columns instead of the smooth surface.
pub const FLAG_COLUMNS: u32 = 4;

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
        Mesh::from_bytes(device, queue, bytemuck::cast_slice(&data.vertices), &data.indices, label)
    }

    fn from_bytes(device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[u8], indices: &[u32], label: &str) -> Mesh {
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (vertices.len() as u64).max(16).next_multiple_of(4),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !vertices.is_empty() {
            queue.write_buffer(&vbuf, 0, vertices);
        }
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: ((indices.len() * 4) as u64).max(16),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !indices.is_empty() {
            queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(indices));
        }
        Mesh { vertices: vbuf, indices: ibuf, index_count: indices.len() as u32 }
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

/// A culling chunk of the terrain index buffer.
#[derive(Clone, Copy)]
struct Chunk {
    first: u32,
    count: u32,
    min: [f32; 3],
    max: [f32; 3],
}

impl Chunk {
    /// Conservative box-vs-frustum test: culled only when all eight
    /// corners lie outside one clip plane.
    fn visible(&self, m: &[f32]) -> bool {
        let mut out = [0u8; 5];
        for k in 0..8 {
            let p = [
                if k & 1 == 0 { self.min[0] } else { self.max[0] },
                if k & 2 == 0 { self.min[1] } else { self.max[1] },
                if k & 4 == 0 { self.min[2] - 0.5 } else { self.max[2] + 0.5 },
            ];
            let c = |row: usize| m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row];
            let (x, y, w) = (c(0), c(1), c(3));
            out[0] += (x < -w) as u8;
            out[1] += (x > w) as u8;
            out[2] += (y < -w) as u8;
            out[3] += (y > w) as u8;
            out[4] += (w < 0.0) as u8;
        }
        out.iter().all(|&n| n < 8)
    }
}

struct Terrain {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    chunks: Vec<Chunk>,
    cells: u32,
    skirt: Mesh,
}

/// Screen-size targets for the HDR chain.
struct Targets {
    depth: wgpu::TextureView,
    hdr: wgpu::TextureView,
    bloom_a: wgpu::TextureView,
    bloom_b: wgpu::TextureView,
    bright_bg: wgpu::BindGroup,
    blur_h_bg: wgpu::BindGroup,
    blur_v_bg: wgpu::BindGroup,
    composite_bg: wgpu::BindGroup,
    size: [u32; 2],
}

struct PostUniforms {
    bright: wgpu::Buffer,
    blur_h: wgpu::Buffer,
    blur_v: wgpu::Buffer,
    composite: wgpu::Buffer,
}

#[wasm_bindgen]
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    hdr_format: wgpu::TextureFormat,
    sky_pipeline: wgpu::RenderPipeline,
    pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    /// Translucent ground for the roots view.
    xray_pipeline: wgpu::RenderPipeline,
    cloud_pipeline: wgpu::RenderPipeline,
    rain_pipeline: wgpu::RenderPipeline,
    particle_pipeline: wgpu::RenderPipeline,
    animal_pipeline: wgpu::RenderPipeline,
    flame_pipeline: wgpu::RenderPipeline,
    shadow_animal_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_terrain_pipeline: wgpu::RenderPipeline,
    column_pipeline: wgpu::RenderPipeline,
    column_xray_pipeline: wgpu::RenderPipeline,
    shadow_column_pipeline: wgpu::RenderPipeline,
    column: Mesh,
    bright_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    /// Globals for the scene and the near cascade (A), and a copy whose
    /// shadow matrix is the far cascade (B).
    globals_a: wgpu::Buffer,
    globals_b: wgpu::Buffer,
    bind_a: wgpu::BindGroup,
    bind_b: wgpu::BindGroup,
    shadow_bind_group: wgpu::BindGroup,
    shadow_near: wgpu::TextureView,
    shadow_far: wgpu::TextureView,
    post_bgl: wgpu::BindGroupLayout,
    post_sampler: wgpu::Sampler,
    post: PostUniforms,
    targets: Targets,
    terrain: Option<Terrain>,
    /// An identity instance for static meshes (the terrain skirt).
    identity: InstanceBuffer,
    /// One mesh and one instance buffer per stream (`scene::stream`).
    meshes: Vec<Mesh>,
    buffers: Vec<InstanceBuffer>,
    backend: &'static str,
}

#[wasm_bindgen]
impl Renderer {
    pub async fn create(canvas: HtmlCanvasElement) -> Result<Renderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let instance = make_instance().await;
        let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(err)?;
        from_surface(instance, surface, width, height).await
    }

    /// The same renderer on an `OffscreenCanvas`, usable from a Web Worker
    /// after `transferControlToOffscreen()`.
    pub async fn create_offscreen(canvas: web_sys::OffscreenCanvas) -> Result<Renderer, JsValue> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let instance = make_instance().await;
        let surface = instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).map_err(err)?;
        from_surface(instance, surface, width, height).await
    }

    pub fn backend(&self) -> String {
        self.backend.to_string()
    }

    /// Whether the HDR target is a float format (else an 8-bit fallback).
    pub fn hdr(&self) -> bool {
        self.hdr_format == wgpu::TextureFormat::Rgba16Float
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.targets = make_targets(
            &self.device,
            self.hdr_format,
            &self.post_bgl,
            &self.post_sampler,
            &self.post,
            width,
            height,
        );
    }

    /// Upload the terrain: vertex bytes (`surface::TerrainVertex`, one per
    /// tile), chunked indices with their table ([first, count, min xyz,
    /// max xyz] per chunk), and the edge skirt mesh.
    pub fn set_terrain(
        &mut self,
        vertices: Vec<u8>,
        indices: Vec<u32>,
        chunks: Vec<f32>,
        skirt_vertices: Vec<u8>,
        skirt_indices: Vec<u32>,
    ) {
        let v = Mesh::from_bytes(&self.device, &self.queue, &vertices, &indices, "terrain");
        let chunks = chunks
            .chunks_exact(8)
            .map(|c| Chunk { first: c[0] as u32, count: c[1] as u32, min: [c[2], c[3], c[4]], max: [c[5], c[6], c[7]] })
            .collect();
        let skirt = Mesh::from_bytes(&self.device, &self.queue, &skirt_vertices, &skirt_indices, "skirt");
        self.terrain = Some(Terrain {
            vertices: v.vertices,
            indices: v.indices,
            chunks,
            cells: (vertices.len() / std::mem::size_of::<TerrainVertex>()) as u32,
            skirt,
        });
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, mesh: &'a Mesh, inst: &'a InstanceBuffer, n: u32) {
        if n == 0 {
            return;
        }
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_vertex_buffer(1, inst.buf.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..n);
    }

    /// The terrain's visible chunks through `m` (a view-projection), with
    /// this frame's ground stream as its per-vertex data.
    fn draw_terrain<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, m: &[f32], ground_n: u32) {
        let Some(t) = &self.terrain else { return };
        if t.cells != ground_n || ground_n == 0 {
            return; // a reseed changed the map and its terrain isn't up yet
        }
        pass.set_vertex_buffer(0, t.vertices.slice(..));
        pass.set_vertex_buffer(1, self.buffers[stream::GROUND].buf.slice(..));
        pass.set_index_buffer(t.indices.slice(..), wgpu::IndexFormat::Uint32);
        for c in &t.chunks {
            if c.visible(m) {
                pass.draw_indexed(c.first..c.first + c.count, 0, 0..1);
            }
        }
    }

    /// The ground as hex columns: one prism instance per tile, reading the
    /// terrain and ground buffers per instance.
    fn draw_columns<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, ground_n: u32) {
        let Some(t) = &self.terrain else { return };
        if t.cells != ground_n || ground_n == 0 {
            return;
        }
        pass.set_vertex_buffer(0, t.vertices.slice(..));
        pass.set_vertex_buffer(1, self.buffers[stream::GROUND].buf.slice(..));
        pass.set_vertex_buffer(2, self.column.vertices.slice(..));
        pass.set_index_buffer(self.column.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.column.index_count, 0, 0..ground_n);
    }

    /// Draw one frame. `uniforms`: the scene globals (`GLOBAL_FLOATS`, see
    /// scene.wgsl) then [bloom strength, exposure, vignette, threshold].
    /// All instance streams arrive packed in one byte buffer
    /// (`scene::FrameInstances::pack`) with a per-stream count. `flags`:
    /// FLAG_ROOTS (glass ground over the root systems), FLAG_BLOOM.
    pub fn render(&mut self, uniforms: Vec<f32>, bytes: Vec<u8>, counts: Vec<u32>, flags: u32) {
        if uniforms.len() < UNIFORM_FLOATS {
            return;
        }
        let roots_view = flags & FLAG_ROOTS != 0;
        let columns = flags & FLAG_COLUMNS != 0;
        let bloom = flags & FLAG_BLOOM != 0;
        let globals = &uniforms[..GLOBAL_FLOATS];
        self.queue.write_buffer(&self.globals_a, 0, bytemuck::cast_slice(globals));
        // B: the same block with the far cascade as the shadow matrix.
        let mut b = globals.to_vec();
        b.copy_within(48..64, 16);
        self.queue.write_buffer(&self.globals_b, 0, bytemuck::cast_slice(&b));
        let view_proj: Vec<f32> = globals[..16].to_vec();
        let near_vp: Vec<f32> = globals[32..48].to_vec();
        let far_vp: Vec<f32> = globals[48..64].to_vec();
        let grade = [globals[112], globals[113], globals[114], 1.0];
        let p = &uniforms[GLOBAL_FLOATS..UNIFORM_FLOATS];
        let [w, h] = self.targets.size;
        let (hw, hh) = ((w / 2).max(1) as f32, (h / 2).max(1) as f32);
        let post = |step: [f32; 4]| -> [f32; 12] {
            [step[0], step[1], step[2], step[3], if bloom { p[0] } else { 0.0 }, p[1], p[2], p[3], grade[0], grade[1], grade[2], 1.0]
        };
        self.queue.write_buffer(&self.post.bright, 0, bytemuck::cast_slice(&post([1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0])));
        self.queue.write_buffer(&self.post.blur_h, 0, bytemuck::cast_slice(&post([1.0 / hw, 1.0 / hh, 1.0, 0.0])));
        self.queue.write_buffer(&self.post.blur_v, 0, bytemuck::cast_slice(&post([1.0 / hw, 1.0 / hh, 0.0, 1.0])));
        self.queue.write_buffer(&self.post.composite, 0, bytemuck::cast_slice(&post([1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0])));

        let stride = std::mem::size_of::<Instance>();
        let mut offset = 0usize;
        let mut n = [0u32; STREAM_COUNT];
        for k in 0..STREAM_COUNT {
            n[k] = counts.get(k).copied().unwrap_or(0);
            let len = n[k] as usize * stride;
            let end = (offset + len).min(bytes.len());
            self.buffers[k].write(&self.device, &self.queue, &bytes[offset..end], "instances");
            offset = end;
        }

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
        let screen = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // Shadow casters: vegetation and props (grass only in the near
        // cascade, where its shadows read), plus the terrain itself.
        let mut casters: Vec<usize> = stream::TREES.to_vec();
        casters.extend(stream::TREES_LOD);
        casters.extend([stream::MUSHROOMS, stream::STUMPS, stream::HOUSES, stream::SHRUBS]);
        for (cascade, view, bind, m) in [
            (0, &self.shadow_near, &self.bind_a, &near_vp),
            (1, &self.shadow_far, &self.bind_b, &far_vp),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, bind, &[]);
            if !roots_view && columns {
                pass.set_pipeline(&self.shadow_column_pipeline);
                self.draw_columns(&mut pass, n[stream::GROUND]);
            } else if !roots_view {
                pass.set_pipeline(&self.shadow_terrain_pipeline);
                self.draw_terrain(&mut pass, m, n[stream::GROUND]);
            }
            pass.set_pipeline(&self.shadow_pipeline);
            for &k in &casters {
                self.draw(&mut pass, &self.meshes[k], &self.buffers[k], n[k]);
            }
            if cascade == 0 {
                for k in stream::GRASS {
                    self.draw(&mut pass, &self.meshes[k], &self.buffers[k], n[k]);
                }
                pass.set_pipeline(&self.shadow_animal_pipeline);
                for k in [stream::GRAZERS, stream::WOLVES] {
                    self.draw(&mut pass, &self.meshes[k], &self.buffers[k], n[k]);
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.bind_a, &[]);
            pass.set_bind_group(1, &self.shadow_bind_group, &[]);
            // The sky dome behind everything.
            pass.set_pipeline(&self.sky_pipeline);
            pass.draw(0..3, 0..1);
            if !roots_view && columns {
                pass.set_pipeline(&self.column_pipeline);
                self.draw_columns(&mut pass, n[stream::GROUND]);
            } else if !roots_view {
                pass.set_pipeline(&self.terrain_pipeline);
                self.draw_terrain(&mut pass, &view_proj, n[stream::GROUND]);
            }
            pass.set_pipeline(&self.pipeline);
            if let Some(t) = &self.terrain {
                if !roots_view && !columns {
                    self.draw(&mut pass, &t.skirt, &self.identity, 1);
                }
            }
            let mut opaque: Vec<usize> = stream::TREES.to_vec();
            opaque.extend(stream::TREES_LOD);
            opaque.extend(stream::GRASS);
            opaque.extend([
                stream::MUSHROOMS,
                stream::BOLTS,
                stream::STUMPS,
                stream::HOUSES,
                stream::FLOWERS,
                stream::SHRUBS,
            ]);
            if roots_view {
                opaque.push(stream::ROOTS);
            }
            for k in opaque {
                self.draw(&mut pass, &self.meshes[k], &self.buffers[k], n[k]);
            }
            pass.set_pipeline(&self.animal_pipeline);
            for k in [stream::GRAZERS, stream::WOLVES] {
                self.draw(&mut pass, &self.meshes[k], &self.buffers[k], n[k]);
            }
            pass.set_pipeline(&self.pipeline);
            if roots_view {
                // The ground as glass over the roots.
                if columns {
                    pass.set_pipeline(&self.column_xray_pipeline);
                    self.draw_columns(&mut pass, n[stream::GROUND]);
                } else {
                    pass.set_pipeline(&self.xray_pipeline);
                    self.draw_terrain(&mut pass, &view_proj, n[stream::GROUND]);
                }
            }
            // Precipitation shafts and motes, then the translucent cloud
            // volumes, smoke and cap clouds.
            pass.set_pipeline(&self.rain_pipeline);
            self.draw(&mut pass, &self.meshes[stream::RAIN], &self.buffers[stream::RAIN], n[stream::RAIN]);
            pass.set_pipeline(&self.particle_pipeline);
            self.draw(&mut pass, &self.meshes[stream::PARTICLES], &self.buffers[stream::PARTICLES], n[stream::PARTICLES]);
            pass.set_pipeline(&self.flame_pipeline);
            self.draw(&mut pass, &self.meshes[stream::FLAMES], &self.buffers[stream::FLAMES], n[stream::FLAMES]);
            pass.set_pipeline(&self.cloud_pipeline);
            let mut volumes: Vec<usize> = stream::CLOUDS.to_vec();
            volumes.extend([stream::SMOKE, stream::CAP]);
            for k in volumes {
                self.draw(&mut pass, &self.meshes[k], &self.buffers[k], n[k]);
            }
        }

        let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                          target: &wgpu::TextureView,
                          pipeline: &wgpu::RenderPipeline,
                          bind: &wgpu::BindGroup,
                          label: &str| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        };
        if bloom {
            fullscreen(&mut encoder, &self.targets.bloom_a, &self.bright_pipeline, &self.targets.bright_bg, "bright");
            fullscreen(&mut encoder, &self.targets.bloom_b, &self.blur_pipeline, &self.targets.blur_h_bg, "blur-h");
            fullscreen(&mut encoder, &self.targets.bloom_a, &self.blur_pipeline, &self.targets.blur_v_bg, "blur-v");
        }
        fullscreen(&mut encoder, &screen, &self.composite_pipeline, &self.targets.composite_bg, "composite");
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

fn texture(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat, w: u32, h: u32, sampled: bool) -> wgpu::TextureView {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if sampled {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING;
    }
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn make_targets(
    device: &wgpu::Device,
    hdr_format: wgpu::TextureFormat,
    bgl: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    post: &PostUniforms,
    w: u32,
    h: u32,
) -> Targets {
    let depth = texture(device, "depth", DEPTH_FORMAT, w, h, false);
    let hdr = texture(device, "hdr", hdr_format, w, h, true);
    let bloom_a = texture(device, "bloom-a", hdr_format, w / 2, h / 2, true);
    let bloom_b = texture(device, "bloom-b", hdr_format, w / 2, h / 2, true);
    let group = |uniform: &wgpu::Buffer, src: &wgpu::TextureView, extra: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post-bg"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(extra) },
            ],
        })
    };
    // Each pass reads textures it does not write (the unused bloom slot
    // gets whichever texture is idle in that pass).
    let bright_bg = group(&post.bright, &hdr, &bloom_b);
    let blur_h_bg = group(&post.blur_h, &bloom_a, &bloom_a);
    let blur_v_bg = group(&post.blur_v, &bloom_b, &bloom_b);
    let composite_bg = group(&post.composite, &hdr, &bloom_a);
    Targets { depth, hdr, bloom_a, bloom_b, bright_bg, blur_h_bg, blur_v_bg, composite_bg, size: [w, h] }
}

struct PipeSpec<'a> {
    label: &'a str,
    vs: &'a str,
    fs: Option<&'a str>,
    buffers: &'a [wgpu::VertexBufferLayout<'a>],
    format: Option<wgpu::TextureFormat>,
    blend: wgpu::BlendState,
    depth: Option<(wgpu::TextureFormat, bool, wgpu::CompareFunction)>,
    cull: Option<wgpu::Face>,
    bias: bool,
}

fn pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, s: PipeSpec) -> wgpu::RenderPipeline {
    let targets = [s.format.map(|format| wgpu::ColorTargetState {
        format,
        blend: Some(s.blend),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(s.label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(s.vs),
            compilation_options: Default::default(),
            buffers: s.buffers,
        },
        fragment: s.fs.map(|fs| wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: s.cull,
            ..Default::default()
        },
        depth_stencil: s.depth.map(|(format, write, compare)| wgpu::DepthStencilState {
            format,
            depth_write_enabled: write,
            depth_compare: compare,
            stencil: Default::default(),
            bias: if s.bias {
                wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 }
            } else {
                Default::default()
            },
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
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
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(err)?;

    let caps = surface.get_capabilities(&adapter);
    // Prefer a non-sRGB surface: colors are authored in display space, and
    // an sRGB target would re-encode (and visibly brighten) them.
    let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
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

    // HDR where the device can render and filter half floats; an 8-bit
    // target otherwise (bloom then only catches what reaches white).
    let f16 = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba16Float);
    let hdr_format = if f16.allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
        && f16.flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
    {
        wgpu::TextureFormat::Rgba16Float
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene.wgsl"),
        source: wgpu::ShaderSource::Wgsl(super::SCENE_WGSL.into()),
    });
    let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("post.wgsl"),
        source: wgpu::ShaderSource::Wgsl(super::POST_WGSL.into()),
    });

    let uniform = |label: &str, size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let globals_a = uniform("globals-a", (GLOBAL_FLOATS * 4) as u64);
    let globals_b = uniform("globals-b", (GLOBAL_FLOATS * 4) as u64);
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
    let bind = |buf: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
        })
    };
    let bind_a = bind(&globals_a);
    let bind_b = bind(&globals_b);

    // Shadow cascades: two depth textures + a comparison sampler (group 1).
    let shadow_near = texture(&device, "shadow-near", SHADOW_FORMAT, SHADOW_SIZE, SHADOW_SIZE, true);
    let shadow_far = texture(&device, "shadow-far", SHADOW_FORMAT, SHADOW_SIZE, SHADOW_SIZE, true);
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow-sampler"),
        compare: Some(wgpu::CompareFunction::LessEqual),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let depth_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadow-bgl"),
        entries: &[
            depth_entry(0),
            depth_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
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
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&shadow_near) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_far) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
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
        array_stride: std::mem::size_of::<Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![4 => Float32x3, 5 => Float32, 6 => Float32, 7 => Float32x3, 8 => Float32, 9 => Float32x2, 10 => Float32],
    };
    // Terrain: the static per-tile vertex, plus the ground instance stream
    // read per VERTEX (tile i's ground data is vertex i's).
    let terrain_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TerrainVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4],
    };
    let ground_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![5 => Float32x3, 6 => Float32, 7 => Float32, 8 => Float32x3, 9 => Float32, 10 => Float32x2, 11 => Float32],
    };
    // Hex columns: the same two per-tile buffers stepped per instance, and
    // the prism mesh's position and normal per vertex.
    let column_buffers = [
        wgpu::VertexBufferLayout { step_mode: wgpu::VertexStepMode::Instance, ..terrain_layout.clone() },
        wgpu::VertexBufferLayout { step_mode: wgpu::VertexStepMode::Instance, ..ground_layout.clone() },
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![12 => Float32x3, 13 => Float32x3],
        },
    ];
    let mesh_buffers = [vertex_layout.clone(), instance_layout.clone()];
    let terrain_buffers = [terrain_layout.clone(), ground_layout.clone()];
    let hdr = Some(hdr_format);
    let opaque_depth = Some((DEPTH_FORMAT, true, wgpu::CompareFunction::Less));
    let blend_depth = Some((DEPTH_FORMAT, false, wgpu::CompareFunction::Less));
    let spec = |label, vs, fs, buffers, blend, depth, cull| PipeSpec {
        label,
        vs,
        fs: Some(fs),
        buffers,
        format: hdr,
        blend,
        depth,
        cull,
        bias: false,
    };

    let sky_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("sky", "vs_sky", "fs_sky", &[], wgpu::BlendState::REPLACE, Some((DEPTH_FORMAT, false, wgpu::CompareFunction::Always)), None),
    );
    let main_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("scene", "vs_main", "fs_main", &mesh_buffers, wgpu::BlendState::REPLACE, opaque_depth, None),
    );
    let terrain_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("terrain", "vs_terrain", "fs_terrain", &terrain_buffers, wgpu::BlendState::REPLACE, opaque_depth, None),
    );
    let column_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("columns", "vs_column", "fs_terrain", &column_buffers, wgpu::BlendState::REPLACE, opaque_depth, None),
    );
    let column_xray_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("columns-xray", "vs_column", "fs_terrain_xray", &column_buffers, wgpu::BlendState::ALPHA_BLENDING, blend_depth, None),
    );
    let xray_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("xray", "vs_terrain", "fs_terrain_xray", &terrain_buffers, wgpu::BlendState::ALPHA_BLENDING, blend_depth, None),
    );
    // Clouds are closed volumes wound counter-clockwise from outside
    // (checked by geometry::tests): draw only their outer skin, so a
    // translucent heap reads as a solid mass; they write depth (with
    // near-transparent fringes discarded) so a cloud's own opaque core
    // hides what hangs behind it.
    let cloud_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("clouds", "vs_cloud", "fs_cloud", &mesh_buffers, wgpu::BlendState::ALPHA_BLENDING, opaque_depth, Some(wgpu::Face::Back)),
    );
    let rain_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("rain", "vs_rain", "fs_rain", &mesh_buffers, wgpu::BlendState::ALPHA_BLENDING, blend_depth, None),
    );
    let particle_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("particles", "vs_particle", "fs_particle", &mesh_buffers, wgpu::BlendState::ALPHA_BLENDING, blend_depth, None),
    );
    let animal_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("animals", "vs_animal", "fs_main", &mesh_buffers, wgpu::BlendState::REPLACE, opaque_depth, None),
    );
    let flame_pipeline = pipeline(
        &device,
        &layout,
        &shader,
        spec("flames", "vs_flame", "fs_flame", &mesh_buffers, wgpu::BlendState::ALPHA_BLENDING, blend_depth, None),
    );
    let shadow_spec = |label, vs, buffers| PipeSpec {
        label,
        vs,
        fs: None,
        buffers,
        format: None,
        blend: wgpu::BlendState::REPLACE,
        depth: Some((SHADOW_FORMAT, true, wgpu::CompareFunction::Less)),
        cull: None,
        bias: true,
    };
    let shadow_pipeline = pipeline(&device, &shadow_layout, &shader, shadow_spec("shadow", "vs_shadow", &mesh_buffers));
    let shadow_animal_pipeline =
        pipeline(&device, &shadow_layout, &shader, shadow_spec("shadow-animals", "vs_animal_shadow", &mesh_buffers));
    let shadow_terrain_pipeline =
        pipeline(&device, &shadow_layout, &shader, shadow_spec("shadow-terrain", "vs_terrain_shadow", &terrain_buffers));
    let shadow_column_pipeline =
        pipeline(&device, &shadow_layout, &shader, shadow_spec("shadow-columns", "vs_column_shadow", &column_buffers));
    let column = Mesh::upload(&device, &queue, &column_mesh(), "column");

    // Post chain.
    let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("post-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let post_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&post_bgl],
        push_constant_ranges: &[],
    });
    let post_spec = |label, fs, format| PipeSpec {
        label,
        vs: "vs_full",
        fs: Some(fs),
        buffers: &[],
        format: Some(format),
        blend: wgpu::BlendState::REPLACE,
        depth: None,
        cull: None,
        bias: false,
    };
    let bright_pipeline = pipeline(&device, &post_layout, &post_shader, post_spec("bright", "fs_bright", hdr_format));
    let blur_pipeline = pipeline(&device, &post_layout, &post_shader, post_spec("blur", "fs_blur", hdr_format));
    let composite_pipeline = pipeline(&device, &post_layout, &post_shader, post_spec("composite", "fs_composite", format));
    let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("post-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let post = PostUniforms {
        bright: uniform("post-bright", 48),
        blur_h: uniform("post-blur-h", 48),
        blur_v: uniform("post-blur-v", 48),
        composite: uniform("post-composite", 48),
    };
    let targets = make_targets(&device, hdr_format, &post_bgl, &post_sampler, &post, width, height);

    // One mesh per stream, in `scene::stream` order.
    let stream_meshes: Vec<MeshData> = (0..STREAM_COUNT)
        .map(|k| {
            if let Some(sp) = stream::TREES.iter().position(|&t| t == k) {
                return tree_mesh_for(sp);
            }
            if let Some(sp) = stream::TREES_LOD.iter().position(|&t| t == k) {
                return tree_lod_mesh_for(sp);
            }
            if let Some(g) = stream::GRASS.iter().position(|&t| t == k) {
                return grass_mesh_for(g);
            }
            if let Some(c) = stream::CLOUDS.iter().position(|&t| t == k) {
                return [cumulus_mesh(), cumulonimbus_mesh(), nimbostratus_mesh(), cirrus_mesh()][c].clone();
            }
            match k {
                stream::MUSHROOMS => mushroom_mesh(),
                stream::BOLTS => bolt_mesh(),
                stream::ROOTS => root_mesh(),
                stream::SUN => sun_mesh(),
                stream::RAIN => rain_mesh(),
                stream::SMOKE => smoke_mesh(),
                stream::STUMPS => stump_mesh(),
                stream::HOUSES => house_mesh(),
                stream::FLOWERS => flower_mesh(),
                stream::SHRUBS => shrub_mesh(),
                stream::PARTICLES => particle_mesh(),
                stream::FLAMES => flame_mesh(),
                stream::GRAZERS => grazer_mesh(),
                stream::WOLVES => wolf_mesh(),
                stream::CAP => cap_mesh(),
                // The ground draws as the terrain surface, not a mesh.
                _ => MeshData { vertices: Vec::new(), indices: Vec::new() },
            }
        })
        .collect();
    let meshes: Vec<Mesh> = stream_meshes.iter().map(|m| Mesh::upload(&device, &queue, m, "stream-mesh")).collect();
    let buffers: Vec<InstanceBuffer> =
        (0..STREAM_COUNT).map(|_| InstanceBuffer::new(&device, 1024 * 48, "instances")).collect();
    let mut identity = InstanceBuffer::new(&device, 64, "identity");
    let one = [Instance { scale: 1.0, prev_scale: 1.0, color: [1.0; 3], gloss: 3.0, ..Default::default() }];
    identity.write(&device, &queue, bytemuck::cast_slice(&one), "identity");

    Ok(Renderer {
        surface,
        device,
        queue,
        config,
        hdr_format,
        sky_pipeline,
        pipeline: main_pipeline,
        terrain_pipeline,
        xray_pipeline,
        cloud_pipeline,
        rain_pipeline,
        particle_pipeline,
        animal_pipeline,
        flame_pipeline,
        shadow_animal_pipeline,
        shadow_pipeline,
        shadow_terrain_pipeline,
        column_pipeline,
        column_xray_pipeline,
        shadow_column_pipeline,
        column,
        bright_pipeline,
        blur_pipeline,
        composite_pipeline,
        globals_a,
        globals_b,
        bind_a,
        bind_b,
        shadow_bind_group,
        shadow_near,
        shadow_far,
        post_bgl,
        post_sampler,
        post,
        targets,
        terrain: None,
        identity,
        meshes,
        buffers,
        backend,
    })
}
