//! vst3_probe — a miniature VST3 host.
//!
//! Loads a .vst3 DLL the way a real host does (LoadLibrary +
//! GetPluginFactory), then walks the entire plugin lifecycle: factory
//! introspection, instantiation, interface queries, bus setup, audio
//! processing with parameter automation queues, and state round-trips.
//! Run it under Wine against a cross-compiled plugin before ever touching
//! Ableton — crashes here cost seconds, crashes in Live cost minutes.
//!
//! Usage: vst3_probe <path-to-plugin-dll>

#![allow(clippy::missing_safety_doc)]

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use lantern_vst3::base::*;
use lantern_vst3::interfaces::*;

// ============================================================================
// Minimal kernel32 bindings
// ============================================================================

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read a vtable of type T from a COM object pointer.
unsafe fn vt<'a, T>(obj: *mut c_void) -> &'a T {
    &**(obj as *mut *const T)
}

macro_rules! check {
    ($cond:expr, $($msg:tt)*) => {
        if !($cond) {
            println!("PROBE_RESULT: FAIL ({})", format!($($msg)*));
            std::process::exit(1);
        }
    };
}

// ============================================================================
// Host-side COM objects: memory IBStream, parameter change queues
// ============================================================================

#[repr(C)]
struct MemStream {
    vtbl: &'static IBStreamVtbl,
    ref_count: AtomicU32,
    data: UnsafeCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl MemStream {
    fn create() -> *mut MemStream {
        Box::into_raw(Box::new(MemStream {
            vtbl: &MEM_STREAM_VTBL,
            ref_count: AtomicU32::new(1),
            data: UnsafeCell::new(Vec::new()),
            pos: Cell::new(0),
        }))
    }

    unsafe fn me<'a>(this: *mut c_void) -> &'a MemStream {
        &*(this as *const MemStream)
    }

    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_IBSTREAM) {
            Self::me(this).ref_count.fetch_add(1, Ordering::Relaxed);
            *obj = this;
            kResultOk
        } else {
            *obj = null_mut();
            kNoInterface
        }
    }
    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        Self::me(this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }
    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        let prev = Self::me(this).ref_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            drop(Box::from_raw(this as *mut MemStream));
            0
        } else {
            prev - 1
        }
    }
    unsafe extern "system" fn read(
        this: *mut c_void,
        buffer: *mut c_void,
        num_bytes: i32,
        num_read: *mut i32,
    ) -> tresult {
        let me = Self::me(this);
        let data = &*me.data.get();
        let pos = me.pos.get();
        let n = (num_bytes.max(0) as usize).min(data.len().saturating_sub(pos));
        std::ptr::copy_nonoverlapping(data.as_ptr().add(pos), buffer as *mut u8, n);
        me.pos.set(pos + n);
        if !num_read.is_null() {
            *num_read = n as i32;
        }
        kResultOk
    }
    unsafe extern "system" fn write(
        this: *mut c_void,
        buffer: *const c_void,
        num_bytes: i32,
        num_written: *mut i32,
    ) -> tresult {
        let me = Self::me(this);
        let data = &mut *me.data.get();
        let pos = me.pos.get();
        let n = num_bytes.max(0) as usize;
        if data.len() < pos + n {
            data.resize(pos + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(pos), n);
        me.pos.set(pos + n);
        if !num_written.is_null() {
            *num_written = n as i32;
        }
        kResultOk
    }
    unsafe extern "system" fn seek(
        this: *mut c_void,
        pos: i64,
        mode: i32,
        result: *mut i64,
    ) -> tresult {
        let me = Self::me(this);
        let len = (*me.data.get()).len() as i64;
        let new_pos = match mode {
            kIBSeekSet => pos,
            kIBSeekCur => me.pos.get() as i64 + pos,
            kIBSeekEnd => len + pos,
            _ => return kInvalidArgument,
        }
        .clamp(0, len);
        me.pos.set(new_pos as usize);
        if !result.is_null() {
            *result = new_pos;
        }
        kResultOk
    }
    unsafe extern "system" fn tell(this: *mut c_void, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        *pos = Self::me(this).pos.get() as i64;
        kResultOk
    }
}

static MEM_STREAM_VTBL: IBStreamVtbl = IBStreamVtbl {
    query_interface: MemStream::query_interface,
    add_ref: MemStream::add_ref,
    release: MemStream::release,
    read: MemStream::read,
    write: MemStream::write,
    seek: MemStream::seek,
    tell: MemStream::tell,
};

#[repr(C)]
struct ParamQueue {
    vtbl: &'static IParamValueQueueVtbl,
    id: u32,
    points: Vec<(i32, f64)>,
}

impl ParamQueue {
    unsafe fn me<'a>(this: *mut c_void) -> &'a ParamQueue {
        &*(this as *const ParamQueue)
    }
    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_IPARAM_VALUE_QUEUE) {
            *obj = this;
            kResultOk
        } else {
            *obj = null_mut();
            kNoInterface
        }
    }
    unsafe extern "system" fn add_ref(_this: *mut c_void) -> u32 {
        2 // stack-owned by the probe; refcounting is a no-op
    }
    unsafe extern "system" fn release(_this: *mut c_void) -> u32 {
        1
    }
    unsafe extern "system" fn get_parameter_id(this: *mut c_void) -> u32 {
        Self::me(this).id
    }
    unsafe extern "system" fn get_point_count(this: *mut c_void) -> i32 {
        Self::me(this).points.len() as i32
    }
    unsafe extern "system" fn get_point(
        this: *mut c_void,
        index: i32,
        sample_offset: *mut i32,
        value: *mut f64,
    ) -> tresult {
        let me = Self::me(this);
        let Some(&(offset, val)) = me.points.get(index.max(0) as usize) else {
            return kInvalidArgument;
        };
        if !sample_offset.is_null() {
            *sample_offset = offset;
        }
        if !value.is_null() {
            *value = val;
        }
        kResultOk
    }
    unsafe extern "system" fn add_point(
        _this: *mut c_void,
        _sample_offset: i32,
        _value: f64,
        _index: *mut i32,
    ) -> tresult {
        kNotImplemented
    }
}

static PARAM_QUEUE_VTBL: IParamValueQueueVtbl = IParamValueQueueVtbl {
    query_interface: ParamQueue::query_interface,
    add_ref: ParamQueue::add_ref,
    release: ParamQueue::release,
    get_parameter_id: ParamQueue::get_parameter_id,
    get_point_count: ParamQueue::get_point_count,
    get_point: ParamQueue::get_point,
    add_point: ParamQueue::add_point,
};

#[repr(C)]
struct ParamChanges {
    vtbl: &'static IParameterChangesVtbl,
    queues: Vec<*mut ParamQueue>,
}

impl ParamChanges {
    unsafe fn me<'a>(this: *mut c_void) -> &'a ParamChanges {
        &*(this as *const ParamChanges)
    }
    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_IPARAMETER_CHANGES) {
            *obj = this;
            kResultOk
        } else {
            *obj = null_mut();
            kNoInterface
        }
    }
    unsafe extern "system" fn add_ref(_this: *mut c_void) -> u32 {
        2
    }
    unsafe extern "system" fn release(_this: *mut c_void) -> u32 {
        1
    }
    unsafe extern "system" fn get_parameter_count(this: *mut c_void) -> i32 {
        Self::me(this).queues.len() as i32
    }
    unsafe extern "system" fn get_parameter_data(
        this: *mut c_void,
        index: i32,
    ) -> *mut IParamValueQueuePtr {
        match Self::me(this).queues.get(index.max(0) as usize) {
            Some(&q) => q as *mut IParamValueQueuePtr,
            None => null_mut(),
        }
    }
    unsafe extern "system" fn add_parameter_data(
        _this: *mut c_void,
        _id: *const u32,
        _index: *mut i32,
    ) -> *mut IParamValueQueuePtr {
        null_mut()
    }
}

static PARAM_CHANGES_VTBL: IParameterChangesVtbl = IParameterChangesVtbl {
    query_interface: ParamChanges::query_interface,
    add_ref: ParamChanges::add_ref,
    release: ParamChanges::release,
    get_parameter_count: ParamChanges::get_parameter_count,
    get_parameter_data: ParamChanges::get_parameter_data,
    add_parameter_data: ParamChanges::add_parameter_data,
};

// ============================================================================
// Host-side IComponentHandler: records the edit callbacks the editor sends
// ============================================================================

#[repr(C)]
struct HostHandler {
    vtbl: &'static IComponentHandlerVtbl,
    begins: AtomicU32,
    performs: AtomicU32,
    ends: AtomicU32,
    last_value: std::sync::atomic::AtomicU64,
}

impl HostHandler {
    fn create() -> *mut HostHandler {
        Box::into_raw(Box::new(HostHandler {
            vtbl: &HOST_HANDLER_VTBL,
            begins: AtomicU32::new(0),
            performs: AtomicU32::new(0),
            ends: AtomicU32::new(0),
            last_value: std::sync::atomic::AtomicU64::new(0),
        }))
    }
    unsafe fn me<'a>(this: *mut c_void) -> &'a HostHandler {
        &*(this as *const HostHandler)
    }
    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if tuid_eq(iid, &IID_FUNKNOWN) || tuid_eq(iid, &IID_ICOMPONENT_HANDLER) {
            *obj = this;
            kResultOk
        } else {
            *obj = null_mut();
            kNoInterface
        }
    }
    unsafe extern "system" fn add_ref(_this: *mut c_void) -> u32 {
        2 // probe-lifetime object
    }
    unsafe extern "system" fn release(_this: *mut c_void) -> u32 {
        1
    }
    unsafe extern "system" fn begin_edit(this: *mut c_void, _id: u32) -> tresult {
        Self::me(this).begins.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
    unsafe extern "system" fn perform_edit(this: *mut c_void, _id: u32, value: f64) -> tresult {
        let me = Self::me(this);
        me.performs.fetch_add(1, Ordering::Relaxed);
        me.last_value.store(value.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe extern "system" fn end_edit(this: *mut c_void, _id: u32) -> tresult {
        Self::me(this).ends.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
    unsafe extern "system" fn restart_component(_this: *mut c_void, _flags: i32) -> tresult {
        kResultOk
    }
}

static HOST_HANDLER_VTBL: IComponentHandlerVtbl = IComponentHandlerVtbl {
    query_interface: HostHandler::query_interface,
    add_ref: HostHandler::add_ref,
    release: HostHandler::release,
    begin_edit: HostHandler::begin_edit,
    perform_edit: HostHandler::perform_edit,
    end_edit: HostHandler::end_edit,
    restart_component: HostHandler::restart_component,
};

// ============================================================================
// The probe
// ============================================================================

const BLOCK: usize = 512;

unsafe fn process_block(
    processor: *mut c_void,
    input_value: f32,
    changes: *mut ParamChanges,
) -> (f32, f32) {
    let mut in_l = vec![input_value; BLOCK];
    let mut in_r = vec![input_value; BLOCK];
    let mut out_l = vec![0.0f32; BLOCK];
    let mut out_r = vec![0.0f32; BLOCK];

    let mut in_ptrs: [*mut c_void; 2] = [in_l.as_mut_ptr().cast(), in_r.as_mut_ptr().cast()];
    let mut out_ptrs: [*mut c_void; 2] = [out_l.as_mut_ptr().cast(), out_r.as_mut_ptr().cast()];

    let mut in_bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: in_ptrs.as_mut_ptr(),
    };
    let mut out_bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        buffers: out_ptrs.as_mut_ptr(),
    };

    let mut data = ProcessData {
        process_mode: kRealtime,
        symbolic_sample_size: kSample32,
        num_samples: BLOCK as i32,
        num_inputs: 1,
        num_outputs: 1,
        inputs: &mut in_bus,
        outputs: &mut out_bus,
        input_param_changes: if changes.is_null() {
            null_mut()
        } else {
            changes as *mut IParameterChangesPtr
        },
        output_param_changes: null_mut(),
        input_events: null_mut(),
        output_events: null_mut(),
        context: null_mut(),
    };

    let res = (vt::<IAudioProcessorVtbl>(processor).process)(processor, &mut data);
    check!(res == kResultOk, "process() returned {res}");
    (out_l[BLOCK - 1], out_r[BLOCK - 1])
}

fn main() {
    let dll_path = std::env::args().nth(1).unwrap_or_else(|| {
        "target/x86_64-pc-windows-gnu/release/lantern_gain.dll".to_string()
    });
    println!("== Lantern VST3 probe: {dll_path} ==");

    unsafe {
        // --- Load the module the way a host does ---
        let module = LoadLibraryW(wide(&dll_path).as_ptr());
        check!(!module.is_null(), "LoadLibraryW failed for {dll_path}");

        let init_dll = GetProcAddress(module, c"InitDll".as_ptr());
        if !init_dll.is_null() {
            let init: extern "system" fn() -> bool = std::mem::transmute(init_dll);
            check!(init(), "InitDll returned false");
        }

        let get_factory = GetProcAddress(module, c"GetPluginFactory".as_ptr());
        check!(!get_factory.is_null(), "GetPluginFactory export missing");
        let get_factory: extern "system" fn() -> *mut c_void = std::mem::transmute(get_factory);
        let factory = get_factory();
        check!(!factory.is_null(), "GetPluginFactory returned null");

        // --- Factory introspection ---
        let fvt = vt::<IPluginFactory3Vtbl>(factory);
        let mut finfo: PFactoryInfo = std::mem::zeroed();
        check!(
            (fvt.get_factory_info)(factory, &mut finfo) == kResultOk,
            "getFactoryInfo failed"
        );
        let class_count = (fvt.count_classes)(factory);
        check!(class_count == 1, "expected 1 class, got {class_count}");

        let mut cinfo: PClassInfo = std::mem::zeroed();
        check!(
            (fvt.get_class_info)(factory, 0, &mut cinfo) == kResultOk,
            "getClassInfo failed"
        );
        let category: String = cinfo
            .category
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8 as char)
            .collect();
        let name: String = cinfo
            .name
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8 as char)
            .collect();
        check!(
            category == "Audio Module Class",
            "unexpected category {category:?}"
        );
        println!("factory ok: class {name:?} category {category:?}");
        // Exact-value assertions only apply to the reference plugin.
        let is_gain = name == "Lantern Gain";

        let mut cinfo2: PClassInfo2 = std::mem::zeroed();
        check!(
            (fvt.get_class_info2)(factory, 0, &mut cinfo2) == kResultOk,
            "getClassInfo2 failed"
        );

        // --- Instantiate ---
        let mut component: *mut c_void = null_mut();
        let res = (fvt.create_instance)(factory, &cinfo.cid, &IID_ICOMPONENT, &mut component);
        check!(
            res == kResultOk && !component.is_null(),
            "createInstance failed ({res})"
        );

        let cvt = vt::<IComponentVtbl>(component);
        let mut processor: *mut c_void = null_mut();
        let mut controller: *mut c_void = null_mut();
        let mut ctx_req: *mut c_void = null_mut();
        check!(
            (cvt.query_interface)(component, &IID_IAUDIO_PROCESSOR, &mut processor) == kResultOk,
            "no IAudioProcessor"
        );
        check!(
            (cvt.query_interface)(component, &IID_IEDIT_CONTROLLER, &mut controller) == kResultOk,
            "no IEditController"
        );
        check!(
            (cvt.query_interface)(component, &IID_IPROCESS_CONTEXT_REQUIREMENTS, &mut ctx_req)
                == kResultOk,
            "no IProcessContextRequirements"
        );
        let mut bogus: *mut c_void = null_mut();
        check!(
            (cvt.query_interface)(component, &IID_ICONNECTION_POINT, &mut bogus) == kNoInterface,
            "unknown IID should return kNoInterface"
        );
        println!("instance ok: component/processor/controller/ctx-req all answer");

        check!(
            (cvt.initialize)(component, null_mut()) == kResultOk,
            "initialize failed"
        );

        // --- Controller introspection ---
        let evt = vt::<IEditControllerVtbl>(controller);
        let param_count = (evt.get_parameter_count)(controller);
        check!(param_count >= 1, "expected params, got {param_count}");
        let mut pinfo: ParameterInfo = std::mem::zeroed();
        for i in 0..param_count {
            check!(
                (evt.get_parameter_info)(controller, i, &mut pinfo) == kResultOk,
                "getParameterInfo({i}) failed"
            );
        }
        (evt.get_parameter_info)(controller, 0, &mut pinfo);
        let title = read_char16(pinfo.title.as_ptr(), 128);
        check!(!title.is_empty(), "param 0 has no title");
        check!(
            (pinfo.flags & kCanAutomate) != 0,
            "param 0 should be automatable"
        );

        let mut text: String128 = [0; 128];
        check!(
            (evt.get_param_string_by_value)(controller, 0, 0.5, text.as_mut_ptr()) == kResultOk,
            "getParamStringByValue failed"
        );
        let display = read_char16(text.as_ptr(), 128);
        if is_gain {
            check!(param_count == 1, "gain: expected 1 param, got {param_count}");
            check!(title == "Gain", "unexpected param title {title:?}");
            check!(display == "+0.0", "0 dB should display as +0.0, got {display:?}");
            let plain = (evt.normalized_param_to_plain)(controller, 0, 1.0);
            check!((plain - 24.0).abs() < 1e-9, "plain(1.0) = {plain}, want 24");
        }
        println!("controller ok: {param_count} params, param 0 {title:?} mid-value {display:?}");

        // Give the controller a host handler so editor gestures reach "Live".
        let handler = HostHandler::create();
        check!(
            (evt.set_component_handler)(controller, handler.cast()) == kResultOk,
            "setComponentHandler failed"
        );

        // --- Audio setup ---
        let pvt = vt::<IAudioProcessorVtbl>(processor);
        check!(
            (pvt.can_process_sample_size)(processor, kSample32) == kResultTrue,
            "must accept 32-bit"
        );
        let setup = ProcessSetup {
            process_mode: kRealtime,
            symbolic_sample_size: kSample32,
            max_samples_per_block: BLOCK as i32,
            sample_rate: 48_000.0,
        };
        check!(
            (pvt.setup_processing)(processor, &setup) == kResultOk,
            "setupProcessing failed"
        );
        (cvt.activate_bus)(component, kAudio, kInput, 0, 1);
        (cvt.activate_bus)(component, kAudio, kOutput, 0, 1);
        check!((cvt.set_active)(component, 1) == kResultOk, "setActive failed");
        (pvt.set_processing)(processor, 1);

        if is_gain {
            // --- Unity gain pass ---
            let (l, r) = process_block(processor, 0.25, null_mut());
            check!(
                (l - 0.25).abs() < 1e-4 && (r - 0.25).abs() < 1e-4,
                "unity gain: expected 0.25, got {l}/{r}"
            );
            println!("audio ok: unity gain passes 0.25 -> {l:.4}");

            // --- Automation via setParamNormalized (+24 dB) ---
            (evt.set_param_normalized)(controller, 0, 1.0);
            let mut last = (0.0, 0.0);
            for _ in 0..20 {
                last = process_block(processor, 0.25, null_mut());
            }
            let expected = 0.25 * 10f32.powf(24.0 / 20.0);
            check!(
                (last.0 - expected).abs() / expected < 0.01,
                "+24dB: expected {expected}, got {}",
                last.0
            );
            println!("params ok: +24 dB via controller -> {:.4}", last.0);

            // --- Automation via IParameterChanges queue (back to 0 dB) ---
            let mut queue = ParamQueue {
                vtbl: &PARAM_QUEUE_VTBL,
                id: 0,
                points: vec![(0, 0.25), (BLOCK as i32 - 1, 0.5)],
            };
            let mut changes = ParamChanges {
                vtbl: &PARAM_CHANGES_VTBL,
                queues: vec![&mut queue as *mut ParamQueue],
            };
            let mut last = (0.0, 0.0);
            for i in 0..20 {
                last = process_block(
                    processor,
                    0.25,
                    if i == 0 { &mut changes } else { null_mut() },
                );
            }
            check!(
                (last.0 - 0.25).abs() < 1e-3,
                "queue automation back to 0 dB: got {}",
                last.0
            );
            let now = (evt.get_param_normalized)(controller, 0);
            check!(
                (now - 0.5).abs() < 1e-9,
                "controller should see queued value 0.5, got {now}"
            );
            println!("params ok: automation queue -> 0 dB -> {:.4}", last.0);
        } else {
            // Generic: audio must flow and stay finite, and queue automation
            // must land on the controller.
            let mut last = (0.0, 0.0);
            for _ in 0..10 {
                last = process_block(processor, 0.25, null_mut());
            }
            check!(
                last.0.is_finite() && last.1.is_finite(),
                "non-finite output {:?}",
                last
            );
            println!("audio ok: 10 blocks processed, output finite ({:.4})", last.0);
            let mut queue = ParamQueue {
                vtbl: &PARAM_QUEUE_VTBL,
                id: 0,
                points: vec![(0, 0.6)],
            };
            let mut changes = ParamChanges {
                vtbl: &PARAM_CHANGES_VTBL,
                queues: vec![&mut queue as *mut ParamQueue],
            };
            process_block(processor, 0.25, &mut changes);
            let now = (evt.get_param_normalized)(controller, 0);
            check!((now - 0.6).abs() < 1e-9, "queue value didn't land: {now}");
            println!("params ok: automation queue landed on param 0");
        }

        // --- State round-trip ---
        let stream = MemStream::create();
        (evt.set_param_normalized)(controller, 0, 0.75);
        check!(
            (cvt.get_state)(component, stream as *mut IBStreamPtr) == kResultOk,
            "getState failed"
        );
        (evt.set_param_normalized)(controller, 0, 0.1);
        // Rewind and restore.
        let svt = vt::<IBStreamVtbl>(stream.cast());
        let mut newpos = 0i64;
        (svt.seek)(stream.cast(), 0, kIBSeekSet, &mut newpos);
        check!(
            (cvt.set_state)(component, stream as *mut IBStreamPtr) == kResultOk,
            "setState failed"
        );
        let restored = (evt.get_param_normalized)(controller, 0);
        check!(
            (restored - 0.75).abs() < 1e-9,
            "state round-trip: want 0.75, got {restored}"
        );
        // setComponentState path (what the controller gets on project load).
        (svt.seek)(stream.cast(), 0, kIBSeekSet, &mut newpos);
        check!(
            (evt.set_component_state)(controller, stream as *mut IBStreamPtr) == kResultOk,
            "setComponentState failed"
        );
        (svt.release)(stream.cast());
        println!("state ok: save/restore round-trips through IBStream");

        // --- Editor hosting (--gui): the child-window wgpu crash test ---
        if std::env::args().any(|a| a == "--gui") {
            let view = (evt.create_view)(controller, c"editor".as_ptr());
            check!(!view.is_null(), "create_view returned null (no editor?)");
            let vvt = vt::<IPlugViewVtbl>(view);
            check!(
                (vvt.is_platform_type_supported)(view, c"HWND".as_ptr()) == kResultTrue,
                "HWND platform type not supported"
            );
            let mut rect = ViewRect::default();
            check!((vvt.get_size)(view, &mut rect) == kResultOk, "getSize failed");
            let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
            check!(w > 0 && h > 0, "bad view size {w}x{h}");
            println!("gui: view reports {w}x{h}");

            let host = create_host_window(w, h);
            check!(!host.is_null(), "host window creation failed");
            check!(
                (vvt.attached)(view, host, c"HWND".as_ptr()) == kResultOk,
                "attached() failed"
            );
            println!("gui: attached, letting it render...");
            // The state-round-trip test left the param at 0.75; reset to the
            // default so the drag test starts from a known value.
            (evt.set_param_normalized)(controller, 0, 0.5);
            pump_ms(1200);

            // --- Synthetic knob drag: down at the knob, 40px up, release ---
            let child = GetWindow(host, GW_CHILD);
            check!(!child.is_null(), "no child editor window found");
            let argv: Vec<String> = std::env::args().collect();
            let (kx, mut ky) = argv
                .iter()
                .position(|a| a == "--drag")
                .and_then(|i| {
                    Some((
                        argv.get(i + 1)?.parse::<i32>().ok()?,
                        argv.get(i + 2)?.parse::<i32>().ok()?,
                    ))
                })
                .unwrap_or((320, 196));
            // The editor SetCaptures on mouse-down, so the REAL cursor's
            // position also streams into the drag. Park it exactly at the
            // drag's end position so synthetic and real coords agree.
            let mut park = WINPOINT { x: kx, y: ky - 40 };
            ClientToScreen(child, &mut park);
            SetCursorPos(park.x, park.y);
            pump_ms(50);
            PostMessageW(child, WM_LBUTTONDOWN, MK_LBUTTON, lp(kx, ky));
            pump_ms(120);
            for _ in 0..8 {
                ky -= 5;
                PostMessageW(child, WM_MOUSEMOVE, MK_LBUTTON, lp(kx, ky));
                pump_ms(40);
            }
            PostMessageW(child, WM_LBUTTONUP, 0, lp(kx, ky));
            pump_ms(200);

            let hh = &*handler;
            let begins = hh.begins.load(Ordering::Relaxed);
            let performs = hh.performs.load(Ordering::Relaxed);
            let ends = hh.ends.load(Ordering::Relaxed);
            check!(begins >= 1, "no beginEdit reached the host");
            check!(performs >= 2, "expected a stream of performEdits, got {performs}");
            check!(ends >= 1, "no endEdit reached the host");
            let dragged = (evt.get_param_normalized)(controller, 0);
            if is_gain {
                check!(
                    (dragged - 0.7).abs() < 0.02,
                    "40px drag up from 0.5 should land ~0.7, got {dragged}"
                );
            } else {
                // Loose bar: the editor polls the REAL keyboard for
                // shift-fine-drag, so a human typing during the probe can
                // shrink the drag 10x. Any clear movement proves the path.
                check!(
                    (dragged - 0.5).abs() > 0.015,
                    "drag didn't move param 0 (still {dragged})"
                );
            }
            println!(
                "automation ok: beginEdit={begins} performEdit={performs} endEdit={ends} value={dragged:.3}"
            );
            pump_ms(600);

            check!((vvt.removed)(view) == kResultOk, "removed() failed");
            let vrem = (vvt.release)(view);
            check!(vrem == 0, "view leaked: refcount {vrem}");
            DestroyWindow(host);
            println!("gui ok: editor attached, rendered, detached cleanly");
        }

        // --- Teardown ---
        (pvt.set_processing)(processor, 0);
        (cvt.set_active)(component, 0);
        (cvt.terminate)(component);
        (vt::<IAudioProcessorVtbl>(processor).release)(processor);
        (vt::<IEditControllerVtbl>(controller).release)(controller);
        (vt::<IProcessContextRequirementsVtbl>(ctx_req).release)(ctx_req);
        let remaining = (cvt.release)(component);
        check!(remaining == 0, "instance leaked: refcount {remaining}");
        let fresult = (fvt.release)(factory);
        check!(fresult == 0, "factory leaked: refcount {fresult}");

        let exit_dll = GetProcAddress(module, c"ExitDll".as_ptr());
        if !exit_dll.is_null() {
            let exit: extern "system" fn() -> bool = std::mem::transmute(exit_dll);
            exit();
        }
        FreeLibrary(module);
    }

    println!("PROBE_RESULT: OK (full lifecycle, audio, automation, state)");
}

// ============================================================================
// user32 bindings for --gui mode (host window + message pump)
// ============================================================================

#[allow(non_snake_case)]
#[repr(C)]
struct WNDCLASSEXW {
    cbSize: u32,
    style: u32,
    lpfnWndProc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: *mut c_void,
    hIcon: *mut c_void,
    hCursor: *mut c_void,
    hbrBackground: *mut c_void,
    lpszMenuName: *const u16,
    lpszClassName: *const u16,
    hIconSm: *mut c_void,
}

#[allow(non_snake_case)]
#[repr(C)]
struct MSG {
    hwnd: *mut c_void,
    message: u32,
    wParam: usize,
    lParam: isize,
    time: u32,
    pt: [i32; 2],
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const PM_REMOVE: u32 = 0x0001;
const IDC_ARROW: usize = 32512;
const GW_CHILD: u32 = 5;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const MK_LBUTTON: usize = 0x0001;

/// Pack client coordinates into a mouse-message lparam.
fn lp(x: i32, y: i32) -> isize {
    (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize
}

/// Pump the message queue for `ms` milliseconds.
unsafe fn pump_ms(ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> *mut c_void;
    fn DestroyWindow(hwnd: *mut c_void) -> i32;
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn PeekMessageW(msg: *mut MSG, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const MSG) -> i32;
    fn DispatchMessageW(msg: *const MSG) -> isize;
    fn LoadCursorW(instance: *mut c_void, name: *const u16) -> *mut c_void;
    fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn GetWindow(hwnd: *mut c_void, cmd: u32) -> *mut c_void;
    fn ClientToScreen(hwnd: *mut c_void, point: *mut WINPOINT) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
}

#[repr(C)]
struct WINPOINT {
    x: i32,
    y: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

unsafe extern "system" fn host_wndproc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn create_host_window(w: i32, h: i32) -> *mut c_void {
    let class_name = wide("LanternProbeHost");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: host_wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: GetModuleHandleW(null()),
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW as *const u16),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: null_mut(),
    };
    RegisterClassExW(&wc);
    // Oversize the outer window; the child sits at (0,0) in the client area.
    CreateWindowExW(
        0,
        class_name.as_ptr(),
        wide("Lantern Editor Probe").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        80,
        80,
        w + 20,
        h + 45,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null_mut(),
    )
}
