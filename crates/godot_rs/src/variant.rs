//! Owned dynamic Godot values and container types.

use core::cell::OnceCell;
use core::fmt;
use core::ops::{Deref, Index};

use godot_rs_api::abi::{
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
/// Values are ordinary Rust-owned data. The SDK serializes them only at a
/// Host call boundary, so they remain safe across project-module reloads.
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
        self.encoded
            .get_or_init(|| encode_root(self).map(Vec::into_boxed_slice))
            .as_deref()
            .map_err(|error| *error)
    }

    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        (godot_rs_api::abi::dynamic_value_ownership_token(bytes).is_none()
            && validate_dynamic_value(AbiValueType::VARIANT, bytes))
        .then(|| decode_root(bytes, CallableDecodeMode::RejectOwned).ok())
        .flatten()
    }

    pub(crate) fn __from_native_bytes(bytes: &[u8]) -> Option<Self> {
        (godot_rs_api::abi::dynamic_value_ownership_token(bytes).is_none()
            && validate_dynamic_value(AbiValueType::VARIANT, bytes))
        .then(|| decode_root(bytes, CallableDecodeMode::Native).ok())
        .flatten()
    }

    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostDynamicValueToken>,
    ) -> Option<Self> {
        if !validate_dynamic_value(AbiValueType::VARIANT, bytes)
            || godot_rs_api::abi::dynamic_value_ownership_token(bytes).is_some()
                != ownership.is_some()
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
        match &mut self.value {
            VariantValue::Object(value) => value.ownership = Some(ownership),
            VariantValue::Array(values) => {
                for value in &mut values.values {
                    value.attach_ownership(std::rc::Rc::clone(&ownership));
                }
            }
            VariantValue::Dictionary(values) => {
                for (key, value) in &mut values.entries {
                    key.attach_ownership(std::rc::Rc::clone(&ownership));
                    value.attach_ownership(std::rc::Rc::clone(&ownership));
                }
            }
            _ => {}
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

/// Owned Godot `Array`, optionally constrained to one element type.
pub struct Array<T = Variant> {
    values: Vec<T>,
    encoded: WireCache,
}

impl<T> Array<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            values: Vec::new(),
            encoded: OnceCell::new(),
        }
    }

    #[must_use]
    pub fn from_vec(values: Vec<T>) -> Self {
        Self {
            values,
            encoded: OnceCell::new(),
        }
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        self.values
    }

    pub fn push(&mut self, value: T) {
        self.invalidate();
        self.values.push(value);
    }

    pub fn pop(&mut self) -> Option<T> {
        self.invalidate();
        self.values.pop()
    }

    pub fn insert(&mut self, index: usize, value: T) {
        self.invalidate();
        self.values.insert(index, value);
    }

    pub fn remove(&mut self, index: usize) -> T {
        self.invalidate();
        self.values.remove(index)
    }

    pub fn clear(&mut self) {
        self.invalidate();
        self.values.clear();
    }

    fn invalidate(&mut self) {
        let _ = self.encoded.take();
    }
}

impl<T: VariantConvert> Array<T> {
    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], VariantError> {
        self.encoded
            .get_or_init(|| {
                let values = self
                    .values
                    .iter()
                    .map(VariantConvert::to_variant)
                    .collect::<Vec<_>>();
                encode_root(&Variant::from(Array::from_vec(values))).map(Vec::into_boxed_slice)
            })
            .as_deref()
            .map_err(|error| *error)
    }

    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        if godot_rs_api::abi::dynamic_value_ownership_token(bytes).is_some()
            || !validate_dynamic_value(AbiValueType::ARRAY, bytes)
        {
            return None;
        }
        let value = decode_root(bytes, CallableDecodeMode::RejectOwned).ok()?;
        let VariantValue::Array(values) = value.value else {
            return None;
        };
        values
            .into_vec()
            .into_iter()
            .map(T::from_variant)
            .collect::<Option<Vec<_>>>()
            .map(Self::from_vec)
    }

    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostDynamicValueToken>,
    ) -> Option<Self> {
        let value = Variant::__from_host_bytes(bytes, ownership)?;
        let VariantValue::Array(values) = value.value else {
            return None;
        };
        values
            .into_vec()
            .into_iter()
            .map(T::from_variant)
            .collect::<Option<Vec<_>>>()
            .map(Self::from_vec)
    }
}

impl<T: Clone> Clone for Array<T> {
    fn clone(&self) -> Self {
        Self::from_vec(self.values.clone())
    }
}

impl<T: fmt::Debug> fmt::Debug for Array<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.values.fmt(formatter)
    }
}

impl<T: PartialEq> PartialEq for Array<T> {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}

impl<T> Default for Array<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<Vec<T>> for Array<T> {
    fn from(values: Vec<T>) -> Self {
        Self::from_vec(values)
    }
}

impl<T> From<Array<T>> for Vec<T> {
    fn from(values: Array<T>) -> Self {
        values.into_vec()
    }
}

impl<T> AsRef<[T]> for Array<T> {
    fn as_ref(&self) -> &[T] {
        &self.values
    }
}

impl<T> Deref for Array<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<'a, T> IntoIterator for &'a Array<T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<T> IntoIterator for Array<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

/// Owned Godot `Dictionary`.
///
/// Entries retain insertion order on the Rust side. Keys use Variant equality,
/// which keeps the API useful for every Godot-compatible key type.
pub struct Dictionary {
    entries: Vec<(Variant, Variant)>,
    encoded: WireCache,
}

impl Dictionary {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            encoded: OnceCell::new(),
        }
    }

    #[must_use]
    pub fn from_entries(entries: Vec<(Variant, Variant)>) -> Self {
        let mut dictionary = Self::new();
        for (key, value) in entries {
            dictionary.insert(key, value);
        }
        dictionary
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn get(&self, key: &Variant) -> Option<&Variant> {
        self.entries
            .iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value))
    }

    pub fn insert(
        &mut self,
        key: impl Into<Variant>,
        value: impl Into<Variant>,
    ) -> Option<Variant> {
        self.invalidate();
        let key = key.into();
        let value = value.into();
        if let Some((_, current)) = self
            .entries
            .iter_mut()
            .find(|(candidate, _)| candidate == &key)
        {
            return Some(core::mem::replace(current, value));
        }
        self.entries.push((key, value));
        None
    }

    pub fn remove(&mut self, key: &Variant) -> Option<Variant> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == key)?;
        self.invalidate();
        Some(self.entries.remove(index).1)
    }

    pub fn clear(&mut self) {
        self.invalidate();
        self.entries.clear();
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&Variant, &Variant)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }

    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], VariantError> {
        self.encoded
            .get_or_init(|| encode_root(&Variant::from(self.clone())).map(Vec::into_boxed_slice))
            .as_deref()
            .map_err(|error| *error)
    }

    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        if godot_rs_api::abi::dynamic_value_ownership_token(bytes).is_some()
            || !validate_dynamic_value(AbiValueType::DICTIONARY, bytes)
        {
            return None;
        }
        let value = decode_root(bytes, CallableDecodeMode::RejectOwned).ok()?;
        let VariantValue::Dictionary(value) = value.value else {
            return None;
        };
        Some(value)
    }

    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostDynamicValueToken>,
    ) -> Option<Self> {
        let value = Variant::__from_host_bytes(bytes, ownership)?;
        let VariantValue::Dictionary(value) = value.value else {
            return None;
        };
        Some(value)
    }

    fn invalidate(&mut self) {
        let _ = self.encoded.take();
    }
}

impl Clone for Dictionary {
    fn clone(&self) -> Self {
        Self::from_entries(self.entries.clone())
    }
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_map()
            .entries(self.entries.iter().map(|(key, value)| (key, value)))
            .finish()
    }
}

impl PartialEq for Dictionary {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .entries
                .iter()
                .all(|(key, value)| other.get(key) == Some(value))
    }
}

impl Index<&Variant> for Dictionary {
    type Output = Variant;

    fn index(&self, index: &Variant) -> &Self::Output {
        self.get(index).expect("Dictionary key does not exist")
    }
}

/// Conversion contract used by typed Godot arrays.
#[doc(hidden)]
pub trait VariantConvert: Sized + Clone {
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
    fn to_variant(&self) -> Variant {
        Variant::from(*self)
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Object(value) = value.value else {
            return None;
        };
        Some(ObjectRef::__from_instance_id(value.object.instance_id()))
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
        Some(GodotRef::from_dynamic_parts(
            value.object.instance_id(),
            value.ownership?,
        ))
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
    fn to_variant(&self) -> Variant {
        self.as_ref().map_or_else(
            || Variant::from(ObjectRef::<T>::unresolved()),
            GodotRef::to_variant,
        )
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Object(value) = value.value else {
            return None;
        };
        if !value.object.is_resolved() {
            return Some(None);
        }
        Some(Some(GodotRef::from_dynamic_parts(
            value.object.instance_id(),
            value.ownership?,
        )))
    }
}

impl<T: VariantConvert> From<Array<T>> for Variant {
    fn from(values: Array<T>) -> Self {
        let values = values
            .into_vec()
            .into_iter()
            .map(|value| value.to_variant())
            .collect();
        Self::new(VariantValue::Array(Array::from_vec(values)))
    }
}

impl<T: VariantConvert> VariantConvert for Array<T> {
    fn to_variant(&self) -> Variant {
        Variant::from(self.clone())
    }

    fn from_variant(value: Variant) -> Option<Self> {
        let VariantValue::Array(values) = value.value else {
            return None;
        };
        values
            .into_vec()
            .into_iter()
            .map(T::from_variant)
            .collect::<Option<Vec<_>>>()
            .map(Self::from_vec)
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
            if values.len() > MAX_CONTAINER_ELEMENTS {
                return Err(VariantError::new(
                    "Godot Array exceeds the element-count limit",
                ));
            }
            output.extend_from_slice(&(values.len() as u64).to_le_bytes());
            for value in values {
                encode_node(value, output, depth + 1)?;
            }
        }
        VariantValue::Dictionary(values) => {
            if values.len() > MAX_CONTAINER_ELEMENTS {
                return Err(VariantError::new(
                    "Godot Dictionary exceeds the entry-count limit",
                ));
            }
            output.extend_from_slice(&(values.len() as u64).to_le_bytes());
            for (key, value) in values.iter() {
                encode_node(key, output, depth + 1)?;
                encode_node(value, output, depth + 1)?;
            }
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
    if flags != 0 {
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
            let ownership = match godot_rs_api::abi::callable_value_ownership_token(payload) {
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
    Ok(Variant::new(VariantValue::Array(Array::from_vec(values))))
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
    Ok(Variant::new(VariantValue::Dictionary(
        Dictionary::from_entries(entries),
    )))
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
    fn nested_dynamic_values_round_trip_canonically() {
        let mut nested = Dictionary::new();
        nested.insert("name", "玩家");
        nested.insert("position", Vector3::new(1.0, -2.0, 3.5));
        nested.insert(
            "values",
            Array::from(vec![
                Variant::from(42_i64),
                Variant::from(PackedByteArray::from(vec![0, 127, 255])),
            ]),
        );
        let value = Variant::from(nested);
        let bytes = value.__bytes().expect("valid dynamic value");
        assert_eq!(Variant::__from_bytes(bytes), Some(value));
    }

    #[test]
    fn standard_callables_round_trip_inside_dynamic_containers() {
        let callback = Callable::from_object_method(
            ObjectRef::<crate::engine::Node>::__from_instance_id(42),
            "_ready",
        );
        let mut dictionary = Dictionary::new();
        dictionary.insert("callback", callback.clone());
        let value = Variant::from(dictionary);
        let bytes = value.__bytes().expect("dynamic Callable wire");
        let restored = Variant::__from_bytes(bytes).expect("dynamic Callable");
        let VariantKind::Dictionary(dictionary) = restored.kind() else {
            panic!("Dictionary");
        };
        let VariantKind::Callable(restored) = dictionary[&Variant::from("callback")].kind() else {
            panic!("Callable");
        };
        assert_eq!(restored, &callback);
    }

    #[test]
    fn signals_round_trip_inside_dynamic_containers() {
        let signal = Signal::from_object(
            ObjectRef::<crate::engine::Node>::__from_instance_id(42),
            "已完成",
        );
        let mut dictionary = Dictionary::new();
        dictionary.insert("signal", signal.clone());
        let value = Variant::from(dictionary);
        let bytes = value.__bytes().expect("dynamic Signal wire");
        let restored = Variant::__from_bytes(bytes).expect("dynamic Signal");
        let VariantKind::Dictionary(dictionary) = restored.kind() else {
            panic!("Dictionary");
        };
        let VariantKind::Signal(restored) = dictionary[&Variant::from("signal")].kind() else {
            panic!("Signal");
        };
        assert_eq!(restored, &signal);
    }

    #[test]
    fn typed_arrays_reject_values_of_the_wrong_type() {
        let values = Array::<Vector2>::from(vec![Vector2::new(1.0, 2.0)]);
        let bytes = values.__bytes().expect("typed array wire").to_vec();
        assert_eq!(Array::<Vector2>::__from_bytes(&bytes), Some(values));
        assert!(Array::<i64>::__from_bytes(&bytes).is_none());
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
