//! The Waveshaper face, portrait: shape picker up top, the transfer curve
//! as the hero — drawn from the exact math the samples run through — the
//! fire line, then DRIVE | MIX | OUTPUT with the SUB block on the right
//! (switch above its crossover drag bar), family output meter tight
//! against it all.

use lantern_ui::lantern_gpu::{BackgroundImage, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

use crate::{
    crush_levels, shaped, Shape, M_LEVEL_L, M_LEVEL_R, M_PEAK_L, M_PEAK_R, M_RMS_L, M_RMS_R,
    P_BIAS, P_DRIVE, P_MIX, P_OUT, P_SHAPE, P_SPLIT, P_SUB,
};

const WIN_W: u32 = 800;
const WIN_H: u32 = 888;

/// Alva's fire, embedded raw (600x666 RGBA — fire_background_3, center-
/// cropped to the window's aspect), drawn under the translucent panel.
static BG_RGBA: &[u8] = include_bytes!("../assets/bg.rgba");

pub fn background() -> BackgroundImage {
    BackgroundImage {
        rgba: BG_RGBA.to_vec(),
        width: 600,
        height: 666,
        alpha: 1.0,
    }
}

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    UiEditor::create_with_background(
        WIN_W,
        WIN_H,
        Theme::lantern(),
        Some(background()),
        params,
        |ui| draw_face(ui),
    )
}

/// The face closure for the preview harness (no host, no window).
pub fn preview_face() -> impl FnMut(&mut Ui) {
    |ui| draw_face(ui)
}

/// The transfer curve, live: input -1..1 across, output -1..1 up, the dim
/// diagonal is unity, the gold curve is what drive + shape + bias do to it.
fn curve_window(ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 10.0, t.well_deep);

    let shape = Shape::from_index((ui.params.normalized(P_SHAPE) * 10.0).round() as usize);
    let gain = 10f32.powf(ui.params.plain(P_DRIVE) as f32 / 20.0);
    let bias = ui.params.plain(P_BIAS) as f32 / 100.0 * 0.75;
    let drive01 = (ui.params.plain(P_DRIVE) as f32 / 36.0).clamp(0.0, 1.0);

    let pad = 18.0;
    let (cx, cy) = (rect.center_x(), rect.center_y());
    let half_w = rect.w * 0.5 - pad;
    let half_h = rect.h * 0.5 - pad;

    // Axes + unity diagonal.
    ui.painter.line(rect.x + 8.0, cy, rect.x + rect.w - 8.0, cy, 1.0, t.track);
    ui.painter.line(cx, rect.y + 8.0, cx, rect.y + rect.h - 8.0, 1.0, t.track);
    ui.painter.line(
        cx - half_w,
        cy + half_h,
        cx + half_w,
        cy - half_h,
        1.0,
        t.track,
    );

    let n = 160;
    let mut prev: Option<(f32, f32)> = None;
    ui.painter.push_clip(rect.inset(4.0));
    for i in 0..=n {
        let xn = i as f32 / n as f32 * 2.0 - 1.0;
        // Crush draws its quantizer staircase; Decimate distorts time,
        // not amplitude, so its transfer is honest unity.
        let y = match shape {
            Shape::Crush => {
                let l = crush_levels(drive01);
                (xn * l).round() / l
            }
            Shape::Decimate => xn,
            _ => shaped(shape, xn * gain, bias),
        }
        .clamp(-1.15, 1.15);
        let px = cx + xn * half_w;
        let py = cy - y * half_h;
        if let Some((ax, ay)) = prev {
            ui.painter.line(ax, ay, px, py, 3.0, t.accent);
        }
        prev = Some((px, py));
    }
    ui.painter.pop_clip();
    ui.painter.rect_border(rect, 10.0, 3.0, t.panel_border);
}

fn draw_face(ui: &mut Ui) {
    ui.face_glass(0.72);
    let t = ui.theme;

    // Shape picker up top.
    ui.dropdown(P_SHAPE, Rect::new(44.0, 44.0, 260.0, 48.0));

    // Hero: the transfer curve.
    let curve = Rect::new(44.0, 112.0, 620.0, 480.0);
    curve_window(ui, curve);

    // The fire line, then the knob row: DRIVE | MIX | OUTPUT, and the SUB
    // block on the right — the switch above its crossover drag bar (bar
    // shares the value boxes' baseline).
    ui.divider(44.0, curve.y + curve.h + 10.0, curve.w);
    let row_y = curve.y + curve.h + 30.0;
    let row_h = 222.0;
    ui.knob_cell(P_DRIVE, Rect::new(44.0, row_y, 150.0, row_h), "DRIVE");
    ui.knob_cell(P_MIX, Rect::new(214.0, row_y, 150.0, row_h), "MIX");
    ui.knob_cell(P_OUT, Rect::new(384.0, row_y, 150.0, row_h), "OUTPUT");

    let sub_cx = 609.0;
    ui.label_centered("SUB SPLIT", sub_cx, row_y, 21.0, t.text_dim);
    ui.toggle(P_SUB, Rect::new(sub_cx - 36.0, row_y + 58.0, 72.0, 48.0), "SUB");
    ui.drag_box(P_SPLIT, Rect::new(sub_cx - 55.0, row_y + 188.0, 110.0, 34.0));

    // Family output meter, tight against the curve and the controls.
    ui.level_meter(
        Rect::new(676.0, curve.y, 80.0, row_y + row_h - curve.y),
        (M_LEVEL_L, M_LEVEL_R),
        (M_PEAK_L, M_PEAK_R),
        (M_RMS_L, M_RMS_R),
    );
}
