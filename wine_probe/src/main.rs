//! Phase-0 probe for the Lantern VST foundation.
//!
//! Proves that `lantern_gpu` (the renderer ported from Lantern DE) can drive
//! a real Win32 window from inside Wine — the exact environment our plugin
//! GUIs will live in under Ableton. Opens a window, renders an animated
//! scene through the SDF painter for ~4 seconds, then renders one frame
//! offscreen and reads pixels back to verify output without needing eyes on
//! the screen. Results go to stdout and probe_report.txt.

mod win32;

use std::f32::consts::PI;
use std::ptr::{null, null_mut};
use std::time::Duration;

use lantern_gpu::{wgpu, Color, GpuContext, Painter, Rect};
use win32::*;

fn frame_count() -> u32 {
    std::env::var("PROBE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Drain the message queue. Returns false if WM_QUIT arrived.
fn pump_messages() -> bool {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                return false;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

fn main() {
    let mut report: Vec<String> = Vec::new();
    macro_rules! log {
        ($($arg:tt)*) => {{
            let line = format!($($arg)*);
            println!("{line}");
            report.push(line);
        }};
    }

    log!("== Lantern GPU probe ==");

    let (hwnd, hinstance, client_w, client_h) = unsafe {
        let hinstance = GetModuleHandleW(null());
        let class_name = wide("LanternProbeWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
            lpfnWndProc: wndproc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW as *const u16),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            log!("PROBE_RESULT: FAIL (RegisterClassExW returned 0)");
            finish(report, 1);
        }

        let title = wide("Lantern GPU Probe");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            900,
            380,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if hwnd.is_null() {
            log!("PROBE_RESULT: FAIL (CreateWindowExW returned null)");
            finish(report, 1);
        }
        ShowWindow(hwnd, SW_SHOWNORMAL);

        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        let client_w = (rect.right - rect.left).max(1) as u32;
        let client_h = (rect.bottom - rect.top).max(1) as u32;
        (hwnd, hinstance, client_w, client_h)
    };
    log!("window created: client {}x{}", client_w, client_h);

    // Backend fallback happens inside from_hwnd (Vulkan -> DX12 -> GL);
    // per-backend failures land on stderr.
    let gpu = match GpuContext::from_hwnd(hwnd, hinstance, client_w, client_h) {
        Ok(gpu) => gpu,
        Err(err) => {
            log!("PROBE_RESULT: FAIL (no wgpu backend worked: {err})");
            finish(report, 1);
        }
    };
    log!(
        "gpu ready: backend={:?} adapter={:?} driver={:?} format={:?} present_mode={:?}",
        gpu.adapter_info.backend,
        gpu.adapter_info.name,
        gpu.adapter_info.driver,
        gpu.format,
        gpu.config.present_mode,
    );

    let mut painter = Painter::new(&gpu);

    // --- Animated on-screen run ---
    let frames = frame_count();
    let mut frames_rendered = 0u32;
    let mut surface_errors = 0u32;
    let mut total_frame_micros = 0u128;
    for frame_idx in 0..frames {
        if !pump_messages() {
            log!("window closed early at frame {frame_idx}");
            break;
        }

        let t = frame_idx as f32 / 60.0;
        painter.clear();
        draw_scene(&mut painter, client_w as f32, client_h as f32, t);

        match painter.render(&gpu, Color::from_rgb8(16, 16, 20)) {
            Ok(()) => {
                frames_rendered += 1;
                total_frame_micros += gpu.timing_snapshot().total_micros;
            }
            Err(err) => {
                surface_errors += 1;
                log!("surface error on frame {frame_idx}: {err:?}");
                if surface_errors > 10 {
                    log!("PROBE_RESULT: FAIL (too many surface errors)");
                    finish(report, 1);
                }
                gpu.surface.configure(&gpu.device, &gpu.config);
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    if frames_rendered > 0 {
        log!(
            "on-screen: {frames_rendered} frames, avg {}us/frame, {surface_errors} surface errors",
            total_frame_micros / frames_rendered as u128
        );
    }

    // --- Offscreen render + readback: verify pixels actually happened ---
    match verify_readback(&gpu, &mut painter, client_w, client_h) {
        Ok(line) => {
            log!("readback: {line}");
            log!(
                "PROBE_RESULT: OK backend={:?} adapter={:?} frames={frames_rendered}",
                gpu.adapter_info.backend,
                gpu.adapter_info.name
            );
            unsafe { DestroyWindow(hwnd) };
            finish(report, 0);
        }
        Err(err) => {
            log!("PROBE_RESULT: FAIL (readback: {err})");
            unsafe { DestroyWindow(hwnd) };
            finish(report, 1);
        }
    }
}

/// A scene that exercises every shape family the widgets will use:
/// gradients, panels with borders and shadows, knobs (arc + needle),
/// meters, dashed lines, polylines, triangles, alpha blending.
fn draw_scene(p: &mut Painter, w: f32, h: f32, t: f32) {
    let value = 0.5 + 0.5 * (t * 1.2).sin();

    // Header: multi-stop gradient strip.
    p.rect_gradient_multi_direct(
        Rect::new(0.0, 0.0, w, 56.0),
        0.0,
        0.0,
        &[
            Color::from_rgb8(24, 18, 48),
            Color::from_rgb8(0, 172, 193),
            Color::from_rgb8(255, 111, 0),
        ],
    );

    // Main panel with drop shadow and border.
    let panel = Rect::new(24.0, 80.0, w - 48.0, h - 112.0);
    p.shadow(panel, 10.0, 12.0, Color::rgba(0.0, 0.0, 0.0, 0.55), 0.0, 6.0);
    p.rect_filled(panel, 10.0, Color::from_rgb8(30, 30, 36));
    p.rect_border(panel, 10.0, 2.0, Color::from_rgb8(70, 70, 82));

    // "Knob": track arc, value arc, needle — the future Threshold knob.
    let (kx, ky, kr) = (panel.x + 90.0, panel.center_y(), 46.0);
    let start = 0.75 * PI;
    let range = 1.5 * PI;
    p.circle_filled(kx, ky, kr - 14.0, Color::from_rgb8(46, 46, 54));
    p.arc(kx, ky, kr, start, range, 5.0, 0.0, Color::from_rgb8(60, 60, 70));
    p.arc(kx, ky, kr, start, range * value, 5.0, 0.0, Color::from_rgb8(0, 229, 255));
    let needle_angle = start + range * value;
    p.line(
        kx,
        ky,
        kx + needle_angle.cos() * (kr - 16.0),
        ky + needle_angle.sin() * (kr - 16.0),
        3.0,
        Color::from_rgb8(240, 240, 245),
    );
    p.circle_filled(kx, ky, 5.0, Color::from_rgb8(240, 240, 245));

    // "GR meter": scale arc + swinging needle, Glue-style.
    let (mx, my, mr) = (panel.x + 260.0, panel.center_y() + 20.0, 60.0);
    p.arc(mx, my, mr, 1.25 * PI, 0.5 * PI, 3.0, 0.0, Color::from_rgb8(80, 80, 92));
    let needle = 1.25 * PI + 0.5 * PI * value;
    p.line(
        mx,
        my,
        mx + needle.cos() * (mr - 4.0),
        my + needle.sin() * (mr - 4.0),
        2.0,
        Color::from_rgb8(255, 171, 64),
    );

    // Polyline sine wave (a future waveform/curve display).
    let wave_left = panel.x + 360.0;
    let wave_w = panel.w - 360.0 - 24.0;
    if wave_w > 40.0 {
        let points: Vec<(f32, f32)> = (0..48)
            .map(|i| {
                let fx = i as f32 / 47.0;
                (
                    wave_left + fx * wave_w,
                    panel.center_y() + (fx * 6.0 * PI + t * 3.0).sin() * 28.0,
                )
            })
            .collect();
        p.polyline(&points, 2.0, Color::from_rgb8(0, 229, 255));
        p.line_dashed(
            wave_left,
            panel.center_y(),
            wave_left + wave_w,
            panel.center_y(),
            1.0,
            6.0,
            4.0,
            Color::rgba(1.0, 1.0, 1.0, 0.25),
        );
    }

    // Alpha-blend row + a triangle, because widgets need both.
    for i in 0..6 {
        p.circle_filled(
            panel.x + 40.0 + i as f32 * 26.0,
            panel.y + 24.0,
            9.0,
            Color::rgba(1.0, 0.42, 0.0, 0.15 + i as f32 * 0.15),
        );
    }
    p.triangle(
        panel.x + 210.0,
        panel.y + 34.0,
        panel.x + 230.0,
        panel.y + 14.0,
        panel.x + 250.0,
        panel.y + 34.0,
        Color::from_rgb8(0, 172, 193),
    );
}

/// Render one frame into an offscreen texture and read pixels back.
/// Returns a description of the sampled pixels, or an error if the GPU
/// output doesn't look rendered.
fn verify_readback(
    gpu: &GpuContext,
    painter: &mut Painter,
    w: u32,
    h: u32,
) -> Result<String, Box<dyn std::error::Error>> {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Probe Readback"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gpu.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Known scene: dark clear, bright circle dead center.
    painter.clear();
    painter.circle_filled(
        w as f32 * 0.5,
        h as f32 * 0.5,
        (w.min(h) as f32) * 0.25,
        Color::from_rgb8(255, 80, 20),
    );

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Probe Readback Encoder"),
        });
    painter.render_pass(gpu, &mut encoder, &view, Color::from_rgb8(10, 10, 12));

    // 256-byte-aligned row stride for the copy.
    let bytes_per_row = (w * 4 + 255) / 256 * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Probe Readback Buffer"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()??;

    let data = slice.get_mapped_range();
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = (y * bytes_per_row + x * 4) as usize;
        [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]
    };
    let center = pixel(w / 2, h / 2);
    let corner = pixel(4, 4);

    // The circle must have painted the center something bright; the corner
    // stays near the clear color. Channel order depends on the surface
    // format (usually BGRA), so just demand a bright channel.
    let center_max = center[0].max(center[1]).max(center[2]);
    let corner_max = corner[0].max(corner[1]).max(corner[2]);
    if center_max < 100 {
        return Err(format!(
            "center pixel too dark ({center:?}) — nothing rendered?"
        )
        .into());
    }
    if corner_max > 80 {
        return Err(format!("corner pixel unexpectedly bright ({corner:?})").into());
    }
    Ok(format!(
        "center={center:?} corner={corner:?} (format {:?})",
        gpu.format
    ))
}

/// Write the report file and exit.
fn finish(report: Vec<String>, code: i32) -> ! {
    let _ = std::fs::write("probe_report.txt", report.join("\n") + "\n");
    std::process::exit(code);
}
