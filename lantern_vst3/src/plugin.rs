//! The API a Lantern plugin implements. Everything a plugin is hangs off one
//! `Dsp` type: metadata, parameter table, and the audio callback. The COM
//! machinery in `instance.rs`/`factory.rs` is generic over it.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::base::TUID;

/// Static plugin metadata, advertised through the factory.
pub struct PluginInfo {
    /// Display name in the host browser.
    pub name: &'static str,
    pub vendor: &'static str,
    /// "x.y.z"
    pub version: &'static str,
    pub url: &'static str,
    pub email: &'static str,
    /// Unique 16-byte class ID — never change it once a plugin has shipped
    /// into projects, or hosts stop matching saved instances.
    pub class_id: TUID,
    /// Pipe-separated Steinberg subcategories, e.g. "Fx|Dynamics".
    pub subcategories: &'static str,
}

/// One parameter. All host-facing values are normalized 0..1; `to_plain` /
/// `from_plain` map to the display range (dB, Hz, ...).
#[derive(Clone, Copy)]
pub struct ParamDef {
    /// Stable ID (persisted in projects — never reuse or renumber).
    pub id: u32,
    pub title: &'static str,
    pub short_title: &'static str,
    pub units: &'static str,
    pub default_normalized: f64,
    /// 0 = continuous, 1 = toggle, N>1 = N+1 discrete steps.
    pub step_count: i32,
    pub can_automate: bool,
    /// normalized -> plain (display) value. Identity if None.
    pub to_plain: Option<fn(f64) -> f64>,
    /// plain -> normalized. Identity if None.
    pub from_plain: Option<fn(f64) -> f64>,
    /// normalized -> display string. Default: plain value with 2 decimals.
    pub format: Option<fn(f64) -> String>,
}

impl ParamDef {
    pub fn plain(&self, normalized: f64) -> f64 {
        match self.to_plain {
            Some(f) => f(normalized),
            None => normalized,
        }
    }

    pub fn normalized_from_plain(&self, plain: f64) -> f64 {
        (match self.from_plain {
            Some(f) => f(plain),
            None => plain,
        })
        .clamp(0.0, 1.0)
    }

    pub fn display(&self, normalized: f64) -> String {
        match self.format {
            Some(f) => f(normalized),
            None => format!("{:.2}", self.plain(normalized)),
        }
    }
}

/// Live normalized parameter values, shared between the host-facing
/// controller (main thread) and the audio thread as f64 bit patterns in
/// atomics. Index = position in `Dsp::PARAMS`.
pub struct ParamStore {
    values: Box<[AtomicU64]>,
}

impl ParamStore {
    pub fn new(defs: &[ParamDef]) -> Self {
        Self {
            values: defs
                .iter()
                .map(|d| AtomicU64::new(d.default_normalized.to_bits()))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> f64 {
        f64::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    pub fn set(&self, index: usize, normalized: f64) {
        self.values[index].store(normalized.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

/// Live meter values flowing audio thread -> editor (gain reduction,
/// output level, ...). Same atomic-f64 pattern as `ParamStore`, opposite
/// direction: `process()` writes, the editor reads.
pub struct MeterStore {
    values: Box<[AtomicU64]>,
}

impl MeterStore {
    pub fn new(count: usize) -> Self {
        Self {
            values: (0..count).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> f64 {
        f64::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    pub fn set(&self, index: usize, value: f64) {
        self.values[index].store(value.to_bits(), Ordering::Relaxed);
    }
}

/// Read view handed to the audio callback.
pub struct ParamValues<'a> {
    pub(crate) store: &'a ParamStore,
    pub(crate) defs: &'static [ParamDef],
}

impl<'a> ParamValues<'a> {
    /// Host-free view over a raw store, for native tools and tests.
    pub fn preview(store: &'a ParamStore, defs: &'static [ParamDef]) -> Self {
        Self { store, defs }
    }
}

impl ParamValues<'_> {
    /// Normalized 0..1 value by index in `Dsp::PARAMS`.
    pub fn normalized(&self, index: usize) -> f64 {
        self.store.get(index)
    }

    /// Plain (display-range) value by index in `Dsp::PARAMS`.
    pub fn plain(&self, index: usize) -> f64 {
        self.defs[index].plain(self.store.get(index))
    }
}

/// Mouse state delivered with every editor input event, in editor-local
/// pixel coordinates (can be outside the window during a captured drag).
#[derive(Clone, Copy, Debug)]
pub struct MouseInput {
    pub x: f32,
    pub y: f32,
    pub shift: bool,
    pub ctrl: bool,
}

/// A plugin editor: lives in a child window inside the host's window, on
/// the host's UI thread. `render` fires ~60 Hz while attached.
pub trait Editor: 'static {
    /// Fixed editor size in pixels (width, height).
    fn size(&self) -> (u32, u32);
    /// The child window exists; create rendering resources on it.
    fn attached(&mut self, hwnd: *mut std::ffi::c_void);
    /// The window is going away; drop rendering resources.
    fn removed(&mut self);
    /// Draw one frame.
    fn render(&mut self);

    // Mouse input (defaults: ignore). The left button is captured between
    // down and up, so drags keep streaming even outside the window.
    fn mouse_down(&mut self, _m: MouseInput) {}
    fn mouse_up(&mut self, _m: MouseInput) {}
    fn mouse_move(&mut self, _m: MouseInput) {}
    /// `delta` is in wheel notches (+ = away from the user).
    fn mouse_wheel(&mut self, _m: MouseInput, _delta: f32) {}
    fn double_click(&mut self, _m: MouseInput) {}

    // Keyboard input (defaults: ignore). The child window takes Win32 focus
    // on click, so keys stream here until the user clicks back into the host.
    /// A translated printable character arrived (control chars filtered out).
    fn key_char(&mut self, _ch: char) {}
    /// A raw key went down (Windows virtual-key code: 0x0D Enter, 0x1B Esc,
    /// 0x08 Backspace, 0x25/0x27 arrows, ...).
    fn key_down(&mut self, _vk: u32) {}
    /// The child window lost keyboard focus (commit pending text entry).
    fn focus_lost(&mut self) {}
    /// True while the editor wants raw keyboard input (a text field is
    /// active). Gates both IPlugView::onKeyDown consumption — so host
    /// shortcuts keep working otherwise — and focus reassertion.
    fn wants_keys(&self) -> bool {
        false
    }

    // Dynamic sizing (defaults: fixed size). A face that wants a new window
    // size leaves a request here; the view polls it after each frame and
    // negotiates with the host (IPlugFrame::resizeView -> onSize).
    /// Take the pending resize request, if any (width, height).
    fn take_resize_request(&mut self) -> Option<(u32, u32)> {
        None
    }
    /// The window is now this size; adjust surfaces. `size()` must report
    /// the new size from here on.
    fn resized(&mut self, _width: u32, _height: u32) {}
}

pub type EditorFactory = fn(ParamsHandle) -> Box<dyn Editor>;

/// A view of the plugin's live parameters for the editor (UI thread).
///
/// Internally raw pointers into the instance (atomic value store + the
/// IComponentHandler slot): valid for as long as the plugin instance lives,
/// which the view guarantees by holding a COM reference on the instance for
/// its own lifetime.
#[derive(Clone, Copy)]
pub struct ParamsHandle {
    store: *const ParamStore,
    defs: &'static [ParamDef],
    handler: *const std::sync::atomic::AtomicPtr<std::ffi::c_void>,
    meters: *const MeterStore,
}

impl ParamsHandle {
    /// Host-free handle over caller-owned stores, for tools that drive a
    /// face without a plugin instance (the preview harness). Edit gestures
    /// still write the store; the host callbacks no-op while `handler`
    /// holds null. Caller keeps all referents alive for the handle's
    /// lifetime — leak them in tools.
    pub fn preview(
        store: &ParamStore,
        defs: &'static [ParamDef],
        handler: &std::sync::atomic::AtomicPtr<std::ffi::c_void>,
        meters: &MeterStore,
    ) -> Self {
        Self::new(store, defs, handler, meters)
    }

    pub(crate) fn new(
        store: &ParamStore,
        defs: &'static [ParamDef],
        handler: &std::sync::atomic::AtomicPtr<std::ffi::c_void>,
        meters: &MeterStore,
    ) -> Self {
        Self {
            store,
            defs,
            handler,
            meters,
        }
    }

    /// Latest meter value from the audio thread (0.0 if out of range).
    pub fn meter(&self, index: usize) -> f64 {
        unsafe {
            if index < (*self.meters).len() {
                (*self.meters).get(index)
            } else {
                0.0
            }
        }
    }

    /// Write a meter slot from the editor — for UI-initiated resets of
    /// DSP-accumulated values (held peaks). The audio thread must treat such
    /// slots as read-modify-write (`max(current, new)`) rather than keeping
    /// its own copy, or the reset gets clobbered.
    pub fn meter_set(&self, index: usize, value: f64) {
        unsafe {
            if index < (*self.meters).len() {
                (*self.meters).set(index, value);
            }
        }
    }

    pub fn count(&self) -> usize {
        self.defs.len()
    }

    pub fn def(&self, index: usize) -> &'static ParamDef {
        &self.defs[index]
    }

    pub fn normalized(&self, index: usize) -> f64 {
        unsafe { (*self.store).get(index) }
    }

    pub fn plain(&self, index: usize) -> f64 {
        self.defs[index].plain(self.normalized(index))
    }

    /// Set from the UI without telling the host. Prefer the begin/perform/end
    /// trio so the host records automation properly.
    pub fn set_normalized(&self, index: usize, value: f64) {
        unsafe { (*self.store).set(index, value) }
    }

    fn with_handler(&self, f: impl FnOnce(*mut std::ffi::c_void, &crate::interfaces::IComponentHandlerVtbl)) {
        unsafe {
            let h = (*self.handler).load(Ordering::Acquire);
            if !h.is_null() {
                let vtbl = &*(*(h as *mut crate::interfaces::IComponentHandlerPtr)).vtbl;
                f(h, vtbl);
            }
        }
    }

    /// Tell the host a user gesture on this parameter started (one per
    /// gesture — a drag is one begin, many performs, one end).
    pub fn begin_edit(&self, index: usize) {
        let id = self.defs[index].id;
        self.with_handler(|h, vtbl| unsafe {
            ((vtbl.begin_edit)(h, id));
        });
    }

    /// Set a new value AND notify the host, so it can record automation.
    pub fn perform_edit(&self, index: usize, value: f64) {
        let value = value.clamp(0.0, 1.0);
        unsafe { (*self.store).set(index, value) };
        let id = self.defs[index].id;
        self.with_handler(|h, vtbl| unsafe {
            ((vtbl.perform_edit)(h, id, value));
        });
    }

    /// Tell the host the gesture ended.
    pub fn end_edit(&self, index: usize) {
        let id = self.defs[index].id;
        self.with_handler(|h, vtbl| unsafe {
            ((vtbl.end_edit)(h, id));
        });
    }
}

/// A note event delivered to an instrument's process block.
#[derive(Clone, Copy, Debug)]
pub struct NoteEvent {
    /// Offset into the block, in samples.
    pub sample_offset: u32,
    pub kind: NoteKind,
}

#[derive(Clone, Copy, Debug)]
pub enum NoteKind {
    On { pitch: u8, velocity: f32 },
    Off { pitch: u8 },
}

/// The audio-processing half of a plugin. Called on the host's audio thread;
/// keep it allocation-free.
///
/// Effects process in place: the host input has already been copied into
/// `buffers` when `process` runs. Instruments (`IS_INSTRUMENT`) get zeroed
/// buffers plus the block's note events via `process_with_events`.
pub trait Dsp: Send + 'static {
    const INFO: PluginInfo;
    const PARAMS: &'static [ParamDef];
    /// How many meter slots this plugin reports (0 = none).
    const METERS: usize = 0;
    /// Editor factory; None = host draws its generic inline UI only.
    const EDITOR: Option<EditorFactory> = None;
    /// True = no audio input, an event input bus, zeroed output buffers.
    /// Pair with an "Instrument|..." subcategory in `INFO`.
    const IS_INSTRUMENT: bool = false;

    fn new() -> Self;

    /// Sample rate / block size are known. Allocate what you need here.
    fn setup(&mut self, sample_rate: f64, max_block_size: usize);

    /// Clear delay lines, envelopes, etc.
    fn reset(&mut self);

    /// Process `buffers` (one slice per channel, stereo) in place.
    /// Write live meter values (gain reduction etc.) into `meters`.
    /// Effects implement this; instruments may leave it and implement
    /// `process_with_events` instead.
    fn process(&mut self, _buffers: &mut [&mut [f32]], _params: &ParamValues, _meters: &MeterStore) {
    }

    /// Host transport at block start (defaults: ignore). `tempo` is BPM or
    /// 0.0 when the host didn't say; `ppq` is the song position in quarter
    /// notes or NaN. Called before `process`/`process_with_events`.
    fn transport(&mut self, _tempo: f64, _ppq: f64, _playing: bool) {}

    /// Instrument entry point: `process`, plus this block's note events
    /// sorted by sample offset. The default forwards to `process`, so
    /// effects never see it.
    fn process_with_events(
        &mut self,
        buffers: &mut [&mut [f32]],
        _events: &[NoteEvent],
        params: &ParamValues,
        meters: &MeterStore,
    ) {
        self.process(buffers, params, meters);
    }
}
