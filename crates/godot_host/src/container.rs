//! Main-thread, Host-owned identities shared by every project-module generation.

use core::slice::from_raw_parts;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use godot_api::abi::*;

use crate::dynamic_value::NativeDynamic;
use crate::engine_call::value::ValueError;
use crate::engine_call::{EngineCallContext, own_returned_dynamic, own_returned_dynamic_snapshot};
use crate::variant_codec::{OwnedVariant, VariantCodec};

struct Entry {
    kind: AbiValueType,
    references: usize,
    value: Rc<OwnedVariant>,
}

thread_local! {
    static VALUES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Leases(pub(crate) Vec<u64>);

impl Drop for Leases {
    fn drop(&mut self) {
        for token in self.0.drain(..) {
            let _ = release(token);
        }
    }
}

fn invalid(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::StaleHandle, message)
}

pub(crate) fn insert(kind: AbiValueType, value: OwnedVariant) -> Result<u64, ValueError> {
    let token = NEXT_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| invalid("shared-container identity space is exhausted"))?;
    VALUES.with(|values| {
        let mut values = values.borrow_mut();
        if values.len() >= 65_536 {
            return Err(invalid("shared-container ownership limit is exceeded"));
        }
        values.insert(
            token,
            Entry {
                kind,
                references: 1,
                value: Rc::new(value),
            },
        );
        Ok(token)
    })
}

pub(crate) fn get(kind: AbiValueType, token: u64) -> Result<Rc<OwnedVariant>, ValueError> {
    VALUES.with(|values| {
        let values = values.borrow();
        let entry = values
            .get(&token)
            .filter(|entry| entry.kind == kind)
            .ok_or_else(|| invalid("shared-container token is stale or has the wrong kind"))?;
        Ok(Rc::clone(&entry.value))
    })
}

pub(crate) fn retain(kind: AbiValueType, token: u64) -> Result<(), ValueError> {
    VALUES.with(|values| {
        let mut values = values.borrow_mut();
        let entry = values
            .get_mut(&token)
            .filter(|entry| entry.kind == kind)
            .ok_or_else(|| invalid("shared-container token is stale or has the wrong kind"))?;
        entry.references = entry
            .references
            .checked_add(1)
            .ok_or_else(|| invalid("shared-container reference count overflowed"))?;
        Ok(())
    })
}

pub(crate) fn release(token: u64) -> Result<(), ValueError> {
    let retired = VALUES.with(|values| {
        let mut values = values.borrow_mut();
        let entry = values
            .get_mut(&token)
            .ok_or_else(|| invalid("shared-container token has already been released"))?;
        entry.references -= 1;
        Ok(if entry.references == 0 {
            values.remove(&token)
        } else {
            None
        })
    })?;
    // Godot destructors can reenter this registry.
    drop(retired);
    Ok(())
}

pub(crate) fn clear() {
    let retired = VALUES.with(|values| core::mem::take(&mut *values.borrow_mut()));
    drop(retired);
}

/// The SDK keeps all pointers and byte ranges readable for this synchronous call.
pub(crate) unsafe extern "C" fn access(
    context: *mut core::ffi::c_void,
    operation: u32,
    kind: AbiValueType,
    token: u64,
    bytes: AbiByteSlice,
    element: AbiByteSlice,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if output.is_null() || !matches!(kind, AbiValueType::ARRAY | AbiValueType::DICTIONARY) {
            return Err(invalid("invalid shared-container ABI request"));
        }
        let value = match operation {
            ABI_CONTAINER_RETAIN => {
                retain(kind, token)?;
                AbiValueV1::NIL
            }
            ABI_CONTAINER_RELEASE => {
                get(kind, token)?;
                release(token)?;
                AbiValueV1::NIL
            }
            _ => {
                let interface =
                    crate::script_instance::active_engine_interface().ok_or_else(|| {
                        ValueError::new(
                            AbiStatus::Unsupported,
                            "shared-container access requires a Godot callback",
                        )
                    })?;
                let context = if context.is_null() {
                    return Err(invalid("shared-container Host context is unavailable"));
                } else {
                    // SAFETY: The ABI table retains this exact context.
                    unsafe { &*context.cast::<EngineCallContext>() }
                };
                let codec = VariantCodec::new(interface)
                    .ok_or_else(|| invalid("shared-container Variant codec is unavailable"))?;
                match operation {
                    ABI_CONTAINER_MATERIALIZE | ABI_CONTAINER_ASSIGN => {
                        if bytes.len > ABI_DYNAMIC_MAX_BYTES
                            || bytes.ptr.is_null()
                            || element.len > 1024
                            || (element.len != 0 && element.ptr.is_null())
                        {
                            return Err(invalid("invalid shared-container input range"));
                        }
                        let element = if element.len == 0 {
                            None
                        } else {
                            // SAFETY: Null and oversized ranges were rejected above;
                            // the SDK keeps these bytes readable for this synchronous call.
                            let element_bytes = unsafe { from_raw_parts(element.ptr, element.len) };
                            Some(
                                core::str::from_utf8(element_bytes).map_err(|_| {
                                    invalid("invalid shared-container element type")
                                })?,
                            )
                        };
                        let abi = AbiValueV1 {
                            type_: kind,
                            reserved_flags: 0,
                            payload: [bytes.ptr as usize as u64, bytes.len as u64],
                        };
                        let source =
                            NativeDynamic::from_abi(interface, kind, abi, element, Some(context))?;
                        if operation == ABI_CONTAINER_ASSIGN {
                            let target = get(kind, token)?;
                            NativeDynamic::assign_shared(&codec, kind, &target, &source)?;
                            AbiValueV1::NIL
                        } else {
                            own_returned_dynamic(context, kind, source)?
                        }
                    }
                    ABI_CONTAINER_SNAPSHOT => {
                        let value = get(kind, token)?;
                        let native = NativeDynamic::from_shared_variant(&codec, kind, &value)?;
                        own_returned_dynamic_snapshot(context, kind, native)?
                    }
                    _ => return Err(invalid("unsupported shared-container operation")),
                }
            }
        };
        // SAFETY: The caller retains one writable output slot.
        unsafe { output.write(value) };
        Ok(())
    }));
    match outcome {
        Ok(Ok(())) => AbiCallResult::OK,
        Ok(Err(error)) => AbiCallResult::failure(error.status(), error.message()),
        Err(_) => AbiCallResult::failure(AbiStatus::Panic, "shared-container operation panicked"),
    }
}
