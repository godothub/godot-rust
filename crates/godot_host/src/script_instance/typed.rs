use super::*;
use godot_api::abi::{
    ABI_SCRIPT_MEMBER_GET_PROPERTY, ABI_SCRIPT_MEMBER_METHOD, ABI_SCRIPT_MEMBER_SET_PROPERTY,
    AbiByteSlice,
};
use std::cell::RefCell;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn failure(status: AbiStatus, message: impl Into<String>) -> ModuleCallError {
    ModuleCallError {
        status,
        message: message.into(),
    }
}

fn with_typed_fault_guard(
    faults: &InstanceFaults,
    invoke: impl FnOnce() -> Result<(), ModuleCallError>,
) -> Result<(), ModuleCallError> {
    if faults.is_fused() {
        return Err(failure(
            AbiStatus::CallbackFailed,
            "Rust script instance is disabled after a previous callback failure",
        ));
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(invoke))
        .unwrap_or_else(|_| Err(failure(AbiStatus::Panic, "typed script dispatch panicked")));
    // Typed errors belong to the Rust caller. Only a panic makes the target's
    // state unsafe to reuse; handled errors do not affect Godot callback faults.
    if matches!(&result, Err(error) if error.status == AbiStatus::Panic) {
        faults.record_failure(AbiStatus::Panic);
    }
    result
}

fn finish(result: Result<(), ModuleCallError>) -> AbiCallResult {
    match result {
        Ok(()) => AbiCallResult::OK,
        Err(error) => LAST_ERROR.with(|slot| {
            let mut message = slot.borrow_mut();
            *message = error.message;
            if message.len() > 4096 {
                let mut end = 4096;
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                message.truncate(end);
            }
            AbiCallResult {
                status: error.status,
                message: AbiByteSlice {
                    ptr: message.as_ptr(),
                    len: message.len(),
                },
            }
        }),
    }
}

fn live_object(
    interface: EngineInterface,
    id: u64,
) -> Result<GDExtensionObjectPtr, ModuleCallError> {
    let lookup = interface
        .object_get_instance_from_id
        .ok_or_else(|| failure(AbiStatus::Internal, "Godot object lookup is unavailable"))?;
    // SAFETY: ObjectDB accepts opaque instance IDs and returns null for stale IDs.
    let object = unsafe { lookup(id) };
    if object.is_null() {
        Err(failure(
            AbiStatus::StaleHandle,
            "Godot object no longer exists",
        ))
    } else {
        Ok(object)
    }
}

fn live_script(
    interface: EngineInterface,
    object: GDExtensionObjectPtr,
    uid: i64,
) -> Result<Arc<RustScriptInstance>, ModuleCallError> {
    let lookup = interface
        .object_get_script_instance
        .ok_or_else(|| failure(AbiStatus::Internal, "Godot script lookup is unavailable"))?;
    let language = crate::script::language_object();
    if language.is_null() {
        return Err(failure(
            AbiStatus::Unsupported,
            "Rust script language is not registered",
        ));
    }
    // SAFETY: The object was just resolved, and this is the registered Rust language.
    let pointer = unsafe { lookup(object, language) };
    let instance = data(pointer).ok_or_else(|| {
        failure(
            AbiStatus::InvalidArgument,
            "object has no attached Rust script",
        )
    })?;
    if !instance.script_uids.contains(&uid) {
        return Err(failure(
            AbiStatus::InvalidArgument,
            format!(
                "object script `{}` does not implement expected Rust script UID {uid}",
                instance.source_path,
            ),
        ));
    }
    Ok(instance)
}

pub(crate) unsafe extern "C" fn validate_object_from_module(
    _context: *mut c_void,
    receiver: u64,
    class_name: AbiByteSlice,
    script_uid: i64,
) -> AbiCallResult {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let interface = active_engine_interface().ok_or_else(|| {
            failure(
                AbiStatus::Unsupported,
                "object casts require a script callback or cooperative task",
            )
        })?;
        if class_name.ptr.is_null() || class_name.len == 0 || class_name.len > 1024 {
            return Err(failure(
                AbiStatus::InvalidArgument,
                "invalid object class contract",
            ));
        }
        // SAFETY: The SDK provides bounded static class text for this call.
        let bytes = unsafe { core::slice::from_raw_parts(class_name.ptr, class_name.len) };
        let name = core::str::from_utf8(bytes)
            .map_err(|_| failure(AbiStatus::InvalidArgument, "object class is not UTF-8"))?;
        let object = live_object(interface, receiver)?;
        let tag = crate::node_resolver::class_tag(interface, name)?;
        let cast = interface.object_cast_to.ok_or_else(|| {
            failure(
                AbiStatus::Internal,
                "object class validation is unavailable",
            )
        })?;
        // SAFETY: Both the live object and class tag belong to this Godot instance.
        if unsafe { cast(object, tag) }.is_null() {
            return Err(failure(
                AbiStatus::InvalidArgument,
                format!("object is not a `{name}`"),
            ));
        }
        if script_uid >= 0 {
            let _ = live_script(interface, object, script_uid)?;
        }
        Ok(())
    }))
    .unwrap_or_else(|_| Err(failure(AbiStatus::Panic, "object validation panicked")));
    finish(result)
}

pub(crate) unsafe extern "C" fn call_script_member_from_module(
    context: *mut c_void,
    receiver: u64,
    uid: i64,
    member: u64,
    operation: u8,
    arguments: *const AbiValueV1,
    count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    if context.is_null() || output.is_null() || count > 1024 || (count != 0 && arguments.is_null())
    {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "invalid typed script call buffers",
        );
    }
    // SAFETY: The caller supplies one writable output slot.
    unsafe { output.write(AbiValueV1::NIL) };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let interface = active_engine_interface().ok_or_else(|| {
            failure(
                AbiStatus::Unsupported,
                "typed calls require a script callback or cooperative task",
            )
        })?;
        let object = live_object(interface, receiver)?;
        let instance = live_script(interface, object, uid)?;
        with_typed_fault_guard(&instance.faults, || {
            let state = instance.state.as_ref().ok_or_else(|| {
                failure(AbiStatus::StaleHandle, "Rust script state no longer exists")
            })?;
            let mut state = state.try_lock().map_err(|error| match error {
                TryLockError::WouldBlock => failure(
                    AbiStatus::ReentrantCall,
                    format!(
                        "synchronous call into `{}` conflicts with its active Rust callback; use a deferred connection",
                        instance.source_path,
                    ),
                ),
                TryLockError::Poisoned(_) => {
                    failure(AbiStatus::Internal, "Rust script state is poisoned")
                }
            })?;
            // SAFETY: The live Host table retains this EngineCallContext.
            let context = unsafe { &*context.cast::<crate::engine_call::EngineCallContext>() };
            let value_owner = state.value_owner();
            if !ptr::eq(context, value_owner.engine_call_context()) {
                return Err(failure(
                    AbiStatus::StaleHandle,
                    "typed target belongs to another module generation",
                ));
            }
            let args = if count == 0 {
                &[]
            } else {
                // SAFETY: Input count and pointer were checked; SDK retains the slice.
                unsafe { core::slice::from_raw_parts(arguments, count as usize) }
            };
            let _scope = ActiveScriptScope::enter(&instance);
            let arguments = crate::engine_call::ScriptCallArguments::new(context, interface, args)
                .map_err(|error| failure(error.status(), error.message()))?;
            let args = arguments.as_slice();
            let value = match operation {
                ABI_SCRIPT_MEMBER_METHOD => {
                    let method = state.typed_method(uid, member).ok_or_else(|| {
                        failure(
                            AbiStatus::InvalidArgument,
                            "Rust script method is missing or incompatible",
                        )
                    })?;
                    state.call_method(&method, args).map_err(|error| {
                        failure(
                            error.status,
                            format!(
                                "{}::{}: {}",
                                instance.source_path,
                                method.name(),
                                error.message,
                            ),
                        )
                    })?
                }
                ABI_SCRIPT_MEMBER_GET_PROPERTY if args.is_empty() => {
                    let index = usize::try_from(member).map_err(|_| {
                        failure(AbiStatus::InvalidArgument, "invalid field index")
                    })?;
                    let field = state.typed_field(uid, index).ok_or_else(|| {
                        failure(AbiStatus::InvalidArgument, "Rust property is missing")
                    })?;
                    state.get_field(&field).map_err(|error| {
                        failure(
                            error.status,
                            format!(
                                "{}::{}: {}",
                                instance.source_path,
                                field.name(),
                                error.message,
                            ),
                        )
                    })?
                }
                ABI_SCRIPT_MEMBER_SET_PROPERTY if args.len() == 1 => {
                    let index = usize::try_from(member).map_err(|_| {
                        failure(AbiStatus::InvalidArgument, "invalid field index")
                    })?;
                    let field = state.typed_field(uid, index).ok_or_else(|| {
                        failure(AbiStatus::InvalidArgument, "Rust property is missing")
                    })?;
                    state.set_field(&field, args[0]).map_err(|error| {
                        failure(
                            error.status,
                            format!(
                                "{}::{}: {}",
                                instance.source_path,
                                field.name(),
                                error.message,
                            ),
                        )
                    })?;
                    return Ok(());
                }
                _ => {
                    return Err(failure(
                        AbiStatus::InvalidArgument,
                        "invalid typed script operation",
                    ));
                }
            };
            let returned = crate::engine_call::copy_script_result(context, interface, &value)
                .map_err(|error| failure(error.status(), error.message()))?;
            // SAFETY: Output is writable and ownership transfers to the caller's SDK.
            unsafe { output.write(returned) };
            Ok(())
        })
    }))
    .unwrap_or_else(|_| Err(failure(AbiStatus::Panic, "typed script dispatch panicked")));
    finish(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_panics_disable_subsequent_methods_and_property_access() {
        for unwind in [false, true] {
            let faults = InstanceFaults::default();
            let calls = Cell::new(0);
            let error = with_typed_fault_guard(&faults, || {
                calls.set(calls.get() + 1);
                if unwind {
                    panic!("typed dispatch failure");
                }
                Err(failure(AbiStatus::Panic, "script panic"))
            })
            .unwrap_err();
            assert_eq!(error.status, AbiStatus::Panic);
            assert!(faults.is_fused());
            for _ in 0..3 {
                let error = with_typed_fault_guard(&faults, || {
                    calls.set(calls.get() + 1);
                    Ok(())
                })
                .unwrap_err();
                assert_eq!(error.status, AbiStatus::CallbackFailed);
            }
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn recoverable_typed_errors_leave_callback_faults_unchanged() {
        let faults = InstanceFaults::default();
        assert_eq!(
            faults.record_failure(AbiStatus::CallbackFailed),
            FaultDisposition::Report
        );
        let previous = faults.state.load(Ordering::Acquire);
        for _ in 0..INSTANCE_FAILURE_LIMIT + 1 {
            let error = with_typed_fault_guard(&faults, || {
                Err(failure(AbiStatus::CallbackFailed, "not enough energy"))
            })
            .unwrap_err();
            assert_eq!(error.message, "not enough energy");
            assert_eq!(faults.state.load(Ordering::Acquire), previous);
        }
        assert!(with_typed_fault_guard(&faults, || Ok(())).is_ok());
        assert_eq!(faults.state.load(Ordering::Acquire), previous);

        faults.record_failure(AbiStatus::Panic);
        assert!(with_typed_fault_guard(&faults, || panic!("must not run")).is_err());
    }
}
