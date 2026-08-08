use godot_api::abi::{AbiPtrcallType, AbiStatus, AbiValueType, AbiValueV1};
use godot_api::{
    GDExtensionConstRefPtr, GDExtensionConstTypePtr, GDExtensionObjectPtr, GDExtensionRefPtr,
    GDExtensionTypePtr,
};

use super::contract::ValueContract;
use crate::callable_value::NativeCallable;
use crate::dynamic_value::NativeDynamic;
use crate::interface::EngineInterface;
use crate::module_value;
use crate::node_path::OwnedNodePath;
use crate::packed_array::{OwnedPackedArray, PackedArrayKind};
use crate::signal_value::NativeSignal;
use crate::string_name::OwnedStringName;
use crate::value::LocalGodotString;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ValueError {
    pub(super) status: AbiStatus,
    pub(super) message: &'static str,
}

impl ValueError {
    pub(crate) const fn new(status: AbiStatus, message: &'static str) -> Self {
        Self { status, message }
    }

    pub(crate) const fn invalid(message: &'static str) -> Self {
        Self::new(AbiStatus::InvalidArgument, message)
    }

    pub(crate) const fn status(self) -> AbiStatus {
        self.status
    }

    pub(crate) const fn message(self) -> &'static str {
        self.message
    }
}

/// One initialized native `Ref<T>` output slot.
///
/// Godot's official ptrcall ABI writes RefCounted returns into a one-pointer
/// `Ref<T>` object. `ref_get_object` reads the pointee, while clearing it
/// through `ref_set_object` performs the matching unreference operation.
pub(crate) struct NativeGodotRef {
    get_object: unsafe extern "C" fn(GDExtensionConstRefPtr) -> GDExtensionObjectPtr,
    set_object: unsafe extern "C" fn(GDExtensionRefPtr, GDExtensionObjectPtr),
    storage: usize,
}

impl NativeGodotRef {
    pub(crate) fn empty(interface: EngineInterface) -> Result<Self, ValueError> {
        let get_object = interface.ref_get_object.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot Ref return access is unavailable",
            )
        })?;
        let set_object = interface.ref_set_object.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot Ref return cleanup is unavailable",
            )
        })?;
        Ok(Self::from_functions(get_object, set_object))
    }

    pub(super) fn from_object(
        interface: EngineInterface,
        object: GDExtensionObjectPtr,
    ) -> Result<Self, ValueError> {
        let mut value = Self::empty(interface)?;
        // SAFETY: `value` owns one initialized null Ref storage slot and
        // `object` was resolved through Godot's ObjectDB.
        unsafe { (value.set_object)(value.as_mut_ptr(), object) };
        Ok(value)
    }

    pub(super) const fn from_functions(
        get_object: unsafe extern "C" fn(GDExtensionConstRefPtr) -> GDExtensionObjectPtr,
        set_object: unsafe extern "C" fn(GDExtensionRefPtr, GDExtensionObjectPtr),
    ) -> Self {
        Self {
            get_object,
            set_object,
            storage: 0,
        }
    }

    pub(crate) fn object(&self) -> GDExtensionObjectPtr {
        // SAFETY: `storage` is an initialized null Ref or was populated by
        // Godot's ptrcall into the same one-pointer Ref representation.
        unsafe { (self.get_object)(core::ptr::from_ref(&self.storage).cast_mut().cast()) }
    }

    fn as_const_ptr(&self) -> GDExtensionConstTypePtr {
        core::ptr::from_ref(&self.storage).cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        core::ptr::from_mut(&mut self.storage).cast()
    }
}

impl Drop for NativeGodotRef {
    fn drop(&mut self) {
        // SAFETY: The storage remains a live Ref value until this drop.
        // Assigning null invokes Godot's official Ref cleanup path.
        unsafe {
            (self.set_object)(
                core::ptr::from_mut(&mut self.storage).cast(),
                core::ptr::null_mut(),
            );
        }
    }
}

pub(super) enum NativeValue {
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
    Object(GDExtensionObjectPtr),
    RefCountedObject(NativeGodotRef),
    Vector2([f32; 2]),
    Vector2i([i32; 2]),
    Vector3([f32; 3]),
    Vector3i([i32; 3]),
    Vector4([f32; 4]),
    Vector4i([i32; 4]),
    Rect2([f32; 4]),
    Rect2i([i32; 4]),
    Quaternion([f32; 4]),
    Plane([f32; 4]),
    Transform2D([f32; 6]),
    Aabb([f32; 6]),
    Basis([f32; 9]),
    Transform3D([f32; 12]),
    Projection([f32; 16]),
    Color([f32; 4]),
    Rid(u64),
    String(Box<LocalGodotString>),
    StringName(Box<OwnedStringName>),
    NodePath(Box<OwnedNodePath>),
    Packed(Box<OwnedPackedArray>),
    Dynamic(Box<NativeDynamic>),
    Callable(Box<NativeCallable>),
    Signal(Box<NativeSignal>),
}

pub(super) struct NativeValueInput<
    ResolveObject,
    CreateString,
    CreateStringName,
    CreateNodePath,
    CreatePacked,
    CreateDynamic,
    CreateCallable,
    CreateSignal,
> {
    pub(super) resolve_object: ResolveObject,
    pub(super) create_string: CreateString,
    pub(super) create_string_name: CreateStringName,
    pub(super) create_node_path: CreateNodePath,
    pub(super) create_packed: CreatePacked,
    pub(super) create_dynamic: CreateDynamic,
    pub(super) create_callable: CreateCallable,
    pub(super) create_signal: CreateSignal,
}

pub(super) struct NativeValueOutput<
    ObjectId,
    OwnObjectRef,
    OwnText,
    OwnMath,
    OwnPacked,
    OwnDynamic,
    OwnCallable,
    OwnSignal,
> {
    pub(super) object_id: ObjectId,
    pub(super) own_object_ref: OwnObjectRef,
    pub(super) own_text: OwnText,
    pub(super) own_math: OwnMath,
    pub(super) own_packed: OwnPacked,
    pub(super) own_dynamic: OwnDynamic,
    pub(super) own_callable: OwnCallable,
    pub(super) own_signal: OwnSignal,
}

impl NativeValue {
    pub(super) fn from_abi<
        ResolveObject,
        CreateString,
        CreateStringName,
        CreateNodePath,
        CreatePacked,
        CreateDynamic,
        CreateCallable,
        CreateSignal,
    >(
        contract: &ValueContract,
        value: AbiValueV1,
        input: NativeValueInput<
            ResolveObject,
            CreateString,
            CreateStringName,
            CreateNodePath,
            CreatePacked,
            CreateDynamic,
            CreateCallable,
            CreateSignal,
        >,
    ) -> Result<Self, ValueError>
    where
        ResolveObject: FnOnce(u64) -> Result<GDExtensionObjectPtr, ValueError>,
        CreateString: FnOnce(&str) -> Result<LocalGodotString, ValueError>,
        CreateStringName: FnOnce(&str) -> Result<OwnedStringName, ValueError>,
        CreateNodePath: FnOnce(&str) -> Result<OwnedNodePath, ValueError>,
        CreatePacked: FnOnce(AbiValueV1) -> Result<OwnedPackedArray, ValueError>,
        CreateDynamic: FnOnce(AbiValueV1) -> Result<NativeDynamic, ValueError>,
        CreateCallable: FnOnce(AbiValueV1) -> Result<NativeCallable, ValueError>,
        CreateSignal: FnOnce(AbiValueV1) -> Result<NativeSignal, ValueError>,
    {
        if value.type_ != contract.value_type || value.reserved_flags != 0 {
            return Err(ValueError::invalid(
                "Godot method argument does not match its generated contract",
            ));
        }
        if !matches!(
            contract.ptrcall_type,
            AbiPtrcallType::VECTOR2
                | AbiPtrcallType::VECTOR2I
                | AbiPtrcallType::VECTOR3
                | AbiPtrcallType::VECTOR3I
                | AbiPtrcallType::VECTOR4
                | AbiPtrcallType::VECTOR4I
                | AbiPtrcallType::RECT2
                | AbiPtrcallType::RECT2I
                | AbiPtrcallType::QUATERNION
                | AbiPtrcallType::PLANE
                | AbiPtrcallType::TRANSFORM2D
                | AbiPtrcallType::AABB
                | AbiPtrcallType::BASIS
                | AbiPtrcallType::TRANSFORM3D
                | AbiPtrcallType::PROJECTION
                | AbiPtrcallType::COLOR
                | AbiPtrcallType::STRING
                | AbiPtrcallType::STRING_NAME
                | AbiPtrcallType::NODE_PATH
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
            return Err(ValueError::invalid(
                "Godot method argument has a non-zero reserved payload",
            ));
        }
        match contract.ptrcall_type {
            AbiPtrcallType::BOOL if value.payload[0] <= 1 && value.payload[1] == 0 => {
                Ok(Self::Bool(value.payload[0] as u8))
            }
            AbiPtrcallType::BOOL => Err(ValueError::invalid(
                "Godot bool argument has an invalid payload",
            )),
            AbiPtrcallType::I8 => signed::<i8>(value).map(i64::from).map(Self::I8),
            AbiPtrcallType::I16 => signed::<i16>(value).map(i64::from).map(Self::I16),
            AbiPtrcallType::I32 => signed::<i32>(value).map(i64::from).map(Self::I32),
            AbiPtrcallType::I64 => Ok(Self::I64(value.payload[0] as i64)),
            AbiPtrcallType::U8 => unsigned::<u8>(value).map(i64::from).map(Self::U8),
            AbiPtrcallType::U16 => unsigned::<u16>(value).map(i64::from).map(Self::U16),
            AbiPtrcallType::U32 => unsigned::<u32>(value).map(i64::from).map(Self::U32),
            AbiPtrcallType::U64 => Ok(Self::U64(value.payload[0])),
            AbiPtrcallType::F32 => {
                let value = f64::from_bits(value.payload[0]);
                let narrowed = value as f32;
                if value.is_finite() && !narrowed.is_finite() {
                    Err(ValueError::invalid(
                        "Godot float argument exceeds the f32 range",
                    ))
                } else {
                    Ok(Self::F32(f64::from(narrowed)))
                }
            }
            AbiPtrcallType::F64 => Ok(Self::F64(f64::from_bits(value.payload[0]))),
            AbiPtrcallType::OBJECT => (input.resolve_object)(value.payload[0]).map(Self::Object),
            AbiPtrcallType::VECTOR2 => value.vector2().map(Self::Vector2).ok_or_else(|| {
                ValueError::invalid("Godot Vector2 argument has an invalid payload")
            }),
            AbiPtrcallType::VECTOR2I => value.vector2i().map(Self::Vector2i).ok_or_else(|| {
                ValueError::invalid("Godot Vector2i argument has an invalid payload")
            }),
            AbiPtrcallType::VECTOR3 => value.vector3().map(Self::Vector3).ok_or_else(|| {
                ValueError::invalid("Godot Vector3 argument has an invalid payload")
            }),
            AbiPtrcallType::VECTOR3I => value.vector3i().map(Self::Vector3i).ok_or_else(|| {
                ValueError::invalid("Godot Vector3i argument has an invalid payload")
            }),
            AbiPtrcallType::VECTOR4 => value.vector4().map(Self::Vector4).ok_or_else(|| {
                ValueError::invalid("Godot Vector4 argument has an invalid payload")
            }),
            AbiPtrcallType::VECTOR4I => value.vector4i().map(Self::Vector4i).ok_or_else(|| {
                ValueError::invalid("Godot Vector4i argument has an invalid payload")
            }),
            AbiPtrcallType::RECT2 => value
                .rect2()
                .map(Self::Rect2)
                .ok_or_else(|| ValueError::invalid("Godot Rect2 argument has an invalid payload")),
            AbiPtrcallType::RECT2I => value
                .rect2i()
                .map(Self::Rect2i)
                .ok_or_else(|| ValueError::invalid("Godot Rect2i argument has an invalid payload")),
            AbiPtrcallType::QUATERNION => {
                value.quaternion().map(Self::Quaternion).ok_or_else(|| {
                    ValueError::invalid("Godot Quaternion argument has an invalid payload")
                })
            }
            AbiPtrcallType::PLANE => value
                .plane()
                .map(Self::Plane)
                .ok_or_else(|| ValueError::invalid("Godot Plane argument has an invalid payload")),
            AbiPtrcallType::TRANSFORM2D => fixed_f32::<6>(value, AbiValueType::TRANSFORM2D)
                .map(Self::Transform2D)
                .ok_or_else(|| {
                    ValueError::invalid("Godot Transform2D argument has an invalid payload")
                }),
            AbiPtrcallType::AABB => fixed_f32::<6>(value, AbiValueType::AABB)
                .map(Self::Aabb)
                .ok_or_else(|| ValueError::invalid("Godot AABB argument has an invalid payload")),
            AbiPtrcallType::BASIS => fixed_f32::<9>(value, AbiValueType::BASIS)
                .map(Self::Basis)
                .ok_or_else(|| ValueError::invalid("Godot Basis argument has an invalid payload")),
            AbiPtrcallType::TRANSFORM3D => fixed_f32::<12>(value, AbiValueType::TRANSFORM3D)
                .map(Self::Transform3D)
                .ok_or_else(|| {
                    ValueError::invalid("Godot Transform3D argument has an invalid payload")
                }),
            AbiPtrcallType::PROJECTION => fixed_f32::<16>(value, AbiValueType::PROJECTION)
                .map(Self::Projection)
                .ok_or_else(|| {
                    ValueError::invalid("Godot Projection argument has an invalid payload")
                }),
            AbiPtrcallType::COLOR => value
                .color()
                .map(Self::Color)
                .ok_or_else(|| ValueError::invalid("Godot Color argument has an invalid payload")),
            AbiPtrcallType::RID => value
                .rid()
                .map(Self::Rid)
                .ok_or_else(|| ValueError::invalid("Godot RID argument has an invalid payload")),
            AbiPtrcallType::STRING => {
                let text = module_value::utf8(&value)
                    .map_err(|_| ValueError::invalid("Godot String argument is not valid UTF-8"))?;
                (input.create_string)(text).map(Box::new).map(Self::String)
            }
            AbiPtrcallType::STRING_NAME => {
                let text = module_value::utf8(&value).map_err(|_| {
                    ValueError::invalid("Godot StringName argument is not valid UTF-8")
                })?;
                (input.create_string_name)(text)
                    .map(Box::new)
                    .map(Self::StringName)
            }
            AbiPtrcallType::NODE_PATH => {
                let text = module_value::utf8(&value).map_err(|_| {
                    ValueError::invalid("Godot NodePath argument is not valid UTF-8")
                })?;
                (input.create_node_path)(text)
                    .map(Box::new)
                    .map(Self::NodePath)
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
                (input.create_packed)(value).map(Box::new).map(Self::Packed)
            }
            AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY => {
                (input.create_dynamic)(value)
                    .map(Box::new)
                    .map(Self::Dynamic)
            }
            AbiPtrcallType::CALLABLE => (input.create_callable)(value)
                .map(Box::new)
                .map(Self::Callable),
            AbiPtrcallType::SIGNAL => (input.create_signal)(value).map(Box::new).map(Self::Signal),
            _ => Err(ValueError::invalid(
                "Godot method argument uses a return-only ptrcall type",
            )),
        }
    }

    pub(super) fn empty_output(
        interface: EngineInterface,
        contract: &ValueContract,
    ) -> Result<Self, ValueError> {
        let value =
            match contract.ptrcall_type {
                AbiPtrcallType::VOID => Self::Void,
                AbiPtrcallType::BOOL => Self::Bool(0),
                AbiPtrcallType::I8 => Self::I8(0),
                AbiPtrcallType::I16 => Self::I16(0),
                AbiPtrcallType::I32 => Self::I32(0),
                AbiPtrcallType::I64 => Self::I64(0),
                AbiPtrcallType::U8 => Self::U8(0),
                AbiPtrcallType::U16 => Self::U16(0),
                AbiPtrcallType::U32 => Self::U32(0),
                AbiPtrcallType::U64 => Self::U64(0),
                AbiPtrcallType::F32 => Self::F32(0.0),
                AbiPtrcallType::F64 => Self::F64(0.0),
                AbiPtrcallType::OBJECT => Self::Object(core::ptr::null_mut()),
                AbiPtrcallType::REFCOUNTED_OBJECT => {
                    Self::RefCountedObject(NativeGodotRef::empty(interface)?)
                }
                AbiPtrcallType::VECTOR2 => Self::Vector2([0.0; 2]),
                AbiPtrcallType::VECTOR2I => Self::Vector2i([0; 2]),
                AbiPtrcallType::VECTOR3 => Self::Vector3([0.0; 3]),
                AbiPtrcallType::VECTOR3I => Self::Vector3i([0; 3]),
                AbiPtrcallType::VECTOR4 => Self::Vector4([0.0; 4]),
                AbiPtrcallType::VECTOR4I => Self::Vector4i([0; 4]),
                AbiPtrcallType::RECT2 => Self::Rect2([0.0; 4]),
                AbiPtrcallType::RECT2I => Self::Rect2i([0; 4]),
                AbiPtrcallType::QUATERNION => Self::Quaternion([0.0; 4]),
                AbiPtrcallType::PLANE => Self::Plane([0.0; 4]),
                AbiPtrcallType::TRANSFORM2D => Self::Transform2D([0.0; 6]),
                AbiPtrcallType::AABB => Self::Aabb([0.0; 6]),
                AbiPtrcallType::BASIS => Self::Basis([0.0; 9]),
                AbiPtrcallType::TRANSFORM3D => Self::Transform3D([0.0; 12]),
                AbiPtrcallType::PROJECTION => Self::Projection([0.0; 16]),
                AbiPtrcallType::COLOR => Self::Color([0.0; 4]),
                AbiPtrcallType::RID => Self::Rid(0),
                AbiPtrcallType::STRING => Self::String(Box::new(
                    LocalGodotString::empty(interface).ok_or_else(|| {
                        ValueError::new(
                            AbiStatus::Internal,
                            "Godot String return storage could not be initialized",
                        )
                    })?,
                )),
                AbiPtrcallType::STRING_NAME => Self::StringName(Box::new(
                    OwnedStringName::empty(interface).ok_or_else(|| {
                        ValueError::new(
                            AbiStatus::Internal,
                            "Godot StringName return storage could not be initialized",
                        )
                    })?,
                )),
                AbiPtrcallType::NODE_PATH => Self::NodePath(Box::new(
                    OwnedNodePath::empty(interface).ok_or_else(|| {
                        ValueError::new(
                            AbiStatus::Internal,
                            "Godot NodePath return storage could not be initialized",
                        )
                    })?,
                )),
                AbiPtrcallType::PACKED_BYTE_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Byte)?,
                )),
                AbiPtrcallType::PACKED_INT32_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Int32)?,
                )),
                AbiPtrcallType::PACKED_INT64_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Int64)?,
                )),
                AbiPtrcallType::PACKED_FLOAT32_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Float32)?,
                )),
                AbiPtrcallType::PACKED_FLOAT64_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Float64)?,
                )),
                AbiPtrcallType::PACKED_STRING_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::String)?,
                )),
                AbiPtrcallType::PACKED_VECTOR2_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Vector2)?,
                )),
                AbiPtrcallType::PACKED_VECTOR3_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Vector3)?,
                )),
                AbiPtrcallType::PACKED_COLOR_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Color)?,
                )),
                AbiPtrcallType::PACKED_VECTOR4_ARRAY => Self::Packed(Box::new(
                    OwnedPackedArray::empty(interface, PackedArrayKind::Vector4)?,
                )),
                AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY => {
                    Self::Dynamic(Box::new(NativeDynamic::empty(
                        interface,
                        contract.value_type,
                    )?))
                }
                AbiPtrcallType::CALLABLE => {
                    Self::Callable(Box::new(NativeCallable::empty(interface)?))
                }
                AbiPtrcallType::SIGNAL => Self::Signal(Box::new(NativeSignal::empty(interface)?)),
                _ => unreachable!("validated contract contains a supported ptrcall type"),
            };
        Ok(value)
    }

    pub(super) fn as_const_ptr(&self) -> GDExtensionConstTypePtr {
        match self {
            Self::Void => core::ptr::null(),
            Self::Bool(value) => core::ptr::from_ref(value).cast(),
            Self::I8(value) => core::ptr::from_ref(value).cast(),
            Self::I16(value) => core::ptr::from_ref(value).cast(),
            Self::I32(value) => core::ptr::from_ref(value).cast(),
            Self::I64(value) => core::ptr::from_ref(value).cast(),
            Self::U8(value) => core::ptr::from_ref(value).cast(),
            Self::U16(value) => core::ptr::from_ref(value).cast(),
            Self::U32(value) => core::ptr::from_ref(value).cast(),
            Self::U64(value) => core::ptr::from_ref(value).cast(),
            Self::F32(value) => core::ptr::from_ref(value).cast(),
            Self::F64(value) => core::ptr::from_ref(value).cast(),
            Self::Object(value) => core::ptr::from_ref(value).cast(),
            Self::RefCountedObject(value) => value.as_const_ptr(),
            Self::Vector2(value) => value.as_ptr().cast(),
            Self::Vector2i(value) => value.as_ptr().cast(),
            Self::Vector3(value) => value.as_ptr().cast(),
            Self::Vector3i(value) => value.as_ptr().cast(),
            Self::Vector4(value) => value.as_ptr().cast(),
            Self::Vector4i(value) => value.as_ptr().cast(),
            Self::Rect2(value) => value.as_ptr().cast(),
            Self::Rect2i(value) => value.as_ptr().cast(),
            Self::Quaternion(value) => value.as_ptr().cast(),
            Self::Plane(value) => value.as_ptr().cast(),
            Self::Transform2D(value) => value.as_ptr().cast(),
            Self::Aabb(value) => value.as_ptr().cast(),
            Self::Basis(value) => value.as_ptr().cast(),
            Self::Transform3D(value) => value.as_ptr().cast(),
            Self::Projection(value) => value.as_ptr().cast(),
            Self::Color(value) => value.as_ptr().cast(),
            Self::Rid(value) => core::ptr::from_ref(value).cast(),
            Self::String(value) => value.as_ptr(),
            Self::StringName(value) => value.as_ptr(),
            Self::NodePath(value) => value.as_ptr(),
            Self::Packed(value) => value.as_const_ptr(),
            Self::Dynamic(value) => value.as_const_ptr(),
            Self::Callable(value) => value.as_ptr(),
            Self::Signal(value) => value.as_ptr(),
        }
    }

    pub(super) fn as_i64(&self) -> Option<i64> {
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

    pub(super) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        match self {
            Self::Void => core::ptr::null_mut(),
            Self::Bool(value) => core::ptr::from_mut(value).cast(),
            Self::I8(value) => core::ptr::from_mut(value).cast(),
            Self::I16(value) => core::ptr::from_mut(value).cast(),
            Self::I32(value) => core::ptr::from_mut(value).cast(),
            Self::I64(value) => core::ptr::from_mut(value).cast(),
            Self::U8(value) => core::ptr::from_mut(value).cast(),
            Self::U16(value) => core::ptr::from_mut(value).cast(),
            Self::U32(value) => core::ptr::from_mut(value).cast(),
            Self::U64(value) => core::ptr::from_mut(value).cast(),
            Self::F32(value) => core::ptr::from_mut(value).cast(),
            Self::F64(value) => core::ptr::from_mut(value).cast(),
            Self::Object(value) => core::ptr::from_mut(value).cast(),
            Self::RefCountedObject(value) => value.as_mut_ptr(),
            Self::Vector2(value) => value.as_mut_ptr().cast(),
            Self::Vector2i(value) => value.as_mut_ptr().cast(),
            Self::Vector3(value) => value.as_mut_ptr().cast(),
            Self::Vector3i(value) => value.as_mut_ptr().cast(),
            Self::Vector4(value) => value.as_mut_ptr().cast(),
            Self::Vector4i(value) => value.as_mut_ptr().cast(),
            Self::Rect2(value) => value.as_mut_ptr().cast(),
            Self::Rect2i(value) => value.as_mut_ptr().cast(),
            Self::Quaternion(value) => value.as_mut_ptr().cast(),
            Self::Plane(value) => value.as_mut_ptr().cast(),
            Self::Transform2D(value) => value.as_mut_ptr().cast(),
            Self::Aabb(value) => value.as_mut_ptr().cast(),
            Self::Basis(value) => value.as_mut_ptr().cast(),
            Self::Transform3D(value) => value.as_mut_ptr().cast(),
            Self::Projection(value) => value.as_mut_ptr().cast(),
            Self::Color(value) => value.as_mut_ptr().cast(),
            Self::Rid(value) => core::ptr::from_mut(value).cast(),
            Self::String(value) => value.as_mut_ptr(),
            Self::StringName(value) => value.as_mut_ptr(),
            Self::NodePath(value) => value.as_mut_ptr(),
            Self::Packed(value) => value.as_mut_ptr(),
            Self::Dynamic(value) => value.as_mut_ptr(),
            Self::Callable(value) => value.as_mut_ptr(),
            Self::Signal(value) => value.as_mut_ptr(),
        }
    }

    /// Destroys initialized return storage immediately before a builtin
    /// placement constructor overwrites it.
    ///
    /// The caller must invoke the matching constructor without returning to
    /// Rust. Once constructed, the existing wrapper again owns the value and
    /// performs its normal destructor on drop.
    pub(super) unsafe fn prepare_builtin_constructor(
        &mut self,
        interface: EngineInterface,
        variant_type: godot_api::GDExtensionVariantType,
    ) -> Result<(), ValueError> {
        let output = self.as_mut_ptr();
        if output.is_null() {
            return Err(ValueError::invalid(
                "Godot builtin constructor received void storage",
            ));
        }
        if variant_type == godot_api::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL {
            let destroy = interface.variant_destroy.ok_or_else(|| {
                ValueError::new(
                    AbiStatus::Internal,
                    "Godot Variant constructor cleanup is unavailable",
                )
            })?;
            // SAFETY: A `NativeDynamic::Variant` output contains one live
            // official Variant immediately before placement construction.
            unsafe { destroy(output.cast()) };
            return Ok(());
        }
        let get_destructor = interface.variant_get_ptr_destructor.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot builtin destructor lookup is unavailable",
            )
        })?;
        // SAFETY: `variant_type` came from the generated builtin name.
        if let Some(destroy) = unsafe { get_destructor(variant_type) } {
            // SAFETY: `output` points to an initialized value of this exact
            // builtin type and is replaced immediately by its constructor.
            unsafe { destroy(output) };
        }
        Ok(())
    }

    pub(super) fn into_abi<
        ObjectId,
        OwnObjectRef,
        OwnText,
        OwnMath,
        OwnPacked,
        OwnDynamic,
        OwnCallable,
        OwnSignal,
    >(
        self,
        contract: &ValueContract,
        output: NativeValueOutput<
            ObjectId,
            OwnObjectRef,
            OwnText,
            OwnMath,
            OwnPacked,
            OwnDynamic,
            OwnCallable,
            OwnSignal,
        >,
    ) -> Result<AbiValueV1, ValueError>
    where
        ObjectId: FnOnce(GDExtensionObjectPtr) -> Result<u64, ValueError>,
        OwnObjectRef:
            FnOnce(GDExtensionObjectPtr, NativeGodotRef) -> Result<AbiValueV1, ValueError>,
        OwnText: FnOnce(AbiValueType, String) -> Result<AbiValueV1, ValueError>,
        OwnMath: FnOnce(AbiValueType, &[f32]) -> Result<AbiValueV1, ValueError>,
        OwnPacked: FnOnce(AbiValueType, Vec<u8>) -> Result<AbiValueV1, ValueError>,
        OwnDynamic: FnOnce(AbiValueType, NativeDynamic) -> Result<AbiValueV1, ValueError>,
        OwnCallable: FnOnce(NativeCallable) -> Result<AbiValueV1, ValueError>,
        OwnSignal: FnOnce(NativeSignal) -> Result<AbiValueV1, ValueError>,
    {
        match (self, contract.ptrcall_type) {
            (Self::Void, AbiPtrcallType::VOID) => Ok(AbiValueV1::NIL),
            (Self::Bool(value), AbiPtrcallType::BOOL) if value <= 1 => {
                Ok(AbiValueV1::from_bool(value != 0))
            }
            (Self::I8(value), AbiPtrcallType::I8) => {
                normalize_signed_output::<i8>(value).map(AbiValueV1::from_i64)
            }
            (Self::I16(value), AbiPtrcallType::I16) => {
                normalize_signed_output::<i16>(value).map(AbiValueV1::from_i64)
            }
            (Self::I32(value), AbiPtrcallType::I32) => {
                normalize_signed_output::<i32>(value).map(AbiValueV1::from_i64)
            }
            (Self::I64(value), AbiPtrcallType::I64) => Ok(AbiValueV1::from_i64(value)),
            (Self::U8(value), AbiPtrcallType::U8) => {
                normalize_unsigned_output::<u8>(value).map(AbiValueV1::from_u64)
            }
            (Self::U16(value), AbiPtrcallType::U16) => {
                normalize_unsigned_output::<u16>(value).map(AbiValueV1::from_u64)
            }
            (Self::U32(value), AbiPtrcallType::U32) => {
                normalize_unsigned_output::<u32>(value).map(AbiValueV1::from_u64)
            }
            (Self::U64(value), AbiPtrcallType::U64) => Ok(AbiValueV1::from_u64(value)),
            (Self::F32(value), AbiPtrcallType::F32) => {
                normalize_f32_output(value).map(AbiValueV1::from_f64)
            }
            (Self::F64(value), AbiPtrcallType::F64) => Ok(AbiValueV1::from_f64(value)),
            (Self::Object(value), AbiPtrcallType::OBJECT) => {
                (output.object_id)(value).map(AbiValueV1::from_object_id)
            }
            (Self::RefCountedObject(value), AbiPtrcallType::REFCOUNTED_OBJECT) => {
                (output.own_object_ref)(value.object(), value)
            }
            (Self::Vector2([x, y]), AbiPtrcallType::VECTOR2) => Ok(AbiValueV1::from_vector2(x, y)),
            (Self::Vector2i([x, y]), AbiPtrcallType::VECTOR2I) => {
                Ok(AbiValueV1::from_vector2i(x, y))
            }
            (Self::Vector3([x, y, z]), AbiPtrcallType::VECTOR3) => {
                Ok(AbiValueV1::from_vector3(x, y, z))
            }
            (Self::Vector3i([x, y, z]), AbiPtrcallType::VECTOR3I) => {
                Ok(AbiValueV1::from_vector3i(x, y, z))
            }
            (Self::Vector4([x, y, z, w]), AbiPtrcallType::VECTOR4) => {
                Ok(AbiValueV1::from_vector4(x, y, z, w))
            }
            (Self::Vector4i([x, y, z, w]), AbiPtrcallType::VECTOR4I) => {
                Ok(AbiValueV1::from_vector4i(x, y, z, w))
            }
            (Self::Rect2([x, y, width, height]), AbiPtrcallType::RECT2) => {
                Ok(AbiValueV1::from_rect2(x, y, width, height))
            }
            (Self::Rect2i([x, y, width, height]), AbiPtrcallType::RECT2I) => {
                Ok(AbiValueV1::from_rect2i(x, y, width, height))
            }
            (Self::Quaternion([x, y, z, w]), AbiPtrcallType::QUATERNION) => {
                Ok(AbiValueV1::from_quaternion(x, y, z, w))
            }
            (Self::Plane([x, y, z, d]), AbiPtrcallType::PLANE) => {
                Ok(AbiValueV1::from_plane(x, y, z, d))
            }
            (Self::Transform2D(value), AbiPtrcallType::TRANSFORM2D) => {
                (output.own_math)(AbiValueType::TRANSFORM2D, &value)
            }
            (Self::Aabb(value), AbiPtrcallType::AABB) => {
                (output.own_math)(AbiValueType::AABB, &value)
            }
            (Self::Basis(value), AbiPtrcallType::BASIS) => {
                (output.own_math)(AbiValueType::BASIS, &value)
            }
            (Self::Transform3D(value), AbiPtrcallType::TRANSFORM3D) => {
                (output.own_math)(AbiValueType::TRANSFORM3D, &value)
            }
            (Self::Projection(value), AbiPtrcallType::PROJECTION) => {
                (output.own_math)(AbiValueType::PROJECTION, &value)
            }
            (Self::Color([r, g, b, a]), AbiPtrcallType::COLOR) => {
                Ok(AbiValueV1::from_color(r, g, b, a))
            }
            (Self::Rid(value), AbiPtrcallType::RID) => Ok(AbiValueV1::from_rid(value)),
            (Self::String(value), AbiPtrcallType::STRING) => {
                let text = value.to_utf8().map_err(|_| {
                    ValueError::new(
                        AbiStatus::Internal,
                        "Godot method returned an invalid String",
                    )
                })?;
                (output.own_text)(AbiValueType::STRING, text)
            }
            (Self::StringName(value), AbiPtrcallType::STRING_NAME) => {
                let text = value.to_utf8().map_err(|_| {
                    ValueError::new(
                        AbiStatus::Internal,
                        "Godot method returned an invalid StringName",
                    )
                })?;
                (output.own_text)(AbiValueType::STRING_NAME, text)
            }
            (Self::NodePath(value), AbiPtrcallType::NODE_PATH) => {
                let text = value.to_utf8().map_err(|_| {
                    ValueError::new(
                        AbiStatus::Internal,
                        "Godot method returned an invalid NodePath",
                    )
                })?;
                (output.own_text)(AbiValueType::NODE_PATH, text)
            }
            (Self::Packed(value), ptrcall_type)
                if packed_ptrcall_matches(value.kind(), ptrcall_type) =>
            {
                let value_type = value.kind().value_type();
                (output.own_packed)(value_type, value.to_bytes()?)
            }
            (
                Self::Dynamic(value),
                AbiPtrcallType::VARIANT | AbiPtrcallType::ARRAY | AbiPtrcallType::DICTIONARY,
            ) => (output.own_dynamic)(contract.value_type, *value),
            (Self::Callable(value), AbiPtrcallType::CALLABLE) => (output.own_callable)(*value),
            (Self::Signal(value), AbiPtrcallType::SIGNAL) => (output.own_signal)(*value),
            _ => Err(ValueError::new(
                AbiStatus::Internal,
                "Godot ptrcall returned an invalid native value",
            )),
        }
    }
}

fn signed<T>(value: AbiValueV1) -> Result<T, ValueError>
where
    T: TryFrom<i64>,
{
    T::try_from(value.payload[0] as i64)
        .map_err(|_| ValueError::invalid("Godot signed integer argument is out of range"))
}

fn unsigned<T>(value: AbiValueV1) -> Result<T, ValueError>
where
    T: TryFrom<u64>,
{
    T::try_from(value.payload[0])
        .map_err(|_| ValueError::invalid("Godot unsigned integer argument is out of range"))
}

fn normalize_signed_output<T>(value: i64) -> Result<i64, ValueError>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map(|_| value).map_err(|_| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot returned a signed integer outside its generated ptrcall range",
        )
    })
}

fn normalize_unsigned_output<T>(value: i64) -> Result<u64, ValueError>
where
    T: TryFrom<u64>,
{
    let value = u64::try_from(value).map_err(|_| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot returned a negative value for an unsigned ptrcall result",
        )
    })?;
    T::try_from(value).map(|_| value).map_err(|_| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot returned an unsigned integer outside its generated ptrcall range",
        )
    })
}

fn normalize_f32_output(value: f64) -> Result<f64, ValueError> {
    let narrowed = value as f32;
    if value.is_finite() && (!narrowed.is_finite() || f64::from(narrowed) != value) {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot returned a double that is not an encoded f32 ptrcall result",
        ));
    }
    Ok(f64::from(narrowed))
}

fn fixed_f32<const N: usize>(value: AbiValueV1, expected: AbiValueType) -> Option<[f32; N]> {
    let (pointer, length) = value.byte_range(expected)?;
    if value.reserved_flags != 0 || length != N * core::mem::size_of::<f32>() {
        return None;
    }
    // SAFETY: The module retains the borrowed buffer through this synchronous
    // call and the exact bounded byte length was validated above.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    Some(core::array::from_fn(|index| {
        let offset = index * 4;
        f32::from_ne_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("f32 byte width"),
        )
    }))
}

fn packed_ptrcall_matches(kind: PackedArrayKind, ptrcall_type: AbiPtrcallType) -> bool {
    matches!(
        (kind, ptrcall_type),
        (PackedArrayKind::Byte, AbiPtrcallType::PACKED_BYTE_ARRAY)
            | (PackedArrayKind::Int32, AbiPtrcallType::PACKED_INT32_ARRAY)
            | (PackedArrayKind::Int64, AbiPtrcallType::PACKED_INT64_ARRAY)
            | (
                PackedArrayKind::Float32,
                AbiPtrcallType::PACKED_FLOAT32_ARRAY
            )
            | (
                PackedArrayKind::Float64,
                AbiPtrcallType::PACKED_FLOAT64_ARRAY
            )
            | (PackedArrayKind::String, AbiPtrcallType::PACKED_STRING_ARRAY)
            | (
                PackedArrayKind::Vector2,
                AbiPtrcallType::PACKED_VECTOR2_ARRAY
            )
            | (
                PackedArrayKind::Vector3,
                AbiPtrcallType::PACKED_VECTOR3_ARRAY
            )
            | (PackedArrayKind::Color, AbiPtrcallType::PACKED_COLOR_ARRAY)
            | (
                PackedArrayKind::Vector4,
                AbiPtrcallType::PACKED_VECTOR4_ARRAY
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    static REF_CLEAR_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn mock_ref_get_object(
        value: GDExtensionConstRefPtr,
    ) -> GDExtensionObjectPtr {
        // SAFETY: The test passes one initialized pointer-width storage slot.
        unsafe { *value.cast::<usize>() as GDExtensionObjectPtr }
    }

    unsafe extern "C" fn mock_ref_set_object(
        value: GDExtensionRefPtr,
        object: GDExtensionObjectPtr,
    ) {
        // SAFETY: The test passes one writable pointer-width storage slot.
        unsafe { *value.cast::<usize>() = object as usize };
        if object.is_null() {
            REF_CLEAR_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn contract(value_type: AbiValueType, ptrcall_type: AbiPtrcallType) -> ValueContract {
        ValueContract {
            value_type,
            ptrcall_type,
            class_name: None,
            typed_array_element: None,
        }
    }

    macro_rules! unused_input {
        () => {
            NativeValueInput {
                resolve_object: |_| unreachable!(),
                create_string: |_: &str| unreachable!(),
                create_string_name: |_: &str| unreachable!(),
                create_node_path: |_: &str| unreachable!(),
                create_packed: |_| unreachable!(),
                create_dynamic: |_| unreachable!(),
                create_callable: |_| unreachable!(),
                create_signal: |_| unreachable!(),
            }
        };
    }

    macro_rules! unused_output {
        () => {
            NativeValueOutput {
                object_id: |_| unreachable!(),
                own_object_ref: |_, _| unreachable!(),
                own_text: |_, _| unreachable!(),
                own_math: |_: AbiValueType, _: &[f32]| unreachable!(),
                own_packed: |_, _| unreachable!(),
                own_dynamic: |_, _| unreachable!(),
                own_callable: |_| unreachable!(),
                own_signal: |_| unreachable!(),
            }
        };
    }

    #[test]
    fn exact_integer_ranges_are_enforced_before_ptrcall() {
        let i8_contract = contract(AbiValueType::I64, AbiPtrcallType::I8);
        let value = NativeValue::from_abi(
            &i8_contract,
            AbiValueV1::from_i64(i8::MAX.into()),
            unused_input!(),
        )
        .expect("i8 max");
        assert!(matches!(value, NativeValue::I8(value) if value == i64::from(i8::MAX)));
        let overflow = NativeValue::from_abi(
            &i8_contract,
            AbiValueV1::from_i64(i16::from(i8::MAX) as i64 + 1),
            unused_input!(),
        )
        .err()
        .expect("i8 overflow");
        assert_eq!(overflow.status, AbiStatus::InvalidArgument);

        let u32_contract = contract(AbiValueType::U64, AbiPtrcallType::U32);
        let encoded = NativeValue::U32(u32::MAX.into())
            .into_abi(&u32_contract, unused_output!())
            .expect("u32 max output");
        assert_eq!(encoded.payload[0], u64::from(u32::MAX));

        let invalid_output = NativeValue::I8(i64::from(i8::MAX) + 1)
            .into_abi(&i8_contract, unused_output!())
            .expect_err("out-of-range i8 output");
        assert_eq!(invalid_output.status, AbiStatus::Internal);

        let negative_unsigned = NativeValue::U32(-1)
            .into_abi(&u32_contract, unused_output!())
            .expect_err("negative u32 output");
        assert_eq!(negative_unsigned.status, AbiStatus::Internal);

        let overflow = NativeValue::from_abi(
            &u32_contract,
            AbiValueV1::from_u64(u64::from(u32::MAX) + 1),
            unused_input!(),
        )
        .err()
        .expect("u32 overflow");
        assert_eq!(overflow.status, AbiStatus::InvalidArgument);
    }

    #[test]
    fn native_outputs_round_trip_to_normalized_values() {
        let float_contract = contract(AbiValueType::F64, AbiPtrcallType::F32);
        let value = NativeValue::F32(1.25)
            .into_abi(&float_contract, unused_output!())
            .expect("f32 output");
        assert_eq!(value.type_, AbiValueType::F64);
        assert_eq!(f64::from_bits(value.payload[0]), 1.25);

        let bool_contract = contract(AbiValueType::BOOL, AbiPtrcallType::BOOL);
        let invalid = NativeValue::Bool(2)
            .into_abi(&bool_contract, unused_output!())
            .expect_err("invalid Godot bool");
        assert_eq!(invalid.status, AbiStatus::Internal);

        let vector = NativeValue::Vector3i([-1, 2, i32::MIN])
            .into_abi(
                &contract(AbiValueType::VECTOR3I, AbiPtrcallType::VECTOR3I),
                unused_output!(),
            )
            .expect("Vector3i output");
        assert_eq!(vector.vector3i(), Some([-1, 2, i32::MIN]));

        let rid = NativeValue::Rid(u64::MAX)
            .into_abi(
                &contract(AbiValueType::RID, AbiPtrcallType::RID),
                unused_output!(),
            )
            .expect("RID output");
        assert_eq!(rid.rid(), Some(u64::MAX));
    }

    #[test]
    fn ptrcall_pointers_reference_exact_native_storage() {
        let mut value = NativeValue::I32(-17);
        // SAFETY: Godot's official PtrToArg<int32_t> EncodeT is int64_t.
        assert_eq!(unsafe { *value.as_const_ptr().cast::<i64>() }, -17);
        // SAFETY: The selected ptrcall transport payload is exactly one i64.
        unsafe { *value.as_mut_ptr().cast::<i64>() = 29 };
        assert!(matches!(value, NativeValue::I32(29)));

        let mut float = NativeValue::F32(f64::from(1.25_f32));
        // SAFETY: Godot's official PtrToArg<float> EncodeT is double.
        assert_eq!(unsafe { *float.as_const_ptr().cast::<f64>() }, 1.25);
        // SAFETY: The selected ptrcall transport payload is exactly one f64.
        unsafe { *float.as_mut_ptr().cast::<f64>() = f64::from(-4.5_f32) };
        assert!(matches!(float, NativeValue::F32(value) if value == -4.5));

        let mut vector = NativeValue::from_abi(
            &contract(AbiValueType::VECTOR3, AbiPtrcallType::VECTOR3),
            AbiValueV1::from_vector3(1.0, 2.0, 3.0),
            unused_input!(),
        )
        .expect("Vector3 input");
        // SAFETY: The selected native payload is exactly three contiguous f32s.
        unsafe { *vector.as_mut_ptr().cast::<f32>().add(1) = -4.0 };
        let encoded = vector
            .into_abi(
                &contract(AbiValueType::VECTOR3, AbiPtrcallType::VECTOR3),
                unused_output!(),
            )
            .expect("Vector3 output");
        assert_eq!(encoded.vector3(), Some([1.0, -4.0, 3.0]));

        let mut rid = NativeValue::from_abi(
            &contract(AbiValueType::RID, AbiPtrcallType::RID),
            AbiValueV1::from_rid(u64::MAX),
            unused_input!(),
        )
        .expect("RID input");
        // SAFETY: RID is exactly one opaque u64 in every authenticated build.
        unsafe { *rid.as_mut_ptr().cast::<u64>() = 42 };
        let encoded = rid
            .into_abi(
                &contract(AbiValueType::RID, AbiPtrcallType::RID),
                unused_output!(),
            )
            .expect("RID output");
        assert_eq!(encoded.rid(), Some(42));
    }

    #[test]
    fn native_ref_storage_is_read_and_cleared_through_official_callbacks() {
        REF_CLEAR_COUNT.store(0, Ordering::SeqCst);
        let object = core::ptr::dangling_mut::<u8>().cast();
        let mut value = NativeGodotRef::from_functions(mock_ref_get_object, mock_ref_set_object);
        // SAFETY: A Godot Ref<T> is one pointer wide and this test models the
        // official ptrcall encoder writing that pointer into the output slot.
        unsafe { *value.as_mut_ptr().cast::<usize>() = object as usize };
        assert_eq!(value.object(), object);
        drop(value);
        assert_eq!(REF_CLEAR_COUNT.load(Ordering::SeqCst), 1);
    }
}
