use core::cell::{Cell, RefCell};
use core::ptr;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use godot_api::abi::{
    AbiValueType, AbiValueV1, callable_value_ownership_token, validate_callable_value,
};

use super::dynamic_value::NativeVariant;
use super::runtime::Interface;
use super::sys;
use super::value::GodotStringName;
use crate::callable::Callable;
use crate::engine::{Object, ObjectRef};
use crate::error::{EngineError, EngineResult};

const CALLABLE_HEADER_BYTES: usize = 32;
const GET_OBJECT_ID_HASH: i64 = 3_173_160_232;
const GET_METHOD_HASH: i64 = 1_825_232_092;
const IS_NULL_HASH: i64 = 3_918_633_141;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    // Keep a trivial TLS slot and explicitly destroy its registry before the
    // library unloads. A droppable Rust TLS value can make glibc defer
    // `dlclose`, leaving a stale Native generation mapped after hot reload.
    static TOKENS: Cell<*mut TokenRegistry> = const { Cell::new(ptr::null_mut()) };
}

type TokenRegistry = RefCell<HashMap<u64, Weak<NativeCallableToken>>>;

fn with_tokens<R>(operation: impl FnOnce(&TokenRegistry) -> R) -> R {
    TOKENS.with(|slot| {
        let pointer = if slot.get().is_null() {
            let pointer = Box::into_raw(Box::new(TokenRegistry::new(HashMap::new())));
            slot.set(pointer);
            pointer
        } else {
            slot.get()
        };
        // SAFETY: Each pointer is allocated and accessed only by its owning
        // thread. `clear_thread_local_state` replaces the slot before freeing
        // it and only runs after Native callbacks at extension shutdown.
        operation(unsafe { &*pointer })
    })
}

pub(crate) fn clear_thread_local_state() {
    TOKENS.with(|slot| {
        let pointer = slot.replace(ptr::null_mut());
        if !pointer.is_null() {
            // SAFETY: Replacing the owning slot transfers the unique Box
            // allocation back to this shutdown path.
            drop(unsafe { Box::from_raw(pointer) });
        }
    });
}

#[repr(C, align(8))]
struct CallableStorage([u8; 16]);

pub(super) struct NativeCallable {
    interface: Interface,
    storage: CallableStorage,
    initialized: bool,
    destroy: sys::GDExtensionPtrDestructor,
    copy: sys::GDExtensionPtrConstructor,
    get_object_id: sys::GDExtensionPtrBuiltInMethod,
    get_method: sys::GDExtensionPtrBuiltInMethod,
    is_null: sys::GDExtensionPtrBuiltInMethod,
}

/// Process-local ownership for an exact Native Callable, including custom and
/// bound callables that cannot be reconstructed from object/method metadata.
pub(crate) struct NativeCallableToken {
    id: u64,
    value: NativeCallable,
}

impl NativeCallableToken {
    fn retain(value: NativeCallable) -> Rc<Self> {
        let id = loop {
            let id = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                break id;
            }
        };
        let value = Rc::new(Self { id, value });
        with_tokens(|tokens| {
            let mut tokens = tokens.borrow_mut();
            tokens.retain(|_, value| value.strong_count() != 0);
            tokens.insert(id, Rc::downgrade(&value));
        });
        value
    }

    pub(crate) const fn id(&self) -> u64 {
        self.id
    }

    fn copy_value(&self) -> EngineResult<NativeCallable> {
        self.value.copy_value()
    }
}

pub(crate) fn retain_rust_callable(token: u64) -> EngineResult<Callable> {
    let ownership = with_tokens(|tokens| tokens.borrow().get(&token).and_then(Weak::upgrade))
        .ok_or_else(|| {
            EngineError::stale_object(format!(
                "Native Callable ownership token {token} is no longer live"
            ))
        })?;
    let object_id = ownership.value.object_id()?;
    let method = ownership.value.method()?;
    Ok(Callable::__from_native_parts(
        ObjectRef::<Object>::__from_instance_id(object_id),
        method,
        ownership,
    ))
}

impl Drop for NativeCallableToken {
    fn drop(&mut self) {
        with_tokens(|tokens| {
            tokens.borrow_mut().remove(&self.id);
        });
    }
}

impl NativeCallable {
    pub(super) fn empty(interface: Interface) -> EngineResult<Self> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 0)?;
        // SAFETY: Storage is aligned and the default constructor has no args.
        unsafe { constructor(value.as_mut_ptr(), ptr::null()) };
        value.initialized = true;
        Ok(value)
    }

    pub(super) fn from_argument(interface: Interface, value: AbiValueV1) -> EngineResult<Self> {
        if value.type_ != AbiValueType::CALLABLE || value.reserved_flags != 0 {
            return Err(EngineError::invalid_argument(
                "Native Callable argument violates its generated contract",
            ));
        }
        let (pointer, length) = value.byte_range(AbiValueType::CALLABLE).ok_or_else(|| {
            EngineError::invalid_argument("Native Callable argument has an invalid range")
        })?;
        // SAFETY: The generated wrapper retains this bounded range for the
        // synchronous call.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        if !validate_callable_value(bytes) {
            return Err(EngineError::invalid_argument(
                "Native Callable argument has an invalid ABI payload",
            ));
        }
        if let Some(token) = callable_value_ownership_token(bytes) {
            return with_tokens(|tokens| tokens.borrow().get(&token).and_then(Weak::upgrade))
                .ok_or_else(|| {
                    EngineError::stale_object(format!(
                        "Native Callable ownership token {token} is no longer live"
                    ))
                })?
                .copy_value();
        }
        let object_id = u64::from_le_bytes(
            bytes[20..28]
                .try_into()
                .expect("validated Callable object ID"),
        );
        let object = if object_id == 0 {
            ptr::null_mut()
        } else {
            // SAFETY: Godot owns and synchronizes its instance-ID table.
            let object = unsafe { (interface.object_get_instance_from_id)(object_id) };
            if object.is_null() {
                return Err(EngineError::stale_object(format!(
                    "Godot Callable target {object_id} no longer exists"
                )));
            }
            object
        };
        let method = core::str::from_utf8(&bytes[CALLABLE_HEADER_BYTES..])
            .expect("validated Callable UTF-8");
        Self::from_standard(interface, object, method)
    }

    fn from_standard(
        interface: Interface,
        object: sys::GDExtensionObjectPtr,
        method: &str,
    ) -> EngineResult<Self> {
        if object.is_null() && method.is_empty() {
            return Self::empty(interface);
        }
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 2)?;
        let method = GodotStringName::new(&interface, method)
            .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
        let arguments = [ptr::from_ref(&object).cast(), method.as_ptr().cast()];
        // SAFETY: Constructor and argument layouts are authenticated Callable
        // metadata.
        unsafe { constructor(value.as_mut_ptr(), arguments.as_ptr()) };
        value.initialized = true;
        Ok(value)
    }

    pub(super) fn from_variant(value: &NativeVariant) -> EngineResult<Self> {
        let mut callable = Self::uninitialized(value.interface())?;
        value.to_raw_value(callable_type(), callable.as_mut_ptr())?;
        callable.initialized = true;
        Ok(callable)
    }

    pub(super) fn to_variant(&self) -> EngineResult<NativeVariant> {
        NativeVariant::from_raw(self.interface, callable_type(), self.as_const_ptr())
    }

    pub(super) fn into_rust(self) -> EngineResult<Callable> {
        if self.is_null()? {
            return Ok(Callable::null());
        }
        let object_id = self.object_id()?;
        let method = self.method()?;
        let ownership = NativeCallableToken::retain(self);
        Ok(Callable::__from_native_parts(
            ObjectRef::<Object>::__from_instance_id(object_id),
            method,
            ownership,
        ))
    }

    fn copy_value(&self) -> EngineResult<Self> {
        let mut value = Self::uninitialized(self.interface)?;
        let copy = self.copy.ok_or_else(|| {
            EngineError::unavailable("Native Callable copy constructor is unavailable")
        })?;
        let arguments = [self.as_const_ptr()];
        // SAFETY: Source is live and destination is uninitialized exact
        // Callable storage.
        unsafe { copy(value.as_mut_ptr(), arguments.as_ptr()) };
        value.initialized = true;
        Ok(value)
    }

    fn object_id(&self) -> EngineResult<u64> {
        let get = self.get_object_id.ok_or_else(|| {
            EngineError::unavailable("Native Callable.get_object_id is unavailable")
        })?;
        let mut object_id = 0_i64;
        // SAFETY: Method takes no arguments and writes one int64.
        unsafe {
            get(
                self.as_const_ptr().cast_mut(),
                ptr::null(),
                ptr::from_mut(&mut object_id).cast(),
                0,
            );
        }
        Ok(object_id as u64)
    }

    fn method(&self) -> EngineResult<String> {
        let get = self
            .get_method
            .ok_or_else(|| EngineError::unavailable("Native Callable.get_method is unavailable"))?;
        let mut method = super::engine_call::NativeTextValue::empty(
            self.interface,
            godot_api::abi::AbiPtrcallType::STRING_NAME,
        )?;
        // SAFETY: Method takes no arguments and writes one StringName.
        unsafe {
            get(
                self.as_const_ptr().cast_mut(),
                ptr::null(),
                method.as_mut_ptr(),
                0,
            );
        }
        method.mark_initialized();
        method.to_rust_string()
    }

    fn is_null(&self) -> EngineResult<bool> {
        let method = self
            .is_null
            .ok_or_else(|| EngineError::unavailable("Native Callable.is_null is unavailable"))?;
        let mut value = 0_u8;
        // SAFETY: Method takes no arguments and writes one bool.
        unsafe {
            method(
                self.as_const_ptr().cast_mut(),
                ptr::null(),
                ptr::from_mut(&mut value).cast(),
                0,
            );
        }
        match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(EngineError::invalid_result(
                "Godot returned a non-canonical Callable.is_null result",
            )),
        }
    }

    pub(super) fn as_const_ptr(&self) -> sys::GDExtensionConstTypePtr {
        self.storage.0.as_ptr().cast()
    }

    pub(super) fn as_mut_ptr(&mut self) -> sys::GDExtensionTypePtr {
        self.storage.0.as_mut_ptr().cast()
    }

    pub(super) fn uninitialized(interface: Interface) -> EngineResult<Self> {
        Ok(Self {
            interface,
            storage: CallableStorage([0; 16]),
            initialized: false,
            destroy: destructor(interface)?,
            copy: Some(constructor(interface, 1)?),
            get_object_id: builtin_method(interface, "get_object_id", GET_OBJECT_ID_HASH)?,
            get_method: builtin_method(interface, "get_method", GET_METHOD_HASH)?,
            is_null: builtin_method(interface, "is_null", IS_NULL_HASH)?,
        })
    }

    pub(super) fn mark_initialized(&mut self) {
        self.initialized = true;
    }
}

impl Drop for NativeCallable {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.destroy {
                // SAFETY: This wrapper owns one initialized Callable.
                unsafe { destroy(self.as_mut_ptr()) };
            }
        }
    }
}

fn constructor(
    interface: Interface,
    index: i32,
) -> EngineResult<
    unsafe extern "C" fn(sys::GDExtensionUninitializedTypePtr, *const sys::GDExtensionConstTypePtr),
> {
    // SAFETY: Type and constructor index come from authenticated metadata.
    let constructor = unsafe { (interface.variant_get_ptr_constructor)(callable_type(), index) };
    constructor
        .ok_or_else(|| EngineError::unavailable("Native Callable constructor is unavailable"))
}

fn destructor(interface: Interface) -> EngineResult<sys::GDExtensionPtrDestructor> {
    // SAFETY: Callable is an official owned builtin type.
    let destroy = unsafe { (interface.variant_get_ptr_destructor)(callable_type()) };
    if destroy.is_none() {
        return Err(EngineError::unavailable(
            "Native Callable destructor is unavailable",
        ));
    }
    Ok(destroy)
}

fn builtin_method(
    interface: Interface,
    name: &str,
    hash: i64,
) -> EngineResult<sys::GDExtensionPtrBuiltInMethod> {
    let name = GodotStringName::new(&interface, name)
        .map_err(|error| EngineError::invalid_result(error.to_string()))?;
    // SAFETY: Type, method and hash are authenticated Callable metadata.
    let method =
        unsafe { (interface.variant_get_ptr_builtin_method)(callable_type(), name.as_ptr(), hash) };
    if method.is_none() {
        return Err(EngineError::unavailable(
            "Native Callable builtin method is unavailable",
        ));
    }
    Ok(method)
}

const fn callable_type() -> sys::GDExtensionVariantType {
    sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callable_wire_and_builtin_metadata_match_the_stable_abi() {
        assert_eq!(CALLABLE_HEADER_BYTES, 32);
        assert_eq!(GET_OBJECT_ID_HASH, 3_173_160_232);
        assert_eq!(GET_METHOD_HASH, 1_825_232_092);
        assert_eq!(IS_NULL_HASH, 3_918_633_141);
    }
}
