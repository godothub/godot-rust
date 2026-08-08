use core::ffi::c_void;
use core::fmt::{self, Write};
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use godot_rs_api::abi::{
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
    reserved[MODULE_API_SLOT_GODOT_API_MAJOR] = godot_rs_api::SELECTED_GODOT_API_MAJOR as usize;
    reserved[MODULE_API_SLOT_GODOT_API_MINOR] = godot_rs_api::SELECTED_GODOT_API_MINOR as usize;
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

/// Clears the Host callback table before a module generation is unloaded.
#[doc(hidden)]
pub unsafe extern "C" fn shutdown(_context: *mut c_void) -> AbiStatus {
    crate::task::cancel_all();
    HOST_API.store(ptr::null_mut(), Ordering::Release);
    AbiStatus::Ok
}

unsafe extern "C" fn poll_tasks() -> AbiStatus {
    match std::panic::catch_unwind(crate::task::poll_frame) {
        Ok(()) => AbiStatus::Ok,
        Err(_) => AbiStatus::Panic,
    }
}

unsafe extern "C" fn cancel_tasks() -> AbiStatus {
    match std::panic::catch_unwind(crate::task::cancel_all) {
        Ok(()) => AbiStatus::Ok,
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
    let Ok((address, length)) = owned_value_range(value) else {
        return AbiStatus::InvalidArgument;
    };
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
        if let Some(token) = godot_rs_api::abi::callable_value_ownership_token(bytes) {
            let _ = release_callable(token);
        }
    } else if matches!(
        value.type_,
        AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY
    ) && !godot_rs_api::abi::visit_dynamic_callable_tokens(bytes, &mut release_callable)
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
    let host = HOST_API.load(Ordering::Acquire);
    if host.is_null() {
        return;
    }
    // SAFETY: The Host table remains valid until shutdown, which only runs
    // after callbacks for this generation have stopped.
    let host = unsafe { &*host };
    let Some(log) = host.log else {
        return;
    };

    let mut message = StackMessage::new();
    let _ = message.write_fmt(arguments);
    let level = match level {
        Level::Info => AbiLogLevel::Info,
        Level::Warning => AbiLogLevel::Warning,
    };
    // SAFETY: Both byte slices remain live for this synchronous Host call.
    unsafe {
        log(
            host.context,
            level,
            AbiByteSlice::from_static("godot_rs"),
            message.as_abi(),
        )
    };
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

pub(crate) fn release_module_output(value: AbiValueV1) {
    if matches!(
        value.reserved_flags,
        ABI_VALUE_OWNED_UTF8 | ABI_VALUE_OWNED_BYTES
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

pub(crate) fn retain_callable_for_transfer(token: u64) -> EngineResult<()> {
    let ownership = retain_callable_value(token)?;
    if let Some(ownership) = ownership {
        // The module-owned Callable byte value releases this exact retained
        // Host reference when the Host finishes consuming the method result.
        core::mem::forget(ownership);
    }
    Ok(())
}

pub(crate) fn retain_dynamic_callables_for_transfer(bytes: &[u8]) -> EngineResult<()> {
    let mut retained = Vec::new();
    let mut error = None;
    let valid =
        godot_rs_api::abi::visit_dynamic_callable_tokens(
            bytes,
            |token| match retain_callable_value(token) {
                Ok(Some(ownership)) => {
                    retained.push(ownership);
                    true
                }
                Ok(None) => true,
                Err(value) => {
                    error = Some(value);
                    false
                }
            },
        );
    if !valid {
        return Err(error.unwrap_or_else(|| {
            EngineError::invalid_result("dynamic Callable ownership metadata is invalid")
        }));
    }
    for ownership in retained {
        // Each module-owned dynamic result releases this transferred reference
        // after the Host finishes consuming its recursive wire value.
        core::mem::forget(ownership);
    }
    Ok(())
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

struct StackMessage {
    bytes: [u8; 1024],
    len: usize,
}

impl StackMessage {
    const fn new() -> Self {
        Self {
            bytes: [0; 1024],
            len: 0,
        }
    }

    fn as_abi(&self) -> AbiByteSlice {
        AbiByteSlice {
            ptr: self.bytes.as_ptr(),
            len: self.len,
        }
    }
}

impl Write for StackMessage {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = self.bytes.len().saturating_sub(self.len);
        let count = remaining.min(value.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&value.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

/// Internal function shape used by generated script indexes.
#[doc(hidden)]
pub type ScriptDescriptorWriter = fn(*mut godot_rs_api::abi::AbiScriptDescriptorV1) -> AbiStatus;

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
        bytes.extend_from_slice(&godot_rs_api::abi::ABI_CALLABLE_MAGIC);
        bytes.extend_from_slice(&godot_rs_api::abi::ABI_CALLABLE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&godot_rs_api::abi::ABI_CALLABLE_OWNED.to_le_bytes());
        bytes.extend_from_slice(&7_u64.to_le_bytes());
        bytes.extend_from_slice(&42_u64.to_le_bytes());
        bytes.extend_from_slice(&(method.len() as u32).to_le_bytes());
        bytes.extend_from_slice(method);
        assert!(godot_rs_api::abi::validate_callable_value(&bytes));

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
