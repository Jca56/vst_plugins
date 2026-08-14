//! face_preview — screenshot Lantern plugin faces to PNGs, natively.
//!
//!   cargo run --release -p face_preview [out_dir]
//!
//! Builds each face with staged demo meter values (a detected note, some
//! signal on the meters, a bit of gain reduction) so the screenshots show
//! the plugins alive, not blank. Output: <out_dir>/<plugin>.png
//! (default target/previews/).

use std::ffi::c_void;
use std::sync::atomic::AtomicPtr;

use lantern_ui::{preview, Theme};
use lantern_vst3::plugin::{Dsp, MeterStore, ParamStore, ParamsHandle};

/// Host-free params + meters for one plugin (leaked: this is a tool).
fn stage<D: Dsp>() -> (ParamsHandle, &'static ParamStore, &'static MeterStore) {
    let store: &'static ParamStore = Box::leak(Box::new(ParamStore::new(D::PARAMS)));
    let meters: &'static MeterStore = Box::leak(Box::new(MeterStore::new(D::METERS)));
    let handler: &'static AtomicPtr<c_void> =
        Box::leak(Box::new(AtomicPtr::new(std::ptr::null_mut())));
    (
        ParamsHandle::preview(store, D::PARAMS, handler, meters),
        store,
        meters,
    )
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/previews".to_string());
    std::fs::create_dir_all(&out).expect("create output dir");
    let theme = Theme::lantern();

    // Keylight, expanded, hearing an F#2 slightly flat.
    {
        use lantern_keylight as k;
        let (params, _store, meters) = stage::<k::KeylightDsp>();
        meters.set(k::M_FREQ, 91.8);
        meters.set(k::M_CLARITY, 0.95);
        meters.set(k::M_LEVEL_L, 0.35);
        meters.set(k::M_LEVEL_R, 0.31);
        meters.set(k::M_PEAK_L, 0.88);
        meters.set(k::M_PEAK_R, 0.83);
        meters.set(k::M_RMS_L, 0.17);
        meters.set(k::M_RMS_R, 0.16);
        let path = format!("{out}/keylight.png");
        preview::render_png(1300, 800, &theme, params, 90, &path, k::preview_face())
            .expect("render keylight");
        println!("wrote {path}");
    }

    // Compressor working: ~6 dB of reduction, healthy output.
    {
        use lantern_compressor as c;
        let (params, _store, meters) = stage::<c::CompressorDsp>();
        meters.set(c::M_GAIN_REDUCTION, 5.6);
        meters.set(c::M_LEVEL_L, 0.42);
        meters.set(c::M_LEVEL_R, 0.38);
        meters.set(c::M_PEAK_L, 0.91);
        meters.set(c::M_PEAK_R, 0.87);
        meters.set(c::M_RMS_L, 0.21);
        meters.set(c::M_RMS_R, 0.19);
        let path = format!("{out}/compressor.png");
        preview::render_png(1200, 790, &theme, params, 90, &path, c::preview_face())
            .expect("render compressor");
        println!("wrote {path}");
    }

    // EQ carving a mix: HPF in, bells moved, dense bass-heavy spectrum.
    {
        use lantern_eq as e;
        let (params, store, meters) = stage::<e::EqDsp>();
        store.set(e::p_enabled(0), 1.0);
        store.set(e::p_gain(1), (3.5 + 15.0) / 30.0);
        store.set(e::p_gain(2), (-4.0 + 15.0) / 30.0);
        store.set(e::p_gain(3), (2.5 + 15.0) / 30.0);
        meters.set(e::M_SAMPLE_RATE, 48_000.0);
        meters.set(e::M_LEVEL_L, 0.40);
        meters.set(e::M_LEVEL_R, 0.36);
        meters.set(e::M_PEAK_L, 0.85);
        meters.set(e::M_PEAK_R, 0.80);
        meters.set(e::M_RMS_L, 0.20);
        meters.set(e::M_RMS_R, 0.18);
        // A plausible dubstep-ish spectrum: sub bump, gentle tilt, hash
        // jitter so it reads as signal rather than a curve.
        let mut seed = 0x1234_5678u32;
        for i in 0..e::SPEC_BINS {
            let f = 20.0 * 1000f64.powf(i as f64 / (e::SPEC_BINS - 1) as f64);
            let tilt = -10.0 * (f / 60.0).max(1.0).log10();
            let sub = 14.0 * (-((f / 55.0).ln().powi(2)) / 0.35).exp();
            let mid = 6.0 * (-((f / 900.0).ln().powi(2)) / 0.8).exp();
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let jitter = (seed >> 24) as f64 / 255.0 * 6.0 - 3.0;
            let db = (-26.0 + tilt + sub + mid + jitter).clamp(-90.0, 0.0);
            meters.set(e::M_SPECTRUM + i, db);
        }
        let path = format!("{out}/eq.png");
        preview::render_png(1340, 820, &theme, params, 90, &path, e::preview_face())
            .expect("render eq");
        println!("wrote {path}");
    }
}
