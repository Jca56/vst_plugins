//! VIZIA editor for Lantern. Controls grouped into panels:
//! Oscillators / Filter + LFO / Amp + Filter envelopes / Output.
//! Visual theme lives in `src/theme.css`.

use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::LanternSynthParams;

#[derive(Lens)]
struct Data {
    params: Arc<LanternSynthParams>,
}

impl Model for Data {}

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (780, 540))
}

/// A parameter name label followed by its slider.
macro_rules! row {
    ($cx:expr, $name:expr, $field:ident) => {{
        Label::new($cx, $name)
            .class("plabel")
            .font_size(12.0)
            .height(Pixels(16.0));
        ParamSlider::new($cx, Data::params, |p| &p.$field)
            .height(Pixels(22.0))
            .width(Stretch(1.0));
    }};
}

/// A panel heading.
macro_rules! section {
    ($cx:expr, $title:expr) => {{
        Label::new($cx, $title)
            .class("section")
            .font_weight(FontWeightKeyword::Bold)
            .font_size(13.0)
            .height(Pixels(20.0))
            .top(Pixels(6.0));
    }};
}

pub(crate) fn create(
    params: Arc<LanternSynthParams>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        assets::register_noto_sans_light(cx);
        assets::register_noto_sans_thin(cx);

        if let Err(err) = cx.add_stylesheet(include_style!("src/theme.css")) {
            nih_plug::nih_error!("Failed to load Lantern stylesheet: {err:?}");
        }

        Data {
            params: params.clone(),
        }
        .build(cx);

        VStack::new(cx, |cx| {
            Label::new(cx, "LANTERN")
                .class("title")
                .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                .font_weight(FontWeightKeyword::Thin)
                .font_size(34.0)
                .height(Pixels(46.0))
                .child_top(Stretch(1.0))
                .child_bottom(Pixels(0.0));

            HStack::new(cx, |cx| {
                // --- Oscillators ---
                VStack::new(cx, |cx| {
                    section!(cx, "OSCILLATORS");
                    row!(cx, "Osc 1 Wave", osc1_wave);
                    row!(cx, "Osc 2 Wave", osc2_wave);
                    row!(cx, "Osc Mix", osc_mix);
                    row!(cx, "Osc 2 Detune", osc2_detune);
                    row!(cx, "Osc 2 Octave", osc2_octave);
                    row!(cx, "FM Amount", fm_amount);
                })
                .class("panel")
                .row_between(Pixels(3.0))
                .width(Stretch(1.0));

                // --- Filter + LFO ---
                VStack::new(cx, |cx| {
                    section!(cx, "FILTER");
                    row!(cx, "Cutoff", cutoff);
                    row!(cx, "Resonance", resonance);
                    row!(cx, "Filter Env", filt_env_amount);
                    section!(cx, "LFO (WOBBLE)");
                    row!(cx, "Rate", lfo_rate);
                    row!(cx, "Depth", lfo_depth);
                    row!(cx, "Wave", lfo_wave);
                })
                .class("panel")
                .row_between(Pixels(3.0))
                .width(Stretch(1.0));

                // --- Envelopes ---
                VStack::new(cx, |cx| {
                    section!(cx, "AMP ENV");
                    row!(cx, "Attack", amp_attack);
                    row!(cx, "Decay", amp_decay);
                    row!(cx, "Sustain", amp_sustain);
                    row!(cx, "Release", amp_release);
                    section!(cx, "FILTER ENV");
                    row!(cx, "Attack", flt_attack);
                    row!(cx, "Decay", flt_decay);
                    row!(cx, "Sustain", flt_sustain);
                    row!(cx, "Release", flt_release);
                })
                .class("panel")
                .row_between(Pixels(3.0))
                .width(Stretch(1.0));

                // --- Output ---
                VStack::new(cx, |cx| {
                    section!(cx, "OUTPUT");
                    row!(cx, "Drive", drive);
                    row!(cx, "Gain", gain);
                })
                .class("panel")
                .row_between(Pixels(3.0))
                .width(Stretch(1.0));
            })
            .col_between(Pixels(12.0))
            .child_space(Pixels(2.0));
        })
        .class("root")
        .child_space(Pixels(12.0))
        .row_between(Pixels(8.0));

        ResizeHandle::new(cx);
    })
}
