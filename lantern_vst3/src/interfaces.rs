//! The VST3 binary interface: vtable layouts and ABI structs, hand-written
//! against Steinberg's spec (interface IDs and method order cross-checked
//! with the vst3-sys reference).
//!
//! Every interface is a C++ object: a pointer to a struct whose first field
//! points to a vtable of `extern "system"` function pointers. Method order
//! is ABI — a swapped pair means calling the wrong function, so the vtable
//! structs below list methods in exactly the C++ declaration order, base
//! interface first (FUnknown, then IPluginBase where inherited, then the
//! interface's own methods).

#![allow(non_upper_case_globals)]

use std::ffi::c_void;

use crate::base::{iid, tresult, String128, TBool, TChar, TUID};

// ============================================================================
// Interface IDs (canonical GUIDs, stored in Windows/COM byte order)
// ============================================================================

pub const IID_FUNKNOWN: TUID = iid(0x00000000, 0x0000, 0x0000, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IPLUGIN_BASE: TUID = iid(
    0x22888DDB, 0x156E, 0x45AE, [0x83, 0x58, 0xB3, 0x48, 0x08, 0x19, 0x06, 0x25],
);
pub const IID_IPLUGIN_FACTORY: TUID = iid(
    0x7A4D811C, 0x5211, 0x4A1F, [0xAE, 0xD9, 0xD2, 0xEE, 0x0B, 0x43, 0xBF, 0x9F],
);
pub const IID_IPLUGIN_FACTORY2: TUID = iid(
    0x0007B650, 0xF24B, 0x4C0B, [0xA4, 0x64, 0xED, 0xB9, 0xF0, 0x0B, 0x2A, 0xBB],
);
pub const IID_IPLUGIN_FACTORY3: TUID = iid(
    0x4555A2AB, 0xC123, 0x4E57, [0x9B, 0x12, 0x29, 0x10, 0x36, 0x87, 0x89, 0x31],
);
pub const IID_ICOMPONENT: TUID = iid(
    0xE831FF31, 0xF2D5, 0x4301, [0x92, 0x8E, 0xBB, 0xEE, 0x25, 0x69, 0x78, 0x02],
);
pub const IID_IAUDIO_PROCESSOR: TUID = iid(
    0x42043F99, 0xB7DA, 0x453C, [0xA5, 0x69, 0xE7, 0x9D, 0x9A, 0xAE, 0xC3, 0x3D],
);
pub const IID_IPROCESS_CONTEXT_REQUIREMENTS: TUID = iid(
    0x2A654303, 0xEF76, 0x4E3D, [0x95, 0xB5, 0xFE, 0x83, 0x73, 0x0E, 0xF6, 0xD0],
);
pub const IID_IEDIT_CONTROLLER: TUID = iid(
    0xDCD7BBE3, 0x7742, 0x448D, [0xA8, 0x74, 0xAA, 0xCC, 0x97, 0x9C, 0x75, 0x9E],
);
pub const IID_ICOMPONENT_HANDLER: TUID = iid(
    0x93A0BEA3, 0x0BD0, 0x45DB, [0x8E, 0x89, 0x0B, 0x0C, 0xC1, 0xE4, 0x6A, 0xC6],
);
pub const IID_IBSTREAM: TUID = iid(
    0xC3BF6EA2, 0x3099, 0x4752, [0x9B, 0x6B, 0xF9, 0x90, 0x1E, 0xE3, 0x3E, 0x9B],
);
pub const IID_IPARAMETER_CHANGES: TUID = iid(
    0xA4779663, 0x0BB6, 0x4A56, [0xB4, 0x43, 0x84, 0xA8, 0x46, 0x6F, 0xEB, 0x9D],
);
pub const IID_IPARAM_VALUE_QUEUE: TUID = iid(
    0x01263A18, 0xED07, 0x4F6F, [0x98, 0xC9, 0xD3, 0x56, 0x46, 0x86, 0xF9, 0xBA],
);
pub const IID_IPLUG_VIEW: TUID = iid(
    0x5BC32507, 0xD060, 0x49EA, [0xA6, 0x15, 0x1B, 0x52, 0x2B, 0x75, 0x5B, 0x29],
);
pub const IID_ICONNECTION_POINT: TUID = iid(
    0x70A4156F, 0x6E6E, 0x4026, [0x98, 0x91, 0x48, 0xBF, 0xAA, 0x60, 0xD8, 0xD1],
);

// ============================================================================
// Enums / constants (plain i32s in the ABI)
// ============================================================================

pub const kAudio: i32 = 0; // MediaTypes
pub const kEvent: i32 = 1;
pub const kInput: i32 = 0; // BusDirections
pub const kOutput: i32 = 1;
pub const kMain: i32 = 0; // BusTypes
pub const kAux: i32 = 1;
pub const kDefaultActive: u32 = 1; // BusFlags

pub const kSample32: i32 = 0;
pub const kSample64: i32 = 1;
pub const kRealtime: i32 = 0;

/// SpeakerArrangement bitmask: L | R.
pub const SPEAKER_STEREO: u64 = 0x3;

pub const kManyInstances: i32 = 0x7FFF_FFFF;
pub const kFactoryUnicode: i32 = 16;

// ParameterInfo flags
pub const kCanAutomate: i32 = 1;
pub const kIsBypass: i32 = 1 << 16;

// IBStream seek modes
pub const kIBSeekSet: i32 = 0;
pub const kIBSeekCur: i32 = 1;
pub const kIBSeekEnd: i32 = 2;

pub const kRootUnitId: i32 = 0;

// ============================================================================
// ABI structs
// ============================================================================

#[repr(C)]
pub struct PFactoryInfo {
    pub vendor: [i8; 64],
    pub url: [i8; 256],
    pub email: [i8; 128],
    pub flags: i32,
}

#[repr(C)]
pub struct PClassInfo {
    pub cid: TUID,
    pub cardinality: i32,
    pub category: [i8; 32],
    pub name: [i8; 64],
}

#[repr(C)]
pub struct PClassInfo2 {
    pub cid: TUID,
    pub cardinality: i32,
    pub category: [i8; 32],
    pub name: [i8; 64],
    pub class_flags: u32,
    pub subcategories: [i8; 128],
    pub vendor: [i8; 64],
    pub version: [i8; 64],
    pub sdk_version: [i8; 64],
}

#[repr(C)]
pub struct PClassInfoW {
    pub cid: TUID,
    pub cardinality: i32,
    pub category: [i8; 32],
    pub name: [i16; 64],
    pub class_flags: u32,
    pub subcategories: [i8; 128],
    pub vendor: [i16; 64],
    pub version: [i16; 64],
    pub sdk_version: [i16; 64],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: String128,
    pub bus_type: i32,
    pub flags: u32,
}

#[repr(C)]
pub struct RoutingInfo {
    pub media_type: i32,
    pub bus_index: i32,
    pub channel: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct ProcessSetup {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    /// `float**` when 32-bit processing, `double**` when 64-bit.
    pub buffers: *mut *mut c_void,
}

#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_param_changes: *mut IParameterChangesPtr,
    pub output_param_changes: *mut IParameterChangesPtr,
    pub input_events: *mut c_void,
    pub output_events: *mut c_void,
    pub context: *mut c_void, // ProcessContext — not consumed yet
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParameterInfo {
    pub id: u32,
    pub title: String128,
    pub short_title: String128,
    pub units: String128,
    pub step_count: i32,
    pub default_normalized_value: f64,
    pub unit_id: i32,
    pub flags: i32,
}

// ============================================================================
// Vtables — plugin-side (we implement these)
// ============================================================================

// Every vtable below starts with FUnknown's three methods, written out
// explicitly: queryInterface, addRef, release. Field order is ABI.

#[repr(C)]
pub struct IPluginFactory3Vtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    // IPluginFactory
    pub get_factory_info: unsafe extern "system" fn(*mut c_void, *mut PFactoryInfo) -> tresult,
    pub count_classes: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_class_info: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo) -> tresult,
    pub create_instance: unsafe extern "system" fn(
        *mut c_void,
        *const TUID,
        *const TUID,
        *mut *mut c_void,
    ) -> tresult,
    // IPluginFactory2
    pub get_class_info2: unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfo2) -> tresult,
    // IPluginFactory3
    pub get_class_info_unicode:
        unsafe extern "system" fn(*mut c_void, i32, *mut PClassInfoW) -> tresult,
    pub set_host_context: unsafe extern "system" fn(*mut c_void, *mut c_void) -> tresult,
}

#[repr(C)]
pub struct IComponentVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    // IPluginBase
    pub initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> tresult,
    pub terminate: unsafe extern "system" fn(*mut c_void) -> tresult,
    // IComponent
    pub get_controller_class_id: unsafe extern "system" fn(*mut c_void, *mut TUID) -> tresult,
    pub set_io_mode: unsafe extern "system" fn(*mut c_void, i32) -> tresult,
    pub get_bus_count: unsafe extern "system" fn(*mut c_void, i32, i32) -> i32,
    pub get_bus_info:
        unsafe extern "system" fn(*mut c_void, i32, i32, i32, *mut BusInfo) -> tresult,
    pub get_routing_info:
        unsafe extern "system" fn(*mut c_void, *mut RoutingInfo, *mut RoutingInfo) -> tresult,
    pub activate_bus: unsafe extern "system" fn(*mut c_void, i32, i32, i32, TBool) -> tresult,
    pub set_active: unsafe extern "system" fn(*mut c_void, TBool) -> tresult,
    pub set_state: unsafe extern "system" fn(*mut c_void, *mut IBStreamPtr) -> tresult,
    pub get_state: unsafe extern "system" fn(*mut c_void, *mut IBStreamPtr) -> tresult,
}

#[repr(C)]
pub struct IAudioProcessorVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub set_bus_arrangements:
        unsafe extern "system" fn(*mut c_void, *mut u64, i32, *mut u64, i32) -> tresult,
    pub get_bus_arrangement: unsafe extern "system" fn(*mut c_void, i32, i32, *mut u64) -> tresult,
    pub can_process_sample_size: unsafe extern "system" fn(*mut c_void, i32) -> tresult,
    pub get_latency_samples: unsafe extern "system" fn(*mut c_void) -> u32,
    pub setup_processing: unsafe extern "system" fn(*mut c_void, *const ProcessSetup) -> tresult,
    pub set_processing: unsafe extern "system" fn(*mut c_void, TBool) -> tresult,
    pub process: unsafe extern "system" fn(*mut c_void, *mut ProcessData) -> tresult,
    pub get_tail_samples: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
pub struct IProcessContextRequirementsVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub get_process_context_requirements: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
pub struct IEditControllerVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    // IPluginBase
    pub initialize: unsafe extern "system" fn(*mut c_void, *mut c_void) -> tresult,
    pub terminate: unsafe extern "system" fn(*mut c_void) -> tresult,
    // IEditController
    pub set_component_state: unsafe extern "system" fn(*mut c_void, *mut IBStreamPtr) -> tresult,
    pub set_state: unsafe extern "system" fn(*mut c_void, *mut IBStreamPtr) -> tresult,
    pub get_state: unsafe extern "system" fn(*mut c_void, *mut IBStreamPtr) -> tresult,
    pub get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_parameter_info:
        unsafe extern "system" fn(*mut c_void, i32, *mut ParameterInfo) -> tresult,
    pub get_param_string_by_value:
        unsafe extern "system" fn(*mut c_void, u32, f64, *mut TChar) -> tresult,
    pub get_param_value_by_string:
        unsafe extern "system" fn(*mut c_void, u32, *const TChar, *mut f64) -> tresult,
    pub normalized_param_to_plain: unsafe extern "system" fn(*mut c_void, u32, f64) -> f64,
    pub plain_param_to_normalized: unsafe extern "system" fn(*mut c_void, u32, f64) -> f64,
    pub get_param_normalized: unsafe extern "system" fn(*mut c_void, u32) -> f64,
    pub set_param_normalized: unsafe extern "system" fn(*mut c_void, u32, f64) -> tresult,
    pub set_component_handler: unsafe extern "system" fn(*mut c_void, *mut c_void) -> tresult,
    pub create_view: unsafe extern "system" fn(*mut c_void, *const i8) -> *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct IPlugViewVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub is_platform_type_supported: unsafe extern "system" fn(*mut c_void, *const i8) -> tresult,
    pub attached: unsafe extern "system" fn(*mut c_void, *mut c_void, *const i8) -> tresult,
    pub removed: unsafe extern "system" fn(*mut c_void) -> tresult,
    pub on_wheel: unsafe extern "system" fn(*mut c_void, f32) -> tresult,
    pub on_key_down: unsafe extern "system" fn(*mut c_void, i16, i16, i16) -> tresult,
    pub on_key_up: unsafe extern "system" fn(*mut c_void, i16, i16, i16) -> tresult,
    pub get_size: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> tresult,
    pub on_size: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> tresult,
    pub on_focus: unsafe extern "system" fn(*mut c_void, TBool) -> tresult,
    pub set_frame: unsafe extern "system" fn(*mut c_void, *mut c_void) -> tresult,
    pub can_resize: unsafe extern "system" fn(*mut c_void) -> tresult,
    pub check_size_constraint: unsafe extern "system" fn(*mut c_void, *mut ViewRect) -> tresult,
}

// ============================================================================
// Vtables — host-side (we call these through raw pointers)
// ============================================================================

/// A pointer to a host COM object: first field is its vtable pointer.
/// `IBStreamPtr` etc. are the pointee types; you always hold `*mut` them.
#[repr(C)]
pub struct IBStreamPtr {
    pub vtbl: *const IBStreamVtbl,
}

#[repr(C)]
pub struct IBStreamVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub read: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut i32) -> tresult,
    pub write: unsafe extern "system" fn(*mut c_void, *const c_void, i32, *mut i32) -> tresult,
    pub seek: unsafe extern "system" fn(*mut c_void, i64, i32, *mut i64) -> tresult,
    pub tell: unsafe extern "system" fn(*mut c_void, *mut i64) -> tresult,
}

#[repr(C)]
pub struct IParameterChangesPtr {
    pub vtbl: *const IParameterChangesVtbl,
}

#[repr(C)]
pub struct IParameterChangesVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub get_parameter_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_parameter_data: unsafe extern "system" fn(*mut c_void, i32) -> *mut IParamValueQueuePtr,
    pub add_parameter_data:
        unsafe extern "system" fn(*mut c_void, *const u32, *mut i32) -> *mut IParamValueQueuePtr,
}

#[repr(C)]
pub struct IParamValueQueuePtr {
    pub vtbl: *const IParamValueQueueVtbl,
}

#[repr(C)]
pub struct IParamValueQueueVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub get_parameter_id: unsafe extern "system" fn(*mut c_void) -> u32,
    pub get_point_count: unsafe extern "system" fn(*mut c_void) -> i32,
    pub get_point: unsafe extern "system" fn(*mut c_void, i32, *mut i32, *mut f64) -> tresult,
    pub add_point: unsafe extern "system" fn(*mut c_void, i32, f64, *mut i32) -> tresult,
}

/// IPlugFrame: the host object a view asks for its own resize.
#[repr(C)]
pub struct IPlugFramePtr {
    pub vtbl: *const IPlugFrameVtbl,
}

#[repr(C)]
pub struct IPlugFrameVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub resize_view:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut ViewRect) -> tresult,
}

#[repr(C)]
pub struct IComponentHandlerPtr {
    pub vtbl: *const IComponentHandlerVtbl,
}

#[repr(C)]
pub struct IComponentHandlerVtbl {
    pub query_interface:
        unsafe extern "system" fn(*mut c_void, *const TUID, *mut *mut c_void) -> tresult,
    pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub begin_edit: unsafe extern "system" fn(*mut c_void, u32) -> tresult,
    pub perform_edit: unsafe extern "system" fn(*mut c_void, u32, f64) -> tresult,
    pub end_edit: unsafe extern "system" fn(*mut c_void, u32) -> tresult,
    pub restart_component: unsafe extern "system" fn(*mut c_void, i32) -> tresult,
}

// ============================================================================
// Safe-ish helpers over host objects
// ============================================================================

/// Borrowed host IBStream.
pub struct BStream(pub *mut IBStreamPtr);

impl BStream {
    /// # Safety
    /// `self.0` must be a valid host IBStream for the duration of the call.
    pub unsafe fn read_exact(&self, buf: &mut [u8]) -> Result<(), tresult> {
        let vtbl = &*(*self.0).vtbl;
        let mut done = 0usize;
        while done < buf.len() {
            let mut got: i32 = 0;
            let res = (vtbl.read)(
                self.0.cast(),
                buf.as_mut_ptr().add(done).cast(),
                (buf.len() - done) as i32,
                &mut got,
            );
            if res != crate::base::kResultOk || got <= 0 {
                return Err(crate::base::kResultFalse);
            }
            done += got as usize;
        }
        Ok(())
    }

    /// # Safety
    /// `self.0` must be a valid host IBStream for the duration of the call.
    pub unsafe fn write_all(&self, buf: &[u8]) -> Result<(), tresult> {
        let vtbl = &*(*self.0).vtbl;
        let mut done = 0usize;
        while done < buf.len() {
            let mut put: i32 = 0;
            let res = (vtbl.write)(
                self.0.cast(),
                buf.as_ptr().add(done).cast(),
                (buf.len() - done) as i32,
                &mut put,
            );
            if res != crate::base::kResultOk || put <= 0 {
                return Err(crate::base::kResultFalse);
            }
            done += put as usize;
        }
        Ok(())
    }
}

/// Call release() on any FUnknown-derived host object pointer.
///
/// # Safety
/// `obj` must be a valid COM object whose vtable starts with FUnknown.
pub unsafe fn release_funknown(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    // Every vtable starts with FUnknown; read the release slot generically.
    #[repr(C)]
    struct AnyVtbl {
        _qi: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }
    let vtbl = *(obj as *mut *const AnyVtbl);
    ((*vtbl).release)(obj);
}

/// Call add_ref() on any FUnknown-derived host object pointer.
///
/// # Safety
/// `obj` must be a valid COM object whose vtable starts with FUnknown.
pub unsafe fn addref_funknown(obj: *mut c_void) {
    if obj.is_null() {
        return;
    }
    #[repr(C)]
    struct AnyVtbl {
        _qi: usize,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        _release: usize,
    }
    let vtbl = *(obj as *mut *const AnyVtbl);
    ((*vtbl).add_ref)(obj);
}
