use core::fmt;
use godot_api::abi::AbiLogLevel;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

use crate::interface::EngineInterface;
use crate::string_name::StaticStringName;
use crate::variant_codec::{OwnedVariant, VariantCodec};

struct EngineLog {
    codec: VariantCodec,
    functions: [godot_api::GDExtensionPtrUtilityFunction; 4],
}

thread_local! {
    static ENGINE_LOG: RefCell<Option<Rc<EngineLog>>> = const { RefCell::new(None) };
}

// Workers enqueue text only; all Godot values are created and released on the main thread.
static PENDING: Mutex<Option<Vec<(AbiLogLevel, String)>>> = Mutex::new(None);

pub(crate) fn initialize(interface: EngineInterface) -> Result<(), String> {
    let codec =
        VariantCodec::new(interface).ok_or("Godot log Variant conversion is unavailable")?;
    let resolve = interface
        .variant_get_ptr_utility_function
        .ok_or("Godot log utilities are unavailable")?;
    let mut functions = [None; 4];
    for (slot, name) in
        functions
            .iter_mut()
            .zip([c"print_verbose", c"print", c"push_warning", c"push_error"])
    {
        let name = StaticStringName::new(interface, name);
        // SAFETY: These four vararg utilities share the official print(Variant, ...) signature.
        *slot = unsafe { resolve(name.as_ptr(), 2_648_703_342) };
        if slot.is_none() {
            return Err("Godot log utility could not be resolved".into());
        }
    }
    *PENDING.lock().unwrap_or_else(|error| error.into_inner()) = Some(Vec::new());
    ENGINE_LOG.with(|slot| *slot.borrow_mut() = Some(Rc::new(EngineLog { codec, functions })));
    Ok(())
}

pub(crate) fn flush() {
    let pending = PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
        .map(std::mem::take)
        .unwrap_or_default();
    for (level, message) in pending {
        module(level, format_args!("{message}"));
    }
}

pub(crate) fn shutdown() {
    let pending = PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
        .unwrap_or_default();
    for (level, message) in pending {
        module(level, format_args!("{message}"));
    }
    ENGINE_LOG.with(|slot| slot.borrow_mut().take());
}

impl EngineLog {
    fn write(&self, level: AbiLogLevel, message: &str) -> bool {
        let Ok(value) = OwnedVariant::from_abi(
            &self.codec,
            godot_api::abi::AbiValueV1::from_borrowed_utf8(message),
        ) else {
            return false;
        };
        let index = match level {
            AbiLogLevel::Debug => 0,
            AbiLogLevel::Info => 1,
            AbiLogLevel::Warning => 2,
            AbiLogLevel::Error => 3,
        };
        let Some(function) = self.functions[index] else {
            return false;
        };
        let arguments = [value.as_ptr()];
        // SAFETY: The utility takes one live Variant argument and returns void.
        unsafe { function(std::ptr::null_mut(), arguments.as_ptr(), 1) };
        true
    }
}

#[cfg(target_os = "android")]
use std::ffi::CString;

#[cfg(target_os = "android")]
const ANDROID_LOG_DEBUG: core::ffi::c_int = 3;
#[cfg(target_os = "android")]
const ANDROID_LOG_INFO: core::ffi::c_int = 4;
#[cfg(target_os = "android")]
const ANDROID_LOG_WARN: core::ffi::c_int = 5;
#[cfg(target_os = "android")]
const ANDROID_LOG_ERROR: core::ffi::c_int = 6;

#[cfg(target_os = "android")]
#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(
        priority: core::ffi::c_int,
        tag: *const core::ffi::c_char,
        text: *const core::ffi::c_char,
    ) -> core::ffi::c_int;
}

pub(crate) fn error(arguments: fmt::Arguments<'_>) {
    module(AbiLogLevel::Error, arguments);
}

pub(crate) fn module(level: AbiLogLevel, arguments: fmt::Arguments<'_>) {
    let message = arguments.to_string();
    let engine = ENGINE_LOG.with(|slot| slot.borrow().clone());
    if let Some(engine) = engine {
        if engine.write(level, &message) {
            return;
        }
    } else if let Some(pending) = PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_mut()
    {
        pending.push((level, message));
        return;
    }
    #[cfg(target_os = "android")]
    {
        let priority = match level {
            AbiLogLevel::Debug => ANDROID_LOG_DEBUG,
            AbiLogLevel::Info => ANDROID_LOG_INFO,
            AbiLogLevel::Warning => ANDROID_LOG_WARN,
            AbiLogLevel::Error => ANDROID_LOG_ERROR,
        };
        android_write(priority, format_args!("{message}"));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = level;
        eprintln!("{message}");
    }
}

#[cfg(target_os = "android")]
fn android_write(priority: core::ffi::c_int, arguments: fmt::Arguments<'_>) {
    const TAG: &[u8] = b"godot-rust\0";
    let message = arguments.to_string().replace('\0', "\\0");
    let Ok(message) = CString::new(message) else {
        return;
    };
    // SAFETY: The Android NDK logging function copies both live,
    // nul-terminated strings before returning.
    unsafe {
        __android_log_write(priority, TAG.as_ptr().cast(), message.as_ptr());
    }
}
