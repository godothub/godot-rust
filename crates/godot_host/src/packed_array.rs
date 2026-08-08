use core::mem;

use godot_api::abi::{ABI_VALUE_OWNED_BYTES, AbiStatus, AbiValueType, AbiValueV1};
use godot_api::{
    GDExtensionConstTypePtr, GDExtensionInterfaceFunctionPtr, GDExtensionPtrBuiltInMethod,
    GDExtensionPtrDestructor, GDExtensionTypePtr, GDExtensionVariantType,
};

use crate::engine_call::value::ValueError;
use crate::interface::EngineInterface;
use crate::string_name::StaticStringName;
use crate::value::{LocalGodotString, read_utf8_string};

const SIZE_HASH: i64 = 3_173_160_232;
const RESIZE_HASH: i64 = 848_867_239;
const PUSH_BACK_HASH: i64 = 816_187_996;
const MAX_PACKED_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKED_STRINGS: usize = 1_000_000;
const STRING_HEADER_BYTES: usize = core::mem::size_of::<u64>();

type MutableIndex = Option<unsafe extern "C" fn(GDExtensionTypePtr, i64) -> GDExtensionTypePtr>;
type ConstIndex =
    Option<unsafe extern "C" fn(GDExtensionConstTypePtr, i64) -> GDExtensionConstTypePtr>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackedArrayKind {
    Byte,
    Int32,
    Int64,
    Float32,
    Float64,
    String,
    Vector2,
    Vector3,
    Color,
    Vector4,
}

impl PackedArrayKind {
    pub(crate) fn from_value_type(value: AbiValueType) -> Option<Self> {
        match value {
            AbiValueType::PACKED_BYTE_ARRAY => Some(Self::Byte),
            AbiValueType::PACKED_INT32_ARRAY => Some(Self::Int32),
            AbiValueType::PACKED_INT64_ARRAY => Some(Self::Int64),
            AbiValueType::PACKED_FLOAT32_ARRAY => Some(Self::Float32),
            AbiValueType::PACKED_FLOAT64_ARRAY => Some(Self::Float64),
            AbiValueType::PACKED_STRING_ARRAY => Some(Self::String),
            AbiValueType::PACKED_VECTOR2_ARRAY => Some(Self::Vector2),
            AbiValueType::PACKED_VECTOR3_ARRAY => Some(Self::Vector3),
            AbiValueType::PACKED_COLOR_ARRAY => Some(Self::Color),
            AbiValueType::PACKED_VECTOR4_ARRAY => Some(Self::Vector4),
            _ => None,
        }
    }

    pub(crate) const fn value_type(self) -> AbiValueType {
        match self {
            Self::Byte => AbiValueType::PACKED_BYTE_ARRAY,
            Self::Int32 => AbiValueType::PACKED_INT32_ARRAY,
            Self::Int64 => AbiValueType::PACKED_INT64_ARRAY,
            Self::Float32 => AbiValueType::PACKED_FLOAT32_ARRAY,
            Self::Float64 => AbiValueType::PACKED_FLOAT64_ARRAY,
            Self::String => AbiValueType::PACKED_STRING_ARRAY,
            Self::Vector2 => AbiValueType::PACKED_VECTOR2_ARRAY,
            Self::Vector3 => AbiValueType::PACKED_VECTOR3_ARRAY,
            Self::Color => AbiValueType::PACKED_COLOR_ARRAY,
            Self::Vector4 => AbiValueType::PACKED_VECTOR4_ARRAY,
        }
    }

    pub(crate) const fn variant_type(self) -> GDExtensionVariantType {
        match self {
            Self::Byte => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY,
            Self::Int32 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY,
            Self::Int64 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY,
            Self::Float32 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY,
            Self::Float64 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY,
            Self::String => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
            Self::Vector2 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY,
            Self::Vector3 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY,
            Self::Color => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY,
            Self::Vector4 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY,
        }
    }

    const fn element_size(self) -> Option<usize> {
        match self {
            Self::Byte => Some(1),
            Self::Int32 | Self::Float32 => Some(4),
            Self::Int64 | Self::Float64 | Self::Vector2 => Some(8),
            Self::Vector3 => Some(12),
            Self::Color | Self::Vector4 => Some(16),
            Self::String => None,
        }
    }

    const fn index_names(self) -> (&'static [u8], &'static [u8]) {
        match self {
            Self::Byte => (
                b"packed_byte_array_operator_index\0",
                b"packed_byte_array_operator_index_const\0",
            ),
            Self::Int32 => (
                b"packed_int32_array_operator_index\0",
                b"packed_int32_array_operator_index_const\0",
            ),
            Self::Int64 => (
                b"packed_int64_array_operator_index\0",
                b"packed_int64_array_operator_index_const\0",
            ),
            Self::Float32 => (
                b"packed_float32_array_operator_index\0",
                b"packed_float32_array_operator_index_const\0",
            ),
            Self::Float64 => (
                b"packed_float64_array_operator_index\0",
                b"packed_float64_array_operator_index_const\0",
            ),
            Self::String => (
                b"packed_string_array_operator_index\0",
                b"packed_string_array_operator_index_const\0",
            ),
            Self::Vector2 => (
                b"packed_vector2_array_operator_index\0",
                b"packed_vector2_array_operator_index_const\0",
            ),
            Self::Vector3 => (
                b"packed_vector3_array_operator_index\0",
                b"packed_vector3_array_operator_index_const\0",
            ),
            Self::Color => (
                b"packed_color_array_operator_index\0",
                b"packed_color_array_operator_index_const\0",
            ),
            Self::Vector4 => (
                b"packed_vector4_array_operator_index\0",
                b"packed_vector4_array_operator_index_const\0",
            ),
        }
    }
}

#[repr(C, align(8))]
struct PackedStorage([u8; 16]);

pub(crate) struct OwnedPackedArray {
    interface: EngineInterface,
    kind: PackedArrayKind,
    storage: PackedStorage,
    destroy: GDExtensionPtrDestructor,
    size: GDExtensionPtrBuiltInMethod,
    resize: GDExtensionPtrBuiltInMethod,
    index: MutableIndex,
    index_const: ConstIndex,
}

impl OwnedPackedArray {
    pub(crate) const fn kind(&self) -> PackedArrayKind {
        self.kind
    }

    pub(crate) fn from_abi(
        interface: EngineInterface,
        value: AbiValueV1,
    ) -> Result<Self, ValueError> {
        let kind = PackedArrayKind::from_value_type(value.type_).ok_or_else(|| {
            ValueError::invalid("Godot packed-array argument has an invalid type")
        })?;
        if !matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES) {
            return Err(ValueError::invalid(
                "Godot packed-array value has invalid ownership flags",
            ));
        }
        let (pointer, length) = value.byte_range(value.type_).ok_or_else(|| {
            ValueError::invalid("Godot packed-array argument has an invalid byte range")
        })?;
        if length > MAX_PACKED_BYTES {
            return Err(ValueError::invalid(
                "Godot packed-array argument exceeds the Host byte limit",
            ));
        }
        // SAFETY: Project modules retain borrowed argument storage for this
        // synchronous call and the byte length is bounded above.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let mut result = Self::empty(interface, kind)?;
        if kind == PackedArrayKind::String {
            result.fill_strings(bytes)?;
        } else {
            result.fill_numeric(bytes)?;
        }
        Ok(result)
    }

    pub(crate) fn empty(
        interface: EngineInterface,
        kind: PackedArrayKind,
    ) -> Result<Self, ValueError> {
        let variant_type = kind.variant_type();
        let get_constructor = interface.variant_get_ptr_constructor.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array constructor lookup is unavailable",
            )
        })?;
        let get_destructor = interface.variant_get_ptr_destructor.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array destructor lookup is unavailable",
            )
        })?;
        let get_method = interface.variant_get_ptr_builtin_method.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array method lookup is unavailable",
            )
        })?;
        // SAFETY: Variant type and constructor index come from the official API.
        let constructor = unsafe { get_constructor(variant_type, 0) }.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array default constructor is unavailable",
            )
        })?;
        // SAFETY: The official API defines one destructor for this builtin.
        let destroy = unsafe { get_destructor(variant_type) };
        destroy.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array destructor is unavailable",
            )
        })?;
        let size_name = StaticStringName::new(interface, c"size");
        let resize_name = StaticStringName::new(interface, c"resize");
        // SAFETY: Names, hashes, and receiver type are authenticated against
        // the official Godot 4.4 baseline API.
        let size = unsafe { get_method(variant_type, size_name.as_ptr(), SIZE_HASH) };
        // SAFETY: See above.
        let resize = unsafe { get_method(variant_type, resize_name.as_ptr(), RESIZE_HASH) };
        size.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array size method is unavailable",
            )
        })?;
        resize.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array resize method is unavailable",
            )
        })?;
        let (index_name, index_const_name) = kind.index_names();
        let index = resolve_index::<MutableIndex>(interface, index_name);
        let index_const = resolve_index::<ConstIndex>(interface, index_const_name);
        index.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array mutable index operation is unavailable",
            )
        })?;
        index_const.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array const index operation is unavailable",
            )
        })?;

        let mut result = Self {
            interface,
            kind,
            storage: PackedStorage([0; 16]),
            destroy,
            size,
            resize,
            index,
            index_const,
        };
        // SAFETY: The storage is large and aligned for every authenticated
        // standard-precision packed-array builtin. Constructor zero initializes it.
        unsafe { constructor(result.as_mut_ptr(), core::ptr::null()) };
        Ok(result)
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, ValueError> {
        let count = self.len()?;
        if self.kind == PackedArrayKind::String {
            let mut encoded = Vec::new();
            encoded.extend_from_slice(
                &u64::try_from(count)
                    .expect("bounded packed count fits u64")
                    .to_le_bytes(),
            );
            for index in 0..count {
                let value = self.const_element(index)?;
                let text = read_utf8_string(self.interface, value).map_err(|_| {
                    ValueError::new(
                        AbiStatus::Internal,
                        "Godot returned invalid UTF-8 in PackedStringArray",
                    )
                })?;
                let required = STRING_HEADER_BYTES
                    .checked_add(text.len())
                    .and_then(|value| encoded.len().checked_add(value))
                    .ok_or_else(|| {
                        ValueError::new(
                            AbiStatus::Unsupported,
                            "Godot PackedStringArray encoding is too large",
                        )
                    })?;
                if required > MAX_PACKED_BYTES {
                    return Err(ValueError::new(
                        AbiStatus::Unsupported,
                        "Godot PackedStringArray exceeds the Host byte limit",
                    ));
                }
                encoded.extend_from_slice(&(text.len() as u64).to_le_bytes());
                encoded.extend_from_slice(text.as_bytes());
            }
            return Ok(encoded);
        }

        let width = self.kind.element_size().expect("numeric packed width");
        let length = count.checked_mul(width).ok_or_else(|| {
            ValueError::new(
                AbiStatus::Unsupported,
                "Godot packed-array byte size overflowed",
            )
        })?;
        if length > MAX_PACKED_BYTES {
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Godot packed-array result exceeds the Host byte limit",
            ));
        }
        if length == 0 {
            return Ok(Vec::new());
        }
        let source = self.const_element(0)?;
        let mut bytes = vec![0_u8; length];
        // SAFETY: Packed arrays expose contiguous native element storage. The
        // count and authenticated element width bound both ranges exactly.
        unsafe {
            core::ptr::copy_nonoverlapping(source.cast::<u8>(), bytes.as_mut_ptr(), length);
        }
        Ok(bytes)
    }

    pub(crate) fn as_const_ptr(&self) -> GDExtensionConstTypePtr {
        self.storage.0.as_ptr().cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        self.storage.0.as_mut_ptr().cast()
    }

    fn fill_numeric(&mut self, bytes: &[u8]) -> Result<(), ValueError> {
        let width = self.kind.element_size().expect("numeric packed width");
        if bytes.len() % width != 0 {
            return Err(ValueError::invalid(
                "Godot packed-array argument has a partial element",
            ));
        }
        let count = bytes.len() / width;
        self.resize(count)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let destination = self.mutable_element(0)?;
        // SAFETY: Resize established a contiguous destination of this exact
        // byte length and the project range was bounded before this copy.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast::<u8>(), bytes.len());
        }
        Ok(())
    }

    fn fill_strings(&mut self, bytes: &[u8]) -> Result<(), ValueError> {
        let values = decode_strings(bytes)?;
        if values.len() > MAX_PACKED_STRINGS {
            return Err(ValueError::invalid(
                "Godot PackedStringArray exceeds the Host element limit",
            ));
        }
        let get_method = self
            .interface
            .variant_get_ptr_builtin_method
            .ok_or_else(|| {
                ValueError::new(
                    AbiStatus::Internal,
                    "Godot packed-array method lookup is unavailable",
                )
            })?;
        let push_name = StaticStringName::new(self.interface, c"push_back");
        let push = {
            // SAFETY: Type, method, and hash come from the official baseline API.
            unsafe { get_method(self.kind.variant_type(), push_name.as_ptr(), PUSH_BACK_HASH) }
        }
        .ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot PackedStringArray push_back method is unavailable",
            )
        })?;
        for value in values {
            let value = LocalGodotString::new_utf8(self.interface, value).ok_or_else(|| {
                ValueError::invalid("PackedStringArray value could not be encoded for Godot")
            })?;
            let arguments = [value.as_ptr()];
            let mut failed = 0_u8;
            // SAFETY: Receiver and String argument are initialized official
            // builtins; push_back takes one argument and returns bool.
            unsafe {
                push(
                    self.as_mut_ptr(),
                    arguments.as_ptr(),
                    (&mut failed as *mut u8).cast(),
                    1,
                );
            }
            if failed != 0 {
                return Err(ValueError::new(
                    AbiStatus::Internal,
                    "Godot rejected a PackedStringArray element",
                ));
            }
        }
        Ok(())
    }

    fn len(&self) -> Result<usize, ValueError> {
        let size = self.size.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array size method is unavailable",
            )
        })?;
        let mut count = 0_i64;
        // SAFETY: Receiver is an initialized packed-array builtin and size has
        // no arguments with one i64 return slot.
        unsafe {
            size(
                self.as_const_ptr().cast_mut(),
                core::ptr::null(),
                (&mut count as *mut i64).cast(),
                0,
            );
        }
        let count = usize::try_from(count).map_err(|_| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array returned an invalid size",
            )
        })?;
        if count > MAX_PACKED_BYTES {
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Godot packed-array exceeds the Host element limit",
            ));
        }
        Ok(count)
    }

    fn resize(&mut self, count: usize) -> Result<(), ValueError> {
        let count = i64::try_from(count)
            .map_err(|_| ValueError::invalid("Godot packed-array element count is out of range"))?;
        let resize = self.resize.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array resize method is unavailable",
            )
        })?;
        let arguments = [core::ptr::from_ref(&count).cast()];
        let mut error = -1_i64;
        // SAFETY: Receiver is initialized; resize takes one int64 argument and
        // returns Godot Error as int64.
        unsafe {
            resize(
                self.as_mut_ptr(),
                arguments.as_ptr(),
                (&mut error as *mut i64).cast(),
                1,
            );
        }
        if error != 0 {
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Godot could not resize a packed-array argument",
            ));
        }
        Ok(())
    }

    fn mutable_element(&mut self, index: usize) -> Result<GDExtensionTypePtr, ValueError> {
        let callback = self.index.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array mutable index operation is unavailable",
            )
        })?;
        let index = i64::try_from(index)
            .map_err(|_| ValueError::invalid("Godot packed-array index is out of range"))?;
        // SAFETY: Caller has resized the same live array above this index.
        let value = unsafe { callback(self.as_mut_ptr(), index) };
        (!value.is_null()).then_some(value).ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot returned a null packed-array element",
            )
        })
    }

    fn const_element(&self, index: usize) -> Result<GDExtensionConstTypePtr, ValueError> {
        let callback = self.index_const.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array const index operation is unavailable",
            )
        })?;
        let index = i64::try_from(index).map_err(|_| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array result index is out of range",
            )
        })?;
        // SAFETY: Index is below the size read from this same live array.
        let value = unsafe { callback(self.as_const_ptr(), index) };
        (!value.is_null()).then_some(value).ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot returned a null packed-array result element",
            )
        })
    }
}

impl Drop for OwnedPackedArray {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            // SAFETY: Storage contains one initialized builtin and is destroyed once.
            unsafe { destroy(self.as_mut_ptr()) };
        }
    }
}

pub(crate) fn read_packed_bytes(
    interface: EngineInterface,
    kind: PackedArrayKind,
    value: GDExtensionConstTypePtr,
) -> Result<Vec<u8>, ValueError> {
    if value.is_null() {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot supplied a null packed-array value",
        ));
    }
    let get_method = interface.variant_get_ptr_builtin_method.ok_or_else(|| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot packed-array method lookup is unavailable",
        )
    })?;
    let size_name = StaticStringName::new(interface, c"size");
    // SAFETY: Type, method, and hash come from the official baseline API.
    let size = unsafe { get_method(kind.variant_type(), size_name.as_ptr(), SIZE_HASH) }
        .ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot packed-array size method is unavailable",
            )
        })?;
    let mut count = 0_i64;
    // SAFETY: The borrowed value is an initialized packed-array builtin.
    unsafe {
        size(
            value.cast_mut(),
            core::ptr::null(),
            (&mut count as *mut i64).cast(),
            0,
        );
    }
    let count = usize::try_from(count).map_err(|_| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot packed-array returned an invalid size",
        )
    })?;
    if count > MAX_PACKED_BYTES {
        return Err(ValueError::new(
            AbiStatus::Unsupported,
            "Godot packed-array exceeds the Host element limit",
        ));
    }
    let (_, index_name) = kind.index_names();
    let index = resolve_index::<ConstIndex>(interface, index_name).ok_or_else(|| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot packed-array const index operation is unavailable",
        )
    })?;
    if kind == PackedArrayKind::String {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(count as u64).to_le_bytes());
        for position in 0..count {
            // SAFETY: Position is below the size read from the same live array.
            let string = unsafe { index(value, position as i64) };
            if string.is_null() {
                return Err(ValueError::new(
                    AbiStatus::Internal,
                    "Godot returned a null PackedStringArray element",
                ));
            }
            let text = read_utf8_string(interface, string).map_err(|_| {
                ValueError::new(
                    AbiStatus::Internal,
                    "Godot returned invalid UTF-8 in PackedStringArray",
                )
            })?;
            let required = encoded
                .len()
                .checked_add(STRING_HEADER_BYTES)
                .and_then(|value| value.checked_add(text.len()))
                .ok_or_else(|| {
                    ValueError::new(
                        AbiStatus::Unsupported,
                        "Godot PackedStringArray encoding is too large",
                    )
                })?;
            if required > MAX_PACKED_BYTES {
                return Err(ValueError::new(
                    AbiStatus::Unsupported,
                    "Godot PackedStringArray exceeds the Host byte limit",
                ));
            }
            encoded.extend_from_slice(&(text.len() as u64).to_le_bytes());
            encoded.extend_from_slice(text.as_bytes());
        }
        return Ok(encoded);
    }
    let width = kind.element_size().expect("numeric packed width");
    let length = count.checked_mul(width).ok_or_else(|| {
        ValueError::new(
            AbiStatus::Unsupported,
            "Godot packed-array byte size overflowed",
        )
    })?;
    if length > MAX_PACKED_BYTES {
        return Err(ValueError::new(
            AbiStatus::Unsupported,
            "Godot packed-array exceeds the Host byte limit",
        ));
    }
    if length == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: Zero is below the non-empty array size.
    let source = unsafe { index(value, 0) };
    if source.is_null() {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot returned a null packed-array element",
        ));
    }
    let mut bytes = vec![0_u8; length];
    // SAFETY: Official packed arrays expose contiguous element storage and
    // the authenticated element width bounds this exact copy.
    unsafe {
        core::ptr::copy_nonoverlapping(source.cast::<u8>(), bytes.as_mut_ptr(), length);
    }
    Ok(bytes)
}

fn decode_strings(bytes: &[u8]) -> Result<Vec<&str>, ValueError> {
    let count = read_u64(bytes, 0)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| ValueError::invalid("PackedStringArray count is invalid"))?;
    if count > MAX_PACKED_STRINGS {
        return Err(ValueError::invalid(
            "PackedStringArray exceeds the Host element limit",
        ));
    }
    let mut values = Vec::with_capacity(count);
    let mut offset = STRING_HEADER_BYTES;
    for _ in 0..count {
        let length = read_u64(bytes, offset)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| ValueError::invalid("PackedStringArray length is invalid"))?;
        offset = offset
            .checked_add(STRING_HEADER_BYTES)
            .ok_or_else(|| ValueError::invalid("PackedStringArray offset overflowed"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| ValueError::invalid("PackedStringArray value overflowed"))?;
        let value = core::str::from_utf8(
            bytes
                .get(offset..end)
                .ok_or_else(|| ValueError::invalid("PackedStringArray value is truncated"))?,
        )
        .map_err(|_| ValueError::invalid("PackedStringArray value is not valid UTF-8"))?;
        values.push(value);
        offset = end;
    }
    if offset != bytes.len() {
        return Err(ValueError::invalid(
            "PackedStringArray has trailing encoded bytes",
        ));
    }
    Ok(values)
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(STRING_HEADER_BYTES)?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn resolve_index<T>(interface: EngineInterface, name: &[u8]) -> T
where
    T: Copy,
{
    // SAFETY: Every name comes from `PackedArrayKind::index_names` and its
    // target uses the official two-argument pointer-returning C signature.
    let raw = unsafe { (interface.get_proc_address)(name.as_ptr().cast()) };
    assert_eq!(
        mem::size_of::<GDExtensionInterfaceFunctionPtr>(),
        mem::size_of::<T>()
    );
    // SAFETY: Size equality was asserted and function pointer representations
    // match the official nullable GDExtension interface convention.
    unsafe { mem::transmute_copy::<GDExtensionInterfaceFunctionPtr, T>(&raw) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_kinds_have_exact_element_widths_and_value_types() {
        for (kind, value_type, width) in [
            (PackedArrayKind::Byte, AbiValueType::PACKED_BYTE_ARRAY, 1),
            (PackedArrayKind::Int32, AbiValueType::PACKED_INT32_ARRAY, 4),
            (PackedArrayKind::Int64, AbiValueType::PACKED_INT64_ARRAY, 8),
            (
                PackedArrayKind::Float32,
                AbiValueType::PACKED_FLOAT32_ARRAY,
                4,
            ),
            (
                PackedArrayKind::Float64,
                AbiValueType::PACKED_FLOAT64_ARRAY,
                8,
            ),
            (
                PackedArrayKind::Vector2,
                AbiValueType::PACKED_VECTOR2_ARRAY,
                8,
            ),
            (
                PackedArrayKind::Vector3,
                AbiValueType::PACKED_VECTOR3_ARRAY,
                12,
            ),
            (PackedArrayKind::Color, AbiValueType::PACKED_COLOR_ARRAY, 16),
            (
                PackedArrayKind::Vector4,
                AbiValueType::PACKED_VECTOR4_ARRAY,
                16,
            ),
        ] {
            assert_eq!(kind.value_type(), value_type);
            assert_eq!(kind.element_size(), Some(width));
        }
        assert_eq!(PackedArrayKind::String.element_size(), None);
    }

    #[test]
    fn string_encoding_rejects_truncation_invalid_utf8_and_trailing_bytes() {
        let mut valid = Vec::new();
        valid.extend_from_slice(&2_u64.to_le_bytes());
        valid.extend_from_slice(&6_u64.to_le_bytes());
        valid.extend_from_slice("你好".as_bytes());
        valid.extend_from_slice(&5_u64.to_le_bytes());
        valid.extend_from_slice(b"Godot");
        assert_eq!(decode_strings(&valid), Ok(vec!["你好", "Godot"]));

        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(decode_strings(&trailing).is_err());
        assert!(decode_strings(&valid[..valid.len() - 1]).is_err());

        let mut invalid = Vec::new();
        invalid.extend_from_slice(&1_u64.to_le_bytes());
        invalid.extend_from_slice(&1_u64.to_le_bytes());
        invalid.push(0xff);
        assert!(decode_strings(&invalid).is_err());
    }
}
