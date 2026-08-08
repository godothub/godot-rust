use core::ffi::{c_char, c_void};
use core::mem::MaybeUninit;
use core::ptr;

use godot_rs_api::abi::{
    ABI_GODOT_API_CONST, ABI_GODOT_API_MUTATES_BASE, ABI_GODOT_API_STATIC, ABI_GODOT_API_VARARG,
    ABI_GODOT_METHOD_STATIC, ABI_GODOT_METHOD_VARARG, AbiByteSlice, AbiGodotApiKind,
    AbiGodotApiSpecV1, AbiGodotMethodSpecV1, AbiGodotValueSpecV1, AbiPtrcallType, AbiValueType,
    AbiValueV1,
};

use super::callable_value::NativeCallable;
use super::dynamic_value::NativeDynamicValue;
use super::packed_array::NativePackedArray;
use super::runtime::{Interface, active_interface};
use super::signal_value::NativeSignal;
use super::value::{GodotString, GodotStringName};
use crate::error::{EngineError, EngineResult};
use crate::math::{
    Aabb, Basis, Color, Plane, Projection, Quaternion, Rect2, Rect2i, Transform2D, Transform3D,
    Vector2, Vector2i, Vector3, Vector3i, Vector4, Vector4i,
};
use crate::variant::VariantKind;

const MAX_NATIVE_ENGINE_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// Owned result storage used by hidden generated Native call glue.
#[doc(hidden)]
pub struct NativeEngineValue {
    abi: AbiValueV1,
    owned_ref: Option<NativeGodotRefToken>,
    dynamic: Option<crate::variant::Variant>,
    callable: Option<crate::callable::Callable>,
}

impl NativeEngineValue {
    fn plain(abi: AbiValueV1) -> Self {
        Self {
            abi,
            owned_ref: None,
            dynamic: None,
            callable: None,
        }
    }

    fn owned_ref(abi: AbiValueV1, owned_ref: NativeGodotRefToken) -> Self {
        Self {
            abi,
            owned_ref: Some(owned_ref),
            dynamic: None,
            callable: None,
        }
    }

    fn dynamic(abi: AbiValueV1, dynamic: crate::variant::Variant) -> Self {
        Self {
            abi,
            owned_ref: None,
            dynamic: Some(dynamic),
            callable: None,
        }
    }

    fn callable(abi: AbiValueV1, callable: crate::callable::Callable) -> Self {
        Self {
            abi,
            owned_ref: None,
            dynamic: None,
            callable: Some(callable),
        }
    }

    pub(crate) const fn abi(&self) -> AbiValueV1 {
        self.abi
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        AbiValueV1,
        Option<NativeGodotRefToken>,
        Option<crate::variant::Variant>,
        Option<crate::callable::Callable>,
    ) {
        (self.abi, self.owned_ref, self.dynamic, self.callable)
    }
}

pub(crate) struct NativeGodotRefToken {
    storage: usize,
    interface: Interface,
}

impl NativeGodotRefToken {
    fn empty(interface: Interface) -> Self {
        Self {
            storage: 0,
            interface,
        }
    }

    pub(super) fn from_object(
        interface: Interface,
        object: super::sys::GDExtensionObjectPtr,
    ) -> Self {
        let mut value = Self::empty(interface);
        // SAFETY: The null Ref storage is initialized and Godot owns the live
        // RefCounted object pointer.
        unsafe { (interface.ref_set_object)(value.as_mut_ptr().cast(), object) };
        value
    }

    fn object(&self) -> super::sys::GDExtensionObjectPtr {
        // SAFETY: Storage is always an initialized null or populated Ref.
        unsafe { (self.interface.ref_get_object)(self.as_const_ptr().cast()) }
    }

    fn as_const_ptr(&self) -> super::sys::GDExtensionConstTypePtr {
        ptr::from_ref(&self.storage).cast()
    }

    fn as_mut_ptr(&mut self) -> super::sys::GDExtensionTypePtr {
        ptr::from_mut(&mut self.storage).cast()
    }
}

impl Drop for NativeGodotRefToken {
    fn drop(&mut self) {
        // SAFETY: Assigning null to the initialized Ref performs the matching
        // unreference operation exactly once.
        unsafe { (self.interface.ref_set_object)(self.as_mut_ptr().cast(), ptr::null_mut()) };
    }
}

enum NativeCallValue {
    Void,
    Bool(u8),
    // Godot's official PtrToArg contract transports every integer narrower
    // than 64 bits through an int64_t slot, including unsigned values.
    I8(i64),
    I16(i64),
    I32(i64),
    I64(i64),
    U8(i64),
    U16(i64),
    U32(i64),
    U64(u64),
    // Godot transports float through a double slot for ptrcall.
    F32(f64),
    F64(f64),
    Text(Box<NativeTextValue>),
    Vector2(Vector2),
    Vector2i(Vector2i),
    Rect2(Rect2),
    Rect2i(Rect2i),
    Vector3(Vector3),
    Vector3i(Vector3i),
    Transform2D(Transform2D),
    Vector4(Vector4),
    Vector4i(Vector4i),
    Plane(Plane),
    Quaternion(Quaternion),
    Aabb(Aabb),
    Basis(Basis),
    Transform3D(Transform3D),
    Projection(Projection),
    Color(Color),
    Object(super::sys::GDExtensionObjectPtr),
    RefCounted(Box<NativeGodotRefToken>),
    Rid(u64),
    PackedArray(Box<NativePackedArray>),
    Dynamic(Box<NativeDynamicValue>),
    Callable(Box<NativeCallable>),
    Signal(Box<NativeSignal>),
}

impl NativeCallValue {
    fn from_argument(
        interface: Interface,
        contract: &AbiGodotValueSpecV1,
        value: AbiValueV1,
    ) -> EngineResult<Self> {
        if value.type_ != contract.value_type || value.reserved_flags != 0 {
            return Err(EngineError::invalid_argument(format!(
                "Native engine argument type {} does not match generated type {} \
                 or carries unsupported flags 0x{:08x}",
                value.type_.0, contract.value_type.0, value.reserved_flags,
            )));
        }
        if !matches!(
            contract.ptrcall_type,
            AbiPtrcallType::STRING
                | AbiPtrcallType::STRING_NAME
                | AbiPtrcallType::NODE_PATH
                | AbiPtrcallType::TRANSFORM2D
                | AbiPtrcallType::AABB
                | AbiPtrcallType::BASIS
                | AbiPtrcallType::TRANSFORM3D
                | AbiPtrcallType::PROJECTION
                | AbiPtrcallType::PACKED_BYTE_ARRAY
                | AbiPtrcallType::PACKED_INT32_ARRAY
                | AbiPtrcallType::PACKED_INT64_ARRAY
                | AbiPtrcallType::PACKED_FLOAT32_ARRAY
                | AbiPtrcallType::PACKED_FLOAT64_ARRAY
                | AbiPtrcallType::PACKED_STRING_ARRAY
                | AbiPtrcallType::PACKED_VECTOR2_ARRAY
                | AbiPtrcallType::PACKED_VECTOR3_ARRAY
                | AbiPtrcallType::PACKED_COLOR_ARRAY
                | AbiPtrcallType::PACKED_VECTOR4_ARRAY
                | AbiPtrcallType::VARIANT
                | AbiPtrcallType::ARRAY
                | AbiPtrcallType::DICTIONARY
                | AbiPtrcallType::CALLABLE
                | AbiPtrcallType::SIGNAL
        ) && value.payload[1] != 0
        {
            return Err(EngineError::invalid_argument(
                "Native engine argument has a non-zero reserved payload",
            ));
        }
        match contract.ptrcall_type {
            AbiPtrcallType::BOOL if value.payload[0] <= 1 => Ok(Self::Bool(value.payload[0] as u8)),
            AbiPtrcallType::I8 => narrow_signed::<i8>(value.payload[0])
                .map(i64::from)
                .map(Self::I8),
            AbiPtrcallType::I16 => narrow_signed::<i16>(value.payload[0])
                .map(i64::from)
                .map(Self::I16),
            AbiPtrcallType::I32 => narrow_signed::<i32>(value.payload[0])
                .map(i64::from)
                .map(Self::I32),
            AbiPtrcallType::I64 => Ok(Self::I64(value.payload[0] as i64)),
            AbiPtrcallType::U8 => narrow_unsigned::<u8>(value.payload[0])
                .map(i64::from)
                .map(Self::U8),
            AbiPtrcallType::U16 => narrow_unsigned::<u16>(value.payload[0])
                .map(i64::from)
                .map(Self::U16),
            AbiPtrcallType::U32 => narrow_unsigned::<u32>(value.payload[0])
                .map(i64::from)
                .map(Self::U32),
            AbiPtrcallType::U64 => Ok(Self::U64(value.payload[0])),
            AbiPtrcallType::F32 => {
                let value = f64::from_bits(value.payload[0]);
                let narrowed = value as f32;
                if value.is_finite() && !narrowed.is_finite() {
                    Err(EngineError::invalid_argument(
                        "Native f32 argument is outside the supported range",
                    ))
                } else {
                    Ok(Self::F32(f64::from(narrowed)))
                }
            }
            AbiPtrcallType::F64 => Ok(Self::F64(f64::from_bits(value.payload[0]))),
            AbiPtrcallType::STRING | AbiPtrcallType::STRING_NAME | AbiPtrcallType::NODE_PATH => {
                NativeTextValue::from_argument(interface, contract, value)
                    .map(Box::new)
                    .map(Self::Text)
            }
            AbiPtrcallType::VECTOR2 => value
                .vector2()
                .map(|[x, y]| Self::Vector2(Vector2::new(x, y)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::VECTOR2I => value
                .vector2i()
                .map(|[x, y]| Self::Vector2i(Vector2i::new(x, y)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::RECT2 => value
                .rect2()
                .map(|[x, y, width, height]| {
                    Self::Rect2(Rect2::from_components(x, y, width, height))
                })
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::RECT2I => value
                .rect2i()
                .map(|[x, y, width, height]| {
                    Self::Rect2i(Rect2i::from_components(x, y, width, height))
                })
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::VECTOR3 => value
                .vector3()
                .map(|[x, y, z]| Self::Vector3(Vector3::new(x, y, z)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::VECTOR3I => value
                .vector3i()
                .map(|[x, y, z]| Self::Vector3i(Vector3i::new(x, y, z)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::TRANSFORM2D => fixed_f32_argument::<6, _>(
                value,
                AbiValueType::TRANSFORM2D,
                Transform2D::__from_components,
            )
            .map(Self::Transform2D),
            AbiPtrcallType::VECTOR4 => value
                .vector4()
                .map(|[x, y, z, w]| Self::Vector4(Vector4::new(x, y, z, w)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::VECTOR4I => value
                .vector4i()
                .map(|[x, y, z, w]| Self::Vector4i(Vector4i::new(x, y, z, w)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::PLANE => value
                .plane()
                .map(|[x, y, z, d]| Self::Plane(Plane::from_components(x, y, z, d)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::QUATERNION => value
                .quaternion()
                .map(|[x, y, z, w]| Self::Quaternion(Quaternion::new(x, y, z, w)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::AABB => {
                fixed_f32_argument::<6, _>(value, AbiValueType::AABB, Aabb::__from_components)
                    .map(Self::Aabb)
            }
            AbiPtrcallType::BASIS => {
                fixed_f32_argument::<9, _>(value, AbiValueType::BASIS, Basis::__from_components)
                    .map(Self::Basis)
            }
            AbiPtrcallType::TRANSFORM3D => fixed_f32_argument::<12, _>(
                value,
                AbiValueType::TRANSFORM3D,
                Transform3D::__from_components,
            )
            .map(Self::Transform3D),
            AbiPtrcallType::PROJECTION => fixed_f32_argument::<16, _>(
                value,
                AbiValueType::PROJECTION,
                Projection::__from_components,
            )
            .map(Self::Projection),
            AbiPtrcallType::COLOR => value
                .color()
                .map(|[r, g, b, a]| Self::Color(Color::rgba(r, g, b, a)))
                .ok_or_else(invalid_argument_value),
            AbiPtrcallType::OBJECT => {
                let object = if value.payload[0] == 0 {
                    ptr::null_mut()
                } else {
                    // SAFETY: Godot owns and synchronizes its instance-ID table.
                    unsafe { (interface.object_get_instance_from_id)(value.payload[0]) }
                };
                if value.payload[0] != 0 && object.is_null() {
                    return Err(EngineError::stale_object(format!(
                        "Godot Object {} no longer exists",
                        value.payload[0]
                    )));
                }
                validate_object_contract(interface, object, contract)?;
                Ok(Self::Object(object))
            }
            AbiPtrcallType::REFCOUNTED_OBJECT => {
                let object = if value.payload[0] == 0 {
                    ptr::null_mut()
                } else {
                    // SAFETY: Godot owns and synchronizes its instance-ID table.
                    unsafe { (interface.object_get_instance_from_id)(value.payload[0]) }
                };
                if value.payload[0] != 0 && object.is_null() {
                    return Err(EngineError::stale_object(format!(
                        "Godot RefCounted Object {} no longer exists",
                        value.payload[0]
                    )));
                }
                validate_object_contract(interface, object, contract)?;
                Ok(Self::RefCounted(Box::new(
                    NativeGodotRefToken::from_object(interface, object),
                )))
            }
            AbiPtrcallType::RID => Ok(Self::Rid(value.payload[0])),
            AbiPtrcallType::PACKED_BYTE_ARRAY
            | AbiPtrcallType::PACKED_INT32_ARRAY
            | AbiPtrcallType::PACKED_INT64_ARRAY
            | AbiPtrcallType::PACKED_FLOAT32_ARRAY
            | AbiPtrcallType::PACKED_FLOAT64_ARRAY
            | AbiPtrcallType::PACKED_STRING_ARRAY
            | AbiPtrcallType::PACKED_VECTOR2_ARRAY
            | AbiPtrcallType::PACKED_VECTOR3_ARRAY
            | AbiPtrcallType::PACKED_COLOR_ARRAY
            | AbiPtrcallType::PACKED_VECTOR4_ARRAY => {
                NativePackedArray::from_argument(interface, contract.ptrcall_type, value)
                    .map(Box::new)
                    .map(Self::PackedArray)
            }
            AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY => {
                NativeDynamicValue::from_argument(interface, contract, value)
                    .map(Box::new)
                    .map(Self::Dynamic)
            }
            AbiPtrcallType::CALLABLE => NativeCallable::from_argument(interface, value)
                .map(Box::new)
                .map(Self::Callable),
            AbiPtrcallType::SIGNAL => NativeSignal::from_argument(interface, value)
                .map(Box::new)
                .map(Self::Signal),
            _ => Err(EngineError::unavailable(
                "this generated value type is not yet available in Native engine calls",
            )),
        }
    }

    fn from_rust_variant(
        interface: Interface,
        contract: &AbiGodotValueSpecV1,
        value: &crate::variant::Variant,
    ) -> EngineResult<Self> {
        use crate::engine::EngineArgument;

        // A Godot `Variant` return contract describes the wrapper, not the
        // concrete value currently stored inside it. Preserve that wrapper so
        // values such as StringName do not get rejected against VARIANT.
        if contract.ptrcall_type == AbiPtrcallType::VARIANT {
            return Self::from_argument(interface, contract, value.__into_engine_argument());
        }

        let abi = match value.kind() {
            VariantKind::Nil => AbiValueV1::NIL,
            VariantKind::Bool(value) => value.__into_engine_argument(),
            VariantKind::Int(value) if contract.value_type == AbiValueType::U64 => {
                AbiValueV1::from_u64(value as u64)
            }
            VariantKind::Int(value) => value.__into_engine_argument(),
            VariantKind::Float(value) => value.__into_engine_argument(),
            VariantKind::String(value) => {
                AbiValueV1::from_borrowed_bytes(AbiValueType::STRING, value.as_bytes())
            }
            VariantKind::StringName(value) => AbiValueV1::from_borrowed_bytes(
                AbiValueType::STRING_NAME,
                value.as_str().as_bytes(),
            ),
            VariantKind::NodePath(value) => {
                AbiValueV1::from_borrowed_bytes(AbiValueType::NODE_PATH, value.as_str().as_bytes())
            }
            VariantKind::Object(value) => AbiValueV1::from_object_id(value.instance_id()),
            VariantKind::Vector2(value) => value.__into_engine_argument(),
            VariantKind::Vector2i(value) => value.__into_engine_argument(),
            VariantKind::Vector3(value) => value.__into_engine_argument(),
            VariantKind::Vector3i(value) => value.__into_engine_argument(),
            VariantKind::Vector4(value) => value.__into_engine_argument(),
            VariantKind::Vector4i(value) => value.__into_engine_argument(),
            VariantKind::Rect2(value) => value.__into_engine_argument(),
            VariantKind::Rect2i(value) => value.__into_engine_argument(),
            VariantKind::Quaternion(value) => value.__into_engine_argument(),
            VariantKind::Plane(value) => value.__into_engine_argument(),
            VariantKind::Transform2D(value) => value.__into_engine_argument(),
            VariantKind::Aabb(value) => value.__into_engine_argument(),
            VariantKind::Basis(value) => value.__into_engine_argument(),
            VariantKind::Transform3D(value) => value.__into_engine_argument(),
            VariantKind::Projection(value) => value.__into_engine_argument(),
            VariantKind::Color(value) => value.__into_engine_argument(),
            VariantKind::Rid(value) => value.__into_engine_argument(),
            VariantKind::PackedByteArray(value) => value.__into_engine_argument(),
            VariantKind::PackedInt32Array(value) => value.__into_engine_argument(),
            VariantKind::PackedInt64Array(value) => value.__into_engine_argument(),
            VariantKind::PackedFloat32Array(value) => value.__into_engine_argument(),
            VariantKind::PackedFloat64Array(value) => value.__into_engine_argument(),
            VariantKind::PackedStringArray(value) => value.__into_engine_argument(),
            VariantKind::PackedVector2Array(value) => value.__into_engine_argument(),
            VariantKind::PackedVector3Array(value) => value.__into_engine_argument(),
            VariantKind::PackedColorArray(value) => value.__into_engine_argument(),
            VariantKind::PackedVector4Array(value) => value.__into_engine_argument(),
            VariantKind::Callable(value) => value.__into_engine_argument(),
            VariantKind::Signal(value) => value.__into_engine_argument(),
            VariantKind::Array(value) => value.__into_engine_argument(),
            VariantKind::Dictionary(value) => value.__into_engine_argument(),
        };
        Self::from_argument(interface, contract, abi)
    }

    fn output(interface: Interface, contract: &AbiGodotValueSpecV1) -> EngineResult<Self> {
        match contract.ptrcall_type {
            AbiPtrcallType::VOID => Ok(Self::Void),
            AbiPtrcallType::BOOL => Ok(Self::Bool(0)),
            AbiPtrcallType::I8 => Ok(Self::I8(0)),
            AbiPtrcallType::I16 => Ok(Self::I16(0)),
            AbiPtrcallType::I32 => Ok(Self::I32(0)),
            AbiPtrcallType::I64 => Ok(Self::I64(0)),
            AbiPtrcallType::U8 => Ok(Self::U8(0)),
            AbiPtrcallType::U16 => Ok(Self::U16(0)),
            AbiPtrcallType::U32 => Ok(Self::U32(0)),
            AbiPtrcallType::U64 => Ok(Self::U64(0)),
            AbiPtrcallType::F32 => Ok(Self::F32(0.0)),
            AbiPtrcallType::F64 => Ok(Self::F64(0.0)),
            AbiPtrcallType::STRING | AbiPtrcallType::STRING_NAME | AbiPtrcallType::NODE_PATH => {
                Ok(Self::Text(Box::new(NativeTextValue::empty(
                    interface,
                    contract.ptrcall_type,
                )?)))
            }
            AbiPtrcallType::VECTOR2 => Ok(Self::Vector2(Vector2::default())),
            AbiPtrcallType::VECTOR2I => Ok(Self::Vector2i(Vector2i::default())),
            AbiPtrcallType::RECT2 => Ok(Self::Rect2(Rect2::default())),
            AbiPtrcallType::RECT2I => Ok(Self::Rect2i(Rect2i::default())),
            AbiPtrcallType::VECTOR3 => Ok(Self::Vector3(Vector3::default())),
            AbiPtrcallType::VECTOR3I => Ok(Self::Vector3i(Vector3i::default())),
            AbiPtrcallType::TRANSFORM2D => Ok(Self::Transform2D(Transform2D::default())),
            AbiPtrcallType::VECTOR4 => Ok(Self::Vector4(Vector4::default())),
            AbiPtrcallType::VECTOR4I => Ok(Self::Vector4i(Vector4i::default())),
            AbiPtrcallType::PLANE => Ok(Self::Plane(Plane::default())),
            AbiPtrcallType::QUATERNION => Ok(Self::Quaternion(Quaternion::default())),
            AbiPtrcallType::AABB => Ok(Self::Aabb(Aabb::default())),
            AbiPtrcallType::BASIS => Ok(Self::Basis(Basis::default())),
            AbiPtrcallType::TRANSFORM3D => Ok(Self::Transform3D(Transform3D::default())),
            AbiPtrcallType::PROJECTION => Ok(Self::Projection(Projection::default())),
            AbiPtrcallType::COLOR => Ok(Self::Color(Color::default())),
            AbiPtrcallType::OBJECT => Ok(Self::Object(ptr::null_mut())),
            AbiPtrcallType::REFCOUNTED_OBJECT => Ok(Self::RefCounted(Box::new(
                NativeGodotRefToken::empty(interface),
            ))),
            AbiPtrcallType::RID => Ok(Self::Rid(0)),
            AbiPtrcallType::PACKED_BYTE_ARRAY
            | AbiPtrcallType::PACKED_INT32_ARRAY
            | AbiPtrcallType::PACKED_INT64_ARRAY
            | AbiPtrcallType::PACKED_FLOAT32_ARRAY
            | AbiPtrcallType::PACKED_FLOAT64_ARRAY
            | AbiPtrcallType::PACKED_STRING_ARRAY
            | AbiPtrcallType::PACKED_VECTOR2_ARRAY
            | AbiPtrcallType::PACKED_VECTOR3_ARRAY
            | AbiPtrcallType::PACKED_COLOR_ARRAY
            | AbiPtrcallType::PACKED_VECTOR4_ARRAY => {
                NativePackedArray::output(interface, contract.ptrcall_type)
                    .map(Box::new)
                    .map(Self::PackedArray)
            }
            AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY => {
                NativeDynamicValue::output(interface, contract.ptrcall_type)
                    .map(Box::new)
                    .map(Self::Dynamic)
            }
            AbiPtrcallType::CALLABLE => NativeCallable::empty(interface)
                .map(Box::new)
                .map(Self::Callable),
            AbiPtrcallType::SIGNAL => NativeSignal::empty(interface)
                .map(Box::new)
                .map(Self::Signal),
            _ => Err(EngineError::unavailable(
                "this generated return type is not yet available in Native engine calls",
            )),
        }
    }

    fn constructor_output(
        interface: Interface,
        contract: &AbiGodotValueSpecV1,
    ) -> EngineResult<Self> {
        match contract.ptrcall_type {
            AbiPtrcallType::STRING | AbiPtrcallType::STRING_NAME | AbiPtrcallType::NODE_PATH => {
                Ok(Self::Text(Box::new(NativeTextValue::output(
                    interface,
                    contract.ptrcall_type,
                )?)))
            }
            AbiPtrcallType::PACKED_BYTE_ARRAY
            | AbiPtrcallType::PACKED_INT32_ARRAY
            | AbiPtrcallType::PACKED_INT64_ARRAY
            | AbiPtrcallType::PACKED_FLOAT32_ARRAY
            | AbiPtrcallType::PACKED_FLOAT64_ARRAY
            | AbiPtrcallType::PACKED_STRING_ARRAY
            | AbiPtrcallType::PACKED_VECTOR2_ARRAY
            | AbiPtrcallType::PACKED_VECTOR3_ARRAY
            | AbiPtrcallType::PACKED_COLOR_ARRAY
            | AbiPtrcallType::PACKED_VECTOR4_ARRAY => {
                NativePackedArray::constructor_output(interface, contract.ptrcall_type)
                    .map(Box::new)
                    .map(Self::PackedArray)
            }
            AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY => {
                NativeDynamicValue::constructor_output(interface, contract.ptrcall_type)
                    .map(Box::new)
                    .map(Self::Dynamic)
            }
            AbiPtrcallType::CALLABLE => NativeCallable::uninitialized(interface)
                .map(Box::new)
                .map(Self::Callable),
            AbiPtrcallType::SIGNAL => NativeSignal::uninitialized(interface)
                .map(Box::new)
                .map(Self::Signal),
            _ => Self::output(interface, contract),
        }
    }

    fn as_const_ptr(&self) -> super::sys::GDExtensionConstTypePtr {
        match self {
            Self::Void => ptr::null(),
            Self::Bool(value) => ptr::from_ref(value).cast(),
            Self::I8(value) => ptr::from_ref(value).cast(),
            Self::I16(value) => ptr::from_ref(value).cast(),
            Self::I32(value) => ptr::from_ref(value).cast(),
            Self::I64(value) => ptr::from_ref(value).cast(),
            Self::U8(value) => ptr::from_ref(value).cast(),
            Self::U16(value) => ptr::from_ref(value).cast(),
            Self::U32(value) => ptr::from_ref(value).cast(),
            Self::U64(value) => ptr::from_ref(value).cast(),
            Self::F32(value) => ptr::from_ref(value).cast(),
            Self::F64(value) => ptr::from_ref(value).cast(),
            Self::Text(value) => value.as_const_ptr(),
            Self::Vector2(value) => ptr::from_ref(value).cast(),
            Self::Vector2i(value) => ptr::from_ref(value).cast(),
            Self::Rect2(value) => ptr::from_ref(value).cast(),
            Self::Rect2i(value) => ptr::from_ref(value).cast(),
            Self::Vector3(value) => ptr::from_ref(value).cast(),
            Self::Vector3i(value) => ptr::from_ref(value).cast(),
            Self::Transform2D(value) => ptr::from_ref(value).cast(),
            Self::Vector4(value) => ptr::from_ref(value).cast(),
            Self::Vector4i(value) => ptr::from_ref(value).cast(),
            Self::Plane(value) => ptr::from_ref(value).cast(),
            Self::Quaternion(value) => ptr::from_ref(value).cast(),
            Self::Aabb(value) => ptr::from_ref(value).cast(),
            Self::Basis(value) => ptr::from_ref(value).cast(),
            Self::Transform3D(value) => ptr::from_ref(value).cast(),
            Self::Projection(value) => ptr::from_ref(value).cast(),
            Self::Color(value) => ptr::from_ref(value).cast(),
            Self::Object(value) => ptr::from_ref(value).cast(),
            Self::RefCounted(value) => value.as_const_ptr(),
            Self::Rid(value) => ptr::from_ref(value).cast(),
            Self::PackedArray(value) => value.as_const_ptr(),
            Self::Dynamic(value) => value.as_const_ptr(),
            Self::Callable(value) => value.as_const_ptr(),
            Self::Signal(value) => value.as_const_ptr(),
        }
    }

    fn as_mut_ptr(&mut self) -> super::sys::GDExtensionTypePtr {
        match self {
            Self::Void => ptr::null_mut(),
            Self::Bool(value) => ptr::from_mut(value).cast(),
            Self::I8(value) => ptr::from_mut(value).cast(),
            Self::I16(value) => ptr::from_mut(value).cast(),
            Self::I32(value) => ptr::from_mut(value).cast(),
            Self::I64(value) => ptr::from_mut(value).cast(),
            Self::U8(value) => ptr::from_mut(value).cast(),
            Self::U16(value) => ptr::from_mut(value).cast(),
            Self::U32(value) => ptr::from_mut(value).cast(),
            Self::U64(value) => ptr::from_mut(value).cast(),
            Self::F32(value) => ptr::from_mut(value).cast(),
            Self::F64(value) => ptr::from_mut(value).cast(),
            Self::Text(value) => value.as_mut_ptr(),
            Self::Vector2(value) => ptr::from_mut(value).cast(),
            Self::Vector2i(value) => ptr::from_mut(value).cast(),
            Self::Rect2(value) => ptr::from_mut(value).cast(),
            Self::Rect2i(value) => ptr::from_mut(value).cast(),
            Self::Vector3(value) => ptr::from_mut(value).cast(),
            Self::Vector3i(value) => ptr::from_mut(value).cast(),
            Self::Transform2D(value) => ptr::from_mut(value).cast(),
            Self::Vector4(value) => ptr::from_mut(value).cast(),
            Self::Vector4i(value) => ptr::from_mut(value).cast(),
            Self::Plane(value) => ptr::from_mut(value).cast(),
            Self::Quaternion(value) => ptr::from_mut(value).cast(),
            Self::Aabb(value) => ptr::from_mut(value).cast(),
            Self::Basis(value) => ptr::from_mut(value).cast(),
            Self::Transform3D(value) => ptr::from_mut(value).cast(),
            Self::Projection(value) => ptr::from_mut(value).cast(),
            Self::Color(value) => ptr::from_mut(value).cast(),
            Self::Object(value) => ptr::from_mut(value).cast(),
            Self::RefCounted(value) => value.as_mut_ptr(),
            Self::Rid(value) => ptr::from_mut(value).cast(),
            Self::PackedArray(value) => value.as_mut_ptr(),
            Self::Dynamic(value) => value.as_mut_ptr(),
            Self::Callable(value) => value.as_mut_ptr(),
            Self::Signal(value) => value.as_mut_ptr(),
        }
    }

    fn into_abi(
        self,
        interface: Interface,
        contract: &AbiGodotValueSpecV1,
    ) -> EngineResult<NativeEngineValue> {
        let value = match (self, contract.ptrcall_type) {
            (Self::Void, AbiPtrcallType::VOID) => AbiValueV1::NIL,
            (Self::Bool(value), AbiPtrcallType::BOOL) if value <= 1 => {
                AbiValueV1::from_bool(value != 0)
            }
            (Self::I8(value), AbiPtrcallType::I8) => {
                AbiValueV1::from_i64(normalize_signed_output::<i8>(value)?)
            }
            (Self::I16(value), AbiPtrcallType::I16) => {
                AbiValueV1::from_i64(normalize_signed_output::<i16>(value)?)
            }
            (Self::I32(value), AbiPtrcallType::I32) => {
                AbiValueV1::from_i64(normalize_signed_output::<i32>(value)?)
            }
            (Self::I64(value), AbiPtrcallType::I64) => AbiValueV1::from_i64(value),
            (Self::U8(value), AbiPtrcallType::U8) => {
                AbiValueV1::from_u64(normalize_unsigned_output::<u8>(value)?)
            }
            (Self::U16(value), AbiPtrcallType::U16) => {
                AbiValueV1::from_u64(normalize_unsigned_output::<u16>(value)?)
            }
            (Self::U32(value), AbiPtrcallType::U32) => {
                AbiValueV1::from_u64(normalize_unsigned_output::<u32>(value)?)
            }
            (Self::U64(value), AbiPtrcallType::U64) => AbiValueV1::from_u64(value),
            (Self::F32(value), AbiPtrcallType::F32) => {
                AbiValueV1::from_f64(normalize_f32_output(value)?)
            }
            (Self::F64(value), AbiPtrcallType::F64) => AbiValueV1::from_f64(value),
            (
                Self::Text(value),
                AbiPtrcallType::STRING | AbiPtrcallType::STRING_NAME | AbiPtrcallType::NODE_PATH,
            ) => {
                let text = value.to_rust_string()?;
                crate::module::owned_text(contract.value_type, text)
            }
            (Self::Vector2(value), AbiPtrcallType::VECTOR2) => {
                AbiValueV1::from_vector2(value.x, value.y)
            }
            (Self::Vector2i(value), AbiPtrcallType::VECTOR2I) => {
                AbiValueV1::from_vector2i(value.x, value.y)
            }
            (Self::Rect2(value), AbiPtrcallType::RECT2) => AbiValueV1::from_rect2(
                value.position.x,
                value.position.y,
                value.size.x,
                value.size.y,
            ),
            (Self::Rect2i(value), AbiPtrcallType::RECT2I) => AbiValueV1::from_rect2i(
                value.position.x,
                value.position.y,
                value.size.x,
                value.size.y,
            ),
            (Self::Vector3(value), AbiPtrcallType::VECTOR3) => {
                AbiValueV1::from_vector3(value.x, value.y, value.z)
            }
            (Self::Vector3i(value), AbiPtrcallType::VECTOR3I) => {
                AbiValueV1::from_vector3i(value.x, value.y, value.z)
            }
            (Self::Transform2D(value), AbiPtrcallType::TRANSFORM2D) => {
                crate::module::owned_f32_components(AbiValueType::TRANSFORM2D, value.__components())
            }
            (Self::Vector4(value), AbiPtrcallType::VECTOR4) => {
                AbiValueV1::from_vector4(value.x, value.y, value.z, value.w)
            }
            (Self::Vector4i(value), AbiPtrcallType::VECTOR4I) => {
                AbiValueV1::from_vector4i(value.x, value.y, value.z, value.w)
            }
            (Self::Plane(value), AbiPtrcallType::PLANE) => {
                AbiValueV1::from_plane(value.normal.x, value.normal.y, value.normal.z, value.d)
            }
            (Self::Quaternion(value), AbiPtrcallType::QUATERNION) => {
                AbiValueV1::from_quaternion(value.x, value.y, value.z, value.w)
            }
            (Self::Aabb(value), AbiPtrcallType::AABB) => {
                crate::module::owned_f32_components(AbiValueType::AABB, value.__components())
            }
            (Self::Basis(value), AbiPtrcallType::BASIS) => {
                crate::module::owned_f32_components(AbiValueType::BASIS, value.__components())
            }
            (Self::Transform3D(value), AbiPtrcallType::TRANSFORM3D) => {
                crate::module::owned_f32_components(AbiValueType::TRANSFORM3D, value.__components())
            }
            (Self::Projection(value), AbiPtrcallType::PROJECTION) => {
                crate::module::owned_f32_components(AbiValueType::PROJECTION, value.__components())
            }
            (Self::Color(value), AbiPtrcallType::COLOR) => {
                AbiValueV1::from_color(value.r, value.g, value.b, value.a)
            }
            (Self::Object(value), AbiPtrcallType::OBJECT) => {
                validate_object_contract(interface, value, contract).map_err(|error| {
                    EngineError::invalid_result(format!(
                        "Native engine returned an invalid typed Object: {error}"
                    ))
                })?;
                let instance_id = if value.is_null() {
                    0
                } else {
                    // SAFETY: Godot initialized the returned live Object pointer.
                    unsafe { (interface.object_get_instance_id)(value) }
                };
                AbiValueV1::from_object_id(instance_id)
            }
            (Self::RefCounted(value), AbiPtrcallType::REFCOUNTED_OBJECT) => {
                let object = value.object();
                validate_object_contract(interface, object, contract).map_err(|error| {
                    EngineError::invalid_result(format!(
                        "Native engine returned an invalid typed RefCounted Object: {error}"
                    ))
                })?;
                let instance_id = if object.is_null() {
                    0
                } else {
                    // SAFETY: Godot initialized the live RefCounted Object pointer.
                    unsafe { (interface.object_get_instance_id)(object) }
                };
                let abi = AbiValueV1::from_object_id(instance_id);
                if abi.type_ != contract.value_type {
                    return Err(EngineError::invalid_result(
                        "Native Ref ptrcall normalized the wrong return type",
                    ));
                }
                if instance_id == 0 {
                    return Ok(NativeEngineValue::plain(abi));
                }
                return Ok(NativeEngineValue::owned_ref(abi, *value));
            }
            (Self::Rid(value), AbiPtrcallType::RID) => AbiValueV1::from_rid(value),
            (
                Self::PackedArray(value),
                AbiPtrcallType::PACKED_BYTE_ARRAY
                | AbiPtrcallType::PACKED_INT32_ARRAY
                | AbiPtrcallType::PACKED_INT64_ARRAY
                | AbiPtrcallType::PACKED_FLOAT32_ARRAY
                | AbiPtrcallType::PACKED_FLOAT64_ARRAY
                | AbiPtrcallType::PACKED_STRING_ARRAY
                | AbiPtrcallType::PACKED_VECTOR2_ARRAY
                | AbiPtrcallType::PACKED_VECTOR3_ARRAY
                | AbiPtrcallType::PACKED_COLOR_ARRAY
                | AbiPtrcallType::PACKED_VECTOR4_ARRAY,
            ) => value.into_abi()?,
            (
                Self::Dynamic(value),
                AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY,
            ) => {
                let rust = value.to_rust()?;
                let bytes = rust.__bytes().map_err(|error| {
                    EngineError::invalid_result(format!(
                        "Native dynamic result could not be encoded: {error}"
                    ))
                })?;
                let abi = crate::module::owned_bytes(contract.value_type, bytes.to_vec());
                return Ok(NativeEngineValue::dynamic(abi, rust));
            }
            (Self::Callable(value), AbiPtrcallType::CALLABLE) => {
                let rust = value.into_rust()?;
                let bytes = rust.__bytes().map_err(|error| {
                    EngineError::invalid_result(format!(
                        "Native Callable result could not be encoded: {}",
                        error.message()
                    ))
                })?;
                let abi = crate::module::owned_bytes(AbiValueType::CALLABLE, bytes.to_vec());
                return Ok(NativeEngineValue::callable(abi, rust));
            }
            (Self::Signal(value), AbiPtrcallType::SIGNAL) => value.into_abi()?,
            _ => {
                return Err(EngineError::invalid_result(
                    "Native ptrcall returned storage that violates its generated contract",
                ));
            }
        };
        if value.type_ != contract.value_type {
            return Err(EngineError::invalid_result(
                "Native ptrcall normalized the wrong generated return type",
            ));
        }
        Ok(NativeEngineValue::plain(value))
    }

    fn mark_output_initialized(&mut self) {
        match self {
            Self::Text(value) => value.mark_initialized(),
            Self::PackedArray(value) => value.mark_initialized(),
            Self::Dynamic(value) => value.mark_initialized(),
            Self::Callable(value) => value.mark_initialized(),
            Self::Signal(value) => value.mark_initialized(),
            _ => {}
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Self::I8(value) => Some(*value),
            Self::I16(value) => Some(*value),
            Self::I32(value) => Some(*value),
            Self::I64(value) => Some(*value),
            Self::U8(value) => Some(*value),
            Self::U16(value) => Some(*value),
            Self::U32(value) => Some(*value),
            Self::U64(value) => i64::try_from(*value).ok(),
            _ => None,
        }
    }

    fn to_native_variant(
        &self,
        interface: Interface,
        ptrcall_type: AbiPtrcallType,
    ) -> EngineResult<super::dynamic_value::NativeVariant> {
        if let Self::Dynamic(value) = self {
            return value.to_native_variant();
        }
        if let Self::RefCounted(value) = self {
            let object = value.object();
            return super::dynamic_value::NativeVariant::from_raw(
                interface,
                ptrcall_variant_type(AbiPtrcallType::OBJECT),
                ptr::from_ref(&object).cast(),
            );
        }
        if matches!(ptrcall_type, AbiPtrcallType::VOID | AbiPtrcallType::VARIANT) {
            return Err(EngineError::invalid_argument(
                "Native variable argument has no concrete Variant type",
            ));
        }
        super::dynamic_value::NativeVariant::from_raw(
            interface,
            ptrcall_variant_type(ptrcall_type),
            self.as_const_ptr(),
        )
    }
}

pub(super) struct NativeTextValue {
    storage: MaybeUninit<usize>,
    interface: Interface,
    variant_type: super::sys::GDExtensionVariantType,
    initialized: bool,
}

impl NativeTextValue {
    pub(super) fn empty(interface: Interface, ptrcall_type: AbiPtrcallType) -> EngineResult<Self> {
        let mut value = Self::output(interface, ptrcall_type)?;
        let constructor = value.constructor(0, "text default")?;
        // SAFETY: The default constructor initializes this exact text storage
        // and accepts no arguments.
        unsafe { constructor(value.as_mut_ptr(), ptr::null()) };
        value.mark_initialized();
        Ok(value)
    }

    fn from_argument(
        interface: Interface,
        contract: &AbiGodotValueSpecV1,
        value: AbiValueV1,
    ) -> EngineResult<Self> {
        let (pointer, length) = value
            .byte_range(contract.value_type)
            .ok_or_else(invalid_argument_value)?;
        if length > MAX_NATIVE_ENGINE_TEXT_BYTES {
            return Err(EngineError::invalid_argument(
                "Native engine text argument exceeds the supported boundary",
            ));
        }
        // SAFETY: The generated wrapper retains the bounded bytes through this
        // synchronous construction.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let text = core::str::from_utf8(bytes)
            .map_err(|_| EngineError::invalid_argument("Native engine text is not valid UTF-8"))?;
        Self::from_rust(interface, contract.ptrcall_type, text)
    }

    pub(super) fn from_rust(
        interface: Interface,
        ptrcall_type: AbiPtrcallType,
        text: &str,
    ) -> EngineResult<Self> {
        if text.len() > MAX_NATIVE_ENGINE_TEXT_BYTES {
            return Err(EngineError::invalid_argument(
                "Native engine text argument exceeds the supported boundary",
            ));
        }
        let mut native = Self::output(interface, ptrcall_type)?;
        let length = i64::try_from(text.len())
            .map_err(|_| EngineError::invalid_argument("Native engine text is too large"))?;
        match ptrcall_type {
            AbiPtrcallType::STRING => {
                // SAFETY: Pointer-sized storage is uninitialized and the
                // official constructor reads exactly the bounded UTF-8 range.
                let error = unsafe {
                    (interface.string_new)(
                        native.as_mut_ptr(),
                        text.as_ptr().cast::<c_char>(),
                        length,
                    )
                };
                if error != 0 {
                    return Err(EngineError::invalid_argument(format!(
                        "Godot rejected Native UTF-8 text with error code {error}"
                    )));
                }
            }
            AbiPtrcallType::STRING_NAME => {
                // SAFETY: Pointer-sized storage is uninitialized and the
                // official constructor reads exactly the bounded UTF-8 range.
                unsafe {
                    (interface.string_name_new)(
                        native.as_mut_ptr(),
                        text.as_ptr().cast::<c_char>(),
                        length,
                    );
                }
            }
            AbiPtrcallType::NODE_PATH => {
                let string = GodotString::new(&interface, text)
                    .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
                let constructor = native.constructor(2, "NodePath(String)")?;
                let arguments = [string.as_ptr()];
                // SAFETY: The selected official constructor accepts one live
                // Godot String and initializes one NodePath.
                unsafe { constructor(native.as_mut_ptr(), arguments.as_ptr()) };
            }
            _ => return Err(invalid_argument_value()),
        }
        native.mark_initialized();
        Ok(native)
    }

    pub(super) fn output(
        interface: Interface,
        ptrcall_type: AbiPtrcallType,
    ) -> EngineResult<NativeTextValue> {
        let variant_type = match ptrcall_type {
            AbiPtrcallType::STRING => {
                super::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING
            }
            AbiPtrcallType::STRING_NAME => {
                super::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME
            }
            AbiPtrcallType::NODE_PATH => {
                super::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH
            }
            _ => {
                return Err(EngineError::invalid_argument(
                    "Native text storage received a non-text ptrcall type",
                ));
            }
        };
        Ok(Self {
            storage: MaybeUninit::uninit(),
            interface,
            variant_type,
            initialized: false,
        })
    }

    fn constructor(
        &self,
        index: i32,
        label: &'static str,
    ) -> EngineResult<
        unsafe extern "C" fn(
            super::sys::GDExtensionUninitializedTypePtr,
            *const super::sys::GDExtensionConstTypePtr,
        ),
    > {
        // SAFETY: The selected type and constructor index come from the same
        // authenticated official API generation.
        unsafe { (self.interface.variant_get_ptr_constructor)(self.variant_type, index) }
            .ok_or_else(|| {
                EngineError::invalid_result(format!(
                    "Godot omitted the generated Native {label} constructor"
                ))
            })
    }

    pub(super) fn to_rust_string(&self) -> EngineResult<String> {
        if !self.initialized {
            return Err(EngineError::invalid_result(
                "Native engine omitted a text return value",
            ));
        }
        if self.variant_type == super::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING
        {
            // SAFETY: This value owns one initialized String until Drop.
            return unsafe { GodotString::copy_ptr_to_rust(&self.interface, self.as_const_ptr()) }
                .map_err(|error| EngineError::invalid_result(error.to_string()));
        }
        let constructor_index = if self.variant_type
            == super::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME
        {
            2
        } else {
            3
        };
        let mut string = Self::output(self.interface, AbiPtrcallType::STRING)?;
        let constructor = string.constructor(constructor_index, "String(text)")?;
        let arguments = [self.as_const_ptr()];
        // SAFETY: Constructor 2 accepts StringName and constructor 3 accepts
        // NodePath; both initialize one Godot String.
        unsafe { constructor(string.as_mut_ptr(), arguments.as_ptr()) };
        string.mark_initialized();
        // SAFETY: The constructor initialized a live String owned by `string`.
        unsafe { GodotString::copy_ptr_to_rust(&self.interface, string.as_const_ptr()) }
            .map_err(|error| EngineError::invalid_result(error.to_string()))
    }

    pub(super) fn as_const_ptr(&self) -> super::sys::GDExtensionConstTypePtr {
        self.storage.as_ptr().cast::<c_void>()
    }

    pub(super) fn as_mut_ptr(&mut self) -> super::sys::GDExtensionTypePtr {
        self.storage.as_mut_ptr().cast::<c_void>()
    }

    pub(super) fn mark_initialized(&mut self) {
        self.initialized = true;
    }
}

impl Drop for NativeTextValue {
    fn drop(&mut self) {
        if !self.initialized {
            return;
        }
        // SAFETY: Runtime initialization required a destructor for every text
        // type accepted here.
        let destructor = unsafe { (self.interface.variant_get_ptr_destructor)(self.variant_type) };
        if let Some(destructor) = destructor {
            // SAFETY: This storage owns one initialized value and Drop runs once.
            unsafe { destructor(self.as_mut_ptr()) };
        } else {
            self.interface.report_error(
                "Godot omitted a Native text destructor after initialization",
                "NativeTextValue::drop",
            );
        }
    }
}

pub(crate) fn invoke_engine_method(
    receiver_id: u64,
    method: &'static AbiGodotMethodSpecV1,
    arguments: &[AbiValueV1],
) -> Option<EngineResult<NativeEngineValue>> {
    let interface = active_interface()?;
    Some(invoke_engine_method_inner(
        interface,
        receiver_id,
        method,
        arguments,
    ))
}

pub(crate) fn invoke_godot_api(
    spec: &'static AbiGodotApiSpecV1,
    base: Option<AbiValueV1>,
    arguments: &[AbiValueV1],
    mutates_base: bool,
) -> Option<EngineResult<(NativeEngineValue, Option<NativeEngineValue>)>> {
    let interface = active_interface()?;
    Some(invoke_godot_api_inner(
        interface,
        spec,
        base,
        arguments,
        mutates_base,
    ))
}

fn invoke_godot_api_inner(
    interface: Interface,
    spec: &'static AbiGodotApiSpecV1,
    base: Option<AbiValueV1>,
    arguments: &[AbiValueV1],
    mutates_base: bool,
) -> EngineResult<(NativeEngineValue, Option<NativeEngineValue>)> {
    let allowed_flags = ABI_GODOT_API_STATIC
        | ABI_GODOT_API_CONST
        | ABI_GODOT_API_VARARG
        | ABI_GODOT_API_MUTATES_BASE;
    if spec.struct_size < AbiGodotApiSpecV1::MINIMUM_SIZE
        || spec.reserved_flags & !allowed_flags != 0
    {
        return Err(EngineError::invalid_argument(
            "Native generated API metadata is incompatible with this SDK",
        ));
    }
    let is_vararg = spec.reserved_flags & ABI_GODOT_API_VARARG != 0;
    let expects_base = spec.base_value.ptrcall_type != AbiPtrcallType::VOID;
    if expects_base != base.is_some()
        || mutates_base != (spec.reserved_flags & ABI_GODOT_API_MUTATES_BASE != 0)
    {
        return Err(EngineError::invalid_argument(
            "Native generated API receiver does not match its contract",
        ));
    }
    let contracts = api_value_contracts(spec)?;
    if (!is_vararg && contracts.len() != arguments.len())
        || (is_vararg && arguments.len() < contracts.len())
    {
        return Err(EngineError::invalid_argument(format!(
            "Native generated API expected {}{} argument(s), received {}",
            contracts.len(),
            if is_vararg { " or more" } else { "" },
            arguments.len()
        )));
    }
    let owner_name = if spec.owner_name.len == 0 {
        "<global>"
    } else {
        abi_text(spec.owner_name, "API owner")?
    };
    let member_name = if spec.member_name.len == 0 {
        "<constructor>"
    } else {
        abi_text(spec.member_name, "API member")?
    };
    let mut native_base = base
        .map(|value| NativeCallValue::from_argument(interface, &spec.base_value, value))
        .transpose()
        .map_err(|error| {
            EngineError::invalid_argument(format!(
                "Native generated API `{owner_name}.{member_name}` rejected its receiver: {error}",
            ))
        })?;
    let variant_contract = AbiGodotValueSpecV1 {
        value_type: AbiValueType::VARIANT,
        ptrcall_type: AbiPtrcallType::VARIANT,
        class_name: AbiByteSlice::EMPTY,
        reserved_flags: 0,
        reserved: [0; 2],
    };
    let native_arguments = arguments
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let contract = contracts.get(index).unwrap_or(&variant_contract);
            NativeCallValue::from_argument(interface, contract, *value).map_err(|error| {
                EngineError::invalid_argument(format!(
                    "Native generated API `{owner_name}.{member_name}` rejected argument \
                     {index} ({} fixed argument(s)): {error}",
                    contracts.len(),
                ))
            })
        })
        .collect::<EngineResult<Vec<_>>>()?;
    let argument_pointers = native_arguments
        .iter()
        .map(NativeCallValue::as_const_ptr)
        .collect::<Vec<_>>();
    let argument_pointer = if argument_pointers.is_empty() {
        ptr::null()
    } else {
        argument_pointers.as_ptr()
    };
    let mut output = if spec.kind == AbiGodotApiKind::BUILTIN_CONSTRUCTOR {
        NativeCallValue::constructor_output(interface, &spec.return_value)?
    } else {
        NativeCallValue::output(interface, &spec.return_value)?
    };
    let owner_type = || -> EngineResult<super::sys::GDExtensionVariantType> {
        if expects_base {
            Ok(ptrcall_variant_type(spec.base_value.ptrcall_type))
        } else {
            let owner = abi_text(spec.owner_name, "builtin owner")?;
            builtin_variant_type(owner).ok_or_else(|| {
                EngineError::invalid_argument(format!(
                    "Native generated API has unknown builtin owner `{owner}`"
                ))
            })
        }
    };
    match spec.kind {
        AbiGodotApiKind::UTILITY_FUNCTION => {
            let member = api_member_name(interface, spec)?;
            let hash = api_numeric_i64(spec.numeric, "utility hash")?;
            let function =
                // SAFETY: Name and hash are authenticated generated metadata.
                unsafe { (interface.variant_get_ptr_utility_function)(member.as_ptr(), hash) }
                    .ok_or_else(|| {
                        EngineError::unavailable(
                            "generated Native utility function is unavailable in this engine",
                        )
                    })?;
            let count = i32::try_from(argument_pointers.len())
                .map_err(|_| EngineError::invalid_argument("Native argument count exceeds i32"))?;
            // SAFETY: Every storage pointer follows the generated contract.
            unsafe { function(output.as_mut_ptr(), argument_pointer, count) };
        }
        AbiGodotApiKind::BUILTIN_CONSTRUCTOR => {
            let index = i32::try_from(spec.numeric).map_err(|_| {
                EngineError::invalid_argument("Native builtin constructor index exceeds i32")
            })?;
            let type_ = ptrcall_variant_type(spec.return_value.ptrcall_type);
            // SAFETY: Type and constructor index are authenticated metadata.
            let function = unsafe { (interface.variant_get_ptr_constructor)(type_, index) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native builtin constructor is unavailable in this engine",
                    )
                })?;
            // SAFETY: Output is uninitialized for owned pointer values and POD
            // constructors overwrite their complete fixed storage.
            unsafe { function(output.as_mut_ptr(), argument_pointer) };
        }
        AbiGodotApiKind::BUILTIN_METHOD => {
            let member = api_member_name(interface, spec)?;
            let hash = api_numeric_i64(spec.numeric, "builtin method hash")?;
            let type_ = owner_type()?;
            let function =
                // SAFETY: Type, name, and hash are authenticated metadata.
                unsafe { (interface.variant_get_ptr_builtin_method)(type_, member.as_ptr(), hash) }
                    .ok_or_else(|| {
                        EngineError::unavailable(
                            "generated Native builtin method is unavailable in this engine",
                        )
                    })?;
            let count = i32::try_from(argument_pointers.len())
                .map_err(|_| EngineError::invalid_argument("Native argument count exceeds i32"))?;
            // SAFETY: Base, arguments, and result use the exact generated storage.
            unsafe {
                function(
                    native_base
                        .as_mut()
                        .map_or(ptr::null_mut(), NativeCallValue::as_mut_ptr),
                    argument_pointer,
                    output.as_mut_ptr(),
                    count,
                );
            }
        }
        AbiGodotApiKind::BUILTIN_OPERATOR => {
            let operator = u32::try_from(spec.numeric)
                .ok()
                .filter(|value| *value < 25)
                .ok_or_else(|| {
                    EngineError::invalid_argument("Native operator ordinal is invalid")
                })?;
            let left_type = owner_type()?;
            let right_type = contracts.first().map_or(
                super::sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
                |contract| ptrcall_variant_type(contract.ptrcall_type),
            );
            // SAFETY: Operator and operand types are authenticated metadata.
            let function = unsafe {
                (interface.variant_get_ptr_operator_evaluator)(
                    super::sys::GDExtensionVariantOperator(operator),
                    left_type,
                    right_type,
                )
            }
            .ok_or_else(|| {
                EngineError::unavailable(
                    "generated Native builtin operator is unavailable in this engine",
                )
            })?;
            let left = native_base
                .as_ref()
                .ok_or_else(|| EngineError::invalid_argument("Native operator has no receiver"))?;
            let right = native_arguments
                .first()
                .map_or(ptr::null(), NativeCallValue::as_const_ptr);
            // SAFETY: Resolver and generated contract select the exact storage.
            unsafe { function(left.as_const_ptr(), right, output.as_mut_ptr()) };
        }
        AbiGodotApiKind::BUILTIN_MEMBER_GETTER => {
            let member = api_member_name(interface, spec)?;
            let type_ = owner_type()?;
            // SAFETY: Type and member are authenticated generated metadata.
            let function = unsafe { (interface.variant_get_ptr_getter)(type_, member.as_ptr()) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native builtin getter is unavailable in this engine",
                    )
                })?;
            let base = native_base
                .as_ref()
                .ok_or_else(|| EngineError::invalid_argument("Native getter has no receiver"))?;
            // SAFETY: Receiver and output follow the exact generated contract.
            unsafe { function(base.as_const_ptr(), output.as_mut_ptr()) };
        }
        AbiGodotApiKind::BUILTIN_MEMBER_SETTER => {
            let member = api_member_name(interface, spec)?;
            let type_ = owner_type()?;
            // SAFETY: Type and member are authenticated generated metadata.
            let function = unsafe { (interface.variant_get_ptr_setter)(type_, member.as_ptr()) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native builtin setter is unavailable in this engine",
                    )
                })?;
            let base = native_base
                .as_mut()
                .ok_or_else(|| EngineError::invalid_argument("Native setter has no receiver"))?;
            let value = native_arguments
                .first()
                .ok_or_else(|| EngineError::invalid_argument("Native setter has no value"))?;
            // SAFETY: Receiver and value follow the exact generated contract.
            unsafe { function(base.as_mut_ptr(), value.as_const_ptr()) };
        }
        AbiGodotApiKind::BUILTIN_INDEXED_GETTER => {
            // SAFETY: Type is authenticated generated metadata.
            let function = unsafe { (interface.variant_get_ptr_indexed_getter)(owner_type()?) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native indexed getter is unavailable in this engine",
                    )
                })?;
            let base = native_base.as_ref().ok_or_else(|| {
                EngineError::invalid_argument("Native indexed getter has no receiver")
            })?;
            let index = native_arguments
                .first()
                .and_then(NativeCallValue::as_i64)
                .ok_or_else(|| {
                    EngineError::invalid_argument("Native builtin index is not an integer")
                })?;
            // SAFETY: Receiver, index, and output follow the generated contract.
            unsafe { function(base.as_const_ptr(), index, output.as_mut_ptr()) };
        }
        AbiGodotApiKind::BUILTIN_INDEXED_SETTER => {
            // SAFETY: Type is authenticated generated metadata.
            let function = unsafe { (interface.variant_get_ptr_indexed_setter)(owner_type()?) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native indexed setter is unavailable in this engine",
                    )
                })?;
            let base = native_base.as_mut().ok_or_else(|| {
                EngineError::invalid_argument("Native indexed setter has no receiver")
            })?;
            let index = native_arguments
                .first()
                .and_then(NativeCallValue::as_i64)
                .ok_or_else(|| {
                    EngineError::invalid_argument("Native builtin index is not an integer")
                })?;
            let value = native_arguments.get(1).ok_or_else(|| {
                EngineError::invalid_argument("Native indexed setter has no value")
            })?;
            // SAFETY: Receiver, index, and value follow the generated contract.
            unsafe { function(base.as_mut_ptr(), index, value.as_const_ptr()) };
        }
        AbiGodotApiKind::BUILTIN_KEYED_GETTER => {
            // SAFETY: Receiver type is authenticated generated metadata.
            let function = unsafe { (interface.variant_get_ptr_keyed_getter)(owner_type()?) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native keyed getter is unavailable in this engine",
                    )
                })?;
            let base = native_base.as_ref().ok_or_else(|| {
                EngineError::invalid_argument("Native keyed getter has no receiver")
            })?;
            let key = native_arguments
                .first()
                .ok_or_else(|| EngineError::invalid_argument("Native keyed getter has no key"))?;
            // SAFETY: Receiver, Variant key, and Variant result follow the
            // generated keyed contract.
            unsafe {
                function(base.as_const_ptr(), key.as_const_ptr(), output.as_mut_ptr());
            }
        }
        AbiGodotApiKind::BUILTIN_KEYED_SETTER => {
            // SAFETY: Receiver type is authenticated generated metadata.
            let function = unsafe { (interface.variant_get_ptr_keyed_setter)(owner_type()?) }
                .ok_or_else(|| {
                    EngineError::unavailable(
                        "generated Native keyed setter is unavailable in this engine",
                    )
                })?;
            let base = native_base.as_mut().ok_or_else(|| {
                EngineError::invalid_argument("Native keyed setter has no receiver")
            })?;
            let key = native_arguments
                .first()
                .ok_or_else(|| EngineError::invalid_argument("Native keyed setter has no key"))?;
            let value = native_arguments
                .get(1)
                .ok_or_else(|| EngineError::invalid_argument("Native keyed setter has no value"))?;
            // SAFETY: Receiver, Variant key, and Variant value follow the
            // generated keyed contract.
            unsafe {
                function(base.as_mut_ptr(), key.as_const_ptr(), value.as_const_ptr());
            }
        }
        AbiGodotApiKind::BUILTIN_CONSTANT => {
            let member = api_member_name(interface, spec)?;
            let type_ = owner_type()?;
            let mut constant = super::dynamic_value::NativeVariant::uninitialized(interface);
            // SAFETY: Type and constant name are authenticated generated
            // metadata and the result points to uninitialized Variant storage.
            unsafe {
                (interface.variant_get_constant_value)(
                    type_,
                    member.as_ptr(),
                    constant.as_mut_ptr(),
                );
            }
            constant.mark_initialized();
            let rust = constant.to_rust(0)?;
            output = NativeCallValue::from_rust_variant(interface, &spec.return_value, &rust)?;
        }
        AbiGodotApiKind::SINGLETON => {
            let member = api_member_name(interface, spec)?;
            // SAFETY: The singleton name is authenticated generated metadata.
            let object = unsafe { (interface.global_get_singleton)(member.as_ptr()) };
            if object.is_null() {
                return Err(EngineError::unavailable(
                    "generated Native singleton is unavailable in this engine",
                ));
            }
            validate_object_contract(interface, object, &spec.return_value)?;
            output = NativeCallValue::Object(object);
        }
        AbiGodotApiKind::OBJECT_CONSTRUCTOR => {
            let class_name = abi_text(spec.owner_name, "constructor class")?;
            let class_name = GodotStringName::new(&interface, class_name)
                .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
            // SAFETY: The class name is authenticated generated metadata.
            let object = unsafe { (interface.classdb_construct_object2)(class_name.as_ptr()) };
            if object.is_null() {
                return Err(EngineError::unavailable(
                    "generated Native Object construction returned null",
                ));
            }
            interface.postinitialize(object);
            if let Err(error) = validate_object_contract(interface, object, &spec.return_value) {
                // SAFETY: The constructed object has not escaped this call.
                unsafe { (interface.object_destroy)(object) };
                return Err(error);
            }
            output = match spec.return_value.ptrcall_type {
                AbiPtrcallType::OBJECT => NativeCallValue::Object(object),
                AbiPtrcallType::REFCOUNTED_OBJECT => NativeCallValue::RefCounted(Box::new(
                    NativeGodotRefToken::from_object(interface, object),
                )),
                _ => {
                    // SAFETY: The object has not escaped after construction.
                    unsafe { (interface.object_destroy)(object) };
                    return Err(EngineError::invalid_argument(
                        "Native Object constructor has a non-Object return contract",
                    ));
                }
            };
        }
        _ => {
            return Err(EngineError::invalid_argument(
                "Native generated API kind is unknown",
            ));
        }
    }
    output.mark_output_initialized();
    let output = output.into_abi(interface, &spec.return_value)?;
    let updated = if mutates_base {
        Some(
            native_base
                .ok_or_else(|| EngineError::invalid_result("Native mutable API lost its receiver"))?
                .into_abi(interface, &spec.base_value)?,
        )
    } else {
        None
    };
    Ok((output, updated))
}

fn invoke_engine_method_inner(
    interface: Interface,
    receiver_id: u64,
    method: &'static AbiGodotMethodSpecV1,
    arguments: &[AbiValueV1],
) -> EngineResult<NativeEngineValue> {
    let allowed_flags = ABI_GODOT_METHOD_STATIC | ABI_GODOT_METHOD_VARARG;
    if method.struct_size < AbiGodotMethodSpecV1::MINIMUM_SIZE
        || method.reserved_flags & !allowed_flags != 0
    {
        return Err(EngineError::invalid_argument(
            "Native generated MethodBind metadata is incompatible with this SDK",
        ));
    }
    let contracts = value_contracts(method)?;
    let is_vararg = method.reserved_flags & ABI_GODOT_METHOD_VARARG != 0;
    if (!is_vararg && contracts.len() != arguments.len())
        || (is_vararg && arguments.len() < contracts.len())
    {
        return Err(EngineError::invalid_argument(format!(
            "Native engine call expected {}{} argument(s), received {}",
            contracts.len(),
            if is_vararg { " or more" } else { "" },
            arguments.len()
        )));
    }
    let class_name = abi_text(method.class_name, "class name")?;
    let receiver = if method.reserved_flags & ABI_GODOT_METHOD_STATIC != 0 {
        if receiver_id != 0 {
            return Err(EngineError::invalid_argument(
                "Native static engine call unexpectedly received an Object",
            ));
        }
        ptr::null_mut()
    } else {
        if receiver_id == 0 {
            return Err(EngineError::invalid_argument(
                "Native instance engine call has no Object receiver",
            ));
        }
        // SAFETY: Godot owns and synchronizes its instance-ID table.
        let receiver = unsafe { (interface.object_get_instance_from_id)(receiver_id) };
        if receiver.is_null() {
            return Err(EngineError::stale_object(format!(
                "Godot Object {receiver_id} no longer exists"
            )));
        }
        validate_object_class(interface, receiver, class_name, "method receiver")?;
        receiver
    };
    let method_name = abi_text(method.method_name, "method name")?;
    let class_name_value = GodotStringName::new(&interface, class_name)
        .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
    let method_name_value = GodotStringName::new(&interface, method_name)
        .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
    let method_hash = i64::try_from(method.method_hash)
        .map_err(|_| EngineError::invalid_argument("Godot MethodBind hash exceeds i64"))?;
    // SAFETY: Names are live and the hash is authenticated generated metadata.
    let method_bind = unsafe {
        (interface.classdb_get_method_bind)(
            class_name_value.as_ptr(),
            method_name_value.as_ptr(),
            method_hash,
        )
    };
    if method_bind.is_null() {
        return Err(EngineError::unavailable(
            "Godot did not return the generated Native MethodBind",
        ));
    }
    let variant_contract = AbiGodotValueSpecV1 {
        value_type: AbiValueType::VARIANT,
        ptrcall_type: AbiPtrcallType::VARIANT,
        class_name: AbiByteSlice::EMPTY,
        reserved_flags: 0,
        reserved: [0; 2],
    };
    let native_arguments = arguments
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let contract = contracts.get(index).unwrap_or(&variant_contract);
            NativeCallValue::from_argument(interface, contract, *value).map_err(|error| {
                EngineError::invalid_argument(format!(
                    "Native engine call `{class_name}.{method_name}` rejected argument \
                         {index} ({} fixed argument(s)): {error}",
                    contracts.len(),
                ))
            })
        })
        .collect::<EngineResult<Vec<_>>>()?;
    if is_vararg {
        let variants = native_arguments
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let contract = contracts.get(index).unwrap_or(&variant_contract);
                value.to_native_variant(interface, contract.ptrcall_type)
            })
            .collect::<EngineResult<Vec<_>>>()?;
        let pointers = variants
            .iter()
            .map(super::dynamic_value::NativeVariant::as_const_ptr)
            .collect::<Vec<_>>();
        let count = i64::try_from(pointers.len()).map_err(|_| {
            EngineError::invalid_argument("Native variable argument count exceeds i64")
        })?;
        let mut output = super::dynamic_value::NativeVariant::uninitialized(interface);
        let mut call_error = super::sys::GDExtensionCallError {
            error: super::sys::GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
            argument: 0,
            expected: 0,
        };
        // SAFETY: MethodBind, receiver, all Variant arguments, output, and
        // error storage remain live for this synchronous official call.
        unsafe {
            (interface.object_method_bind_call)(
                method_bind,
                receiver,
                if pointers.is_empty() {
                    ptr::null()
                } else {
                    pointers.as_ptr()
                },
                count,
                output.as_mut_ptr(),
                ptr::from_mut(&mut call_error),
            );
        }
        // Godot constructs the Variant result even when dispatch reports an
        // error, so it must always receive its paired destructor.
        output.mark_initialized();
        if call_error.error != super::sys::GDExtensionCallErrorType::GDEXTENSION_CALL_OK {
            return Err(EngineError::invalid_argument(format!(
                "Godot rejected Native variable call `{class_name}.{method_name}` \
                 with error {}, argument {}, expected {}",
                call_error.error.0, call_error.argument, call_error.expected
            )));
        }
        let rust = output.to_rust(0)?;
        return NativeCallValue::from_rust_variant(interface, &method.return_value, &rust)?
            .into_abi(interface, &method.return_value);
    }
    let argument_pointers = native_arguments
        .iter()
        .map(NativeCallValue::as_const_ptr)
        .collect::<Vec<_>>();
    let mut output = NativeCallValue::output(interface, &method.return_value)?;
    // SAFETY: MethodBind, receiver, argument storage, and output storage all
    // match the authenticated generated ptrcall contract.
    unsafe {
        (interface.object_method_bind_ptrcall)(
            method_bind,
            receiver,
            if argument_pointers.is_empty() {
                ptr::null()
            } else {
                argument_pointers.as_ptr()
            },
            output.as_mut_ptr(),
        );
    }
    output.mark_output_initialized();
    output.into_abi(interface, &method.return_value)
}

fn validate_object_contract(
    interface: Interface,
    object: super::sys::GDExtensionObjectPtr,
    contract: &AbiGodotValueSpecV1,
) -> EngineResult<()> {
    if object.is_null() || contract.class_name.len == 0 {
        return Ok(());
    }
    let class_name = abi_text(contract.class_name, "Object class name")?;
    validate_object_class(interface, object, class_name, "method argument")
}

fn validate_object_class(
    interface: Interface,
    object: super::sys::GDExtensionObjectPtr,
    class_name: &str,
    role: &str,
) -> EngineResult<()> {
    let class_name_value = GodotStringName::new(&interface, class_name)
        .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
    // SAFETY: The StringName remains initialized for the lookup and Godot owns
    // the ClassDB tag for the engine lifetime.
    let class_tag = unsafe { (interface.classdb_get_class_tag)(class_name_value.as_ptr()) };
    if class_tag.is_null() {
        return Err(EngineError::invalid_argument(format!(
            "Godot has no ClassDB tag for Native {role} class `{class_name}`"
        )));
    }
    // SAFETY: Both values are engine-owned live pointers and the cast only
    // performs Godot's ClassDB inheritance check.
    if unsafe { (interface.object_cast_to)(object, class_tag) }.is_null() {
        return Err(EngineError::invalid_argument(format!(
            "Native {role} is not a `{class_name}`"
        )));
    }
    Ok(())
}

fn value_contracts(
    method: &'static AbiGodotMethodSpecV1,
) -> EngineResult<&'static [AbiGodotValueSpecV1]> {
    if method.arguments.len == 0 {
        return Ok(&[]);
    }
    if method.arguments.ptr.is_null() || method.arguments.len > 1_024 {
        return Err(EngineError::invalid_argument(
            "Native generated method has an invalid argument contract list",
        ));
    }
    // SAFETY: Generated method specs point to immutable static contract arrays.
    Ok(unsafe { core::slice::from_raw_parts(method.arguments.ptr, method.arguments.len) })
}

fn api_value_contracts(
    spec: &'static AbiGodotApiSpecV1,
) -> EngineResult<&'static [AbiGodotValueSpecV1]> {
    if spec.arguments.len == 0 {
        return Ok(&[]);
    }
    if spec.arguments.ptr.is_null() || spec.arguments.len > 1_024 {
        return Err(EngineError::invalid_argument(
            "Native generated API has an invalid argument contract list",
        ));
    }
    // SAFETY: Generated API specs point to immutable static contract arrays.
    Ok(unsafe { core::slice::from_raw_parts(spec.arguments.ptr, spec.arguments.len) })
}

fn api_member_name(
    interface: Interface,
    spec: &'static AbiGodotApiSpecV1,
) -> EngineResult<GodotStringName> {
    let member = abi_text(spec.member_name, "API member name")?;
    GodotStringName::new(&interface, member)
        .map_err(|error| EngineError::invalid_argument(error.to_string()))
}

fn api_numeric_i64(value: u64, label: &str) -> EngineResult<i64> {
    i64::try_from(value).map_err(|_| {
        EngineError::invalid_argument(format!("Native generated {label} exceeds the engine ABI"))
    })
}

fn ptrcall_variant_type(type_: AbiPtrcallType) -> super::sys::GDExtensionVariantType {
    super::sys::GDExtensionVariantType(match type_ {
        AbiPtrcallType::VOID | AbiPtrcallType::VARIANT => 0,
        AbiPtrcallType::BOOL => 1,
        AbiPtrcallType::I8
        | AbiPtrcallType::I16
        | AbiPtrcallType::I32
        | AbiPtrcallType::I64
        | AbiPtrcallType::U8
        | AbiPtrcallType::U16
        | AbiPtrcallType::U32
        | AbiPtrcallType::U64 => 2,
        AbiPtrcallType::F32 | AbiPtrcallType::F64 => 3,
        AbiPtrcallType::STRING => 4,
        AbiPtrcallType::VECTOR2 => 5,
        AbiPtrcallType::VECTOR2I => 6,
        AbiPtrcallType::RECT2 => 7,
        AbiPtrcallType::RECT2I => 8,
        AbiPtrcallType::VECTOR3 => 9,
        AbiPtrcallType::VECTOR3I => 10,
        AbiPtrcallType::TRANSFORM2D => 11,
        AbiPtrcallType::VECTOR4 => 12,
        AbiPtrcallType::VECTOR4I => 13,
        AbiPtrcallType::PLANE => 14,
        AbiPtrcallType::QUATERNION => 15,
        AbiPtrcallType::AABB => 16,
        AbiPtrcallType::BASIS => 17,
        AbiPtrcallType::TRANSFORM3D => 18,
        AbiPtrcallType::PROJECTION => 19,
        AbiPtrcallType::COLOR => 20,
        AbiPtrcallType::STRING_NAME => 21,
        AbiPtrcallType::NODE_PATH => 22,
        AbiPtrcallType::RID => 23,
        AbiPtrcallType::OBJECT | AbiPtrcallType::REFCOUNTED_OBJECT => 24,
        AbiPtrcallType::CALLABLE => 25,
        AbiPtrcallType::SIGNAL => 26,
        AbiPtrcallType::DICTIONARY => 27,
        AbiPtrcallType::ARRAY => 28,
        AbiPtrcallType::PACKED_BYTE_ARRAY => 29,
        AbiPtrcallType::PACKED_INT32_ARRAY => 30,
        AbiPtrcallType::PACKED_INT64_ARRAY => 31,
        AbiPtrcallType::PACKED_FLOAT32_ARRAY => 32,
        AbiPtrcallType::PACKED_FLOAT64_ARRAY => 33,
        AbiPtrcallType::PACKED_STRING_ARRAY => 34,
        AbiPtrcallType::PACKED_VECTOR2_ARRAY => 35,
        AbiPtrcallType::PACKED_VECTOR3_ARRAY => 36,
        AbiPtrcallType::PACKED_COLOR_ARRAY => 37,
        AbiPtrcallType::PACKED_VECTOR4_ARRAY => 38,
        _ => 0,
    })
}

fn builtin_variant_type(name: &str) -> Option<super::sys::GDExtensionVariantType> {
    let raw = match name {
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
    };
    Some(super::sys::GDExtensionVariantType(raw))
}

fn abi_text(value: AbiByteSlice, label: &str) -> EngineResult<&'static str> {
    if value.ptr.is_null() || value.len == 0 || value.len > 16 * 1024 {
        return Err(EngineError::invalid_argument(format!(
            "Native generated {label} is invalid"
        )));
    }
    // SAFETY: Generated method names are immutable static UTF-8 byte slices.
    let bytes = unsafe { core::slice::from_raw_parts(value.ptr, value.len) };
    core::str::from_utf8(bytes).map_err(|_| {
        EngineError::invalid_argument(format!("Native generated {label} is not UTF-8"))
    })
}

fn narrow_signed<T>(raw: u64) -> EngineResult<T>
where
    T: TryFrom<i64>,
{
    T::try_from(raw as i64).map_err(|_| {
        EngineError::invalid_argument("Native signed integer argument is out of range")
    })
}

fn narrow_unsigned<T>(raw: u64) -> EngineResult<T>
where
    T: TryFrom<u64>,
{
    T::try_from(raw).map_err(|_| {
        EngineError::invalid_argument("Native unsigned integer argument is out of range")
    })
}

fn normalize_signed_output<T>(value: i64) -> EngineResult<i64>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map(|_| value).map_err(|_| {
        EngineError::invalid_result(
            "Native ptrcall returned a signed integer outside its generated range",
        )
    })
}

fn normalize_unsigned_output<T>(value: i64) -> EngineResult<u64>
where
    T: TryFrom<u64>,
{
    let value = u64::try_from(value).map_err(|_| {
        EngineError::invalid_result(
            "Native ptrcall returned a negative value for an unsigned result",
        )
    })?;
    T::try_from(value).map(|_| value).map_err(|_| {
        EngineError::invalid_result(
            "Native ptrcall returned an unsigned integer outside its generated range",
        )
    })
}

fn normalize_f32_output(value: f64) -> EngineResult<f64> {
    let narrowed = value as f32;
    if value.is_finite() && (!narrowed.is_finite() || f64::from(narrowed) != value) {
        return Err(EngineError::invalid_result(
            "Native ptrcall returned a double that is not an encoded f32 result",
        ));
    }
    Ok(f64::from(narrowed))
}

fn fixed_f32_argument<const N: usize, T>(
    value: AbiValueV1,
    expected: AbiValueType,
    construct: impl FnOnce([f32; N]) -> T,
) -> EngineResult<T> {
    let (pointer, length) = value
        .byte_range(expected)
        .ok_or_else(invalid_argument_value)?;
    if length != N * core::mem::size_of::<f32>() {
        return Err(invalid_argument_value());
    }
    // SAFETY: The generated argument wrapper keeps this exact bounded byte
    // range alive for the synchronous Native ptrcall.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    let components = core::array::from_fn(|index| {
        let offset = index * core::mem::size_of::<f32>();
        f32::from_ne_bytes(
            bytes[offset..offset + core::mem::size_of::<f32>()]
                .try_into()
                .expect("one f32 component has an exact byte width"),
        )
    });
    Ok(construct(components))
}

fn invalid_argument_value() -> EngineError {
    EngineError::invalid_argument(
        "Native engine argument violates its generated value representation",
    )
}

const _: () = {
    assert!(
        core::mem::size_of::<super::sys::GDExtensionObjectPtr>() == core::mem::size_of::<usize>()
    );
    assert!(core::mem::size_of::<*mut c_void>() == core::mem::size_of::<usize>());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_ptrcall_storage_matches_godot_encode_types() {
        let mut signed = NativeCallValue::I32(-17);
        // SAFETY: Godot's official PtrToArg<int32_t> EncodeT is int64_t.
        assert_eq!(unsafe { *signed.as_const_ptr().cast::<i64>() }, -17);
        // SAFETY: The selected ptrcall transport payload is exactly one i64.
        unsafe { *signed.as_mut_ptr().cast::<i64>() = 29 };
        assert!(matches!(signed, NativeCallValue::I32(29)));

        let mut unsigned = NativeCallValue::U32(i64::from(u32::MAX));
        // SAFETY: Godot transports uint32_t through a signed int64_t slot.
        let encoded_unsigned = unsafe { *unsigned.as_const_ptr().cast::<i64>() };
        assert_eq!(encoded_unsigned, i64::from(u32::MAX));
        // SAFETY: The selected ptrcall transport payload is exactly one i64.
        unsafe { *unsigned.as_mut_ptr().cast::<i64>() = 42 };
        assert!(matches!(unsigned, NativeCallValue::U32(42)));

        let mut float = NativeCallValue::F32(f64::from(1.25_f32));
        // SAFETY: Godot's official PtrToArg<float> EncodeT is double.
        assert_eq!(unsafe { *float.as_const_ptr().cast::<f64>() }, 1.25);
        // SAFETY: The selected ptrcall transport payload is exactly one f64.
        unsafe { *float.as_mut_ptr().cast::<f64>() = f64::from(-4.5_f32) };
        assert!(matches!(float, NativeCallValue::F32(value) if value == -4.5));
    }

    #[test]
    fn scalar_ptrcall_outputs_are_validated_before_normalization() {
        assert_eq!(
            normalize_signed_output::<i8>(i64::from(i8::MIN)).expect("i8 minimum"),
            i64::from(i8::MIN)
        );
        assert!(normalize_signed_output::<i8>(i64::from(i8::MAX) + 1).is_err());
        assert_eq!(
            normalize_unsigned_output::<u32>(i64::from(u32::MAX)).expect("u32 maximum"),
            u64::from(u32::MAX)
        );
        assert!(normalize_unsigned_output::<u32>(-1).is_err());
        assert_eq!(
            normalize_f32_output(f64::from(0.1_f32)).expect("encoded f32"),
            f64::from(0.1_f32)
        );
        assert!(normalize_f32_output(0.1_f64).is_err());
    }

    #[test]
    fn ptrcall_types_map_to_the_official_variant_ordinals() {
        assert_eq!(ptrcall_variant_type(AbiPtrcallType::BOOL).0, 1);
        assert_eq!(ptrcall_variant_type(AbiPtrcallType::I32).0, 2);
        assert_eq!(ptrcall_variant_type(AbiPtrcallType::F64).0, 3);
        assert_eq!(ptrcall_variant_type(AbiPtrcallType::STRING_NAME).0, 21);
        assert_eq!(ptrcall_variant_type(AbiPtrcallType::OBJECT).0, 24);
        assert_eq!(
            ptrcall_variant_type(AbiPtrcallType::PACKED_VECTOR4_ARRAY).0,
            38
        );
    }

    #[test]
    fn builtin_names_map_to_the_same_official_ordinals() {
        for (name, ptrcall_type) in [
            ("Vector2", AbiPtrcallType::VECTOR2),
            ("Transform3D", AbiPtrcallType::TRANSFORM3D),
            ("StringName", AbiPtrcallType::STRING_NAME),
            ("RID", AbiPtrcallType::RID),
            ("Array", AbiPtrcallType::ARRAY),
            ("PackedColorArray", AbiPtrcallType::PACKED_COLOR_ARRAY),
        ] {
            assert_eq!(
                builtin_variant_type(name),
                Some(ptrcall_variant_type(ptrcall_type)),
                "{name}"
            );
        }
        assert!(builtin_variant_type("UnknownBuiltin").is_none());
    }
}
