use core::fmt;
use godot_api::abi::AbiLogLevel;

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
    #[cfg(target_os = "android")]
    android_write(ANDROID_LOG_ERROR, arguments);
    #[cfg(not(target_os = "android"))]
    eprintln!("{arguments}");
}

pub(crate) fn module(level: AbiLogLevel, arguments: fmt::Arguments<'_>) {
    #[cfg(target_os = "android")]
    {
        let priority = match level {
            AbiLogLevel::Debug => ANDROID_LOG_DEBUG,
            AbiLogLevel::Info => ANDROID_LOG_INFO,
            AbiLogLevel::Warning => ANDROID_LOG_WARN,
            AbiLogLevel::Error => ANDROID_LOG_ERROR,
        };
        android_write(priority, arguments);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = level;
        eprintln!("{arguments}");
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
