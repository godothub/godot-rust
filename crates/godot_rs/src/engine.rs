use core::fmt;
use core::marker::PhantomData;
use godot_rs_api::abi::{AbiGodotApiSpecV1, AbiGodotMethodSpecV1, AbiValueType, AbiValueV1};
use std::rc::Rc;

use crate::callable::Callable;
use crate::error::{EngineError, EngineResult};
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

const MAX_ENGINE_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// Marker implemented by generated Godot engine class types.
pub trait GodotClass {
    /// Godot ClassDB name.
    const CLASS_NAME: &'static str;
}

macro_rules! define_engine_classes {
    ($($name:ident),+ $(,)?) => {
        $(
            #[doc = concat!("Typed marker for Godot `", stringify!($name), "`.")]
            #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
            pub struct $name;

            impl GodotClass for $name {
                const CLASS_NAME: &'static str = stringify!($name);
            }
        )+
    };
}

godot_rs_api::godot_rs_for_each_engine_class!(define_engine_classes);

/// Marks a Godot class that is the same as or derives from `Base`.
pub trait Inherits<Base: GodotClass>: GodotClass {}

impl<T: GodotClass> Inherits<T> for T {}

/// Generation-checked reference to a Godot object.
///
/// Zero is reserved for an unresolved reference. The Host validates the
/// object identity and module generation whenever the handle is used.
#[repr(transparent)]
pub struct ObjectRef<T: GodotClass> {
    raw: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T: GodotClass> ObjectRef<T> {
    /// Creates an unresolved object reference for generated initializers.
    #[doc(hidden)]
    #[must_use]
    pub const fn unresolved() -> Self {
        Self {
            raw: 0,
            marker: PhantomData,
        }
    }

    /// Whether the reference has been resolved by the Host.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        self.raw != 0
    }

    /// Opaque Godot Object instance ID.
    #[must_use]
    pub const fn instance_id(self) -> u64 {
        self.raw
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn __from_instance_id(raw: u64) -> Self {
        Self {
            raw,
            marker: PhantomData,
        }
    }
}

impl<T: GodotClass> Clone for ObjectRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: GodotClass> Copy for ObjectRef<T> {}

impl<T: GodotClass> Default for ObjectRef<T> {
    fn default() -> Self {
        Self::unresolved()
    }
}

impl<T: GodotClass> PartialEq for ObjectRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T: GodotClass> Eq for ObjectRef<T> {}

impl<T: GodotClass> fmt::Debug for ObjectRef<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObjectRef")
            .field("class", &T::CLASS_NAME)
            .field("instance_id", &self.raw)
            .finish()
    }
}

/// Owned reference to a Godot `RefCounted` object returned by the engine.
///
/// Cloning this value keeps the same Godot reference alive without another
/// engine call. The Host releases the native `Ref<T>` when the final Rust
/// clone is dropped. Use [`Self::object_ref`] when an API accepts an ordinary
/// object argument.
pub struct GodotRef<T: GodotClass> {
    object: ObjectRef<T>,
    ownership: SharedGodotRefOwnership,
}

pub(crate) enum GodotRefOwnership {
    Object(crate::module::HostObjectRefToken),
    Dynamic(crate::module::HostDynamicValueToken),
    Native(Box<crate::native::NativeGodotRefToken>),
}

impl Drop for GodotRefOwnership {
    fn drop(&mut self) {
        match self {
            Self::Object(value) => {
                let _ = value;
            }
            Self::Dynamic(value) => {
                let _ = value;
            }
            Self::Native(value) => {
                let _ = value;
            }
        }
    }
}

pub(crate) type SharedGodotRefOwnership = Rc<GodotRefOwnership>;

impl<T: GodotClass> GodotRef<T> {
    /// Returns the copyable object identity carried by this owned reference.
    #[must_use]
    pub const fn object_ref(&self) -> ObjectRef<T> {
        self.object
    }

    /// Opaque Godot Object instance ID.
    #[must_use]
    pub const fn instance_id(&self) -> u64 {
        self.object.instance_id()
    }

    pub(crate) fn from_owned_parts(
        object_id: u64,
        ownership: crate::module::HostObjectRefToken,
    ) -> Self {
        Self {
            object: ObjectRef::__from_instance_id(object_id),
            ownership: Rc::new(GodotRefOwnership::Object(ownership)),
        }
    }

    pub(crate) fn from_dynamic_parts(object_id: u64, ownership: SharedGodotRefOwnership) -> Self {
        Self {
            object: ObjectRef::__from_instance_id(object_id),
            ownership,
        }
    }

    pub(crate) fn from_native_parts(
        object_id: u64,
        ownership: crate::native::NativeGodotRefToken,
    ) -> Self {
        Self {
            object: ObjectRef::__from_instance_id(object_id),
            ownership: Rc::new(GodotRefOwnership::Native(Box::new(ownership))),
        }
    }

    pub(crate) fn dynamic_ownership(&self) -> SharedGodotRefOwnership {
        Rc::clone(&self.ownership)
    }
}

impl<T: GodotClass> Clone for GodotRef<T> {
    fn clone(&self) -> Self {
        Self {
            object: self.object,
            ownership: Rc::clone(&self.ownership),
        }
    }
}

impl<T: GodotClass> PartialEq for GodotRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.object == other.object
    }
}

impl<T: GodotClass> Eq for GodotRef<T> {}

impl<T: GodotClass> fmt::Debug for GodotRef<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GodotRef")
            .field("class", &T::CLASS_NAME)
            .field("instance_id", &self.instance_id())
            .finish()
    }
}

/// Typed object reference used by `#[node]` script fields.
pub type NodeRef<T> = ObjectRef<T>;

/// Current Godot object that owns one Rust script callback.
///
/// The proxy resolves the owner only when a generated method is called, so
/// creating it is infallible while stale or out-of-scope use remains explicit.
pub struct Base<T: GodotClass> {
    marker: PhantomData<fn() -> T>,
}

impl<T: GodotClass> Base<T> {
    #[doc(hidden)]
    #[must_use]
    pub const fn __current() -> Self {
        Self {
            marker: PhantomData,
        }
    }

    /// Resolves the current owning object to a reusable typed handle.
    pub fn object_ref(self) -> EngineResult<ObjectRef<T>> {
        current_object()
    }
}

impl<T: GodotClass> Clone for Base<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: GodotClass> Copy for Base<T> {}

impl<T: GodotClass> fmt::Debug for Base<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Base")
            .field("class", &T::CLASS_NAME)
            .finish()
    }
}

/// Receiver abstraction used by generated Godot API traits.
#[doc(hidden)]
pub trait EngineObject {
    type Class: GodotClass;

    fn __engine_object(&self) -> EngineResult<ObjectRef<Self::Class>>;
}

impl<T: GodotClass> EngineObject for ObjectRef<T> {
    type Class = T;

    fn __engine_object(&self) -> EngineResult<ObjectRef<Self::Class>> {
        Ok(*self)
    }
}

impl<T: GodotClass> EngineObject for Base<T> {
    type Class = T;

    fn __engine_object(&self) -> EngineResult<ObjectRef<Self::Class>> {
        current_object()
    }
}

impl<T: GodotClass> EngineObject for GodotRef<T> {
    type Class = T;

    fn __engine_object(&self) -> EngineResult<ObjectRef<Self::Class>> {
        Ok(self.object)
    }
}

/// Returns the Godot object that owns the currently executing Rust script.
#[doc(hidden)]
pub fn current_object<T: GodotClass>() -> EngineResult<ObjectRef<T>> {
    crate::module::current_owner_id().map(ObjectRef::__from_instance_id)
}

/// Converts one generated method argument to the stable project ABI.
#[doc(hidden)]
pub trait EngineArgument {
    fn __into_engine_argument(self) -> AbiValueV1;
}

/// Decodes one generated method return from the stable project ABI.
#[doc(hidden)]
pub trait EngineReturn: Sized {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self>;

    #[doc(hidden)]
    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, owned_ref, dynamic, callable) = value.into_parts();
        if owned_ref.is_some() || dynamic.is_some() || callable.is_some() {
            return Err(EngineError::invalid_result(
                "Native Godot API returned unexpected owned result storage",
            ));
        }
        Self::__from_engine_return(abi)
    }

    #[doc(hidden)]
    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        Self::__from_engine_return(value.abi())
    }
}

/// Portable receiver representation used by generated builtin APIs.
///
/// Implementations encode Rust-owned values into the stable project ABI and
/// replace mutable receivers only after the Host returned a complete owned
/// snapshot.
#[doc(hidden)]
pub trait EngineBuiltinValue: EngineReturn {
    fn __as_builtin_argument(&self) -> AbiValueV1;

    fn __replace_native_builtin(&mut self, value: AbiValueV1) -> EngineResult<()> {
        *self = Self::__from_engine_return(value)?;
        Ok(())
    }

    fn __replace_native_dynamic(&mut self, _value: Variant) -> EngineResult<()> {
        Err(EngineError::invalid_result(
            "Native Godot API returned a dynamic receiver for a non-dynamic builtin",
        ))
    }

    fn __replace_builtin(&mut self, value: crate::module::HostMethodValue) -> EngineResult<()> {
        *self = Self::__from_host_return(value)?;
        Ok(())
    }
}

/// Integer representation shared by generated Godot enums and bitfields.
///
/// Godot may add enum values in a later compatible engine release, so the
/// generated public types are transparent integer newtypes instead of closed
/// Rust enums.
#[doc(hidden)]
pub trait GodotIntegerValue: Copy {
    const SIGNED: bool;
    const PROPERTY_OPTIONS: &'static [godot_rs_api::abi::AbiGodotIntegerOptionV1];
    const PROPERTY_DEFAULT_RAW: u64;

    fn __raw(self) -> u64;
    fn __from_raw(raw: u64) -> Self;
}

impl<T: GodotIntegerValue> EngineArgument for T {
    fn __into_engine_argument(self) -> AbiValueV1 {
        if T::SIGNED {
            AbiValueV1::from_i64(self.__raw() as i64)
        } else {
            AbiValueV1::from_u64(self.__raw())
        }
    }
}

impl<T: GodotIntegerValue> EngineReturn for T {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let expected = if T::SIGNED {
            AbiValueType::I64
        } else {
            AbiValueType::U64
        };
        valid_value(value, expected)?;
        Ok(Self::__from_raw(value.payload[0]))
    }
}

macro_rules! define_godot_enum {
    (
        $(#[$type_meta:meta])*
        pub struct $name:ident {
            $(#[$first_value_meta:meta])*
            $first_value_name:ident = $first_value:expr
            $(
                , $(#[$value_meta:meta])*
                $value_name:ident = $value:expr
            )* $(,)?
        }
    ) => {
        $(#[$type_meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(i64);

        #[allow(non_upper_case_globals)]
        impl $name {
            $(#[$first_value_meta])*
            pub const $first_value_name: Self = Self($first_value);
            $(
                $(#[$value_meta])*
                pub const $value_name: Self = Self($value);
            )*

            /// Preserves an integer value even when it was added by a newer
            /// compatible Godot release.
            #[must_use]
            pub const fn from_ord(value: i64) -> Self {
                Self(value)
            }

            /// Returns the integer value used by Godot.
            #[must_use]
            pub const fn ord(self) -> i64 {
                self.0
            }
        }

        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self::from_ord(value)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                value.ord()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self($first_value)
            }
        }

        impl $crate::engine::GodotIntegerValue for $name {
            const SIGNED: bool = true;
            const PROPERTY_OPTIONS: &'static [$crate::abi::AbiGodotIntegerOptionV1] = &[
                $crate::abi::AbiGodotIntegerOptionV1 {
                    name: $crate::abi::AbiByteSlice::from_static(stringify!($first_value_name)),
                    raw: $first_value as u64,
                },
                $(
                    $crate::abi::AbiGodotIntegerOptionV1 {
                        name: $crate::abi::AbiByteSlice::from_static(stringify!($value_name)),
                        raw: $value as u64,
                    },
                )*
            ];
            const PROPERTY_DEFAULT_RAW: u64 = $first_value as u64;

            fn __raw(self) -> u64 {
                self.0 as u64
            }

            fn __from_raw(raw: u64) -> Self {
                Self(raw as i64)
            }
        }

        impl $crate::native::value::private::Sealed for $name {}

        impl $crate::native::GodotValue for $name {
            const __VARIANT_TYPE: Option<$crate::native::sys::GDExtensionVariantType> = Some(
                $crate::native::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
            );
        }

        impl $crate::native::value::GodotValueAbi for $name {
            const VARIANT_TYPE: Option<$crate::native::sys::GDExtensionVariantType> = Some(
                $crate::native::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
            );

            unsafe fn from_variant(
                interface: &$crate::native::runtime::Interface,
                value: $crate::native::sys::GDExtensionConstVariantPtr,
            ) -> Self {
                // SAFETY: ClassDB validated the dynamic argument type.
                unsafe { $crate::native::value::integer_from_variant(interface, value) }
            }

            unsafe fn write_variant(
                self,
                interface: &$crate::native::runtime::Interface,
                destination: $crate::native::sys::GDExtensionVariantPtr,
            ) {
                // SAFETY: ClassDB allocated the declared integer return slot.
                unsafe {
                    $crate::native::value::integer_write_variant(
                        self,
                        interface,
                        destination,
                    )
                };
            }

            unsafe fn from_ptr(
                _interface: &$crate::native::runtime::Interface,
                value: $crate::native::sys::GDExtensionConstTypePtr,
            ) -> Self {
                // SAFETY: Ptrcall metadata guarantees integer storage.
                unsafe { $crate::native::value::integer_from_ptr(value) }
            }

            unsafe fn write_ptr(
                self,
                _interface: &$crate::native::runtime::Interface,
                destination: $crate::native::sys::GDExtensionTypePtr,
            ) {
                // SAFETY: Ptrcall metadata guarantees integer storage.
                unsafe { $crate::native::value::integer_write_ptr(self, destination) };
            }
        }
    };
}

macro_rules! define_godot_bitfield {
    (
        $(#[$type_meta:meta])*
        pub struct $name:ident {
            $(#[$first_value_meta:meta])*
            $first_value_name:ident = $first_value:expr
            $(
                , $(#[$value_meta:meta])*
                $value_name:ident = $value:expr
            )* $(,)?
        }
    ) => {
        $(#[$type_meta])*
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(u64);

        #[allow(non_upper_case_globals)]
        impl $name {
            $(#[$first_value_meta])*
            pub const $first_value_name: Self = Self($first_value);
            $(
                $(#[$value_meta])*
                pub const $value_name: Self = Self($value);
            )*

            /// Returns an empty set of flags.
            #[must_use]
            pub const fn empty() -> Self {
                Self(0)
            }

            /// Preserves every bit, including flags added by a newer
            /// compatible Godot release.
            #[must_use]
            pub const fn from_bits_retain(bits: u64) -> Self {
                Self(bits)
            }

            /// Returns the raw bit set used by Godot.
            #[must_use]
            pub const fn bits(self) -> u64 {
                self.0
            }

            /// Whether no flags are set.
            #[must_use]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// Whether every flag in `other` is set.
            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            /// Whether at least one flag in `other` is set.
            #[must_use]
            pub const fn intersects(self, other: Self) -> bool {
                self.0 & other.0 != 0
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self::from_bits_retain(value)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.bits()
            }
        }

        impl core::ops::BitOr for $name {
            type Output = Self;

            fn bitor(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }
        }

        impl core::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, other: Self) {
                self.0 |= other.0;
            }
        }

        impl core::ops::BitAnd for $name {
            type Output = Self;

            fn bitand(self, other: Self) -> Self {
                Self(self.0 & other.0)
            }
        }

        impl core::ops::BitAndAssign for $name {
            fn bitand_assign(&mut self, other: Self) {
                self.0 &= other.0;
            }
        }

        impl core::ops::BitXor for $name {
            type Output = Self;

            fn bitxor(self, other: Self) -> Self {
                Self(self.0 ^ other.0)
            }
        }

        impl core::ops::BitXorAssign for $name {
            fn bitxor_assign(&mut self, other: Self) {
                self.0 ^= other.0;
            }
        }

        impl core::ops::Not for $name {
            type Output = Self;

            fn not(self) -> Self {
                Self(!self.0)
            }
        }

        impl $crate::engine::GodotIntegerValue for $name {
            const SIGNED: bool = false;
            const PROPERTY_OPTIONS: &'static [$crate::abi::AbiGodotIntegerOptionV1] = &[
                $crate::abi::AbiGodotIntegerOptionV1 {
                    name: $crate::abi::AbiByteSlice::from_static(stringify!($first_value_name)),
                    raw: $first_value as u64,
                },
                $(
                    $crate::abi::AbiGodotIntegerOptionV1 {
                        name: $crate::abi::AbiByteSlice::from_static(stringify!($value_name)),
                        raw: $value as u64,
                    },
                )*
            ];
            const PROPERTY_DEFAULT_RAW: u64 = 0;

            fn __raw(self) -> u64 {
                self.0
            }

            fn __from_raw(raw: u64) -> Self {
                Self(raw)
            }
        }

        impl $crate::native::value::private::Sealed for $name {}

        impl $crate::native::GodotValue for $name {
            const __VARIANT_TYPE: Option<$crate::native::sys::GDExtensionVariantType> = Some(
                $crate::native::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
            );
        }

        impl $crate::native::value::GodotValueAbi for $name {
            const VARIANT_TYPE: Option<$crate::native::sys::GDExtensionVariantType> = Some(
                $crate::native::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
            );

            unsafe fn from_variant(
                interface: &$crate::native::runtime::Interface,
                value: $crate::native::sys::GDExtensionConstVariantPtr,
            ) -> Self {
                // SAFETY: ClassDB validated the dynamic argument type.
                unsafe { $crate::native::value::integer_from_variant(interface, value) }
            }

            unsafe fn write_variant(
                self,
                interface: &$crate::native::runtime::Interface,
                destination: $crate::native::sys::GDExtensionVariantPtr,
            ) {
                // SAFETY: ClassDB allocated the declared integer return slot.
                unsafe {
                    $crate::native::value::integer_write_variant(
                        self,
                        interface,
                        destination,
                    )
                };
            }

            unsafe fn from_ptr(
                _interface: &$crate::native::runtime::Interface,
                value: $crate::native::sys::GDExtensionConstTypePtr,
            ) -> Self {
                // SAFETY: Ptrcall metadata guarantees integer storage.
                unsafe { $crate::native::value::integer_from_ptr(value) }
            }

            unsafe fn write_ptr(
                self,
                _interface: &$crate::native::runtime::Interface,
                destination: $crate::native::sys::GDExtensionTypePtr,
            ) {
                // SAFETY: Ptrcall metadata guarantees integer storage.
                unsafe { $crate::native::value::integer_write_ptr(self, destination) };
            }
        }
    };
}

impl EngineArgument for bool {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_bool(self)
    }
}

impl EngineReturn for bool {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::BOOL)?;
        match value.payload[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid_return()),
        }
    }
}

macro_rules! copy_builtin_value {
    ($($type:ty),+ $(,)?) => {
        $(
            impl EngineBuiltinValue for $type {
                fn __as_builtin_argument(&self) -> AbiValueV1 {
                    (*self).__into_engine_argument()
                }
            }
        )+
    };
}

macro_rules! signed_engine_value {
    ($($type:ty),+ $(,)?) => {
        $(
            impl EngineArgument for $type {
                fn __into_engine_argument(self) -> AbiValueV1 {
                    AbiValueV1::from_i64(i64::from(self))
                }
            }

            impl EngineReturn for $type {
                fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
                    valid_value(value, AbiValueType::I64)?;
                    Self::try_from(value.payload[0] as i64).map_err(|_| invalid_return())
                }
            }
        )+
    };
}

signed_engine_value!(i8, i16, i32, i64);

macro_rules! unsigned_engine_value {
    ($($type:ty),+ $(,)?) => {
        $(
            impl EngineArgument for $type {
                fn __into_engine_argument(self) -> AbiValueV1 {
                    AbiValueV1::from_u64(u64::from(self))
                }
            }

            impl EngineReturn for $type {
                fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
                    valid_value(value, AbiValueType::U64)?;
                    Self::try_from(value.payload[0]).map_err(|_| invalid_return())
                }
            }
        )+
    };
}

unsigned_engine_value!(u8, u16, u32, u64);

impl EngineArgument for char {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_u64(u64::from(u32::from(self)))
    }
}

impl EngineReturn for char {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::U64)?;
        u32::try_from(value.payload[0])
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(invalid_return)
    }
}

impl EngineArgument for f32 {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_f64(f64::from(self))
    }
}

impl EngineReturn for f32 {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::F64)?;
        let value = f64::from_bits(value.payload[0]);
        let narrowed = value as f32;
        if value.is_finite() && !narrowed.is_finite() {
            Err(invalid_return())
        } else {
            Ok(narrowed)
        }
    }
}

impl EngineArgument for f64 {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_f64(self)
    }
}

impl EngineReturn for f64 {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::F64)?;
        Ok(f64::from_bits(value.payload[0]))
    }
}

copy_builtin_value!(bool, i8, i16, i32, i64, u8, u16, u32, u64, char, f32, f64);

impl EngineArgument for &str {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_borrowed_utf8(self)
    }
}

/// Encodes a generated Godot `StringName` argument without exposing ABI details.
#[doc(hidden)]
#[must_use]
pub fn string_name_argument(value: &str) -> AbiValueV1 {
    AbiValueV1::from_borrowed_string_name(value)
}

/// Encodes a generated Godot `NodePath` argument without exposing ABI details.
#[doc(hidden)]
#[must_use]
pub fn node_path_argument(value: &str) -> AbiValueV1 {
    AbiValueV1::from_borrowed_node_path(value)
}

impl EngineReturn for String {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        owned_engine_text(value, AbiValueType::STRING)
    }
}

impl EngineReturn for StringName {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        owned_engine_text(value, AbiValueType::STRING_NAME).map(Self::from)
    }
}

impl EngineReturn for NodePath {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        owned_engine_text(value, AbiValueType::NODE_PATH).map(Self::from)
    }
}

impl EngineBuiltinValue for String {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        self.as_str().__into_engine_argument()
    }
}

impl EngineBuiltinValue for StringName {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        string_name_argument(self.as_str())
    }
}

impl EngineBuiltinValue for NodePath {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        node_path_argument(self.as_str())
    }
}

impl EngineArgument for Vector2 {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_vector2(self.x, self.y)
    }
}

impl EngineReturn for Vector2 {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y] = value.vector2().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y))
    }
}

impl EngineArgument for Vector3 {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_vector3(self.x, self.y, self.z)
    }
}

impl EngineReturn for Vector3 {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, z] = value.vector3().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y, z))
    }
}

impl EngineArgument for Color {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_color(self.r, self.g, self.b, self.a)
    }
}

impl EngineReturn for Color {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [r, g, b, a] = value.color().ok_or_else(invalid_return)?;
        Ok(Self::rgba(r, g, b, a))
    }
}

impl EngineArgument for Vector2i {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_vector2i(self.x, self.y)
    }
}

impl EngineReturn for Vector2i {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y] = value.vector2i().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y))
    }
}

impl EngineArgument for Vector3i {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_vector3i(self.x, self.y, self.z)
    }
}

impl EngineReturn for Vector3i {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, z] = value.vector3i().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y, z))
    }
}

impl EngineArgument for Rect2 {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_rect2(self.position.x, self.position.y, self.size.x, self.size.y)
    }
}

impl EngineReturn for Rect2 {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, width, height] = value.rect2().ok_or_else(invalid_return)?;
        Ok(Self::from_components(x, y, width, height))
    }
}

impl EngineArgument for Rect2i {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_rect2i(self.position.x, self.position.y, self.size.x, self.size.y)
    }
}

impl EngineReturn for Rect2i {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, width, height] = value.rect2i().ok_or_else(invalid_return)?;
        Ok(Self::from_components(x, y, width, height))
    }
}

impl EngineArgument for Quaternion {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_quaternion(self.x, self.y, self.z, self.w)
    }
}

impl EngineReturn for Quaternion {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, z, w] = value.quaternion().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y, z, w))
    }
}

impl EngineArgument for Plane {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_plane(self.normal.x, self.normal.y, self.normal.z, self.d)
    }
}

impl EngineReturn for Plane {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, z, d] = value.plane().ok_or_else(invalid_return)?;
        Ok(Self::from_components(x, y, z, d))
    }
}

impl EngineArgument for Vector4 {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_vector4(self.x, self.y, self.z, self.w)
    }
}

impl EngineReturn for Vector4 {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, z, w] = value.vector4().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y, z, w))
    }
}

impl EngineArgument for Vector4i {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_vector4i(self.x, self.y, self.z, self.w)
    }
}

impl EngineReturn for Vector4i {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let [x, y, z, w] = value.vector4i().ok_or_else(invalid_return)?;
        Ok(Self::new(x, y, z, w))
    }
}

copy_builtin_value!(
    Vector2, Vector2i, Vector3, Vector3i, Vector4, Vector4i, Rect2, Rect2i, Quaternion, Plane,
    Color
);

macro_rules! fixed_math_engine_value {
    ($type:ty, $abi_type:ident, $components:expr, $count:expr) => {
        impl EngineArgument for &$type {
            fn __into_engine_argument(self) -> AbiValueV1 {
                AbiValueV1::from_borrowed_f32_components(
                    AbiValueType::$abi_type,
                    self.__components(),
                )
            }
        }

        impl EngineReturn for $type {
            fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
                fixed_f32_components::<$count>(value, AbiValueType::$abi_type).map($components)
            }
        }
    };
}

fixed_math_engine_value!(Transform2D, TRANSFORM2D, Transform2D::__from_components, 6);
fixed_math_engine_value!(Aabb, AABB, Aabb::__from_components, 6);
fixed_math_engine_value!(Basis, BASIS, Basis::__from_components, 9);
fixed_math_engine_value!(Transform3D, TRANSFORM3D, Transform3D::__from_components, 12);
fixed_math_engine_value!(Projection, PROJECTION, Projection::__from_components, 16);

macro_rules! borrowed_builtin_value {
    ($($type:ty),+ $(,)?) => {
        $(
            impl EngineBuiltinValue for $type {
                fn __as_builtin_argument(&self) -> AbiValueV1 {
                    self.__into_engine_argument()
                }
            }
        )+
    };
}

borrowed_builtin_value!(Transform2D, Aabb, Basis, Transform3D, Projection);

macro_rules! packed_engine_value {
    ($type:ty, $abi_type:ident) => {
        impl EngineArgument for &$type {
            fn __into_engine_argument(self) -> AbiValueV1 {
                AbiValueV1::from_borrowed_bytes(AbiValueType::$abi_type, self.__bytes())
            }
        }

        impl EngineReturn for $type {
            fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
                let bytes = copy_owned_engine_bytes(value, AbiValueType::$abi_type)?;
                Self::__from_bytes(&bytes).ok_or_else(invalid_return)
            }
        }
    };
}

packed_engine_value!(PackedByteArray, PACKED_BYTE_ARRAY);
packed_engine_value!(PackedInt32Array, PACKED_INT32_ARRAY);
packed_engine_value!(PackedInt64Array, PACKED_INT64_ARRAY);
packed_engine_value!(PackedFloat32Array, PACKED_FLOAT32_ARRAY);
packed_engine_value!(PackedFloat64Array, PACKED_FLOAT64_ARRAY);
packed_engine_value!(PackedStringArray, PACKED_STRING_ARRAY);
packed_engine_value!(PackedVector2Array, PACKED_VECTOR2_ARRAY);
packed_engine_value!(PackedVector3Array, PACKED_VECTOR3_ARRAY);
packed_engine_value!(PackedColorArray, PACKED_COLOR_ARRAY);
packed_engine_value!(PackedVector4Array, PACKED_VECTOR4_ARRAY);

borrowed_builtin_value!(
    PackedByteArray,
    PackedInt32Array,
    PackedInt64Array,
    PackedFloat32Array,
    PackedFloat64Array,
    PackedStringArray,
    PackedVector2Array,
    PackedVector3Array,
    PackedColorArray,
    PackedVector4Array
);

fn dynamic_engine_argument(
    type_: AbiValueType,
    bytes: Result<&[u8], crate::variant::VariantError>,
) -> AbiValueV1 {
    bytes.map_or(
        AbiValueV1 {
            type_,
            reserved_flags: 0,
            payload: [0; 2],
        },
        |bytes| AbiValueV1::from_borrowed_bytes(type_, bytes),
    )
}

impl EngineArgument for &Variant {
    fn __into_engine_argument(self) -> AbiValueV1 {
        dynamic_engine_argument(AbiValueType::VARIANT, self.__bytes())
    }
}

impl EngineReturn for Variant {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value, AbiValueType::VARIANT)?;
        Self::__from_bytes(&bytes).ok_or_else(invalid_return)
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value.abi(), AbiValueType::VARIANT)?;
        let token = godot_rs_api::abi::dynamic_value_ownership_token(&bytes).ok_or_else(|| {
            EngineError::invalid_result("Godot returned a Variant without Host ownership")
        })?;
        let ownership = crate::module::retain_dynamic_value(AbiValueType::VARIANT, token)?;
        Self::__from_host_bytes(&bytes, ownership).ok_or_else(invalid_return)
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, owned_ref, dynamic, callable) = value.into_parts();
        if abi.type_ != AbiValueType::VARIANT || owned_ref.is_some() || callable.is_some() {
            return Err(invalid_return());
        }
        dynamic.ok_or_else(invalid_return)
    }
}

impl EngineBuiltinValue for Variant {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        self.__into_engine_argument()
    }

    fn __replace_native_dynamic(&mut self, value: Variant) -> EngineResult<()> {
        *self = value;
        Ok(())
    }
}

impl<T: VariantConvert> EngineArgument for &Array<T> {
    fn __into_engine_argument(self) -> AbiValueV1 {
        dynamic_engine_argument(AbiValueType::ARRAY, self.__bytes())
    }
}

impl<T: VariantConvert> EngineReturn for Array<T> {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value, AbiValueType::ARRAY)?;
        Self::__from_bytes(&bytes).ok_or_else(invalid_return)
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value.abi(), AbiValueType::ARRAY)?;
        let token = godot_rs_api::abi::dynamic_value_ownership_token(&bytes).ok_or_else(|| {
            EngineError::invalid_result("Godot returned an Array without Host ownership")
        })?;
        let ownership = crate::module::retain_dynamic_value(AbiValueType::ARRAY, token)?;
        Self::__from_host_bytes(&bytes, ownership).ok_or_else(invalid_return)
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, owned_ref, dynamic, callable) = value.into_parts();
        if abi.type_ != AbiValueType::ARRAY || owned_ref.is_some() || callable.is_some() {
            return Err(invalid_return());
        }
        dynamic
            .and_then(<Self as VariantConvert>::from_variant)
            .ok_or_else(invalid_return)
    }
}

impl<T: VariantConvert> EngineBuiltinValue for Array<T> {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        self.__into_engine_argument()
    }

    fn __replace_native_dynamic(&mut self, value: Variant) -> EngineResult<()> {
        *self = <Self as VariantConvert>::from_variant(value).ok_or_else(invalid_return)?;
        Ok(())
    }
}

impl EngineArgument for &Dictionary {
    fn __into_engine_argument(self) -> AbiValueV1 {
        dynamic_engine_argument(AbiValueType::DICTIONARY, self.__bytes())
    }
}

impl EngineReturn for Dictionary {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value, AbiValueType::DICTIONARY)?;
        Self::__from_bytes(&bytes).ok_or_else(invalid_return)
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value.abi(), AbiValueType::DICTIONARY)?;
        let token = godot_rs_api::abi::dynamic_value_ownership_token(&bytes).ok_or_else(|| {
            EngineError::invalid_result("Godot returned a Dictionary without Host ownership")
        })?;
        let ownership = crate::module::retain_dynamic_value(AbiValueType::DICTIONARY, token)?;
        Self::__from_host_bytes(&bytes, ownership).ok_or_else(invalid_return)
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, owned_ref, dynamic, callable) = value.into_parts();
        if abi.type_ != AbiValueType::DICTIONARY || owned_ref.is_some() || callable.is_some() {
            return Err(invalid_return());
        }
        dynamic
            .and_then(<Self as VariantConvert>::from_variant)
            .ok_or_else(invalid_return)
    }
}

impl EngineBuiltinValue for Dictionary {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        self.__into_engine_argument()
    }

    fn __replace_native_dynamic(&mut self, value: Variant) -> EngineResult<()> {
        *self = <Self as VariantConvert>::from_variant(value).ok_or_else(invalid_return)?;
        Ok(())
    }
}

impl EngineArgument for &Callable {
    fn __into_engine_argument(self) -> AbiValueV1 {
        self.__bytes().map_or(
            AbiValueV1 {
                type_: AbiValueType::CALLABLE,
                reserved_flags: 0,
                payload: [0; 2],
            },
            |bytes| AbiValueV1::from_borrowed_bytes(AbiValueType::CALLABLE, bytes),
        )
    }
}

impl EngineReturn for Callable {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let (pointer, length) = value
            .byte_range(AbiValueType::CALLABLE)
            .ok_or_else(invalid_return)?;
        if value.reserved_flags != 0 || length > MAX_ENGINE_TEXT_BYTES {
            return Err(invalid_return());
        }
        // SAFETY: Extension Mode retains this synchronous result while
        // the generated wrapper decodes it.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        Self::__from_bytes(bytes).ok_or_else(invalid_return)
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value.abi(), AbiValueType::CALLABLE)?;
        let token = godot_rs_api::abi::callable_value_ownership_token(&bytes).ok_or_else(|| {
            EngineError::invalid_result("Godot returned a Callable without Host ownership")
        })?;
        let ownership = crate::module::retain_callable_value(token)?;
        Self::__from_host_bytes(&bytes, ownership).ok_or_else(invalid_return)
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, owned_ref, dynamic, callable) = value.into_parts();
        if abi.type_ != AbiValueType::CALLABLE || owned_ref.is_some() || dynamic.is_some() {
            return Err(invalid_return());
        }
        callable.ok_or_else(invalid_return)
    }
}

impl EngineBuiltinValue for Callable {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        self.__into_engine_argument()
    }
}

impl<T> EngineArgument for &Signal<T> {
    fn __into_engine_argument(self) -> AbiValueV1 {
        self.__bytes().map_or(
            AbiValueV1 {
                type_: AbiValueType::SIGNAL,
                reserved_flags: 0,
                payload: [0; 2],
            },
            |bytes| AbiValueV1::from_borrowed_bytes(AbiValueType::SIGNAL, bytes),
        )
    }
}

impl<T> EngineReturn for Signal<T> {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        let (pointer, length) = value
            .byte_range(AbiValueType::SIGNAL)
            .ok_or_else(invalid_return)?;
        if value.reserved_flags != 0 || length > MAX_ENGINE_TEXT_BYTES {
            return Err(invalid_return());
        }
        // SAFETY: Extension Mode retains this synchronous result while
        // the generated wrapper decodes it.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        Self::__from_bytes(bytes).ok_or_else(invalid_return)
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        let bytes = copy_owned_engine_bytes(value.abi(), AbiValueType::SIGNAL)?;
        Self::__from_bytes(&bytes).ok_or_else(invalid_return)
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, owned_ref, dynamic, callable) = value.into_parts();
        if owned_ref.is_some() || dynamic.is_some() || callable.is_some() {
            return Err(invalid_return());
        }
        let bytes = copy_owned_engine_bytes(abi, AbiValueType::SIGNAL)?;
        Self::__from_bytes(&bytes).ok_or_else(invalid_return)
    }
}

impl<T> EngineBuiltinValue for Signal<T> {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        self.__into_engine_argument()
    }
}

impl EngineArgument for Rid {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_rid(self.id())
    }
}

impl EngineReturn for Rid {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        value.rid().map(Rid::from_raw).ok_or_else(invalid_return)
    }
}

impl EngineBuiltinValue for Rid {
    fn __as_builtin_argument(&self) -> AbiValueV1 {
        (*self).__into_engine_argument()
    }
}

impl<T: GodotClass> EngineArgument for ObjectRef<T> {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_object_id(self.raw)
    }
}

impl<T: GodotClass> EngineArgument for Option<ObjectRef<T>> {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_object_id(self.map_or(0, ObjectRef::instance_id))
    }
}

impl<T: GodotClass> EngineArgument for &GodotRef<T> {
    fn __into_engine_argument(self) -> AbiValueV1 {
        AbiValueV1::from_object_id(self.instance_id())
    }
}

impl<T: GodotClass> EngineReturn for ObjectRef<T> {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::OBJECT_ID)?;
        if value.payload[0] == 0 {
            return Err(EngineError::invalid_result(
                "Godot returned null for a non-null object result",
            ));
        }
        Ok(Self::__from_instance_id(value.payload[0]))
    }
}

impl<T: GodotClass> EngineReturn for Option<ObjectRef<T>> {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::OBJECT_ID)?;
        Ok((value.payload[0] != 0).then(|| ObjectRef::__from_instance_id(value.payload[0])))
    }
}

impl<T: GodotClass> EngineReturn for GodotRef<T> {
    fn __from_engine_return(_value: AbiValueV1) -> EngineResult<Self> {
        Err(EngineError::invalid_result(
            "Godot returned a RefCounted object without ownership",
        ))
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        let Some((object_id, ownership)) = value.into_owned_object_ref()? else {
            return Err(EngineError::invalid_result(
                "Godot returned null for a non-null RefCounted result",
            ));
        };
        Ok(Self::from_owned_parts(object_id, ownership))
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, ownership, dynamic, callable) = value.into_parts();
        if dynamic.is_some() || callable.is_some() {
            return Err(invalid_return());
        }
        valid_value(abi, AbiValueType::OBJECT_ID)?;
        if abi.payload[0] == 0 {
            return Err(EngineError::invalid_result(
                "Godot returned null for a non-null RefCounted result",
            ));
        }
        let ownership = ownership.ok_or_else(|| {
            EngineError::invalid_result("Native RefCounted result omitted ownership")
        })?;
        Ok(Self::from_native_parts(abi.payload[0], ownership))
    }
}

impl<T: GodotClass> EngineReturn for Option<GodotRef<T>> {
    fn __from_engine_return(_value: AbiValueV1) -> EngineResult<Self> {
        Err(EngineError::invalid_result(
            "Godot returned a RefCounted object without ownership",
        ))
    }

    fn __from_host_return(value: crate::module::HostMethodValue) -> EngineResult<Self> {
        Ok(value
            .into_owned_object_ref()?
            .map(|(object_id, ownership)| GodotRef::from_owned_parts(object_id, ownership)))
    }

    fn __from_native_return(value: crate::native::NativeEngineValue) -> EngineResult<Self> {
        let (abi, ownership, dynamic, callable) = value.into_parts();
        if dynamic.is_some() || callable.is_some() {
            return Err(invalid_return());
        }
        valid_value(abi, AbiValueType::OBJECT_ID)?;
        if abi.payload[0] == 0 {
            if ownership.is_some() {
                return Err(EngineError::invalid_result(
                    "Native null RefCounted result carried ownership",
                ));
            }
            return Ok(None);
        }
        let ownership = ownership.ok_or_else(|| {
            EngineError::invalid_result("Native RefCounted result omitted ownership")
        })?;
        Ok(Some(GodotRef::from_native_parts(abi.payload[0], ownership)))
    }
}

impl EngineReturn for () {
    fn __from_engine_return(value: AbiValueV1) -> EngineResult<Self> {
        valid_value(value, AbiValueType::NIL)
    }
}

/// Invokes one statically generated Godot MethodBind contract.
#[doc(hidden)]
pub fn invoke_engine_method<T: GodotClass, R: EngineReturn>(
    receiver: ObjectRef<T>,
    method: &'static AbiGodotMethodSpecV1,
    arguments: &[AbiValueV1],
) -> EngineResult<R> {
    if let Some(result) = crate::native::invoke_engine_method(receiver.raw, method, arguments) {
        return decode_native_engine_value(result?);
    }
    let value = crate::module::call_godot_method(receiver.raw, method, arguments)?;
    R::__from_host_return(value)
}

/// Invokes a generated receiver-free utility, constructor, singleton, or
/// static builtin contract.
#[doc(hidden)]
pub fn invoke_godot_api<R: EngineReturn>(
    spec: &'static AbiGodotApiSpecV1,
    arguments: &[AbiValueV1],
) -> EngineResult<R> {
    if let Some(result) = crate::native::invoke_godot_api(spec, None, arguments, false) {
        let (output, updated) = result?;
        debug_assert!(updated.is_none());
        return decode_native_engine_value(output);
    }
    let (output, updated) = crate::module::call_godot_api(spec, None, arguments, false)?;
    debug_assert!(updated.is_none());
    R::__from_host_return(output)
}

/// Invokes a generated const builtin method, operator, member, or index.
#[doc(hidden)]
pub fn invoke_builtin_api<B: EngineBuiltinValue, R: EngineReturn>(
    base: &B,
    spec: &'static AbiGodotApiSpecV1,
    arguments: &[AbiValueV1],
) -> EngineResult<R> {
    if let Some(result) =
        crate::native::invoke_godot_api(spec, Some(base.__as_builtin_argument()), arguments, false)
    {
        let (output, updated) = result?;
        debug_assert!(updated.is_none());
        return decode_native_engine_value(output);
    }
    let (output, updated) =
        crate::module::call_godot_api(spec, Some(base.__as_builtin_argument()), arguments, false)?;
    debug_assert!(updated.is_none());
    R::__from_host_return(output)
}

/// Invokes a generated builtin operation that mutates its receiver.
#[doc(hidden)]
pub fn invoke_builtin_api_mut<B: EngineBuiltinValue, R: EngineReturn>(
    base: &mut B,
    spec: &'static AbiGodotApiSpecV1,
    arguments: &[AbiValueV1],
) -> EngineResult<R> {
    if let Some(result) =
        crate::native::invoke_godot_api(spec, Some(base.__as_builtin_argument()), arguments, true)
    {
        let (output, updated) = result?;
        let result = decode_native_engine_value(output)?;
        let updated = updated.ok_or_else(|| {
            EngineError::invalid_result("Native Godot API omitted a mutable builtin receiver")
        })?;
        let (updated, owned_ref, dynamic, callable) = updated.into_parts();
        if owned_ref.is_some() || callable.is_some() {
            return Err(EngineError::invalid_result(
                "Native mutable builtin receiver carried RefCounted ownership",
            ));
        }
        let replace = if let Some(dynamic) = dynamic {
            base.__replace_native_dynamic(dynamic)
        } else {
            base.__replace_native_builtin(updated)
        };
        release_native_engine_value(updated)?;
        replace?;
        return Ok(result);
    }
    let (output, updated) =
        crate::module::call_godot_api(spec, Some(base.__as_builtin_argument()), arguments, true)?;
    let result = R::__from_host_return(output)?;
    let updated = updated.ok_or_else(|| {
        EngineError::invalid_result("the godot-rust Host omitted a mutable builtin receiver")
    })?;
    base.__replace_builtin(updated)?;
    Ok(result)
}

fn decode_native_engine_value<R: EngineReturn>(
    value: crate::native::NativeEngineValue,
) -> EngineResult<R> {
    let abi = value.abi();
    let decoded = R::__from_native_return(value);
    release_native_engine_value(abi)?;
    decoded
}

fn release_native_engine_value(value: AbiValueV1) -> EngineResult<()> {
    if !matches!(
        value.reserved_flags,
        godot_rs_api::abi::ABI_VALUE_OWNED_UTF8 | godot_rs_api::abi::ABI_VALUE_OWNED_BYTES
    ) {
        return Ok(());
    }
    // SAFETY: Native engine calls allocate these values through the same SDK
    // module allocator immediately before returning them.
    let status = unsafe { crate::module::drop_native_engine_value(value) };
    if status == godot_rs_api::abi::AbiStatus::Ok {
        Ok(())
    } else {
        Err(EngineError::invalid_result(
            "Native engine return allocation could not be released",
        ))
    }
}

fn valid_value(value: AbiValueV1, expected: AbiValueType) -> EngineResult<()> {
    if value.type_ == expected && value.reserved_flags == 0 && value.payload[1] == 0 {
        Ok(())
    } else {
        Err(invalid_return())
    }
}

fn fixed_f32_components<const N: usize>(
    value: AbiValueV1,
    expected: AbiValueType,
) -> EngineResult<[f32; N]> {
    let (pointer, length) = value.byte_range(expected).ok_or_else(invalid_return)?;
    if length != N * core::mem::size_of::<f32>() {
        return Err(invalid_return());
    }
    // SAFETY: The Host-owned range remains live for this return conversion,
    // and the exact bounded length was validated above.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    Ok(core::array::from_fn(|index| {
        let offset = index * 4;
        f32::from_ne_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("one f32 component has four bytes"),
        )
    }))
}

fn copy_owned_engine_bytes(value: AbiValueV1, expected: AbiValueType) -> EngineResult<Vec<u8>> {
    if value.reserved_flags != godot_rs_api::abi::ABI_VALUE_OWNED_BYTES {
        return Err(invalid_return());
    }
    let (pointer, length) = value.byte_range(expected).ok_or_else(invalid_return)?;
    if length > MAX_ENGINE_TEXT_BYTES {
        return Err(invalid_return());
    }
    // SAFETY: HostMethodValue retains this Host allocation until the return
    // conversion completes. The byte count is bounded before slice creation.
    Ok(unsafe { core::slice::from_raw_parts(pointer, length) }.to_vec())
}

fn owned_engine_text(value: AbiValueV1, expected: AbiValueType) -> EngineResult<String> {
    if value.type_ != expected || value.reserved_flags != godot_rs_api::abi::ABI_VALUE_OWNED_UTF8 {
        return Err(invalid_return());
    }
    let address = usize::try_from(value.payload[0]).map_err(|_| invalid_return())?;
    let length = usize::try_from(value.payload[1]).map_err(|_| invalid_return())?;
    if address == 0 || length > MAX_ENGINE_TEXT_BYTES {
        return Err(invalid_return());
    }
    // SAFETY: Host-owned return values stay live until `EngineReturn`
    // finishes; the ABI bounds the readable byte range before access.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| invalid_return())
}

fn invalid_return() -> EngineError {
    EngineError::invalid_result("the godot-rust Host returned a value with the wrong type")
}

/// Borrowed input-event handle valid only for one Godot callback.
///
/// The opaque value is a Godot Object instance ID. The project module never
/// receives the engine's native Object pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct InputEventRef {
    raw: u64,
}

impl InputEventRef {
    /// Builds a callback-scoped event handle from the module ABI value.
    #[doc(hidden)]
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self { raw }
    }

    /// Returns the opaque callback-scoped value for generated dispatch code.
    #[doc(hidden)]
    #[must_use]
    pub const fn into_raw(self) -> u64 {
        self.raw
    }
}

// Godot method names and arities are part of the engine API and cannot follow
// Rust's optional style lints without changing their public meaning.
#[allow(clippy::too_many_arguments, clippy::wrong_self_convention)]
#[rustfmt::skip]
mod generated_api {
    include!(concat!(env!("OUT_DIR"), "/engine_api.rs"));
}
pub use generated_api::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_class_set_generates_script_and_native_markers() {
        assert_eq!(AnimationPlayer::CLASS_NAME, "AnimationPlayer");
        assert_eq!(RichTextLabel::CLASS_NAME, "RichTextLabel");
        assert_eq!(
            <crate::native::classes::AnimationPlayer as crate::native::GodotClass>::CLASS_NAME,
            "AnimationPlayer"
        );
        assert_eq!(
            <crate::native::classes::RichTextLabel as crate::native::GodotClass>::CLASS_NAME,
            "RichTextLabel"
        );
    }

    #[test]
    fn object_references_are_copyable_ids_with_stale_safe_defaults() {
        let unresolved = ObjectRef::<Node>::unresolved();
        assert!(!unresolved.is_resolved());
        assert_eq!(unresolved.instance_id(), 0);

        let object = ObjectRef::<Node>::__from_instance_id(42);
        assert!(object.is_resolved());
        assert_eq!(object.instance_id(), 42);
        assert_eq!(object, object);
        assert!(format!("{object:?}").contains("instance_id: 42"));
    }

    #[test]
    fn normalized_engine_values_validate_exact_return_types() {
        assert!(bool::__from_engine_return(AbiValueV1::from_bool(true)).expect("bool"));
        assert_eq!(
            u32::__from_engine_return(AbiValueV1::from_u64(u32::MAX.into())).expect("u32"),
            u32::MAX
        );
        assert!(
            u8::__from_engine_return(AbiValueV1::from_u64(u16::from(u8::MAX) as u64 + 1)).is_err()
        );
        assert_eq!(
            char::__from_engine_return(AbiValueV1::from_u64(u64::from(u32::from('界'))))
                .expect("char32"),
            '界'
        );
        assert!(char::__from_engine_return(AbiValueV1::from_u64(0xD800)).is_err());
        assert!(ObjectRef::<Node>::__from_engine_return(AbiValueV1::from_object_id(0)).is_err());
        assert_eq!(
            Option::<ObjectRef<Node>>::__from_engine_return(AbiValueV1::from_object_id(0))
                .expect("nullable object"),
            None
        );
        assert_eq!(
            Vector2::__from_engine_return(AbiValueV1::from_vector2(1.5, -2.0)).expect("Vector2"),
            Vector2::new(1.5, -2.0)
        );
        assert_eq!(
            Vector3::__from_engine_return(AbiValueV1::from_vector3(1.0, 2.0, 3.0))
                .expect("Vector3"),
            Vector3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(
            Color::__from_engine_return(AbiValueV1::from_color(0.1, 0.2, 0.3, 0.4)).expect("Color"),
            Color::rgba(0.1, 0.2, 0.3, 0.4)
        );
        let text = "你好，Godot";
        let mut encoded = AbiValueV1::from_borrowed_utf8(text);
        encoded.reserved_flags = godot_rs_api::abi::ABI_VALUE_OWNED_UTF8;
        assert_eq!(String::__from_engine_return(encoded).expect("String"), text);
        encoded.type_ = AbiValueType::STRING_NAME;
        assert_eq!(
            StringName::__from_engine_return(encoded).expect("StringName"),
            StringName::from(text)
        );
        assert_eq!(
            string_name_argument(text),
            AbiValueV1::from_borrowed_string_name(text)
        );
        assert_eq!(
            Vector2i::__from_engine_return(AbiValueV1::from_vector2i(-1, 2)).expect("Vector2i"),
            Vector2i::new(-1, 2)
        );
        assert_eq!(
            Vector3i::__from_engine_return(AbiValueV1::from_vector3i(3, -4, 5)).expect("Vector3i"),
            Vector3i::new(3, -4, 5)
        );
        assert_eq!(
            Rect2::__from_engine_return(AbiValueV1::from_rect2(1.0, 2.0, 3.0, 4.0)).expect("Rect2"),
            Rect2::from_components(1.0, 2.0, 3.0, 4.0)
        );
        assert_eq!(
            Rect2i::__from_engine_return(AbiValueV1::from_rect2i(1, 2, 3, 4)).expect("Rect2i"),
            Rect2i::from_components(1, 2, 3, 4)
        );
        assert_eq!(
            Quaternion::__from_engine_return(AbiValueV1::from_quaternion(0.0, 0.5, 0.0, 1.0))
                .expect("Quaternion"),
            Quaternion::new(0.0, 0.5, 0.0, 1.0)
        );
        assert_eq!(
            Plane::__from_engine_return(AbiValueV1::from_plane(0.0, 1.0, 0.0, 5.0)).expect("Plane"),
            Plane::from_components(0.0, 1.0, 0.0, 5.0)
        );
        assert_eq!(
            Vector4::__from_engine_return(AbiValueV1::from_vector4(1.0, 2.0, 3.0, 4.0))
                .expect("Vector4"),
            Vector4::new(1.0, 2.0, 3.0, 4.0)
        );
        assert_eq!(
            Vector4i::__from_engine_return(AbiValueV1::from_vector4i(1, 2, 3, 4))
                .expect("Vector4i"),
            Vector4i::new(1, 2, 3, 4)
        );
    }

    #[test]
    fn generated_enums_and_bitfields_preserve_unknown_compatible_values() {
        assert_eq!(global::Error::OK.ord(), 0);
        let future_error = global::Error::from_ord(9_999);
        let encoded = future_error.__into_engine_argument();
        assert_eq!(encoded.type_, AbiValueType::I64);
        assert_eq!(encoded.payload, [9_999, 0]);
        assert_eq!(
            global::Error::__from_engine_return(encoded).expect("unknown enum value"),
            future_error
        );

        let buttons = global::MouseButtonMask::MOUSE_BUTTON_MASK_LEFT
            | global::MouseButtonMask::MOUSE_BUTTON_MASK_RIGHT;
        assert!(buttons.contains(global::MouseButtonMask::MOUSE_BUTTON_MASK_LEFT));
        assert!(buttons.intersects(global::MouseButtonMask::MOUSE_BUTTON_MASK_RIGHT));
        let future_flags = global::MouseButtonMask::from_bits_retain(1_u64 << 63);
        let encoded = future_flags.__into_engine_argument();
        assert_eq!(encoded.type_, AbiValueType::U64);
        assert_eq!(
            global::MouseButtonMask::__from_engine_return(encoded).expect("unknown bitfield value"),
            future_flags
        );
    }

    #[test]
    fn generated_methods_apply_to_subclasses_and_fail_cleanly_without_a_host() {
        fn inherits_node<T: Inherits<Node>>() {}
        fn inherits_object<T: Inherits<Object>>() {}
        inherits_node::<Node2D>();
        inherits_object::<Node2D>();

        let node = ObjectRef::<Node2D>::__from_instance_id(42);
        let error = node
            .set_process(true)
            .expect_err("unit test has no initialized Host");
        assert_eq!(error.kind(), crate::error::EngineErrorKind::Unsupported);
        assert_eq!(GENERATED_GODOT_API, godot_rs_api::SELECTED_GODOT_API);
        let generated_methods = std::hint::black_box(GENERATED_ENGINE_METHOD_COUNT);
        let generated_static_methods = std::hint::black_box(GENERATED_STATIC_ENGINE_METHOD_COUNT);
        let generated_vararg_methods = std::hint::black_box(GENERATED_VARARG_ENGINE_METHOD_COUNT);
        assert!(generated_methods > 0);
        assert!(generated_static_methods > 0);
        assert!(generated_vararg_methods > 0);
        assert_eq!(
            generated_methods + SKIPPED_ENGINE_METHOD_COUNT,
            generated_methods
                + VIRTUAL_ENGINE_METHOD_COUNT
                + HASHLESS_ENGINE_METHOD_COUNT
                + TYPE_BLOCKED_ENGINE_METHOD_COUNT
                + UNSAFE_POINTER_ENGINE_METHOD_COUNT
        );
        assert_eq!(
            VIRTUAL_ENGINE_METHOD_COUNT,
            GENERATED_VIRTUAL_OVERRIDE_COUNT
                + UNSUPPORTED_VIRTUAL_OVERRIDE_COUNT
                + UNSAFE_POINTER_VIRTUAL_OVERRIDE_COUNT
        );
    }

    #[test]
    fn generated_non_methodbind_surface_is_typed_and_fail_closed() {
        let vector = Vector2::new(3.0, 4.0);
        let error =
            builtin::vector2::length(&vector).expect_err("unit test has no initialized Host");
        assert_eq!(error.kind(), crate::error::EngineErrorKind::Unsupported);
        let error = vector
            .godot_member_x()
            .expect_err("generated builtin extension trait also uses the Host");
        assert_eq!(error.kind(), crate::error::EngineErrorKind::Unsupported);

        let error = Input::singleton().expect_err("unit test has no initialized Host");
        assert_eq!(error.kind(), crate::error::EngineErrorKind::Unsupported);

        let error = Node::new_godot().expect_err("unit test has no initialized Host");
        assert_eq!(error.kind(), crate::error::EngineErrorKind::Unsupported);

        let node = ObjectRef::<Node>::__from_instance_id(42);
        let ready = node.signal_ready().expect("typed signal handle");
        assert_eq!(ready.name(), "ready");
        assert_eq!(ready.object_ref().expect("signal owner").instance_id(), 42);
    }
}
