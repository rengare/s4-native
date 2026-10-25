//! An offscreen wgpu renderer for what the game's hardware renderer asks of Direct3D 7: lists of
//! pre-transformed, lit, textured triangles (screen-space positions, a diffuse and a specular colour,
//! one set of texture coordinates), alpha-blended, no depth buffer.
//!
//! The renderer knows nothing about the game or about DirectDraw: the host (`s4-host`) keeps the
//! guest's surfaces, converts their 16-bit pixels to RGBA8, and calls [`Gpu::begin_scene`],
//! [`Gpu::draw`] and [`Gpu::end_scene`], which returns the rendered target as RGBA8 to be copied back
//! into the guest's surface.

use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

mod remote;
pub use remote::RemoteGpu;

/// RGBA8 pixels of one row of a 16-bit surface (`src` is `w * 2` bytes, `dst` `w * 4`); `fmt` 0: 5-6-5, 1: 1-5-5-5, 2: 4-4-4-4.
pub fn convert_row(fmt: u32, src: &[u8], dst: &mut [u8]) {
    // Eight pixels at a time, in a shape the compiler turns into vector code.
    #[inline(always)]
    fn go(src: &[u8], dst: &mut [u8], convert: impl Fn(u32) -> u32) {
        let (src8, src_rest) = src.as_chunks::<16>();
        let (dst8, dst_rest) = dst.as_chunks_mut::<32>();
        for (sp, dp) in src8.iter().zip(dst8.iter_mut()) {
            let mut px = [0u32; 8];
            for i in 0..8 {
                px[i] = convert(u16::from_le_bytes([sp[2 * i], sp[2 * i + 1]]) as u32);
            }
            for i in 0..8 {
                dp[4 * i..4 * i + 4].copy_from_slice(&px[i].to_le_bytes());
            }
        }
        for (d, p) in dst_rest.chunks_exact_mut(4).zip(src_rest.chunks_exact(2)) {
            d.copy_from_slice(&convert(u16::from_le_bytes([p[0], p[1]]) as u32).to_le_bytes());
        }
    }
    let e5 = |v: u32| (v << 3) | (v >> 2);
    let e6 = |v: u32| (v << 2) | (v >> 4);
    let e4 = |v: u32| v * 17;
    match fmt {
        1 => go(src, dst, |p| e5((p >> 10) & 31) | e5((p >> 5) & 31) << 8 | e5(p & 31) << 16 | (p >> 15) * 0xff00_0000),
        2 => go(src, dst, |p| e4((p >> 8) & 15) | e4((p >> 4) & 15) << 8 | e4(p & 15) << 16 | e4(p >> 12) << 24),
        _ => go(src, dst, |p| e5(p >> 11) | e6((p >> 5) & 63) << 8 | e5(p & 31) << 16 | 0xff00_0000),
    }
}

/// RGBA8 pixels of packed rows (`w * 2` bytes each) of a 16-bit surface.
pub fn convert_packed(fmt: u32, data: &[u8], w: u32) -> Vec<u8> {
    let mut out = vec![0u8; data.len() * 2];
    for (s, d) in data.chunks_exact((w * 2) as usize).zip(out.chunks_exact_mut((w * 4) as usize)) {
        convert_row(fmt, s, d);
    }
    out
}

/// A pre-transformed vertex as Direct3D 7 calls it `D3DTLVERTEX` (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rhw: f32,
    /// `D3DCOLOR` 0xAARRGGBB.
    pub diffuse: u32,
    pub specular: u32,
    pub u: f32,
    pub v: f32,
}

/// The render state of one batch of triangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawState {
    pub texture: Option<TextureId>,
    /// `ALPHABLENDENABLE` with `SRCALPHA` / `INVSRCALPHA`; off draws opaque.
    pub blend: bool,
    pub linear: bool,
    /// Scissor rectangle (x, y, width, height) in target pixels, or the whole target.
    pub clip: Option<(u32, u32, u32, u32)>,
}

pub type TextureId = u32;

/// Statistics: render passes submitted, and how many of those a texture write forced in the middle of a scene.
pub static PASSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static TEXTURE_FLUSHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub type TargetId = u32;

static NEXT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// A fresh id for a texture or target (ids are unique across all of them, so a [`RemoteGpu`] can hand them out before the
/// renderer thread has made the object).
pub fn alloc_id() -> u32 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

struct Texture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    /// Bind groups for the point and linear samplers.
    groups: [Option<wgpu::BindGroup>; 2],
}

struct Target {
    size: (u32, u32),
    /// Render scale (supersampling factor): the render texture is `size * scale`.
    scale: u32,
    render: wgpu::Texture,
    view: wgpu::TextureView,
    /// Where a scaled picture is reduced to `size` (only for scale > 1).
    resolve: Option<wgpu::Texture>,
    readback: wgpu::Buffer,
    padded_row: u32,
    /// The scene in progress.
    scene: Option<Scene>,
    /// A texture that shows this target's picture (made on first use), for drawing one target onto another.
    alias: Option<TextureId>,
    /// Staging buffers for [`Gpu::present_async`], each with a flag that is set while the worker still reads it.
    ring: Vec<(wgpu::Buffer, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
}

/// Receives a finished picture from [`Gpu::present_async`]: width, height, pixels 0x00RRGGBB.
pub type PresentSink = std::sync::Arc<dyn Fn(u32, u32, Vec<u32>) + Send + Sync>;

struct PresentJob {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    size: (u32, u32),
    padded_row: u32,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    sink: PresentSink,
}

#[derive(Default)]
struct Scene {
    /// Pixels to start from (RGBA8, `size.0 * size.1 * 4`); taken when the first batch is submitted.
    initial: Option<Vec<u8>>,
    /// Some batch of this scene has been submitted already (later ones draw over its result).
    started: bool,
    vertices: Vec<Vertex>,
    draws: Vec<(DrawState, std::ops::Range<u32>)>,
    /// Bounding box (x0, y0, x1, y1) of everything drawn in this scene, in target pixels.
    dirty: Option<(u32, u32, u32, u32)>,
}

/// A part of a target's picture.
pub struct Region {
    /// x, y, width, height in target pixels.
    pub rect: (u32, u32, u32, u32),
    /// RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub adapter_name: String,
    pipeline_blend: wgpu::RenderPipeline,
    pipeline_opaque: wgpu::RenderPipeline,
    pipeline_copy: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    samplers: [wgpu::Sampler; 2],
    white: TextureId,
    textures: HashMap<TextureId, Texture>,
    targets: HashMap<TargetId, Target>,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    uniform_buffer: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    /// The thread that reads presented pictures back (started on first use).
    presenter: Option<std::sync::mpsc::Sender<PresentJob>>,
}

const SHADER: &str = r#"
struct Uniforms { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) diffuse: vec4<f32>,
    @location(1) specular: vec4<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) p: vec4<f32>, @location(1) d: vec4<f32>, @location(2) s: vec4<f32>, @location(3) uv: vec2<f32>) -> VOut {
    var o: VOut;
    // Direct3D 7 puts the centre of pixel (i, j) at (i, j); here it is at (i + 0.5, j + 0.5).
    let x = (p.x + 0.5) / u.size.x * 2.0 - 1.0;
    let y = 1.0 - (p.y + 0.5) / u.size.y * 2.0;
    o.pos = vec4<f32>(x, y, 0.5, 1.0);
    o.diffuse = d;
    o.specular = s;
    o.uv = uv;
    return o;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, i.uv);
    // Default texture stage: colour = texture * diffuse, alpha = texture alpha; the specular colour is added.
    return vec4<f32>(t.rgb * i.diffuse.rgb + i.specular.rgb, t.a);
}

struct FsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FsOut {
    var o: FsOut;
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment
fn fs_copy(i: FsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
"#;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Name of the graphics adapter [`Gpu::new`] would use, without opening a device (a cheap check that wgpu has one).
pub fn probe() -> Result<String, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map(|a| a.get_info().name)
    .map_err(|e| format!("no graphics adapter: {e}"))
}

impl Gpu {
    /// Opens a device (Vulkan, Metal, DX12 or GL, whichever the machine has).
    pub fn new() -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no graphics adapter: {e}"))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("cannot open the graphics device: {e}"))?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("s4 shader"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("layout"), bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)], immediate_size: 0 });
        let attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4Bgra, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4Bgra, offset: 20, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 3 },
        ];
        let make = |blend: Option<wgpu::BlendState>, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline_blend = make(Some(wgpu::BlendState::ALPHA_BLENDING), "blend");
        let pipeline_opaque = make(None, "opaque");
        let pipeline_copy = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("copy"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_copy"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = |filter: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let samplers = [sampler(wgpu::FilterMode::Nearest), sampler(wgpu::FilterMode::Linear)];
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("uniforms"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() }],
        });
        let vertex_capacity = 1 << 20;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("vertices"), size: vertex_capacity, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut gpu = Gpu {
            device,
            queue,
            adapter_name,
            presenter: None,
            pipeline_blend,
            pipeline_opaque,
            pipeline_copy,
            texture_layout,
            samplers,
            white: 0,
            textures: HashMap::new(),
            targets: HashMap::new(),
            vertex_buffer,
            vertex_capacity,
            uniform_buffer,
            uniform_group,
        };
        let white = gpu.create_texture(1, 1);
        gpu.write_texture(white, &[255, 255, 255, 255]);
        gpu.white = white;
        Ok(gpu)
    }

    pub fn create_texture(&mut self, w: u32, h: u32) -> TextureId {
        let id = alloc_id();
        self.create_texture_as(id, w, h);
        id
    }

    pub fn create_texture_as(&mut self, id: TextureId, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.insert(id, Texture { texture, view, size: (w, h), groups: [None, None] });
    }

    pub fn destroy_texture(&mut self, id: TextureId) {
        // Batched draws that use the texture are drawn while it still exists.
        self.flush_users(id);
        self.textures.remove(&id);
    }

    /// Replaces the whole texture with RGBA8 pixels (`w * h * 4` bytes).
    pub fn write_texture(&mut self, id: TextureId, rgba: &[u8]) {
        let Some(t) = self.textures.get(&id) else { return };
        let (w, h) = t.size;
        self.write_texture_region(id, (0, 0, w, h), rgba);
    }

    /// Draws what the scenes have collected so far that uses the texture.
    fn flush_users(&mut self, id: TextureId) {
        let users: Vec<TargetId> = self.targets.iter().filter(|(_, t)| t.scene.as_ref().is_some_and(|s| s.draws.iter().any(|(d, _)| d.texture == Some(id)))).map(|(&k, _)| k).collect();
        for u in users {
            TEXTURE_FLUSHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.submit_draws(u);
        }
    }

    /// Replaces a rectangle (x, y, width, height) of the texture with RGBA8 pixels.
    pub fn write_texture_region(&mut self, id: TextureId, rect: (u32, u32, u32, u32), rgba: &[u8]) {
        // Draws already recorded must see the old pixels, as they would on a real device that executes in order.
        self.flush_users(id);
        let Some(t) = self.textures.get(&id) else { return };
        let (x, y, w, h) = rect;
        assert_eq!(rgba.len(), (w * h * 4) as usize, "texture data size");
        assert!(x + w <= t.size.0 && y + h <= t.size.1, "texture region");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// A render target of `w` x `h`, drawn at `scale` times that size and reduced when read back (`scale` 1 = none).
    pub fn create_target(&mut self, w: u32, h: u32, scale: u32) -> TargetId {
        let id = alloc_id();
        self.create_target_as(id, w, h, scale);
        id
    }

    pub fn create_target_as(&mut self, id: TargetId, w: u32, h: u32, scale: u32) {
        let (w, h, scale) = (w.max(1), h.max(1), scale.max(1));
        let make = |label: &str, w: u32, h: u32| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let render = make("target", w * scale, h * scale);
        let view = render.create_view(&wgpu::TextureViewDescriptor::default());
        let resolve = (scale > 1).then(|| make("resolve", w, h));
        let padded_row = (w * 4 + 255) & !255;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.targets.insert(id, Target { size: (w, h), scale, render, view, resolve, readback, padded_row, scene: None, alias: None, ring: Vec::new() });
    }

    pub fn destroy_target(&mut self, id: TargetId) {
        if let Some(t) = self.targets.remove(&id) {
            if let Some(a) = t.alias {
                self.textures.remove(&a);
            }
        }
    }

    /// A texture that shows the target's picture, to draw it onto another target ([`Gpu::blit`]).
    pub fn target_texture(&mut self, id: TargetId) -> Option<TextureId> {
        self.target_texture_as(id, alloc_id())
    }

    /// As [`Gpu::target_texture`]; `alias` is the id a new alias texture gets (the target's existing alias otherwise).
    pub fn target_texture_as(&mut self, id: TargetId, alias: TextureId) -> Option<TextureId> {
        let t = self.targets.get(&id)?;
        if let Some(a) = t.alias {
            return Some(a);
        }
        let (texture, view, size) = (t.render.clone(), t.render.create_view(&wgpu::TextureViewDescriptor::default()), (t.size.0 * t.scale, t.size.1 * t.scale));
        self.textures.insert(alias, Texture { texture, view, size, groups: [None, None] });
        self.targets.get_mut(&id)?.alias = Some(alias);
        Some(alias)
    }

    /// Copies the rectangle `src_rect` (x, y, width, height) of a texture onto the target at `dst`, at once and in order with the
    /// other drawing. `blend` composites by the texture's alpha (a colour key made transparent), else the pixels are copied.
    pub fn blit(&mut self, target: TargetId, texture: TextureId, src_rect: (u32, u32, u32, u32), dst: (u32, u32), blend: bool) {
        let Some(&Texture { size: (tw, th), .. }) = self.textures.get(&texture).map(|t| &*t) else { return };
        // What the source target has drawn so far must be in it before it is read.
        if let Some(src) = self.targets.iter().find(|(_, t)| t.alias == Some(texture)).map(|(&k, _)| k) {
            self.submit_draws(src);
        }
        // Joins the scene's batch (opening a scene when there is none); it reaches the GPU with the rest of the batch.
        if !self.in_scene(target) {
            self.begin_scene(target, None);
        }
        let (sx, sy, w, h) = src_rect;
        // Texel centres: a pixel of the quad samples the middle of its texel.
        let (u0, v0) = ((sx as f32 + 0.5) / tw as f32, (sy as f32 + 0.5) / th as f32);
        let (u1, v1) = ((sx as f32 + w as f32 + 0.5) / tw as f32, (sy as f32 + h as f32 + 0.5) / th as f32);
        let (x0, y0, x1, y1) = (dst.0 as f32, dst.1 as f32, (dst.0 + w) as f32, (dst.1 + h) as f32);
        let v = |x, y, u, v| Vertex { x, y, z: 0.5, rhw: 1.0, diffuse: 0xffff_ffff, specular: 0, u, v };
        let quad = [v(x0, y0, u0, v0), v(x1, y0, u1, v0), v(x0, y1, u0, v1), v(x1, y0, u1, v0), v(x1, y1, u1, v1), v(x0, y1, u0, v1)];
        self.draw(target, DrawState { texture: Some(texture), blend, linear: false, clip: None }, &quad);
    }

    /// Finishes the scene without reading anything back (the picture stays on the GPU).
    pub fn end_scene_lazy(&mut self, id: TargetId) {
        if self.targets.get(&id).is_some_and(|t| t.scene.is_some()) {
            self.submit_draws(id);
            if let Some(t) = self.targets.get_mut(&id) {
                t.scene = None;
            }
        }
    }

    /// Starts copying the target's whole picture to the host in the background; `sink` gets it when it is ready, in order.
    /// Returns false (and sends nothing) when every staging buffer is still busy or the target is supersampled.
    pub fn present_async(&mut self, id: TargetId, sink: &PresentSink) -> bool {
        self.submit_draws(id);
        let Some(t) = self.targets.get_mut(&id) else { return false };
        if t.scale != 1 {
            return false;
        }
        let (w, h, padded_row) = (t.size.0, t.size.1, t.padded_row);
        if t.ring.is_empty() {
            for _ in 0..3 {
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("present"), size: (padded_row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
                t.ring.push((buffer, Default::default()));
            }
        }
        let Some((buffer, busy)) = t.ring.iter().find(|(_, b)| !b.load(std::sync::atomic::Ordering::Acquire)).map(|(a, b)| (a.clone(), b.clone())) else { return false };
        busy.store(true, std::sync::atomic::Ordering::Release);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.render, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));
        let job = PresentJob { device: self.device.clone(), buffer, size: (w, h), padded_row, busy, sink: sink.clone() };
        let tx = self.presenter.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<PresentJob>();
            std::thread::Builder::new()
                .name("gpu-present".into())
                .spawn(move || {
                    for job in rx {
                        let (w, h) = job.size;
                        let slice = job.buffer.slice(..(job.padded_row * h) as u64);
                        let (done_tx, done_rx) = std::sync::mpsc::channel();
                        slice.map_async(wgpu::MapMode::Read, move |r| {
                            let _ = done_tx.send(r);
                        });
                        let _ = job.device.poll(wgpu::PollType::wait_indefinitely());
                        if done_rx.recv().ok().and_then(|r| r.ok()).is_some() {
                            if let Ok(data) = slice.get_mapped_range() {
                                let mut pixels = Vec::with_capacity((w * h) as usize);
                                for row in 0..h as usize {
                                    let r = &data[row * job.padded_row as usize..row * job.padded_row as usize + (w * 4) as usize];
                                    // The surface is 16-bit: keep what it could show (5-6-5 bits per colour), as the software path does.
                                    pixels.extend(r.chunks_exact(4).map(|p| {
                                        let (r, g, b) = ((p[0] >> 3) as u32, (p[1] >> 2) as u32, (p[2] >> 3) as u32);
                                        ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2)
                                    }));
                                }
                                drop(data);
                                job.buffer.unmap();
                                job.busy.store(false, std::sync::atomic::Ordering::Release);
                                (job.sink)(w, h, pixels);
                                continue;
                            }
                        }
                        job.busy.store(false, std::sync::atomic::Ordering::Release);
                    }
                })
                .expect("present thread");
            tx
        });
        tx.send(job).is_ok()
    }

    pub fn target_size(&self, id: TargetId) -> Option<(u32, u32)> {
        self.targets.get(&id).map(|t| t.size)
    }

    /// Starts a scene on a target. `initial` is the picture to draw over (RGBA8); without it the scene draws over what the
    /// target holds from the last scene (transparent black at first).
    pub fn begin_scene(&mut self, id: TargetId, initial: Option<Vec<u8>>) {
        // Blits batched outside a scene are drawn first.
        self.submit_draws(id);
        if let Some(t) = self.targets.get_mut(&id) {
            t.scene = Some(Scene { initial, ..Default::default() });
        }
    }

    /// Replaces a rectangle (x, y, width, height) of an unscaled target's picture with RGBA8 pixels, outside a scene.
    /// Returns false (and does nothing) for a supersampled target.
    pub fn write_target(&mut self, id: TargetId, rect: (u32, u32, u32, u32), rgba: &[u8]) -> bool {
        self.submit_draws(id);
        let Some(t) = self.targets.get(&id) else { return false };
        let (x, y, w, h) = rect;
        if t.scale != 1 || t.scene.as_ref().is_some_and(|s| s.initial.is_some()) || x + w > t.size.0 || y + h > t.size.1 || rgba.len() != (w * h * 4) as usize {
            return false;
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t.render, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        true
    }

    pub fn in_scene(&self, id: TargetId) -> bool {
        self.targets.get(&id).is_some_and(|t| t.scene.is_some())
    }

    /// Adds triangles (a list of three vertices each) to the scene.
    pub fn draw(&mut self, id: TargetId, state: DrawState, vertices: &[Vertex]) {
        if !self.targets.get(&id).is_some_and(|t| t.scene.is_some()) {
            return;
        }
        let (w, h) = (self.targets[&id].size.0, self.targets[&id].size.1);
        let scene = self.targets.get_mut(&id).and_then(|t| t.scene.as_mut()).unwrap();
        let mut b = (u32::MAX, u32::MAX, 0u32, 0u32);
        for v in vertices {
            b.0 = b.0.min(v.x.floor().max(0.0) as u32);
            b.1 = b.1.min(v.y.floor().max(0.0) as u32);
            b.2 = b.2.max((v.x.ceil() + 1.0).max(0.0) as u32);
            b.3 = b.3.max((v.y.ceil() + 1.0).max(0.0) as u32);
        }
        if let Some((cx, cy, cw, ch)) = state.clip {
            b = (b.0.max(cx), b.1.max(cy), b.2.min(cx + cw), b.3.min(cy + ch));
        }
        b = (b.0.min(w), b.1.min(h), b.2.min(w), b.3.min(h));
        if b.0 < b.2 && b.1 < b.3 {
            scene.dirty = Some(match scene.dirty {
                Some(d) => (d.0.min(b.0), d.1.min(b.1), d.2.max(b.2), d.3.max(b.3)),
                None => b,
            });
        }
        let start = scene.vertices.len() as u32;
        scene.vertices.extend_from_slice(vertices);
        let end = scene.vertices.len() as u32;
        match scene.draws.last_mut() {
            Some((s, r)) if *s == state => r.end = end,
            _ => scene.draws.push((state, start..end)),
        }
    }

    fn bind_group(&mut self, id: TextureId, linear: bool) {
        let Some(t) = self.textures.get_mut(&id) else { return };
        let k = linear as usize;
        if t.groups[k].is_none() {
            t.groups[k] = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("texture"),
                layout: &self.texture_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[k]) }],
            }));
        }
    }

    fn texture_group(&self, view: &wgpu::TextureView, linear: bool) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("temporary"),
            layout: &self.texture_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[linear as usize]) }],
        })
    }

    fn upload(&self, texture: &wgpu::Texture, rgba: &[u8], w: u32, h: u32) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Draws what the scene has collected so far into the target (no read-back); the scene stays open.
    fn submit_draws(&mut self, id: TargetId) {
        let Some(scene) = self.targets.get_mut(&id).and_then(|t| t.scene.as_mut()) else { return };
        if scene.draws.is_empty() && (scene.started || scene.initial.is_none()) {
            return;
        }
        // Batched draws of other targets that sample this target's picture must see it as it is now, before it changes.
        if let Some(alias) = self.targets[&id].alias {
            let readers: Vec<TargetId> = self.targets.iter().filter(|(k, t)| **k != id && t.scene.as_ref().is_some_and(|s| s.draws.iter().any(|(d, _)| d.texture == Some(alias)))).map(|(&k, _)| k).collect();
            for r in readers {
                self.submit_draws(r);
            }
        }
        let Some(scene) = self.targets.get_mut(&id).and_then(|t| t.scene.as_mut()) else { return };
        PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let draws = std::mem::take(&mut scene.draws);
        let vertices = std::mem::take(&mut scene.vertices);
        let initial = scene.initial.take();
        let started = std::mem::replace(&mut scene.started, true);
        let (w, h, scale) = {
            let t = &self.targets[&id];
            (t.size.0, t.size.1, t.scale)
        };
        // Resources the draws need.
        let needed: Vec<(TextureId, bool)> = draws.iter().map(|(s, _)| (s.texture.filter(|t| self.textures.contains_key(t)).unwrap_or(self.white), s.linear)).collect();
        for &(tex, linear) in &needed {
            self.bind_group(tex, linear);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&vertices);
        if bytes.len() as u64 > self.vertex_capacity {
            self.vertex_capacity = (bytes.len() as u64).next_power_of_two();
            self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("vertices"), size: self.vertex_capacity, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.vertex_buffer, 0, bytes);
        }
        let (rw, rh) = (w * scale, h * scale);
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[w as f32, h as f32, 0.0f32, 0.0f32]));
        let target = &self.targets[&id];
        // The picture to start from: written straight into an unscaled target, or copied up through a temporary texture.
        let mut initial_group: Option<wgpu::BindGroup> = None;
        let mut keep_alive: Option<wgpu::Texture> = None;
        let mut load = wgpu::LoadOp::Load;
        if !started {
            if let Some(initial) = &initial {
                load = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
                if scale == 1 {
                    self.upload(&target.render, initial, w, h);
                    load = wgpu::LoadOp::Load;
                } else {
                    let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("initial"),
                        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: FORMAT,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    self.upload(&tex, initial, w, h);
                    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                    initial_group = Some(self.texture_group(&view, false));
                    keep_alive = Some(tex);
                }
            }
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scene") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target.view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.uniform_group, &[]);
            if let Some(group) = &initial_group {
                pass.set_pipeline(&self.pipeline_copy);
                pass.set_bind_group(1, group, &[]);
                pass.draw(0..3, 0..1);
            }
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            // The vertices are in 1x pixels; the shader maps them onto the (possibly larger) target.
            for (i, (state, range)) in draws.iter().enumerate() {
                if range.is_empty() {
                    continue;
                }
                pass.set_pipeline(if state.blend { &self.pipeline_blend } else { &self.pipeline_opaque });
                let (tex, linear) = needed[i];
                pass.set_bind_group(1, self.textures[&tex].groups[linear as usize].as_ref().unwrap(), &[]);
                match state.clip {
                    Some((x, y, cw, ch)) => {
                        let (x, y) = (x.min(w), y.min(h));
                        let (cw, ch) = (cw.min(w - x), ch.min(h - y));
                        if cw == 0 || ch == 0 {
                            continue;
                        }
                        pass.set_scissor_rect(x * scale, y * scale, cw * scale, ch * scale);
                    }
                    None => pass.set_scissor_rect(0, 0, rw, rh),
                }
                pass.draw(range.clone(), 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        drop(keep_alive.take());
    }

    /// Finishes the scene and returns what it changed (`None` when there was no scene; an empty region when nothing was drawn
    /// over a picture the host already has).
    pub fn end_scene(&mut self, id: TargetId) -> Option<Region> {
        self.targets.get(&id)?.scene.as_ref()?;
        self.submit_draws(id);
        let scene = self.targets.get_mut(&id)?.scene.take()?;
        // The host's copy of the picture equals what the scene started from, so only what was drawn has to go back.
        let Some(d) = scene.dirty else { return Some(Region { rect: (0, 0, 0, 0), rgba: Vec::new() }) };
        let rect = (d.0, d.1, d.2 - d.0, d.3 - d.1);
        let target = &self.targets[&id];
        let (rx, ry, rw, rh) = rect;
        let padded_row = (rw * 4 + 255) & !255;
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("read-back") });
        // Reduce a supersampled picture to the target's size (linear filtering averages 2x2 blocks).
        let read_from: &wgpu::Texture = if let Some(resolve) = &target.resolve {
            let resolve_view = resolve.create_view(&wgpu::TextureViewDescriptor::default());
            let group = self.texture_group(&target.view, true);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &resolve_view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline_copy);
            pass.set_bind_group(0, &self.uniform_group, &[]);
            pass.set_bind_group(1, &group, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            resolve
        } else {
            &target.render
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: read_from, mip_level: 0, origin: wgpu::Origin3d { x: rx, y: ry, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &target.readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(rh) } },
            wgpu::Extent3d { width: rw, height: rh, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = target.readback.slice(..(padded_row * rh) as u64);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((rw * rh * 4) as usize);
        for row in 0..rh {
            let s = (row * padded_row) as usize;
            out.extend_from_slice(&data[s..s + (rw * 4) as usize]);
        }
        drop(data);
        target.readback.unmap();
        Some(Region { rect, rgba: out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x0: f32, y0: f32, x1: f32, y1: f32, color: u32, uv: (f32, f32, f32, f32)) -> Vec<Vertex> {
        let v = |x, y, u, v| Vertex { x, y, z: 0.5, rhw: 1.0, diffuse: color, specular: 0, u, v };
        vec![v(x0, y0, uv.0, uv.1), v(x1, y0, uv.2, uv.1), v(x0, y1, uv.0, uv.3), v(x1, y0, uv.2, uv.1), v(x1, y1, uv.2, uv.3), v(x0, y1, uv.0, uv.3)]
    }

    fn state() -> DrawState {
        DrawState { texture: None, blend: false, linear: false, clip: None }
    }

    fn gpu() -> Option<Gpu> {
        match Gpu::new() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("skipped: {e}");
                None
            }
        }
    }

    /// The whole picture of a `w` x `h` target from a region (outside it: transparent black).
    fn full(r: &Region, w: u32, h: u32) -> Vec<u8> {
        let mut img = vec![0u8; (w * h * 4) as usize];
        let (rx, ry, rw, rh) = r.rect;
        for y in 0..rh {
            let d = (((ry + y) * w + rx) * 4) as usize;
            img[d..d + (rw * 4) as usize].copy_from_slice(&r.rgba[(y * rw * 4) as usize..((y + 1) * rw * 4) as usize]);
        }
        img
    }

    fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn a_coloured_quad_covers_its_pixels() {
        let Some(mut g) = gpu() else { return };
        let t = g.create_target(16, 16, 1);
        g.begin_scene(t, None);
        g.draw(t, state(), &quad(2.0, 2.0, 10.0, 10.0, 0xff_ff_80_00, (0.0, 0.0, 1.0, 1.0)));
        let r = g.end_scene(t).unwrap();
        assert_eq!(r.rect, (2, 2, 9, 9), "the box of what was drawn, a pixel wider than the quad");
        let img = full(&r, 16, 16);
        assert_eq!(px(&img, 16, 5, 5), [255, 128, 0, 255]);
        assert_eq!(px(&img, 16, 14, 14), [0, 0, 0, 0], "outside stays as it was");
    }

    #[test]
    fn texture_is_modulated_by_the_diffuse_colour() {
        let Some(mut g) = gpu() else { return };
        let tex = g.create_texture(2, 2);
        g.write_texture(tex, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
        let t = g.create_target(8, 8, 1);
        g.begin_scene(t, None);
        g.draw(t, DrawState { texture: Some(tex), ..state() }, &quad(0.0, 0.0, 8.0, 8.0, 0xff_ff_ff_ff, (0.0, 0.0, 1.0, 1.0)));
        let img = full(&g.end_scene(t).unwrap(), 8, 8);
        assert_eq!(px(&img, 8, 1, 1), [255, 0, 0, 255]);
        assert_eq!(px(&img, 8, 6, 1), [0, 255, 0, 255]);
        assert_eq!(px(&img, 8, 1, 6), [0, 0, 255, 255]);
        assert_eq!(px(&img, 8, 6, 6), [255, 255, 255, 255]);
    }

    #[test]
    fn alpha_blending_mixes_with_the_picture_below() {
        let Some(mut g) = gpu() else { return };
        let tex = g.create_texture(1, 1);
        g.write_texture(tex, &[255, 255, 255, 128]);
        let t = g.create_target(4, 4, 1);
        g.begin_scene(t, Some(vec![100u8; 4 * 4 * 4]));
        g.draw(t, DrawState { texture: Some(tex), blend: true, ..state() }, &quad(0.0, 0.0, 4.0, 4.0, 0xff_ff_ff_ff, (0.0, 0.0, 1.0, 1.0)));
        let img = full(&g.end_scene(t).unwrap(), 4, 4);
        let p = px(&img, 4, 2, 2);
        assert!((p[0] as i32 - 178).abs() <= 2, "{p:?}");
    }

    #[test]
    fn supersampled_targets_read_back_at_their_size() {
        let Some(mut g) = gpu() else { return };
        let t = g.create_target(8, 8, 2);
        g.begin_scene(t, Some(vec![50u8; 8 * 8 * 4]));
        g.draw(t, state(), &quad(0.0, 0.0, 4.0, 8.0, 0xff_ff_00_00, (0.0, 0.0, 1.0, 1.0)));
        let r = g.end_scene(t).unwrap();
        assert_eq!(r.rgba.len() as u32, r.rect.2 * r.rect.3 * 4);
        let img = full(&r, 8, 8);
        assert_eq!(px(&img, 8, 1, 4), [255, 0, 0, 255]);
    }

    /// What a presented pixel looks like: 5-6-5 bits per colour, expanded.
    fn q565(rgb: [u8; 3]) -> u32 {
        let (r, g, b) = ((rgb[0] >> 3) as u32, (rgb[1] >> 2) as u32, (rgb[2] >> 3) as u32);
        ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2)
    }

    /// The target's whole picture through `present_async`.
    fn presented(g: &mut Gpu, t: TargetId) -> (u32, u32, Vec<u32>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: PresentSink = std::sync::Arc::new(move |w, h, px| {
            let _ = tx.send((w, h, px));
        });
        assert!(g.present_async(t, &sink));
        rx.recv_timeout(std::time::Duration::from_secs(10)).expect("a picture")
    }

    #[test]
    fn a_blit_copies_exact_pixels() {
        let Some(mut g) = gpu() else { return };
        let tex = g.create_texture(4, 4);
        let mut rgba = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                rgba.extend([x * 60, y * 60, 100, 255]);
            }
        }
        g.write_texture(tex, &rgba);
        let t = g.create_target(8, 8, 1);
        g.blit(t, tex, (1, 1, 2, 3), (3, 4), false);
        let (w, h, px) = presented(&mut g, t);
        assert_eq!((w, h), (8, 8));
        for y in 0..8u32 {
            for x in 0..8u32 {
                let inside = (3..5).contains(&x) && (4..7).contains(&y);
                let want = if inside { q565([((x - 3 + 1) * 60) as u8, ((y - 4 + 1) * 60) as u8, 100]) } else { 0 };
                assert_eq!(px[(y * 8 + x) as usize], want, "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn a_blended_blit_keeps_what_is_under_transparent_texels() {
        let Some(mut g) = gpu() else { return };
        let tex = g.create_texture(2, 1);
        g.write_texture(tex, &[255, 0, 0, 255, 0, 255, 0, 0]);
        let t = g.create_target(4, 4, 1);
        g.begin_scene(t, Some(vec![9; 4 * 4 * 4]));
        g.end_scene_lazy(t);
        g.blit(t, tex, (0, 0, 2, 1), (1, 1), true);
        let (_, _, px) = presented(&mut g, t);
        assert_eq!(px[(1 * 4 + 1) as usize], q565([255, 0, 0]));
        assert_eq!(px[(1 * 4 + 2) as usize], q565([9, 9, 9]), "the transparent texel leaves the picture alone");
    }

    #[test]
    fn a_target_can_be_drawn_onto_another() {
        let Some(mut g) = gpu() else { return };
        let a = g.create_target(4, 4, 1);
        g.begin_scene(a, None);
        g.draw(a, state(), &quad(0.0, 0.0, 4.0, 4.0, 0xff_00_80_ff, (0.0, 0.0, 1.0, 1.0)));
        // No end_scene: the blit must see what the scene drew so far.
        let b = g.create_target(4, 4, 1);
        let ta = g.target_texture(a).unwrap();
        g.blit(b, ta, (0, 0, 4, 4), (0, 0), false);
        let (_, _, px) = presented(&mut g, b);
        assert!(px.iter().all(|&p| p == q565([0, 0x80, 0xff])), "{:x?}", &px[..4]);
    }
}
