//! The plugin COM object: one allocation exposing IComponent,
//! IAudioProcessor, IEditController, and IProcessContextRequirements
//! (single-component plugin, like JUCE does it — Ableton is fine with this).
//!
//! Layout: four vtable pointers first (repr(C)), so a pointer to the struct
//! is a valid IComponent, and pointers to the 2nd/3rd/4th fields are valid
//! IAudioProcessor / IEditController / IProcessContextRequirements. Each
//! thunk subtracts its slot's offset to recover the instance.
//!
//! Threading: the host calls processor methods on the audio thread and
//! controller methods on the main thread, concurrently. Everything shared
//! is atomic (`ParamStore`, handler pointer, setup values); the DSP state in
//! `UnsafeCell` is only touched from audio-side calls plus setup/activate,
//! which the host contract serializes against process().

use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::base::*;
use crate::interfaces::*;
use crate::plugin::{Dsp, MeterStore, NoteEvent, NoteKind, ParamStore, ParamValues};

const STATE_MAGIC: u32 = u32::from_le_bytes(*b"LNTN");
const STATE_VERSION: u32 = 1;

#[repr(C)]
pub struct PluginInstance<D: Dsp> {
    component_vtbl: &'static IComponentVtbl,
    processor_vtbl: &'static IAudioProcessorVtbl,
    controller_vtbl: &'static IEditControllerVtbl,
    ctx_req_vtbl: &'static IProcessContextRequirementsVtbl,
    ref_count: AtomicU32,
    params: ParamStore,
    meters: MeterStore,
    sample_rate_bits: AtomicU64,
    max_block: AtomicUsize,
    handler: AtomicPtr<c_void>,
    dsp: std::cell::UnsafeCell<D>,
}

impl<D: Dsp> PluginInstance<D> {
    /// Heap-allocate an instance with refcount 1 (the caller's reference).
    pub fn create_raw() -> *mut Self {
        Box::into_raw(Box::new(Self {
            component_vtbl: Self::COMPONENT_VTBL,
            processor_vtbl: Self::PROCESSOR_VTBL,
            controller_vtbl: Self::CONTROLLER_VTBL,
            ctx_req_vtbl: Self::CTX_REQ_VTBL,
            ref_count: AtomicU32::new(1),
            params: ParamStore::new(D::PARAMS),
            meters: MeterStore::new(D::METERS),
            sample_rate_bits: AtomicU64::new(44_100f64.to_bits()),
            max_block: AtomicUsize::new(0),
            handler: AtomicPtr::new(null_mut()),
            dsp: std::cell::UnsafeCell::new(D::new()),
        }))
    }

    // ------------------------------------------------------------------
    // this-pointer recovery (each vtable slot is a distinct COM identity)
    // ------------------------------------------------------------------

    unsafe fn from_component<'a>(this: *mut c_void) -> &'a Self {
        &*(this as *const Self)
    }
    unsafe fn from_processor<'a>(this: *mut c_void) -> &'a Self {
        &*((this as *const u8).sub(offset_of!(Self, processor_vtbl)) as *const Self)
    }
    unsafe fn from_controller<'a>(this: *mut c_void) -> &'a Self {
        &*((this as *const u8).sub(offset_of!(Self, controller_vtbl)) as *const Self)
    }
    unsafe fn from_ctx_req<'a>(this: *mut c_void) -> &'a Self {
        &*((this as *const u8).sub(offset_of!(Self, ctx_req_vtbl)) as *const Self)
    }

    // ------------------------------------------------------------------
    // FUnknown
    // ------------------------------------------------------------------

    unsafe fn query_interface_impl(&self, iid: *const TUID, obj: *mut *mut c_void) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        let base = self as *const Self as *const u8;
        let out: *mut c_void = if tuid_eq(iid, &IID_FUNKNOWN)
            || tuid_eq(iid, &IID_IPLUGIN_BASE)
            || tuid_eq(iid, &IID_ICOMPONENT)
        {
            base as *mut c_void
        } else if tuid_eq(iid, &IID_IAUDIO_PROCESSOR) {
            base.add(offset_of!(Self, processor_vtbl)) as *mut c_void
        } else if tuid_eq(iid, &IID_IEDIT_CONTROLLER) {
            base.add(offset_of!(Self, controller_vtbl)) as *mut c_void
        } else if tuid_eq(iid, &IID_IPROCESS_CONTEXT_REQUIREMENTS) {
            base.add(offset_of!(Self, ctx_req_vtbl)) as *mut c_void
        } else {
            *obj = null_mut();
            return kNoInterface;
        };
        self.ref_count.fetch_add(1, Ordering::Relaxed);
        *obj = out;
        kResultOk
    }

    fn add_ref_impl(&self) -> u32 {
        self.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe fn release_impl(this: *const Self) -> u32 {
        let prev = (*this).ref_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            drop(Box::from_raw(this as *mut Self));
            0
        } else {
            prev - 1
        }
    }

    fn param_index(id: u32) -> Option<usize> {
        D::PARAMS.iter().position(|p| p.id == id)
    }

    // ------------------------------------------------------------------
    // State (shared by IComponent::set/getState and setComponentState)
    // ------------------------------------------------------------------

    unsafe fn write_state(&self, stream: *mut IBStreamPtr) -> tresult {
        if stream.is_null() {
            return kInvalidArgument;
        }
        let n = D::PARAMS.len();
        let mut buf = Vec::with_capacity(12 + n * 12);
        buf.extend_from_slice(&STATE_MAGIC.to_le_bytes());
        buf.extend_from_slice(&STATE_VERSION.to_le_bytes());
        buf.extend_from_slice(&(n as u32).to_le_bytes());
        for (i, def) in D::PARAMS.iter().enumerate() {
            buf.extend_from_slice(&def.id.to_le_bytes());
            buf.extend_from_slice(&self.params.get(i).to_le_bytes());
        }
        match BStream(stream).write_all(&buf) {
            Ok(()) => kResultOk,
            Err(e) => e,
        }
    }

    unsafe fn read_state(&self, stream: *mut IBStreamPtr) -> tresult {
        if stream.is_null() {
            return kInvalidArgument;
        }
        let s = BStream(stream);
        let mut header = [0u8; 12];
        if s.read_exact(&mut header).is_err() {
            return kResultFalse;
        }
        let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let count = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if magic != STATE_MAGIC || version == 0 || version > STATE_VERSION || count > 4096 {
            return kResultFalse;
        }
        for _ in 0..count {
            let mut entry = [0u8; 12];
            if s.read_exact(&mut entry).is_err() {
                return kResultFalse;
            }
            let id = u32::from_le_bytes(entry[0..4].try_into().unwrap());
            let value = f64::from_le_bytes(entry[4..12].try_into().unwrap());
            if let Some(idx) = Self::param_index(id) {
                self.params.set(idx, value.clamp(0.0, 1.0));
            }
        }
        kResultOk
    }

    // ==================================================================
    // IComponent thunks
    // ==================================================================

    unsafe extern "system" fn c_query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        Self::from_component(this).query_interface_impl(iid, obj)
    }
    unsafe extern "system" fn c_add_ref(this: *mut c_void) -> u32 {
        Self::from_component(this).add_ref_impl()
    }
    unsafe extern "system" fn c_release(this: *mut c_void) -> u32 {
        Self::release_impl(Self::from_component(this))
    }
    unsafe extern "system" fn c_initialize(_this: *mut c_void, _context: *mut c_void) -> tresult {
        kResultOk
    }
    unsafe extern "system" fn c_terminate(this: *mut c_void) -> tresult {
        let me = Self::from_component(this);
        let old = me.handler.swap(null_mut(), Ordering::AcqRel);
        release_funknown(old);
        kResultOk
    }
    unsafe extern "system" fn c_get_controller_class_id(
        _this: *mut c_void,
        _tuid: *mut TUID,
    ) -> tresult {
        // Single-component: no separate controller class.
        kResultFalse
    }
    unsafe extern "system" fn c_set_io_mode(_this: *mut c_void, _mode: i32) -> tresult {
        kNotImplemented
    }
    unsafe extern "system" fn c_get_bus_count(_this: *mut c_void, media: i32, dir: i32) -> i32 {
        if media == kAudio {
            // Instruments have no audio input.
            if D::IS_INSTRUMENT && dir == kInput {
                0
            } else {
                1
            }
        } else if media == kEvent && D::IS_INSTRUMENT && dir == kInput {
            1
        } else {
            0
        }
    }
    unsafe extern "system" fn c_get_bus_info(
        _this: *mut c_void,
        media: i32,
        dir: i32,
        index: i32,
        info: *mut BusInfo,
    ) -> tresult {
        if info.is_null() || index != 0 {
            return kInvalidArgument;
        }
        if media == kAudio {
            if D::IS_INSTRUMENT && dir == kInput {
                return kInvalidArgument;
            }
            let info = &mut *info;
            info.media_type = kAudio;
            info.direction = dir;
            info.channel_count = 2;
            write_char16(&mut info.name, if dir == kInput { "Input" } else { "Output" });
            info.bus_type = kMain;
            info.flags = kDefaultActive;
            kResultOk
        } else if media == kEvent && D::IS_INSTRUMENT && dir == kInput {
            let info = &mut *info;
            info.media_type = kEvent;
            info.direction = dir;
            info.channel_count = 16;
            write_char16(&mut info.name, "MIDI In");
            info.bus_type = kMain;
            info.flags = kDefaultActive;
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe extern "system" fn c_get_routing_info(
        _this: *mut c_void,
        _in_info: *mut RoutingInfo,
        _out_info: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }
    unsafe extern "system" fn c_activate_bus(
        _this: *mut c_void,
        media: i32,
        dir: i32,
        index: i32,
        _state: TBool,
    ) -> tresult {
        let valid = index == 0
            && match media {
                m if m == kAudio => !(D::IS_INSTRUMENT && dir == kInput),
                m if m == kEvent => D::IS_INSTRUMENT && dir == kInput,
                _ => false,
            };
        if valid {
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe extern "system" fn c_set_active(this: *mut c_void, state: TBool) -> tresult {
        let me = Self::from_component(this);
        if state != 0 {
            // Host contract: not processing while (de)activating.
            (*me.dsp.get()).reset();
        }
        kResultOk
    }
    unsafe extern "system" fn c_set_state(this: *mut c_void, state: *mut IBStreamPtr) -> tresult {
        Self::from_component(this).read_state(state)
    }
    unsafe extern "system" fn c_get_state(this: *mut c_void, state: *mut IBStreamPtr) -> tresult {
        Self::from_component(this).write_state(state)
    }

    const COMPONENT_VTBL: &'static IComponentVtbl = &IComponentVtbl {
        query_interface: Self::c_query_interface,
        add_ref: Self::c_add_ref,
        release: Self::c_release,
        initialize: Self::c_initialize,
        terminate: Self::c_terminate,
        get_controller_class_id: Self::c_get_controller_class_id,
        set_io_mode: Self::c_set_io_mode,
        get_bus_count: Self::c_get_bus_count,
        get_bus_info: Self::c_get_bus_info,
        get_routing_info: Self::c_get_routing_info,
        activate_bus: Self::c_activate_bus,
        set_active: Self::c_set_active,
        set_state: Self::c_set_state,
        get_state: Self::c_get_state,
    };

    // ==================================================================
    // IAudioProcessor thunks
    // ==================================================================

    unsafe extern "system" fn p_query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        Self::from_processor(this).query_interface_impl(iid, obj)
    }
    unsafe extern "system" fn p_add_ref(this: *mut c_void) -> u32 {
        Self::from_processor(this).add_ref_impl()
    }
    unsafe extern "system" fn p_release(this: *mut c_void) -> u32 {
        Self::release_impl(Self::from_processor(this))
    }
    unsafe extern "system" fn p_set_bus_arrangements(
        _this: *mut c_void,
        inputs: *mut u64,
        num_ins: i32,
        outputs: *mut u64,
        num_outs: i32,
    ) -> tresult {
        // Fixed stereo out; effects add a fixed stereo in.
        let ok = if D::IS_INSTRUMENT {
            num_ins == 0 && num_outs == 1 && !outputs.is_null() && *outputs == SPEAKER_STEREO
        } else {
            num_ins == 1
                && num_outs == 1
                && !inputs.is_null()
                && !outputs.is_null()
                && *inputs == SPEAKER_STEREO
                && *outputs == SPEAKER_STEREO
        };
        if ok {
            kResultTrue
        } else {
            kResultFalse
        }
    }
    unsafe extern "system" fn p_get_bus_arrangement(
        _this: *mut c_void,
        dir: i32,
        index: i32,
        arr: *mut u64,
    ) -> tresult {
        if arr.is_null() || index != 0 || (D::IS_INSTRUMENT && dir == kInput) {
            return kInvalidArgument;
        }
        *arr = SPEAKER_STEREO;
        kResultOk
    }
    unsafe extern "system" fn p_can_process_sample_size(
        _this: *mut c_void,
        size: i32,
    ) -> tresult {
        if size == kSample32 {
            kResultTrue
        } else {
            kResultFalse
        }
    }
    unsafe extern "system" fn p_get_latency_samples(_this: *mut c_void) -> u32 {
        0
    }
    unsafe extern "system" fn p_setup_processing(
        this: *mut c_void,
        setup: *const ProcessSetup,
    ) -> tresult {
        if setup.is_null() {
            return kInvalidArgument;
        }
        let me = Self::from_processor(this);
        let setup = &*setup;
        me.sample_rate_bits
            .store(setup.sample_rate.to_bits(), Ordering::Relaxed);
        me.max_block
            .store(setup.max_samples_per_block.max(0) as usize, Ordering::Relaxed);
        // Host contract: never called while processing.
        (*me.dsp.get()).setup(setup.sample_rate, setup.max_samples_per_block.max(0) as usize);
        kResultOk
    }
    unsafe extern "system" fn p_set_processing(_this: *mut c_void, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe extern "system" fn p_process(this: *mut c_void, data: *mut ProcessData) -> tresult {
        let me = Self::from_processor(this);
        if data.is_null() {
            return kInvalidArgument;
        }
        let data = &mut *data;

        // Apply parameter changes: last point of each queue wins for this
        // block (sample-accurate splitting can come later).
        let changes = data.input_param_changes;
        if !changes.is_null() {
            let cvt = &*(*changes).vtbl;
            let count = (cvt.get_parameter_count)(changes.cast());
            for qi in 0..count {
                let queue = (cvt.get_parameter_data)(changes.cast(), qi);
                if queue.is_null() {
                    continue;
                }
                let qvt = &*(*queue).vtbl;
                let id = (qvt.get_parameter_id)(queue.cast());
                let points = (qvt.get_point_count)(queue.cast());
                if points <= 0 {
                    continue;
                }
                let mut offset: i32 = 0;
                let mut value: f64 = 0.0;
                if (qvt.get_point)(queue.cast(), points - 1, &mut offset, &mut value) == kResultOk
                {
                    if let Some(idx) = Self::param_index(id) {
                        me.params.set(idx, value);
                    }
                }
            }
        }

        // Flush-only calls come with no buffers; that's fine.
        if data.num_samples <= 0 {
            return kResultOk;
        }
        if data.symbolic_sample_size != kSample32 {
            return kNotImplemented;
        }
        if data.num_outputs < 1 || data.outputs.is_null() {
            return kResultOk;
        }

        // Collect this block's note events (instruments only), sorted by
        // sample offset. Fixed-capacity: keep the audio thread heap-free.
        let mut events = [NoteEvent {
            sample_offset: 0,
            kind: NoteKind::Off { pitch: 0 },
        }; 128];
        let mut n_events = 0usize;
        if D::IS_INSTRUMENT && !data.input_events.is_null() {
            let list = data.input_events as *mut IEventListPtr;
            let vt = &*(*list).vtbl;
            let count = (vt.get_event_count)(list.cast());
            for i in 0..count {
                if n_events == events.len() {
                    break;
                }
                let mut ev = std::mem::zeroed::<Event>();
                if (vt.get_event)(list.cast(), i, &mut ev) != kResultOk {
                    continue;
                }
                let offset = ev.sample_offset.max(0) as u32;
                let kind = match ev.event_type {
                    K_NOTE_ON_EVENT => {
                        let on = ev.payload.note_on;
                        // MIDI convention: note-on at zero velocity is off.
                        if on.velocity > 0.0 {
                            NoteKind::On {
                                pitch: on.pitch.clamp(0, 127) as u8,
                                velocity: on.velocity,
                            }
                        } else {
                            NoteKind::Off {
                                pitch: on.pitch.clamp(0, 127) as u8,
                            }
                        }
                    }
                    K_NOTE_OFF_EVENT => NoteKind::Off {
                        pitch: ev.payload.note_off.pitch.clamp(0, 127) as u8,
                    },
                    _ => continue,
                };
                events[n_events] = NoteEvent {
                    sample_offset: offset,
                    kind,
                };
                n_events += 1;
            }
            events[..n_events].sort_unstable_by_key(|e| e.sample_offset);
        }

        let out_bus = &mut *data.outputs;
        let num_samples = data.num_samples as usize;
        if out_bus.buffers.is_null() {
            return kResultOk;
        }

        let mut ptrs: [*mut f32; 2] = [null_mut(); 2];
        let nch;
        if D::IS_INSTRUMENT {
            // No audio input: hand the DSP zeroed output buffers.
            nch = out_bus.num_channels.clamp(0, 2) as usize;
            if nch == 0 {
                return kResultOk;
            }
            for ch in 0..nch {
                let op = *out_bus.buffers.add(ch) as *mut f32;
                if op.is_null() {
                    return kResultOk;
                }
                std::ptr::write_bytes(op, 0, num_samples);
                ptrs[ch] = op;
            }
        } else {
            if data.num_inputs < 1 || data.inputs.is_null() {
                return kResultOk;
            }
            let in_bus = &*data.inputs;
            nch = in_bus.num_channels.min(out_bus.num_channels).clamp(0, 2) as usize;
            if nch == 0 || in_bus.buffers.is_null() {
                return kResultOk;
            }
            // Copy input into output (host may or may not process in
            // place), then hand the DSP the output buffers.
            for ch in 0..nch {
                let ip = *in_bus.buffers.add(ch) as *mut f32;
                let op = *out_bus.buffers.add(ch) as *mut f32;
                if ip.is_null() || op.is_null() {
                    return kResultOk;
                }
                if op != ip {
                    std::ptr::copy_nonoverlapping(ip, op, num_samples);
                }
                ptrs[ch] = op;
            }
        }

        let mut slices: [&mut [f32]; 2] = [
            std::slice::from_raw_parts_mut(ptrs[0], num_samples),
            if nch > 1 {
                std::slice::from_raw_parts_mut(ptrs[1], num_samples)
            } else {
                &mut []
            },
        ];

        let values = ParamValues {
            store: &me.params,
            defs: D::PARAMS,
        };
        (*me.dsp.get()).process_with_events(
            &mut slices[..nch],
            &events[..n_events],
            &values,
            &me.meters,
        );

        out_bus.silence_flags = 0;
        kResultOk
    }
    unsafe extern "system" fn p_get_tail_samples(_this: *mut c_void) -> u32 {
        0
    }

    const PROCESSOR_VTBL: &'static IAudioProcessorVtbl = &IAudioProcessorVtbl {
        query_interface: Self::p_query_interface,
        add_ref: Self::p_add_ref,
        release: Self::p_release,
        set_bus_arrangements: Self::p_set_bus_arrangements,
        get_bus_arrangement: Self::p_get_bus_arrangement,
        can_process_sample_size: Self::p_can_process_sample_size,
        get_latency_samples: Self::p_get_latency_samples,
        setup_processing: Self::p_setup_processing,
        set_processing: Self::p_set_processing,
        process: Self::p_process,
        get_tail_samples: Self::p_get_tail_samples,
    };

    // ==================================================================
    // IProcessContextRequirements thunks
    // ==================================================================

    unsafe extern "system" fn r_query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        Self::from_ctx_req(this).query_interface_impl(iid, obj)
    }
    unsafe extern "system" fn r_add_ref(this: *mut c_void) -> u32 {
        Self::from_ctx_req(this).add_ref_impl()
    }
    unsafe extern "system" fn r_release(this: *mut c_void) -> u32 {
        Self::release_impl(Self::from_ctx_req(this))
    }
    unsafe extern "system" fn r_get_requirements(_this: *mut c_void) -> u32 {
        0 // no process-context data needed yet (tempo sync comes later)
    }

    const CTX_REQ_VTBL: &'static IProcessContextRequirementsVtbl =
        &IProcessContextRequirementsVtbl {
            query_interface: Self::r_query_interface,
            add_ref: Self::r_add_ref,
            release: Self::r_release,
            get_process_context_requirements: Self::r_get_requirements,
        };

    // ==================================================================
    // IEditController thunks
    // ==================================================================

    unsafe extern "system" fn e_query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        Self::from_controller(this).query_interface_impl(iid, obj)
    }
    unsafe extern "system" fn e_add_ref(this: *mut c_void) -> u32 {
        Self::from_controller(this).add_ref_impl()
    }
    unsafe extern "system" fn e_release(this: *mut c_void) -> u32 {
        Self::release_impl(Self::from_controller(this))
    }
    unsafe extern "system" fn e_initialize(_this: *mut c_void, _context: *mut c_void) -> tresult {
        kResultOk
    }
    unsafe extern "system" fn e_terminate(this: *mut c_void) -> tresult {
        let me = Self::from_controller(this);
        let old = me.handler.swap(null_mut(), Ordering::AcqRel);
        release_funknown(old);
        kResultOk
    }
    unsafe extern "system" fn e_set_component_state(
        this: *mut c_void,
        state: *mut IBStreamPtr,
    ) -> tresult {
        // Single component: same parameter store, same format.
        Self::from_controller(this).read_state(state)
    }
    unsafe extern "system" fn e_set_state(_this: *mut c_void, _state: *mut IBStreamPtr) -> tresult {
        // No controller-only (GUI) state yet.
        kResultOk
    }
    unsafe extern "system" fn e_get_state(_this: *mut c_void, _state: *mut IBStreamPtr) -> tresult {
        kResultOk
    }
    unsafe extern "system" fn e_get_parameter_count(_this: *mut c_void) -> i32 {
        D::PARAMS.len() as i32
    }
    unsafe extern "system" fn e_get_parameter_info(
        _this: *mut c_void,
        index: i32,
        info: *mut ParameterInfo,
    ) -> tresult {
        if info.is_null() || index < 0 || index as usize >= D::PARAMS.len() {
            return kInvalidArgument;
        }
        let def = &D::PARAMS[index as usize];
        let info = &mut *info;
        info.id = def.id;
        write_char16(&mut info.title, def.title);
        write_char16(&mut info.short_title, def.short_title);
        write_char16(&mut info.units, def.units);
        info.step_count = def.step_count;
        info.default_normalized_value = def.default_normalized;
        info.unit_id = kRootUnitId;
        info.flags = if def.can_automate { kCanAutomate } else { 0 };
        kResultOk
    }
    unsafe extern "system" fn e_get_param_string_by_value(
        _this: *mut c_void,
        id: u32,
        value_normalized: f64,
        string: *mut TChar,
    ) -> tresult {
        let Some(idx) = Self::param_index(id) else {
            return kInvalidArgument;
        };
        if string.is_null() {
            return kInvalidArgument;
        }
        let text = D::PARAMS[idx].display(value_normalized);
        let dst = std::slice::from_raw_parts_mut(string, 128);
        write_char16(dst, &text);
        kResultOk
    }
    unsafe extern "system" fn e_get_param_value_by_string(
        _this: *mut c_void,
        id: u32,
        string: *const TChar,
        value_normalized: *mut f64,
    ) -> tresult {
        let Some(idx) = Self::param_index(id) else {
            return kInvalidArgument;
        };
        if string.is_null() || value_normalized.is_null() {
            return kInvalidArgument;
        }
        let text = read_char16(string, 128);
        let Ok(plain) = text.trim().trim_end_matches(char::is_alphabetic).trim().parse::<f64>()
        else {
            return kResultFalse;
        };
        *value_normalized = D::PARAMS[idx].normalized_from_plain(plain);
        kResultOk
    }
    unsafe extern "system" fn e_normalized_param_to_plain(
        _this: *mut c_void,
        id: u32,
        value_normalized: f64,
    ) -> f64 {
        match Self::param_index(id) {
            Some(idx) => D::PARAMS[idx].plain(value_normalized),
            None => 0.0,
        }
    }
    unsafe extern "system" fn e_plain_param_to_normalized(
        _this: *mut c_void,
        id: u32,
        plain: f64,
    ) -> f64 {
        match Self::param_index(id) {
            Some(idx) => D::PARAMS[idx].normalized_from_plain(plain),
            None => 0.0,
        }
    }
    unsafe extern "system" fn e_get_param_normalized(this: *mut c_void, id: u32) -> f64 {
        let me = Self::from_controller(this);
        match Self::param_index(id) {
            Some(idx) => me.params.get(idx),
            None => 0.0,
        }
    }
    unsafe extern "system" fn e_set_param_normalized(
        this: *mut c_void,
        id: u32,
        value: f64,
    ) -> tresult {
        let me = Self::from_controller(this);
        let Some(idx) = Self::param_index(id) else {
            return kInvalidArgument;
        };
        me.params.set(idx, value);
        kResultOk
    }
    unsafe extern "system" fn e_set_component_handler(
        this: *mut c_void,
        handler: *mut c_void,
    ) -> tresult {
        let me = Self::from_controller(this);
        addref_funknown(handler);
        let old = me.handler.swap(handler, Ordering::AcqRel);
        release_funknown(old);
        kResultOk
    }
    unsafe extern "system" fn e_create_view(this: *mut c_void, _name: *const i8) -> *mut c_void {
        let Some(factory) = D::EDITOR else {
            // No editor: the host draws its generic inline parameter panel.
            return null_mut();
        };
        let me = Self::from_controller(this);
        let editor = factory(crate::plugin::ParamsHandle::new(
            &me.params,
            D::PARAMS,
            &me.handler,
            &me.meters,
        ));
        // The view holds a COM ref on the component (the base of this object).
        let component = (this as *mut u8).sub(offset_of!(Self, controller_vtbl)) as *mut c_void;
        crate::view::PlugView::create(component, editor)
    }

    const CONTROLLER_VTBL: &'static IEditControllerVtbl = &IEditControllerVtbl {
        query_interface: Self::e_query_interface,
        add_ref: Self::e_add_ref,
        release: Self::e_release,
        initialize: Self::e_initialize,
        terminate: Self::e_terminate,
        set_component_state: Self::e_set_component_state,
        set_state: Self::e_set_state,
        get_state: Self::e_get_state,
        get_parameter_count: Self::e_get_parameter_count,
        get_parameter_info: Self::e_get_parameter_info,
        get_param_string_by_value: Self::e_get_param_string_by_value,
        get_param_value_by_string: Self::e_get_param_value_by_string,
        normalized_param_to_plain: Self::e_normalized_param_to_plain,
        plain_param_to_normalized: Self::e_plain_param_to_normalized,
        get_param_normalized: Self::e_get_param_normalized,
        set_param_normalized: Self::e_set_param_normalized,
        set_component_handler: Self::e_set_component_handler,
        create_view: Self::e_create_view,
    };
}

impl<D: Dsp> Drop for PluginInstance<D> {
    fn drop(&mut self) {
        let old = self.handler.swap(null_mut(), Ordering::AcqRel);
        unsafe { release_funknown(old) };
    }
}
