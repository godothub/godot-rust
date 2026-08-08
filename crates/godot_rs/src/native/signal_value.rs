use core::ptr;

use godot_rs_api::abi::{AbiValueType, AbiValueV1, validate_signal_value};

use super::dynamic_value::NativeVariant;
use super::runtime::Interface;
use super::sys;
use super::value::GodotStringName;
use crate::error::{EngineError, EngineResult};

const SIGNAL_HEADER_BYTES: usize = 24;
const GET_OBJECT_ID_HASH: i64 = 3_173_160_232;
const GET_NAME_HASH: i64 = 1_825_232_092;

#[repr(C, align(8))]
struct SignalStorage([u8; 16]);

pub(super) struct NativeSignal {
    interface: Interface,
    storage: SignalStorage,
    initialized: bool,
    destroy: sys::GDExtensionPtrDestructor,
    get_object_id: sys::GDExtensionPtrBuiltInMethod,
    get_name: sys::GDExtensionPtrBuiltInMethod,
}

impl NativeSignal {
    pub(super) fn empty(interface: Interface) -> EngineResult<Self> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 0)?;
        // SAFETY: Storage is aligned and the default constructor has no args.
        unsafe { constructor(value.as_mut_ptr(), ptr::null()) };
        value.initialized = true;
        Ok(value)
    }

    pub(super) fn from_argument(interface: Interface, value: AbiValueV1) -> EngineResult<Self> {
        if value.type_ != AbiValueType::SIGNAL || value.reserved_flags != 0 {
            return Err(EngineError::invalid_argument(
                "Native Signal argument violates its generated contract",
            ));
        }
        let (pointer, length) = value
            .byte_range(AbiValueType::SIGNAL)
            .ok_or_else(|| EngineError::invalid_argument("Native Signal has an invalid range"))?;
        // SAFETY: The generated wrapper retains this bounded range for the
        // synchronous call.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        if !validate_signal_value(bytes) {
            return Err(EngineError::invalid_argument(
                "Native Signal has an invalid ABI payload",
            ));
        }
        let object_id = u64::from_le_bytes(
            bytes[12..20]
                .try_into()
                .expect("validated Signal object ID"),
        );
        if object_id == 0 {
            return Self::empty(interface);
        }
        // SAFETY: Godot owns and synchronizes its instance-ID table.
        let object = unsafe { (interface.object_get_instance_from_id)(object_id) };
        if object.is_null() {
            return Err(EngineError::stale_object(format!(
                "Godot Signal target {object_id} no longer exists"
            )));
        }
        let name =
            core::str::from_utf8(&bytes[SIGNAL_HEADER_BYTES..]).expect("validated Signal UTF-8");
        Self::from_standard(interface, object, name)
    }

    fn from_standard(
        interface: Interface,
        object: sys::GDExtensionObjectPtr,
        name: &str,
    ) -> EngineResult<Self> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 2)?;
        let name = GodotStringName::new(&interface, name)
            .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
        let arguments = [ptr::from_ref(&object).cast(), name.as_ptr().cast()];
        // SAFETY: Constructor index and argument layouts come from the
        // authenticated Signal API.
        unsafe { constructor(value.as_mut_ptr(), arguments.as_ptr()) };
        value.initialized = true;
        Ok(value)
    }

    pub(super) fn from_variant(value: &NativeVariant) -> EngineResult<Self> {
        let mut signal = Self::uninitialized(value.interface())?;
        value.to_raw_value(signal_type(), signal.as_mut_ptr())?;
        signal.initialized = true;
        Ok(signal)
    }

    pub(super) fn to_variant(&self) -> EngineResult<NativeVariant> {
        NativeVariant::from_raw(self.interface, signal_type(), self.as_const_ptr())
    }

    pub(super) fn into_abi(self) -> EngineResult<AbiValueV1> {
        self.to_bytes()
            .map(|bytes| crate::module::owned_bytes(AbiValueType::SIGNAL, bytes))
    }

    pub(super) fn to_bytes(&self) -> EngineResult<Vec<u8>> {
        let object_id = self.object_id()?;
        let name = self.name()?;
        let name_length = u32::try_from(name.len())
            .map_err(|_| EngineError::invalid_result("Native Signal name is too large"))?;
        let mut bytes = Vec::with_capacity(SIGNAL_HEADER_BYTES + name.len());
        bytes.extend_from_slice(&godot_rs_api::abi::ABI_SIGNAL_MAGIC);
        bytes.extend_from_slice(&godot_rs_api::abi::ABI_SIGNAL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&object_id.to_le_bytes());
        bytes.extend_from_slice(&name_length.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        if !validate_signal_value(&bytes) {
            return Err(EngineError::invalid_result(
                "Godot returned an invalid Native Signal",
            ));
        }
        Ok(bytes)
    }

    fn object_id(&self) -> EngineResult<u64> {
        let get = self.get_object_id.ok_or_else(|| {
            EngineError::unavailable("Native Signal.get_object_id is unavailable")
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

    fn name(&self) -> EngineResult<String> {
        let get = self
            .get_name
            .ok_or_else(|| EngineError::unavailable("Native Signal.get_name is unavailable"))?;
        let mut name = super::engine_call::NativeTextValue::empty(
            self.interface,
            godot_rs_api::abi::AbiPtrcallType::STRING_NAME,
        )?;
        // SAFETY: Method takes no arguments and writes one StringName.
        unsafe {
            get(
                self.as_const_ptr().cast_mut(),
                ptr::null(),
                name.as_mut_ptr(),
                0,
            );
        }
        name.mark_initialized();
        name.to_rust_string()
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
            storage: SignalStorage([0; 16]),
            initialized: false,
            destroy: destructor(interface)?,
            get_object_id: builtin_method(interface, "get_object_id", GET_OBJECT_ID_HASH)?,
            get_name: builtin_method(interface, "get_name", GET_NAME_HASH)?,
        })
    }

    pub(super) fn mark_initialized(&mut self) {
        self.initialized = true;
    }
}

impl Drop for NativeSignal {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.destroy {
                // SAFETY: This wrapper owns one initialized Signal.
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
    let constructor = unsafe { (interface.variant_get_ptr_constructor)(signal_type(), index) };
    constructor.ok_or_else(|| EngineError::unavailable("Native Signal constructor is unavailable"))
}

fn destructor(interface: Interface) -> EngineResult<sys::GDExtensionPtrDestructor> {
    // SAFETY: Signal is an official owned builtin type.
    let destroy = unsafe { (interface.variant_get_ptr_destructor)(signal_type()) };
    if destroy.is_none() {
        return Err(EngineError::unavailable(
            "Native Signal destructor is unavailable",
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
    // SAFETY: Type, method and hash are authenticated Signal metadata.
    let method =
        unsafe { (interface.variant_get_ptr_builtin_method)(signal_type(), name.as_ptr(), hash) };
    if method.is_none() {
        return Err(EngineError::unavailable(
            "Native Signal builtin method is unavailable",
        ));
    }
    Ok(method)
}

const fn signal_type() -> sys::GDExtensionVariantType {
    sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_wire_offsets_match_the_stable_abi() {
        assert_eq!(SIGNAL_HEADER_BYTES, 24);
        assert_eq!(GET_OBJECT_ID_HASH, 3_173_160_232);
        assert_eq!(GET_NAME_HASH, 1_825_232_092);
    }
}
