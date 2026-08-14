//! Lantern Compressor — a character compressor with a drive stage.
//!
//! Signal flow:
//!   input ─┬─> sidechain tap ─> HPF ─> stereo-linked peak detect
//!          │                             │
//!          │                    soft-knee gain computer (dB domain)
//!          │                             │
//!          │              attack / release ballistics (manual or
//!          │              crest-factor-adaptive auto release)
//!          │                             │
//!          └─> VCA (gain reduction) ─> drive (soft clip) ─> makeup ─> mix w/ dry
//!
//! Detection is stereo-linked (max of both channels) so the image never wanders.
//! No custom editor on purpose: without one, Ableton renders the parameters as
//! its own inline sliders in the device chain, exactly like a stock device.

use nih_plug::prelude::*;
use std::f32::consts::TAU;
use std::num::NonZeroU32;
use std::sync::Arc;

// ============================================================================
// Sidechain high-pass (RBJ biquad, Butterworth Q)
// ============================================================================

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn set_highpass(&mut self, freq: f32, sample_rate: f32) {
        let w0 = TAU * (freq / sample_rate).min(0.49);
        let (sin, cos) = w0.sin_cos();
        // Butterworth: Q = 1/sqrt(2).
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 + cos) / (2.0 * a0);
        self.b1 = -(1.0 + cos) / a0;
        self.b2 = self.b0;
        self.a1 = (-2.0 * cos) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

// ============================================================================
// Plugin state
// ============================================================================

pub struct LanternCompressor {
    params: Arc<LanternCompressorParams>,
    sample_rate: f32,
    /// Sidechain HPFs, one per channel.
    sc_filters: [Biquad; 2],
    /// Frequency the sidechain filters were last computed for.
    sc_filter_freq: f32,
    /// Current gain reduction in dB (>= 0: the amount being taken off).
    gr_db: f32,
    /// Squared peak / RMS envelopes of the detection signal, driving the
    /// crest-factor-adaptive auto release.
    peak_sq: f32,
    rms_sq: f32,
}

impl Default for LanternCompressor {
    fn default() -> Self {
        Self {
            params: Arc::new(LanternCompressorParams::default()),
            sample_rate: 44_100.0,
            sc_filters: [Biquad::default(); 2],
            sc_filter_freq: -1.0,
            gr_db: 0.0,
            peak_sq: 0.0,
            rms_sq: 0.0,
        }
    }
}

// ============================================================================
// Parameters
// ============================================================================

#[derive(Params)]
struct LanternCompressorParams {
    #[id = "thresh"]
    threshold: FloatParam,
    #[id = "ratio"]
    ratio: FloatParam,
    #[id = "knee"]
    knee: FloatParam,
    #[id = "attack"]
    attack: FloatParam,
    #[id = "release"]
    release: FloatParam,
    #[id = "autorel"]
    auto_release: BoolParam,
    #[id = "schpf"]
    sc_hpf: FloatParam,
    #[id = "drive"]
    drive: FloatParam,
    #[id = "makeup"]
    makeup: FloatParam,
    #[id = "automkup"]
    auto_makeup: BoolParam,
    #[id = "mix"]
    mix: FloatParam,
}

impl Default for LanternCompressorParams {
    fn default() -> Self {
        Self {
            threshold: FloatParam::new(
                "Threshold",
                -18.0,
                FloatRange::Linear {
                    min: -60.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.1)
            .with_smoother(SmoothingStyle::Linear(20.0)),

            ratio: FloatParam::new(
                "Ratio",
                4.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 20.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_value_to_string(formatters::v2s_compression_ratio(1))
            .with_string_to_value(formatters::s2v_compression_ratio()),

            knee: FloatParam::new(
                "Knee",
                6.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 24.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.1),

            attack: FloatParam::new(
                "Attack",
                5.0,
                FloatRange::Skewed {
                    min: 0.05,
                    max: 100.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms"),

            release: FloatParam::new(
                "Release",
                150.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms"),

            // On by default; turn off to use the Release knob directly.
            auto_release: BoolParam::new("Auto Release", true),

            sc_hpf: FloatParam::new(
                "SC HPF",
                80.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 500.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" Hz"),

            drive: FloatParam::new(
                "Drive",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 24.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.1)
            .with_smoother(SmoothingStyle::Linear(20.0)),

            makeup: FloatParam::new(
                "Makeup",
                0.0,
                FloatRange::Linear {
                    min: -12.0,
                    max: 24.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.1)
            .with_smoother(SmoothingStyle::Linear(20.0)),

            auto_makeup: BoolParam::new("Auto Makeup", false),

            mix: FloatParam::new(
                "Mix",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
        }
    }
}

// ============================================================================
// Plugin implementation
// ============================================================================

impl Plugin for LanternCompressor {
    const NAME: &'static str = "Lantern Compressor";
    const VENDOR: &'static str = "Alva";
    const URL: &'static str = "https://github.com/";
    const EMAIL: &'static str = "noreply@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    // No editor() override: with no custom GUI the host draws its own inline
    // panel, which is exactly what we want in Ableton's device chain.

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.sc_filter_freq = -1.0; // force filter recompute on first block
        true
    }

    fn reset(&mut self) {
        for f in &mut self.sc_filters {
            f.reset();
        }
        self.gr_db = 0.0;
        self.peak_sq = 0.0;
        self.rms_sq = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let sr = self.sample_rate;

        // Per-block parameters (these don't need per-sample smoothing).
        let ratio = self.params.ratio.value();
        let knee = self.params.knee.value();
        let attack_s = self.params.attack.value() * 1e-3;
        let release_s = self.params.release.value() * 1e-3;
        let auto_release = self.params.auto_release.value();
        let auto_makeup = self.params.auto_makeup.value();

        let atk_coef = 1.0 - (-1.0 / (sr * attack_s)).exp();
        let rel_coef = 1.0 - (-1.0 / (sr * release_s)).exp();
        // ~200 ms window for the crest-factor trackers.
        let crest_coef = 1.0 - (-1.0 / (sr * 0.2)).exp();

        let sc_freq = self.params.sc_hpf.value();
        if sc_freq != self.sc_filter_freq {
            for f in &mut self.sc_filters {
                f.set_highpass(sc_freq, sr);
            }
            self.sc_filter_freq = sc_freq;
        }

        let slope = 1.0 - 1.0 / ratio;
        let half_knee = knee * 0.5;

        for channel_samples in buffer.iter_samples() {
            // Per-sample smoothed parameters.
            let threshold = self.params.threshold.smoothed.next();
            let drive_db = self.params.drive.smoothed.next();
            let makeup_db = self.params.makeup.smoothed.next();
            let mix = self.params.mix.smoothed.next();

            let mut samples = channel_samples.into_iter();
            let Some(left) = samples.next() else { continue };
            let right = samples.next();

            let in_l = *left;
            let in_r = right.as_ref().map(|s| **s).unwrap_or(in_l);

            // --- Detection: stereo-linked peak of the HPF'd sidechain ---
            let det_l = self.sc_filters[0].process(in_l);
            let det_r = self.sc_filters[1].process(in_r);
            let det = det_l.abs().max(det_r.abs());

            // Crest-factor envelopes track continuously so the auto release
            // has history the moment it's needed.
            let d2 = det * det;
            self.rms_sq += crest_coef * (d2 - self.rms_sq);
            self.peak_sq = (self.peak_sq + crest_coef * (d2 - self.peak_sq)).max(d2);

            // --- Gain computer: soft-knee static curve in dB ---
            let x_db = util::gain_to_db(det);
            let over = x_db - threshold;
            let gr_target = if over <= -half_knee {
                0.0
            } else if over >= half_knee {
                slope * over
            } else {
                let t = over + half_knee;
                slope * t * t / (2.0 * knee).max(1e-9)
            };

            // --- Ballistics (smoothed in the dB domain) ---
            let coef = if gr_target > self.gr_db {
                atk_coef
            } else if auto_release {
                // Crest-factor-adaptive release (after Giannoulis et al.):
                // spiky material recovers fast, sustained material slow.
                let crest_sq = (self.peak_sq / self.rms_sq.max(1e-12)).max(1.0);
                let t_rel = (2.0 / crest_sq).clamp(0.06, 1.2);
                1.0 - (-1.0 / (sr * t_rel)).exp()
            } else {
                rel_coef
            };
            self.gr_db += (gr_target - self.gr_db) * coef;

            // --- Gains ---
            let mut total_makeup_db = makeup_db;
            if auto_makeup {
                // Half the reduction a 0 dBFS signal would get: keeps the
                // level roughly steady as you push threshold/ratio.
                total_makeup_db += -threshold * slope * 0.5;
            }
            let vca = util::db_to_gain(-self.gr_db);
            let makeup_gain = util::db_to_gain(total_makeup_db);

            // Drive: ceiling-normalized tanh, faded in over the first 3 dB so
            // Drive = 0 is bit-transparent.
            let drive_amt = (drive_db / 3.0).min(1.0);
            let drive_gain = util::db_to_gain(drive_db);
            let drive_norm = 1.0 / drive_gain.tanh();

            let shape = |x: f32| -> f32 {
                let wet = x * vca;
                let driven = if drive_amt > 0.0 {
                    let shaped = (wet * drive_gain).tanh() * drive_norm;
                    wet + (shaped - wet) * drive_amt
                } else {
                    wet
                };
                let processed = driven * makeup_gain;
                x + (processed - x) * mix
            };

            *left = shape(in_l);
            if let Some(right) = right {
                *right = shape(in_r);
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for LanternCompressor {
    const CLAP_ID: &'static str = "com.alva.lantern-compressor";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Character compressor with sidechain HPF, auto release, and a drive stage");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Compressor,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ];
}

impl Vst3Plugin for LanternCompressor {
    // Any unique 16 bytes works; this spells it out.
    const VST3_CLASS_ID: [u8; 16] = *b"LanternCompAlva!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(LanternCompressor);
nih_export_vst3!(LanternCompressor);
