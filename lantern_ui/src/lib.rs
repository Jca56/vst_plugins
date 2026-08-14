//! lantern_ui — the Lantern widget kit.
//!
//! Immediate-mode widgets drawn with lantern_gpu's painter, text via fontdue
//! + our ported glyph atlas, and every user gesture reported to the host
//! through begin/perform/endEdit so automation records like a stock device.
//!
//! A plugin face is one closure:
//!
//! ```ignore
//! fn make_editor(params: ParamsHandle) -> Box<dyn Editor> {
//!     UiEditor::create(560, 320, Theme::lantern(), params, |ui| {
//!         let panel = ui.face("LANTERN GAIN");
//!         ui.knob(0, panel.center_x(), panel.center_y(), 64.0, "GAIN");
//!     })
//! }
//! ```

mod editor;
mod layout;
pub mod preview;
mod text;
mod theme;
mod ui;

pub use editor::UiEditor;
pub use layout::Grid;
pub use text::TextRenderer;
pub use theme::Theme;
pub use ui::{InputState, KeyEvent, Ui, UiMemory};

pub use lantern_gpu;
