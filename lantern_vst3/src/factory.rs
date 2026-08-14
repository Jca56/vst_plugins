//! The plugin factory: the first object a host talks to. Advertises one
//! class (the plugin) and creates instances of it.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::base::*;
use crate::instance::PluginInstance;
use crate::interfaces::*;
use crate::plugin::Dsp;

#[repr(C)]
pub struct Factory<D: Dsp> {
    vtbl: &'static IPluginFactory3Vtbl,
    ref_count: AtomicU32,
    _marker: PhantomData<D>,
}

impl<D: Dsp> Factory<D> {
    /// Create a factory with refcount 1, returned as FUnknown/IPluginFactory.
    pub fn create() -> *mut c_void {
        Box::into_raw(Box::new(Self {
            vtbl: Self::VTBL,
            ref_count: AtomicU32::new(1),
            _marker: PhantomData,
        })) as *mut c_void
    }

    unsafe fn from_this<'a>(this: *mut c_void) -> &'a Self {
        &*(this as *const Self)
    }

    unsafe extern "system" fn query_interface(
        this: *mut c_void,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        if tuid_eq(iid, &IID_FUNKNOWN)
            || tuid_eq(iid, &IID_IPLUGIN_FACTORY)
            || tuid_eq(iid, &IID_IPLUGIN_FACTORY2)
            || tuid_eq(iid, &IID_IPLUGIN_FACTORY3)
        {
            Self::from_this(this).ref_count.fetch_add(1, Ordering::Relaxed);
            *obj = this;
            kResultOk
        } else {
            *obj = null_mut();
            kNoInterface
        }
    }

    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        Self::from_this(this).ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        let me = Self::from_this(this);
        let prev = me.ref_count.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            drop(Box::from_raw(this as *mut Self));
            0
        } else {
            prev - 1
        }
    }

    unsafe extern "system" fn get_factory_info(
        _this: *mut c_void,
        info: *mut PFactoryInfo,
    ) -> tresult {
        if info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        write_char8(&mut info.vendor, D::INFO.vendor);
        write_char8(&mut info.url, D::INFO.url);
        write_char8(&mut info.email, D::INFO.email);
        info.flags = kFactoryUnicode;
        kResultOk
    }

    unsafe extern "system" fn count_classes(_this: *mut c_void) -> i32 {
        1
    }

    unsafe extern "system" fn get_class_info(
        _this: *mut c_void,
        index: i32,
        info: *mut PClassInfo,
    ) -> tresult {
        if info.is_null() || index != 0 {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = D::INFO.class_id;
        info.cardinality = kManyInstances;
        write_char8(&mut info.category, "Audio Module Class");
        write_char8(&mut info.name, D::INFO.name);
        kResultOk
    }

    unsafe extern "system" fn create_instance(
        _this: *mut c_void,
        cid: *const TUID,
        iid: *const TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        *obj = null_mut();
        if !tuid_eq(cid, &D::INFO.class_id) {
            return kInvalidArgument;
        }
        let instance = PluginInstance::<D>::create_raw();
        // Hand out the requested interface, then drop our creation ref.
        let vtbl = &**(instance as *mut *const IComponentVtbl);
        let result = (vtbl.query_interface)(instance.cast(), iid, obj);
        (vtbl.release)(instance.cast());
        if result == kResultOk {
            kResultOk
        } else {
            kNoInterface
        }
    }

    unsafe extern "system" fn get_class_info2(
        _this: *mut c_void,
        index: i32,
        info: *mut PClassInfo2,
    ) -> tresult {
        if info.is_null() || index != 0 {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = D::INFO.class_id;
        info.cardinality = kManyInstances;
        write_char8(&mut info.category, "Audio Module Class");
        write_char8(&mut info.name, D::INFO.name);
        info.class_flags = 0;
        write_char8(&mut info.subcategories, D::INFO.subcategories);
        write_char8(&mut info.vendor, D::INFO.vendor);
        write_char8(&mut info.version, D::INFO.version);
        write_char8(&mut info.sdk_version, "VST 3.7.4");
        kResultOk
    }

    unsafe extern "system" fn get_class_info_unicode(
        _this: *mut c_void,
        index: i32,
        info: *mut PClassInfoW,
    ) -> tresult {
        if info.is_null() || index != 0 {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = D::INFO.class_id;
        info.cardinality = kManyInstances;
        write_char8(&mut info.category, "Audio Module Class");
        write_char16(&mut info.name, D::INFO.name);
        info.class_flags = 0;
        write_char8(&mut info.subcategories, D::INFO.subcategories);
        write_char16(&mut info.vendor, D::INFO.vendor);
        write_char16(&mut info.version, D::INFO.version);
        write_char16(&mut info.sdk_version, "VST 3.7.4");
        kResultOk
    }

    unsafe extern "system" fn set_host_context(
        _this: *mut c_void,
        _context: *mut c_void,
    ) -> tresult {
        kResultOk
    }

    const VTBL: &'static IPluginFactory3Vtbl = &IPluginFactory3Vtbl {
        query_interface: Self::query_interface,
        add_ref: Self::add_ref,
        release: Self::release,
        get_factory_info: Self::get_factory_info,
        count_classes: Self::count_classes,
        get_class_info: Self::get_class_info,
        create_instance: Self::create_instance,
        get_class_info2: Self::get_class_info2,
        get_class_info_unicode: Self::get_class_info_unicode,
        set_host_context: Self::set_host_context,
    };
}
