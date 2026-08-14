//! The Blacklight face, rebuilt from a clean slate: controls move in one
//! round at a time, checked in the preview harness as they land.
//!
//! Current state: three sub-panels across the top — OSC 1, OSC 2, FILTER,
//! left to right. No section titles (Alva knows the rooms of their own
//! house); the space below is reserved for what comes next.

use lantern_ui::lantern_gpu::{Color, Rect};
use lantern_ui::{Theme, Ui, UiEditor};
use lantern_vst3::plugin::{Editor, ParamsHandle};

const WIN_W: u32 = 1360;
const WIN_H: u32 = 884;

/// Sub-panel fill: one step lighter than the face panel.
fn sub_panel(ui: &mut Ui, rect: Rect) {
    let t = ui.theme;
    ui.painter.rect_filled(rect, 10.0, Color::from_rgb8(26, 26, 31));
    ui.painter.rect_border(rect, 10.0, 2.5, t.panel_border);
}

pub fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
    UiEditor::create(WIN_W, WIN_H, Theme::lantern(), params, draw)
}

/// The face closure for the preview harness. Render at 1360x884.
pub fn preview_face() -> impl FnMut(&mut Ui) {
    draw
}

fn draw(ui: &mut Ui) {
    ui.face();

    // Three rooms across the top: OSC 1 | OSC 2 | FILTER.
    for i in 0..3 {
        sub_panel(
            ui,
            Rect::new(44.0 + i as f32 * 434.0, 44.0, 404.0, 320.0),
        );
    }
}
