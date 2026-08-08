//! Owned Godot Callable handles.

use core::cell::OnceCell;
use core::fmt;
use std::rc::Rc;

use godot_rs_api::abi::{
    ABI_CALLABLE_MAGIC, ABI_CALLABLE_OWNED, ABI_CALLABLE_VERSION, callable_value_ownership_token,
    validate_callable_value,
};

use crate::engine::{GodotClass, GodotRef, Object, ObjectRef, SharedGodotRefOwnership};
use crate::string_name::StringName;

const CALLABLE_HEADER_BYTES: usize = 32;

/// A Godot method target that can be passed through Script Mode safely.
///
/// Standard callables can be created from an object and method name. Callables
/// received from Godot also retain their exact native form, including bound
/// arguments and custom Callable implementations, until the final Rust clone
/// is dropped.
pub struct Callable {
    object: ObjectRef<Object>,
    method: StringName,
    host_token: u64,
    host_ownership: Option<Rc<crate::module::HostCallableValueToken>>,
    native_ownership: Option<Rc<crate::native::NativeCallableToken>>,
    target_ownership: Option<SharedGodotRefOwnership>,
    encoded: OnceCell<Result<Box<[u8]>, CallableError>>,
}

/// Failure while encoding a Callable for the Host ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallableError {
    message: &'static str,
}

impl CallableError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn message(self) -> &'static str {
        self.message
    }
}

impl fmt::Display for CallableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for CallableError {}

impl Callable {
    /// Creates a null Callable.
    #[must_use]
    pub fn null() -> Self {
        Self::from_parts(
            ObjectRef::unresolved(),
            StringName::default(),
            0,
            None,
            None,
            None,
        )
    }

    /// Creates a standard Callable targeting one Godot object method.
    #[must_use]
    pub fn from_object_method<T: GodotClass>(
        object: ObjectRef<T>,
        method: impl Into<StringName>,
    ) -> Self {
        Self::from_parts(
            ObjectRef::__from_instance_id(object.instance_id()),
            method.into(),
            0,
            None,
            None,
            None,
        )
    }

    /// Creates a standard Callable and keeps a RefCounted target alive.
    #[must_use]
    pub fn from_godot_ref_method<T: GodotClass>(
        object: &GodotRef<T>,
        method: impl Into<StringName>,
    ) -> Self {
        Self::from_parts(
            ObjectRef::__from_instance_id(object.instance_id()),
            method.into(),
            0,
            None,
            None,
            Some(object.dynamic_ownership()),
        )
    }

    /// Returns the target object identity reported by Godot.
    #[must_use]
    pub const fn object(&self) -> ObjectRef<Object> {
        self.object
    }

    /// Returns the target method spelling reported by Godot.
    #[must_use]
    pub fn method(&self) -> &StringName {
        &self.method
    }

    /// Whether this is the null standard Callable.
    #[must_use]
    pub fn is_null(&self) -> bool {
        self.host_token == 0
            && self.native_ownership.is_none()
            && !self.object.is_resolved()
            && self.method.is_empty()
    }

    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], CallableError> {
        self.encoded
            .get_or_init(|| encode(self).map(Vec::into_boxed_slice))
            .as_deref()
            .map_err(|error| *error)
    }

    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        (callable_value_ownership_token(bytes).is_none() && validate_callable_value(bytes))
            .then(|| decode(bytes, None))
            .flatten()
    }

    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostCallableValueToken>,
    ) -> Option<Self> {
        if !validate_callable_value(bytes)
            || callable_value_ownership_token(bytes).is_some() != ownership.is_some()
        {
            return None;
        }
        decode(bytes, ownership.map(Rc::new))
    }

    pub(crate) const fn __host_token(&self) -> u64 {
        self.host_token
    }

    pub(crate) fn __from_native_parts(
        object: ObjectRef<Object>,
        method: String,
        ownership: Rc<crate::native::NativeCallableToken>,
    ) -> Self {
        Self::from_parts(
            object,
            StringName::from(method),
            0,
            None,
            Some(ownership),
            None,
        )
    }

    fn from_parts(
        object: ObjectRef<Object>,
        method: StringName,
        host_token: u64,
        host_ownership: Option<Rc<crate::module::HostCallableValueToken>>,
        native_ownership: Option<Rc<crate::native::NativeCallableToken>>,
        target_ownership: Option<SharedGodotRefOwnership>,
    ) -> Self {
        Self {
            object,
            method,
            host_token,
            host_ownership,
            native_ownership,
            target_ownership,
            encoded: OnceCell::new(),
        }
    }
}

impl Clone for Callable {
    fn clone(&self) -> Self {
        Self::from_parts(
            self.object,
            self.method.clone(),
            self.host_token,
            self.host_ownership.as_ref().map(Rc::clone),
            self.native_ownership.as_ref().map(Rc::clone),
            self.target_ownership.as_ref().map(Rc::clone),
        )
    }
}

impl Default for Callable {
    fn default() -> Self {
        Self::null()
    }
}

impl PartialEq for Callable {
    fn eq(&self, other: &Self) -> bool {
        self.object == other.object
            && self.method == other.method
            && self.host_token == other.host_token
            && self.native_ownership.as_ref().map(|value| value.id())
                == other.native_ownership.as_ref().map(|value| value.id())
    }
}

impl Eq for Callable {}

impl fmt::Debug for Callable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Callable")
            .field("object", &self.object.instance_id())
            .field("method", &self.method)
            .field("host_owned", &(self.host_token != 0))
            .field("native_owned", &self.native_ownership.is_some())
            .finish()
    }
}

fn encode(value: &Callable) -> Result<Vec<u8>, CallableError> {
    let method = value.method.as_str().as_bytes();
    if method.contains(&0) {
        return Err(CallableError::new(
            "Callable method names cannot contain a nul byte",
        ));
    }
    let method_length = u32::try_from(method.len())
        .map_err(|_| CallableError::new("Callable method name exceeds the ABI limit"))?;
    let mut output = Vec::with_capacity(CALLABLE_HEADER_BYTES + method.len());
    output.extend_from_slice(&ABI_CALLABLE_MAGIC);
    output.extend_from_slice(&ABI_CALLABLE_VERSION.to_le_bytes());
    let ownership_token = value
        .native_ownership
        .as_ref()
        .map_or(value.host_token, |value| value.id());
    output.extend_from_slice(
        &if ownership_token == 0 {
            0_u16
        } else {
            ABI_CALLABLE_OWNED
        }
        .to_le_bytes(),
    );
    output.extend_from_slice(&ownership_token.to_le_bytes());
    output.extend_from_slice(&value.object.instance_id().to_le_bytes());
    output.extend_from_slice(&method_length.to_le_bytes());
    output.extend_from_slice(method);
    if !validate_callable_value(&output) {
        return Err(CallableError::new(
            "Callable could not be encoded for the Host ABI",
        ));
    }
    Ok(output)
}

fn decode(
    bytes: &[u8],
    ownership: Option<Rc<crate::module::HostCallableValueToken>>,
) -> Option<Callable> {
    if !validate_callable_value(bytes) {
        return None;
    }
    let token = callable_value_ownership_token(bytes).unwrap_or(0);
    let object = u64::from_le_bytes(bytes.get(20..28)?.try_into().ok()?);
    let method = core::str::from_utf8(bytes.get(CALLABLE_HEADER_BYTES..)?).ok()?;
    Some(Callable::from_parts(
        ObjectRef::__from_instance_id(object),
        StringName::from(method),
        token,
        ownership,
        None,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Node;

    #[test]
    fn standard_callables_round_trip_without_process_local_state() {
        let callable =
            Callable::from_object_method(ObjectRef::<Node>::__from_instance_id(42), "_ready");
        let bytes = callable.__bytes().expect("Callable wire");
        let restored = Callable::__from_bytes(bytes).expect("standard Callable");
        assert_eq!(restored.object().instance_id(), 42);
        assert_eq!(restored.method().as_str(), "_ready");
        assert!(!restored.is_null());
        assert!(Callable::null().is_null());
    }
}
