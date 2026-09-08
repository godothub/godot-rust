use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use godot_api::abi::{
    ABI_SCRIPT_MEMBER_GET_PROPERTY, ABI_SCRIPT_MEMBER_METHOD, ABI_SCRIPT_MEMBER_SET_PROPERTY,
    AbiValueV1,
};

use crate::engine::{EngineReturn, ObjectRef};
use crate::error::{EngineError, EngineResult};

use super::ScriptClass;

/// Representation of a generated script method proxy.
///
/// # Safety
///
/// Implementors must be `#[repr(transparent)]` wrappers around exactly one
/// `ObjectRef<T>`, with no additional validity requirements. Proxy methods
/// must validate the target through the script bridge before using its state.
#[doc(hidden)]
pub unsafe trait ScriptApi<T: ScriptClass> {}

static SCRIPT_UIDS: OnceLock<Mutex<HashMap<&'static str, i64>>> = OnceLock::new();

pub(crate) fn clear_script_registry() {
    if let Some(registry) = SCRIPT_UIDS.get() {
        let mut identities = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = core::mem::take(&mut *identities);
    }
}

pub(super) fn register_script_uid(identity: &'static str, uid: i64) {
    SCRIPT_UIDS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("script identity registry poisoned")
        .insert(identity, uid);
}

#[doc(hidden)]
pub fn __registered_script_uid(identity: &'static str) -> EngineResult<i64> {
    SCRIPT_UIDS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| EngineError::invalid_result("script identity registry poisoned"))?
        .get(identity)
        .copied()
        .ok_or_else(|| {
            EngineError::invalid_argument(format!(
                "Rust script `{identity}` is not registered in this project module"
            ))
        })
}

#[doc(hidden)]
pub fn __call_script_method<T: ScriptClass, R: EngineReturn>(
    object: ObjectRef<T>,
    method: u64,
    arguments: &[AbiValueV1],
) -> EngineResult<R> {
    call_member(object, method, ABI_SCRIPT_MEMBER_METHOD, arguments)
}

#[doc(hidden)]
pub fn __get_script_property<T: ScriptClass, R: EngineReturn>(
    object: ObjectRef<T>,
    field: u32,
) -> EngineResult<R> {
    call_member(
        object,
        u64::from(field),
        ABI_SCRIPT_MEMBER_GET_PROPERTY,
        &[],
    )
}

#[doc(hidden)]
pub fn __set_script_property<T: ScriptClass>(
    object: ObjectRef<T>,
    field: u32,
    value: AbiValueV1,
) -> EngineResult<()> {
    call_member(
        object,
        u64::from(field),
        ABI_SCRIPT_MEMBER_SET_PROPERTY,
        &[value],
    )
}

fn call_member<T: ScriptClass, R: EngineReturn>(
    object: ObjectRef<T>,
    member: u64,
    operation: u8,
    arguments: &[AbiValueV1],
) -> EngineResult<R> {
    let uid = T::__script_resource_uid()?.ok_or_else(|| {
        EngineError::invalid_result("typed script reference has no registered script identity")
    })?;
    let value =
        crate::module::call_script_member(object.instance_id(), uid, member, operation, arguments)?;
    R::__from_host_return(value)
}
