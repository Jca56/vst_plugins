//! lantern_vst3 — a hand-rolled VST3 plugin ABI in pure Rust.
//!
//! Zero dependencies: every vtable, interface ID, and struct layout in here
//! is our own code, written against Steinberg's documented VST3 spec
//! (Windows/COM flavor, since Ableton-under-Wine is a Windows host).
//!
//! A plugin is one type implementing [`plugin::Dsp`] plus one macro call:
//!
//! ```ignore
//! struct MyDsp { /* ... */ }
//! impl lantern_vst3::plugin::Dsp for MyDsp { /* INFO, PARAMS, process... */ }
//! lantern_vst3::export_plugin!(MyDsp);
//! ```
//!
//! The crate then provides the factory, the single-component COM object
//! (IComponent + IAudioProcessor + IEditController), parameter plumbing,
//! and versioned state persistence.

pub mod base;
pub mod interfaces;
pub mod plugin;

// The COM objects and window plumbing are Windows-only; gating them lets
// plugin crates compile natively for tools (face previews, tests).
#[cfg(windows)]
pub mod factory;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod view;
#[cfg(windows)]
mod win32;

pub use base::{iid, TUID};
pub use plugin::{Dsp, Editor, EditorFactory, ParamDef, ParamValues, ParamsHandle, PluginInfo};

/// Emit the DLL entry points for one plugin type.
#[macro_export]
macro_rules! export_plugin {
    ($dsp:ty) => {
        #[cfg(windows)]
        #[no_mangle]
        pub unsafe extern "system" fn GetPluginFactory() -> *mut ::std::ffi::c_void {
            $crate::factory::Factory::<$dsp>::create()
        }

        #[cfg(windows)]
        #[no_mangle]
        pub extern "system" fn InitDll() -> bool {
            true
        }

        #[cfg(windows)]
        #[no_mangle]
        pub extern "system" fn ExitDll() -> bool {
            true
        }
    };
}
