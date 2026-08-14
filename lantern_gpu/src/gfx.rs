//! GPU device, surface, and frame lifecycle.
//!
//! Adapted from Lantern DE's `lntrn-gfx`: the X11 surface shim became a Win32
//! one (this crate targets plugin windows inside Ableton-under-Wine), and
//! pollster was replaced with a local `block_on`.

use std::ffi::c_void;
use std::num::NonZeroIsize;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, Win32WindowHandle,
    WindowsDisplayHandle,
};

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
}

/// Raw-handle shim over a Win32 window, consumed by
/// `create_surface_unsafe(SurfaceTargetUnsafe::from_window(...))`.
struct Win32Surface {
    hwnd: isize,
    hinstance: isize,
}

unsafe impl Send for Win32Surface {}
unsafe impl Sync for Win32Surface {}

impl HasDisplayHandle for Win32Surface {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        let handle = WindowsDisplayHandle::new();
        Ok(unsafe {
            raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Windows(handle))
        })
    }
}

impl HasWindowHandle for Win32Surface {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let hwnd =
            NonZeroIsize::new(self.hwnd).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = NonZeroIsize::new(self.hinstance);
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

pub struct GpuContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// None for offscreen (headless) contexts, which render into textures.
    pub surface: Option<wgpu::Surface<'static>>,
    pub config: wgpu::SurfaceConfiguration,
    pub format: wgpu::TextureFormat,
    pub adapter_info: wgpu::AdapterInfo,
    instance: Arc<wgpu::Instance>,
    timing: Arc<Mutex<FrameTimingSnapshot>>,
}

pub struct Frame {
    output: wgpu::SurfaceTexture,
    view: wgpu::TextureView,
    encoder: wgpu::CommandEncoder,
    label: &'static str,
    started_at: Instant,
    acquire_micros: u128,
    timing: Arc<Mutex<FrameTimingSnapshot>>,
}

#[derive(Clone, Debug, Default)]
pub struct FrameTimingSnapshot {
    pub label: &'static str,
    pub acquire_micros: u128,
    pub encode_micros: u128,
    pub submit_micros: u128,
    pub present_micros: u128,
    pub total_micros: u128,
}

impl GpuContext {
    /// Create a context rendering into an existing Win32 window (e.g. the
    /// plugin editor window a VST3 host hands us). Tries backends in order:
    /// Vulkan (winevulkan passes it straight to the host driver under Wine),
    /// then DX12 (vkd3d), then GL (WGL).
    pub fn from_hwnd(
        hwnd: *mut c_void,
        hinstance: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // wgpu's Vulkan backend refuses a Win32 surface without an hinstance;
        // derive it from the window when the caller doesn't have one handy.
        #[cfg(target_os = "windows")]
        let hinstance = if hinstance.is_null() {
            const GWLP_HINSTANCE: i32 = -6;
            unsafe { GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as *mut c_void }
        } else {
            hinstance
        };
        let target = Win32Surface {
            hwnd: hwnd as isize,
            hinstance: hinstance as isize,
        };

        let mut last_err: Option<Box<dyn std::error::Error>> = None;
        for backends in [wgpu::Backends::VULKAN, wgpu::Backends::DX12, wgpu::Backends::GL] {
            match create_surface_context(&target, width, height, backends) {
                Ok(parts) => return Ok(Self::from_parts(parts)),
                Err(err) => {
                    eprintln!("[lantern-gpu] backend {backends:?} failed: {err}");
                    last_err = Some(err);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| "no wgpu backend produced a surface".into()))
    }

    /// Create a context from anything implementing the raw-window-handle
    /// traits, on a specific set of backends.
    pub fn from_window<W>(
        window: &W,
        width: u32,
        height: u32,
        backends: wgpu::Backends,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        W: HasDisplayHandle + HasWindowHandle,
    {
        Ok(Self::from_parts(create_surface_context(
            window, width, height, backends,
        )?))
    }

    /// Additional surface sharing an existing context's instance/device/queue
    /// — for hosts that open several plugin editor windows at once.
    pub fn from_parent_shared<W>(
        instance: Arc<wgpu::Instance>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
        adapter_info: wgpu::AdapterInfo,
        window: &W,
        width: u32,
        height: u32,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        W: HasDisplayHandle + HasWindowHandle,
    {
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(window)?)
        }?;

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Mailbox,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        Ok(Self {
            device,
            queue,
            surface: Some(surface),
            config,
            format,
            adapter_info,
            instance,
            timing: Arc::new(Mutex::new(FrameTimingSnapshot::default())),
        })
    }

    fn from_parts(parts: SurfaceParts) -> Self {
        let (instance, device, queue, surface, config, format, adapter_info) = parts;
        Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            surface: Some(surface),
            config,
            format,
            adapter_info,
            instance: Arc::new(instance),
            timing: Arc::new(Mutex::new(FrameTimingSnapshot::default())),
        }
    }

    pub fn instance_arc(&self) -> Arc<wgpu::Instance> {
        Arc::clone(&self.instance)
    }
    pub fn device_arc(&self) -> Arc<wgpu::Device> {
        Arc::clone(&self.device)
    }
    pub fn queue_arc(&self) -> Arc<wgpu::Queue> {
        Arc::clone(&self.queue)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        // Surface textures above the device's 2D-texture limit fail wgpu
        // validation and abort the process. Clamp instead: the frame renders
        // at the cap and the compositor/viewport stretches it — mildly soft
        // beats dead.
        let max_dim = self.device.limits().max_texture_dimension_2d;
        let (width, height) = (width.min(max_dim), height.min(max_dim));
        if self.config.width == width && self.config.height == height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
    }

    /// Headless context for tools: no surface, render into textures.
    /// Backend-agnostic and OS-native — this is what lets the face-preview
    /// harness screenshot plugin faces on Linux without a host or a window.
    pub fn offscreen(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;
        let adapter_info = adapter.get_info();
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Lantern Gfx Offscreen"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))?;
        // Match the on-screen pipeline's sRGB rendering so preview colors
        // are what the plugin window shows; RGBA order reads back PNG-ready.
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let max_dim = device.limits().max_texture_dimension_2d;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.min(max_dim),
            height: height.min(max_dim),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            surface: None,
            config,
            format,
            adapter_info,
            instance: Arc::new(instance),
            timing: Arc::new(Mutex::new(FrameTimingSnapshot::default())),
        })
    }

    pub fn begin_frame(&self, label: &'static str) -> Result<Frame, wgpu::SurfaceError> {
        let acquire_started = Instant::now();
        let surface = self.surface.as_ref().ok_or(wgpu::SurfaceError::Lost)?;
        let output = surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        let acquire_micros = acquire_started.elapsed().as_micros();

        if timing_enabled() {
            eprintln!(
                "[lantern-gpu] frame={} acquire_us={} size={}x{}",
                label,
                acquire_micros,
                self.width(),
                self.height()
            );
        }

        Ok(Frame {
            output,
            view,
            encoder,
            label,
            started_at: Instant::now(),
            acquire_micros,
            timing: Arc::clone(&self.timing),
        })
    }

    pub fn timing_snapshot(&self) -> FrameTimingSnapshot {
        self.timing
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }

    pub fn width(&self) -> u32 {
        self.config.width
    }

    pub fn height(&self) -> u32 {
        self.config.height
    }
}

impl Frame {
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn encoder_mut(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    /// Submit the current encoder and create a fresh one. Use between render
    /// layers so that text vertex-buffer writes from a later `prepare()` don't
    /// overwrite data still needed by an earlier render pass.
    pub fn flush(&mut self, gpu: &GpuContext) {
        let old = std::mem::replace(
            &mut self.encoder,
            gpu.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some(self.label),
                }),
        );
        gpu.queue.submit(std::iter::once(old.finish()));
    }

    pub fn submit(self, queue: &wgpu::Queue) {
        let submit_started = Instant::now();
        queue.submit(std::iter::once(self.encoder.finish()));
        let submit_micros = submit_started.elapsed().as_micros();

        let present_started = Instant::now();
        self.output.present();
        let present_micros = present_started.elapsed().as_micros();
        let encode_micros = self.started_at.elapsed().as_micros();
        let total_micros = encode_micros + self.acquire_micros;

        if let Ok(mut snapshot) = self.timing.lock() {
            *snapshot = FrameTimingSnapshot {
                label: self.label,
                acquire_micros: self.acquire_micros,
                encode_micros,
                submit_micros,
                present_micros,
                total_micros,
            };
        }

        if timing_enabled() {
            eprintln!(
                "[lantern-gpu] frame={} encode_us={} submit_us={} present_us={} total_us={} acquire_us={}",
                self.label,
                encode_micros,
                submit_micros,
                present_micros,
                total_micros,
                self.acquire_micros,
            );
        }
    }
}

type SurfaceParts = (
    wgpu::Instance,
    wgpu::Device,
    wgpu::Queue,
    wgpu::Surface<'static>,
    wgpu::SurfaceConfiguration,
    wgpu::TextureFormat,
    wgpu::AdapterInfo,
);

fn create_surface_context<W>(
    window: &W,
    width: u32,
    height: u32,
    backends: wgpu::Backends,
) -> Result<SurfaceParts, Box<dyn std::error::Error>>
where
    W: HasDisplayHandle + HasWindowHandle,
{
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });

    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(window)?)
    }?;

    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))?;
    let adapter_info = adapter.get_info();

    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Lantern Gfx"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))?;

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .find(|candidate| candidate.is_srgb())
        .copied()
        .unwrap_or(caps.formats[0]);

    // LANTERN_GPU_PRESENT overrides present-mode choice (immediate | mailbox
    // | fifo) — Fifo has shown pathological blocking under winevulkan.
    let requested = std::env::var("LANTERN_GPU_PRESENT").ok();
    let present_mode = match requested.as_deref() {
        Some("immediate") if caps.present_modes.contains(&wgpu::PresentMode::Immediate) => {
            wgpu::PresentMode::Immediate
        }
        Some("mailbox") if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) => {
            wgpu::PresentMode::Mailbox
        }
        Some("fifo") => wgpu::PresentMode::Fifo,
        _ => {
            if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
                // Measured under winevulkan (2026-08): Fifo blocks ~1s per
                // acquire once the swapchain fills, Immediate runs at ~0.5ms.
                // Our GUIs self-pace with a timer, so no-vsync is safe.
                wgpu::PresentMode::Immediate
            } else {
                caps.present_modes[0]
            }
        }
    };
    eprintln!(
        "[lantern-gpu] present_mode={:?} available={:?}",
        present_mode, caps.present_modes
    );

    let alpha_mode = if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
    {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
    {
        wgpu::CompositeAlphaMode::PostMultiplied
    } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Inherit) {
        wgpu::CompositeAlphaMode::Inherit
    } else {
        caps.alpha_modes[0]
    };
    eprintln!(
        "[lantern-gpu] backend={:?} adapter={:?} alpha_mode={:?} format={:?}",
        adapter_info.backend, adapter_info.name, alpha_mode, format
    );

    // Same guard as GpuContext::resize — an initial size past the device
    // limit would abort in surface.configure below.
    let max_dim = device.limits().max_texture_dimension_2d;
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.min(max_dim),
        height: height.min(max_dim),
        present_mode,
        alpha_mode,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    Ok((instance, device, queue, surface, config, format, adapter_info))
}

/// Local stand-in for pollster: park this thread until the future resolves.
/// wgpu's request_adapter/request_device futures are effectively ready or
/// driven by wgpu's own threads, so a thread-parking waker is all we need.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};

    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("LANTERN_GPU_TIMING")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}
