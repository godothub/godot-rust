use core::ffi::{c_char, c_void};
use core::mem::MaybeUninit;

use crate::callable::Callable;
use crate::math::{
    Aabb, Basis, Color, Plane, Projection, Quaternion, Rect2, Rect2i, Transform2D, Transform3D,
    Vector2, Vector2i, Vector3, Vector3i, Vector4, Vector4i,
};
use crate::node_path::NodePath;
use crate::packed_array::{
    PackedByteArray, PackedColorArray, PackedFloat32Array, PackedFloat64Array, PackedInt32Array,
    PackedInt64Array, PackedStringArray, PackedVector2Array, PackedVector3Array,
    PackedVector4Array,
};
use crate::rid::Rid;
use crate::signal::Signal;
use crate::string_name::StringName;
use crate::variant::{Array, Dictionary, Variant, VariantConvert};

use super::dynamic_value::NativeVariant;
use super::runtime::Interface;
use super::{NativeError, sys};

const MAX_NATIVE_TEXT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct GodotStringName {
    storage: MaybeUninit<usize>,
    interface: Interface,
}

impl GodotStringName {
    pub fn new(interface: &Interface, value: &str) -> Result<Self, NativeError> {
        let length = i64::try_from(value.len())
            .map_err(|_| NativeError::new("StringName is too large for Godot"))?;
        let mut name = Self {
            storage: MaybeUninit::uninit(),
            interface: *interface,
        };
        // SAFETY: Pointer-sized aligned storage matches StringName in every
        // supported official 32/64-bit API configuration.
        unsafe {
            (interface.string_name_new)(name.as_mut_ptr(), value.as_ptr().cast::<c_char>(), length);
        }
        Ok(name)
    }

    pub fn as_ptr(&self) -> sys::GDExtensionConstStringNamePtr {
        self.storage.as_ptr().cast::<c_void>()
    }

    pub(crate) unsafe fn matches_ptr(&self, value: sys::GDExtensionConstStringNamePtr) -> bool {
        if value.is_null() {
            return false;
        }
        // SAFETY: Godot passes an initialized StringName using the same
        // pointer-sized representation validated during Native startup.
        let incoming = unsafe { value.cast::<usize>().read() };
        // SAFETY: `new` initialized this StringName and it remains live.
        incoming == unsafe { self.storage.assume_init() }
    }

    pub(crate) unsafe fn copy_ptr_to_rust(
        interface: &Interface,
        value: sys::GDExtensionConstStringNamePtr,
    ) -> Result<String, NativeError> {
        if value.is_null() {
            return Err(NativeError::new("Godot supplied a null StringName"));
        }
        let native = NativeVariant::from_raw(
            *interface,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
            value,
        )
        .map_err(|error| NativeError::new(error.to_string()))?;
        match native
            .to_rust(0)
            .map_err(|error| NativeError::new(error.to_string()))?
            .kind()
        {
            crate::variant::VariantKind::StringName(value) => Ok(value.as_str().to_owned()),
            _ => Err(NativeError::new(
                "Godot StringName conversion returned the wrong Variant type",
            )),
        }
    }

    fn as_mut_ptr(&mut self) -> sys::GDExtensionUninitializedStringNamePtr {
        self.storage.as_mut_ptr().cast::<c_void>()
    }
}

impl Drop for GodotStringName {
    fn drop(&mut self) {
        // SAFETY: `new` initialized this storage and this Drop runs once.
        unsafe { (self.interface.string_name_destroy)(self.as_mut_ptr()) };
    }
}

pub(crate) struct GodotString {
    storage: MaybeUninit<usize>,
    interface: Interface,
}

impl GodotString {
    pub fn new(interface: &Interface, value: &str) -> Result<Self, NativeError> {
        let length = i64::try_from(value.len())
            .map_err(|_| NativeError::new("String is too large for Godot"))?;
        let mut string = Self {
            storage: MaybeUninit::uninit(),
            interface: *interface,
        };
        // SAFETY: Pointer-sized aligned storage matches String in every
        // supported official 32/64-bit API configuration.
        let error = unsafe {
            (interface.string_new)(string.as_mut_ptr(), value.as_ptr().cast::<c_char>(), length)
        };
        if error != 0 {
            return Err(NativeError::new(format!(
                "Godot rejected a UTF-8 String with error code {error}"
            )));
        }
        Ok(string)
    }

    pub fn as_ptr(&self) -> sys::GDExtensionConstStringPtr {
        self.storage.as_ptr().cast::<c_void>()
    }

    unsafe fn from_variant(interface: &Interface, value: sys::GDExtensionConstVariantPtr) -> Self {
        let mut string = Self {
            storage: MaybeUninit::uninit(),
            interface: *interface,
        };
        // SAFETY: The caller checked that `value` is a live String Variant and
        // this constructor initializes exactly one Godot String.
        unsafe { (interface.string_from_variant)(string.as_mut_ptr(), value.cast_mut()) };
        string
    }

    fn to_rust_string(&self) -> Result<String, NativeError> {
        // SAFETY: `self` owns one initialized Godot String.
        let length = unsafe {
            (self.interface.string_to_utf8_chars)(self.as_ptr(), core::ptr::null_mut(), 0)
        };
        let length = usize::try_from(length)
            .map_err(|_| NativeError::new("Godot returned an invalid UTF-8 String length"))?;
        if length > MAX_NATIVE_TEXT_BYTES {
            return Err(NativeError::new(format!(
                "Godot String exceeds the {MAX_NATIVE_TEXT_BYTES} byte Native boundary limit"
            )));
        }
        let mut bytes = vec![0_u8; length];
        // SAFETY: The buffer has exactly `length` writable bytes and `self`
        // remains initialized throughout the conversion.
        let written = unsafe {
            (self.interface.string_to_utf8_chars)(
                self.as_ptr(),
                bytes.as_mut_ptr().cast::<c_char>(),
                length as i64,
            )
        };
        if written != length as i64 {
            return Err(NativeError::new(
                "Godot changed a String while converting it to UTF-8",
            ));
        }
        String::from_utf8(bytes)
            .map_err(|_| NativeError::new("Godot returned invalid UTF-8 for a String"))
    }

    pub(crate) unsafe fn copy_ptr_to_rust(
        interface: &Interface,
        value: sys::GDExtensionConstTypePtr,
    ) -> Result<String, NativeError> {
        let string = Self {
            // SAFETY: This temporary only borrows the initialized pointer-sized
            // Godot String representation and is intentionally forgotten.
            storage: MaybeUninit::new(unsafe { value.cast::<usize>().read() }),
            interface: *interface,
        };
        let result = string.to_rust_string();
        core::mem::forget(string);
        result
    }

    pub(crate) unsafe fn move_into_ptr(mut self, destination: sys::GDExtensionTypePtr) {
        // SAFETY: `self` owns one initialized pointer-sized Godot String and
        // ptrcall metadata provides uninitialized destination storage.
        let raw = unsafe { self.storage.assume_init_read() };
        // SAFETY: The destination has the exact String ptrcall layout.
        unsafe { destination.cast::<usize>().write(raw) };
        // The destination now owns the String representation.
        self.storage = MaybeUninit::uninit();
        core::mem::forget(self);
    }

    fn as_mut_ptr(&mut self) -> sys::GDExtensionUninitializedStringPtr {
        self.storage.as_mut_ptr().cast::<c_void>()
    }
}

impl Drop for GodotString {
    fn drop(&mut self) {
        // SAFETY: `new` initialized this storage and this Drop runs once.
        unsafe { (self.interface.string_destroy)(self.as_mut_ptr()) };
    }
}

pub(crate) mod private {
    pub trait Sealed {}
}

/// Rust values supported by the safe Native method ABI.
///
/// This trait is implemented by `()`, `bool`, `i64`, and `f64`. More generated
/// Godot types will be added without exposing raw Variant pointers.
pub trait GodotValue: private::Sealed + Sized + 'static {
    #[doc(hidden)]
    const __VARIANT_TYPE: Option<sys::GDExtensionVariantType>;
    #[doc(hidden)]
    const __CLASS_NAME: &'static str = "";
}

pub(crate) trait GodotValueAbi: GodotValue {
    const VARIANT_TYPE: Option<sys::GDExtensionVariantType>;

    unsafe fn from_variant(interface: &Interface, value: sys::GDExtensionConstVariantPtr) -> Self;

    unsafe fn write_variant(self, interface: &Interface, destination: sys::GDExtensionVariantPtr);

    unsafe fn from_ptr(interface: &Interface, value: sys::GDExtensionConstTypePtr) -> Self;

    unsafe fn write_ptr(self, interface: &Interface, destination: sys::GDExtensionTypePtr);
}

fn dynamic_value_from_variant<T>(
    interface: &Interface,
    value: sys::GDExtensionConstVariantPtr,
    label: &str,
) -> T
where
    T: VariantConvert + Default,
{
    let result = NativeVariant::copy_from(*interface, value)
        .to_rust(0)
        .and_then(|value| {
            T::from_variant(value).ok_or_else(|| {
                crate::error::EngineError::invalid_argument(format!(
                    "Godot Variant does not contain the registered Native {label} type"
                ))
            })
        });
    result.unwrap_or_else(|error| {
        interface.report_error(&error.to_string(), "Native dynamic argument");
        T::default()
    })
}

fn dynamic_value_from_ptr<T>(
    interface: &Interface,
    variant_type: sys::GDExtensionVariantType,
    value: sys::GDExtensionConstTypePtr,
    label: &str,
) -> T
where
    T: VariantConvert + Default,
{
    let native = if variant_type == sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL {
        NativeVariant::copy_from(*interface, value.cast())
    } else {
        match NativeVariant::from_raw(*interface, variant_type, value) {
            Ok(value) => value,
            Err(error) => {
                interface.report_error(&error.to_string(), "Native dynamic ptrcall argument");
                return T::default();
            }
        }
    };
    native
        .to_rust(0)
        .ok()
        .and_then(T::from_variant)
        .unwrap_or_else(|| {
            interface.report_error(
                &format!("Godot ptrcall does not contain the registered Native {label} type"),
                "Native dynamic ptrcall argument",
            );
            T::default()
        })
}

fn write_dynamic_variant<T>(
    value: T,
    interface: &Interface,
    destination: sys::GDExtensionVariantPtr,
    label: &str,
) where
    T: VariantConvert + Default,
{
    let write = |value: &T| {
        NativeVariant::from_rust(*interface, &value.to_variant(), 0)
            .and_then(|native| native.copy_to_variant(destination))
    };
    if let Err(error) = write(&value) {
        interface.report_error(
            &format!("could not encode Native {label} return: {error}"),
            "Native dynamic return",
        );
        if let Err(fallback_error) = write(&T::default()) {
            interface.report_error(
                &format!("could not initialize fallback Native {label}: {fallback_error}"),
                "Native dynamic return",
            );
        }
    }
}

fn write_dynamic_ptr<T>(
    value: T,
    interface: &Interface,
    variant_type: sys::GDExtensionVariantType,
    destination: sys::GDExtensionTypePtr,
    label: &str,
) where
    T: VariantConvert + Default,
{
    let write = |value: &T| {
        let native = NativeVariant::from_rust(*interface, &value.to_variant(), 0)?;
        if variant_type == sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL {
            native.copy_to_variant(destination.cast())
        } else {
            native.to_raw_value(variant_type, destination)
        }
    };
    if let Err(error) = write(&value) {
        interface.report_error(
            &format!("could not encode Native {label} ptrcall return: {error}"),
            "Native dynamic ptrcall return",
        );
        if let Err(fallback_error) = write(&T::default()) {
            interface.report_error(
                &format!("could not initialize fallback Native {label}: {fallback_error}"),
                "Native dynamic ptrcall return",
            );
        }
    }
}

macro_rules! dynamic_value {
    ($rust:ty, $variant:ident, $label:literal) => {
        impl private::Sealed for $rust {}

        impl GodotValue for $rust {
            const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::$variant);
        }

        impl GodotValueAbi for $rust {
            const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::$variant);

            unsafe fn from_variant(
                interface: &Interface,
                value: sys::GDExtensionConstVariantPtr,
            ) -> Self {
                dynamic_value_from_variant(interface, value, $label)
            }

            unsafe fn write_variant(
                self,
                interface: &Interface,
                destination: sys::GDExtensionVariantPtr,
            ) {
                write_dynamic_variant(self, interface, destination, $label);
            }

            unsafe fn from_ptr(interface: &Interface, value: sys::GDExtensionConstTypePtr) -> Self {
                dynamic_value_from_ptr(
                    interface,
                    sys::GDExtensionVariantType::$variant,
                    value,
                    $label,
                )
            }

            unsafe fn write_ptr(self, interface: &Interface, destination: sys::GDExtensionTypePtr) {
                write_dynamic_ptr(
                    self,
                    interface,
                    sys::GDExtensionVariantType::$variant,
                    destination,
                    $label,
                );
            }
        }
    };
}

impl private::Sealed for () {}
impl GodotValue for () {
    const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> = None;
}

impl GodotValueAbi for () {
    const VARIANT_TYPE: Option<sys::GDExtensionVariantType> = None;

    unsafe fn from_variant(
        _interface: &Interface,
        _value: sys::GDExtensionConstVariantPtr,
    ) -> Self {
    }

    unsafe fn write_variant(
        self,
        _interface: &Interface,
        _destination: sys::GDExtensionVariantPtr,
    ) {
    }

    unsafe fn from_ptr(_interface: &Interface, _value: sys::GDExtensionConstTypePtr) -> Self {}

    unsafe fn write_ptr(self, _interface: &Interface, _destination: sys::GDExtensionTypePtr) {}
}

macro_rules! scalar_value {
    (
        $rust:ty,
        $raw:ty,
        $variant:ident,
        $from_variant:ident,
        $to_variant:ident,
        $to_raw:expr,
        $from_raw:expr
    ) => {
        impl private::Sealed for $rust {}
        impl GodotValue for $rust {
            const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::$variant);
        }

        impl GodotValueAbi for $rust {
            const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::$variant);

            unsafe fn from_variant(
                interface: &Interface,
                value: sys::GDExtensionConstVariantPtr,
            ) -> Self {
                let mut raw = MaybeUninit::<$raw>::uninit();
                // SAFETY: The caller validates the Variant type first and the
                // generated constructor writes exactly `$raw`.
                unsafe {
                    (interface.$from_variant)(raw.as_mut_ptr().cast::<c_void>(), value.cast_mut());
                }
                // SAFETY: The generated constructor above initialized `raw`.
                ($from_raw)(unsafe { raw.assume_init() })
            }

            unsafe fn write_variant(
                self,
                interface: &Interface,
                destination: sys::GDExtensionVariantPtr,
            ) {
                let mut raw: $raw = ($to_raw)(self);
                // SAFETY: The generated constructor reads exactly `$raw` and
                // initializes the destination Variant.
                unsafe {
                    (interface.$to_variant)(destination, (&mut raw as *mut $raw).cast::<c_void>());
                }
            }

            unsafe fn from_ptr(
                _interface: &Interface,
                value: sys::GDExtensionConstTypePtr,
            ) -> Self {
                // SAFETY: Ptrcall metadata guarantees the exact scalar type.
                ($from_raw)(unsafe { value.cast::<$raw>().read() })
            }

            unsafe fn write_ptr(
                self,
                _interface: &Interface,
                destination: sys::GDExtensionTypePtr,
            ) {
                // SAFETY: Ptrcall metadata guarantees writable scalar storage.
                unsafe {
                    destination.cast::<$raw>().write(($to_raw)(self));
                }
            }
        }
    };
}

scalar_value!(
    bool,
    sys::GDExtensionBool,
    GDEXTENSION_VARIANT_TYPE_BOOL,
    bool_from_variant,
    variant_from_bool,
    |value: bool| u8::from(value),
    |value: u8| value != 0
);
scalar_value!(
    i64,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    |value: i64| value,
    |value: i64| value
);
scalar_value!(
    i32,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    i64::from,
    |value: i64| i32::try_from(value).unwrap_or_default()
);
scalar_value!(
    i16,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    i64::from,
    |value: i64| i16::try_from(value).unwrap_or_default()
);
scalar_value!(
    i8,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    i64::from,
    |value: i64| i8::try_from(value).unwrap_or_default()
);
scalar_value!(
    u64,
    u64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    |value: u64| value,
    |value: u64| value
);
scalar_value!(
    u32,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    i64::from,
    |value: i64| u32::try_from(value).unwrap_or_default()
);
scalar_value!(
    u16,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    i64::from,
    |value: i64| u16::try_from(value).unwrap_or_default()
);
scalar_value!(
    u8,
    i64,
    GDEXTENSION_VARIANT_TYPE_INT,
    int_from_variant,
    variant_from_int,
    i64::from,
    |value: i64| u8::try_from(value).unwrap_or_default()
);
scalar_value!(
    f64,
    f64,
    GDEXTENSION_VARIANT_TYPE_FLOAT,
    float_from_variant,
    variant_from_float,
    |value: f64| value,
    |value: f64| value
);
scalar_value!(
    f32,
    f64,
    GDEXTENSION_VARIANT_TYPE_FLOAT,
    float_from_variant,
    variant_from_float,
    f64::from,
    |value: f64| {
        let narrowed = value as f32;
        if value.is_finite() && !narrowed.is_finite() {
            0.0
        } else {
            narrowed
        }
    }
);

pub(crate) unsafe fn integer_from_variant<T: crate::engine::GodotIntegerValue>(
    interface: &Interface,
    value: sys::GDExtensionConstVariantPtr,
) -> T {
    let mut raw = MaybeUninit::<i64>::uninit();
    // SAFETY: The caller validated an integer Variant and the official
    // constructor writes one i64.
    unsafe { (interface.int_from_variant)(raw.as_mut_ptr().cast(), value.cast_mut()) };
    // SAFETY: The constructor above initialized the exact integer storage.
    T::__from_raw(unsafe { raw.assume_init() } as u64)
}

pub(crate) unsafe fn integer_write_variant<T: crate::engine::GodotIntegerValue>(
    value: T,
    interface: &Interface,
    destination: sys::GDExtensionVariantPtr,
) {
    let mut raw = value.__raw() as i64;
    // SAFETY: The destination is an uninitialized Variant return slot and
    // Godot integer Variants use one i64 for enums and bitfields.
    unsafe { (interface.variant_from_int)(destination, core::ptr::from_mut(&mut raw).cast()) };
}

pub(crate) unsafe fn integer_from_ptr<T: crate::engine::GodotIntegerValue>(
    value: sys::GDExtensionConstTypePtr,
) -> T {
    // SAFETY: ClassDB registered the method argument as a Godot integer.
    T::__from_raw(unsafe { value.cast::<i64>().read() } as u64)
}

pub(crate) unsafe fn integer_write_ptr<T: crate::engine::GodotIntegerValue>(
    value: T,
    destination: sys::GDExtensionTypePtr,
) {
    // SAFETY: ClassDB registered the return slot as a Godot integer.
    unsafe { destination.cast::<i64>().write(value.__raw() as i64) };
}

impl private::Sealed for String {}
impl GodotValue for String {
    const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
        Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING);
}

impl GodotValueAbi for String {
    const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
        Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING);

    unsafe fn from_variant(interface: &Interface, value: sys::GDExtensionConstVariantPtr) -> Self {
        // SAFETY: The caller verified the exact dynamic Variant type.
        let value = unsafe { GodotString::from_variant(interface, value) };
        value.to_rust_string().unwrap_or_else(|error| {
            interface.report_error(&error.to_string(), "Native String argument");
            String::new()
        })
    }

    unsafe fn write_variant(self, interface: &Interface, destination: sys::GDExtensionVariantPtr) {
        let value = match GodotString::new(interface, &self) {
            Ok(value) => value,
            Err(error) => {
                interface.report_error(&error.to_string(), "Native String return");
                return;
            }
        };
        // SAFETY: `value` is an initialized String and destination is the
        // Variant output declared during registration.
        unsafe { (interface.variant_from_string)(destination, value.as_ptr().cast_mut()) };
    }

    unsafe fn from_ptr(interface: &Interface, value: sys::GDExtensionConstTypePtr) -> Self {
        // SAFETY: Ptrcall metadata guarantees initialized String storage.
        unsafe { GodotString::copy_ptr_to_rust(interface, value) }.unwrap_or_else(|error| {
            interface.report_error(&error.to_string(), "Native String ptrcall argument");
            String::new()
        })
    }

    unsafe fn write_ptr(self, interface: &Interface, destination: sys::GDExtensionTypePtr) {
        match GodotString::new(interface, &self) {
            // SAFETY: Ptrcall metadata guarantees writable String storage.
            Ok(value) => unsafe { value.move_into_ptr(destination) },
            Err(error) => {
                interface.report_error(&error.to_string(), "Native String ptrcall return");
            }
        }
    }
}

fn validate_object_class<T: crate::engine::GodotClass>(
    interface: &Interface,
    object: sys::GDExtensionObjectPtr,
) -> bool {
    if object.is_null() {
        return true;
    }
    let class_name = match GodotStringName::new(interface, T::CLASS_NAME) {
        Ok(value) => value,
        Err(error) => {
            interface.report_error(&error.to_string(), "Native Object class validation");
            return false;
        }
    };
    // SAFETY: The StringName is live and the selected interface owns the
    // ClassDB tag for the engine lifetime.
    let class_tag = unsafe { (interface.classdb_get_class_tag)(class_name.as_ptr()) };
    if class_tag.is_null() {
        interface.report_error(
            &format!("Godot has no ClassDB tag for `{}`", T::CLASS_NAME),
            "Native Object class validation",
        );
        return false;
    }
    // SAFETY: Both the object and ClassDB tag are engine-owned live pointers.
    !unsafe { (interface.object_cast_to)(object, class_tag) }.is_null()
}

unsafe fn object_from_variant(
    interface: &Interface,
    value: sys::GDExtensionConstVariantPtr,
) -> sys::GDExtensionObjectPtr {
    // SAFETY: Runtime initialization validated the Object constructor.
    let constructor = unsafe {
        (interface.get_variant_to_type_constructor)(
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
        )
    }
    .expect("Native runtime validates the Object Variant constructor");
    let mut object = core::ptr::null_mut::<c_void>();
    // SAFETY: The caller checked the live Object Variant and this output has
    // the exact GDExtensionObjectPtr storage expected by the constructor.
    unsafe {
        constructor(
            (&mut object as *mut sys::GDExtensionObjectPtr).cast::<c_void>(),
            value.cast_mut(),
        );
    }
    object
}

fn object_ref_from_pointer<T: crate::engine::GodotClass>(
    interface: &Interface,
    object: sys::GDExtensionObjectPtr,
) -> crate::engine::ObjectRef<T> {
    if object.is_null() {
        return crate::engine::ObjectRef::unresolved();
    }
    if !validate_object_class::<T>(interface, object) {
        interface.report_error(
            &format!("Godot Object is not a `{}`", T::CLASS_NAME),
            "Native Object argument",
        );
        return crate::engine::ObjectRef::unresolved();
    }
    // SAFETY: The pointer is live and was validated against the requested
    // ClassDB class immediately above.
    let instance_id = unsafe { (interface.object_get_instance_id)(object) };
    crate::engine::ObjectRef::__from_instance_id(instance_id)
}

fn resolve_object_ref<T: crate::engine::GodotClass>(
    interface: &Interface,
    value: crate::engine::ObjectRef<T>,
) -> sys::GDExtensionObjectPtr {
    if !value.is_resolved() {
        return core::ptr::null_mut();
    }
    // SAFETY: Godot owns the instance-ID registry for the engine lifetime.
    let object = unsafe { (interface.object_get_instance_from_id)(value.instance_id()) };
    if object.is_null() || !validate_object_class::<T>(interface, object) {
        interface.report_error(
            &format!(
                "Godot Object {} is stale or is not a `{}`",
                value.instance_id(),
                T::CLASS_NAME
            ),
            "Native Object return",
        );
        return core::ptr::null_mut();
    }
    object
}

unsafe fn write_object_variant(
    interface: &Interface,
    mut object: sys::GDExtensionObjectPtr,
    destination: sys::GDExtensionVariantPtr,
) {
    // SAFETY: Runtime initialization validated the Object constructor.
    let constructor = unsafe {
        (interface.get_variant_from_type_constructor)(
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
        )
    }
    .expect("Native runtime validates the Object-to-Variant constructor");
    // SAFETY: `object` is exact pointer storage and destination is the
    // declared Object Variant output.
    unsafe {
        constructor(
            destination,
            (&mut object as *mut sys::GDExtensionObjectPtr).cast::<c_void>(),
        );
    }
}

macro_rules! object_value {
    ($rust:ty, $nullable:expr) => {
        impl<T: crate::engine::GodotClass + 'static> private::Sealed for $rust {}

        impl<T: crate::engine::GodotClass + 'static> GodotValue for $rust {
            const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT);
            const __CLASS_NAME: &'static str = T::CLASS_NAME;
        }

        impl<T: crate::engine::GodotClass + 'static> GodotValueAbi for $rust {
            const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT);

            unsafe fn from_variant(
                interface: &Interface,
                value: sys::GDExtensionConstVariantPtr,
            ) -> Self {
                // SAFETY: The caller validated the exact Object Variant type.
                let object = unsafe { object_from_variant(interface, value) };
                object_value_from_ref::<T, _>(object_ref_from_pointer(interface, object), $nullable)
            }

            unsafe fn write_variant(
                self,
                interface: &Interface,
                destination: sys::GDExtensionVariantPtr,
            ) {
                let object = resolve_object_value(interface, self);
                // SAFETY: The resolved pointer is live or null.
                unsafe { write_object_variant(interface, object, destination) };
            }

            unsafe fn from_ptr(interface: &Interface, value: sys::GDExtensionConstTypePtr) -> Self {
                // SAFETY: Ptrcall metadata guarantees readable Object pointer storage.
                let object = unsafe { value.cast::<sys::GDExtensionObjectPtr>().read() };
                object_value_from_ref::<T, _>(object_ref_from_pointer(interface, object), $nullable)
            }

            unsafe fn write_ptr(self, interface: &Interface, destination: sys::GDExtensionTypePtr) {
                let object = resolve_object_value(interface, self);
                // SAFETY: Ptrcall metadata guarantees writable Object pointer storage.
                unsafe {
                    destination
                        .cast::<sys::GDExtensionObjectPtr>()
                        .write(object)
                };
            }
        }
    };
}

fn object_value_from_ref<T: crate::engine::GodotClass, V: ObjectValue<T>>(
    value: crate::engine::ObjectRef<T>,
    nullable: bool,
) -> V {
    V::from_ref(value, nullable)
}

fn resolve_object_value<T: crate::engine::GodotClass, V: ObjectValue<T>>(
    interface: &Interface,
    value: V,
) -> sys::GDExtensionObjectPtr {
    value.into_ref().map_or(core::ptr::null_mut(), |value| {
        resolve_object_ref(interface, value)
    })
}

trait ObjectValue<T: crate::engine::GodotClass>: Sized {
    fn from_ref(value: crate::engine::ObjectRef<T>, nullable: bool) -> Self;
    fn into_ref(self) -> Option<crate::engine::ObjectRef<T>>;
}

impl<T: crate::engine::GodotClass> ObjectValue<T> for crate::engine::ObjectRef<T> {
    fn from_ref(value: crate::engine::ObjectRef<T>, _nullable: bool) -> Self {
        value
    }

    fn into_ref(self) -> Option<crate::engine::ObjectRef<T>> {
        Some(self)
    }
}

impl<T: crate::engine::GodotClass> ObjectValue<T> for Option<crate::engine::ObjectRef<T>> {
    fn from_ref(value: crate::engine::ObjectRef<T>, _nullable: bool) -> Self {
        value.is_resolved().then_some(value)
    }

    fn into_ref(self) -> Option<crate::engine::ObjectRef<T>> {
        self
    }
}

object_value!(crate::engine::ObjectRef<T>, false);
object_value!(Option<crate::engine::ObjectRef<T>>, true);

impl<T: crate::engine::GodotClass + 'static> private::Sealed
    for Option<crate::engine::GodotRef<T>>
{
}

impl<T: crate::engine::GodotClass + 'static> GodotValue for Option<crate::engine::GodotRef<T>> {
    const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
        Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT);
    const __CLASS_NAME: &'static str = T::CLASS_NAME;
}

impl<T: crate::engine::GodotClass + 'static> GodotValueAbi for Option<crate::engine::GodotRef<T>> {
    const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
        Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT);

    unsafe fn from_variant(interface: &Interface, value: sys::GDExtensionConstVariantPtr) -> Self {
        validate_owned_object_type(
            interface,
            dynamic_value_from_variant(interface, value, "RefCounted Object"),
        )
    }

    unsafe fn write_variant(self, interface: &Interface, destination: sys::GDExtensionVariantPtr) {
        write_dynamic_variant(self, interface, destination, "RefCounted Object");
    }

    unsafe fn from_ptr(interface: &Interface, value: sys::GDExtensionConstTypePtr) -> Self {
        validate_owned_object_type(
            interface,
            dynamic_value_from_ptr(
                interface,
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
                value,
                "RefCounted Object",
            ),
        )
    }

    unsafe fn write_ptr(self, interface: &Interface, destination: sys::GDExtensionTypePtr) {
        write_dynamic_ptr(
            self,
            interface,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
            destination,
            "RefCounted Object",
        );
    }
}

fn validate_owned_object_type<T: crate::engine::GodotClass>(
    interface: &Interface,
    value: Option<crate::engine::GodotRef<T>>,
) -> Option<crate::engine::GodotRef<T>> {
    let value = value?;
    let object = resolve_object_ref(interface, value.object_ref());
    if object.is_null() || !validate_object_class::<T>(interface, object) {
        interface.report_error(
            &format!("Godot RefCounted Object is not a live `{}`", T::CLASS_NAME),
            "Native RefCounted Object argument",
        );
        None
    } else {
        Some(value)
    }
}

macro_rules! copy_value {
    ($rust:ty, $variant:ident) => {
        impl private::Sealed for $rust {}
        impl GodotValue for $rust {
            const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::$variant);
        }

        impl GodotValueAbi for $rust {
            const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
                Some(sys::GDExtensionVariantType::$variant);

            unsafe fn from_variant(
                interface: &Interface,
                value: sys::GDExtensionConstVariantPtr,
            ) -> Self {
                // SAFETY: Runtime initialization validated this generated
                // Variant type against the selected official interface.
                let constructor = unsafe {
                    (interface.get_variant_to_type_constructor)(
                        sys::GDExtensionVariantType::$variant,
                    )
                }
                .expect("Native runtime validates every safe value constructor");
                let mut output = MaybeUninit::<Self>::uninit();
                // SAFETY: The caller validated the Variant type and the
                // selected standard Godot ABI uses this exact repr(C) layout.
                unsafe { constructor(output.as_mut_ptr().cast::<c_void>(), value.cast_mut()) };
                // SAFETY: The official constructor initialized `output`.
                unsafe { output.assume_init() }
            }

            unsafe fn write_variant(
                mut self,
                interface: &Interface,
                destination: sys::GDExtensionVariantPtr,
            ) {
                // SAFETY: Runtime initialization validated this generated
                // Variant type against the selected official interface.
                let constructor = unsafe {
                    (interface.get_variant_from_type_constructor)(
                        sys::GDExtensionVariantType::$variant,
                    )
                }
                .expect("Native runtime validates every safe value constructor");
                // SAFETY: The standard Godot ABI reads the exact repr(C)
                // value and initializes the declared Variant output.
                unsafe {
                    constructor(destination, (&mut self as *mut Self).cast::<c_void>());
                }
            }

            unsafe fn from_ptr(
                _interface: &Interface,
                value: sys::GDExtensionConstTypePtr,
            ) -> Self {
                // SAFETY: Ptrcall metadata guarantees exact readable storage.
                unsafe { value.cast::<Self>().read() }
            }

            unsafe fn write_ptr(
                self,
                _interface: &Interface,
                destination: sys::GDExtensionTypePtr,
            ) {
                // SAFETY: Ptrcall metadata guarantees exact writable storage.
                unsafe { destination.cast::<Self>().write(self) };
            }
        }
    };
}

copy_value!(Vector2, GDEXTENSION_VARIANT_TYPE_VECTOR2);
copy_value!(Vector2i, GDEXTENSION_VARIANT_TYPE_VECTOR2I);
copy_value!(Rect2, GDEXTENSION_VARIANT_TYPE_RECT2);
copy_value!(Rect2i, GDEXTENSION_VARIANT_TYPE_RECT2I);
copy_value!(Vector3, GDEXTENSION_VARIANT_TYPE_VECTOR3);
copy_value!(Vector3i, GDEXTENSION_VARIANT_TYPE_VECTOR3I);
copy_value!(Transform2D, GDEXTENSION_VARIANT_TYPE_TRANSFORM2D);
copy_value!(Vector4, GDEXTENSION_VARIANT_TYPE_VECTOR4);
copy_value!(Vector4i, GDEXTENSION_VARIANT_TYPE_VECTOR4I);
copy_value!(Plane, GDEXTENSION_VARIANT_TYPE_PLANE);
copy_value!(Quaternion, GDEXTENSION_VARIANT_TYPE_QUATERNION);
copy_value!(Aabb, GDEXTENSION_VARIANT_TYPE_AABB);
copy_value!(Basis, GDEXTENSION_VARIANT_TYPE_BASIS);
copy_value!(Transform3D, GDEXTENSION_VARIANT_TYPE_TRANSFORM3D);
copy_value!(Projection, GDEXTENSION_VARIANT_TYPE_PROJECTION);
copy_value!(Color, GDEXTENSION_VARIANT_TYPE_COLOR);
copy_value!(Rid, GDEXTENSION_VARIANT_TYPE_RID);

dynamic_value!(
    StringName,
    GDEXTENSION_VARIANT_TYPE_STRING_NAME,
    "StringName"
);
dynamic_value!(NodePath, GDEXTENSION_VARIANT_TYPE_NODE_PATH, "NodePath");
dynamic_value!(Variant, GDEXTENSION_VARIANT_TYPE_NIL, "Variant");
dynamic_value!(
    Dictionary,
    GDEXTENSION_VARIANT_TYPE_DICTIONARY,
    "Dictionary"
);
dynamic_value!(Callable, GDEXTENSION_VARIANT_TYPE_CALLABLE, "Callable");
dynamic_value!(Signal, GDEXTENSION_VARIANT_TYPE_SIGNAL, "Signal");
dynamic_value!(
    PackedByteArray,
    GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY,
    "PackedByteArray"
);
dynamic_value!(
    PackedInt32Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY,
    "PackedInt32Array"
);
dynamic_value!(
    PackedInt64Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY,
    "PackedInt64Array"
);
dynamic_value!(
    PackedFloat32Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY,
    "PackedFloat32Array"
);
dynamic_value!(
    PackedFloat64Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY,
    "PackedFloat64Array"
);
dynamic_value!(
    PackedStringArray,
    GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
    "PackedStringArray"
);
dynamic_value!(
    PackedVector2Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY,
    "PackedVector2Array"
);
dynamic_value!(
    PackedVector3Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY,
    "PackedVector3Array"
);
dynamic_value!(
    PackedVector4Array,
    GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY,
    "PackedVector4Array"
);
dynamic_value!(
    PackedColorArray,
    GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY,
    "PackedColorArray"
);

impl<T> private::Sealed for Array<T> where T: VariantConvert + 'static {}

impl<T> GodotValue for Array<T>
where
    T: VariantConvert + 'static,
{
    const __VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
        Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY);
}

impl<T> GodotValueAbi for Array<T>
where
    T: VariantConvert + 'static,
{
    const VARIANT_TYPE: Option<sys::GDExtensionVariantType> =
        Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY);

    unsafe fn from_variant(interface: &Interface, value: sys::GDExtensionConstVariantPtr) -> Self {
        dynamic_value_from_variant(interface, value, "Array")
    }

    unsafe fn write_variant(self, interface: &Interface, destination: sys::GDExtensionVariantPtr) {
        write_dynamic_variant(self, interface, destination, "Array");
    }

    unsafe fn from_ptr(interface: &Interface, value: sys::GDExtensionConstTypePtr) -> Self {
        dynamic_value_from_ptr(
            interface,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
            value,
            "Array",
        )
    }

    unsafe fn write_ptr(self, interface: &Interface, destination: sys::GDExtensionTypePtr) {
        write_dynamic_ptr(
            self,
            interface,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
            destination,
            "Array",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_reference_types_remain_pointer_sized() {
        assert_eq!(
            core::mem::size_of::<MaybeUninit<usize>>(),
            usize::BITS as usize / 8
        );
    }

    #[test]
    fn scalar_types_map_to_exact_godot_variant_types() {
        assert_eq!(
            <bool as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL)
        );
        assert_eq!(
            <i64 as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT)
        );
        assert_eq!(
            <f64 as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT)
        );
        assert_eq!(
            <i32 as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT)
        );
        assert_eq!(
            <f32 as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT)
        );
        assert_eq!(
            <String as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING)
        );
        assert_eq!(<() as GodotValueAbi>::VARIANT_TYPE, None);
    }

    #[test]
    fn math_and_rid_types_map_to_exact_godot_variant_types() {
        assert_eq!(
            <Vector2 as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2)
        );
        assert_eq!(
            <Transform3D as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D)
        );
        assert_eq!(
            <Projection as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION)
        );
        assert_eq!(
            <Rid as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID)
        );
    }

    #[test]
    fn owned_native_method_values_cover_dynamic_and_generated_integer_types() {
        let array_type = sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY;
        let callable_type = sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE;
        let integer_type = sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT;
        assert_eq!(
            <Array<StringName> as GodotValueAbi>::VARIANT_TYPE,
            Some(array_type)
        );
        assert_eq!(
            <Dictionary as GodotValueAbi>::VARIANT_TYPE,
            Some(sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY)
        );
        assert_eq!(
            <Callable as GodotValueAbi>::VARIANT_TYPE,
            Some(callable_type)
        );
        assert_eq!(
            <crate::engine::global::Error as GodotValueAbi>::VARIANT_TYPE,
            Some(integer_type)
        );
        assert_eq!(
            <crate::engine::global::PropertyUsageFlags as GodotValueAbi>::VARIANT_TYPE,
            Some(integer_type)
        );
    }
}
