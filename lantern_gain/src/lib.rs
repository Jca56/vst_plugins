//! Lantern Gain — one knob, zero dependencies, entirely ours.
//!
//! The Phase-1 milestone plugin: if this shows up in Ableton with an inline
//! Gain slider, automates, and saves state, the hand-rolled ABI works.

use lantern_ui::{Theme, UiEditor};
use lantern_vst3::plugin::{
    Dsp, Editor, EditorFactory, MeterStore, ParamDef, ParamValues, ParamsHandle, PluginInfo,
};

const GAIN_DB_RANGE: f64 = 24.0; // -24..+24 dB

fn gain_to_plain(normalized: f64) -> f64 {
    normalized * (2.0 * GAIN_DB_RANGE) - GAIN_DB_RANGE
}

fn gain_from_plain(plain: f64) -> f64 {
    (plain + GAIN_DB_RANGE) / (2.0 * GAIN_DB_RANGE)
}

fn gain_format(normalized: f64) -> String {
    format!("{:+.1}", gain_to_plain(normalized))
}

pub struct GainDsp {
    /// One-pole smoothed linear gain, to avoid zipper noise on automation.
    current_gain: f32,
    smooth_coef: f32,
}

impl Dsp for GainDsp {
    const INFO: PluginInfo = PluginInfo {
        name: "Lantern Gain",
        vendor: "Alva",
        version: "0.1.0",
        url: "https://github.com/",
        email: "noreply@example.com",
        class_id: *b"LanternGainAlva!",
        subcategories: "Fx|Tools",
    };

    const PARAMS: &'static [ParamDef] = &[ParamDef {
        id: 0,
        title: "Gain",
        short_title: "Gain",
        units: "dB",
        default_normalized: 0.5,
        step_count: 0,
        can_automate: true,
        to_plain: Some(gain_to_plain),
        from_plain: Some(gain_from_plain),
        format: Some(gain_format),
    }];

    const EDITOR: Option<EditorFactory> = Some(make_editor);

    fn new() -> Self {
        Self {
            current_gain: 1.0,
            smooth_coef: 0.0,
        }
    }

    fn setup(&mut self, sample_rate: f64, _max_block_size: usize) {
        // ~5 ms one-pole time constant.
        self.smooth_coef = 1.0 - (-1.0 / (sample_rate as f32 * 0.005)).exp();
    }

    fn reset(&mut self) {
        self.current_gain = 1.0;
    }

    fn process(&mut self, buffers: &mut [&mut [f32]], params: &ParamValues, _meters: &MeterStore) {
        let target_gain = 10.0f32.powf(params.plain(0) as f32 / 20.0);
        for i in 0..buffers.first().map(|b| b.len()).unwrap_or(0) {
            self.current_gain += (target_gain - self.current_gain) * self.smooth_coef;
            for buf in buffers.iter_mut() {
                buf[i] *= self.current_gain;
            }
        }
    }
}

lantern_vst3::export_plugin!(GainDsp);

// ============================================================================
// Editor: an interactive Lantern face (drag the knob, host records it)
// ============================================================================

fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    // Big and chonky: ya boy can't see very well.
    UiEditor::create(640, 380, Theme::lantern(), params, |ui| {
        let panel = ui.face();
        ui.knob(0, panel.center_x(), panel.center_y() + 6.0, 84.0, "GAIN");
    })
}
