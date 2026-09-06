//! GPU context and render targets.
//!
//! The scene is drawn into an offscreen colour target at a configurable
//! internal resolution and then point-upscaled to the window. That single
//! decision buys three things at once: the authentic look of a console
//! renderer, a resolution scale that turns a slideshow into a playable frame
//! rate on weak hardware, and a natural place to hang the post pass.

use std::sync::Arc;
use winit::window::Window;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The offscreen target is plain 8-bit; there is no HDR anywhere in this game.
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct Gpu {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub adapter_name: String,
    pub backend: String,
    /// True when the adapter reports itself as integrated or software, which
    /// we use to pick safer defaults the first time the game runs.
    pub low_power: bool,
    pub max_texture_layers: u32,
}

impl Gpu {
    pub fn new(window: Arc<Window>, vsync: bool) -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY | wgpu::Backends::GL),
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("could not create a drawing surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .or_else(|_| {
            // Fall back to whatever the machine has, including a software
            // adapter: a playable low frame rate beats refusing to launch.
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: true,
            }))
        })
        .map_err(|e| format!("no usable graphics adapter: {e}"))?;

        let info = adapter.get_info();
        let low_power = matches!(
            info.device_type,
            wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::Cpu | wgpu::DeviceType::Other
        );

        // Ask only for what the game actually uses, so it runs on the widest
        // possible range of hardware including GL-only drivers.
        let limits = wgpu::Limits {
            max_texture_dimension_2d: 4096,
            max_texture_array_layers: 128,
            ..wgpu::Limits::downlevel_defaults()
        }
        .using_resolution(adapter.limits());

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("hardpoint device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("could not create a graphics device: {e}"))?;

        // A device error should tell the player something useful rather than
        // aborting with a wgpu panic.
        device.on_uncaptured_error(Box::new(|e| {
            eprintln!("[graphics] {e}");
        }));

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: pick_present_mode(&caps, vsync),
            // Two, not one.
            //
            // A latency of one makes the CPU wait for the GPU to finish the
            // previous frame before it can acquire an image for the next, so
            // the two never overlap and every frame costs CPU time plus GPU
            // time instead of the larger of the two. On this game that was six
            // of every seven milliseconds of the draw spent blocked in
            // `get_current_texture`, on a scene of ninety thousand triangles
            // that the hardware finishes in well under a millisecond.
            //
            // Two is the smallest value that lets them pipeline, and costs at
            // most one frame of extra input latency - which at these frame
            // rates is a few milliseconds, and buys back a great many more.
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let max_texture_layers = device.limits().max_texture_array_layers;

        Ok(Gpu {
            surface,
            device,
            queue,
            config,
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            low_power,
            max_texture_layers,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 { return; }
        if width == self.config.width && height == self.config.height { return; }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_vsync(&mut self, vsync: bool) {
        let want = if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync };
        if self.config.present_mode == want { return; }
        self.config.present_mode = want;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
    }
}

fn pick_present_mode(caps: &wgpu::SurfaceCapabilities, vsync: bool) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::AutoVsync
    } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::AutoVsync
    }
}

/// The offscreen colour and depth the scene is drawn into.
pub struct SceneTargets {
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    /// Resolve target when multisampling is on; otherwise the same as above.
    pub resolved_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    multisampled: bool,
    resolved: Option<wgpu::Texture>,
}

impl SceneTargets {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, samples: u32) -> SceneTargets {
        let width = width.max(1);
        let height = height.max(1);
        let samples = if samples >= 4 { 4 } else { 1 };
        let multisampled = samples > 1;

        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene colour"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());

        let (resolved, resolved_view) = if multisampled {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scene resolve"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SCENE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let v = t.create_view(&Default::default());
            (Some(t), v)
        } else {
            (None, color.create_view(&Default::default()))
        };

        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene depth"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());

        SceneTargets {
            color,
            color_view,
            resolved_view,
            depth_view,
            width,
            height,
            samples,
            multisampled,
            resolved,
        }
    }

    pub fn matches(&self, width: u32, height: u32, samples: u32) -> bool {
        let samples = if samples >= 4 { 4 } else { 1 };
        self.width == width.max(1) && self.height == height.max(1) && self.samples == samples
    }

    /// The attachment pair for the last pass of the frame, which is where the
    /// multisample resolve has to happen.
    pub fn attachment(&self, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPassColorAttachment<'_> {
        wgpu::RenderPassColorAttachment {
            view: &self.color_view,
            resolve_target: if self.multisampled { Some(&self.resolved_view) } else { None },
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        }
    }

    /// The same attachment for an intermediate pass. Resolving more than once
    /// per frame is legal but wasteful, so only the final pass asks for it.
    pub fn attachment_raw(&self, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPassColorAttachment<'_> {
        wgpu::RenderPassColorAttachment {
            view: &self.color_view,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        }
    }

    /// The view the blit pass should sample.
    pub fn sample_view(&self) -> &wgpu::TextureView {
        if self.multisampled { &self.resolved_view } else { &self.color_view }
    }

    pub fn memory_bytes(&self) -> usize {
        let px = (self.width * self.height) as usize;
        let mut b = px * 4 * self.samples as usize + px * 4 * self.samples as usize;
        if self.resolved.is_some() { b += px * 4; }
        b
    }
}

/// The two quarter-resolution targets the bloom passes ping-pong between.
///
/// Quarter resolution in each axis, so a sixteenth of the pixels: the blur is
/// wide and low-frequency by definition and there is nothing in it worth
/// resolving finely. Both are the scene format, so the hardware does the sRGB
/// decode and encode and the blur happens in linear light, which is the only
/// way it stays neutral instead of tinting the highlights.
pub struct BloomTargets {
    pub a_view: wgpu::TextureView,
    pub b_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    _a: wgpu::Texture,
    _b: wgpu::Texture,
}

impl BloomTargets {
    pub fn new(device: &wgpu::Device, scene_width: u32, scene_height: u32) -> BloomTargets {
        let width = (scene_width / 4).max(1);
        let height = (scene_height / 4).max(1);
        let make = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SCENE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let a = make("bloom a");
        let b = make("bloom b");
        let a_view = a.create_view(&Default::default());
        let b_view = b.create_view(&Default::default());
        BloomTargets { a_view, b_view, width, height, _a: a, _b: b }
    }

    pub fn memory_bytes(&self) -> usize {
        (self.width * self.height) as usize * 4 * 2
    }
}

/// A GPU buffer that grows on demand. Used for the per-frame instance and UI
/// streams, which vary in size but never shrink much frame to frame.
pub struct DynBuffer {
    pub buffer: wgpu::Buffer,
    pub capacity: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl DynBuffer {
    pub fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages, capacity: u64) -> DynBuffer {
        let capacity = capacity.max(256);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        DynBuffer { buffer, capacity, usage, label }
    }

    /// Uploads bytes, reallocating only when the data no longer fits.
    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        if bytes.is_empty() { return; }
        let needed = bytes.len() as u64;
        if needed > self.capacity {
            // Grow generously so a busy frame does not reallocate every time.
            let capacity = (needed * 2).next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: capacity,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = capacity;
        }
        queue.write_buffer(&self.buffer, 0, bytes);
    }
}
