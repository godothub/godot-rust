use core::ffi::c_void;
use core::ptr;
use godot_rs_api::{
    GDExtensionBool, GDExtensionClassLibraryPtr, GDExtensionInitialization,
    GDExtensionInitializationLevel, GDExtensionInterfaceGetProcAddress,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock};

use crate::interface::EngineInterface;
use crate::runtime::HostRuntime;

const GDEXTENSION_FALSE: GDExtensionBool = 0;
const GDEXTENSION_TRUE: GDExtensionBool = 1;

static ENGINE_INTERFACE: OnceLock<EngineInterface> = OnceLock::new();
static HOST_RUNTIME: Mutex<Option<HostRuntime>> = Mutex::new(None);

unsafe extern "C" fn initialize(_userdata: *mut c_void, level: GDExtensionInitializationLevel) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if level == GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SCENE {
            let Some(interface) = ENGINE_INTERFACE.get().copied() else {
                return;
            };
            let Ok(mut runtime) = HOST_RUNTIME.lock() else {
                return;
            };
            if runtime.is_none() {
                match HostRuntime::start(interface) {
                    Ok(started) => *runtime = Some(started),
                    Err(error) => host_eprintln!("godot-rust: {error}"),
                }
            }
        }
    }));
}

unsafe extern "C" fn deinitialize(_userdata: *mut c_void, level: GDExtensionInitializationLevel) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if level == GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SCENE {
            if let Ok(mut runtime) = HOST_RUNTIME.lock() {
                if let Some(mut running) = runtime.take() {
                    running.shutdown();
                }
            }
        }
    }));
}

unsafe fn initialize_entry(
    get_proc_address: GDExtensionInterfaceGetProcAddress,
    library: GDExtensionClassLibraryPtr,
    initialization: *mut GDExtensionInitialization,
) -> bool {
    if initialization.is_null() {
        return false;
    }

    // SAFETY: Inputs are supplied by Godot for the duration of the entry call.
    let Ok(engine) = (unsafe { EngineInterface::load(get_proc_address, library) }) else {
        return false;
    };

    if ENGINE_INTERFACE.set(engine).is_err() {
        return false;
    }

    // Touch bootstrap values here so a future registration layer cannot
    // accidentally replace the validated interface with ad-hoc globals.
    let _ = (
        engine.get_proc_address,
        engine.library(),
        engine.version(),
        engine.resolved_function_count(),
    );

    // SAFETY: Null was rejected above and Godot requires this entry point to
    // populate the pointed initialization descriptor before returning true.
    let initialization = unsafe { &mut *initialization };
    initialization.minimum_initialization_level =
        GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SCENE;
    initialization.userdata = ptr::null_mut();
    initialization.initialize = Some(initialize);
    initialization.deinitialize = Some(deinitialize);
    true
}

/// GDExtension entry symbol referenced by the plugin's `.gdextension` file.
///
/// # Safety
///
/// Godot must pass pointers and callbacks matching its official 4.4
/// GDExtension ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn godot_rust_init(
    get_proc_address: GDExtensionInterfaceGetProcAddress,
    library: GDExtensionClassLibraryPtr,
    initialization: *mut GDExtensionInitialization,
) -> GDExtensionBool {
    match catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: This exported function has the same preconditions as
        // `initialize_entry`, and contains the ABI panic boundary.
        unsafe { initialize_entry(get_proc_address, library, initialization) }
    })) {
        Ok(true) => GDEXTENSION_TRUE,
        Ok(false) | Err(_) => GDEXTENSION_FALSE,
    }
}
