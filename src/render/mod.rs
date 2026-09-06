//! The renderer.
//!
//! Five pipelines and one offscreen target. The world draws in one or two
//! calls per visible cluster, characters and props draw as one instanced call,
//! particles as another, and the interface as one more. A busy frame is a
//! couple of dozen draw calls, which is the entire reason the game holds its
//! frame rate on hardware that has no business running a shooter.

pub mod gpu;

use crate::assets::font::{FontAtlas, ATLAS_H, ATLAS_W};
use crate::assets::meshgen::{self, MapMesh, PartInstance, PartVertex, WorldVertex};
use crate::assets::texgen::{self, Sprite, TextureArray};
use crate::maps::Env;
use crate::math::{Aabb, Frustum};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use gpu::{BloomTargets, DynBuffer, Gpu, SceneTargets, DEPTH_FORMAT, SCENE_FORMAT};
use std::sync::Arc;
use winit::window::Window;

/// Decodes an authored (sRGB) colour to linear, matching the shader helper.
#[inline]
fn to_linear(c: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    [f(c[0]), f(c[1]), f(c[2])]
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    view_proj_vm: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    camera_right: [f32; 4],
    camera_up: [f32; 4],
    fog_color: [f32; 4],
    fog_params: [f32; 4],
    sky_top: [f32; 4],
    sky_horizon: [f32; 4],
    screen: [f32; 4],
    time: [f32; 4],
    retro: [f32; 4],
    grade: [f32; 4],
    sun: [f32; 4],
    warm: [f32; 4],
    cool: [f32; 4],
    /// Bloom strength, bloom threshold, and the bloom target's texel size.
    post: [f32; 4],
    /// Per-material gloss, four to a row because a uniform array of scalars
    /// is padded to sixteen bytes an element on every backend that matters.
    gloss: [[f32; 4]; MAT_ROWS],
}

/// How bright a pixel has to be before it blooms. The scene target is
/// eight-bit and clamps at one, so this is the top quarter of the range: lit
/// windows, muzzle flashes, the sun and its glare, and nothing else.
const BLOOM_THRESHOLD: f32 = 0.85;

/// How far away a decal is still drawn.
///
/// Blood is small and dark; past this it is a couple of pixels that the fog
/// has most of anyway, and there are far too many of them to spend the
/// instance on. Generous enough that it is never seen to arrive.
const DECAL_RANGE: f32 = 70.0;

/// Every material, four per row; must match the `gloss` array in the shader.
const MAT_ROWS: usize = crate::assets::materials::MAT_COUNT.div_ceil(4);

/// Packs `Mat::gloss` into the layout the shader indexes. Materials never
/// change at runtime, so this is built once.
fn gloss_table() -> [[f32; 4]; MAT_ROWS] {
    let mut rows = [[0.0f32; 4]; MAT_ROWS];
    for i in 0..crate::assets::materials::MAT_COUNT {
        rows[i / 4][i % 4] = crate::assets::materials::Mat::from_index(i as u8).gloss();
    }
    rows
}

/// One billboarded or ground-aligned quad.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SpriteInstance {
    pub pos: [f32; 3],
    pub size: [f32; 2],
    pub rot: f32,
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    /// 0 = camera facing, 1 = lying on the ground.
    pub mode: f32,
    _pad: [f32; 3],
}

impl SpriteInstance {
    pub fn billboard(pos: Vec3, size: f32, rot: f32, sprite: Sprite, color: [f32; 4]) -> SpriteInstance {
        SpriteInstance {
            pos: [pos.x, pos.y, pos.z],
            size: [size, size],
            rot,
            uv_rect: sprite.uv(),
            color,
            mode: 0.0,
            _pad: [0.0; 3],
        }
    }
    pub fn ground(pos: Vec3, size: f32, rot: f32, sprite: Sprite, color: [f32; 4]) -> SpriteInstance {
        SpriteInstance {
            pos: [pos.x, pos.y, pos.z],
            size: [size, size],
            rot,
            uv_rect: sprite.uv(),
            color,
            mode: 1.0,
            _pad: [0.0; 3],
        }
    }
    /// A decal lying on an arbitrary surface.
    ///
    /// `normal` is the surface it is stuck to; the quad is built in the plane
    /// perpendicular to it and spun by `rot` within that plane. The caller is
    /// responsible for lifting `pos` off the surface far enough not to fight
    /// it in the depth buffer.
    ///
    /// The normal travels in the padding the instance already carried, so an
    /// oriented decal costs exactly what a flat one did.
    pub fn decal(pos: Vec3, normal: Vec3, size: f32, rot: f32, sprite: Sprite, color: [f32; 4]) -> SpriteInstance {
        SpriteInstance {
            pos: [pos.x, pos.y, pos.z],
            size: [size, size],
            rot,
            uv_rect: sprite.uv(),
            color,
            mode: 2.0,
            _pad: [normal.x, normal.y, normal.z],
        }
    }
    pub fn stretched(pos: Vec3, w: f32, h: f32, rot: f32, sprite: Sprite, color: [f32; 4]) -> SpriteInstance {
        SpriteInstance {
            pos: [pos.x, pos.y, pos.z],
            size: [w, h],
            rot,
            uv_rect: sprite.uv(),
            color,
            mode: 0.0,
            _pad: [0.0; 3],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// 0 solid, 1 font, 2 sprite atlas.
    pub mode: f32,
    _pad: [f32; 3],
}

impl UiVertex {
    #[inline]
    pub fn new(pos: [f32; 2], uv: [f32; 2], color: [f32; 4], mode: f32) -> UiVertex {
        UiVertex { pos, uv, color, mode, _pad: [0.0; 3] }
    }
}

/// Visual settings the renderer needs each frame.
#[derive(Clone, Copy, Debug)]
pub struct RenderSettings {
    /// Fraction of the window resolution the 3D scene renders at.
    pub resolution_scale: f32,
    pub msaa: bool,
    /// 0 disables vertex snapping; higher values snap harder.
    pub vertex_snap: f32,
    /// 0..1 blend toward affine (non perspective-correct) texturing.
    pub affine_texturing: f32,
    pub scanlines: f32,
    pub vignette: f32,
    pub exposure: f32,
    pub saturation: f32,
    /// Multiplier on the map's fog distance.
    pub view_distance: f32,
    pub post_processing: bool,
    /// Highest mip the world sampler may use, for texture quality.
    pub texture_lod_bias: f32,
    /// Anisotropic samples; 1 means off and restores point magnification.
    pub anisotropy: u8,
    pub shadows: bool,
    pub particles: f32,
    /// Strength of the shared high-frequency detail layer, 0 disables it.
    pub detail: f32,
    /// How much of the blurred bright pass is added back. 0 disables the
    /// bloom chain entirely, and with it three render passes.
    pub bloom: f32,
    /// The per-map warm/cool split-tone. Off means the picture is graded by
    /// exposure alone.
    pub film_grade: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            resolution_scale: 1.0,
            msaa: false,
            vertex_snap: 0.0,
            affine_texturing: 0.0,
            scanlines: 0.0,
            vignette: 0.22,
            exposure: 1.0,
            saturation: 1.0,
            view_distance: 1.0,
            post_processing: true,
            texture_lod_bias: 0.0,
            anisotropy: 8,
            shadows: true,
            particles: 1.0,
            detail: 0.30,
            bloom: 0.30,
            film_grade: true,
        }
    }
}

/// Camera parameters for one frame.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov_y: 75f32.to_radians(),
            near: 0.05,
            far: 400.0,
        }
    }
}

impl Camera {
    pub fn view(&self) -> Mat4 {
        let dir = crate::math::dir_from_angles(self.yaw, self.pitch);
        let up = Mat4::from_axis_angle(dir, self.roll).transform_vector3(Vec3::Y);
        Mat4::look_to_rh(self.position, dir, up)
    }
    /// The projection the scene is actually drawn with: reversed depth.
    pub fn proj(&self, aspect: f32) -> Mat4 {
        reverse_z_perspective(self.fov_y, aspect.max(0.1), self.near, self.far)
    }
    /// A conventional 0..1 projection, used only to extract culling planes.
    /// Reversed depth swaps the near and far rows, which would invert both
    /// planes and quietly cull the whole world.
    pub fn cull_proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect.max(0.1), self.near, self.far)
    }
    pub fn view_proj(&self, aspect: f32) -> Mat4 { self.proj(aspect) * self.view() }
}

/// Right-handed perspective that maps the near plane to depth 1 and the far
/// plane to depth 0.
///
/// Floating-point depth clusters its precision near zero, and a conventional
/// projection spends that precision on the near plane where nothing needs it.
/// Reversing the range puts it where the geometry is instead, which turns the
/// millimetres of separation this game's overlapping floor slabs rely on from
/// a coin toss at forty metres into an exact answer at four hundred.
pub fn reverse_z_perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    let span = (far - near).max(1e-6);
    Mat4::from_cols(
        glam::Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, f, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, near / span, -1.0),
        glam::Vec4::new(0.0, 0.0, far * near / span, 0.0),
    )
}

/// The GPU-side copy of one map's geometry.
pub struct MapGpu {
    /// `(brush, index range, bounds)` for each breakable, drawn one by one so
    /// a destroyed brush is simply a range that is not submitted.
    pub breakables: Vec<(u32, std::ops::Range<u32>, Aabb)>,
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    clusters: Vec<meshgen::Cluster>,
    pub triangles: usize,
    pub bytes: usize,
}

pub struct Renderer {
    pub gpu: Gpu,
    targets: SceneTargets,
    bloom: BloomTargets,
    pub settings: RenderSettings,
    samples: u32,

    globals: DynBuffer,
    globals_bg: wgpu::BindGroup,
    globals_layout: wgpu::BindGroupLayout,

    world_bg: wgpu::BindGroup,
    world_layout: wgpu::BindGroupLayout,
    atlas_bg: wgpu::BindGroup,
    atlas_layout: wgpu::BindGroupLayout,
    scene_bg: wgpu::BindGroup,
    bright_bg: wgpu::BindGroup,
    blur_h_bg: wgpu::BindGroup,
    blur_v_bg: wgpu::BindGroup,
    scene_layout: wgpu::BindGroupLayout,

    shader: wgpu::ShaderModule,
    pipe_sky: wgpu::RenderPipeline,
    pipe_world: wgpu::RenderPipeline,
    pipe_world_cutout: wgpu::RenderPipeline,
    pipe_part: wgpu::RenderPipeline,
    pipe_viewmodel: wgpu::RenderPipeline,
    pipe_sprite: wgpu::RenderPipeline,
    pipe_bright: wgpu::RenderPipeline,
    pipe_blur_h: wgpu::RenderPipeline,
    pipe_blur_v: wgpu::RenderPipeline,
    pipe_blit: wgpu::RenderPipeline,
    pipe_ui: wgpu::RenderPipeline,

    /// One mesh per `PartShape`, and one instance stream per mesh. A shape is
    /// a draw call, not a per-instance branch.
    shape_vb: Vec<wgpu::Buffer>,
    shape_ib: Vec<wgpu::Buffer>,
    shape_indices: Vec<u32>,
    shape_bufs: Vec<DynBuffer>,
    vm_shape_bufs: Vec<DynBuffer>,
    quad_vb: wgpu::Buffer,
    quad_ib: wgpu::Buffer,

    sprite_buf: DynBuffer,
    ui_buf: DynBuffer,

    parts: Vec<Vec<PartInstance>>,
    viewmodel: Vec<Vec<PartInstance>>,
    pub sprites: Vec<SpriteInstance>,
    /// Decals, kept apart from the other sprites because they are permanent
    /// and there are a great many of them. Only the ones on screen are turned
    /// into instances, and that test needs the frustum, which does not exist
    /// until the frame is being drawn.
    decals: Vec<SpriteInstance>,
    pub ui: Vec<UiVertex>,

    pub map: Option<MapGpu>,
    pub font: FontAtlas,
    pub stats: RenderStats,
    texture_bytes: usize,
    texture_size: u32,
    /// Per-material gloss, packed once at start-up.
    gloss: [[f32; 4]; MAT_ROWS],
    /// Mirror of the collision world's destruction set, for culling draws.
    destroyed: Vec<bool>,

    /// Set to have the next frame copied back to system memory.
    pub capture_request: bool,
    /// The most recent capture, as `(width, height, RGBA)`.
    pub captured: Option<(u32, u32, Vec<u8>)>,
    capture: Option<(wgpu::Texture, wgpu::TextureView, wgpu::Buffer, u32, u32, u32)>,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub triangles: u32,
    pub clusters_drawn: u32,
    pub clusters_total: u32,
    pub sprites: u32,
    /// Decals held, and how many of those were drawn this frame.
    pub decals_held: u32,
    pub decals_drawn: u32,
    pub ui_quads: u32,
}

impl Renderer {
    pub fn new(window: Arc<Window>, settings: RenderSettings, vsync: bool, texture_size: u32) -> Result<Renderer, String> {
        let gpu = Gpu::new(window, vsync)?;
        let device = &gpu.device;

        // ------------------------------------------------------- resources
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hardpoint shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
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
        let globals = DynBuffer::new(
            device,
            "globals",
            wgpu::BufferUsages::UNIFORM,
            std::mem::size_of::<Globals>() as u64,
        );
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.buffer.as_entire_binding() }],
        });

        // World texture array.
        let world_array = texgen::generate_world_array(texture_size);
        let texture_bytes = world_array.bytes();
        let (world_layout, world_bg) = build_world_bindings(device, &gpu.queue, &world_array, settings.texture_lod_bias, settings.anisotropy);

        // Sprite and font atlases.
        let font = FontAtlas::build();
        let (atlas_layout, atlas_bg) = build_atlas_bindings(device, &gpu.queue, &font);

        let samples = if settings.msaa { 4 } else { 1 };
        let targets = SceneTargets::new(
            device,
            (gpu.config.width as f32 * settings.resolution_scale) as u32,
            (gpu.config.height as f32 * settings.resolution_scale) as u32,
            samples,
        );

        let bloom = BloomTargets::new(device, targets.width, targets.height);
        let (scene_layout, post_bgs) = build_scene_bindings(device, &targets, &bloom);
        let PostBindGroups { scene: scene_bg, bright: bright_bg, blur_h: blur_h_bg, blur_v: blur_v_bg } = post_bgs;

        let (cube_v, cube_i) = meshgen::unit_cube();
        let mut shape_vb = Vec::with_capacity(meshgen::PART_SHAPES);
        let mut shape_ib = Vec::with_capacity(meshgen::PART_SHAPES);
        let mut shape_indices = Vec::with_capacity(meshgen::PART_SHAPES);
        for shape in meshgen::ALL_SHAPES {
            let (v, i) = meshgen::shape_mesh(shape);
            shape_vb.push(create_buffer(device, "part vertices", bytemuck::cast_slice(&v), wgpu::BufferUsages::VERTEX));
            shape_ib.push(create_buffer(device, "part indices", bytemuck::cast_slice(&i), wgpu::BufferUsages::INDEX));
            shape_indices.push(i.len() as u32);
        }
        let _ = (&cube_v, &cube_i);
        let (quad_v, quad_i) = meshgen::unit_quad();
        let quad_vb = create_buffer(device, "quad vertices", bytemuck::cast_slice(&quad_v), wgpu::BufferUsages::VERTEX);
        let quad_ib = create_buffer(device, "quad indices", bytemuck::cast_slice(&quad_i), wgpu::BufferUsages::INDEX);

        let layouts = Layouts {
            globals: &globals_layout,
            world: &world_layout,
            atlas: &atlas_layout,
            scene: &scene_layout,
        };
        let pipes = build_pipelines(device, &shader, &layouts, samples, gpu.config.format);

        let sprite_buf = DynBuffer::new(device, "sprite instances", wgpu::BufferUsages::VERTEX, 256 * 1024);
        let ui_buf = DynBuffer::new(device, "ui vertices", wgpu::BufferUsages::VERTEX, 512 * 1024);
        // Sized for a full server up front. An instance is eighty bytes, a
        // soldier is twenty-eight of them, and a sixteen-player match with
        // props can put well over a thousand into a single shape's stream.
        // Growing a GPU buffer mid-match reallocates and stalls, which shows
        // up as exactly the sort of isolated dropped frame that is hardest to
        // attribute later.
        let shape_bufs: Vec<DynBuffer> = (0..meshgen::PART_SHAPES)
            .map(|_| DynBuffer::new(device, "part instances", wgpu::BufferUsages::VERTEX, 192 * 1024))
            .collect();
        let vm_shape_bufs: Vec<DynBuffer> = (0..meshgen::PART_SHAPES)
            .map(|_| DynBuffer::new(device, "viewmodel instances", wgpu::BufferUsages::VERTEX, 16 * 1024))
            .collect();

        Ok(Renderer {
            gpu,
            targets,
            bloom,
            settings,
            samples,
            globals,
            globals_bg,
            globals_layout,
            world_bg,
            world_layout,
            atlas_bg,
            atlas_layout,
            scene_bg,
            bright_bg,
            blur_h_bg,
            blur_v_bg,
            scene_layout,
            shader,
            pipe_sky: pipes.sky,
            pipe_world: pipes.world,
            pipe_world_cutout: pipes.world_cutout,
            pipe_part: pipes.part,
            pipe_viewmodel: pipes.viewmodel,
            pipe_sprite: pipes.sprite,
            pipe_bright: pipes.bright,
            pipe_blur_h: pipes.blur_h,
            pipe_blur_v: pipes.blur_v,
            pipe_blit: pipes.blit,
            pipe_ui: pipes.ui,
            shape_vb,
            shape_ib,
            shape_indices,
            shape_bufs,
            vm_shape_bufs,
            quad_vb,
            quad_ib,
            sprite_buf,
            ui_buf,
            parts: (0..meshgen::PART_SHAPES).map(|_| Vec::with_capacity(256)).collect(),
            viewmodel: (0..meshgen::PART_SHAPES).map(|_| Vec::with_capacity(32)).collect(),
            sprites: Vec::with_capacity(2048),
            decals: Vec::with_capacity(4096),
            ui: Vec::with_capacity(8192),
            map: None,
            font,
            stats: RenderStats::default(),
            texture_bytes,
            texture_size,
            gloss: gloss_table(),
            destroyed: Vec::new(),
            capture_request: false,
            captured: None,
            capture: None,
        })
    }

    /// Creates or reuses the readback texture and buffer.
    fn prepare_capture(&mut self) -> (wgpu::Texture, wgpu::TextureView, wgpu::Buffer, u32, u32, u32) {
        let w = self.gpu.config.width;
        let h = self.gpu.config.height;
        // Buffer rows must be a multiple of 256 bytes.
        let row = ((w * 4) + 255) / 256 * 256;
        if let Some((t, v, b, cw, ch, cr)) = &self.capture {
            if *cw == w && *ch == h {
                return (t.clone(), v.clone(), b.clone(), *cw, *ch, *cr);
            }
        }
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.gpu.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.capture = Some((texture.clone(), view.clone(), buffer.clone(), w, h, row));
        (texture, view, buffer, w, h, row)
    }

    fn read_capture(&mut self, buffer: &wgpu::Buffer, w: u32, h: u32, row: u32) {
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        let _ = self.gpu.device.poll(wgpu::PollType::Wait);
        if rx.recv().map(|r| r.is_err()).unwrap_or(true) { return; }
        let data = slice.get_mapped_range();
        let mut out = vec![0u8; (w * h * 4) as usize];
        let swap = matches!(
            self.gpu.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        for y in 0..h {
            let src = (y * row) as usize;
            let dst = (y * w * 4) as usize;
            for x in 0..w as usize {
                let s = src + x * 4;
                let d = dst + x * 4;
                if swap {
                    out[d] = data[s + 2];
                    out[d + 1] = data[s + 1];
                    out[d + 2] = data[s];
                } else {
                    out[d] = data[s];
                    out[d + 1] = data[s + 1];
                    out[d + 2] = data[s + 2];
                }
                out[d + 3] = 255;
            }
        }
        drop(data);
        buffer.unmap();
        self.captured = Some((w, h, out));
    }

    pub fn adapter_name(&self) -> &str { &self.gpu.adapter_name }
    /// True when the adapter is integrated, software or otherwise weak.
    pub fn low_power(&self) -> bool { self.gpu.low_power }
    pub fn backend(&self) -> &str { &self.gpu.backend }
    pub fn texture_memory(&self) -> usize { self.texture_bytes }
    pub fn scene_memory(&self) -> usize { self.targets.memory_bytes() + self.bloom.memory_bytes() }
    pub fn internal_size(&self) -> (u32, u32) { (self.targets.width, self.targets.height) }
    pub fn window_size(&self) -> (u32, u32) { (self.gpu.config.width, self.gpu.config.height) }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        self.rebuild_targets();
    }

    pub fn apply_settings(&mut self, settings: RenderSettings, vsync: bool) {
        let msaa_changed = settings.msaa != self.settings.msaa;
        let lod_changed = (settings.texture_lod_bias - self.settings.texture_lod_bias).abs() > 0.01
            || settings.anisotropy != self.settings.anisotropy;
        let scale_changed = (settings.resolution_scale - self.settings.resolution_scale).abs() > 0.001;
        self.settings = settings;
        self.gpu.set_vsync(vsync);
        if msaa_changed || scale_changed {
            self.rebuild_targets();
        }
        if msaa_changed {
            self.rebuild_pipelines();
        }
        if lod_changed {
            let size = self.texture_size;
            self.rebuild_world_textures(size);
        }
    }

    fn rebuild_targets(&mut self) {
        let samples = if self.settings.msaa { 4 } else { 1 };
        let w = ((self.gpu.config.width as f32 * self.settings.resolution_scale) as u32).max(64);
        let h = ((self.gpu.config.height as f32 * self.settings.resolution_scale) as u32).max(64);
        if self.targets.matches(w, h, samples) { return; }
        self.targets = SceneTargets::new(&self.gpu.device, w, h, samples);
        self.samples = self.targets.samples;
        self.bloom = BloomTargets::new(&self.gpu.device, self.targets.width, self.targets.height);
        let (layout, bgs) = build_scene_bindings(&self.gpu.device, &self.targets, &self.bloom);
        self.scene_layout = layout;
        self.scene_bg = bgs.scene;
        self.bright_bg = bgs.bright;
        self.blur_h_bg = bgs.blur_h;
        self.blur_v_bg = bgs.blur_v;
    }

    fn rebuild_pipelines(&mut self) {
        let layouts = Layouts {
            globals: &self.globals_layout,
            world: &self.world_layout,
            atlas: &self.atlas_layout,
            scene: &self.scene_layout,
        };
        let p = build_pipelines(&self.gpu.device, &self.shader, &layouts, self.samples, self.gpu.config.format);
        self.pipe_sky = p.sky;
        self.pipe_world = p.world;
        self.pipe_world_cutout = p.world_cutout;
        self.pipe_part = p.part;
        self.pipe_viewmodel = p.viewmodel;
        self.pipe_sprite = p.sprite;
        self.pipe_bright = p.bright;
        self.pipe_blur_h = p.blur_h;
        self.pipe_blur_v = p.blur_v;
        self.pipe_blit = p.blit;
        self.pipe_ui = p.ui;
    }

    /// Regenerates the material array at `size` and rebinds it.
    ///
    /// The sampler carries the anisotropy and LOD clamp, so a filtering change
    /// alone does not need new pixels - but the bind group does, and the
    /// texture has to be recreated to be rebound. Keeping the size on the
    /// renderer is what stops that path from silently dropping every surface
    /// in the game back to the lowest resolution, which is what it used to do.
    fn rebuild_world_textures(&mut self, size: u32) {
        let array = texgen::generate_world_array(size);
        self.texture_bytes = array.bytes();
        self.texture_size = size;
        let (layout, bg) = build_world_bindings(&self.gpu.device, &self.gpu.queue, &array, self.settings.texture_lod_bias, self.settings.anisotropy);
        self.world_layout = layout;
        self.world_bg = bg;
        self.rebuild_pipelines();
    }

    /// Changes texture resolution, if it actually differs.
    pub fn set_texture_size(&mut self, size: u32) {
        if size == self.texture_size { return; }
        self.rebuild_world_textures(size);
    }

    pub fn texture_size(&self) -> u32 { self.texture_size }

    /// Uploads a map's geometry, replacing whatever was loaded.
    pub fn upload_map(&mut self, mesh: &MapMesh) {
        let vertex = create_buffer(&self.gpu.device, "map vertices", bytemuck::cast_slice(&mesh.vertices), wgpu::BufferUsages::VERTEX);
        let index = create_buffer(&self.gpu.device, "map indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX);
        let bytes = mesh.vertices.len() * std::mem::size_of::<WorldVertex>() + mesh.indices.len() * 4;
        self.map = Some(MapGpu {
            breakables: mesh.breakables.clone(),
            vertex,
            index,
            clusters: mesh.clusters.clone(),
            triangles: mesh.triangle_count,
            bytes,
        });
    }

    pub fn clear_map(&mut self) { self.map = None; self.destroyed.clear(); }

    /// Tells the renderer which brushes have been shot away.
    pub fn set_destroyed(&mut self, destroyed: &[bool]) {
        if self.destroyed.len() != destroyed.len() {
            self.destroyed = destroyed.to_vec();
        } else {
            self.destroyed.copy_from_slice(destroyed);
        }
    }

    /// A painter over this frame's interface vertex stream.
    pub fn painter(&mut self) -> crate::ui::draw::Painter<'_> {
        let w = self.gpu.config.width as f32;
        let h = self.gpu.config.height as f32;
        let Renderer { ui, font, .. } = self;
        crate::ui::draw::Painter::new(ui, font, w, h)
    }

    /// An interactive interface layer over this frame's vertex stream.
    pub fn widgets(&mut self, nav: crate::ui::widgets::Nav, focus: usize) -> crate::ui::widgets::Ui<'_> {
        let w = self.gpu.config.width as f32;
        let h = self.gpu.config.height as f32;
        let Renderer { ui, font, .. } = self;
        crate::ui::widgets::Ui::new(ui, font, w, h, nav, focus)
    }

    /// Starts a frame: clears the per-frame lists.
    pub fn begin(&mut self) {
        for p in self.parts.iter_mut() { p.clear(); }
        for p in self.viewmodel.iter_mut() { p.clear(); }
        self.sprites.clear();
        self.decals.clear();
        self.ui.clear();
        self.stats = RenderStats::default();
    }

    #[inline]
    pub fn push_part(&mut self, shape: meshgen::PartShape, i: PartInstance) {
        self.parts[shape as usize].push(i);
    }
    #[inline]
    pub fn push_viewmodel(&mut self, shape: meshgen::PartShape, i: PartInstance) {
        self.viewmodel[shape as usize].push(i);
    }
    #[inline]
    pub fn push_sprite(&mut self, s: SpriteInstance) { self.sprites.push(s); }

    /// Queues a decal. Unlike a sprite this is not necessarily drawn: decals
    /// are permanent, a busy match leaves tens of thousands of them, and all
    /// but the handful in front of the player are discarded once the frustum
    /// for the frame is known.
    pub fn push_decal(&mut self, s: SpriteInstance) { self.decals.push(s); }

    /// Draws the frame and presents it.
    pub fn render(&mut self, camera: &Camera, env: &Env, time: f32, flash: f32, damage: f32) -> Result<(), wgpu::SurfaceError> {
        let frame = match self.gpu.surface.get_current_texture() {
            // A suboptimal frame means the swapchain no longer matches the
            // surface - which is exactly what a fullscreen transition
            // produces. Presenting it anyway is what stretches or tears the
            // first frames at the new size, so take the reconfigure instead.
            Ok(f) if f.suboptimal => {
                drop(f);
                self.gpu.reconfigure();
                self.gpu.surface.get_current_texture()?
            }
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                self.gpu.reconfigure();
                self.gpu.surface.get_current_texture()?
            }
            // A timed-out acquire is transient; skipping the frame is right.
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(e) => return Err(e),
        };
        let view = frame.texture.create_view(&Default::default());

        let aspect = self.targets.width as f32 / self.targets.height.max(1) as f32;
        let view_m = camera.view();
        let view_proj = camera.proj(aspect) * view_m;
        let frustum = Frustum::from_view_proj(camera.cull_proj(aspect) * view_m);

        // The viewmodel gets a narrower field of view and its own near plane,
        // which is how these games kept a weapon from clipping into walls. It
        // draws in a pass of its own against a freshly cleared depth buffer,
        // so its parts occlude each other but nothing in the world.
        let vm_proj = reverse_z_perspective(camera.fov_y * 0.86, aspect, 0.010, 12.0);

        let inv_view = view_m.inverse();
        let right = inv_view.x_axis.truncate();
        let up = inv_view.y_axis.truncate();

        let fog_start = env.fog_start * self.settings.view_distance;
        let fog_end = env.fog_end * self.settings.view_distance;

        let (warm, cool) = if self.settings.film_grade {
            (env.grade_warm, env.grade_cool)
        } else {
            ([1.0; 3], [1.0; 3])
        };
        // Bloom is post-processing and the cheapest thing to drop, so it goes
        // with the rest of the post chain rather than having a switch of its
        // own.
        let bloom_on = self.settings.post_processing && self.settings.bloom > 0.001;
        let fog = to_linear(env.fog_color);
        let sky_top = to_linear(env.sky_top);
        let sky_horizon = to_linear(env.sky_horizon);
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            view_proj_vm: vm_proj.to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 0.0],
            camera_right: [right.x, right.y, right.z, 0.0],
            camera_up: [up.x, up.y, up.z, 0.0],
            fog_color: [fog[0], fog[1], fog[2], fog_start],
            fog_params: [fog_end, 0.0, (camera.fov_y * 0.5).tan(), 0.0],
            sky_top: [sky_top[0], sky_top[1], sky_top[2], 0.0],
            sky_horizon: [sky_horizon[0], sky_horizon[1], sky_horizon[2], 0.0],
            screen: [
                self.gpu.config.width as f32,
                self.gpu.config.height as f32,
                1.0 / self.gpu.config.width as f32,
                1.0 / self.gpu.config.height as f32,
            ],
            time: [time, crate::assets::materials::Mat::WaterSurface.layer() as f32, flash, damage],
            retro: [
                self.settings.vertex_snap,
                self.settings.affine_texturing,
                if self.settings.post_processing { self.settings.scanlines } else { 0.0 },
                if self.settings.post_processing { self.settings.vignette } else { 0.0 },
            ],
            sun: [-env.sun_dir.x, -env.sun_dir.y, -env.sun_dir.z, env.cloud_cover],
            // The w channels carry fog shape, not colour, so the grade toggle
            // only ever neutralises the tints.
            warm: [warm[0], warm[1], warm[2], env.fog_height_falloff],
            cool: [cool[0], cool[1], cool[2], env.fog_floor],
            post: [
                if bloom_on { self.settings.bloom } else { 0.0 },
                BLOOM_THRESHOLD,
                1.0 / self.bloom.width as f32,
                1.0 / self.bloom.height as f32,
            ],
            gloss: self.gloss,
            grade: [
                self.settings.detail,
                crate::assets::texgen::DETAIL_LAYER as f32,
                self.settings.exposure,
                self.settings.saturation,
            ],
        };
        self.globals.write(&self.gpu.device, &self.gpu.queue, bytemuck::bytes_of(&globals));

        // Upload the per-frame streams.
        for (i, list) in self.parts.iter().enumerate() {
            self.shape_bufs[i].write(&self.gpu.device, &self.gpu.queue, bytemuck::cast_slice(list));
        }
        for (i, list) in self.viewmodel.iter().enumerate() {
            self.vm_shape_bufs[i].write(&self.gpu.device, &self.gpu.queue, bytemuck::cast_slice(list));
        }
        // Decals into instances, now that there is a frustum to test against.
        //
        // Blood does not wash off, so by the end of a match there are of the
        // order of a hundred thousand of these. Submitting them all cost
        // seven megabytes of instance data a frame and showed up as a hitch
        // that got worse the longer the match ran. Almost none of them are on
        // screen at any moment: one behind the player, or on the far side of
        // the wall they are looking at, is worth nothing.
        {
            let eye = camera.position;
            let before = self.sprites.len();
            self.stats.decals_held = self.decals.len() as u32;
            for d in &self.decals {
                let pos = Vec3::from(d.pos);
                let to_eye = eye - pos;
                let dist_sq = to_eye.length_squared();
                // Ordered cheapest test first, because this loop is walked
                // once per decal per frame and most of them fail it.
                if dist_sq > DECAL_RANGE * DECAL_RANGE { continue; }
                // A decal is stuck to a surface; if that surface faces away
                // from the eye, it is on the back of something.
                let normal = Vec3::new(d._pad[0], d._pad[1], d._pad[2]);
                if normal.dot(to_eye) <= 0.0 { continue; }
                // The quad's corner is half its width from the middle in both
                // directions, so the diagonal bounds it.
                if !frustum.test_sphere(pos, d.size[0] * 0.75) { continue; }
                self.sprites.push(*d);
            }
            self.stats.decals_drawn = (self.sprites.len() - before) as u32;
        }
        self.sprite_buf.write(&self.gpu.device, &self.gpu.queue, bytemuck::cast_slice(&self.sprites));
        self.ui_buf.write(&self.gpu.device, &self.gpu.queue, bytemuck::cast_slice(&self.ui));

        let mut enc = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

        // ---------------------------------------------------- scene pass
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(self.targets.attachment_raw(wgpu::LoadOp::Clear(wgpu::Color {
                    r: fog[0] as f64,
                    g: fog[1] as f64,
                    b: fog[2] as f64,
                    a: 1.0,
                })))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        // Reversed depth: the far plane is zero.
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_bind_group(0, &self.globals_bg, &[]);
            rp.set_bind_group(1, &self.world_bg, &[]);
            rp.set_bind_group(2, &self.atlas_bg, &[]);

            // World, cluster by cluster.
            if let Some(map) = &self.map {
                self.stats.clusters_total = map.clusters.len() as u32;
                rp.set_vertex_buffer(0, map.vertex.slice(..));
                rp.set_index_buffer(map.index.slice(..), wgpu::IndexFormat::Uint32);

                // Merge runs of consecutive visible clusters into one call.
                //
                // Neighbouring clusters are usually either both visible or
                // both not, and the index buffer is laid out so a run of them
                // is contiguous, so this typically turns a few hundred draws
                // into a few dozen without weakening the culling at all: a
                // cluster that fails the frustum test still ends the run.
                rp.set_pipeline(&self.pipe_world);
                let mut run: Option<std::ops::Range<u32>> = None;
                for c in &map.clusters {
                    let visible = !c.opaque.is_empty() && frustum.test_aabb(&c.bounds);
                    if visible {
                        self.stats.clusters_drawn += 1;
                        run = Some(match run {
                            Some(r) if r.end == c.opaque.start => r.start..c.opaque.end,
                            Some(r) => {
                                rp.draw_indexed(r.clone(), 0, 0..1);
                                self.stats.draw_calls += 1;
                                self.stats.triangles += (r.end - r.start) / 3;
                                c.opaque.clone()
                            }
                            None => c.opaque.clone(),
                        });
                    } else if let Some(r) = run.take() {
                        rp.draw_indexed(r.clone(), 0, 0..1);
                        self.stats.draw_calls += 1;
                        self.stats.triangles += (r.end - r.start) / 3;
                    }
                }
                if let Some(r) = run {
                    rp.draw_indexed(r.clone(), 0, 0..1);
                    self.stats.draw_calls += 1;
                    self.stats.triangles += (r.end - r.start) / 3;
                }
                // Breakables, each its own draw so one can vanish.
                if !map.breakables.is_empty() {
                    for (brush, range, bounds) in &map.breakables {
                        if self.destroyed.get(*brush as usize).copied().unwrap_or(false) { continue; }
                        if !frustum.test_aabb(bounds) { continue; }
                        rp.draw_indexed(range.clone(), 0, 0..1);
                        self.stats.draw_calls += 1;
                        self.stats.triangles += (range.end - range.start) / 3;
                    }
                }

                rp.set_pipeline(&self.pipe_world_cutout);
                let mut run: Option<std::ops::Range<u32>> = None;
                for c in &map.clusters {
                    let visible = !c.cutout.is_empty() && frustum.test_aabb(&c.bounds);
                    if visible {
                        run = Some(match run {
                            Some(r) if r.end == c.cutout.start => r.start..c.cutout.end,
                            Some(r) => {
                                rp.draw_indexed(r.clone(), 0, 0..1);
                                self.stats.draw_calls += 1;
                                self.stats.triangles += (r.end - r.start) / 3;
                                c.cutout.clone()
                            }
                            None => c.cutout.clone(),
                        });
                    } else if let Some(r) = run.take() {
                        rp.draw_indexed(r.clone(), 0, 0..1);
                        self.stats.draw_calls += 1;
                        self.stats.triangles += (r.end - r.start) / 3;
                    }
                }
                if let Some(r) = run {
                    rp.draw_indexed(r.clone(), 0, 0..1);
                    self.stats.draw_calls += 1;
                    self.stats.triangles += (r.end - r.start) / 3;
                }
            }

            // Characters and props: one draw per shape.
            if self.parts.iter().any(|p| !p.is_empty()) {
                rp.set_pipeline(&self.pipe_part);
                for (i, list) in self.parts.iter().enumerate() {
                    if list.is_empty() { continue; }
                    rp.set_vertex_buffer(0, self.shape_vb[i].slice(..));
                    rp.set_vertex_buffer(1, self.shape_bufs[i].buffer.slice(..));
                    rp.set_index_buffer(self.shape_ib[i].slice(..), wgpu::IndexFormat::Uint16);
                    rp.draw_indexed(0..self.shape_indices[i], 0, 0..list.len() as u32);
                    self.stats.draw_calls += 1;
                    self.stats.triangles += (self.shape_indices[i] / 3) * list.len() as u32;
                }
            }

            // Sky last among the opaque passes. Under reversed depth the
            // cleared buffer is the far plane, so this only shades pixels the
            // world did not cover - which on an enclosed map is almost none of
            // them, and the sky shader is the most expensive one here.
            rp.set_pipeline(&self.pipe_sky);
            rp.draw(0..3, 0..1);
            self.stats.draw_calls += 1;

            // Particles and decals.
            if !self.sprites.is_empty() {
                rp.set_pipeline(&self.pipe_sprite);
                rp.set_vertex_buffer(0, self.quad_vb.slice(..));
                rp.set_vertex_buffer(1, self.sprite_buf.buffer.slice(..));
                rp.set_index_buffer(self.quad_ib.slice(..), wgpu::IndexFormat::Uint16);
                rp.draw_indexed(0..6, 0, 0..self.sprites.len() as u32);
                self.stats.draw_calls += 1;
                self.stats.sprites = self.sprites.len() as u32;
            }

        }

        // ------------------------------------------------- viewmodel pass
        //
        // A pass of its own, over the finished scene, against a depth buffer
        // cleared back to the far plane. That is what lets the weapon's own
        // boxes occlude each other correctly - a grip in front of a receiver,
        // a hand in front of a magazine - while still never being occluded by
        // a wall the player is standing against. Sharing the world's depth
        // buffer would force a choice between the two, and the previous
        // always-pass state chose neither: the parts drew in submission order
        // and the hands landed on top of the gun.
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewmodel"),
                color_attachments: &[Some(self.targets.attachment(wgpu::LoadOp::Load))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if self.viewmodel.iter().any(|p| !p.is_empty()) {
                rp.set_bind_group(0, &self.globals_bg, &[]);
                rp.set_bind_group(1, &self.world_bg, &[]);
                rp.set_bind_group(2, &self.atlas_bg, &[]);
                rp.set_pipeline(&self.pipe_viewmodel);
                for (i, list) in self.viewmodel.iter().enumerate() {
                    if list.is_empty() { continue; }
                    rp.set_vertex_buffer(0, self.shape_vb[i].slice(..));
                    rp.set_vertex_buffer(1, self.vm_shape_bufs[i].buffer.slice(..));
                    rp.set_index_buffer(self.shape_ib[i].slice(..), wgpu::IndexFormat::Uint16);
                    rp.draw_indexed(0..self.shape_indices[i], 0, 0..list.len() as u32);
                    self.stats.draw_calls += 1;
                }
            }
        }

        // ------------------------------------------------------------ bloom
        //
        // Three quarter-resolution passes: extract the bright part of the
        // scene, blur it across, blur it down. Sixteenth-area targets and a
        // five-tap kernel each way, so the whole chain costs about an eighth
        // of one full-screen pass.
        if bloom_on {
            let mut pass = |label: &'static str,
                            target: &wgpu::TextureView,
                            pipeline: &wgpu::RenderPipeline,
                            bind: &wgpu::BindGroup| {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rp.set_bind_group(0, &self.globals_bg, &[]);
                rp.set_bind_group(1, &self.world_bg, &[]);
                rp.set_bind_group(2, &self.atlas_bg, &[]);
                rp.set_bind_group(3, bind, &[]);
                rp.set_pipeline(pipeline);
                rp.draw(0..3, 0..1);
            };
            pass("bloom bright", &self.bloom.a_view, &self.pipe_bright, &self.bright_bg);
            pass("bloom blur h", &self.bloom.b_view, &self.pipe_blur_h, &self.blur_h_bg);
            pass("bloom blur v", &self.bloom.a_view, &self.pipe_blur_v, &self.blur_v_bg);
            self.stats.draw_calls += 3;
        }

        // ------------------------------------------------ present + interface
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // The present pipelines share a layout with the scene ones, so
            // every group must be bound even though the blit only samples the
            // scene target.
            rp.set_bind_group(0, &self.globals_bg, &[]);
            rp.set_bind_group(1, &self.world_bg, &[]);
            rp.set_bind_group(2, &self.atlas_bg, &[]);
            rp.set_bind_group(3, &self.scene_bg, &[]);
            rp.set_pipeline(&self.pipe_blit);
            rp.draw(0..3, 0..1);
            self.stats.draw_calls += 1;

            if !self.ui.is_empty() {
                rp.set_pipeline(&self.pipe_ui);
                rp.set_vertex_buffer(0, self.ui_buf.buffer.slice(..));
                rp.draw(0..self.ui.len() as u32, 0..1);
                self.stats.draw_calls += 1;
                self.stats.ui_quads = self.ui.len() as u32 / 6;
            }
        }

        // An optional second copy of the present pass into a readable
        // texture. Only ever runs when a screenshot was asked for.
        let capture = if self.capture_request {
            self.capture_request = false;
            Some(self.prepare_capture())
        } else {
            None
        };
        if let Some((_, view, buffer, w, h, row)) = capture.as_ref() {
            {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("capture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rp.set_bind_group(0, &self.globals_bg, &[]);
                rp.set_bind_group(1, &self.world_bg, &[]);
                rp.set_bind_group(2, &self.atlas_bg, &[]);
                rp.set_bind_group(3, &self.scene_bg, &[]);
                rp.set_pipeline(&self.pipe_blit);
                rp.draw(0..3, 0..1);
                if !self.ui.is_empty() {
                    rp.set_pipeline(&self.pipe_ui);
                    rp.set_vertex_buffer(0, self.ui_buf.buffer.slice(..));
                    rp.draw(0..self.ui.len() as u32, 0..1);
                }
            }
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &capture.as_ref().unwrap().0,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(*row),
                        rows_per_image: Some(*h),
                    },
                },
                wgpu::Extent3d { width: *w, height: *h, depth_or_array_layers: 1 },
            );
        }

        self.gpu.queue.submit(Some(enc.finish()));
        frame.present();

        if let Some((_, _, buffer, w, h, row)) = capture {
            self.read_capture(&buffer, w, h, row);
        }
        Ok(())
    }
}

// --------------------------------------------------------------- plumbing

struct Layouts<'a> {
    globals: &'a wgpu::BindGroupLayout,
    world: &'a wgpu::BindGroupLayout,
    atlas: &'a wgpu::BindGroupLayout,
    scene: &'a wgpu::BindGroupLayout,
}

struct Pipelines {
    sky: wgpu::RenderPipeline,
    bright: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    world: wgpu::RenderPipeline,
    world_cutout: wgpu::RenderPipeline,
    part: wgpu::RenderPipeline,
    viewmodel: wgpu::RenderPipeline,
    sprite: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    ui: wgpu::RenderPipeline,
}

fn create_buffer(device: &wgpu::Device, label: &str, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: data,
        usage,
    })
}

fn build_world_bindings(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    array: &TextureArray,
    lod_bias: f32,
    anisotropy: u8,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    let layers = array.layers.len() as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("world materials"),
        size: wgpu::Extent3d { width: array.size, height: array.size, depth_or_array_layers: layers },
        mip_level_count: array.mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    for (layer, l) in array.layers.iter().enumerate() {
        let mut size = array.size;
        for (mip, data) in l.mips.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
            size = (size / 2).max(1);
        }
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    // Anisotropic filtering needs all three filters linear, so it and point
    // magnification are mutually exclusive. Point sampling is the sharper,
    // more period-correct look, but it leaves every wall seen at a grazing
    // angle smeared into blotches by isotropic mip selection, which reads as
    // the texture crawling as you walk. Anisotropy on by default; the
    // Authentic preset turns it off and takes the smear.
    let aniso = anisotropy.clamp(1, 16) as u16;
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("world sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: if aniso > 1 { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest },
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Linear,
        lod_min_clamp: lod_bias.max(0.0),
        lod_max_clamp: 32.0,
        anisotropy_clamp: aniso,
        ..Default::default()
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("world textures"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("world textures"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    (layout, bind)
}

fn build_atlas_bindings(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    font: &FontAtlas,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    let (sprite_size, sprite_px) = texgen::generate_sprite_atlas();
    let sprite_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sprites"),
        size: wgpu::Extent3d { width: sprite_size, height: sprite_size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &sprite_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &sprite_px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(sprite_size * 4),
            rows_per_image: Some(sprite_size),
        },
        wgpu::Extent3d { width: sprite_size, height: sprite_size, depth_or_array_layers: 1 },
    );

    // The font is single channel, expanded to RGBA for format simplicity.
    let mut font_px = vec![0u8; ATLAS_W * ATLAS_H * 4];
    for (i, v) in font.pixels.iter().enumerate() {
        font_px[i * 4] = *v;
        font_px[i * 4 + 1] = *v;
        font_px[i * 4 + 2] = *v;
        font_px[i * 4 + 3] = *v;
    }
    let font_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("font"),
        size: wgpu::Extent3d { width: ATLAS_W as u32, height: ATLAS_H as u32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &font_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &font_px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(ATLAS_W as u32 * 4),
            rows_per_image: Some(ATLAS_H as u32),
        },
        wgpu::Extent3d { width: ATLAS_W as u32, height: ATLAS_H as u32, depth_or_array_layers: 1 },
    );

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("atlases"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
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
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlases"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&sprite_tex.create_view(&Default::default())),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&font_tex.create_view(&Default::default())),
            },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    // The textures must outlive the bind group; leaking them is correct here
    // because they live for the whole process and are created exactly once.
    std::mem::forget(sprite_tex);
    std::mem::forget(font_tex);
    (layout, bind)
}

/// The four bind groups the post chain needs, all against one layout.
struct PostBindGroups {
    scene: wgpu::BindGroup,
    bright: wgpu::BindGroup,
    blur_h: wgpu::BindGroup,
    blur_v: wgpu::BindGroup,
}

/// Bindings for every pass that reads a full-screen texture.
///
/// One layout serves all four: slot 0 is whatever that pass is reading, slot
/// 2 is the finished bloom (only the blit looks at it), and there are two
/// samplers because they want opposite things. The scene is magnified with
/// point sampling, which is what makes a low internal resolution read as
/// chunky rather than as blurry; the bloom chain wants linear everywhere,
/// because a quarter-resolution blur point-sampled up is a grid of squares.
fn build_scene_bindings(
    device: &wgpu::Device,
    targets: &SceneTargets,
    bloom: &BloomTargets,
) -> (wgpu::BindGroupLayout, PostBindGroups) {
    let point = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scene sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let linear = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("post sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let tex = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let smp = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("post"),
        entries: &[tex(0), smp(1), tex(2), smp(3)],
    });

    let group = |label: &'static str, source: &wgpu::TextureView, bloom_view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&point) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(bloom_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&linear) },
            ],
        })
    };

    let groups = PostBindGroups {
        scene: group("present", targets.sample_view(), &bloom.a_view),
        // Slot 2 is never read by these two, but it must not name the
        // texture the pass is writing to: a texture cannot be a colour target
        // and a bound resource in the same pass, whether or not the shader
        // touches it.
        bright: group("bloom bright", targets.sample_view(), targets.sample_view()),
        blur_h: group("bloom blur h", &bloom.a_view, &bloom.a_view),
        blur_v: group("bloom blur v", &bloom.b_view, &bloom.b_view),
    };
    (layout, groups)
}

fn build_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layouts: &Layouts,
    samples: u32,
    surface_format: wgpu::TextureFormat,
) -> Pipelines {
    let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene pipeline layout"),
        bind_group_layouts: &[layouts.globals, layouts.world, layouts.atlas],
        push_constant_ranges: &[],
    });
    let present_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("present pipeline layout"),
        bind_group_layouts: &[layouts.globals, layouts.world, layouts.atlas, layouts.scene],
        push_constant_ranges: &[],
    });

    let ms = wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false };
    let ms_one = wgpu::MultisampleState::default();

    let depth_write = |write: bool, compare: wgpu::CompareFunction| wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: write,
        depth_compare: compare,
        stencil: Default::default(),
        bias: Default::default(),
    };

    let scene_target = [Some(wgpu::ColorTargetState {
        format: SCENE_FORMAT,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let scene_blend = [Some(wgpu::ColorTargetState {
        format: SCENE_FORMAT,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::COLOR,
    })];
    let present_target = [Some(wgpu::ColorTargetState {
        format: surface_format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let present_blend = [Some(wgpu::ColorTargetState {
        format: surface_format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let world_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Uint32];
    let world_layout_desc = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<WorldVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &world_attrs,
    };

    let part_vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    let part_instance_attrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
    let part_layouts = [
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PartVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &part_vertex_attrs,
        },
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PartInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &part_instance_attrs,
        },
    ];

    let sprite_vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 8 => Float32x3, 9 => Float32x2];
    let sprite_corner_attrs = wgpu::vertex_attr_array![0 => Float32x2];
    let sprite_instance_attrs = wgpu::vertex_attr_array![1 => Float32x3, 2 => Float32x2, 3 => Float32, 4 => Float32x4, 5 => Float32x4, 6 => Float32, 7 => Float32x3];
    let _ = sprite_vertex_attrs;
    let sprite_layouts = [
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PartVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &sprite_corner_attrs,
        },
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SpriteInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &sprite_instance_attrs,
        },
    ];

    let ui_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32];
    let ui_layout_desc = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<UiVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ui_attrs,
    };

    let make = |label: &str,
                layout: &wgpu::PipelineLayout,
                vs: &str,
                fs: &str,
                buffers: &[wgpu::VertexBufferLayout],
                targets: &[Option<wgpu::ColorTargetState>],
                depth: Option<wgpu::DepthStencilState>,
                cull: Option<wgpu::Face>,
                multisample: wgpu::MultisampleState| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vs),
                buffers,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                targets,
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: cull,
                ..Default::default()
            },
            depth_stencil: depth,
            multisample,
            multiview: None,
            cache: None,
        })
    };

    Pipelines {
        sky: make("sky", &scene_layout, "vs_sky", "fs_sky", &[], &scene_target,
                  Some(depth_write(false, wgpu::CompareFunction::GreaterEqual)), None, ms),
        world: make("world", &scene_layout, "vs_world", "fs_world", &[world_layout_desc.clone()], &scene_target,
                    Some(depth_write(true, wgpu::CompareFunction::Greater)), Some(wgpu::Face::Back), ms),
        world_cutout: make("world cutout", &scene_layout, "vs_world", "fs_world_cutout", &[world_layout_desc], &scene_target,
                           Some(depth_write(true, wgpu::CompareFunction::Greater)), None, ms),
        part: make("parts", &scene_layout, "vs_part", "fs_part", &part_layouts, &scene_target,
                   Some(depth_write(true, wgpu::CompareFunction::Greater)), Some(wgpu::Face::Back), ms),
        viewmodel: make("viewmodel", &scene_layout, "vs_viewmodel", "fs_viewmodel", &part_layouts, &scene_target,
                        Some(depth_write(true, wgpu::CompareFunction::Greater)), Some(wgpu::Face::Back), ms),
        sprite: make("sprites", &scene_layout, "vs_sprite", "fs_sprite", &sprite_layouts, &scene_blend,
                     Some(depth_write(false, wgpu::CompareFunction::Greater)), None, ms),
        bright: make("bloom bright", &present_layout, "vs_blit", "fs_bright", &[], &scene_target, None, None, ms_one),
        blur_h: make("bloom blur h", &present_layout, "vs_blit", "fs_blur_h", &[], &scene_target, None, None, ms_one),
        blur_v: make("bloom blur v", &present_layout, "vs_blit", "fs_blur_v", &[], &scene_target, None, None, ms_one),
        blit: make("blit", &present_layout, "vs_blit", "fs_blit", &[], &present_target, None, None, ms_one),
        ui: make("ui", &present_layout, "vs_ui", "fs_ui", &[ui_layout_desc], &present_blend, None, None, ms_one),
    }
}

#[cfg(test)]
mod shader_tests {
    /// Compiles and validates `shaders.wgsl`.
    ///
    /// The renderer builds the shader module when the window opens, so a typo
    /// in the WGSL is not a compile error - it is a black screen on launch,
    /// with the message somewhere in the wgpu log. naga is the compiler wgpu
    /// itself uses, and it needs no adapter to reject a bad shader, so the
    /// whole thing can be checked here in a couple of milliseconds.
    #[test]
    fn the_shader_compiles() {
        let src = include_str!("shaders.wgsl");
        let module = match naga::front::wgsl::parse_str(src) {
            Ok(m) => m,
            Err(e) => panic!("shaders.wgsl does not parse:\n{}", e.emit_to_string(src)),
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        );
        if let Err(e) = validator.validate(&module) {
            panic!("shaders.wgsl does not validate:\n{}", e.emit_to_string(src));
        }
    }
}
