use godot_api::abi::{ABI_VALUE_OWNED_UTF8, AbiValueType, AbiValueV1};
use godot_api::{
    GDExtensionConstVariantPtr, GDExtensionPtrConstructor, GDExtensionPtrDestructor,
    GDExtensionVariantFromTypeConstructorFunc, GDExtensionVariantGetInternalPtrFunc,
    GDExtensionVariantPtr, GDExtensionVariantType, GDObjectInstanceID,
};

use crate::callable_value::{CallableCallBacking, NativeCallable};
use crate::dynamic_value::{
    DynamicCallBacking, construct_dynamic_variant, decode_dynamic_variant, replace_dynamic_variant,
};
use crate::engine_call::EngineCallContext;
use crate::engine_call::value::ValueError;
use crate::interface::EngineInterface;
use crate::module_value;
use crate::node_path::{OwnedNodePath, read_utf8_node_path};
use crate::packed_array::{OwnedPackedArray, PackedArrayKind, read_packed_bytes};
use crate::signal_value::NativeSignal;
use crate::string_name::{OwnedStringName, StaticStringName, read_utf8_string_name};
use crate::value::{LocalGodotString, read_utf8_string};

/// Cached official Variant codecs for ScriptInstance calls.
pub(crate) struct VariantCodec {
    interface: EngineInterface,
    bool_internal: GDExtensionVariantGetInternalPtrFunc,
    int_internal: GDExtensionVariantGetInternalPtrFunc,
    float_internal: GDExtensionVariantGetInternalPtrFunc,
    string_internal: GDExtensionVariantGetInternalPtrFunc,
    string_name_internal: GDExtensionVariantGetInternalPtrFunc,
    node_path_internal: GDExtensionVariantGetInternalPtrFunc,
    rid_internal: GDExtensionVariantGetInternalPtrFunc,
    rect2_internal: GDExtensionVariantGetInternalPtrFunc,
    rect2i_internal: GDExtensionVariantGetInternalPtrFunc,
    quaternion_internal: GDExtensionVariantGetInternalPtrFunc,
    plane_internal: GDExtensionVariantGetInternalPtrFunc,
    vector4_internal: GDExtensionVariantGetInternalPtrFunc,
    vector4i_internal: GDExtensionVariantGetInternalPtrFunc,
    transform2d_internal: GDExtensionVariantGetInternalPtrFunc,
    aabb_internal: GDExtensionVariantGetInternalPtrFunc,
    basis_internal: GDExtensionVariantGetInternalPtrFunc,
    transform3d_internal: GDExtensionVariantGetInternalPtrFunc,
    projection_internal: GDExtensionVariantGetInternalPtrFunc,
    bool_from: GDExtensionVariantFromTypeConstructorFunc,
    int_from: GDExtensionVariantFromTypeConstructorFunc,
    float_from: GDExtensionVariantFromTypeConstructorFunc,
    string_from: GDExtensionVariantFromTypeConstructorFunc,
    string_name_from: GDExtensionVariantFromTypeConstructorFunc,
    node_path_from: GDExtensionVariantFromTypeConstructorFunc,
    rid_from: GDExtensionVariantFromTypeConstructorFunc,
    object_from: GDExtensionVariantFromTypeConstructorFunc,
    vector2_from: GDExtensionVariantFromTypeConstructorFunc,
    vector2i_from: GDExtensionVariantFromTypeConstructorFunc,
    vector3_from: GDExtensionVariantFromTypeConstructorFunc,
    vector3i_from: GDExtensionVariantFromTypeConstructorFunc,
    color_from: GDExtensionVariantFromTypeConstructorFunc,
    rect2_from: GDExtensionVariantFromTypeConstructorFunc,
    rect2i_from: GDExtensionVariantFromTypeConstructorFunc,
    quaternion_from: GDExtensionVariantFromTypeConstructorFunc,
    plane_from: GDExtensionVariantFromTypeConstructorFunc,
    vector4_from: GDExtensionVariantFromTypeConstructorFunc,
    vector4i_from: GDExtensionVariantFromTypeConstructorFunc,
    transform2d_from: GDExtensionVariantFromTypeConstructorFunc,
    aabb_from: GDExtensionVariantFromTypeConstructorFunc,
    basis_from: GDExtensionVariantFromTypeConstructorFunc,
    transform3d_from: GDExtensionVariantFromTypeConstructorFunc,
    projection_from: GDExtensionVariantFromTypeConstructorFunc,
    vector2_new: GDExtensionPtrConstructor,
    vector2i_new: GDExtensionPtrConstructor,
    vector3_new: GDExtensionPtrConstructor,
    vector3i_new: GDExtensionPtrConstructor,
    color_new: GDExtensionPtrConstructor,
    vector2_drop: GDExtensionPtrDestructor,
    vector2i_drop: GDExtensionPtrDestructor,
    vector3_drop: GDExtensionPtrDestructor,
    vector3i_drop: GDExtensionPtrDestructor,
    color_drop: GDExtensionPtrDestructor,
    components: MathComponentNames,
}

pub(crate) struct VariantDecodeBacking<'a> {
    pub(crate) strings: &'a mut Vec<String>,
    pub(crate) math: &'a mut Vec<Box<[f32]>>,
    pub(crate) packed: &'a mut Vec<Box<[u8]>>,
    pub(crate) dynamic: &'a mut Vec<DynamicCallBacking>,
    pub(crate) callable: &'a mut Vec<CallableCallBacking>,
    pub(crate) dynamic_context: Option<&'a EngineCallContext>,
}

struct MathComponentNames {
    x: StaticStringName,
    y: StaticStringName,
    z: StaticStringName,
    r: StaticStringName,
    g: StaticStringName,
    b: StaticStringName,
    a: StaticStringName,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VariantTypeMismatch {
    pub(crate) expected: GDExtensionVariantType,
}

#[repr(C, align(8))]
struct VariantStorage([u8; 40]);

#[repr(C, align(8))]
struct BuiltinStorage([u8; 32]);

pub(crate) struct OwnedVariant {
    interface: EngineInterface,
    storage: VariantStorage,
    initialized: bool,
}

struct OwnedBuiltin {
    storage: BuiltinStorage,
    destroy: GDExtensionPtrDestructor,
}

impl VariantCodec {
    pub(crate) const fn interface(&self) -> EngineInterface {
        self.interface
    }

    pub(crate) fn new(interface: EngineInterface) -> Option<Self> {
        let get_internal = interface.variant_get_ptr_internal_getter?;
        let get_from = interface.get_variant_from_type_constructor?;
        let get_constructor = interface.variant_get_ptr_constructor?;
        let get_destructor = interface.variant_get_ptr_destructor?;
        // SAFETY: All enum values are official Godot Variant types.
        let bool_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL) };
        // SAFETY: See above.
        let int_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT) };
        // SAFETY: See above.
        let float_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT) };
        // SAFETY: See above.
        let string_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING) };
        // SAFETY: See above.
        let string_name_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME) };
        // SAFETY: NodePath is an official Variant builtin type.
        let node_path_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH) };
        // SAFETY: See above.
        let rid_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID) };
        // SAFETY: The fixed-layout math types are official Variant builtins.
        let rect2_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2) };
        // SAFETY: See above.
        let rect2i_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I) };
        // SAFETY: See above.
        let quaternion_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION) };
        // SAFETY: See above.
        let plane_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE) };
        // SAFETY: See above.
        let vector4_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4) };
        // SAFETY: See above.
        let vector4i_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I) };
        // SAFETY: The matrix-like types are official Variant builtins.
        let transform2d_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D) };
        // SAFETY: See above.
        let aabb_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB) };
        // SAFETY: See above.
        let basis_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS) };
        // SAFETY: See above.
        let transform3d_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D) };
        // SAFETY: See above.
        let projection_internal =
            unsafe { get_internal(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION) };
        // SAFETY: All enum values are official Godot Variant types.
        let bool_from = unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL) };
        // SAFETY: See above.
        let int_from = unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT) };
        // SAFETY: See above.
        let float_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT) };
        // SAFETY: See above.
        let string_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING) };
        // SAFETY: StringName is an official Godot Variant builtin type.
        let string_name_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME) };
        // SAFETY: NodePath is an official Variant builtin type.
        let node_path_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH) };
        // SAFETY: RID is an official Godot Variant builtin type.
        let rid_from = unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID) };
        // SAFETY: Object is an official Godot Variant type.
        let object_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT) };
        // SAFETY: The math types are official Variant builtins.
        let vector2_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2) };
        // SAFETY: See above.
        let vector2i_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I) };
        // SAFETY: See above.
        let vector3_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3) };
        // SAFETY: See above.
        let vector3i_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I) };
        // SAFETY: See above.
        let color_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR) };
        // SAFETY: The fixed-layout math types are official Variant builtins.
        let rect2_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2) };
        // SAFETY: See above.
        let rect2i_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I) };
        // SAFETY: See above.
        let quaternion_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION) };
        // SAFETY: See above.
        let plane_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE) };
        // SAFETY: See above.
        let vector4_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4) };
        // SAFETY: See above.
        let vector4i_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I) };
        // SAFETY: The matrix-like types are official Variant builtins.
        let transform2d_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D) };
        // SAFETY: See above.
        let aabb_from = unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB) };
        // SAFETY: See above.
        let basis_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS) };
        // SAFETY: See above.
        let transform3d_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D) };
        // SAFETY: See above.
        let projection_from =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION) };
        // SAFETY: Constructor zero is the official default constructor.
        let vector2_new =
            unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2, 0) };
        // SAFETY: See above.
        let vector2i_new = unsafe {
            get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I, 0)
        };
        // SAFETY: See above.
        let vector3_new =
            unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3, 0) };
        // SAFETY: See above.
        let vector3i_new = unsafe {
            get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I, 0)
        };
        // SAFETY: See above.
        let color_new =
            unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR, 0) };
        // SAFETY: These destructors match the official builtin types. Trivial
        // builtins may legitimately have no destructor.
        let vector2_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2) };
        // SAFETY: See above.
        let vector2i_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I) };
        // SAFETY: See above.
        let vector3_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3) };
        // SAFETY: See above.
        let vector3i_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I) };
        // SAFETY: See above.
        let color_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR) };
        bool_internal?;
        int_internal?;
        float_internal?;
        string_internal?;
        string_name_internal?;
        node_path_internal?;
        rid_internal?;
        rect2_internal?;
        rect2i_internal?;
        quaternion_internal?;
        plane_internal?;
        vector4_internal?;
        vector4i_internal?;
        transform2d_internal?;
        aabb_internal?;
        basis_internal?;
        transform3d_internal?;
        projection_internal?;
        bool_from?;
        int_from?;
        float_from?;
        string_from?;
        string_name_from?;
        node_path_from?;
        rid_from?;
        object_from?;
        vector2_from?;
        vector2i_from?;
        vector3_from?;
        vector3i_from?;
        color_from?;
        rect2_from?;
        rect2i_from?;
        quaternion_from?;
        plane_from?;
        vector4_from?;
        vector4i_from?;
        transform2d_from?;
        aabb_from?;
        basis_from?;
        transform3d_from?;
        projection_from?;
        vector2_new?;
        vector2i_new?;
        vector3_new?;
        vector3i_new?;
        color_new?;
        interface.variant_get_named?;
        interface.variant_set_named?;
        interface.variant_new_copy?;
        Some(Self {
            interface,
            bool_internal,
            int_internal,
            float_internal,
            string_internal,
            string_name_internal,
            node_path_internal,
            rid_internal,
            rect2_internal,
            rect2i_internal,
            quaternion_internal,
            plane_internal,
            vector4_internal,
            vector4i_internal,
            transform2d_internal,
            aabb_internal,
            basis_internal,
            transform3d_internal,
            projection_internal,
            bool_from,
            int_from,
            float_from,
            string_from,
            string_name_from,
            node_path_from,
            rid_from,
            object_from,
            vector2_from,
            vector2i_from,
            vector3_from,
            vector3i_from,
            color_from,
            rect2_from,
            rect2i_from,
            quaternion_from,
            plane_from,
            vector4_from,
            vector4i_from,
            transform2d_from,
            aabb_from,
            basis_from,
            transform3d_from,
            projection_from,
            vector2_new,
            vector2i_new,
            vector3_new,
            vector3i_new,
            color_new,
            vector2_drop,
            vector2i_drop,
            vector3_drop,
            vector3i_drop,
            color_drop,
            components: MathComponentNames {
                x: StaticStringName::new(interface, c"x"),
                y: StaticStringName::new(interface, c"y"),
                z: StaticStringName::new(interface, c"z"),
                r: StaticStringName::new(interface, c"r"),
                g: StaticStringName::new(interface, c"g"),
                b: StaticStringName::new(interface, c"b"),
                a: StaticStringName::new(interface, c"a"),
            },
        })
    }

    pub(crate) fn decode(
        &self,
        value: GDExtensionConstVariantPtr,
        expected: AbiValueType,
        backing: VariantDecodeBacking<'_>,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let VariantDecodeBacking {
            strings: string_backing,
            math: math_backing,
            packed: packed_backing,
            dynamic: dynamic_backing,
            callable: callable_backing,
            dynamic_context,
        } = backing;
        match expected {
            AbiValueType::NIL => {
                let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL;
                (self.variant_type(value) == Some(expected))
                    .then_some(AbiValueV1::NIL)
                    .ok_or(VariantTypeMismatch { expected })
            }
            AbiValueType::BOOL => self.read_bool(value).map(AbiValueV1::from_bool),
            AbiValueType::I64 => self.read_i64(value).map(AbiValueV1::from_i64),
            AbiValueType::U64 => self
                .read_i64(value)
                .map(|value| AbiValueV1::from_u64(value as u64)),
            AbiValueType::F64 => self.read_f64(value).map(AbiValueV1::from_f64),
            AbiValueType::STRING => self.read_string(value, string_backing),
            AbiValueType::STRING_NAME => self.read_string_name(value, string_backing),
            AbiValueType::NODE_PATH => self.read_node_path(value, string_backing),
            AbiValueType::OBJECT_ID => self.read_object_id(value).map(AbiValueV1::from_object_id),
            AbiValueType::VECTOR2 => self.read_vector2(value),
            AbiValueType::VECTOR2I => self.read_vector2i(value),
            AbiValueType::VECTOR3 => self.read_vector3(value),
            AbiValueType::VECTOR3I => self.read_vector3i(value),
            AbiValueType::VECTOR4 => self
                .read_f32x4(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4,
                    self.vector4_internal,
                )
                .map(|[x, y, z, w]| AbiValueV1::from_vector4(x, y, z, w)),
            AbiValueType::VECTOR4I => self
                .read_i32x4(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I,
                    self.vector4i_internal,
                )
                .map(|[x, y, z, w]| AbiValueV1::from_vector4i(x, y, z, w)),
            AbiValueType::RECT2 => self
                .read_f32x4(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2,
                    self.rect2_internal,
                )
                .map(|[x, y, width, height]| AbiValueV1::from_rect2(x, y, width, height)),
            AbiValueType::RECT2I => self
                .read_i32x4(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I,
                    self.rect2i_internal,
                )
                .map(|[x, y, width, height]| AbiValueV1::from_rect2i(x, y, width, height)),
            AbiValueType::QUATERNION => self
                .read_f32x4(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION,
                    self.quaternion_internal,
                )
                .map(|[x, y, z, w]| AbiValueV1::from_quaternion(x, y, z, w)),
            AbiValueType::PLANE => self
                .read_f32x4(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE,
                    self.plane_internal,
                )
                .map(|[x, y, z, d]| AbiValueV1::from_plane(x, y, z, d)),
            AbiValueType::TRANSFORM2D => {
                let value = self.read_f32_components::<6>(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D,
                    self.transform2d_internal,
                )?;
                Ok(borrow_math_backing(
                    math_backing,
                    AbiValueType::TRANSFORM2D,
                    &value,
                ))
            }
            AbiValueType::AABB => {
                let value = self.read_f32_components::<6>(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB,
                    self.aabb_internal,
                )?;
                Ok(borrow_math_backing(
                    math_backing,
                    AbiValueType::AABB,
                    &value,
                ))
            }
            AbiValueType::BASIS => {
                let value = self.read_f32_components::<9>(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS,
                    self.basis_internal,
                )?;
                Ok(borrow_math_backing(
                    math_backing,
                    AbiValueType::BASIS,
                    &value,
                ))
            }
            AbiValueType::TRANSFORM3D => {
                let value = self.read_f32_components::<12>(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D,
                    self.transform3d_internal,
                )?;
                Ok(borrow_math_backing(
                    math_backing,
                    AbiValueType::TRANSFORM3D,
                    &value,
                ))
            }
            AbiValueType::PROJECTION => {
                let value = self.read_f32_components::<16>(
                    value,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION,
                    self.projection_internal,
                )?;
                Ok(borrow_math_backing(
                    math_backing,
                    AbiValueType::PROJECTION,
                    &value,
                ))
            }
            AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY => {
                self.read_packed(value, expected, packed_backing)
            }
            AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY => {
                let value = decode_dynamic_variant(self, value, expected, dynamic_context)
                    .map_err(|_| VariantTypeMismatch {
                        expected: dynamic_variant_type(expected),
                    })?;
                dynamic_backing.push(value);
                Ok(dynamic_backing
                    .last()
                    .expect("dynamic backing was just appended")
                    .abi(expected))
            }
            AbiValueType::CALLABLE => {
                let context = dynamic_context.ok_or(VariantTypeMismatch {
                    expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE,
                })?;
                let value =
                    CallableCallBacking::from_variant(self, value, context).map_err(|_| {
                        VariantTypeMismatch {
                            expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE,
                        }
                    })?;
                callable_backing.push(value);
                Ok(callable_backing
                    .last()
                    .expect("Callable backing was just appended")
                    .abi())
            }
            AbiValueType::SIGNAL => {
                let value =
                    NativeSignal::from_variant(self, value).map_err(|_| VariantTypeMismatch {
                        expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL,
                    })?;
                let bytes = value.to_bytes().map_err(|_| VariantTypeMismatch {
                    expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL,
                })?;
                packed_backing.push(bytes.into_boxed_slice());
                Ok(AbiValueV1::from_borrowed_bytes(
                    AbiValueType::SIGNAL,
                    packed_backing
                        .last()
                        .expect("Signal backing was just appended"),
                ))
            }
            AbiValueType::COLOR => self.read_color(value),
            AbiValueType::RID => self.read_rid(value).map(AbiValueV1::from_rid),
            _ => Err(VariantTypeMismatch {
                expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
            }),
        }
    }

    pub(crate) fn read_string_value(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<String, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING;
        let value = self.read_internal(value, expected, self.string_internal)?;
        read_utf8_string(self.interface, value.cast()).map_err(|_| VariantTypeMismatch { expected })
    }

    pub(crate) fn object_is_class(&self, object_id: u64, class_name: &str) -> bool {
        if object_id == 0 {
            return true;
        }
        let Some(object_from_id) = self.interface.object_get_instance_from_id else {
            return false;
        };
        // SAFETY: Object instance IDs are opaque integers accepted by Godot.
        let object = unsafe { object_from_id(object_id) };
        if object.is_null() {
            return false;
        }
        let Some(class_name) = OwnedStringName::new(self.interface, class_name) else {
            return false;
        };
        let Some(get_class_tag) = self.interface.classdb_get_class_tag else {
            return false;
        };
        // SAFETY: `class_name` owns an initialized StringName for this lookup.
        let class_tag = unsafe { get_class_tag(class_name.as_ptr()) };
        if class_tag.is_null() {
            return false;
        }
        let Some(cast_to) = self.interface.object_cast_to else {
            return false;
        };
        // SAFETY: The object came from ObjectDB and the tag came from ClassDB.
        !unsafe { cast_to(object, class_tag) }.is_null()
    }

    pub(crate) fn encode_with_array_type(
        &self,
        value: AbiValueV1,
        output: GDExtensionVariantPtr,
        typed_array_element: Option<&str>,
    ) -> Result<(), ()> {
        self.encode_with_context(value, output, typed_array_element, None)
    }

    pub(crate) fn encode_with_context(
        &self,
        value: AbiValueV1,
        output: GDExtensionVariantPtr,
        typed_array_element: Option<&str>,
        context: Option<&EngineCallContext>,
    ) -> Result<(), ()> {
        if output.is_null() {
            return Err(());
        }
        match value.type_ {
            AbiValueType::NIL if value.reserved_flags == 0 && value.payload == [0; 2] => {
                self.replace_with_nil(output)
            }
            AbiValueType::BOOL
                if value.reserved_flags == 0 && value.payload[0] <= 1 && value.payload[1] == 0 =>
            {
                let raw = value.payload[0] as u8;
                self.replace_with(output, self.bool_from, (&raw as *const u8).cast())
            }
            AbiValueType::I64 if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let raw = value.payload[0] as i64;
                self.replace_with(output, self.int_from, (&raw as *const i64).cast())
            }
            AbiValueType::U64 if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let raw = value.payload[0] as i64;
                self.replace_with(output, self.int_from, (&raw as *const i64).cast())
            }
            AbiValueType::F64 if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let raw = f64::from_bits(value.payload[0]);
                self.replace_with(output, self.float_from, (&raw as *const f64).cast())
            }
            AbiValueType::STRING if matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8) => {
                let text = module_value::utf8(&value)?;
                let string = LocalGodotString::new_utf8(self.interface, text).ok_or(())?;
                self.replace_with(output, self.string_from, string.as_ptr())
            }
            AbiValueType::STRING_NAME
                if matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8) =>
            {
                let text = module_value::utf8(&value)?;
                let string_name = OwnedStringName::new(self.interface, text).ok_or(())?;
                self.replace_with(output, self.string_name_from, string_name.as_ptr())
            }
            AbiValueType::NODE_PATH if matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8) => {
                let text = module_value::utf8(&value)?;
                let node_path = OwnedNodePath::new(self.interface, text).ok_or(())?;
                self.replace_with(output, self.node_path_from, node_path.as_ptr())
            }
            AbiValueType::OBJECT_ID if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let object = self.object_from_id(value.payload[0])?;
                self.replace_with(
                    output,
                    self.object_from,
                    (&object as *const *mut core::ffi::c_void).cast_mut().cast(),
                )
            }
            AbiValueType::RID => {
                let raw = value.rid().ok_or(())?;
                self.replace_with(output, self.rid_from, (&raw as *const u64).cast())
            }
            AbiValueType::VECTOR2
            | AbiValueType::VECTOR2I
            | AbiValueType::VECTOR3
            | AbiValueType::VECTOR3I
            | AbiValueType::COLOR
            | AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY => {
                let replacement = OwnedVariant::from_abi(self, value)?;
                self.replace_with_variant(output, &replacement)
            }
            AbiValueType::RECT2 => {
                let raw = value.rect2().ok_or(())?;
                self.replace_with(output, self.rect2_from, raw.as_ptr().cast())
            }
            AbiValueType::RECT2I => {
                let raw = value.rect2i().ok_or(())?;
                self.replace_with(output, self.rect2i_from, raw.as_ptr().cast())
            }
            AbiValueType::QUATERNION => {
                let raw = value.quaternion().ok_or(())?;
                self.replace_with(output, self.quaternion_from, raw.as_ptr().cast())
            }
            AbiValueType::PLANE => {
                let raw = value.plane().ok_or(())?;
                self.replace_with(output, self.plane_from, raw.as_ptr().cast())
            }
            AbiValueType::VECTOR4 => {
                let raw = value.vector4().ok_or(())?;
                self.replace_with(output, self.vector4_from, raw.as_ptr().cast())
            }
            AbiValueType::VECTOR4I => {
                let raw = value.vector4i().ok_or(())?;
                self.replace_with(output, self.vector4i_from, raw.as_ptr().cast())
            }
            AbiValueType::TRANSFORM2D => {
                let raw = abi_f32_components::<6>(value, AbiValueType::TRANSFORM2D)?;
                self.replace_with(output, self.transform2d_from, raw.as_ptr().cast())
            }
            AbiValueType::AABB => {
                let raw = abi_f32_components::<6>(value, AbiValueType::AABB)?;
                self.replace_with(output, self.aabb_from, raw.as_ptr().cast())
            }
            AbiValueType::BASIS => {
                let raw = abi_f32_components::<9>(value, AbiValueType::BASIS)?;
                self.replace_with(output, self.basis_from, raw.as_ptr().cast())
            }
            AbiValueType::TRANSFORM3D => {
                let raw = abi_f32_components::<12>(value, AbiValueType::TRANSFORM3D)?;
                self.replace_with(output, self.transform3d_from, raw.as_ptr().cast())
            }
            AbiValueType::PROJECTION => {
                let raw = abi_f32_components::<16>(value, AbiValueType::PROJECTION)?;
                self.replace_with(output, self.projection_from, raw.as_ptr().cast())
            }
            AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY => {
                replace_dynamic_variant(self, value, output, typed_array_element, context)
                    .map_err(|_| ())
            }
            AbiValueType::CALLABLE => {
                let callable =
                    NativeCallable::from_abi(self.interface, value, context, |object_id| {
                        self.object_from_id(object_id).map_err(|_| {
                            ValueError::invalid("Godot Callable target no longer exists")
                        })
                    })
                    .map_err(|_| ())?;
                let variant = callable.to_variant(self).map_err(|_| ())?;
                self.replace_with_variant(output, &variant)
            }
            AbiValueType::SIGNAL => {
                let signal = NativeSignal::from_abi(self.interface, value, |object_id| {
                    self.object_from_id(object_id)
                        .map_err(|_| ValueError::invalid("Godot Signal target no longer exists"))
                })
                .map_err(|_| ())?;
                let variant = signal.to_variant().map_err(|_| ())?;
                self.replace_with_variant(output, &variant)
            }
            _ => Err(()),
        }
    }

    pub(crate) fn construct_with_context(
        &self,
        value: AbiValueV1,
        output: GDExtensionVariantPtr,
        typed_array_element: Option<&str>,
        context: Option<&EngineCallContext>,
    ) -> Result<(), ()> {
        if output.is_null() {
            return Err(());
        }
        match value.type_ {
            AbiValueType::NIL if value.reserved_flags == 0 && value.payload == [0; 2] => {
                let constructor = self.interface.variant_new_nil.ok_or(())?;
                // SAFETY: ScriptExtension supplies uninitialized Variant
                // return storage.
                unsafe { constructor(output) };
                Ok(())
            }
            AbiValueType::BOOL
                if value.reserved_flags == 0 && value.payload[0] <= 1 && value.payload[1] == 0 =>
            {
                let raw = value.payload[0] as u8;
                Self::construct_with(output, self.bool_from, (&raw as *const u8).cast())
            }
            AbiValueType::I64 if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let raw = value.payload[0] as i64;
                Self::construct_with(output, self.int_from, (&raw as *const i64).cast())
            }
            AbiValueType::U64 if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let raw = value.payload[0] as i64;
                Self::construct_with(output, self.int_from, (&raw as *const i64).cast())
            }
            AbiValueType::F64 if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let raw = f64::from_bits(value.payload[0]);
                Self::construct_with(output, self.float_from, (&raw as *const f64).cast())
            }
            AbiValueType::STRING if matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8) => {
                let text = module_value::utf8(&value)?;
                let string = LocalGodotString::new_utf8(self.interface, text).ok_or(())?;
                Self::construct_with(output, self.string_from, string.as_ptr())
            }
            AbiValueType::STRING_NAME
                if matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8) =>
            {
                let text = module_value::utf8(&value)?;
                let string_name = OwnedStringName::new(self.interface, text).ok_or(())?;
                Self::construct_with(output, self.string_name_from, string_name.as_ptr())
            }
            AbiValueType::NODE_PATH if matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8) => {
                let text = module_value::utf8(&value)?;
                let node_path = OwnedNodePath::new(self.interface, text).ok_or(())?;
                Self::construct_with(output, self.node_path_from, node_path.as_ptr())
            }
            AbiValueType::OBJECT_ID if value.reserved_flags == 0 && value.payload[1] == 0 => {
                let object = self.object_from_id(value.payload[0])?;
                Self::construct_with(
                    output,
                    self.object_from,
                    (&object as *const *mut core::ffi::c_void).cast_mut().cast(),
                )
            }
            AbiValueType::RID => {
                let raw = value.rid().ok_or(())?;
                Self::construct_with(output, self.rid_from, (&raw as *const u64).cast())
            }
            AbiValueType::VECTOR2 => {
                let [x, y] = value.vector2().ok_or(())?;
                self.construct_math(
                    output,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2,
                    &[(&self.components.x, x), (&self.components.y, y)],
                )
            }
            AbiValueType::VECTOR2I => {
                let [x, y] = value.vector2i().ok_or(())?;
                self.construct_integer_math(
                    output,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I,
                    &[(&self.components.x, x), (&self.components.y, y)],
                )
            }
            AbiValueType::VECTOR3 => {
                let [x, y, z] = value.vector3().ok_or(())?;
                self.construct_math(
                    output,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3,
                    &[
                        (&self.components.x, x),
                        (&self.components.y, y),
                        (&self.components.z, z),
                    ],
                )
            }
            AbiValueType::VECTOR3I => {
                let [x, y, z] = value.vector3i().ok_or(())?;
                self.construct_integer_math(
                    output,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I,
                    &[
                        (&self.components.x, x),
                        (&self.components.y, y),
                        (&self.components.z, z),
                    ],
                )
            }
            AbiValueType::COLOR => {
                let [r, g, b, a] = value.color().ok_or(())?;
                self.construct_math(
                    output,
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR,
                    &[
                        (&self.components.r, r),
                        (&self.components.g, g),
                        (&self.components.b, b),
                        (&self.components.a, a),
                    ],
                )
            }
            AbiValueType::RECT2 => {
                let raw = value.rect2().ok_or(())?;
                Self::construct_with(output, self.rect2_from, raw.as_ptr().cast())
            }
            AbiValueType::RECT2I => {
                let raw = value.rect2i().ok_or(())?;
                Self::construct_with(output, self.rect2i_from, raw.as_ptr().cast())
            }
            AbiValueType::QUATERNION => {
                let raw = value.quaternion().ok_or(())?;
                Self::construct_with(output, self.quaternion_from, raw.as_ptr().cast())
            }
            AbiValueType::PLANE => {
                let raw = value.plane().ok_or(())?;
                Self::construct_with(output, self.plane_from, raw.as_ptr().cast())
            }
            AbiValueType::VECTOR4 => {
                let raw = value.vector4().ok_or(())?;
                Self::construct_with(output, self.vector4_from, raw.as_ptr().cast())
            }
            AbiValueType::VECTOR4I => {
                let raw = value.vector4i().ok_or(())?;
                Self::construct_with(output, self.vector4i_from, raw.as_ptr().cast())
            }
            AbiValueType::TRANSFORM2D => {
                let raw = abi_f32_components::<6>(value, AbiValueType::TRANSFORM2D)?;
                Self::construct_with(output, self.transform2d_from, raw.as_ptr().cast())
            }
            AbiValueType::AABB => {
                let raw = abi_f32_components::<6>(value, AbiValueType::AABB)?;
                Self::construct_with(output, self.aabb_from, raw.as_ptr().cast())
            }
            AbiValueType::BASIS => {
                let raw = abi_f32_components::<9>(value, AbiValueType::BASIS)?;
                Self::construct_with(output, self.basis_from, raw.as_ptr().cast())
            }
            AbiValueType::TRANSFORM3D => {
                let raw = abi_f32_components::<12>(value, AbiValueType::TRANSFORM3D)?;
                Self::construct_with(output, self.transform3d_from, raw.as_ptr().cast())
            }
            AbiValueType::PROJECTION => {
                let raw = abi_f32_components::<16>(value, AbiValueType::PROJECTION)?;
                Self::construct_with(output, self.projection_from, raw.as_ptr().cast())
            }
            AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY => {
                let packed = OwnedPackedArray::from_abi(self.interface, value).map_err(|_| ())?;
                let get_from = self.interface.get_variant_from_type_constructor.ok_or(())?;
                // SAFETY: The normalized type maps one-to-one to this official
                // packed-array Variant type.
                let constructor = unsafe {
                    get_from(
                        PackedArrayKind::from_value_type(value.type_)
                            .ok_or(())?
                            .variant_type(),
                    )
                }
                .ok_or(())?;
                // SAFETY: Output is uninitialized Variant storage and packed
                // owns one initialized builtin of the exact requested type.
                unsafe { constructor(output, packed.as_const_ptr().cast_mut()) };
                Ok(())
            }
            AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY => {
                construct_dynamic_variant(self, value, output, typed_array_element, context)
                    .map_err(|_| ())
            }
            AbiValueType::CALLABLE => {
                let callable =
                    NativeCallable::from_abi(self.interface, value, context, |object_id| {
                        self.object_from_id(object_id).map_err(|_| {
                            ValueError::invalid("Godot Callable target no longer exists")
                        })
                    })
                    .map_err(|_| ())?;
                let get_from = self.interface.get_variant_from_type_constructor.ok_or(())?;
                let constructor = {
                    // SAFETY: Callable is an official Variant builtin type.
                    unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE) }
                }
                .ok_or(())?;
                // SAFETY: Output is uninitialized Variant storage and the
                // source owns one initialized Callable.
                unsafe { constructor(output, callable.as_ptr().cast_mut()) };
                Ok(())
            }
            AbiValueType::SIGNAL => {
                let signal = NativeSignal::from_abi(self.interface, value, |object_id| {
                    self.object_from_id(object_id)
                        .map_err(|_| ValueError::invalid("Godot Signal target no longer exists"))
                })
                .map_err(|_| ())?;
                let get_from = self.interface.get_variant_from_type_constructor.ok_or(())?;
                let constructor = {
                    // SAFETY: Signal is an official Variant builtin type.
                    unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL) }
                }
                .ok_or(())?;
                // SAFETY: Output is uninitialized Variant storage and source
                // owns one initialized Signal.
                unsafe { constructor(output, signal.as_ptr().cast_mut()) };
                Ok(())
            }
            _ => Err(()),
        }
    }

    pub(crate) fn read_f64(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<f64, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT;
        if self.variant_type(value) != Some(expected) {
            return Err(VariantTypeMismatch { expected });
        }
        let get_internal = self
            .float_internal
            .expect("float Variant getter was cached during construction");
        // SAFETY: The type check above proves the Variant contains a float.
        let value = unsafe { get_internal(value.cast_mut()) };
        if value.is_null() {
            return Err(VariantTypeMismatch { expected });
        }
        // SAFETY: Godot's FLOAT internal getter returns a live `double` pointer
        // for the duration of this synchronous ScriptInstance call.
        Ok(unsafe { value.cast::<f64>().read() })
    }

    fn read_bool(&self, value: GDExtensionConstVariantPtr) -> Result<bool, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL;
        let value = self.read_internal(value, expected, self.bool_internal)?;
        // SAFETY: Godot's BOOL internal getter returns a live C++ bool.
        Ok(unsafe { value.cast::<u8>().read() } != 0)
    }

    fn read_i64(&self, value: GDExtensionConstVariantPtr) -> Result<i64, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT;
        let value = self.read_internal(value, expected, self.int_internal)?;
        // SAFETY: Godot's INT internal getter returns a live `int64_t`.
        Ok(unsafe { value.cast::<i64>().read() })
    }

    fn read_string(
        &self,
        value: GDExtensionConstVariantPtr,
        backing: &mut Vec<String>,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING;
        let value = self.read_internal(value, expected, self.string_internal)?;
        let text = read_utf8_string(self.interface, value.cast())
            .map_err(|_| VariantTypeMismatch { expected })?;
        backing.push(text);
        Ok(AbiValueV1::from_borrowed_utf8(
            backing.last().expect("String backing was just appended"),
        ))
    }

    fn read_string_name(
        &self,
        value: GDExtensionConstVariantPtr,
        backing: &mut Vec<String>,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME;
        let value = self.read_internal(value, expected, self.string_name_internal)?;
        let text = read_utf8_string_name(self.interface, value.cast())
            .map_err(|_| VariantTypeMismatch { expected })?;
        backing.push(text);
        Ok(AbiValueV1::from_borrowed_string_name(
            backing
                .last()
                .expect("StringName backing was just appended"),
        ))
    }

    fn read_node_path(
        &self,
        value: GDExtensionConstVariantPtr,
        backing: &mut Vec<String>,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH;
        let value = self.read_internal(value, expected, self.node_path_internal)?;
        let text = read_utf8_node_path(self.interface, value.cast())
            .map_err(|_| VariantTypeMismatch { expected })?;
        backing.push(text);
        Ok(AbiValueV1::from_borrowed_node_path(
            backing.last().expect("NodePath backing was just appended"),
        ))
    }

    fn read_rid(&self, value: GDExtensionConstVariantPtr) -> Result<u64, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID;
        let value = self.read_internal(value, expected, self.rid_internal)?;
        // SAFETY: The official RID internal getter returns its fixed eight-byte
        // opaque value, whose size is authenticated for every API target.
        Ok(unsafe { value.cast::<u64>().read() })
    }

    fn read_packed(
        &self,
        value: GDExtensionConstVariantPtr,
        expected_type: AbiValueType,
        backing: &mut Vec<Box<[u8]>>,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let kind = PackedArrayKind::from_value_type(expected_type).ok_or(VariantTypeMismatch {
            expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
        })?;
        let expected = kind.variant_type();
        if self.variant_type(value) != Some(expected) {
            return Err(VariantTypeMismatch { expected });
        }
        let get_internal = self
            .interface
            .variant_get_ptr_internal_getter
            .ok_or(VariantTypeMismatch { expected })?;
        let get_internal = {
            // SAFETY: Expected is one official packed-array Variant type.
            unsafe { get_internal(expected) }
        }
        .ok_or(VariantTypeMismatch { expected })?;
        // SAFETY: The exact Variant type was checked above.
        let packed = unsafe { get_internal(value.cast_mut()) };
        let bytes = read_packed_bytes(self.interface, kind, packed.cast())
            .map_err(|_| VariantTypeMismatch { expected })?
            .into_boxed_slice();
        backing.push(bytes);
        Ok(AbiValueV1::from_borrowed_bytes(
            expected_type,
            backing
                .last()
                .expect("packed-array backing was just appended"),
        ))
    }

    pub(crate) fn read_object_id(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<GDObjectInstanceID, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT;
        match self.variant_type(value) {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL) => return Ok(0),
            Some(actual) if actual == expected => {}
            _ => {
                return Err(VariantTypeMismatch { expected });
            }
        }
        let get_id = self
            .interface
            .variant_get_object_instance_id
            .expect("required Variant object ID getter was resolved");
        // SAFETY: `value` is a live Object Variant for this call.
        Ok(unsafe { get_id(value) })
    }

    pub(crate) fn object_from_id(&self, object_id: u64) -> Result<*mut core::ffi::c_void, ()> {
        if object_id == 0 {
            return Ok(core::ptr::null_mut());
        }
        let get_instance = self.interface.object_get_instance_from_id.ok_or(())?;
        // SAFETY: Godot instance IDs are opaque integers accepted by ObjectDB.
        let object = unsafe { get_instance(object_id) };
        (!object.is_null()).then_some(object).ok_or(())
    }

    fn read_vector2(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2;
        self.require_variant_type(value, expected)?;
        let x = self
            .read_math_component(value, &self.components.x)
            .ok_or(VariantTypeMismatch { expected })?;
        let y = self
            .read_math_component(value, &self.components.y)
            .ok_or(VariantTypeMismatch { expected })?;
        Ok(AbiValueV1::from_vector2(x, y))
    }

    fn read_vector2i(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I;
        self.require_variant_type(value, expected)?;
        let x = self
            .read_integer_component(value, &self.components.x)
            .ok_or(VariantTypeMismatch { expected })?;
        let y = self
            .read_integer_component(value, &self.components.y)
            .ok_or(VariantTypeMismatch { expected })?;
        Ok(AbiValueV1::from_vector2i(x, y))
    }

    fn read_vector3(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3;
        self.require_variant_type(value, expected)?;
        let x = self
            .read_math_component(value, &self.components.x)
            .ok_or(VariantTypeMismatch { expected })?;
        let y = self
            .read_math_component(value, &self.components.y)
            .ok_or(VariantTypeMismatch { expected })?;
        let z = self
            .read_math_component(value, &self.components.z)
            .ok_or(VariantTypeMismatch { expected })?;
        Ok(AbiValueV1::from_vector3(x, y, z))
    }

    fn read_vector3i(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I;
        self.require_variant_type(value, expected)?;
        let x = self
            .read_integer_component(value, &self.components.x)
            .ok_or(VariantTypeMismatch { expected })?;
        let y = self
            .read_integer_component(value, &self.components.y)
            .ok_or(VariantTypeMismatch { expected })?;
        let z = self
            .read_integer_component(value, &self.components.z)
            .ok_or(VariantTypeMismatch { expected })?;
        Ok(AbiValueV1::from_vector3i(x, y, z))
    }

    fn read_color(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Result<AbiValueV1, VariantTypeMismatch> {
        let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR;
        self.require_variant_type(value, expected)?;
        let r = self
            .read_math_component(value, &self.components.r)
            .ok_or(VariantTypeMismatch { expected })?;
        let g = self
            .read_math_component(value, &self.components.g)
            .ok_or(VariantTypeMismatch { expected })?;
        let b = self
            .read_math_component(value, &self.components.b)
            .ok_or(VariantTypeMismatch { expected })?;
        let a = self
            .read_math_component(value, &self.components.a)
            .ok_or(VariantTypeMismatch { expected })?;
        Ok(AbiValueV1::from_color(r, g, b, a))
    }

    fn read_f32x4(
        &self,
        value: GDExtensionConstVariantPtr,
        expected: GDExtensionVariantType,
        getter: GDExtensionVariantGetInternalPtrFunc,
    ) -> Result<[f32; 4], VariantTypeMismatch> {
        let value = self.read_internal(value, expected, getter)?;
        // SAFETY: The selected official standard-precision builtin consists
        // of exactly four contiguous f32 components.
        Ok(unsafe { value.cast::<[f32; 4]>().read() })
    }

    fn read_f32_components<const N: usize>(
        &self,
        value: GDExtensionConstVariantPtr,
        expected: GDExtensionVariantType,
        getter: GDExtensionVariantGetInternalPtrFunc,
    ) -> Result<[f32; N], VariantTypeMismatch> {
        let value = self.read_internal(value, expected, getter)?;
        // SAFETY: Authenticated standard-precision layouts contain exactly N
        // contiguous f32 components for the selected builtin.
        Ok(unsafe { value.cast::<[f32; N]>().read() })
    }

    fn read_i32x4(
        &self,
        value: GDExtensionConstVariantPtr,
        expected: GDExtensionVariantType,
        getter: GDExtensionVariantGetInternalPtrFunc,
    ) -> Result<[i32; 4], VariantTypeMismatch> {
        let value = self.read_internal(value, expected, getter)?;
        // SAFETY: The selected official integer builtin consists of exactly
        // four contiguous i32 components.
        Ok(unsafe { value.cast::<[i32; 4]>().read() })
    }

    fn require_variant_type(
        &self,
        value: GDExtensionConstVariantPtr,
        expected: GDExtensionVariantType,
    ) -> Result<(), VariantTypeMismatch> {
        (self.variant_type(value) == Some(expected))
            .then_some(())
            .ok_or(VariantTypeMismatch { expected })
    }

    fn read_math_component(
        &self,
        value: GDExtensionConstVariantPtr,
        name: &StaticStringName,
    ) -> Option<f32> {
        let get_named = self.interface.variant_get_named?;
        let mut component = OwnedVariant::uninitialized(self.interface);
        let mut valid = 0;
        // SAFETY: `value` and `name` are initialized official values, and the
        // result points to uninitialized Variant storage. Godot constructs a
        // result even when the named lookup is invalid.
        unsafe { get_named(value, name.as_ptr(), component.as_mut_ptr(), &mut valid) };
        component.mark_initialized();
        if valid == 0 {
            return None;
        }
        let value = self.read_f64(component.as_ptr()).ok()?;
        let converted = value as f32;
        (!value.is_finite() || converted.is_finite()).then_some(converted)
    }

    fn read_integer_component(
        &self,
        value: GDExtensionConstVariantPtr,
        name: &StaticStringName,
    ) -> Option<i32> {
        let get_named = self.interface.variant_get_named?;
        let mut component = OwnedVariant::uninitialized(self.interface);
        let mut valid = 0;
        // SAFETY: `value` and `name` are initialized official values, and the
        // result points to uninitialized Variant storage.
        unsafe { get_named(value, name.as_ptr(), component.as_mut_ptr(), &mut valid) };
        component.mark_initialized();
        if valid == 0 {
            return None;
        }
        i32::try_from(self.read_i64(component.as_ptr()).ok()?).ok()
    }

    pub(crate) fn variant_type(
        &self,
        value: GDExtensionConstVariantPtr,
    ) -> Option<GDExtensionVariantType> {
        if value.is_null() {
            return None;
        }
        let get_type = self
            .interface
            .variant_get_type
            .expect("required Variant type getter was resolved");
        // SAFETY: The ScriptInstance call supplies live argument Variants.
        Some(unsafe { get_type(value) })
    }

    fn read_internal(
        &self,
        value: GDExtensionConstVariantPtr,
        expected: GDExtensionVariantType,
        getter: GDExtensionVariantGetInternalPtrFunc,
    ) -> Result<*mut core::ffi::c_void, VariantTypeMismatch> {
        if self.variant_type(value) != Some(expected) {
            return Err(VariantTypeMismatch { expected });
        }
        let getter = getter.expect("Variant getter was cached during construction");
        // SAFETY: The type check above matches the cached getter.
        let value = unsafe { getter(value.cast_mut()) };
        (!value.is_null())
            .then_some(value)
            .ok_or(VariantTypeMismatch { expected })
    }

    fn replace_with(
        &self,
        output: GDExtensionVariantPtr,
        constructor: GDExtensionVariantFromTypeConstructorFunc,
        value: *const core::ffi::c_void,
    ) -> Result<(), ()> {
        let destroy = self.interface.variant_destroy.ok_or(())?;
        let constructor = constructor.ok_or(())?;
        // SAFETY: ScriptInstance supplies an initialized Variant. Destruction
        // is immediately followed by placement construction in the same slot.
        unsafe {
            destroy(output);
            constructor(output, value.cast_mut());
        }
        Ok(())
    }

    fn replace_with_variant(
        &self,
        output: GDExtensionVariantPtr,
        value: &OwnedVariant,
    ) -> Result<(), ()> {
        let destroy = self.interface.variant_destroy.ok_or(())?;
        let copy = self.interface.variant_new_copy.ok_or(())?;
        // SAFETY: `output` contains one initialized Variant. After destroying
        // it, the official copy constructor initializes the same storage from
        // the retained temporary.
        unsafe {
            destroy(output);
            copy(output, value.as_ptr());
        }
        Ok(())
    }

    fn construct_math(
        &self,
        output: GDExtensionVariantPtr,
        type_: GDExtensionVariantType,
        components: &[(&StaticStringName, f32)],
    ) -> Result<(), ()> {
        let (constructor, destructor, from_type) = self.math_codec(type_).ok_or(())?;
        let builtin = OwnedBuiltin::new(constructor, destructor).ok_or(())?;
        let from_type = from_type.ok_or(())?;
        let set_named = self.interface.variant_set_named.ok_or(())?;
        // SAFETY: `builtin` owns a default-constructed value of `type_`; the
        // matching official converter initializes the Variant output.
        unsafe { from_type(output, builtin.as_ptr().cast_mut()) };

        for (name, component) in components {
            let value = OwnedVariant::from_abi(self, AbiValueV1::from_f64(f64::from(*component)))
                .expect("float Variant constructor was cached");
            let mut valid = 0;
            // SAFETY: Output, component name and component Variant are live
            // official values for this synchronous named assignment.
            unsafe {
                set_named(output, name.as_ptr(), value.as_ptr(), &mut valid);
            }
            if valid == 0 {
                self.destroy_constructed(output);
                return Err(());
            }
        }
        Ok(())
    }

    fn construct_integer_math(
        &self,
        output: GDExtensionVariantPtr,
        type_: GDExtensionVariantType,
        components: &[(&StaticStringName, i32)],
    ) -> Result<(), ()> {
        let (constructor, destructor, from_type) = self.math_codec(type_).ok_or(())?;
        let builtin = OwnedBuiltin::new(constructor, destructor).ok_or(())?;
        let from_type = from_type.ok_or(())?;
        let set_named = self.interface.variant_set_named.ok_or(())?;
        // SAFETY: `builtin` owns a default-constructed value of `type_`; the
        // matching official converter initializes the Variant output.
        unsafe { from_type(output, builtin.as_ptr().cast_mut()) };

        for (name, component) in components {
            let value = OwnedVariant::from_abi(self, AbiValueV1::from_i64(i64::from(*component)))
                .expect("integer Variant constructor was cached");
            let mut valid = 0;
            // SAFETY: Output, component name and component Variant are live
            // official values for this synchronous named assignment.
            unsafe { set_named(output, name.as_ptr(), value.as_ptr(), &mut valid) };
            if valid == 0 {
                self.destroy_constructed(output);
                return Err(());
            }
        }
        Ok(())
    }

    fn math_codec(
        &self,
        type_: GDExtensionVariantType,
    ) -> Option<(
        GDExtensionPtrConstructor,
        GDExtensionPtrDestructor,
        GDExtensionVariantFromTypeConstructorFunc,
    )> {
        match type_ {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2 => {
                Some((self.vector2_new, self.vector2_drop, self.vector2_from))
            }
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I => {
                Some((self.vector2i_new, self.vector2i_drop, self.vector2i_from))
            }
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3 => {
                Some((self.vector3_new, self.vector3_drop, self.vector3_from))
            }
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I => {
                Some((self.vector3i_new, self.vector3i_drop, self.vector3i_from))
            }
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR => {
                Some((self.color_new, self.color_drop, self.color_from))
            }
            _ => None,
        }
    }

    fn destroy_constructed(&self, value: GDExtensionVariantPtr) {
        if let Some(destroy) = self.interface.variant_destroy {
            // SAFETY: Called only after a from-type constructor initialized
            // the output and before returning the construction failure.
            unsafe { destroy(value) };
        }
    }

    fn construct_with(
        output: GDExtensionVariantPtr,
        constructor: GDExtensionVariantFromTypeConstructorFunc,
        value: *const core::ffi::c_void,
    ) -> Result<(), ()> {
        let constructor = constructor.ok_or(())?;
        // SAFETY: The caller supplies uninitialized Variant storage and a
        // matching live builtin value.
        unsafe { constructor(output, value.cast_mut()) };
        Ok(())
    }

    fn replace_with_nil(&self, output: GDExtensionVariantPtr) -> Result<(), ()> {
        let destroy = self.interface.variant_destroy.ok_or(())?;
        let construct = self.interface.variant_new_nil.ok_or(())?;
        // SAFETY: ScriptInstance supplies initialized Variant storage; it is
        // destroyed and reconstructed as Nil before returning to Godot.
        unsafe {
            destroy(output);
            construct(output);
        }
        Ok(())
    }
}

fn borrow_math_backing(
    backing: &mut Vec<Box<[f32]>>,
    type_: AbiValueType,
    components: &[f32],
) -> AbiValueV1 {
    backing.push(components.to_vec().into_boxed_slice());
    AbiValueV1::from_borrowed_f32_components(
        type_,
        backing
            .last()
            .expect("math backing was just appended")
            .as_ref(),
    )
}

fn abi_f32_components<const N: usize>(
    value: AbiValueV1,
    expected: AbiValueType,
) -> Result<[f32; N], ()> {
    let (pointer, length) = value.byte_range(expected).ok_or(())?;
    if length != N * core::mem::size_of::<f32>() {
        return Err(());
    }
    // SAFETY: The retained module or Host owner keeps this exact bounded
    // range alive through the synchronous Variant conversion.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    Ok(core::array::from_fn(|index| {
        let offset = index * 4;
        f32::from_ne_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("f32 byte width"),
        )
    }))
}

const fn dynamic_variant_type(value: AbiValueType) -> GDExtensionVariantType {
    match value {
        AbiValueType::ARRAY => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        AbiValueType::DICTIONARY => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        // A Variant accepts multiple concrete Godot types. NIL is the safest
        // public fallback for Godot's call-error field when recursive payload
        // encoding fails for reasons other than a top-level type mismatch.
        _ => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
    }
}

impl OwnedVariant {
    pub(crate) const fn interface(&self) -> EngineInterface {
        self.interface
    }

    pub(crate) fn from_abi(codec: &VariantCodec, value: AbiValueV1) -> Result<Self, ()> {
        Self::from_abi_with_context(codec, value, None, None)
    }

    pub(crate) fn from_abi_with_context(
        codec: &VariantCodec,
        value: AbiValueV1,
        typed_array_element: Option<&str>,
        context: Option<&EngineCallContext>,
    ) -> Result<Self, ()> {
        let mut variant = Self::uninitialized(codec.interface);
        codec.construct_with_context(value, variant.as_mut_ptr(), typed_array_element, context)?;
        variant.mark_initialized();
        Ok(variant)
    }

    pub(crate) fn from_string_name(
        codec: &VariantCodec,
        value: &OwnedStringName,
    ) -> Result<Self, ()> {
        let constructor = codec.string_name_from.ok_or(())?;
        let mut variant = Self::uninitialized(codec.interface);
        // SAFETY: `value` owns a live StringName matching the cached official
        // from-type Variant constructor.
        unsafe { constructor(variant.as_mut_ptr(), value.as_ptr().cast_mut()) };
        variant.mark_initialized();
        Ok(variant)
    }

    pub(crate) fn uninitialized(interface: EngineInterface) -> Self {
        Self {
            interface,
            storage: VariantStorage([0; 40]),
            initialized: false,
        }
    }

    pub(crate) fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstVariantPtr {
        self.storage.0.as_ptr().cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionVariantPtr {
        self.storage.0.as_mut_ptr().cast()
    }
}

impl OwnedBuiltin {
    fn new(
        constructor: GDExtensionPtrConstructor,
        destroy: GDExtensionPtrDestructor,
    ) -> Option<Self> {
        let constructor = constructor?;
        let mut value = Self {
            storage: BuiltinStorage([0; 32]),
            destroy,
        };
        // SAFETY: Storage is large and aligned enough for all supported vector
        // and Color values in every official Godot 4.4 configuration.
        unsafe { constructor(value.as_mut_ptr(), core::ptr::null()) };
        Some(value)
    }

    fn as_ptr(&self) -> *const core::ffi::c_void {
        self.storage.0.as_ptr().cast()
    }

    fn as_mut_ptr(&mut self) -> *mut core::ffi::c_void {
        self.storage.0.as_mut_ptr().cast()
    }
}

impl Drop for OwnedBuiltin {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            // SAFETY: This wrapper owns one default-constructed builtin and
            // releases it exactly once.
            unsafe { destroy(self.as_mut_ptr()) };
        }
    }
}

impl Drop for OwnedVariant {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.interface.variant_destroy {
                // SAFETY: `initialized` is set only after placement
                // construction, and this wrapper destroys the temporary
                // exactly once.
                unsafe { destroy(self.as_mut_ptr()) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_storage_covers_all_official_math_configurations() {
        assert_eq!(core::mem::size_of::<BuiltinStorage>(), 32);
        assert_eq!(core::mem::align_of::<BuiltinStorage>(), 8);
        for summary in godot_api::api_snapshot::BUILTIN_SIZES {
            if matches!(
                summary.name,
                "Vector2" | "Vector2i" | "Vector3" | "Vector3i" | "Color"
            ) {
                assert!(
                    summary.size <= core::mem::size_of::<BuiltinStorage>() as u64,
                    "{} {} exceeds Host math storage",
                    summary.configuration,
                    summary.name
                );
            }
        }
    }

    #[test]
    fn rid_storage_is_exactly_eight_bytes_in_every_official_configuration() {
        let sizes = godot_api::api_snapshot::BUILTIN_SIZES
            .iter()
            .filter(|summary| summary.name == "RID")
            .collect::<Vec<_>>();
        assert_eq!(sizes.len(), 4);
        for summary in sizes {
            assert_eq!(
                summary.size, 8,
                "{} RID storage changed",
                summary.configuration
            );
        }
    }

    #[test]
    fn four_component_transport_matches_standard_precision_layouts() {
        let architecture = if usize::BITS == 64 {
            "float_64"
        } else {
            "float_32"
        };
        let values = godot_api::api_snapshot::BUILTIN_SIZES
            .iter()
            .filter(|summary| {
                summary.configuration == architecture
                    && matches!(
                        summary.name,
                        "Rect2" | "Rect2i" | "Quaternion" | "Plane" | "Vector4" | "Vector4i"
                    )
            })
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 6);
        for summary in values {
            assert_eq!(
                summary.size, 16,
                "{} {} changed its fixed transport layout",
                summary.configuration, summary.name
            );
        }
    }

    #[test]
    fn matrix_transport_matches_standard_precision_layouts() {
        let architecture = if usize::BITS == 64 {
            "float_64"
        } else {
            "float_32"
        };
        let expected = [
            ("Transform2D", 24),
            ("AABB", 24),
            ("Basis", 36),
            ("Transform3D", 48),
            ("Projection", 64),
        ];
        for (name, size) in expected {
            let summary = godot_api::api_snapshot::BUILTIN_SIZES
                .iter()
                .find(|summary| summary.configuration == architecture && summary.name == name)
                .expect("authenticated matrix layout");
            assert_eq!(
                summary.size, size,
                "{} {} changed its fixed transport layout",
                summary.configuration, summary.name
            );
        }
    }
}
