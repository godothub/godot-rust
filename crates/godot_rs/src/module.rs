use core::ffi::c_void;
use core::fmt;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::sync::RwLock;

use godot_api::abi::{
    ABI_MODULE_EXTENSION_GODOT_API, ABI_MODULE_EXTENSION_OWNED_VALUES, ABI_MODULE_EXTENSION_TASKS,
    ABI_VALUE_OWNED_BYTES, ABI_VALUE_OWNED_CALLABLE, ABI_VALUE_OWNED_DYNAMIC_GROUP,
    ABI_VALUE_OWNED_OBJECT_REF, ABI_VALUE_OWNED_UTF8, AbiByteSlice, AbiCallGodotApiFn,
    AbiCallGodotMethodFn, AbiCallResult, AbiCallSuperMethodFn, AbiCancelSignalFn,
    AbiCurrentOwnerFn, AbiDropHostValueFn, AbiEmitSignalFn, AbiGetScriptDescriptorFn,
    AbiGodotApiSpecV1, AbiGodotMethodSpecV1, AbiHeader, AbiLogLevel, AbiModuleShutdownFn,
    AbiPollSignalFn, AbiRetainCallableValueFn, AbiRetainDynamicValueFn, AbiStatus, AbiValueType,
    AbiValueV1, AbiWatchSignalFn, HOST_API_SLOT_CALL_GODOT_API, HOST_API_SLOT_CALL_GODOT_METHOD,
    HOST_API_SLOT_CALL_SUPER_METHOD, HOST_API_SLOT_CANCEL_SIGNAL, HOST_API_SLOT_CURRENT_OWNER,
    HOST_API_SLOT_DROP_VALUE, HOST_API_SLOT_EMIT_SIGNAL, HOST_API_SLOT_POLL_SIGNAL,
    HOST_API_SLOT_RETAIN_CALLABLE_VALUE, HOST_API_SLOT_RETAIN_DYNAMIC_VALUE,
    HOST_API_SLOT_WATCH_SIGNAL, HostApiV1, MODULE_API_SLOT_CANCEL_TASKS,
    MODULE_API_SLOT_DROP_VALUE, MODULE_API_SLOT_GODOT_API_MAJOR, MODULE_API_SLOT_GODOT_API_MINOR,
    MODULE_API_SLOT_POLL_TASKS, ModuleApiV1,
};

use crate::error::{EngineError, EngineResult};
use crate::log::Level;

static HOST_API: AtomicPtr<HostApiV1> = AtomicPtr::new(ptr::null_mut());
// Logging is available to blocking workers. Shutdown must wait for their
// current Host log callbacks before the Host may unload its own library.
static HOST_LOG_ACCESS: RwLock<()> = RwLock::new(());

type ReturnRegistry = core::cell::RefCell<ModuleReturns>;

thread_local! {
    // A trivial TLS slot avoids registering a TLS destructor for this registry.
    // Module shutdown releases it explicitly before revoking Host access.
    static MODULE_RETURNS: core::cell::Cell<*mut ReturnRegistry> = const { core::cell::Cell::new(ptr::null_mut()) };
    static RELEASING_RETURN_BACKINGS: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

struct ModuleReturns {
    next_token: u64,
    objects: std::collections::HashMap<u64, (u64, crate::engine::SharedGodotRefOwnership)>,
    bytes: std::collections::HashMap<usize, ModuleReturnValue>,
}

fn with_return_registry<R>(operation: impl FnOnce(&ReturnRegistry) -> R) -> R {
    MODULE_RETURNS.with(|slot| {
        let pointer = if slot.get().is_null() {
            let pointer = Box::into_raw(Box::new(ReturnRegistry::new(ModuleReturns {
                next_token: 1,
                objects: std::collections::HashMap::new(),
                bytes: std::collections::HashMap::new(),
            })));
            slot.set(pointer);
            pointer
        } else {
            slot.get()
        };
        // SAFETY: Only this thread accesses its registry. Shutdown drops it after
        // all module callbacks and resource destructors have finished.
        operation(unsafe { &*pointer })
    })
}

struct ModuleReturnValue {
    value_type: AbiValueType,
    bytes: Box<[u8]>,
    _value: crate::variant::Variant,
}

pub(crate) fn owned_dynamic_value(
    value_type: AbiValueType,
    value: crate::variant::Variant,
) -> Result<AbiValueV1, AbiCallResult> {
    let bytes = value
        .__bytes()
        .map_err(|error| AbiCallResult::callback_failed(error.message()))?
        .to_vec()
        .into_boxed_slice();
    owned_retained_bytes(value_type, bytes, value)
}

pub(crate) fn owned_callable_value(
    value: crate::callable::Callable,
) -> Result<AbiValueV1, AbiCallResult> {
    let bytes = value
        .__bytes()
        .map_err(|error| AbiCallResult::callback_failed(error.message()))?
        .to_vec()
        .into_boxed_slice();
    owned_retained_bytes(
        AbiValueType::CALLABLE,
        bytes,
        crate::variant::Variant::from(value),
    )
}

fn owned_retained_bytes(
    value_type: AbiValueType,
    bytes: Box<[u8]>,
    value: crate::variant::Variant,
) -> Result<AbiValueV1, AbiCallResult> {
    if RELEASING_RETURN_BACKINGS.get() {
        return Err(AbiCallResult::failure(
            AbiStatus::Unsupported,
            "module return values are being released",
        ));
    }
    with_return_registry(|registry| {
        let mut registry = registry.try_borrow_mut().map_err(|_| {
            AbiCallResult::failure(
                AbiStatus::ReentrantCall,
                "module return registry is already borrowed",
            )
        })?;
        if registry.bytes.len() >= 65_536 {
            return Err(AbiCallResult::failure(
                AbiStatus::Unsupported,
                "too many unreleased module returns",
            ));
        }
        let address = bytes.as_ptr() as usize;
        let output = AbiValueV1 {
            type_: value_type,
            reserved_flags: ABI_VALUE_OWNED_BYTES,
            payload: [address as u64, bytes.len() as u64],
        };
        registry.bytes.insert(
            address,
            ModuleReturnValue {
                value_type,
                bytes,
                _value: value,
            },
        );
        Ok(output)
    })
}

fn release_return_backings() {
    struct ReleaseScope;
    impl Drop for ReleaseScope {
        fn drop(&mut self) {
            RELEASING_RETURN_BACKINGS.set(false);
        }
    }
    RELEASING_RETURN_BACKINGS.set(true);
    let _scope = ReleaseScope;
    MODULE_RETURNS.with(|slot| {
        let pointer = slot.get();
        if pointer.is_null() {
            return;
        }
        // SAFETY: This thread still owns the registry until the final Box drop.
        let registry = unsafe { &*pointer };
        let (objects, bytes) = {
            let mut registry = registry.borrow_mut();
            (
                core::mem::take(&mut registry.objects),
                core::mem::take(&mut registry.bytes),
            )
        };
        // Resource destruction can reenter the module; leave the empty registry
        // available while rejecting any new return ownership during shutdown.
        drop(objects);
        drop(bytes);
        slot.set(ptr::null_mut());
        // SAFETY: The cleared slot transfers its unique allocation to shutdown.
        drop(unsafe { Box::from_raw(pointer) });
    });
}

pub(crate) fn owned_object_ref<T: crate::engine::GodotClass>(
    value: crate::engine::GodotRef<T>,
) -> Result<AbiValueV1, AbiCallResult> {
    const MAX_MODULE_OBJECT_REFS: usize = 65_536;
    if RELEASING_RETURN_BACKINGS.get() {
        return Err(AbiCallResult::failure(
            AbiStatus::Unsupported,
            "module return values are being released",
        ));
    }
    with_return_registry(|registry| {
        let mut registry = registry.try_borrow_mut().map_err(|_| {
            AbiCallResult::failure(
                AbiStatus::ReentrantCall,
                "module object return registry is already borrowed",
            )
        })?;
        if registry.objects.len() >= MAX_MODULE_OBJECT_REFS {
            return Err(AbiCallResult::failure(
                AbiStatus::Unsupported,
                "too many unreleased module object returns",
            ));
        }
        let token = registry.next_token;
        registry.next_token = token.checked_add(1).ok_or_else(|| {
            AbiCallResult::failure(
                AbiStatus::Unsupported,
                "module object return token space is exhausted",
            )
        })?;
        let object_id = value.instance_id();
        registry
            .objects
            .insert(token, (object_id, value.dynamic_ownership()));
        Ok(AbiValueV1 {
            type_: AbiValueType::OBJECT_ID,
            reserved_flags: godot_api::abi::ABI_VALUE_MODULE_OBJECT_REF,
            payload: [object_id, token],
        })
    })
}

pub(crate) fn clone_module_object_ref<T: crate::engine::GodotClass>(
    value: AbiValueV1,
) -> Option<crate::engine::GodotRef<T>> {
    if MODULE_RETURNS.get().is_null()
        || value.type_ != AbiValueType::OBJECT_ID
        || value.reserved_flags != godot_api::abi::ABI_VALUE_MODULE_OBJECT_REF
    {
        return None;
    }
    with_return_registry(|registry| {
        let registry = registry.try_borrow().ok()?;
        let (object_id, ownership) = registry.objects.get(&value.payload[1])?;
        (*object_id == value.payload[0])
            .then(|| crate::engine::GodotRef::from_dynamic_parts(*object_id, ownership.clone()))
    })
}

fn drop_module_object_ref(value: AbiValueV1) -> AbiStatus {
    if value.type_ != AbiValueType::OBJECT_ID || value.payload[0] == 0 || value.payload[1] == 0 {
        return AbiStatus::InvalidArgument;
    }
    if MODULE_RETURNS.get().is_null() {
        return AbiStatus::StaleHandle;
    }
    let removed = with_return_registry(|registry| {
        let Ok(mut registry) = registry.try_borrow_mut() else {
            return Err(AbiStatus::ReentrantCall);
        };
        let Some((object_id, _)) = registry.objects.get(&value.payload[1]) else {
            return Err(AbiStatus::StaleHandle);
        };
        if *object_id != value.payload[0] {
            return Err(AbiStatus::InvalidArgument);
        }
        Ok(registry.objects.remove(&value.payload[1]))
    });
    match removed {
        Ok(owned) => {
            // Releasing the final reference may enter Godot; keep the registry
            // borrow out of that call so another return can be released safely.
            drop(owned);
            AbiStatus::Ok
        }
        Err(status) => status,
    }
}

/// Initializes the generated project-module table.
///
/// # Safety
///
/// `host` and `output` must point to live ABI tables owned by the Host.
#[doc(hidden)]
pub unsafe fn initialize(
    host: *const HostApiV1,
    output: *mut ModuleApiV1,
    script_count: u32,
    get_script: AbiGetScriptDescriptorFn,
    shutdown: AbiModuleShutdownFn,
) -> AbiStatus {
    if host.is_null() || output.is_null() || get_script.is_none() {
        return AbiStatus::InvalidArgument;
    }
    // SAFETY: Null was rejected and the caller owns a live Host table.
    let host_api = unsafe { &*host };
    if !host_api.header.is_compatible(HostApiV1::MINIMUM_SIZE, 0) {
        return AbiStatus::Unsupported;
    }
    match HOST_API.compare_exchange(
        ptr::null_mut(),
        host.cast_mut(),
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => {}
        Err(current) if ptr::eq(current, host.cast_mut()) => {}
        Err(_) => return AbiStatus::Internal,
    }

    let mut reserved = [0; 13];
    reserved[MODULE_API_SLOT_DROP_VALUE] = drop_owned_value as *const () as usize;
    reserved[MODULE_API_SLOT_GODOT_API_MAJOR] = godot_api::SELECTED_GODOT_API_MAJOR as usize;
    reserved[MODULE_API_SLOT_GODOT_API_MINOR] = godot_api::SELECTED_GODOT_API_MINOR as usize;
    reserved[MODULE_API_SLOT_POLL_TASKS] = poll_tasks as *const () as usize;
    reserved[MODULE_API_SLOT_CANCEL_TASKS] = cancel_tasks as *const () as usize;
    let module = ModuleApiV1 {
        header: AbiHeader::new(ModuleApiV1::MINIMUM_SIZE),
        context: ptr::null_mut(),
        shutdown,
        script_count,
        reserved_flags: ABI_MODULE_EXTENSION_OWNED_VALUES
            | ABI_MODULE_EXTENSION_GODOT_API
            | ABI_MODULE_EXTENSION_TASKS,
        get_script,
        reserved,
    };
    // SAFETY: The caller supplied a non-null writable Module table.
    unsafe { output.write(module) };
    AbiStatus::Ok
}

/// Revokes Host callbacks even when running workers prevent module unloading.
#[doc(hidden)]
pub unsafe extern "C" fn shutdown(_context: *mut c_void) -> AbiStatus {
    // Always release main-thread engine values before engine teardown, even
    // when detached workers require the library to remain loaded. Future
    // destructors may log, so cancellation must precede the exclusive lock.
    let status = cancel_tasks_for_unload(false);
    let released = std::panic::catch_unwind(release_return_backings).is_ok();
    crate::script::clear_script_registry();
    let _logs = HOST_LOG_ACCESS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    HOST_API.store(ptr::null_mut(), Ordering::Release);
    if released { status } else { AbiStatus::Panic }
}

unsafe extern "C" fn poll_tasks() -> AbiStatus {
    match std::panic::catch_unwind(crate::task::poll_frame) {
        Ok(()) => AbiStatus::Ok,
        Err(_) => AbiStatus::Panic,
    }
}

unsafe extern "C" fn cancel_tasks() -> AbiStatus {
    cancel_tasks_for_unload(true)
}

fn cancel_tasks_for_unload(preserve_tasks_if_busy: bool) -> AbiStatus {
    match std::panic::catch_unwind(|| {
        if preserve_tasks_if_busy && !crate::task::blocking_workers_finished() {
            return AbiStatus::Unsupported;
        }
        crate::task::cancel_all();
        // User destructors can start blocking work while tasks are cancelled.
        if crate::task::blocking_workers_finished() {
            AbiStatus::Ok
        } else {
            AbiStatus::Unsupported
        }
    }) {
        Ok(status) => status,
        Err(_) => AbiStatus::Panic,
    }
}

/// Allocates one UTF-8 value that the Host releases through the module table.
#[doc(hidden)]
#[must_use]
pub fn owned_utf8(value: String) -> AbiValueV1 {
    owned_text(AbiValueType::STRING, value)
}

/// Allocates an owned UTF-8 value with an exact text-like ABI type.
#[doc(hidden)]
#[must_use]
pub fn owned_text(value_type: AbiValueType, value: String) -> AbiValueV1 {
    assert!(matches!(
        value_type,
        AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH
    ));
    let bytes = value.into_bytes().into_boxed_slice();
    let length = bytes.len();
    let pointer = Box::into_raw(bytes) as *mut u8;
    AbiValueV1 {
        type_: value_type,
        reserved_flags: ABI_VALUE_OWNED_UTF8,
        payload: [
            pointer as usize as u64,
            u64::try_from(length).expect("supported pointer widths fit the UTF-8 ABI"),
        ],
    }
}

/// Allocates fixed-layout components released through the module table.
#[doc(hidden)]
#[must_use]
pub fn owned_f32_components(value_type: AbiValueType, value: &[f32]) -> AbiValueV1 {
    assert!(matches!(
        value_type,
        AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION
    ));
    let bytes = value
        .iter()
        .flat_map(|component| component.to_ne_bytes())
        .collect::<Vec<_>>();
    owned_bytes(value_type, bytes)
}

/// Allocates one dynamic byte value released through the module table.
#[doc(hidden)]
#[must_use]
pub fn owned_bytes(value_type: AbiValueType, bytes: Vec<u8>) -> AbiValueV1 {
    assert!(matches!(
        value_type,
        AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION
            | AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY
            | AbiValueType::VARIANT
            | AbiValueType::ARRAY
            | AbiValueType::DICTIONARY
            | AbiValueType::CALLABLE
            | AbiValueType::SIGNAL
    ));
    let bytes = bytes.into_boxed_slice();
    let length = bytes.len();
    let pointer = Box::into_raw(bytes) as *mut u8;
    AbiValueV1 {
        type_: value_type,
        reserved_flags: ABI_VALUE_OWNED_BYTES,
        payload: [
            pointer as usize as u64,
            u64::try_from(length).expect("supported pointer widths fit the value ABI"),
        ],
    }
}

/// Releases one dynamic value allocated by [`owned_utf8`].
#[doc(hidden)]
pub unsafe extern "C" fn drop_owned_value(value: AbiValueV1) -> AbiStatus {
    if value.reserved_flags == godot_api::abi::ABI_VALUE_MODULE_OBJECT_REF {
        return std::panic::catch_unwind(|| drop_module_object_ref(value))
            .unwrap_or(AbiStatus::Panic);
    }
    let Ok((address, length)) = owned_value_range(value) else {
        return AbiStatus::InvalidArgument;
    };
    let retained =
        if MODULE_RETURNS.get().is_null() {
            Ok(None)
        } else {
            with_return_registry(|registry| {
                let mut values = registry
                    .try_borrow_mut()
                    .map_err(|_| AbiStatus::ReentrantCall)?;
                if values.bytes.get(&address).is_some_and(|entry| {
                    entry.bytes.len() != length || entry.value_type != value.type_
                }) {
                    return Err(AbiStatus::InvalidArgument);
                }
                Ok(values.bytes.remove(&address))
            })
        };
    match retained {
        Err(status) => return status,
        Ok(Some(value)) => {
            return if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(value))).is_ok()
            {
                AbiStatus::Ok
            } else {
                AbiStatus::Panic
            };
        }
        Ok(None) => {}
    }
    // SAFETY: The module owns this exact bounded range until the allocation is
    // reconstructed below.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    let host = HOST_API.load(Ordering::Acquire);
    let mut release_status = AbiStatus::Ok;
    let mut release_callable = |token| {
        let status = release_host_value(
            host,
            AbiValueV1 {
                type_: AbiValueType::CALLABLE,
                reserved_flags: ABI_VALUE_OWNED_CALLABLE,
                payload: [token, 0],
            },
        );
        if status != AbiStatus::Ok && release_status == AbiStatus::Ok {
            release_status = status;
        }
        true
    };
    if value.type_ == AbiValueType::CALLABLE {
        if let Some(token) = godot_api::abi::callable_value_ownership_token(bytes) {
            let _ = release_callable(token);
        }
    } else if matches!(
        value.type_,
        AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY
    ) && !godot_api::abi::visit_dynamic_callable_tokens(bytes, &mut release_callable)
        && release_status == AbiStatus::Ok
    {
        release_status = AbiStatus::InvalidArgument;
    }
    drop_owned_range(address, length);
    release_status
}

/// Releases only the temporary byte allocation produced while normalizing a
/// Native engine return. Callable ownership is carried separately by
/// `NativeEngineValue` and must not be sent through Host callbacks.
pub(crate) unsafe fn drop_native_engine_value(value: AbiValueV1) -> AbiStatus {
    let Ok((address, length)) = owned_value_range(value) else {
        return AbiStatus::InvalidArgument;
    };
    drop_owned_range(address, length);
    AbiStatus::Ok
}

fn owned_value_range(value: AbiValueV1) -> Result<(usize, usize), AbiStatus> {
    let valid_text = matches!(
        value.type_,
        AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH
    ) && value.reserved_flags == ABI_VALUE_OWNED_UTF8;
    let valid_bytes = matches!(
        value.type_,
        AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION
            | AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY
            | AbiValueType::VARIANT
            | AbiValueType::ARRAY
            | AbiValueType::DICTIONARY
            | AbiValueType::CALLABLE
            | AbiValueType::SIGNAL
    ) && value.reserved_flags == ABI_VALUE_OWNED_BYTES;
    if !valid_text && !valid_bytes {
        return Err(AbiStatus::InvalidArgument);
    }
    let Ok(address) = usize::try_from(value.payload[0]) else {
        return Err(AbiStatus::InvalidArgument);
    };
    let Ok(length) = usize::try_from(value.payload[1]) else {
        return Err(AbiStatus::InvalidArgument);
    };
    if address == 0 {
        return Err(AbiStatus::InvalidArgument);
    }
    Ok((address, length))
}

fn drop_owned_range(address: usize, length: usize) {
    let slice = ptr::slice_from_raw_parts_mut(address as *mut u8, length);
    // SAFETY: The exact thin pointer and slice length came from
    // `Box<[u8]>` in `owned_utf8` and this callback is invoked once.
    unsafe { drop(Box::from_raw(slice)) };
}

pub(crate) fn write_log(level: Level, arguments: fmt::Arguments<'_>) {
    if HOST_API.load(Ordering::Acquire).is_null() {
        write_fallback_log(level, arguments);
        return;
    }
    // Display implementations are user code and may log or initiate shutdown.
    // Format before retaining the Host log callback to permit that re-entry.
    let message = arguments.to_string();
    let logs = HOST_LOG_ACCESS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let host = HOST_API.load(Ordering::Acquire);
    if host.is_null() {
        drop(logs);
        write_fallback_log(level, format_args!("{message}"));
        return;
    }
    // SAFETY: The read guard prevents shutdown from clearing this table and
    // returning to a Host that may then unload the callback's library.
    let host = unsafe { &*host };
    let Some(log) = host.log else {
        return;
    };

    let level = match level {
        Level::Debug => AbiLogLevel::Debug,
        Level::Info => AbiLogLevel::Info,
        Level::Warning => AbiLogLevel::Warning,
        Level::Error => AbiLogLevel::Error,
    };
    // SAFETY: Both byte slices remain live for this synchronous Host call.
    unsafe {
        log(
            host.context,
            level,
            AbiByteSlice::from_static("godot_rs"),
            AbiByteSlice {
                ptr: message.as_ptr(),
                len: message.len(),
            },
        )
    };
}

fn write_fallback_log(level: Level, arguments: fmt::Arguments<'_>) {
    let message = arguments.to_string();
    if crate::native::runtime::active_interface().is_some() {
        let value = crate::variant::Variant::from(message.clone());
        let result = match level {
            Level::Debug => crate::engine::utility::print_verbose(&value, &[]),
            Level::Info => crate::engine::utility::print(&value, &[]),
            Level::Warning => crate::engine::utility::push_warning(&value, &[]),
            Level::Error => crate::engine::utility::push_error(&value, &[]),
        };
        if result.is_ok() {
            return;
        }
    }
    eprintln!("{message}");
}

pub(crate) fn emit_signal(field_index: u32, arguments: &[AbiValueV1]) -> AbiCallResult {
    let host = HOST_API.load(Ordering::Acquire);
    if host.is_null() {
        return AbiCallResult::failure(
            AbiStatus::Unsupported,
            "the godot-rust Host is not initialized",
        );
    }
    // SAFETY: The Host table remains valid until module shutdown.
    let host = unsafe { &*host };
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 3) {
        return AbiCallResult::failure(
            AbiStatus::Unsupported,
            "the godot-rust Host does not support signal emission",
        );
    }
    let callback_slot = host.reserved[HOST_API_SLOT_EMIT_SIGNAL];
    if callback_slot == 0 {
        return AbiCallResult::failure(
            AbiStatus::Unsupported,
            "the godot-rust Host did not provide signal emission",
        );
    }
    // SAFETY: ABI minor 3 defines reserved slot zero as `AbiEmitSignalFn`.
    let callback = unsafe { core::mem::transmute::<usize, AbiEmitSignalFn>(callback_slot) }
        .expect("non-zero callback slot");
    let Ok(argument_count) = u32::try_from(arguments.len()) else {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "signal argument count exceeds the project ABI",
        );
    };
    // SAFETY: The callback is retained by this live Host table and the scalar
    // argument slice remains valid for the synchronous invocation.
    unsafe {
        callback(
            host.context,
            field_index,
            arguments.as_ptr(),
            argument_count,
        )
    }
}

pub(crate) fn watch_signal(signal: AbiValueV1) -> EngineResult<u64> {
    let host = engine_host_api("the godot-rust Host does not support Signal futures")?;
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 34) {
        return Err(EngineError::unavailable(
            "the godot-rust Host ABI is too old for Signal futures",
        ));
    }
    let callback_slot = host.reserved[HOST_API_SLOT_WATCH_SIGNAL];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide Signal futures",
        ));
    }
    // SAFETY: ABI minor 34 defines this slot as `AbiWatchSignalFn`.
    let callback = unsafe { core::mem::transmute::<usize, AbiWatchSignalFn>(callback_slot) }
        .expect("non-zero callback slot");
    let mut token = 0_u64;
    // SAFETY: The callback belongs to the live Host, the encoded Signal
    // remains live for this synchronous invocation, and `token` is writable.
    let result = unsafe { callback(host.context, signal, &mut token) };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    if token == 0 {
        return Err(EngineError::invalid_result(
            "the godot-rust Host returned an invalid Signal future token",
        ));
    }
    Ok(token)
}

pub(crate) fn poll_signal(token: u64) -> EngineResult<bool> {
    let host = engine_host_api("the godot-rust Host does not support Signal futures")?;
    let callback_slot = host.reserved[HOST_API_SLOT_POLL_SIGNAL];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide Signal future polling",
        ));
    }
    // SAFETY: ABI minor 34 defines this slot as `AbiPollSignalFn`.
    let callback = unsafe { core::mem::transmute::<usize, AbiPollSignalFn>(callback_slot) }
        .expect("non-zero callback slot");
    let mut fired = 0_u8;
    // SAFETY: The callback belongs to the live Host and `fired` is writable
    // for the complete synchronous invocation.
    let result = unsafe { callback(host.context, token, &mut fired) };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    match fired {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(EngineError::invalid_result(
            "the godot-rust Host returned an invalid Signal future state",
        )),
    }
}

pub(crate) fn poll_signal_values(token: u64) -> EngineResult<Option<Vec<crate::variant::Variant>>> {
    let host = engine_host_api("the godot-rust Host does not support typed Signal futures")?;
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 37) {
        return Err(EngineError::unavailable(
            "typed Signal futures require godot-rust Host ABI 3.37 or later",
        ));
    }
    let callback_slot = host.reserved[godot_api::abi::HOST_API_SLOT_POLL_SIGNAL_VALUES];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide typed Signal polling",
        ));
    }
    // SAFETY: ABI minor 37 defines this slot as AbiPollSignalValuesFn.
    let callback = unsafe {
        core::mem::transmute::<usize, godot_api::abi::AbiPollSignalValuesFn>(callback_slot)
    }
    .expect("non-zero callback slot");
    let mut fired = 0_u8;
    let mut value = AbiValueV1::NIL;
    // SAFETY: The Host owns the callback, and both outputs remain writable
    // throughout this synchronous call. HostMethodValue owns returned storage.
    let result = unsafe { callback(host.context, token, &mut fired, &mut value) };
    let value = HostMethodValue { value, host };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    match fired {
        0 => Ok(None),
        1 if value.abi() == AbiValueV1::NIL => Ok(Some(Vec::new())),
        1 => {
            use crate::engine::EngineReturn;
            let values =
                crate::variant::Array::<crate::variant::Variant>::__from_host_return(value)?;
            Ok(Some(values.to_vec()?))
        }
        _ => Err(EngineError::invalid_result(
            "the godot-rust Host returned an invalid Signal future state",
        )),
    }
}

pub(crate) fn cancel_signal(token: u64) {
    let host = HOST_API.load(Ordering::Acquire);
    if host.is_null() {
        return;
    }
    // SAFETY: The table remains live until module shutdown, which first drops
    // all cooperative tasks.
    let host = unsafe { &*host };
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 34) {
        return;
    }
    let callback_slot = host.reserved[HOST_API_SLOT_CANCEL_SIGNAL];
    if callback_slot == 0 {
        return;
    }
    // SAFETY: ABI minor 34 defines this slot as `AbiCancelSignalFn`.
    let callback = unsafe { core::mem::transmute::<usize, AbiCancelSignalFn>(callback_slot) }
        .expect("non-zero callback slot");
    // SAFETY: The callback and context belong to the live Host table.
    let _ = unsafe { callback(host.context, token) };
}

pub(crate) fn current_owner_id() -> EngineResult<u64> {
    let host = engine_host_api("the godot-rust Host does not support current object access")?;
    let callback_slot = host.reserved[HOST_API_SLOT_CURRENT_OWNER];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide current object access",
        ));
    }
    // SAFETY: ABI minor 5 defines this reserved slot as
    // `AbiCurrentOwnerFn`; non-zero is the Some representation.
    let callback = unsafe { core::mem::transmute::<usize, AbiCurrentOwnerFn>(callback_slot) }
        .expect("non-zero callback slot");
    let mut output = 0_u64;
    // SAFETY: The callback belongs to the live Host table and `output` is
    // writable for the complete synchronous invocation.
    let result = unsafe { callback(host.context, &mut output) };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    if output == 0 {
        return Err(EngineError::invalid_result(
            "the godot-rust Host returned an invalid current object",
        ));
    }
    Ok(output)
}

pub(crate) fn call_godot_method(
    receiver: u64,
    method: &'static AbiGodotMethodSpecV1,
    arguments: &[AbiValueV1],
) -> EngineResult<HostMethodValue> {
    let host = engine_host_api("the godot-rust Host does not support Godot method calls")?;
    let callback_slot = host.reserved[HOST_API_SLOT_CALL_GODOT_METHOD];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide Godot method calls",
        ));
    }
    // SAFETY: ABI minor 5 defines this reserved slot as
    // `AbiCallGodotMethodFn`; non-zero is the Some representation.
    let callback = unsafe { core::mem::transmute::<usize, AbiCallGodotMethodFn>(callback_slot) }
        .expect("non-zero callback slot");
    let argument_count = u32::try_from(arguments.len()).map_err(|_| {
        EngineError::invalid_result("Godot method argument count exceeds the project ABI")
    })?;
    let mut output = AbiValueV1::NIL;
    // SAFETY: Generated method metadata has static storage, the callback
    // belongs to the live Host table, and both value buffers remain live for
    // this synchronous invocation.
    let result = unsafe {
        callback(
            host.context,
            receiver,
            method,
            arguments.as_ptr(),
            argument_count,
            &mut output,
        )
    };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    Ok(HostMethodValue {
        value: output,
        host: ptr::from_ref(host),
    })
}

pub(crate) fn call_super_method(name: &str, arguments: &[AbiValueV1]) -> EngineResult<AbiValueV1> {
    let host = engine_host_api("the godot-rust Host does not support base script calls")?;
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 35) {
        return Err(EngineError::unavailable(
            "the godot-rust Host ABI is too old for base script calls",
        ));
    }
    let callback_slot = host.reserved[HOST_API_SLOT_CALL_SUPER_METHOD];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide base script calls",
        ));
    }
    let argument_count = u32::try_from(arguments.len()).map_err(|_| {
        EngineError::invalid_result("base script argument count exceeds the project ABI")
    })?;
    // SAFETY: ABI minor 35 defines this slot as `AbiCallSuperMethodFn`.
    let callback = unsafe { core::mem::transmute::<usize, AbiCallSuperMethodFn>(callback_slot) }
        .expect("non-zero callback slot");
    let mut output = AbiValueV1::NIL;
    // SAFETY: Method text and arguments remain live for this complete
    // synchronous callback and output is writable.
    let result = unsafe {
        callback(
            host.context,
            AbiByteSlice {
                ptr: name.as_ptr(),
                len: name.len(),
            },
            arguments.as_ptr(),
            argument_count,
            &mut output,
        )
    };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    Ok(output)
}

pub(crate) fn call_script_member(
    receiver: u64,
    script_uid: i64,
    member: u64,
    operation: u8,
    arguments: &[AbiValueV1],
) -> EngineResult<HostMethodValue> {
    let host = engine_host_api("typed script access requires the Script Mode Host")?;
    let slot = host.reserved[godot_api::abi::HOST_API_SLOT_CALL_SCRIPT_MEMBER];
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 37) || slot == 0 {
        return Err(EngineError::unavailable(
            "this Host does not support typed script access",
        ));
    }
    let callback = {
        // SAFETY: ABI 37 assigns this nonzero slot to AbiCallScriptMemberFn.
        unsafe { core::mem::transmute::<usize, godot_api::abi::AbiCallScriptMemberFn>(slot) }
    }
    .expect("nonzero script callback");
    let count = u32::try_from(arguments.len())
        .map_err(|_| EngineError::invalid_argument("too many typed script arguments"))?;
    let mut output = AbiValueV1::NIL;
    // SAFETY: The Host and all buffers stay alive throughout this synchronous call.
    let result = unsafe {
        callback(
            host.context,
            receiver,
            script_uid,
            member,
            operation,
            arguments.as_ptr(),
            count,
            &mut output,
        )
    };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    Ok(HostMethodValue {
        value: output,
        host: ptr::from_ref(host),
    })
}

pub(crate) fn validate_object(
    receiver: u64,
    class_name: &'static str,
    script_uid: Option<i64>,
) -> EngineResult<()> {
    let host = engine_host_api("object validation requires an active Godot runtime")?;
    let slot = host.reserved[godot_api::abi::HOST_API_SLOT_VALIDATE_OBJECT];
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 37) || slot == 0 {
        return Err(EngineError::unavailable(
            "this Host does not support checked object casts",
        ));
    }
    let callback = {
        // SAFETY: ABI 37 assigns this nonzero slot to AbiValidateObjectFn.
        unsafe { core::mem::transmute::<usize, godot_api::abi::AbiValidateObjectFn>(slot) }
    }
    .expect("nonzero object validation callback");
    // SAFETY: Static class text and the Host table remain live for the call.
    let result = unsafe {
        callback(
            host.context,
            receiver,
            AbiByteSlice::from_static(class_name),
            script_uid.unwrap_or(-1),
        )
    };
    if result.status == AbiStatus::Ok {
        Ok(())
    } else {
        Err(EngineError::from_abi(result))
    }
}

pub(crate) fn release_module_output(value: AbiValueV1) {
    if matches!(
        value.reserved_flags,
        ABI_VALUE_OWNED_UTF8 | ABI_VALUE_OWNED_BYTES | godot_api::abi::ABI_VALUE_MODULE_OBJECT_REF
    ) {
        // SAFETY: A successful base-method call transfers exactly one
        // project-module-owned value back to this SDK.
        let _ = unsafe { drop_owned_value(value) };
    }
}

pub(crate) fn call_godot_api(
    spec: &'static AbiGodotApiSpecV1,
    base: Option<AbiValueV1>,
    arguments: &[AbiValueV1],
    mutates_base: bool,
) -> EngineResult<(HostMethodValue, Option<HostMethodValue>)> {
    let host = engine_host_api("the godot-rust Host does not support generated Godot APIs")?;
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 32) {
        return Err(EngineError::unavailable(
            "the godot-rust Host ABI is too old for generated Godot APIs",
        ));
    }
    let callback_slot = host.reserved[HOST_API_SLOT_CALL_GODOT_API];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide generated Godot APIs",
        ));
    }
    // SAFETY: ABI minor 32 defines this slot as `AbiCallGodotApiFn`.
    let callback = unsafe { core::mem::transmute::<usize, AbiCallGodotApiFn>(callback_slot) }
        .expect("non-zero callback slot");
    let argument_count = u32::try_from(arguments.len()).map_err(|_| {
        EngineError::invalid_result("Godot API argument count exceeds the project ABI")
    })?;
    let mut output = AbiValueV1::NIL;
    let mut updated_base = AbiValueV1::NIL;
    // SAFETY: The generated contract and all value buffers remain live for
    // this synchronous Host call.
    let result = unsafe {
        callback(
            host.context,
            spec,
            base.as_ref().map_or(ptr::null(), ptr::from_ref),
            arguments.as_ptr(),
            argument_count,
            &mut output,
            if mutates_base {
                &mut updated_base
            } else {
                ptr::null_mut()
            },
        )
    };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    let host = ptr::from_ref(host);
    Ok((
        HostMethodValue {
            value: output,
            host,
        },
        mutates_base.then_some(HostMethodValue {
            value: updated_base,
            host,
        }),
    ))
}

#[doc(hidden)]
pub struct HostMethodValue {
    value: AbiValueV1,
    host: *const HostApiV1,
}

impl HostMethodValue {
    #[doc(hidden)]
    pub const fn abi(&self) -> AbiValueV1 {
        self.value
    }

    pub(crate) fn into_owned_object_ref(
        mut self,
    ) -> EngineResult<Option<(u64, HostObjectRefToken)>> {
        if self.value.type_ != AbiValueType::OBJECT_ID {
            return Err(EngineError::invalid_result(
                "the godot-rust Host returned a non-object RefCounted value",
            ));
        }
        if self.value.reserved_flags == 0 && self.value.payload == [0, 0] {
            return Ok(None);
        }
        if self.value.reserved_flags != ABI_VALUE_OWNED_OBJECT_REF
            || self.value.payload[0] == 0
            || self.value.payload[1] == 0
        {
            return Err(EngineError::invalid_result(
                "the godot-rust Host returned invalid RefCounted ownership",
            ));
        }
        let object_id = self.value.payload[0];
        let token = HostObjectRefToken {
            value: self.value,
            host: self.host,
        };
        self.value = AbiValueV1::NIL;
        Ok(Some((object_id, token)))
    }
}

pub(crate) fn retain_object_ref(object_id: u64) -> EngineResult<HostObjectRefToken> {
    let host = engine_host_api("retaining a GodotRef requires the Script Mode Host")?;
    let slot = host.reserved[godot_api::abi::HOST_API_SLOT_RETAIN_OBJECT];
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 37) || slot == 0 {
        return Err(EngineError::unavailable(
            "this Host cannot retain borrowed GodotRef arguments",
        ));
    }
    let callback = {
        // SAFETY: ABI minor 37 assigns this nonzero slot to AbiRetainObjectFn.
        unsafe { core::mem::transmute::<usize, godot_api::abi::AbiRetainObjectFn>(slot) }
    }
    .expect("nonzero retention callback");
    let mut value = AbiValueV1::NIL;
    // SAFETY: The Host and writable output remain live through this call.
    let result = unsafe { callback(host.context, object_id, &mut value) };
    let value = HostMethodValue { value, host };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    let (retained_id, ownership) = value
        .into_owned_object_ref()?
        .ok_or_else(|| EngineError::invalid_result("the Host retained a null GodotRef"))?;
    if retained_id != object_id {
        return Err(EngineError::invalid_result(
            "the Host retained a different GodotRef object",
        ));
    }
    Ok(ownership)
}

pub(crate) fn take_owned_object_ref(
    value: AbiValueV1,
) -> Option<Option<(u64, HostObjectRefToken)>> {
    let host =
        engine_host_api("the godot-rust Host does not support owned Godot references").ok()?;
    HostMethodValue { value, host }.into_owned_object_ref().ok()
}

impl Drop for HostMethodValue {
    fn drop(&mut self) {
        if !matches!(
            self.value.reserved_flags,
            ABI_VALUE_OWNED_UTF8 | ABI_VALUE_OWNED_OBJECT_REF | ABI_VALUE_OWNED_BYTES
        ) {
            return;
        }
        release_host_value(self.host, self.value);
    }
}

pub(crate) struct HostObjectRefToken {
    value: AbiValueV1,
    host: *const HostApiV1,
}

impl Drop for HostObjectRefToken {
    fn drop(&mut self) {
        release_host_value(self.host, self.value);
    }
}

pub(crate) struct HostDynamicValueToken {
    value: AbiValueV1,
    host: *const HostApiV1,
}

pub(crate) struct HostContainerToken {
    kind: AbiValueType,
    token: u64,
    host: *const HostApiV1,
}

impl Drop for HostContainerToken {
    fn drop(&mut self) {
        // SAFETY: Container ownership is released before its module's Host table is retired.
        let host = unsafe { &*self.host };
        let slot = host.reserved[godot_api::abi::HOST_API_SLOT_CONTAINER];
        // SAFETY: The Host ABI defines this initialized callback slot.
        let callback =
            unsafe { core::mem::transmute::<usize, godot_api::abi::AbiContainerFn>(slot) };
        if let Some(callback) = callback {
            let mut output = AbiValueV1::NIL;
            // SAFETY: Only opaque identity values and live output storage cross this call.
            let _ = unsafe {
                callback(
                    host.context,
                    godot_api::abi::ABI_CONTAINER_RELEASE,
                    self.kind,
                    self.token,
                    AbiByteSlice::EMPTY,
                    AbiByteSlice::EMPTY,
                    &mut output,
                )
            };
        }
    }
}

pub(crate) fn retain_container(kind: AbiValueType, token: u64) -> EngineResult<HostContainerToken> {
    let value = container_access(godot_api::abi::ABI_CONTAINER_RETAIN, kind, token, &[], None)?;
    Ok(HostContainerToken {
        kind,
        token,
        host: value.host,
    })
}

pub(crate) fn container_access(
    operation: u32,
    kind: AbiValueType,
    token: u64,
    bytes: &[u8],
    element: Option<&str>,
) -> EngineResult<HostMethodValue> {
    let host = engine_host_api("shared containers require the godot-rust Host")?;
    let slot = host.reserved[godot_api::abi::HOST_API_SLOT_CONTAINER];
    if slot == 0 {
        return Err(EngineError::unavailable(
            "the Host did not provide shared containers",
        ));
    }
    // SAFETY: ABI 4 defines this callback slot as AbiContainerFn.
    let callback = unsafe { core::mem::transmute::<usize, godot_api::abi::AbiContainerFn>(slot) }
        .expect("container callback");
    let element = element.unwrap_or("");
    let mut output = AbiValueV1::NIL;
    // SAFETY: Both input slices and the writable output slot remain live throughout the synchronous call.
    let result = unsafe {
        callback(
            host.context,
            operation,
            kind,
            token,
            AbiByteSlice {
                ptr: bytes.as_ptr(),
                len: bytes.len(),
            },
            AbiByteSlice {
                ptr: element.as_ptr(),
                len: element.len(),
            },
            &mut output,
        )
    };
    if result.status != AbiStatus::Ok {
        return Err(EngineError::from_abi(result));
    }
    Ok(HostMethodValue {
        value: output,
        host: ptr::from_ref(host),
    })
}

impl Drop for HostDynamicValueToken {
    fn drop(&mut self) {
        release_host_value(self.host, self.value);
    }
}

pub(crate) struct HostCallableValueToken {
    value: AbiValueV1,
    host: *const HostApiV1,
}

impl Drop for HostCallableValueToken {
    fn drop(&mut self) {
        release_host_value(self.host, self.value);
    }
}

pub(crate) fn retain_dynamic_value(
    value_type: AbiValueType,
    token: u64,
) -> EngineResult<Option<HostDynamicValueToken>> {
    if token == 0 {
        return Ok(None);
    }
    let host = engine_host_api("the godot-rust Host does not support owned dynamic values")?;
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 23) {
        return Err(EngineError::unavailable(
            "the godot-rust Host is too old to retain dynamic values",
        ));
    }
    let callback_slot = host.reserved[HOST_API_SLOT_RETAIN_DYNAMIC_VALUE];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide dynamic-value retention",
        ));
    }
    // SAFETY: ABI minor 23 defines this slot as
    // `AbiRetainDynamicValueFn`; non-zero is the Some representation.
    let callback = unsafe {
        core::mem::transmute::<usize, AbiRetainDynamicValueFn>(callback_slot)
            .expect("non-zero callback slot")
    };
    // SAFETY: The callback belongs to the retained Host table.
    let status = unsafe { callback(host.context, token) };
    if status != AbiStatus::Ok {
        return Err(EngineError::invalid_result(
            "the godot-rust Host rejected dynamic-value ownership",
        ));
    }
    Ok(Some(HostDynamicValueToken {
        value: AbiValueV1 {
            type_: value_type,
            reserved_flags: ABI_VALUE_OWNED_DYNAMIC_GROUP,
            payload: [token, 0],
        },
        host: ptr::from_ref(host),
    }))
}

pub(crate) fn retain_callable_value(token: u64) -> EngineResult<Option<HostCallableValueToken>> {
    if token == 0 {
        return Ok(None);
    }
    let host = engine_host_api("the godot-rust Host does not support owned Callables")?;
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 25) {
        return Err(EngineError::unavailable(
            "the godot-rust Host is too old to retain Callables",
        ));
    }
    let callback_slot = host.reserved[HOST_API_SLOT_RETAIN_CALLABLE_VALUE];
    if callback_slot == 0 {
        return Err(EngineError::unavailable(
            "the godot-rust Host did not provide Callable retention",
        ));
    }
    // SAFETY: ABI minor 25 defines this slot as
    // `AbiRetainCallableValueFn`; non-zero is the Some representation.
    let callback = unsafe {
        core::mem::transmute::<usize, AbiRetainCallableValueFn>(callback_slot)
            .expect("non-zero callback slot")
    };
    // SAFETY: The callback belongs to the retained Host table.
    let status = unsafe { callback(host.context, token) };
    if status != AbiStatus::Ok {
        return Err(EngineError::invalid_result(
            "the godot-rust Host rejected Callable ownership",
        ));
    }
    Ok(Some(HostCallableValueToken {
        value: AbiValueV1 {
            type_: AbiValueType::CALLABLE,
            reserved_flags: ABI_VALUE_OWNED_CALLABLE,
            payload: [token, 0],
        },
        host: ptr::from_ref(host),
    }))
}

fn release_host_value(host: *const HostApiV1, value: AbiValueV1) -> AbiStatus {
    if host.is_null() {
        return AbiStatus::InvalidArgument;
    }
    // SAFETY: Owned Host values never outlive the project-module generation
    // whose retained Host table created them.
    let host = unsafe { &*host };
    let callback_slot = host.reserved[HOST_API_SLOT_DROP_VALUE];
    if callback_slot == 0 {
        return AbiStatus::Unsupported;
    }
    // SAFETY: ABI minor 12 defines this reserved slot as
    // `AbiDropHostValueFn`; non-zero is the Some representation.
    let callback = unsafe { core::mem::transmute::<usize, AbiDropHostValueFn>(callback_slot) }
        .expect("non-zero callback slot");
    // SAFETY: The caller transfers each owned value through its paired Host
    // callback exactly once.
    unsafe { callback(host.context, value) }
}

fn engine_host_api(unavailable: &'static str) -> EngineResult<&'static HostApiV1> {
    let host = HOST_API.load(Ordering::Acquire);
    if host.is_null() {
        return Err(EngineError::unavailable(
            "the godot-rust Host is not initialized",
        ));
    }
    // SAFETY: The table remains live until module shutdown; callers copy the
    // selected callback and invoke it synchronously.
    let host = unsafe { &*host };
    if !host.header.is_compatible(HostApiV1::MINIMUM_SIZE, 5) {
        return Err(EngineError::unavailable(unavailable));
    }
    Ok(host)
}

/// Internal function shape used by generated script indexes.
#[doc(hidden)]
pub type ScriptDescriptorWriter = fn(*mut godot_api::abi::AbiScriptDescriptorV1) -> AbiStatus;

/// Generates the one stable project-module entry from a plugin-managed script
/// index. Ordinary script files never invoke this macro themselves.
#[macro_export]
macro_rules! script_module {
    ($($script:ty => ($source_path:literal, $resource_uid:literal)),* $(,)?) => {
        unsafe extern "C" fn __godot_rs_get_script(
            index: u32,
            output: *mut $crate::abi::AbiScriptDescriptorV1,
        ) -> $crate::abi::AbiStatus {
            let writers: &[$crate::module::ScriptDescriptorWriter] = &[
                $(|output| {
                    // SAFETY: The generated ABI getter forwards the Host output slot.
                    unsafe {
                        $crate::script::write_abi_script_descriptor::<$script>(
                            $source_path,
                            $resource_uid,
                            output,
                        )
                    }
                }),*
            ];
            let Some(writer) = writers.get(index as usize) else {
                return $crate::abi::AbiStatus::InvalidArgument;
            };
            writer(output)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn godot_rs_module_entry(
            host: *const $crate::abi::HostApiV1,
            output: *mut $crate::abi::ModuleApiV1,
        ) -> $crate::abi::AbiStatus {
            let script_count = 0_u32 $(+ {
                let _ = stringify!($script);
                1_u32
            })*;
            // SAFETY: The Host owns both ABI tables for this entry call.
            unsafe {
                $crate::module::initialize(
                    host,
                    output,
                    script_count,
                    Some(__godot_rs_get_script),
                    Some($crate::module::shutdown),
                )
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn release_test_host_value(
        context: *mut c_void,
        value: AbiValueV1,
    ) -> AbiStatus {
        // SAFETY: The test supplies a live bool as the callback context.
        unsafe { context.cast::<bool>().write(true) };
        // SAFETY: The test value was allocated by `owned_utf8` and is released once.
        unsafe { drop_owned_value(value) }
    }

    unsafe extern "C" fn release_test_object_ref(
        context: *mut c_void,
        value: AbiValueV1,
    ) -> AbiStatus {
        if value.type_ != AbiValueType::OBJECT_ID
            || value.reserved_flags != ABI_VALUE_OWNED_OBJECT_REF
        {
            return AbiStatus::InvalidArgument;
        }
        // SAFETY: The test supplies a live bool as the callback context.
        unsafe { context.cast::<bool>().write(true) };
        AbiStatus::Ok
    }

    #[test]
    fn module_object_return_keeps_last_reference_until_matching_release() {
        let mut released = false;
        let mut reserved = [0; 16];
        reserved[HOST_API_SLOT_DROP_VALUE] = release_test_object_ref as *const () as usize;
        let host = HostApiV1 {
            header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
            context: ptr::from_mut(&mut released).cast(),
            log: None,
            reserved,
        };
        let object = crate::engine::GodotRef::<crate::engine::RefCounted>::from_owned_parts(
            42,
            HostObjectRefToken {
                value: AbiValueV1 {
                    type_: AbiValueType::OBJECT_ID,
                    reserved_flags: ABI_VALUE_OWNED_OBJECT_REF,
                    payload: [42, 7],
                },
                host: ptr::from_ref(&host),
            },
        );
        let output = owned_object_ref(object).expect("module owns the returned reference");
        assert!(
            !released,
            "return conversion must keep the only engine reference alive"
        );
        assert_eq!(
            output.reserved_flags,
            godot_api::abi::ABI_VALUE_MODULE_OBJECT_REF
        );
        let wrong_id = AbiValueV1 {
            payload: [43, output.payload[1]],
            ..output
        };
        assert_eq!(drop_module_object_ref(wrong_id), AbiStatus::InvalidArgument);
        assert!(!released);
        // SAFETY: The module-owned output remains live throughout decoding.
        let decoded = unsafe {
            <crate::engine::GodotRef<crate::engine::RefCounted>
            as crate::script::FromAbiValue>::from_abi(output)
        }
        .expect("same-module super result");
        // SAFETY: output transfers exactly one registered module return reference.
        assert_eq!(unsafe { drop_owned_value(output) }, AbiStatus::Ok);
        assert!(!released, "decoded super result retains its own reference");
        assert_eq!(drop_module_object_ref(output), AbiStatus::StaleHandle);
        drop(decoded);
        assert!(released);
        release_return_backings();
    }

    fn test_object(host: &HostApiV1) -> crate::engine::GodotRef<crate::engine::RefCounted> {
        crate::engine::GodotRef::from_owned_parts(
            42,
            HostObjectRefToken {
                value: AbiValueV1 {
                    type_: AbiValueType::OBJECT_ID,
                    reserved_flags: ABI_VALUE_OWNED_OBJECT_REF,
                    payload: [42, 7],
                },
                host: ptr::from_ref(host),
            },
        )
    }

    fn object_container(kind: usize, host: &HostApiV1) -> (AbiValueType, crate::variant::Variant) {
        use crate::variant::{Array, Dictionary, Variant};
        let object = test_object(host);
        match kind {
            0 => (AbiValueType::VARIANT, Variant::from(object)),
            1 => (
                AbiValueType::ARRAY,
                Variant::from(Array::from_vec(vec![object]).expect("array initializer")),
            ),
            2 => (
                AbiValueType::DICTIONARY,
                Variant::from(
                    Dictionary::from_entries(vec![(
                        Variant::from("resource"),
                        Variant::from(object),
                    )])
                    .expect("dictionary initializer"),
                ),
            ),
            _ => unreachable!(),
        }
    }

    #[test]
    fn dynamic_returns_and_signal_arguments_keep_their_last_resource_alive() {
        use crate::signal::SignalValue;
        for kind in 0..3 {
            let mut released = false;
            let mut reserved = [0; 16];
            reserved[HOST_API_SLOT_DROP_VALUE] = release_test_object_ref as *const () as usize;
            let host = HostApiV1 {
                header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
                context: ptr::from_mut(&mut released).cast(),
                log: None,
                reserved,
            };
            let (value_type, value) = object_container(kind, &host);
            let output = owned_dynamic_value(value_type, value).expect("owned container result");
            assert!(
                !released,
                "return wire must retain embedded resource ownership"
            );
            let wrong_type = AbiValueV1 {
                type_: AbiValueType::CALLABLE,
                ..output
            };
            assert_eq!(
                // SAFETY: A mismatched type is rejected without consuming the live allocation.
                unsafe { drop_owned_value(wrong_type) },
                AbiStatus::InvalidArgument
            );
            assert!(!released);
            // SAFETY: This releases the exact registered output once.
            assert_eq!(unsafe { drop_owned_value(output) }, AbiStatus::Ok);
            assert!(released);

            released = false;
            let (_, value) = object_container(kind, &host);
            let mut backing = None;
            let signal_value = value.into_signal_value(&mut backing);
            assert_eq!(signal_value.type_, AbiValueType::VARIANT);
            assert!(
                !released,
                "synchronous signal arguments must own embedded resources"
            );
            drop(backing);
            assert!(released);
        }
        release_return_backings();
    }

    #[test]
    fn callable_return_keeps_its_refcounted_target_until_native_copy() {
        use crate::script::IntoMethodResult;
        let mut released = false;
        let mut reserved = [0; 16];
        reserved[HOST_API_SLOT_DROP_VALUE] = release_test_object_ref as *const () as usize;
        let host = HostApiV1 {
            header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
            context: ptr::from_mut(&mut released).cast(),
            log: None,
            reserved,
        };
        let object = test_object(&host);
        let callable = crate::callable::Callable::from_godot_ref_method(&object, "get_value");
        drop(object);
        let mut output = AbiValueV1::NIL;
        assert_eq!(callable.write_result(&mut output).status, AbiStatus::Ok);
        assert!(!released);
        let (pointer, length) = output
            .byte_range(AbiValueType::CALLABLE)
            .expect("Callable output");
        // SAFETY: The registered module return remains live until the drop below.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        assert!(godot_api::abi::callable_value_retains_target(bytes));
        // SAFETY: This releases the exact registered output once.
        assert_eq!(unsafe { drop_owned_value(output) }, AbiStatus::Ok);
        assert!(released);
        release_return_backings();
    }

    #[test]
    fn dynamic_result_rejects_null_output_and_shutdown_releases_return_backings() {
        use crate::script::IntoMethodResult;
        let mut released = false;
        let mut reserved = [0; 16];
        reserved[HOST_API_SLOT_DROP_VALUE] = release_test_object_ref as *const () as usize;
        let host = HostApiV1 {
            header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
            context: ptr::from_mut(&mut released).cast(),
            log: None,
            reserved,
        };
        let (_, value) = object_container(1, &host);
        assert_eq!(
            // SAFETY: Null is explicitly allowed and rejected by the ABI encoder.
            unsafe { crate::script::encode_method_result(value, ptr::null_mut()) }.status,
            AbiStatus::InvalidArgument
        );
        assert!(released);
        assert!(with_return_registry(|values| values
            .borrow()
            .bytes
            .is_empty()));
        released = false;
        let (_, value) = object_container(2, &host);
        let mut output = AbiValueV1::NIL;
        assert_eq!(value.write_result(&mut output).status, AbiStatus::Ok);
        assert!(!released);
        release_return_backings();
        assert!(released);
        assert!(MODULE_RETURNS.get().is_null());
    }

    #[test]
    fn owned_utf8_values_use_the_paired_module_releaser() {
        let value = owned_utf8("你好，Godot".to_owned());
        assert_eq!(value.type_, AbiValueType::STRING);
        assert_eq!(value.reserved_flags, ABI_VALUE_OWNED_UTF8);
        let pointer = usize::try_from(value.payload[0]).expect("pointer");
        let length = usize::try_from(value.payload[1]).expect("length");
        // SAFETY: The owned value remains allocated until the paired release.
        let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
        assert_eq!(core::str::from_utf8(bytes), Ok("你好，Godot"));
        // SAFETY: This is the only release of the value above.
        let status = unsafe { drop_owned_value(value) };
        assert_eq!(status, AbiStatus::Ok);

        let borrowed = AbiValueV1::from_borrowed_utf8("borrowed");
        // SAFETY: Invalid input is rejected without touching its buffer.
        let status = unsafe { drop_owned_value(borrowed) };
        assert_eq!(status, AbiStatus::InvalidArgument);
    }

    #[test]
    fn owned_math_values_use_the_paired_module_releaser() {
        let components = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0];
        let value = owned_f32_components(AbiValueType::TRANSFORM3D, &components);
        assert_eq!(value.type_, AbiValueType::TRANSFORM3D);
        assert_eq!(value.reserved_flags, ABI_VALUE_OWNED_BYTES);
        let (pointer, length) = value
            .byte_range(AbiValueType::TRANSFORM3D)
            .expect("owned component bytes");
        assert_eq!(length, 12 * core::mem::size_of::<f32>());
        // SAFETY: The module retains this exact allocation until the paired
        // release below.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let returned = bytes
            .chunks_exact(core::mem::size_of::<f32>())
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().expect("f32 byte width")))
            .collect::<Vec<_>>();
        assert_eq!(returned, components);
        // SAFETY: This is the only release of the value above.
        assert_eq!(unsafe { drop_owned_value(value) }, AbiStatus::Ok);
    }

    #[test]
    fn native_engine_return_release_does_not_call_the_script_host() {
        let method = b"bound";
        let mut bytes = Vec::with_capacity(32 + method.len());
        bytes.extend_from_slice(&godot_api::abi::ABI_CALLABLE_MAGIC);
        bytes.extend_from_slice(&godot_api::abi::ABI_CALLABLE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&godot_api::abi::ABI_CALLABLE_OWNED.to_le_bytes());
        bytes.extend_from_slice(&7_u64.to_le_bytes());
        bytes.extend_from_slice(&42_u64.to_le_bytes());
        bytes.extend_from_slice(&(method.len() as u32).to_le_bytes());
        bytes.extend_from_slice(method);
        assert!(godot_api::abi::validate_callable_value(&bytes));

        let value = owned_bytes(AbiValueType::CALLABLE, bytes);
        // SAFETY: This is the only release of the allocation above. Native
        // Callable ownership lives in NativeEngineValue's side channel, so
        // this path must not consult a Script Host for token 7.
        assert_eq!(
            // SAFETY: `value` still owns the exact allocation created above.
            unsafe { drop_native_engine_value(value) },
            AbiStatus::Ok
        );
    }

    #[test]
    fn generated_engine_results_release_host_owned_text() {
        let mut released = false;
        let mut reserved = [0; 16];
        reserved[HOST_API_SLOT_DROP_VALUE] = release_test_host_value as *const () as usize;
        let host = HostApiV1 {
            header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
            context: ptr::from_mut(&mut released).cast(),
            log: None,
            reserved,
        };
        let value = HostMethodValue {
            value: owned_utf8("returned text".to_owned()),
            host: ptr::from_ref(&host),
        };
        assert_eq!(value.abi().reserved_flags, ABI_VALUE_OWNED_UTF8);
        drop(value);
        assert!(released);
    }

    #[test]
    fn refcounted_result_transfers_release_to_the_owned_token() {
        let mut released = false;
        let mut reserved = [0; 16];
        reserved[HOST_API_SLOT_DROP_VALUE] = release_test_object_ref as *const () as usize;
        let host = HostApiV1 {
            header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
            context: ptr::from_mut(&mut released).cast(),
            log: None,
            reserved,
        };
        let value = HostMethodValue {
            value: AbiValueV1 {
                type_: AbiValueType::OBJECT_ID,
                reserved_flags: ABI_VALUE_OWNED_OBJECT_REF,
                payload: [42, 7],
            },
            host: ptr::from_ref(&host),
        };
        let (object_id, ownership) = value
            .into_owned_object_ref()
            .expect("valid owned object")
            .expect("non-null object");
        assert_eq!(object_id, 42);
        assert!(!released);
        drop(ownership);
        assert!(released);
    }
}
