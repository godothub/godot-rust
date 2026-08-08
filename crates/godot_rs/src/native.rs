//! Independent Native GDExtension entry and selected official Raw ABI.

use core::cell::{Cell, RefCell};
use core::ffi::c_void;
use core::marker::PhantomData;
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

mod callable_value;
mod class;
mod dynamic_value;
mod engine_call;
mod method;
mod packed_array;
pub(crate) mod runtime;
mod signal_value;
pub(crate) mod value;

pub use crate::script::{RpcConfig, RpcMode, RpcTransferMode};
pub use class::{
    Base, ClassRegistrar, GodotClass, NOTIFICATION_EXTENSION_RELOADED, NativeClass, NativeProperty,
    NativePropertyOptions, NativePropertyValidation, NativeVirtualRegistrar, classes,
};
pub use method::{MethodRegistration, NativeMethod, NativeVirtualContract, NativeVirtualMethod};
pub use value::GodotValue;

pub(crate) use callable_value::{NativeCallableToken, retain_rust_callable};
pub(crate) use godot_api::api_snapshot;
pub use godot_api::native as sys;

#[doc(hidden)]
pub use engine_call::NativeEngineValue;
pub(crate) use engine_call::{NativeGodotRefToken, invoke_engine_method, invoke_godot_api};

/// Godot Major/Minor selected by the plugin for this Native build.
pub const GODOT_API: &str = godot_api::NATIVE_GODOT_API;
/// Entry symbol written into generated `.gdextension` descriptors.
pub const ENTRY_SYMBOL: &str = godot_api::NATIVE_ENTRY_SYMBOL;

const GDEXTENSION_FALSE: sys::GDExtensionBool = 0;
const GDEXTENSION_TRUE: sys::GDExtensionBool = 1;
static LIBRARY_HEALTHY: AtomicBool = AtomicBool::new(true);

struct ExtensionRuntimeHealth {
    _base: Base<crate::engine::RefCounted>,
}

impl NativeClass for ExtensionRuntimeHealth {
    type Base = crate::engine::RefCounted;

    const CLASS_NAME: &'static str = "GodotRsExtensionRuntime";

    fn init(base: Base<Self::Base>) -> Self {
        Self { _base: base }
    }

    fn register_methods(registrar: &mut ClassRegistrar<'_, Self>) -> NativeResult {
        registrar.method("is_healthy", Self::is_healthy)?;
        Ok(())
    }
}

impl ExtensionRuntimeHealth {
    fn is_healthy(&self) -> bool {
        LIBRARY_HEALTHY.load(Ordering::Acquire)
    }
}

/// Stable initialization levels presented to user Native extensions.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InitializationLevel {
    Core,
    Servers,
    Scene,
    Editor,
}

impl InitializationLevel {
    fn to_raw(self) -> sys::GDExtensionInitializationLevel {
        match self {
            Self::Core => sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_CORE,
            Self::Servers => {
                sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SERVERS
            }
            Self::Scene => sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SCENE,
            Self::Editor => sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_EDITOR,
        }
    }

    fn from_raw(level: sys::GDExtensionInitializationLevel) -> Option<Self> {
        match level {
            sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_CORE => {
                Some(Self::Core)
            }
            sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SERVERS => {
                Some(Self::Servers)
            }
            sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SCENE => {
                Some(Self::Scene)
            }
            sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_EDITOR => {
                Some(Self::Editor)
            }
            _ => None,
        }
    }
}

/// Engine handles that remain valid for one Native extension library lifetime.
pub struct InitializationContext {
    get_proc_address: sys::GDExtensionInterfaceGetProcAddress,
    interface: runtime::Interface,
    active_level: Cell<Option<InitializationLevel>>,
    registrations: RefCell<Vec<class::RegisteredClass>>,
    poisoned: AtomicBool,
}

impl InitializationContext {
    /// Returns the Godot function lookup callback for advanced self-owned bindings.
    #[must_use]
    pub const fn get_proc_address(&self) -> sys::GDExtensionInterfaceGetProcAddress {
        self.get_proc_address
    }

    /// Returns the opaque Godot library pointer.
    #[must_use]
    pub const fn library(&self) -> sys::GDExtensionClassLibraryPtr {
        self.interface.library
    }

    /// Returns whether a prior callback failed or panicked.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
}

/// Actionable failure returned from a Native initialization callback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeError {
    message: String,
}

impl NativeError {
    /// Creates an error whose message can be shown in Godot output.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for NativeError {}

/// Result returned by Native extension initialization callbacks.
pub type NativeResult = Result<(), NativeError>;

/// User-owned Native GDExtension lifecycle.
///
/// Implementations are registered with [`crate::gdextension!`]. All callbacks
/// are caught at the C ABI boundary; one error poisons later user callbacks
/// while still allowing Godot to release the internal context.
pub trait ExtensionLibrary: Sized + 'static {
    /// Earliest Godot initialization level needed by the extension.
    const MINIMUM_LEVEL: InitializationLevel = InitializationLevel::Scene;

    /// Called as Godot initializes each level at or above `MINIMUM_LEVEL`.
    fn on_level_initialize(
        _context: &InitializationContext,
        _level: InitializationLevel,
    ) -> NativeResult {
        Ok(())
    }

    /// Called in reverse order while Godot shuts levels down.
    fn on_level_deinitialize(
        _context: &InitializationContext,
        _level: InitializationLevel,
    ) -> NativeResult {
        Ok(())
    }
}

struct LibraryContext<L> {
    engine: InitializationContext,
    marker: PhantomData<fn() -> L>,
}

unsafe extern "C" fn initialize_level<L: ExtensionLibrary>(
    userdata: *mut c_void,
    raw_level: sys::GDExtensionInitializationLevel,
) {
    // SAFETY: `entry` allocates this exact context and Godot returns the same
    // userdata through the final Core deinitialization callback.
    let Some(context) = (unsafe { (userdata as *mut LibraryContext<L>).as_ref() }) else {
        return;
    };
    if context.engine.is_poisoned() {
        return;
    }
    let Some(level) = InitializationLevel::from_raw(raw_level) else {
        context.engine.poisoned.store(true, Ordering::Release);
        eprintln!("godot-rust Native received an invalid initialization level");
        return;
    };
    if level < L::MINIMUM_LEVEL {
        return;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _active_interface = runtime::activate_interface(context.engine.interface);
        context.engine.active_level.set(Some(level));
        if level == L::MINIMUM_LEVEL {
            context.engine.register_class::<ExtensionRuntimeHealth>()?;
        }
        L::on_level_initialize(&context.engine, level)
    }));
    context.engine.active_level.set(None);
    handle_callback_result(&context.engine, "initialize", level, result);
}

unsafe extern "C" fn deinitialize_level<L: ExtensionLibrary>(
    userdata: *mut c_void,
    raw_level: sys::GDExtensionInitializationLevel,
) {
    let context_pointer = userdata as *mut LibraryContext<L>;
    // SAFETY: See `initialize_level`; the allocation remains alive until the
    // final Core callback has completed.
    let Some(context) = (unsafe { context_pointer.as_ref() }) else {
        return;
    };
    let Some(level) = InitializationLevel::from_raw(raw_level) else {
        context.engine.poisoned.store(true, Ordering::Release);
        eprintln!("godot-rust Native received an invalid deinitialization level");
        return;
    };
    if level >= L::MINIMUM_LEVEL && !context.engine.is_poisoned() {
        context.engine.active_level.set(Some(level));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _active_interface = runtime::activate_interface(context.engine.interface);
            L::on_level_deinitialize(&context.engine, level)
        }));
        context.engine.active_level.set(None);
        handle_callback_result(&context.engine, "deinitialize", level, result);
    }
    context.engine.unregister_level(level);
    if level == L::MINIMUM_LEVEL {
        callable_value::clear_thread_local_state();
    }
    if level == InitializationLevel::Core {
        // SAFETY: Godot deinitializes every library through Core, regardless
        // of its advertised minimum level, and does not reuse userdata after
        // this final callback.
        drop(unsafe { Box::from_raw(context_pointer) });
    }
}

fn handle_callback_result(
    context: &InitializationContext,
    operation: &str,
    level: InitializationLevel,
    result: Result<NativeResult, Box<dyn core::any::Any + Send>>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            LIBRARY_HEALTHY.store(false, Ordering::Release);
            context.poisoned.store(true, Ordering::Release);
            eprintln!("godot-rust Native {operation} {level:?} failed: {error}");
        }
        Err(_) => {
            LIBRARY_HEALTHY.store(false, Ordering::Release);
            context.poisoned.store(true, Ordering::Release);
            eprintln!("godot-rust Native {operation} {level:?} panicked");
        }
    }
}

/// Initializes one generated Native GDExtension entry.
///
/// # Safety
///
/// Godot must provide callbacks and pointers matching the selected official
/// GDExtension ABI, and must honor the initialization descriptor lifetime.
#[doc(hidden)]
pub unsafe fn entry<L: ExtensionLibrary>(
    get_proc_address: sys::GDExtensionInterfaceGetProcAddress,
    library: sys::GDExtensionClassLibraryPtr,
    initialization: *mut sys::GDExtensionInitialization,
) -> sys::GDExtensionBool {
    match catch_unwind(AssertUnwindSafe(|| {
        LIBRARY_HEALTHY.store(true, Ordering::Release);
        if get_proc_address.is_none() || library.is_null() || initialization.is_null() {
            return false;
        }
        // SAFETY: Entry arguments were validated and the loader only casts
        // official function names to types generated from the selected API.
        let interface = match unsafe { runtime::Interface::load(get_proc_address, library) } {
            Ok(interface) => interface,
            Err(error) => {
                eprintln!("godot-rust Native initialization failed: {error}");
                return false;
            }
        };
        let context = Box::new(LibraryContext::<L> {
            engine: InitializationContext {
                get_proc_address,
                interface,
                active_level: Cell::new(None),
                registrations: RefCell::new(Vec::new()),
                poisoned: AtomicBool::new(false),
            },
            marker: PhantomData,
        });
        let userdata = Box::into_raw(context).cast::<c_void>();
        // SAFETY: Null was rejected above and Godot gives exclusive access to
        // this descriptor for the duration of the entry call.
        let initialization = unsafe { &mut *initialization };
        initialization.minimum_initialization_level = L::MINIMUM_LEVEL.to_raw();
        initialization.userdata = userdata;
        initialization.initialize = Some(initialize_level::<L>);
        initialization.deinitialize = Some(deinitialize_level::<L>);
        true
    })) {
        Ok(true) => GDEXTENSION_TRUE,
        Ok(false) | Err(_) => GDEXTENSION_FALSE,
    }
}

/// Generates the single entry symbol used by a Extension Mode library.
#[macro_export]
macro_rules! gdextension {
    ($library:ty) => {
        /// Entry point generated by `godot_rs::gdextension!`.
        ///
        /// # Safety
        ///
        /// This function may only be invoked by Godot using its selected
        /// official GDExtension ABI.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn godot_rs_native_init(
            get_proc_address: $crate::native::sys::GDExtensionInterfaceGetProcAddress,
            library: $crate::native::sys::GDExtensionClassLibraryPtr,
            initialization: *mut $crate::native::sys::GDExtensionInitialization,
        ) -> $crate::native::sys::GDExtensionBool {
            // SAFETY: The generated entry has exactly the preconditions of the
            // internal ABI boundary and forwards Godot's arguments unchanged.
            unsafe { $crate::native::entry::<$library>(get_proc_address, library, initialization) }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr;

    struct TestExtension;

    impl ExtensionLibrary for TestExtension {}

    unsafe extern "C" fn fake_get_proc_address(
        _name: *const core::ffi::c_char,
    ) -> sys::GDExtensionInterfaceFunctionPtr {
        None
    }

    fn empty_initialization() -> sys::GDExtensionInitialization {
        sys::GDExtensionInitialization {
            minimum_initialization_level:
                sys::GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_CORE,
            userdata: ptr::null_mut(),
            initialize: None,
            deinitialize: None,
        }
    }

    #[test]
    fn sdk_and_raw_bindings_use_the_same_selected_api() {
        let expected_hash = match GODOT_API {
            "4.4" => "355ff4c6254fdd434ea16d9a8ef0f18e3f95aeb3e3f00d98db10769ece3c7fe5",
            "4.5" => "a40ac4fca0f526910bd0e6afc6da6c169f50801c84d4e29c4ce2891cadc7b550",
            "4.6" => "6228db9a0be9bb154a89911a69ee7ec767f2d40004b3ed5b1d08eebf90bcd16b",
            "4.7" => "640b48188708ba0016f8d7ace9e0e1d3279a41fa1226c59ff3193b15538bd254",
            other => panic!("unexpected selected Godot API {other}"),
        };
        assert_eq!(sys::GODOT_GDEXTENSION_INTERFACE_SHA256, expected_hash);
    }

    #[test]
    fn native_entry_rejects_an_incomplete_official_interface() {
        let mut initialization = empty_initialization();
        let library = ptr::dangling_mut::<c_void>();
        // SAFETY: The fake lookup callback intentionally returns no required
        // functions so entry must reject it without installing callbacks.
        let accepted = unsafe {
            entry::<TestExtension>(Some(fake_get_proc_address), library, &mut initialization)
        };
        assert_eq!(accepted, GDEXTENSION_FALSE);
        assert!(initialization.initialize.is_none());
        assert!(initialization.userdata.is_null());
    }

    #[test]
    fn invalid_entry_arguments_are_rejected() {
        let mut initialization = empty_initialization();
        // SAFETY: Rejected null values are intentionally supplied.
        let accepted =
            unsafe { entry::<TestExtension>(None, ptr::null_mut(), &mut initialization) };
        assert_eq!(accepted, GDEXTENSION_FALSE);
        assert!(initialization.initialize.is_none());
    }

    #[test]
    fn native_lifecycle_filters_user_levels_but_releases_at_core() {
        let minimum = InitializationLevel::Scene;
        let levels = [
            InitializationLevel::Core,
            InitializationLevel::Servers,
            InitializationLevel::Scene,
            InitializationLevel::Editor,
        ];
        let user_levels: Vec<_> = levels
            .into_iter()
            .filter(|level| *level >= minimum)
            .collect();

        assert_eq!(
            user_levels,
            [InitializationLevel::Scene, InitializationLevel::Editor]
        );
        assert_ne!(minimum, InitializationLevel::Core);
        let deinitialization: Vec<_> = levels.into_iter().rev().collect();
        assert_eq!(
            deinitialization.last().copied(),
            Some(InitializationLevel::Core)
        );
    }
}
