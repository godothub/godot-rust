//! Dynamic Godot values, shared engine containers and explicit Rust snapshots.

use core::cell::OnceCell;
use core::fmt;
mod container;
pub use container::{Array, ArraySnapshot, Dictionary, DictionarySnapshot};

use godot_api::abi::{
    ABI_DYNAMIC_MAGIC as WIRE_MAGIC, ABI_DYNAMIC_MAX_BYTES as MAX_WIRE_BYTES,
    ABI_DYNAMIC_MAX_DEPTH as MAX_NESTING_DEPTH, ABI_DYNAMIC_MAX_ELEMENTS as MAX_CONTAINER_ELEMENTS,
    ABI_DYNAMIC_VERSION as WIRE_VERSION, AbiValueType, validate_dynamic_value,
};

use crate::callable::Callable;
use crate::engine::{
    GodotClass, GodotRef, GodotRefOwnership, Object, ObjectRef, SharedGodotRefOwnership,
};
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

const WIRE_HEADER_BYTES: usize = 20;
const NODE_HEADER_BYTES: usize = 16;

/// Failure while encoding a dynamic value for the Host ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VariantError {
    message: &'static str,
}

impl VariantError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn message(self) -> &'static str {
        self.message
    }
}

impl fmt::Display for VariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for VariantError {}

type WireCache = OnceCell<Result<Box<[u8]>, VariantError>>;

/// One owned Godot-compatible dynamic value.
///
/// Scalar payloads are Rust-owned. Array and Dictionary payloads share their
/// engine container through runtime-owned opaque handles, including across
/// compatible project-module reloads.
pub struct Variant {
    value: VariantValue,
    encoded: WireCache,
}

#[derive(Clone, Debug, PartialEq)]
enum VariantValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringName(StringName),
    NodePath(NodePath),
    Object(DynamicObject),
    Vector2(Vector2),
    Vector2i(Vector2i),
    Vector3(Vector3),
    Vector3i(Vector3i),
    Vector4(Vector4),
    Vector4i(Vector4i),
    Rect2(Rect2),
    Rect2i(Rect2i),
    Quaternion(Quaternion),
    Plane(Plane),
    Transform2D(Transform2D),
    Aabb(Aabb),
    Basis(Basis),
    Transform3D(Transform3D),
    Projection(Projection),
    Color(Color),
    Rid(Rid),
    PackedByteArray(PackedByteArray),
    PackedInt32Array(PackedInt32Array),
    PackedInt64Array(PackedInt64Array),
    PackedFloat32Array(PackedFloat32Array),
    PackedFloat64Array(PackedFloat64Array),
    PackedStringArray(PackedStringArray),
    PackedVector2Array(PackedVector2Array),
    PackedVector3Array(PackedVector3Array),
    PackedColorArray(PackedColorArray),
    PackedVector4Array(PackedVector4Array),
    Callable(Callable),
    Signal(Signal),
    Array(Array),
    Dictionary(Dictionary),
}

/// Borrowed view used to inspect a [`Variant`] without copying its payload.
#[derive(Clone, Copy, Debug)]
pub enum VariantKind<'a> {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(&'a str),
    StringName(&'a StringName),
    NodePath(&'a NodePath),
    Object(ObjectRef<Object>),
    Vector2(Vector2),
    Vector2i(Vector2i),
    Vector3(Vector3),
    Vector3i(Vector3i),
    Vector4(Vector4),
    Vector4i(Vector4i),
    Rect2(Rect2),
    Rect2i(Rect2i),
    Quaternion(Quaternion),
    Plane(Plane),
    Transform2D(&'a Transform2D),
    Aabb(&'a Aabb),
    Basis(&'a Basis),
    Transform3D(&'a Transform3D),
    Projection(&'a Projection),
    Color(Color),
    Rid(Rid),
    PackedByteArray(&'a PackedByteArray),
    PackedInt32Array(&'a PackedInt32Array),
    PackedInt64Array(&'a PackedInt64Array),
    PackedFloat32Array(&'a PackedFloat32Array),
    PackedFloat64Array(&'a PackedFloat64Array),
    PackedStringArray(&'a PackedStringArray),
    PackedVector2Array(&'a PackedVector2Array),
    PackedVector3Array(&'a PackedVector3Array),
    PackedColorArray(&'a PackedColorArray),
    PackedVector4Array(&'a PackedVector4Array),
    Callable(&'a Callable),
    Signal(&'a Signal),
    Array(&'a Array),
    Dictionary(&'a Dictionary),
}

#[derive(Clone)]
struct DynamicObject {
    object: ObjectRef<Object>,
    ownership: Option<SharedGodotRefOwnership>,
}

impl fmt::Debug for DynamicObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Object")
            .field("instance_id", &self.object.instance_id())
            .finish()
    }
}

impl PartialEq for DynamicObject {
    fn eq(&self, other: &Self) -> bool {
        self.object == other.object
    }
}

impl Variant {
    /// Creates Godot `null`.
    #[must_use]
    pub const fn nil() -> Self {
        Self {
            value: VariantValue::Nil,
            encoded: OnceCell::new(),
        }
    }

    /// Returns a borrowed, strongly typed view of this value.
    #[must_use]
    pub fn kind(&self) -> VariantKind<'_> {
        match &self.value {
            VariantValue::Nil => VariantKind::Nil,
            VariantValue::Bool(value) => VariantKind::Bool(*value),
            VariantValue::Int(value) => VariantKind::Int(*value),
            VariantValue::Float(value) => VariantKind::Float(*value),
            VariantValue::String(value) => VariantKind::String(value),
            VariantValue::StringName(value) => VariantKind::StringName(value),
            VariantValue::NodePath(value) => VariantKind::NodePath(value),
            VariantValue::Object(value) => VariantKind::Object(value.object),
            VariantValue::Vector2(value) => VariantKind::Vector2(*value),
            VariantValue::Vector2i(value) => VariantKind::Vector2i(*value),
            VariantValue::Vector3(value) => VariantKind::Vector3(*value),
            VariantValue::Vector3i(value) => VariantKind::Vector3i(*value),
            VariantValue::Vector4(value) => VariantKind::Vector4(*value),
            VariantValue::Vector4i(value) => VariantKind::Vector4i(*value),
            VariantValue::Rect2(value) => VariantKind::Rect2(*value),
            VariantValue::Rect2i(value) => VariantKind::Rect2i(*value),
            VariantValue::Quaternion(value) => VariantKind::Quaternion(*value),
            VariantValue::Plane(value) => VariantKind::Plane(*value),
            VariantValue::Transform2D(value) => VariantKind::Transform2D(value),
            VariantValue::Aabb(value) => VariantKind::Aabb(value),
            VariantValue::Basis(value) => VariantKind::Basis(value),
            VariantValue::Transform3D(value) => VariantKind::Transform3D(value),
            VariantValue::Projection(value) => VariantKind::Projection(value),
            VariantValue::Color(value) => VariantKind::Color(*value),
            VariantValue::Rid(value) => VariantKind::Rid(*value),
            VariantValue::PackedByteArray(value) => VariantKind::PackedByteArray(value),
            VariantValue::PackedInt32Array(value) => VariantKind::PackedInt32Array(value),
            VariantValue::PackedInt64Array(value) => VariantKind::PackedInt64Array(value),
            VariantValue::PackedFloat32Array(value) => VariantKind::PackedFloat32Array(value),
            VariantValue::PackedFloat64Array(value) => VariantKind::PackedFloat64Array(value),
            VariantValue::PackedStringArray(value) => VariantKind::PackedStringArray(value),
            VariantValue::PackedVector2Array(value) => VariantKind::PackedVector2Array(value),
            VariantValue::PackedVector3Array(value) => VariantKind::PackedVector3Array(value),
            VariantValue::PackedColorArray(value) => VariantKind::PackedColorArray(value),
            VariantValue::PackedVector4Array(value) => VariantKind::PackedVector4Array(value),
            VariantValue::Callable(value) => VariantKind::Callable(value),
            VariantValue::Signal(value) => VariantKind::Signal(value),
            VariantValue::Array(value) => VariantKind::Array(value),
            VariantValue::Dictionary(value) => VariantKind::Dictionary(value),
        }
    }

    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], VariantError> {
        match &self.value {
            VariantValue::Array(array) => return array.__bytes(),
            VariantValue::Dictionary(dictionary) => return dictionary.__bytes(),
            _ => {}
        }
        self.encoded
            .get_or_init(|| encode_root(self).map(Vec::into_boxed_slice))
            .as_deref()
            .map_err(|error| *error)
    }

    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        (godot_api::abi::dynamic_value_ownership_token(bytes).is_none()
            && validate_dynamic_value(AbiValueType::VARIANT, bytes))
        .then(|| decode_root(bytes, CallableDecodeMode::RejectOwned).ok())
        .flatten()
    }

    pub(crate) fn __from_native_bytes(bytes: &[u8]) -> Option<Self> {
        (godot_api::abi::dynamic_value_ownership_token(bytes).is_none()
            && validate_dynamic_value(AbiValueType::VARIANT, bytes))
        .then(|| decode_root(bytes, CallableDecodeMode::Native).ok())
        .flatten()
    }

    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostDynamicValueToken>,
    ) -> Option<Self> {
        if !validate_dynamic_value(AbiValueType::VARIANT, bytes)
            || godot_api::abi::dynamic_value_ownership_token(bytes).is_some() != ownership.is_some()
        {
            return None;
        }
        let mut value = decode_root(bytes, CallableDecodeMode::Host).ok()?;
        if let Some(ownership) = ownership {
            value.attach_ownership(std::rc::Rc::new(GodotRefOwnership::Dynamic(ownership)));
        }
        Some(value)
    }

    fn new(value: VariantValue) -> Self {
        Self {
            value,
            encoded: OnceCell::new(),
        }
    }

    fn attach_ownership(&mut self, ownership: SharedGodotRefOwnership) {
        if let VariantValue::Object(value) = &mut self.value {
            value.ownership = Some(ownership);
        }
    }
}

impl Clone for Variant {
    fn clone(&self) -> Self {
        Self::new(self.value.clone())
    }
}

impl Default for Variant {
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Debug for Variant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(formatter)
    }
}

impl PartialEq for Variant {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// Conversion contract used by typed Godot arrays.
#[doc(hidden)]
pub trait VariantConvert: Sized + Clone {
    fn __element_type() -> Option<&'static str> {
        None
    }
    fn to_variant(&self) -> Variant;
    fn from_variant(value: Variant) -> Option<Self>;
}

impl VariantConvert for Variant {
    fn to_variant(&self) -> Variant {
        self.clone()
    }

    fn from_variant(value: Variant) -> Option<Self> {
        Some(value)
    }
}

macro_rules! variant_conversion {
    ($type:ty, $variant:ident) => {
        impl From<$type> for Variant {
            fn from(value: $type) -> Self {
                Self::new(VariantValue::$variant(value))
            }
        }

        impl VariantConvert for $type {
            fn __element_type() -> Option<&'static str> {
                Some(match stringify!($variant) {
                    "Bool" => "bool",
                    "Int" => "int",
                    "Float" => "float",
                    "Rid" => "RID",
                    "Aabb" => "AABB",
                    name => name,
                })
            }
            fn to_variant(&self) -> Variant {
                Variant::from(self.clone())
            }

            fn from_variant(value: Variant) -> Option<Self> {
                let VariantValue::$variant(value) = value.value else {
                    return None;
                };
                Some(value)
            }
        }
    };
}

variant_conversion!(bool, Bool);
variant_conversion!(i64, Int);
variant_conversion!(f64, Float);
variant_conversion!(String, String);
variant_conversion!(StringName, StringName);
variant_conversion!(NodePath, NodePath);
variant_conversion!(Vector2, Vector2);
variant_conversion!(Vector2i, Vector2i);
variant_conversion!(Vector3, Vector3);
variant_conversion!(Vector3i, Vector3i);
variant_conversion!(Vector4, Vector4);
variant_conversion!(Vector4i, Vector4i);
variant_conversion!(Rect2, Rect2);
variant_conversion!(Rect2i, Rect2i);
variant_conversion!(Quaternion, Quaternion);
variant_conversion!(Plane, Plane);
variant_conversion!(Transform2D, Transform2D);
variant_conversion!(Aabb, Aabb);
variant_conversion!(Basis, Basis);
variant_conversion!(Transform3D, Transform3D);
variant_conversion!(Projection, Projection);
variant_conversion!(Color, Color);
variant_conversion!(Rid, Rid);
variant_conversion!(PackedByteArray, PackedByteArray);
variant_conversion!(PackedInt32Array, PackedInt32Array);
variant_conversion!(PackedInt64Array, PackedInt64Array);
variant_conversion!(PackedFloat32Array, PackedFloat32Array);
variant_conversion!(PackedFloat64Array, PackedFloat64Array);
variant_conversion!(PackedStringArray, PackedStringArray);
variant_conversion!(PackedVector2Array, PackedVector2Array);
variant_conversion!(PackedVector3Array, PackedVector3Array);
variant_conversion!(PackedColorArray, PackedColorArray);
variant_conversion!(PackedVector4Array, PackedVector4Array);
variant_conversion!(Callable, Callable);
variant_conversion!(Signal, Signal);
variant_conversion!(Dictionary, Dictionary);

impl From<()> for Variant {
    fn from((): ()) -> Self {
        Self::nil()
    }
}

impl VariantConvert for () {
    fn to_variant(&self) -> Variant {
        Variant::nil()
    }

    fn from_variant(value: Variant) -> Option<Self> {
        matches!(value.value, VariantValue::Nil).then_some(())
    }
}

impl From<i32> for Variant {
    fn from(value: i32) -> Self {
        Self::from(i64::from(value))
    }
}

impl VariantConvert for i32 {
    fn __element_type() -> Option<&'static str> {
        Some("int")
    }
    fn to_variant(&self) -> Variant {
        Variant::from(*self)
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Int(value) = value.value else {
            return None;
        };
        Self::try_from(value).ok()
    }
}

impl From<f32> for Variant {
    fn from(value: f32) -> Self {
        Self::from(f64::from(value))
    }
}

impl VariantConvert for f32 {
    fn __element_type() -> Option<&'static str> {
        Some("float")
    }
    fn to_variant(&self) -> Variant {
        Variant::from(*self)
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Float(value) = value.value else {
            return None;
        };
        let narrowed = value as f32;
        (!value.is_finite() || narrowed.is_finite()).then_some(narrowed)
    }
}

impl<T: crate::engine::GodotIntegerValue> VariantConvert for T {
    fn __element_type() -> Option<&'static str> {
        Some("int")
    }
    fn to_variant(&self) -> Variant {
        Variant::from(self.__raw() as i64)
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Int(value) = value.value else {
            return None;
        };
        Some(Self::__from_raw(value as u64))
    }
}

impl From<&str> for Variant {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}

impl<T: GodotClass> From<ObjectRef<T>> for Variant {
    fn from(value: ObjectRef<T>) -> Self {
        Self::new(VariantValue::Object(DynamicObject {
            object: ObjectRef::__from_instance_id(value.instance_id()),
            ownership: None,
        }))
    }
}

impl<T: GodotClass> VariantConvert for ObjectRef<T> {
    fn __element_type() -> Option<&'static str> {
        Some(T::CLASS_NAME)
    }
    fn to_variant(&self) -> Variant {
        Variant::from(*self)
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Object(value) = value.value else {
            return None;
        };
        let object_id = value.object.instance_id();
        crate::engine::validate_object::<T>(object_id).ok()?;
        Some(ObjectRef::__from_instance_id(object_id))
    }
}

impl<T: GodotClass> From<Option<ObjectRef<T>>> for Variant {
    fn from(value: Option<ObjectRef<T>>) -> Self {
        Variant::from(value.unwrap_or_else(ObjectRef::unresolved))
    }
}

impl<T: GodotClass> VariantConvert for Option<ObjectRef<T>> {
    fn __element_type() -> Option<&'static str> {
        Some(T::CLASS_NAME)
    }
    fn to_variant(&self) -> Variant {
        Variant::from(*self)
    }

    fn from_variant(value: Variant) -> Option<Self> {
        match &value.value {
            VariantValue::Nil => Some(None),
            VariantValue::Object(object) if !object.object.is_resolved() => Some(None),
            _ => ObjectRef::from_variant(value).map(Some),
        }
    }
}

impl<T: GodotClass> From<GodotRef<T>> for Variant {
    fn from(value: GodotRef<T>) -> Self {
        Self::new(VariantValue::Object(DynamicObject {
            object: ObjectRef::__from_instance_id(value.instance_id()),
            ownership: Some(value.dynamic_ownership()),
        }))
    }
}

impl<T: GodotClass> VariantConvert for GodotRef<T> {
    fn __element_type() -> Option<&'static str> {
        Some(T::CLASS_NAME)
    }
    fn to_variant(&self) -> Variant {
        Variant::new(VariantValue::Object(DynamicObject {
            object: ObjectRef::__from_instance_id(self.instance_id()),
            ownership: Some(self.dynamic_ownership()),
        }))
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Object(value) = value.value else {
            return None;
        };
        let object_id = value.object.instance_id();
        crate::engine::validate_object::<T>(object_id).ok()?;
        Some(GodotRef::from_dynamic_parts(object_id, value.ownership?))
    }
}

impl<T: GodotClass> From<Option<GodotRef<T>>> for Variant {
    fn from(value: Option<GodotRef<T>>) -> Self {
        value.map_or_else(
            || Variant::from(ObjectRef::<T>::unresolved()),
            Variant::from,
        )
    }
}

impl<T: GodotClass> VariantConvert for Option<GodotRef<T>> {
    fn __element_type() -> Option<&'static str> {
        Some(T::CLASS_NAME)
    }
    fn to_variant(&self) -> Variant {
        self.as_ref().map_or_else(
            || Variant::from(ObjectRef::<T>::unresolved()),
            GodotRef::to_variant,
        )
    }

    fn from_variant(value: Variant) -> Option<Self> {
        match &value.value {
            VariantValue::Nil => Some(None),
            VariantValue::Object(object) if !object.object.is_resolved() => Some(None),
            _ => GodotRef::from_variant(value).map(Some),
        }
    }
}

impl<T: VariantConvert> From<Array<T>> for Variant {
    fn from(values: Array<T>) -> Self {
        Self::new(VariantValue::Array(values.untyped()))
    }
}

impl<T: VariantConvert> VariantConvert for Array<T> {
    fn __element_type() -> Option<&'static str> {
        Some("Array")
    }
    fn to_variant(&self) -> Variant {
        Variant::from(self.clone())
    }
    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Array(values) = value.value else {
            return None;
        };
        Self::try_from_untyped(values).ok()
    }
}

fn encode_root(value: &Variant) -> Result<Vec<u8>, VariantError> {
    let mut output = Vec::new();
    output.extend_from_slice(&WIRE_MAGIC);
    output.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&0_u64.to_le_bytes());
    encode_node(value, &mut output, 0)?;
    if output.len() > MAX_WIRE_BYTES {
        return Err(VariantError::new(
            "dynamic Godot value exceeds the 64 MiB ABI limit",
        ));
    }
    Ok(output)
}

pub(crate) fn encode_container_values(
    kind: AbiValueType,
    values: &[Variant],
) -> crate::error::EngineResult<Vec<u8>> {
    use crate::error::EngineError;
    let count = if kind == AbiValueType::DICTIONARY {
        if values.len() % 2 != 0 {
            return Err(EngineError::invalid_argument(
                "Dictionary snapshot omitted a value",
            ));
        }
        values.len() / 2
    } else {
        values.len()
    };
    if count > MAX_CONTAINER_ELEMENTS {
        return Err(EngineError::invalid_argument(
            "container snapshot exceeds the element limit",
        ));
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&WIRE_MAGIC);
    bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&kind.0.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&(count as u64).to_le_bytes());
    for value in values {
        encode_node(value, &mut bytes, 1)
            .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
    }
    let length = (bytes.len() - 36) as u64;
    bytes[28..36].copy_from_slice(&length.to_le_bytes());
    Ok(bytes)
}

pub(crate) fn decode_native_snapshot(
    bytes: &[u8],
    kind: AbiValueType,
) -> crate::error::EngineResult<Vec<Variant>> {
    decode_snapshot(bytes, kind, CallableDecodeMode::Native)
        .map_err(|error| crate::error::EngineError::invalid_argument(error.to_string()))
}

fn decode_host_snapshot(
    value: crate::module::HostMethodValue,
    kind: AbiValueType,
) -> crate::error::EngineResult<Vec<Variant>> {
    use crate::error::EngineError;
    let abi = value.abi();
    let (pointer, length) = abi
        .byte_range(kind)
        .ok_or_else(|| EngineError::invalid_result("invalid container snapshot range"))?;
    if length > MAX_WIRE_BYTES {
        return Err(EngineError::invalid_result(
            "container snapshot exceeds the byte limit",
        ));
    }
    // SAFETY: HostMethodValue retains this validated output allocation.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    let token = godot_api::abi::dynamic_value_ownership_token(bytes)
        .ok_or_else(|| EngineError::invalid_result("container snapshot omitted ownership"))?;
    let owner = crate::module::retain_dynamic_value(kind, token)?
        .map(|owner| std::rc::Rc::new(GodotRefOwnership::Dynamic(owner)))
        .ok_or_else(|| {
            EngineError::invalid_result("container snapshot omitted its ownership lease")
        })?;
    let mut values = decode_snapshot(bytes, kind, CallableDecodeMode::Host)
        .map_err(|error| EngineError::invalid_result(error.to_string()))?;
    for value in &mut values {
        value.attach_ownership(std::rc::Rc::clone(&owner));
    }
    Ok(values)
}

fn decode_snapshot(
    bytes: &[u8],
    kind: AbiValueType,
    mode: CallableDecodeMode,
) -> Result<Vec<Variant>, VariantError> {
    if !validate_dynamic_value(kind, bytes) || bytes.get(24..28) != Some(&[0, 0, 0, 0]) {
        return Err(VariantError::new("invalid inline container snapshot"));
    }
    let count = read_count(bytes, 36, bytes.len())?;
    let count = if kind == AbiValueType::DICTIONARY {
        count * 2
    } else {
        count
    };
    let mut offset = 44;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(decode_node(bytes, &mut offset, 1, mode)?);
    }
    if offset != bytes.len() {
        return Err(VariantError::new("container snapshot has trailing bytes"));
    }
    Ok(values)
}

pub(crate) fn builtin_type_id(name: &str) -> Option<i64> {
    Some(match name {
        "Nil" => 0,
        "bool" => 1,
        "int" => 2,
        "float" => 3,
        "String" => 4,
        "Vector2" => 5,
        "Vector2i" => 6,
        "Rect2" => 7,
        "Rect2i" => 8,
        "Vector3" => 9,
        "Vector3i" => 10,
        "Transform2D" => 11,
        "Vector4" => 12,
        "Vector4i" => 13,
        "Plane" => 14,
        "Quaternion" => 15,
        "AABB" => 16,
        "Basis" => 17,
        "Transform3D" => 18,
        "Projection" => 19,
        "Color" => 20,
        "StringName" => 21,
        "NodePath" => 22,
        "RID" => 23,
        "Object" => 24,
        "Callable" => 25,
        "Signal" => 26,
        "Dictionary" => 27,
        "Array" => 28,
        "PackedByteArray" => 29,
        "PackedInt32Array" => 30,
        "PackedInt64Array" => 31,
        "PackedFloat32Array" => 32,
        "PackedFloat64Array" => 33,
        "PackedStringArray" => 34,
        "PackedVector2Array" => 35,
        "PackedVector3Array" => 36,
        "PackedColorArray" => 37,
        "PackedVector4Array" => 38,
        _ => return None,
    })
}

pub(crate) fn builtin_type_name(id: i64) -> Option<&'static str> {
    const NAMES: &[&str] = &[
        "Nil",
        "bool",
        "int",
        "float",
        "String",
        "Vector2",
        "Vector2i",
        "Rect2",
        "Rect2i",
        "Vector3",
        "Vector3i",
        "Transform2D",
        "Vector4",
        "Vector4i",
        "Plane",
        "Quaternion",
        "AABB",
        "Basis",
        "Transform3D",
        "Projection",
        "Color",
        "StringName",
        "NodePath",
        "RID",
        "Object",
        "Callable",
        "Signal",
        "Dictionary",
        "Array",
        "PackedByteArray",
        "PackedInt32Array",
        "PackedInt64Array",
        "PackedFloat32Array",
        "PackedFloat64Array",
        "PackedStringArray",
        "PackedVector2Array",
        "PackedVector3Array",
        "PackedColorArray",
        "PackedVector4Array",
    ];
    usize::try_from(id)
        .ok()
        .and_then(|index| NAMES.get(index).copied())
}

pub(crate) fn kind_type_id(kind: VariantKind<'_>) -> i64 {
    match kind {
        VariantKind::Nil => 0,
        VariantKind::Bool(_) => 1,
        VariantKind::Int(_) => 2,
        VariantKind::Float(_) => 3,
        VariantKind::String(_) => 4,
        VariantKind::Vector2(_) => 5,
        VariantKind::Vector2i(_) => 6,
        VariantKind::Rect2(_) => 7,
        VariantKind::Rect2i(_) => 8,
        VariantKind::Vector3(_) => 9,
        VariantKind::Vector3i(_) => 10,
        VariantKind::Transform2D(_) => 11,
        VariantKind::Vector4(_) => 12,
        VariantKind::Vector4i(_) => 13,
        VariantKind::Plane(_) => 14,
        VariantKind::Quaternion(_) => 15,
        VariantKind::Aabb(_) => 16,
        VariantKind::Basis(_) => 17,
        VariantKind::Transform3D(_) => 18,
        VariantKind::Projection(_) => 19,
        VariantKind::Color(_) => 20,
        VariantKind::StringName(_) => 21,
        VariantKind::NodePath(_) => 22,
        VariantKind::Rid(_) => 23,
        VariantKind::Object(_) => 24,
        VariantKind::Callable(_) => 25,
        VariantKind::Signal(_) => 26,
        VariantKind::Dictionary(_) => 27,
        VariantKind::Array(_) => 28,
        VariantKind::PackedByteArray(_) => 29,
        VariantKind::PackedInt32Array(_) => 30,
        VariantKind::PackedInt64Array(_) => 31,
        VariantKind::PackedFloat32Array(_) => 32,
        VariantKind::PackedFloat64Array(_) => 33,
        VariantKind::PackedStringArray(_) => 34,
        VariantKind::PackedVector2Array(_) => 35,
        VariantKind::PackedVector3Array(_) => 36,
        VariantKind::PackedColorArray(_) => 37,
        VariantKind::PackedVector4Array(_) => 38,
    }
}

fn encode_node(value: &Variant, output: &mut Vec<u8>, depth: usize) -> Result<(), VariantError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(VariantError::new(
            "dynamic Godot value exceeds the nesting-depth limit",
        ));
    }
    let type_ = variant_type(&value.value);
    let header = output.len();
    output.extend_from_slice(&type_.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u64.to_le_bytes());
    let payload_start = output.len();
    match &value.value {
        VariantValue::Nil => {}
        VariantValue::Bool(value) => output.push(u8::from(*value)),
        VariantValue::Int(value) => output.extend_from_slice(&value.to_le_bytes()),
        VariantValue::Float(value) => output.extend_from_slice(&value.to_bits().to_le_bytes()),
        VariantValue::String(value) => output.extend_from_slice(value.as_bytes()),
        VariantValue::StringName(value) => output.extend_from_slice(value.as_str().as_bytes()),
        VariantValue::NodePath(value) => output.extend_from_slice(value.as_str().as_bytes()),
        VariantValue::Object(value) => {
            output.extend_from_slice(&value.object.instance_id().to_le_bytes());
        }
        VariantValue::Vector2(value) => write_f32s(output, &[value.x, value.y]),
        VariantValue::Vector2i(value) => write_i32s(output, &[value.x, value.y]),
        VariantValue::Vector3(value) => write_f32s(output, &[value.x, value.y, value.z]),
        VariantValue::Vector3i(value) => write_i32s(output, &[value.x, value.y, value.z]),
        VariantValue::Vector4(value) => write_f32s(output, &[value.x, value.y, value.z, value.w]),
        VariantValue::Vector4i(value) => write_i32s(output, &[value.x, value.y, value.z, value.w]),
        VariantValue::Rect2(value) => write_f32s(
            output,
            &[
                value.position.x,
                value.position.y,
                value.size.x,
                value.size.y,
            ],
        ),
        VariantValue::Rect2i(value) => write_i32s(
            output,
            &[
                value.position.x,
                value.position.y,
                value.size.x,
                value.size.y,
            ],
        ),
        VariantValue::Quaternion(value) => {
            write_f32s(output, &[value.x, value.y, value.z, value.w]);
        }
        VariantValue::Plane(value) => {
            write_f32s(
                output,
                &[value.normal.x, value.normal.y, value.normal.z, value.d],
            );
        }
        VariantValue::Transform2D(value) => write_f32s(output, value.__components()),
        VariantValue::Aabb(value) => write_f32s(output, value.__components()),
        VariantValue::Basis(value) => write_f32s(output, value.__components()),
        VariantValue::Transform3D(value) => write_f32s(output, value.__components()),
        VariantValue::Projection(value) => write_f32s(output, value.__components()),
        VariantValue::Color(value) => write_f32s(output, &[value.r, value.g, value.b, value.a]),
        VariantValue::Rid(value) => output.extend_from_slice(&value.id().to_le_bytes()),
        VariantValue::PackedByteArray(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedInt32Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedInt64Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedFloat32Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedFloat64Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedStringArray(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedVector2Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedVector3Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedColorArray(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::PackedVector4Array(value) => output.extend_from_slice(value.__bytes()),
        VariantValue::Callable(value) => {
            output.extend_from_slice(value.__bytes().map_err(|_| {
                VariantError::new("Godot Callable could not be encoded in a Variant")
            })?);
        }
        VariantValue::Signal(value) => {
            output.extend_from_slice(value.__bytes().map_err(|_| {
                VariantError::new("Godot Signal could not be encoded in a Variant")
            })?);
        }
        VariantValue::Array(values) => {
            let bytes = values.__bytes()?;
            output[header + 4..header + 8].copy_from_slice(&bytes[24..28]);
            output.extend_from_slice(&bytes[36..]);
        }
        VariantValue::Dictionary(values) => {
            let bytes = values.__bytes()?;
            output[header + 4..header + 8].copy_from_slice(&bytes[24..28]);
            output.extend_from_slice(&bytes[36..]);
        }
    }
    let payload_length = output
        .len()
        .checked_sub(payload_start)
        .and_then(|length| u64::try_from(length).ok())
        .ok_or_else(|| VariantError::new("dynamic Godot value length overflowed"))?;
    output[header + 8..header + 16].copy_from_slice(&payload_length.to_le_bytes());
    if output.len() > MAX_WIRE_BYTES {
        return Err(VariantError::new(
            "dynamic Godot value exceeds the 64 MiB ABI limit",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CallableDecodeMode {
    RejectOwned,
    Host,
    Native,
}

fn decode_root(bytes: &[u8], callable_mode: CallableDecodeMode) -> Result<Variant, VariantError> {
    if bytes.len() < WIRE_HEADER_BYTES
        || bytes[..8] != WIRE_MAGIC
        || read_u16(bytes, 8) != Some(WIRE_VERSION)
        || bytes.len() > MAX_WIRE_BYTES
    {
        return Err(VariantError::new(
            "dynamic Godot value has an invalid ABI header",
        ));
    }
    let mut offset = WIRE_HEADER_BYTES;
    let value = decode_node(bytes, &mut offset, 0, callable_mode)?;
    if offset != bytes.len() {
        return Err(VariantError::new(
            "dynamic Godot value has trailing ABI bytes",
        ));
    }
    Ok(value)
}

fn decode_node(
    bytes: &[u8],
    offset: &mut usize,
    depth: usize,
    callable_mode: CallableDecodeMode,
) -> Result<Variant, VariantError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(VariantError::new(
            "dynamic Godot value exceeds the nesting-depth limit",
        ));
    }
    let header_end = offset
        .checked_add(NODE_HEADER_BYTES)
        .ok_or_else(|| VariantError::new("dynamic Godot value offset overflowed"))?;
    let header = bytes
        .get(*offset..header_end)
        .ok_or_else(|| VariantError::new("dynamic Godot value is truncated"))?;
    let type_ = u32::from_le_bytes(header[..4].try_into().expect("u32 width"));
    let flags = u32::from_le_bytes(header[4..8].try_into().expect("u32 width"));
    let length = usize::try_from(u64::from_le_bytes(
        header[8..16].try_into().expect("u64 width"),
    ))
    .map_err(|_| VariantError::new("dynamic Godot value length is out of range"))?;
    if flags != 0 && flags != godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER {
        return Err(VariantError::new(
            "dynamic Godot value uses unsupported ABI flags",
        ));
    }
    let payload_start = header_end;
    let payload_end = payload_start
        .checked_add(length)
        .ok_or_else(|| VariantError::new("dynamic Godot value length overflowed"))?;
    let payload = bytes
        .get(payload_start..payload_end)
        .ok_or_else(|| VariantError::new("dynamic Godot value payload is truncated"))?;
    *offset = payload_end;

    if flags == godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER {
        let token = read_u64_exact(payload)?;
        return match (type_, callable_mode) {
            (37, CallableDecodeMode::Host) => Array::<Variant>::from_host(token)
                .map(Variant::from)
                .map_err(|_| VariantError::new("invalid Host Array token")),
            (38, CallableDecodeMode::Host) => Dictionary::from_host(token)
                .map(Variant::from)
                .map_err(|_| VariantError::new("invalid Host Dictionary token")),
            (37, CallableDecodeMode::Native) => {
                crate::native::retain_container(AbiValueType::ARRAY, token)
                    .map(Array::<Variant>::from_native)
                    .map(Variant::from)
                    .map_err(|_| VariantError::new("invalid Native Array token"))
            }
            (38, CallableDecodeMode::Native) => {
                crate::native::retain_container(AbiValueType::DICTIONARY, token)
                    .map(Dictionary::from_native)
                    .map(Variant::from)
                    .map_err(|_| VariantError::new("invalid Native Dictionary token"))
            }
            _ => Err(VariantError::new(
                "container handles require their owning runtime",
            )),
        };
    }

    let value = match type_ {
        0 if payload.is_empty() => Variant::nil(),
        1 if payload.len() == 1 && payload[0] <= 1 => Variant::from(payload[0] != 0),
        2 => Variant::from(read_i64_exact(payload)?),
        3 => Variant::from(f64::from_bits(read_u64_exact(payload)?)),
        4 => Variant::new(VariantValue::Object(DynamicObject {
            object: ObjectRef::__from_instance_id(read_u64_exact(payload)?),
            ownership: None,
        })),
        6 => Variant::from(read_text(payload)?.to_owned()),
        7 => Variant::from(Vector2::new(read_f32(payload, 0)?, read_f32(payload, 1)?)),
        8 => Variant::from(Vector3::new(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
        )),
        9 => Variant::from(Color::rgba(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        )),
        10 => Variant::from(Vector2i::new(read_i32(payload, 0)?, read_i32(payload, 1)?)),
        11 => Variant::from(Vector3i::new(
            read_i32(payload, 0)?,
            read_i32(payload, 1)?,
            read_i32(payload, 2)?,
        )),
        12 => Variant::from(Rid::from_raw(read_u64_exact(payload)?)),
        13 => Variant::from(StringName::from(read_text(payload)?)),
        14 => Variant::from(NodePath::from(read_text(payload)?)),
        15 => Variant::from(Rect2::from_components(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        )),
        16 => Variant::from(Rect2i::from_components(
            read_i32(payload, 0)?,
            read_i32(payload, 1)?,
            read_i32(payload, 2)?,
            read_i32(payload, 3)?,
        )),
        17 => Variant::from(Quaternion::new(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        )),
        18 => Variant::from(Plane::from_components(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        )),
        19 => Variant::from(Vector4::new(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        )),
        20 => Variant::from(Vector4i::new(
            read_i32(payload, 0)?,
            read_i32(payload, 1)?,
            read_i32(payload, 2)?,
            read_i32(payload, 3)?,
        )),
        21 => Variant::from(Transform2D::__from_components(read_f32_array(payload)?)),
        22 => Variant::from(Aabb::__from_components(read_f32_array(payload)?)),
        23 => Variant::from(Basis::__from_components(read_f32_array(payload)?)),
        24 => Variant::from(Transform3D::__from_components(read_f32_array(payload)?)),
        25 => Variant::from(Projection::__from_components(read_f32_array(payload)?)),
        26 => packed(payload, PackedByteArray::__from_bytes)?,
        27 => packed(payload, PackedInt32Array::__from_bytes)?,
        28 => packed(payload, PackedInt64Array::__from_bytes)?,
        29 => packed(payload, PackedFloat32Array::__from_bytes)?,
        30 => packed(payload, PackedFloat64Array::__from_bytes)?,
        31 => packed(payload, PackedStringArray::__from_bytes)?,
        32 => packed(payload, PackedVector2Array::__from_bytes)?,
        33 => packed(payload, PackedVector3Array::__from_bytes)?,
        34 => packed(payload, PackedColorArray::__from_bytes)?,
        35 => packed(payload, PackedVector4Array::__from_bytes)?,
        37 => decode_array(bytes, payload_start, payload_end, depth, callable_mode)?,
        38 => decode_dictionary(bytes, payload_start, payload_end, depth, callable_mode)?,
        39 => {
            let ownership = match godot_api::abi::callable_value_ownership_token(payload) {
                Some(token) if matches!(callable_mode, CallableDecodeMode::Host) => {
                    crate::module::retain_callable_value(token).map_err(|_| {
                        VariantError::new("Host Callable ownership could not be retained")
                    })?
                }
                Some(token) if matches!(callable_mode, CallableDecodeMode::Native) => {
                    let callable = crate::native::retain_rust_callable(token).map_err(|_| {
                        VariantError::new("Native Callable ownership could not be retained")
                    })?;
                    return Ok(Variant::from(callable));
                }
                Some(_) => {
                    return Err(VariantError::new(
                        "process-local Host Callable cannot be decoded without its Host",
                    ));
                }
                None => None,
            };
            let callable = Callable::__from_host_bytes(payload, ownership)
                .ok_or_else(|| VariantError::new("dynamic Godot Callable payload is invalid"))?;
            Variant::from(callable)
        }
        40 => {
            let signal = Signal::__from_bytes(payload)
                .ok_or_else(|| VariantError::new("dynamic Godot Signal payload is invalid"))?;
            Variant::from(signal)
        }
        _ => {
            return Err(VariantError::new(
                "dynamic Godot value has an invalid type or payload",
            ));
        }
    };
    Ok(value)
}

fn decode_array(
    bytes: &[u8],
    payload_start: usize,
    payload_end: usize,
    depth: usize,
    callable_mode: CallableDecodeMode,
) -> Result<Variant, VariantError> {
    let count = read_count(bytes, payload_start, payload_end)?;
    let mut offset = payload_start + 8;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(decode_node(bytes, &mut offset, depth + 1, callable_mode)?);
    }
    if offset != payload_end {
        return Err(VariantError::new(
            "Godot Array ABI payload is not canonical",
        ));
    }
    Array::from_vec(values)
        .map(Variant::from)
        .map_err(|_| VariantError::new("Array snapshot could not be materialized"))
}

fn decode_dictionary(
    bytes: &[u8],
    payload_start: usize,
    payload_end: usize,
    depth: usize,
    callable_mode: CallableDecodeMode,
) -> Result<Variant, VariantError> {
    let count = read_count(bytes, payload_start, payload_end)?;
    let mut offset = payload_start + 8;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let key = decode_node(bytes, &mut offset, depth + 1, callable_mode)?;
        let value = decode_node(bytes, &mut offset, depth + 1, callable_mode)?;
        entries.push((key, value));
    }
    if offset != payload_end {
        return Err(VariantError::new(
            "Godot Dictionary ABI payload is not canonical",
        ));
    }
    Dictionary::from_entries(entries)
        .map(Variant::from)
        .map_err(|_| VariantError::new("Dictionary snapshot could not be materialized"))
}

fn read_count(bytes: &[u8], start: usize, end: usize) -> Result<usize, VariantError> {
    let header = bytes
        .get(start..start + 8)
        .filter(|_| start + 8 <= end)
        .ok_or_else(|| VariantError::new("Godot container ABI count is truncated"))?;
    let count = usize::try_from(u64::from_le_bytes(header.try_into().expect("u64 width")))
        .map_err(|_| VariantError::new("Godot container ABI count is out of range"))?;
    if count > MAX_CONTAINER_ELEMENTS {
        return Err(VariantError::new(
            "Godot container exceeds the element-count limit",
        ));
    }
    Ok(count)
}

fn packed<T>(
    payload: &[u8],
    decode: impl FnOnce(&[u8]) -> Option<T>,
) -> Result<Variant, VariantError>
where
    Variant: From<T>,
{
    decode(payload)
        .map(Variant::from)
        .ok_or_else(|| VariantError::new("Godot packed-array ABI payload is invalid"))
}

fn variant_type(value: &VariantValue) -> u32 {
    match value {
        VariantValue::Nil => 0,
        VariantValue::Bool(_) => 1,
        VariantValue::Int(_) => 2,
        VariantValue::Float(_) => 3,
        VariantValue::Object(_) => 4,
        VariantValue::String(_) => 6,
        VariantValue::Vector2(_) => 7,
        VariantValue::Vector3(_) => 8,
        VariantValue::Color(_) => 9,
        VariantValue::Vector2i(_) => 10,
        VariantValue::Vector3i(_) => 11,
        VariantValue::Rid(_) => 12,
        VariantValue::StringName(_) => 13,
        VariantValue::NodePath(_) => 14,
        VariantValue::Rect2(_) => 15,
        VariantValue::Rect2i(_) => 16,
        VariantValue::Quaternion(_) => 17,
        VariantValue::Plane(_) => 18,
        VariantValue::Vector4(_) => 19,
        VariantValue::Vector4i(_) => 20,
        VariantValue::Transform2D(_) => 21,
        VariantValue::Aabb(_) => 22,
        VariantValue::Basis(_) => 23,
        VariantValue::Transform3D(_) => 24,
        VariantValue::Projection(_) => 25,
        VariantValue::PackedByteArray(_) => 26,
        VariantValue::PackedInt32Array(_) => 27,
        VariantValue::PackedInt64Array(_) => 28,
        VariantValue::PackedFloat32Array(_) => 29,
        VariantValue::PackedFloat64Array(_) => 30,
        VariantValue::PackedStringArray(_) => 31,
        VariantValue::PackedVector2Array(_) => 32,
        VariantValue::PackedVector3Array(_) => 33,
        VariantValue::PackedColorArray(_) => 34,
        VariantValue::PackedVector4Array(_) => 35,
        VariantValue::Callable(_) => 39,
        VariantValue::Signal(_) => 40,
        VariantValue::Array(_) => 37,
        VariantValue::Dictionary(_) => 38,
    }
}

fn write_f32s(output: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        output.extend_from_slice(&value.to_bits().to_le_bytes());
    }
}

fn write_i32s(output: &mut Vec<u8>, values: &[i32]) {
    for value in values {
        output.extend_from_slice(&value.to_le_bytes());
    }
}

fn read_text(bytes: &[u8]) -> Result<&str, VariantError> {
    core::str::from_utf8(bytes)
        .map_err(|_| VariantError::new("dynamic Godot text is not valid UTF-8"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_i64_exact(bytes: &[u8]) -> Result<i64, VariantError> {
    Ok(i64::from_le_bytes(bytes.try_into().map_err(|_| {
        VariantError::new("dynamic Godot integer has an invalid size")
    })?))
}

fn read_u64_exact(bytes: &[u8]) -> Result<u64, VariantError> {
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
        VariantError::new("dynamic Godot integer has an invalid size")
    })?))
}

fn read_f32(bytes: &[u8], index: usize) -> Result<f32, VariantError> {
    let bits = read_i32(bytes, index)? as u32;
    Ok(f32::from_bits(bits))
}

fn read_i32(bytes: &[u8], index: usize) -> Result<i32, VariantError> {
    let start = index
        .checked_mul(4)
        .ok_or_else(|| VariantError::new("dynamic Godot component index overflowed"))?;
    Ok(i32::from_le_bytes(
        bytes
            .get(start..start + 4)
            .ok_or_else(|| VariantError::new("dynamic Godot component payload is truncated"))?
            .try_into()
            .expect("i32 width"),
    ))
}

fn read_f32_array<const N: usize>(bytes: &[u8]) -> Result<[f32; N], VariantError> {
    if bytes.len() != N * 4 {
        return Err(VariantError::new(
            "dynamic Godot math payload has an invalid size",
        ));
    }
    Ok(core::array::from_fn(|index| {
        read_f32(bytes, index).expect("validated math payload contains every component")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nullable_object_conversions_accept_nil_and_reject_unvalidated_handles() {
        use crate::engine::{Node, Resource};
        assert_eq!(
            Option::<ObjectRef<Node>>::from_variant(Variant::nil()),
            Some(None)
        );
        assert_eq!(
            Option::<GodotRef<Resource>>::from_variant(Variant::nil()),
            Some(None)
        );
        let null = Variant::from(Option::<ObjectRef<Node>>::None);
        assert_eq!(Option::<ObjectRef<Node>>::from_variant(null), Some(None));
        let unresolved = Variant::from(ObjectRef::<Node>::__from_instance_id(42));
        assert!(
            ObjectRef::<Node>::from_variant(unresolved).is_none(),
            "dynamic objects need a live engine for class validation"
        );
    }

    #[test]
    fn explicit_snapshots_round_trip_canonically() {
        let values = vec![
            Variant::from("name"),
            Variant::from("玩家"),
            Variant::from("position"),
            Variant::from(Vector3::new(1.0, -2.0, 3.5)),
            Variant::from("packed"),
            Variant::from(PackedByteArray::from(vec![0, 127, 255])),
        ];
        let bytes =
            encode_container_values(AbiValueType::DICTIONARY, &values).expect("snapshot wire");
        let restored = decode_snapshot(
            &bytes,
            AbiValueType::DICTIONARY,
            CallableDecodeMode::RejectOwned,
        )
        .expect("snapshot");
        assert_eq!(restored, values);
    }

    #[test]
    fn standard_callables_round_trip_inside_snapshots() {
        let callback = Callable::from_object_method(
            ObjectRef::<crate::engine::Node>::__from_instance_id(42),
            "_ready",
        );
        let values = vec![Variant::from("callback"), Variant::from(callback.clone())];
        let bytes =
            encode_container_values(AbiValueType::DICTIONARY, &values).expect("snapshot wire");
        let restored = decode_snapshot(
            &bytes,
            AbiValueType::DICTIONARY,
            CallableDecodeMode::RejectOwned,
        )
        .expect("snapshot");
        assert_eq!(restored, values);
    }

    #[test]
    fn signals_round_trip_inside_snapshots() {
        let signal = Signal::from_object(
            ObjectRef::<crate::engine::Node>::__from_instance_id(42),
            "已完成",
        );
        let values = vec![Variant::from("signal"), Variant::from(signal)];
        let bytes =
            encode_container_values(AbiValueType::DICTIONARY, &values).expect("snapshot wire");
        let restored = decode_snapshot(
            &bytes,
            AbiValueType::DICTIONARY,
            CallableDecodeMode::RejectOwned,
        )
        .expect("snapshot");
        assert_eq!(restored, values);
    }

    #[test]
    fn lazy_containers_share_identity_and_snapshots_are_independent() {
        let array = Array::<i64>::new();
        assert!(array.is_same(&array.clone()).expect("local identity"));
        let snapshot = ArraySnapshot::from_vec(vec![1_i64, 2]);
        let mut independent = snapshot.clone();
        independent.as_mut_slice()[0] = 99;
        assert_eq!(snapshot.as_slice(), &[1, 2]);
        let initialized = Array::from_vec(vec![1_i64, 2]).expect("offline initializer");
        assert_eq!(
            initialized.get(1).expect("owned initializer element"),
            Some(2)
        );
        let bytes = initialized.__bytes().expect("standalone default wire");
        assert!(validate_dynamic_value(AbiValueType::ARRAY, bytes));
        let dictionary =
            Dictionary::from_entries(vec![(Variant::from("answer"), Variant::from(42_i64))])
                .expect("offline dictionary initializer");
        assert!(validate_dynamic_value(
            AbiValueType::DICTIONARY,
            dictionary.__bytes().expect("standalone dictionary default")
        ));
        let snapshot = DictionarySnapshot::from_entries(vec![(
            Variant::from("answer"),
            Variant::from(42_i64),
        )]);
        let map = snapshot
            .into_hash_map::<String, i64>()
            .expect("Rust-owned map");
        assert_eq!(map.get("answer"), Some(&42));
        let duplicate = DictionarySnapshot::from_entries(vec![
            (Variant::from("same"), Variant::from(1_i64)),
            (Variant::from("same"), Variant::from(2_i64)),
        ]);
        assert!(duplicate.into_hash_map::<String, i64>().is_err());
    }

    #[test]
    fn dynamic_wire_rejects_truncation_extensions_and_trailing_bytes() {
        let value = Variant::from("你好");
        let bytes = value.__bytes().expect("wire").to_vec();
        assert!(Variant::__from_bytes(&bytes[..bytes.len() - 1]).is_none());
        let mut extension = bytes.clone();
        extension[16] = 1;
        assert!(Variant::__from_bytes(&extension).is_none());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(Variant::__from_bytes(&trailing).is_none());
    }
}
