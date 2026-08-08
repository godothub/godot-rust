use core::ptr;

use godot_rs_api::abi::{
    ABI_SIGNAL_MAGIC, ABI_SIGNAL_VERSION, ABI_VALUE_OWNED_BYTES, AbiStatus, AbiValueType,
    AbiValueV1, validate_signal_value,
};
use godot_rs_api::{
    GDExtensionConstTypePtr, GDExtensionConstVariantPtr, GDExtensionObjectPtr, GDExtensionTypePtr,
    GDExtensionVariantType,
};

use crate::engine_call::value::ValueError;
use crate::interface::EngineInterface;
use crate::string_name::OwnedStringName;
use crate::variant_codec::{OwnedVariant, VariantCodec};

const SIGNAL_HEADER_BYTES: usize = 24;
const GET_OBJECT_ID_HASH: i64 = 3_173_160_232;
const GET_NAME_HASH: i64 = 1_825_232_092;
const CONNECT_HASH: i64 = 979_702_392;
const DISCONNECT_HASH: i64 = 3_470_848_906;

#[repr(C, align(8))]
struct SignalStorage([u8; 16]);

type SignalConstructor = unsafe extern "C" fn(GDExtensionTypePtr, *const GDExtensionConstTypePtr);
type SignalDestructor = unsafe extern "C" fn(GDExtensionTypePtr);
type SignalBuiltinMethod = unsafe extern "C" fn(
    GDExtensionTypePtr,
    *const GDExtensionConstTypePtr,
    GDExtensionTypePtr,
    i32,
);

/// One initialized native Godot Signal.
pub(crate) struct NativeSignal {
    interface: EngineInterface,
    storage: SignalStorage,
    initialized: bool,
    destroy: SignalDestructor,
    get_object_id: SignalBuiltinMethod,
    get_name: SignalBuiltinMethod,
}

impl NativeSignal {
    pub(crate) fn empty(interface: EngineInterface) -> Result<Self, ValueError> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 0)?;
        // SAFETY: Storage is correctly aligned and the default constructor has
        // no arguments.
        unsafe { constructor(value.as_mut_ptr(), ptr::null()) };
        value.initialized = true;
        Ok(value)
    }

    pub(crate) fn from_standard(
        interface: EngineInterface,
        object: GDExtensionObjectPtr,
        name: &OwnedStringName,
    ) -> Result<Self, ValueError> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 2)?;
        let arguments = [
            ptr::from_ref(&object).cast(),
            name.as_ptr().cast::<core::ffi::c_void>(),
        ];
        // SAFETY: Constructor index and argument layout come from the
        // authenticated Signal API.
        unsafe { constructor(value.as_mut_ptr(), arguments.as_ptr()) };
        value.initialized = true;
        Ok(value)
    }

    pub(crate) fn from_variant(
        codec: &VariantCodec,
        value: GDExtensionConstVariantPtr,
    ) -> Result<Self, ValueError> {
        if codec.variant_type(value)
            != Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL)
        {
            return Err(invalid("Godot Variant is not a Signal"));
        }
        let interface = codec.interface();
        let mut signal = Self::uninitialized(interface)?;
        let get_to = interface
            .get_variant_to_type_constructor
            .ok_or_else(|| internal("Godot Variant-to-Signal constructor is unavailable"))?;
        // SAFETY: Signal is an official Variant builtin type.
        let convert = unsafe { get_to(signal_type()) }
            .ok_or_else(|| internal("Godot Variant-to-Signal conversion is unavailable"))?;
        // SAFETY: Output is aligned Signal storage and input is a live Signal
        // Variant.
        unsafe { convert(signal.as_mut_ptr(), value.cast_mut()) };
        signal.initialized = true;
        Ok(signal)
    }

    pub(crate) fn to_variant(&self) -> Result<OwnedVariant, ValueError> {
        let get_from = self
            .interface
            .get_variant_from_type_constructor
            .ok_or_else(|| internal("Godot Signal-to-Variant constructor is unavailable"))?;
        // SAFETY: Signal is an official Variant builtin type.
        let convert = unsafe { get_from(signal_type()) }
            .ok_or_else(|| internal("Godot Signal-to-Variant conversion is unavailable"))?;
        let mut variant = OwnedVariant::uninitialized(self.interface);
        // SAFETY: Destination is uninitialized Variant storage and source is
        // one live Signal.
        unsafe { convert(variant.as_mut_ptr(), self.as_ptr().cast_mut()) };
        variant.mark_initialized();
        Ok(variant)
    }

    pub(crate) fn object_id(&self) -> u64 {
        let mut object_id = 0_i64;
        // SAFETY: This builtin method takes no arguments and writes an int64
        // return value.
        unsafe {
            (self.get_object_id)(
                self.as_mut_const_ptr(),
                ptr::null(),
                ptr::from_mut(&mut object_id).cast(),
                0,
            );
        }
        object_id as u64
    }

    pub(crate) fn name(&self) -> Result<String, ValueError> {
        let mut name = OwnedStringName::empty(self.interface)
            .ok_or_else(|| internal("Godot StringName could not be initialized"))?;
        // SAFETY: This builtin method takes no arguments and writes one
        // initialized StringName result.
        unsafe {
            (self.get_name)(self.as_mut_const_ptr(), ptr::null(), name.as_mut_ptr(), 0);
        }
        name.to_utf8()
            .map_err(|_| internal("Godot Signal returned invalid UTF-8"))
    }

    pub(crate) fn connect(
        &mut self,
        callable: &crate::callable_value::NativeCallable,
        flags: i64,
    ) -> Result<i64, ValueError> {
        let connect = builtin_method(self.interface, c"connect", CONNECT_HASH)?;
        let arguments = [callable.as_ptr(), ptr::from_ref(&flags).cast()];
        let mut error = 0_i64;
        // SAFETY: The authenticated Signal builtin contract accepts one
        // Callable and one int64 flag value and writes one int64 Error.
        unsafe {
            connect(
                self.as_mut_ptr(),
                arguments.as_ptr(),
                ptr::from_mut(&mut error).cast(),
                2,
            );
        }
        Ok(error)
    }

    pub(crate) fn disconnect(
        &mut self,
        callable: &crate::callable_value::NativeCallable,
    ) -> Result<(), ValueError> {
        let disconnect = builtin_method(self.interface, c"disconnect", DISCONNECT_HASH)?;
        let arguments = [callable.as_ptr()];
        // SAFETY: The authenticated Signal builtin contract accepts one
        // Callable and has no return value.
        unsafe {
            disconnect(self.as_mut_ptr(), arguments.as_ptr(), ptr::null_mut(), 1);
        }
        Ok(())
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, ValueError> {
        let object_id = self.object_id();
        let name = self.name()?;
        let name_length = u32::try_from(name.len())
            .map_err(|_| unsupported("Godot Signal name exceeds the Host limit"))?;
        let mut bytes = Vec::with_capacity(SIGNAL_HEADER_BYTES + name.len());
        bytes.extend_from_slice(&ABI_SIGNAL_MAGIC);
        bytes.extend_from_slice(&ABI_SIGNAL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&object_id.to_le_bytes());
        bytes.extend_from_slice(&name_length.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        validate_signal_value(&bytes)
            .then_some(bytes)
            .ok_or_else(|| internal("Godot Signal produced an invalid ABI payload"))
    }

    pub(crate) fn from_abi(
        interface: EngineInterface,
        value: AbiValueV1,
        resolve_object: impl FnOnce(u64) -> Result<GDExtensionObjectPtr, ValueError>,
    ) -> Result<Self, ValueError> {
        if value.type_ != AbiValueType::SIGNAL
            || !matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
        {
            return Err(invalid(
                "Signal argument does not match its generated contract",
            ));
        }
        let (pointer, length) = value
            .byte_range(AbiValueType::SIGNAL)
            .ok_or_else(|| invalid("Signal argument has an invalid byte range"))?;
        // SAFETY: The project module synchronously retains this bounded range.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        if !validate_signal_value(bytes) {
            return Err(invalid("Signal argument has an invalid ABI payload"));
        }
        let object_id = u64::from_le_bytes(
            bytes[12..20]
                .try_into()
                .expect("validated Signal object ID"),
        );
        if object_id == 0 {
            return Self::empty(interface);
        }
        let name =
            core::str::from_utf8(&bytes[SIGNAL_HEADER_BYTES..]).expect("validated Signal UTF-8");
        let object = resolve_object(object_id)?;
        let name = OwnedStringName::new(interface, name)
            .ok_or_else(|| invalid("Signal name could not be encoded"))?;
        Self::from_standard(interface, object, &name)
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstTypePtr {
        self.storage.0.as_ptr().cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        self.storage.0.as_mut_ptr().cast()
    }

    fn as_mut_const_ptr(&self) -> GDExtensionTypePtr {
        self.storage.0.as_ptr().cast_mut().cast()
    }

    fn uninitialized(interface: EngineInterface) -> Result<Self, ValueError> {
        Ok(Self {
            interface,
            storage: SignalStorage([0; 16]),
            initialized: false,
            destroy: destructor(interface)?,
            get_object_id: builtin_method(interface, c"get_object_id", GET_OBJECT_ID_HASH)?,
            get_name: builtin_method(interface, c"get_name", GET_NAME_HASH)?,
        })
    }
}

impl Drop for NativeSignal {
    fn drop(&mut self) {
        if !self.initialized {
            return;
        }
        // SAFETY: This wrapper owns one initialized Signal.
        unsafe { (self.destroy)(self.as_mut_ptr()) };
    }
}

fn constructor(interface: EngineInterface, index: i32) -> Result<SignalConstructor, ValueError> {
    let get = interface
        .variant_get_ptr_constructor
        .ok_or_else(|| internal("Godot builtin constructors are unavailable"))?;
    // SAFETY: Signal and constructor indices are authenticated.
    unsafe { get(signal_type(), index) }
        .ok_or_else(|| internal("Godot Signal constructor is unavailable"))
}

fn destructor(interface: EngineInterface) -> Result<SignalDestructor, ValueError> {
    let get = interface
        .variant_get_ptr_destructor
        .ok_or_else(|| internal("Godot builtin destructors are unavailable"))?;
    // SAFETY: Signal is an official builtin type.
    unsafe { get(signal_type()) }.ok_or_else(|| internal("Godot Signal destructor is unavailable"))
}

fn builtin_method(
    interface: EngineInterface,
    name: &'static core::ffi::CStr,
    hash: i64,
) -> Result<SignalBuiltinMethod, ValueError> {
    let get = interface
        .variant_get_ptr_builtin_method
        .ok_or_else(|| internal("Godot builtin method lookup is unavailable"))?;
    let name = crate::string_name::StaticStringName::new(interface, name);
    // SAFETY: Name and hash come from the authenticated Signal API.
    unsafe { get(signal_type(), name.as_ptr(), hash) }
        .ok_or_else(|| internal("Godot Signal builtin method is unavailable"))
}

const fn signal_type() -> GDExtensionVariantType {
    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL
}

fn invalid(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::InvalidArgument, message)
}

fn unsupported(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Unsupported, message)
}

fn internal(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Internal, message)
}
