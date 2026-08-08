use core::ptr;

use godot_api::abi::{
    ABI_DYNAMIC_MAX_ELEMENTS, ABI_GODOT_VALUE_TYPED_ARRAY, AbiGodotValueSpecV1, AbiPtrcallType,
    AbiValueType, AbiValueV1, validate_dynamic_value,
};

use super::engine_call::{NativeGodotRefToken, NativeTextValue};
use super::packed_array::NativePackedArray;
use super::runtime::Interface;
use super::sys;
use super::value::GodotStringName;
use crate::engine::{GodotRef, Object, ObjectRef, RefCounted};
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
use crate::string_name::StringName;
use crate::variant::{Array, Dictionary, Variant, VariantKind};

const SIZE_HASH: i64 = 3_173_160_232;
const RESIZE_HASH: i64 = 848_867_239;
const KEYS_HASH: i64 = 4_144_163_970;
const MAX_NESTING_DEPTH: usize = 64;

#[repr(C, align(8))]
struct VariantStorage([u8; 40]);

pub(super) struct NativeVariant {
    interface: Interface,
    storage: VariantStorage,
    initialized: bool,
}

impl NativeVariant {
    pub(super) fn uninitialized(interface: Interface) -> Self {
        Self {
            interface,
            storage: VariantStorage([0; 40]),
            initialized: false,
        }
    }

    fn nil(interface: Interface) -> Self {
        let mut value = Self::uninitialized(interface);
        // SAFETY: Storage is aligned and large enough for every authenticated
        // Variant layout.
        unsafe { (interface.variant_new_nil)(value.as_mut_ptr()) };
        value.initialized = true;
        value
    }

    pub(super) fn copy_from(interface: Interface, value: sys::GDExtensionConstVariantPtr) -> Self {
        let mut result = Self::uninitialized(interface);
        // SAFETY: Source is a live Variant and destination is uninitialized
        // aligned storage.
        unsafe { (interface.variant_new_copy)(result.as_mut_ptr(), value) };
        result.initialized = true;
        result
    }

    pub(super) fn from_raw(
        interface: Interface,
        variant_type: sys::GDExtensionVariantType,
        value: sys::GDExtensionConstTypePtr,
    ) -> EngineResult<Self> {
        // SAFETY: The concrete type and source storage are selected together
        // from one Rust Variant branch.
        let constructor = unsafe { (interface.get_variant_from_type_constructor)(variant_type) }
            .ok_or_else(|| {
                EngineError::unavailable("Godot omitted a Native type-to-Variant constructor")
            })?;
        let mut result = Self::uninitialized(interface);
        // SAFETY: Destination is uninitialized Variant storage and the source
        // has the exact concrete type requested above.
        unsafe { constructor(result.as_mut_ptr(), value.cast_mut()) };
        result.initialized = true;
        Ok(result)
    }

    pub(super) fn from_rust(
        interface: Interface,
        value: &Variant,
        depth: usize,
    ) -> EngineResult<Self> {
        check_depth(depth)?;
        match value.kind() {
            VariantKind::Nil => Ok(Self::nil(interface)),
            VariantKind::Bool(value) => Self::from_raw(
                interface,
                variant_type_bool(),
                ptr::from_ref(&u8::from(value)).cast(),
            ),
            VariantKind::Int(value) => {
                Self::from_raw(interface, variant_type_int(), ptr::from_ref(&value).cast())
            }
            VariantKind::Float(value) => Self::from_raw(
                interface,
                variant_type_float(),
                ptr::from_ref(&value).cast(),
            ),
            VariantKind::String(value) => {
                let value = NativeTextValue::from_rust(interface, AbiPtrcallType::STRING, value)?;
                Self::from_raw(interface, variant_type_string(), value.as_const_ptr())
            }
            VariantKind::StringName(value) => {
                let value = NativeTextValue::from_rust(
                    interface,
                    AbiPtrcallType::STRING_NAME,
                    value.as_str(),
                )?;
                Self::from_raw(interface, variant_type_string_name(), value.as_const_ptr())
            }
            VariantKind::NodePath(value) => {
                let value = NativeTextValue::from_rust(
                    interface,
                    AbiPtrcallType::NODE_PATH,
                    value.as_str(),
                )?;
                Self::from_raw(interface, variant_type_node_path(), value.as_const_ptr())
            }
            VariantKind::Object(value) => {
                let object = resolve_object(interface, value.instance_id())?;
                Self::from_raw(
                    interface,
                    variant_type_object(),
                    ptr::from_ref(&object).cast(),
                )
            }
            VariantKind::Vector2(value) => from_copy(interface, variant_type_vector2(), &value),
            VariantKind::Vector2i(value) => from_copy(interface, variant_type_vector2i(), &value),
            VariantKind::Vector3(value) => from_copy(interface, variant_type_vector3(), &value),
            VariantKind::Vector3i(value) => from_copy(interface, variant_type_vector3i(), &value),
            VariantKind::Vector4(value) => from_copy(interface, variant_type_vector4(), &value),
            VariantKind::Vector4i(value) => from_copy(interface, variant_type_vector4i(), &value),
            VariantKind::Rect2(value) => from_copy(interface, variant_type_rect2(), &value),
            VariantKind::Rect2i(value) => from_copy(interface, variant_type_rect2i(), &value),
            VariantKind::Quaternion(value) => {
                from_copy(interface, variant_type_quaternion(), &value)
            }
            VariantKind::Plane(value) => from_copy(interface, variant_type_plane(), &value),
            VariantKind::Transform2D(value) => {
                from_copy(interface, variant_type_transform2d(), value)
            }
            VariantKind::Aabb(value) => from_copy(interface, variant_type_aabb(), value),
            VariantKind::Basis(value) => from_copy(interface, variant_type_basis(), value),
            VariantKind::Transform3D(value) => {
                from_copy(interface, variant_type_transform3d(), value)
            }
            VariantKind::Projection(value) => {
                from_copy(interface, variant_type_projection(), value)
            }
            VariantKind::Color(value) => from_copy(interface, variant_type_color(), &value),
            VariantKind::Rid(value) => {
                let value = value.id();
                Self::from_raw(interface, variant_type_rid(), ptr::from_ref(&value).cast())
            }
            VariantKind::PackedByteArray(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_BYTE_ARRAY,
                AbiValueV1::from_borrowed_bytes(AbiValueType::PACKED_BYTE_ARRAY, value.__bytes()),
            ),
            VariantKind::PackedInt32Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_INT32_ARRAY,
                AbiValueV1::from_borrowed_bytes(AbiValueType::PACKED_INT32_ARRAY, value.__bytes()),
            ),
            VariantKind::PackedInt64Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_INT64_ARRAY,
                AbiValueV1::from_borrowed_bytes(AbiValueType::PACKED_INT64_ARRAY, value.__bytes()),
            ),
            VariantKind::PackedFloat32Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_FLOAT32_ARRAY,
                AbiValueV1::from_borrowed_bytes(
                    AbiValueType::PACKED_FLOAT32_ARRAY,
                    value.__bytes(),
                ),
            ),
            VariantKind::PackedFloat64Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_FLOAT64_ARRAY,
                AbiValueV1::from_borrowed_bytes(
                    AbiValueType::PACKED_FLOAT64_ARRAY,
                    value.__bytes(),
                ),
            ),
            VariantKind::PackedStringArray(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_STRING_ARRAY,
                AbiValueV1::from_borrowed_bytes(AbiValueType::PACKED_STRING_ARRAY, value.__bytes()),
            ),
            VariantKind::PackedVector2Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_VECTOR2_ARRAY,
                AbiValueV1::from_borrowed_bytes(
                    AbiValueType::PACKED_VECTOR2_ARRAY,
                    value.__bytes(),
                ),
            ),
            VariantKind::PackedVector3Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_VECTOR3_ARRAY,
                AbiValueV1::from_borrowed_bytes(
                    AbiValueType::PACKED_VECTOR3_ARRAY,
                    value.__bytes(),
                ),
            ),
            VariantKind::PackedColorArray(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_COLOR_ARRAY,
                AbiValueV1::from_borrowed_bytes(AbiValueType::PACKED_COLOR_ARRAY, value.__bytes()),
            ),
            VariantKind::PackedVector4Array(value) => packed_variant(
                interface,
                AbiPtrcallType::PACKED_VECTOR4_ARRAY,
                AbiValueV1::from_borrowed_bytes(
                    AbiValueType::PACKED_VECTOR4_ARRAY,
                    value.__bytes(),
                ),
            ),
            VariantKind::Array(value) => {
                let value = NativeArray::from_rust(interface, value, None, depth + 1)?;
                Self::from_raw(interface, variant_type_array(), value.as_const_ptr())
            }
            VariantKind::Dictionary(value) => {
                let value = NativeDictionary::from_rust(interface, value, depth + 1)?;
                Self::from_raw(interface, variant_type_dictionary(), value.as_const_ptr())
            }
            VariantKind::Signal(value) => {
                let bytes = value.__bytes().map_err(|error| {
                    EngineError::invalid_argument(format!(
                        "Native Signal could not be encoded: {}",
                        error.message()
                    ))
                })?;
                let value = super::signal_value::NativeSignal::from_argument(
                    interface,
                    AbiValueV1::from_borrowed_bytes(AbiValueType::SIGNAL, bytes),
                )?;
                value.to_variant()
            }
            VariantKind::Callable(value) => {
                let bytes = value.__bytes().map_err(|error| {
                    EngineError::invalid_argument(format!(
                        "Native Callable could not be encoded: {}",
                        error.message()
                    ))
                })?;
                let value = super::callable_value::NativeCallable::from_argument(
                    interface,
                    AbiValueV1::from_borrowed_bytes(AbiValueType::CALLABLE, bytes),
                )?;
                value.to_variant()
            }
        }
    }

    pub(super) fn to_rust(&self, depth: usize) -> EngineResult<Variant> {
        check_depth(depth)?;
        // SAFETY: This wrapper owns an initialized Variant.
        let variant_type = unsafe { (self.interface.variant_get_type)(self.as_const_ptr()) };
        if variant_type == variant_type_nil() {
            return Ok(Variant::nil());
        }
        if variant_type == variant_type_bool() {
            let value: u8 = self.to_copy(variant_type)?;
            return match value {
                0 => Ok(Variant::from(false)),
                1 => Ok(Variant::from(true)),
                _ => Err(EngineError::invalid_result(
                    "Godot returned a non-canonical Variant bool",
                )),
            };
        }
        if variant_type == variant_type_int() {
            return Ok(Variant::from(self.to_copy::<i64>(variant_type)?));
        }
        if variant_type == variant_type_float() {
            return Ok(Variant::from(self.to_copy::<f64>(variant_type)?));
        }
        if variant_type == variant_type_string() {
            return self.to_text(AbiPtrcallType::STRING).map(Variant::from);
        }
        if variant_type == variant_type_string_name() {
            return self
                .to_text(AbiPtrcallType::STRING_NAME)
                .map(StringName::from)
                .map(Variant::from);
        }
        if variant_type == variant_type_node_path() {
            return self
                .to_text(AbiPtrcallType::NODE_PATH)
                .map(NodePath::from)
                .map(Variant::from);
        }
        if variant_type == variant_type_object() {
            return self.object_to_rust();
        }
        macro_rules! copy_variant {
            ($variant_type:expr, $type:ty) => {
                if variant_type == $variant_type {
                    return Ok(Variant::from(self.to_copy::<$type>(variant_type)?));
                }
            };
        }
        copy_variant!(variant_type_vector2(), Vector2);
        copy_variant!(variant_type_vector2i(), Vector2i);
        copy_variant!(variant_type_vector3(), Vector3);
        copy_variant!(variant_type_vector3i(), Vector3i);
        copy_variant!(variant_type_vector4(), Vector4);
        copy_variant!(variant_type_vector4i(), Vector4i);
        copy_variant!(variant_type_rect2(), Rect2);
        copy_variant!(variant_type_rect2i(), Rect2i);
        copy_variant!(variant_type_quaternion(), Quaternion);
        copy_variant!(variant_type_plane(), Plane);
        copy_variant!(variant_type_transform2d(), Transform2D);
        copy_variant!(variant_type_aabb(), Aabb);
        copy_variant!(variant_type_basis(), Basis);
        copy_variant!(variant_type_transform3d(), Transform3D);
        copy_variant!(variant_type_projection(), Projection);
        copy_variant!(variant_type_color(), Color);
        if variant_type == variant_type_rid() {
            return Ok(Variant::from(Rid::from_raw(
                self.to_copy::<u64>(variant_type)?,
            )));
        }
        if let Some((ptrcall_type, value_type)) = packed_types_from_variant(variant_type) {
            let mut packed = NativePackedArray::output(self.interface, ptrcall_type)?;
            self.to_raw(variant_type, packed.as_mut_ptr())?;
            let bytes = packed.to_bytes()?;
            return packed_rust_variant(value_type, &bytes);
        }
        if variant_type == variant_type_array() {
            let array = NativeArray::from_variant(self)?;
            return Ok(Variant::from(array.to_rust(depth + 1)?));
        }
        if variant_type == variant_type_dictionary() {
            let dictionary = NativeDictionary::from_variant(self)?;
            return Ok(Variant::from(dictionary.to_rust(depth + 1)?));
        }
        if variant_type == variant_type_signal() {
            let signal = super::signal_value::NativeSignal::from_variant(self)?;
            let bytes = signal.to_bytes()?;
            return crate::signal::Signal::__from_bytes(&bytes)
                .map(Variant::from)
                .ok_or_else(|| {
                    EngineError::invalid_result("Godot returned an invalid Native Signal")
                });
        }
        if variant_type == variant_type_callable() {
            let callable = super::callable_value::NativeCallable::from_variant(self)?;
            return callable.into_rust().map(Variant::from);
        }
        Err(EngineError::unavailable(
            "Godot returned a Native Variant type that is not yet convertible",
        ))
    }

    pub(super) const fn interface(&self) -> Interface {
        self.interface
    }

    pub(super) fn to_raw_value(
        &self,
        variant_type: sys::GDExtensionVariantType,
        output: sys::GDExtensionTypePtr,
    ) -> EngineResult<()> {
        self.to_raw(variant_type, output)
    }

    pub(super) fn copy_to_variant(
        &self,
        output: sys::GDExtensionUninitializedVariantPtr,
    ) -> EngineResult<()> {
        if output.is_null() {
            return Err(EngineError::invalid_argument(
                "Native Variant output pointer is null",
            ));
        }
        // SAFETY: `self` owns one initialized Variant and the caller supplies
        // the uninitialized return slot declared by ClassDB metadata.
        unsafe { (self.interface.variant_new_copy)(output, self.as_const_ptr()) };
        Ok(())
    }

    fn object_to_rust(&self) -> EngineResult<Variant> {
        // SAFETY: Godot accepts any live Variant and returns zero for null.
        let object_id =
            unsafe { (self.interface.variant_get_object_instance_id)(self.as_const_ptr()) };
        if object_id == 0 {
            return Ok(Variant::from(ObjectRef::<Object>::unresolved()));
        }
        let object = resolve_object(self.interface, object_id)?;
        if object_is_refcounted(self.interface, object)? {
            let ownership = NativeGodotRefToken::from_object(self.interface, object);
            return Ok(Variant::from(GodotRef::<RefCounted>::from_native_parts(
                object_id, ownership,
            )));
        }
        Ok(Variant::from(ObjectRef::<Object>::__from_instance_id(
            object_id,
        )))
    }

    fn to_text(&self, ptrcall_type: AbiPtrcallType) -> EngineResult<String> {
        let mut value = NativeTextValue::output(self.interface, ptrcall_type)?;
        self.to_raw(
            match ptrcall_type {
                AbiPtrcallType::STRING => variant_type_string(),
                AbiPtrcallType::STRING_NAME => variant_type_string_name(),
                AbiPtrcallType::NODE_PATH => variant_type_node_path(),
                _ => {
                    return Err(EngineError::invalid_result(
                        "Native Variant text conversion received a non-text type",
                    ));
                }
            },
            value.as_mut_ptr(),
        )?;
        value.mark_initialized();
        value.to_rust_string()
    }

    fn to_copy<T: Copy + Default>(
        &self,
        variant_type: sys::GDExtensionVariantType,
    ) -> EngineResult<T> {
        let mut value = T::default();
        self.to_raw(variant_type, ptr::from_mut(&mut value).cast())?;
        Ok(value)
    }

    fn to_raw(
        &self,
        variant_type: sys::GDExtensionVariantType,
        output: sys::GDExtensionTypePtr,
    ) -> EngineResult<()> {
        // SAFETY: The caller matched the concrete Variant type and supplies
        // the corresponding uninitialized raw storage.
        let constructor = unsafe { (self.interface.get_variant_to_type_constructor)(variant_type) }
            .ok_or_else(|| {
                EngineError::unavailable("Godot omitted a Native Variant-to-type constructor")
            })?;
        // SAFETY: See the checked concrete type contract above.
        unsafe { constructor(output, self.as_const_ptr().cast_mut()) };
        Ok(())
    }

    pub(super) fn as_const_ptr(&self) -> sys::GDExtensionConstVariantPtr {
        self.storage.0.as_ptr().cast()
    }

    pub(super) fn as_mut_ptr(&mut self) -> sys::GDExtensionVariantPtr {
        self.storage.0.as_mut_ptr().cast()
    }

    pub(super) fn mark_initialized(&mut self) {
        self.initialized = true;
    }
}

impl Drop for NativeVariant {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: This wrapper owns one initialized Variant and drops once.
            unsafe { (self.interface.variant_destroy)(self.as_mut_ptr()) };
        }
    }
}

pub(super) enum NativeDynamicValue {
    Variant(NativeVariant),
    Array(NativeArray),
    Dictionary(NativeDictionary),
}

impl NativeDynamicValue {
    pub(super) fn from_argument(
        interface: Interface,
        contract: &AbiGodotValueSpecV1,
        value: AbiValueV1,
    ) -> EngineResult<Self> {
        if value.type_ != contract.value_type
            || value.reserved_flags != 0
            || !validate_dynamic_value(value.type_, value_bytes(value)?)
        {
            return Err(EngineError::invalid_argument(
                "Native dynamic argument violates its generated contract",
            ));
        }
        let rust = Variant::__from_native_bytes(value_bytes(value)?).ok_or_else(|| {
            EngineError::invalid_argument("Native dynamic argument could not be decoded")
        })?;
        match contract.ptrcall_type {
            AbiPtrcallType::VARIANT => {
                NativeVariant::from_rust(interface, &rust, 0).map(Self::Variant)
            }
            AbiPtrcallType::ARRAY => {
                let VariantKind::Array(values) = rust.kind() else {
                    return Err(EngineError::invalid_argument(
                        "Native Array argument has a non-Array root",
                    ));
                };
                let typed = typed_array_element(contract)?;
                NativeArray::from_rust(interface, values, typed, 0).map(Self::Array)
            }
            AbiPtrcallType::DICTIONARY => {
                let VariantKind::Dictionary(values) = rust.kind() else {
                    return Err(EngineError::invalid_argument(
                        "Native Dictionary argument has a non-Dictionary root",
                    ));
                };
                NativeDictionary::from_rust(interface, values, 0).map(Self::Dictionary)
            }
            _ => Err(EngineError::invalid_argument(
                "Native dynamic storage received a non-dynamic contract",
            )),
        }
    }

    pub(super) fn output(interface: Interface, ptrcall_type: AbiPtrcallType) -> EngineResult<Self> {
        match ptrcall_type {
            AbiPtrcallType::VARIANT => Ok(Self::Variant(NativeVariant::nil(interface))),
            AbiPtrcallType::ARRAY => NativeArray::empty(interface).map(Self::Array),
            AbiPtrcallType::DICTIONARY => NativeDictionary::empty(interface).map(Self::Dictionary),
            _ => Err(EngineError::invalid_argument(
                "Native dynamic output received a non-dynamic contract",
            )),
        }
    }

    pub(super) fn constructor_output(
        interface: Interface,
        ptrcall_type: AbiPtrcallType,
    ) -> EngineResult<Self> {
        match ptrcall_type {
            AbiPtrcallType::VARIANT => Ok(Self::Variant(NativeVariant::uninitialized(interface))),
            AbiPtrcallType::ARRAY => NativeArray::uninitialized(interface).map(Self::Array),
            AbiPtrcallType::DICTIONARY => {
                NativeDictionary::uninitialized(interface).map(Self::Dictionary)
            }
            _ => Err(EngineError::invalid_argument(
                "Native dynamic constructor output received a non-dynamic contract",
            )),
        }
    }

    pub(super) fn mark_initialized(&mut self) {
        match self {
            Self::Variant(value) => value.mark_initialized(),
            Self::Array(value) => value.mark_initialized(),
            Self::Dictionary(value) => value.mark_initialized(),
        }
    }

    pub(super) fn as_const_ptr(&self) -> sys::GDExtensionConstTypePtr {
        match self {
            Self::Variant(value) => value.as_const_ptr().cast(),
            Self::Array(value) => value.as_const_ptr(),
            Self::Dictionary(value) => value.as_const_ptr(),
        }
    }

    pub(super) fn as_mut_ptr(&mut self) -> sys::GDExtensionTypePtr {
        match self {
            Self::Variant(value) => value.as_mut_ptr().cast(),
            Self::Array(value) => value.as_mut_ptr(),
            Self::Dictionary(value) => value.as_mut_ptr(),
        }
    }

    pub(super) fn to_rust(&self) -> EngineResult<Variant> {
        match self {
            Self::Variant(value) => value.to_rust(0),
            Self::Array(value) => value.to_rust(0).map(Variant::from),
            Self::Dictionary(value) => value.to_rust(0).map(Variant::from),
        }
    }

    pub(super) fn to_native_variant(&self) -> EngineResult<NativeVariant> {
        match self {
            Self::Variant(value) => Ok(NativeVariant::copy_from(
                value.interface,
                value.as_const_ptr(),
            )),
            Self::Array(value) => {
                NativeVariant::from_raw(value.interface, variant_type_array(), value.as_const_ptr())
            }
            Self::Dictionary(value) => NativeVariant::from_raw(
                value.interface,
                variant_type_dictionary(),
                value.as_const_ptr(),
            ),
        }
    }
}

pub(super) struct NativeArray {
    interface: Interface,
    storage: [usize; 2],
    initialized: bool,
    destroy: sys::GDExtensionPtrDestructor,
    size: sys::GDExtensionPtrBuiltInMethod,
    resize: sys::GDExtensionPtrBuiltInMethod,
}

impl NativeArray {
    fn empty(interface: Interface) -> EngineResult<Self> {
        let mut value = Self::uninitialized(interface)?;
        construct_default(interface, variant_type_array(), value.as_mut_ptr())?;
        value.initialized = true;
        Ok(value)
    }

    fn uninitialized(interface: Interface) -> EngineResult<Self> {
        Ok(Self {
            interface,
            storage: [0; 2],
            initialized: false,
            destroy: destructor(interface, variant_type_array())?,
            size: builtin_method(interface, variant_type_array(), "size", SIZE_HASH)?,
            resize: builtin_method(interface, variant_type_array(), "resize", RESIZE_HASH)?,
        })
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn from_rust(
        interface: Interface,
        values: &Array,
        typed_element: Option<&str>,
        depth: usize,
    ) -> EngineResult<Self> {
        check_depth(depth)?;
        if values.len() > ABI_DYNAMIC_MAX_ELEMENTS {
            return Err(EngineError::invalid_argument(
                "Native Array exceeds the element boundary",
            ));
        }
        let mut result = Self::empty(interface)?;
        if let Some(element) = typed_element {
            result.set_typed(element)?;
        }
        result.resize(values.len())?;
        for (index, value) in values.iter().enumerate() {
            let value = NativeVariant::from_rust(interface, value, depth + 1)?;
            result.set(index, &value)?;
        }
        Ok(result)
    }

    fn from_variant(value: &NativeVariant) -> EngineResult<Self> {
        let mut result = Self::empty(value.interface)?;
        value.to_raw(variant_type_array(), result.as_mut_ptr())?;
        Ok(result)
    }

    fn to_rust(&self, depth: usize) -> EngineResult<Array> {
        check_depth(depth)?;
        let count = self.len()?;
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let value = self.get(index)?;
            values.push(borrowed_variant_to_rust(self.interface, value, depth + 1)?);
        }
        Ok(Array::from_vec(values))
    }

    fn set_typed(&mut self, element: &str) -> EngineResult<()> {
        let (variant_type, class_name) = match builtin_variant_type(element) {
            Some(value) => (value, ""),
            None => {
                let name = GodotStringName::new(&self.interface, element)
                    .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
                // SAFETY: The generated class spelling is an initialized
                // StringName for this lookup.
                if unsafe { (self.interface.classdb_get_class_tag)(name.as_ptr()) }.is_null() {
                    return Err(EngineError::invalid_argument(
                        "Native typed Array references an unavailable Godot class",
                    ));
                }
                (variant_type_object(), element)
            }
        };
        let class_name = GodotStringName::new(&self.interface, class_name)
            .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
        let script = NativeVariant::nil(self.interface);
        // SAFETY: Array and metadata are initialized official values.
        unsafe {
            (self.interface.array_set_typed)(
                self.as_mut_ptr(),
                variant_type,
                class_name.as_ptr(),
                script.as_const_ptr(),
            );
        }
        Ok(())
    }

    fn len(&self) -> EngineResult<usize> {
        builtin_size(self.size, self.as_const_ptr(), "Array")
    }

    fn resize(&mut self, count: usize) -> EngineResult<()> {
        let count = i64::try_from(count)
            .map_err(|_| EngineError::invalid_argument("Native Array size is out of range"))?;
        let arguments = [ptr::from_ref(&count).cast()];
        let mut error = -1_i64;
        let resize = self
            .resize
            .ok_or_else(|| EngineError::unavailable("Native Array.resize is unavailable"))?;
        // SAFETY: Receiver is initialized and resize accepts one int64.
        unsafe {
            resize(
                self.as_mut_ptr(),
                arguments.as_ptr(),
                ptr::from_mut(&mut error).cast(),
                1,
            );
        }
        if error != 0 {
            return Err(EngineError::unavailable(
                "Godot could not resize a Native Array",
            ));
        }
        Ok(())
    }

    fn set(&mut self, index: usize, value: &NativeVariant) -> EngineResult<()> {
        let index = i64::try_from(index)
            .map_err(|_| EngineError::invalid_argument("Native Array index is out of range"))?;
        // SAFETY: The caller resized this same Array beyond `index`.
        let slot = unsafe { (self.interface.array_operator_index)(self.as_mut_ptr(), index) };
        replace_variant(self.interface, slot, value.as_const_ptr())
    }

    fn get(&self, index: usize) -> EngineResult<sys::GDExtensionConstVariantPtr> {
        let index = i64::try_from(index)
            .map_err(|_| EngineError::invalid_result("Native Array index is out of range"))?;
        // SAFETY: The index is below the size read from this same Array.
        let value =
            unsafe { (self.interface.array_operator_index_const)(self.as_const_ptr(), index) };
        (!value.is_null())
            .then_some(value.cast_const())
            .ok_or_else(|| EngineError::invalid_result("Godot returned a null Array element"))
    }

    fn as_const_ptr(&self) -> sys::GDExtensionConstTypePtr {
        ptr::from_ref(&self.storage).cast()
    }

    fn as_mut_ptr(&mut self) -> sys::GDExtensionTypePtr {
        ptr::from_mut(&mut self.storage).cast()
    }
}

impl Drop for NativeArray {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.destroy {
                // SAFETY: This wrapper owns one initialized Array.
                unsafe { destroy(self.as_mut_ptr()) };
            }
        }
    }
}

pub(super) struct NativeDictionary {
    interface: Interface,
    storage: [usize; 2],
    initialized: bool,
    destroy: sys::GDExtensionPtrDestructor,
}

impl NativeDictionary {
    fn empty(interface: Interface) -> EngineResult<Self> {
        let mut value = Self::uninitialized(interface)?;
        construct_default(interface, variant_type_dictionary(), value.as_mut_ptr())?;
        value.initialized = true;
        Ok(value)
    }

    fn uninitialized(interface: Interface) -> EngineResult<Self> {
        Ok(Self {
            interface,
            storage: [0; 2],
            initialized: false,
            destroy: destructor(interface, variant_type_dictionary())?,
        })
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn from_rust(interface: Interface, value: &Dictionary, depth: usize) -> EngineResult<Self> {
        check_depth(depth)?;
        if value.len() > ABI_DYNAMIC_MAX_ELEMENTS {
            return Err(EngineError::invalid_argument(
                "Native Dictionary exceeds the entry boundary",
            ));
        }
        let mut result = Self::empty(interface)?;
        for (key, value) in value.iter() {
            let key = NativeVariant::from_rust(interface, key, depth + 1)?;
            let value = NativeVariant::from_rust(interface, value, depth + 1)?;
            result.insert(&key, &value)?;
        }
        Ok(result)
    }

    fn from_variant(value: &NativeVariant) -> EngineResult<Self> {
        let mut result = Self::empty(value.interface)?;
        value.to_raw(variant_type_dictionary(), result.as_mut_ptr())?;
        Ok(result)
    }

    fn to_rust(&self, depth: usize) -> EngineResult<Dictionary> {
        check_depth(depth)?;
        let count = builtin_size(
            builtin_method(self.interface, variant_type_dictionary(), "size", SIZE_HASH)?,
            self.as_const_ptr(),
            "Dictionary",
        )?;
        if count > ABI_DYNAMIC_MAX_ELEMENTS {
            return Err(EngineError::invalid_result(
                "Godot returned a Dictionary beyond the entry boundary",
            ));
        }
        let keys_method =
            builtin_method(self.interface, variant_type_dictionary(), "keys", KEYS_HASH)?
                .ok_or_else(|| EngineError::unavailable("Native Dictionary.keys is unavailable"))?;
        let mut keys = NativeArray::empty(self.interface)?;
        // SAFETY: Receiver and Array result storage are initialized exact
        // builtin values.
        unsafe {
            keys_method(
                self.as_const_ptr().cast_mut(),
                ptr::null(),
                keys.as_mut_ptr(),
                0,
            );
        }
        if keys.len()? != count {
            return Err(EngineError::invalid_result(
                "Godot changed Dictionary keys while reading a Native result",
            ));
        }
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let key = keys.get(index)?;
            // SAFETY: Dictionary and key are live throughout this lookup.
            let value = unsafe {
                (self.interface.dictionary_operator_index_const)(self.as_const_ptr(), key)
            };
            if value.is_null() {
                return Err(EngineError::invalid_result(
                    "Godot returned a null Dictionary value",
                ));
            }
            entries.push((
                borrowed_variant_to_rust(self.interface, key, depth + 1)?,
                borrowed_variant_to_rust(self.interface, value.cast_const(), depth + 1)?,
            ));
        }
        Ok(Dictionary::from_entries(entries))
    }

    fn insert(&mut self, key: &NativeVariant, value: &NativeVariant) -> EngineResult<()> {
        // SAFETY: Dictionary and key are initialized official values.
        let slot = unsafe {
            (self.interface.dictionary_operator_index)(self.as_mut_ptr(), key.as_const_ptr())
        };
        replace_variant(self.interface, slot, value.as_const_ptr())
    }

    fn as_const_ptr(&self) -> sys::GDExtensionConstTypePtr {
        ptr::from_ref(&self.storage).cast()
    }

    fn as_mut_ptr(&mut self) -> sys::GDExtensionTypePtr {
        ptr::from_mut(&mut self.storage).cast()
    }
}

impl Drop for NativeDictionary {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.destroy {
                // SAFETY: This wrapper owns one initialized Dictionary.
                unsafe { destroy(self.as_mut_ptr()) };
            }
        }
    }
}

fn borrowed_variant_to_rust(
    interface: Interface,
    value: sys::GDExtensionConstVariantPtr,
    depth: usize,
) -> EngineResult<Variant> {
    let mut copy = NativeVariant::uninitialized(interface);
    // SAFETY: Source is a live borrowed Variant and destination is
    // uninitialized aligned storage.
    unsafe { (interface.variant_new_copy)(copy.as_mut_ptr(), value) };
    copy.initialized = true;
    copy.to_rust(depth)
}

fn replace_variant(
    interface: Interface,
    output: sys::GDExtensionVariantPtr,
    value: sys::GDExtensionConstVariantPtr,
) -> EngineResult<()> {
    if output.is_null() {
        return Err(EngineError::invalid_result(
            "Godot returned a null Native dynamic value slot",
        ));
    }
    // SAFETY: Output is an initialized container slot. It is destroyed and
    // immediately copy-constructed from a live Variant.
    unsafe {
        (interface.variant_destroy)(output);
        (interface.variant_new_copy)(output, value);
    }
    Ok(())
}

fn construct_default(
    interface: Interface,
    variant_type: sys::GDExtensionVariantType,
    output: sys::GDExtensionTypePtr,
) -> EngineResult<()> {
    // SAFETY: Constructor zero is the official default constructor.
    let constructor = unsafe { (interface.variant_get_ptr_constructor)(variant_type, 0) }
        .ok_or_else(|| EngineError::unavailable("Native builtin constructor is unavailable"))?;
    // SAFETY: Output points to uninitialized exact builtin storage.
    unsafe { constructor(output, ptr::null()) };
    Ok(())
}

fn destructor(
    interface: Interface,
    variant_type: sys::GDExtensionVariantType,
) -> EngineResult<sys::GDExtensionPtrDestructor> {
    // SAFETY: The selected official builtin owns one matching destructor.
    let destroy = unsafe { (interface.variant_get_ptr_destructor)(variant_type) };
    if destroy.is_none() {
        return Err(EngineError::unavailable(
            "Native builtin destructor is unavailable",
        ));
    }
    Ok(destroy)
}

fn builtin_method(
    interface: Interface,
    variant_type: sys::GDExtensionVariantType,
    name: &str,
    hash: i64,
) -> EngineResult<sys::GDExtensionPtrBuiltInMethod> {
    let name = GodotStringName::new(&interface, name)
        .map_err(|error| EngineError::invalid_result(error.to_string()))?;
    // SAFETY: Type, name and hash are authenticated generated API values.
    let method =
        unsafe { (interface.variant_get_ptr_builtin_method)(variant_type, name.as_ptr(), hash) };
    if method.is_none() {
        return Err(EngineError::unavailable(
            "Godot omitted a required Native builtin method",
        ));
    }
    Ok(method)
}

fn builtin_size(
    method: sys::GDExtensionPtrBuiltInMethod,
    value: sys::GDExtensionConstTypePtr,
    label: &str,
) -> EngineResult<usize> {
    let method =
        method.ok_or_else(|| EngineError::unavailable("Native builtin size is unavailable"))?;
    let mut count = 0_i64;
    // SAFETY: Receiver is initialized and size has no arguments.
    unsafe {
        method(
            value.cast_mut(),
            ptr::null(),
            ptr::from_mut(&mut count).cast(),
            0,
        );
    }
    usize::try_from(count)
        .map_err(|_| EngineError::invalid_result(format!("Godot returned an invalid {label} size")))
}

fn packed_variant(
    interface: Interface,
    ptrcall_type: AbiPtrcallType,
    value: AbiValueV1,
) -> EngineResult<NativeVariant> {
    let packed = NativePackedArray::from_argument(interface, ptrcall_type, value)?;
    let variant_type = packed_types_from_ptrcall(ptrcall_type)
        .map(|(variant_type, _)| variant_type)
        .ok_or_else(|| EngineError::invalid_argument("invalid packed Variant contract"))?;
    NativeVariant::from_raw(interface, variant_type, packed.as_const_ptr())
}

fn packed_rust_variant(value_type: AbiValueType, bytes: &[u8]) -> EngineResult<Variant> {
    macro_rules! packed {
        ($abi:ident, $type:ty) => {
            if value_type == AbiValueType::$abi {
                return <$type>::__from_bytes(bytes)
                    .map(Variant::from)
                    .ok_or_else(|| {
                        EngineError::invalid_result(
                            "Godot returned an invalid Native packed-array payload",
                        )
                    });
            }
        };
    }
    packed!(PACKED_BYTE_ARRAY, PackedByteArray);
    packed!(PACKED_INT32_ARRAY, PackedInt32Array);
    packed!(PACKED_INT64_ARRAY, PackedInt64Array);
    packed!(PACKED_FLOAT32_ARRAY, PackedFloat32Array);
    packed!(PACKED_FLOAT64_ARRAY, PackedFloat64Array);
    packed!(PACKED_STRING_ARRAY, PackedStringArray);
    packed!(PACKED_VECTOR2_ARRAY, PackedVector2Array);
    packed!(PACKED_VECTOR3_ARRAY, PackedVector3Array);
    packed!(PACKED_COLOR_ARRAY, PackedColorArray);
    packed!(PACKED_VECTOR4_ARRAY, PackedVector4Array);
    Err(EngineError::invalid_result(
        "Godot returned an unknown Native packed-array type",
    ))
}

fn packed_types_from_ptrcall(
    ptrcall_type: AbiPtrcallType,
) -> Option<(sys::GDExtensionVariantType, AbiValueType)> {
    Some(match ptrcall_type {
        AbiPtrcallType::PACKED_BYTE_ARRAY => (
            variant_type_packed_byte_array(),
            AbiValueType::PACKED_BYTE_ARRAY,
        ),
        AbiPtrcallType::PACKED_INT32_ARRAY => (
            variant_type_packed_int32_array(),
            AbiValueType::PACKED_INT32_ARRAY,
        ),
        AbiPtrcallType::PACKED_INT64_ARRAY => (
            variant_type_packed_int64_array(),
            AbiValueType::PACKED_INT64_ARRAY,
        ),
        AbiPtrcallType::PACKED_FLOAT32_ARRAY => (
            variant_type_packed_float32_array(),
            AbiValueType::PACKED_FLOAT32_ARRAY,
        ),
        AbiPtrcallType::PACKED_FLOAT64_ARRAY => (
            variant_type_packed_float64_array(),
            AbiValueType::PACKED_FLOAT64_ARRAY,
        ),
        AbiPtrcallType::PACKED_STRING_ARRAY => (
            variant_type_packed_string_array(),
            AbiValueType::PACKED_STRING_ARRAY,
        ),
        AbiPtrcallType::PACKED_VECTOR2_ARRAY => (
            variant_type_packed_vector2_array(),
            AbiValueType::PACKED_VECTOR2_ARRAY,
        ),
        AbiPtrcallType::PACKED_VECTOR3_ARRAY => (
            variant_type_packed_vector3_array(),
            AbiValueType::PACKED_VECTOR3_ARRAY,
        ),
        AbiPtrcallType::PACKED_COLOR_ARRAY => (
            variant_type_packed_color_array(),
            AbiValueType::PACKED_COLOR_ARRAY,
        ),
        AbiPtrcallType::PACKED_VECTOR4_ARRAY => (
            variant_type_packed_vector4_array(),
            AbiValueType::PACKED_VECTOR4_ARRAY,
        ),
        _ => return None,
    })
}

fn packed_types_from_variant(
    variant_type: sys::GDExtensionVariantType,
) -> Option<(AbiPtrcallType, AbiValueType)> {
    [
        AbiPtrcallType::PACKED_BYTE_ARRAY,
        AbiPtrcallType::PACKED_INT32_ARRAY,
        AbiPtrcallType::PACKED_INT64_ARRAY,
        AbiPtrcallType::PACKED_FLOAT32_ARRAY,
        AbiPtrcallType::PACKED_FLOAT64_ARRAY,
        AbiPtrcallType::PACKED_STRING_ARRAY,
        AbiPtrcallType::PACKED_VECTOR2_ARRAY,
        AbiPtrcallType::PACKED_VECTOR3_ARRAY,
        AbiPtrcallType::PACKED_COLOR_ARRAY,
        AbiPtrcallType::PACKED_VECTOR4_ARRAY,
    ]
    .into_iter()
    .find_map(|ptrcall_type| {
        let (candidate, value_type) = packed_types_from_ptrcall(ptrcall_type)?;
        (candidate == variant_type).then_some((ptrcall_type, value_type))
    })
}

fn typed_array_element(contract: &AbiGodotValueSpecV1) -> EngineResult<Option<&str>> {
    if contract.reserved_flags & !ABI_GODOT_VALUE_TYPED_ARRAY != 0 {
        return Err(EngineError::invalid_argument(
            "Native Array contract uses unknown metadata flags",
        ));
    }
    if contract.reserved_flags & ABI_GODOT_VALUE_TYPED_ARRAY == 0 {
        return Ok(None);
    }
    let length = contract.class_name.len;
    if length == 0 || contract.class_name.ptr.is_null() {
        return Err(EngineError::invalid_argument(
            "Native typed Array contract has no element type",
        ));
    }
    // SAFETY: Generated metadata has static storage and its length is fixed by
    // the authenticated code generator.
    let bytes = unsafe { core::slice::from_raw_parts(contract.class_name.ptr, length) };
    core::str::from_utf8(bytes)
        .map(Some)
        .map_err(|_| EngineError::invalid_argument("Native typed Array metadata is not UTF-8"))
}

fn value_bytes(value: AbiValueV1) -> EngineResult<&'static [u8]> {
    let (pointer, length) = value.byte_range(value.type_).ok_or_else(|| {
        EngineError::invalid_argument("Native dynamic value has an invalid range")
    })?;
    // SAFETY: This temporary view is used only during the synchronous engine
    // call. The `'static` spelling is internal and never escapes this module.
    Ok(unsafe { core::slice::from_raw_parts(pointer, length) })
}

fn resolve_object(interface: Interface, object_id: u64) -> EngineResult<sys::GDExtensionObjectPtr> {
    if object_id == 0 {
        return Ok(ptr::null_mut());
    }
    // SAFETY: Godot owns and synchronizes its instance-ID table.
    let object = unsafe { (interface.object_get_instance_from_id)(object_id) };
    (!object.is_null()).then_some(object).ok_or_else(|| {
        EngineError::stale_object(format!("Godot Object {object_id} no longer exists"))
    })
}

fn object_is_refcounted(
    interface: Interface,
    object: sys::GDExtensionObjectPtr,
) -> EngineResult<bool> {
    if object.is_null() {
        return Ok(false);
    }
    let class_name = GodotStringName::new(&interface, "RefCounted")
        .map_err(|error| EngineError::invalid_result(error.to_string()))?;
    // SAFETY: Class spelling and object pointer are live official values.
    let tag = unsafe { (interface.classdb_get_class_tag)(class_name.as_ptr()) };
    if tag.is_null() {
        return Err(EngineError::invalid_result(
            "Godot omitted the RefCounted ClassDB tag",
        ));
    }
    // SAFETY: Godot validates the object against its own ClassDB tag.
    Ok(!unsafe { (interface.object_cast_to)(object, tag) }.is_null())
}

fn check_depth(depth: usize) -> EngineResult<()> {
    if depth > MAX_NESTING_DEPTH {
        Err(EngineError::invalid_argument(
            "Native dynamic value exceeds the nesting-depth boundary",
        ))
    } else {
        Ok(())
    }
}

fn from_copy<T: Copy>(
    interface: Interface,
    variant_type: sys::GDExtensionVariantType,
    value: &T,
) -> EngineResult<NativeVariant> {
    NativeVariant::from_raw(interface, variant_type, ptr::from_ref(value).cast())
}

fn builtin_variant_type(name: &str) -> Option<sys::GDExtensionVariantType> {
    Some(match name {
        "Nil" | "Variant" => variant_type_nil(),
        "bool" => variant_type_bool(),
        "int" => variant_type_int(),
        "float" => variant_type_float(),
        "String" => variant_type_string(),
        "Vector2" => variant_type_vector2(),
        "Vector2i" => variant_type_vector2i(),
        "Rect2" => variant_type_rect2(),
        "Rect2i" => variant_type_rect2i(),
        "Vector3" => variant_type_vector3(),
        "Vector3i" => variant_type_vector3i(),
        "Transform2D" => variant_type_transform2d(),
        "Vector4" => variant_type_vector4(),
        "Vector4i" => variant_type_vector4i(),
        "Plane" => variant_type_plane(),
        "Quaternion" => variant_type_quaternion(),
        "AABB" => variant_type_aabb(),
        "Basis" => variant_type_basis(),
        "Transform3D" => variant_type_transform3d(),
        "Projection" => variant_type_projection(),
        "Color" => variant_type_color(),
        "StringName" => variant_type_string_name(),
        "NodePath" => variant_type_node_path(),
        "RID" => variant_type_rid(),
        "Object" => variant_type_object(),
        "Callable" => variant_type_callable(),
        "Signal" => variant_type_signal(),
        "Dictionary" => variant_type_dictionary(),
        "Array" => variant_type_array(),
        "PackedByteArray" => variant_type_packed_byte_array(),
        "PackedInt32Array" => variant_type_packed_int32_array(),
        "PackedInt64Array" => variant_type_packed_int64_array(),
        "PackedFloat32Array" => variant_type_packed_float32_array(),
        "PackedFloat64Array" => variant_type_packed_float64_array(),
        "PackedStringArray" => variant_type_packed_string_array(),
        "PackedVector2Array" => variant_type_packed_vector2_array(),
        "PackedVector3Array" => variant_type_packed_vector3_array(),
        "PackedColorArray" => variant_type_packed_color_array(),
        "PackedVector4Array" => variant_type_packed_vector4_array(),
        _ => return None,
    })
}

macro_rules! variant_types {
    ($($function:ident => $constant:ident),+ $(,)?) => {
        $(
            const fn $function() -> sys::GDExtensionVariantType {
                sys::GDExtensionVariantType::$constant
            }
        )+
    };
}

variant_types!(
    variant_type_nil => GDEXTENSION_VARIANT_TYPE_NIL,
    variant_type_bool => GDEXTENSION_VARIANT_TYPE_BOOL,
    variant_type_int => GDEXTENSION_VARIANT_TYPE_INT,
    variant_type_float => GDEXTENSION_VARIANT_TYPE_FLOAT,
    variant_type_string => GDEXTENSION_VARIANT_TYPE_STRING,
    variant_type_vector2 => GDEXTENSION_VARIANT_TYPE_VECTOR2,
    variant_type_vector2i => GDEXTENSION_VARIANT_TYPE_VECTOR2I,
    variant_type_rect2 => GDEXTENSION_VARIANT_TYPE_RECT2,
    variant_type_rect2i => GDEXTENSION_VARIANT_TYPE_RECT2I,
    variant_type_vector3 => GDEXTENSION_VARIANT_TYPE_VECTOR3,
    variant_type_vector3i => GDEXTENSION_VARIANT_TYPE_VECTOR3I,
    variant_type_transform2d => GDEXTENSION_VARIANT_TYPE_TRANSFORM2D,
    variant_type_vector4 => GDEXTENSION_VARIANT_TYPE_VECTOR4,
    variant_type_vector4i => GDEXTENSION_VARIANT_TYPE_VECTOR4I,
    variant_type_plane => GDEXTENSION_VARIANT_TYPE_PLANE,
    variant_type_quaternion => GDEXTENSION_VARIANT_TYPE_QUATERNION,
    variant_type_aabb => GDEXTENSION_VARIANT_TYPE_AABB,
    variant_type_basis => GDEXTENSION_VARIANT_TYPE_BASIS,
    variant_type_transform3d => GDEXTENSION_VARIANT_TYPE_TRANSFORM3D,
    variant_type_projection => GDEXTENSION_VARIANT_TYPE_PROJECTION,
    variant_type_color => GDEXTENSION_VARIANT_TYPE_COLOR,
    variant_type_string_name => GDEXTENSION_VARIANT_TYPE_STRING_NAME,
    variant_type_node_path => GDEXTENSION_VARIANT_TYPE_NODE_PATH,
    variant_type_rid => GDEXTENSION_VARIANT_TYPE_RID,
    variant_type_object => GDEXTENSION_VARIANT_TYPE_OBJECT,
    variant_type_callable => GDEXTENSION_VARIANT_TYPE_CALLABLE,
    variant_type_signal => GDEXTENSION_VARIANT_TYPE_SIGNAL,
    variant_type_dictionary => GDEXTENSION_VARIANT_TYPE_DICTIONARY,
    variant_type_array => GDEXTENSION_VARIANT_TYPE_ARRAY,
    variant_type_packed_byte_array => GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY,
    variant_type_packed_int32_array => GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY,
    variant_type_packed_int64_array => GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY,
    variant_type_packed_float32_array => GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY,
    variant_type_packed_float64_array => GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY,
    variant_type_packed_string_array => GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
    variant_type_packed_vector2_array => GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY,
    variant_type_packed_vector3_array => GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY,
    variant_type_packed_color_array => GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY,
    variant_type_packed_vector4_array => GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_portable_packed_type_maps_both_directions() {
        for ptrcall_type in [
            AbiPtrcallType::PACKED_BYTE_ARRAY,
            AbiPtrcallType::PACKED_INT32_ARRAY,
            AbiPtrcallType::PACKED_INT64_ARRAY,
            AbiPtrcallType::PACKED_FLOAT32_ARRAY,
            AbiPtrcallType::PACKED_FLOAT64_ARRAY,
            AbiPtrcallType::PACKED_STRING_ARRAY,
            AbiPtrcallType::PACKED_VECTOR2_ARRAY,
            AbiPtrcallType::PACKED_VECTOR3_ARRAY,
            AbiPtrcallType::PACKED_COLOR_ARRAY,
            AbiPtrcallType::PACKED_VECTOR4_ARRAY,
        ] {
            let (variant_type, value_type) =
                packed_types_from_ptrcall(ptrcall_type).expect("forward mapping");
            assert_eq!(
                packed_types_from_variant(variant_type),
                Some((ptrcall_type, value_type))
            );
        }
    }

    #[test]
    fn typed_array_builtins_cover_every_portable_dynamic_type() {
        for name in [
            "Nil",
            "bool",
            "int",
            "float",
            "String",
            "Vector2",
            "Vector3",
            "Color",
            "StringName",
            "NodePath",
            "RID",
            "Object",
            "Dictionary",
            "Array",
            "PackedByteArray",
            "PackedVector4Array",
        ] {
            assert!(builtin_variant_type(name).is_some(), "{name}");
        }
    }
}
