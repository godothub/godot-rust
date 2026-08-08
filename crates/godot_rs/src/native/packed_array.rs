use core::mem;
use core::ptr;

use godot_rs_api::abi::{AbiPtrcallType, AbiValueType, AbiValueV1};

use super::runtime::Interface;
use super::sys;
use super::value::{GodotString, GodotStringName};
use crate::error::{EngineError, EngineResult};

const SIZE_HASH: i64 = 3_173_160_232;
const RESIZE_HASH: i64 = 848_867_239;
const PUSH_BACK_HASH: i64 = 816_187_996;
const MAX_PACKED_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKED_ELEMENTS: usize = 1_000_000;
const STRING_HEADER_BYTES: usize = size_of::<u64>();

type MutableIndex =
    Option<unsafe extern "C" fn(sys::GDExtensionTypePtr, i64) -> sys::GDExtensionTypePtr>;
type ConstIndex =
    Option<unsafe extern "C" fn(sys::GDExtensionConstTypePtr, i64) -> sys::GDExtensionConstTypePtr>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackedArrayKind {
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
    fn from_ptrcall(value: AbiPtrcallType) -> Option<Self> {
        match value {
            AbiPtrcallType::PACKED_BYTE_ARRAY => Some(Self::Byte),
            AbiPtrcallType::PACKED_INT32_ARRAY => Some(Self::Int32),
            AbiPtrcallType::PACKED_INT64_ARRAY => Some(Self::Int64),
            AbiPtrcallType::PACKED_FLOAT32_ARRAY => Some(Self::Float32),
            AbiPtrcallType::PACKED_FLOAT64_ARRAY => Some(Self::Float64),
            AbiPtrcallType::PACKED_STRING_ARRAY => Some(Self::String),
            AbiPtrcallType::PACKED_VECTOR2_ARRAY => Some(Self::Vector2),
            AbiPtrcallType::PACKED_VECTOR3_ARRAY => Some(Self::Vector3),
            AbiPtrcallType::PACKED_COLOR_ARRAY => Some(Self::Color),
            AbiPtrcallType::PACKED_VECTOR4_ARRAY => Some(Self::Vector4),
            _ => None,
        }
    }

    const fn value_type(self) -> AbiValueType {
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

    const fn variant_type(self) -> sys::GDExtensionVariantType {
        match self {
            Self::Byte => sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY,
            Self::Int32 => sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY,
            Self::Int64 => sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY,
            Self::Float32 => {
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY
            }
            Self::Float64 => {
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY
            }
            Self::String => {
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY
            }
            Self::Vector2 => {
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY
            }
            Self::Vector3 => {
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY
            }
            Self::Color => sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY,
            Self::Vector4 => {
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY
            }
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

    const fn index_symbols(self) -> (&'static [u8], &'static [u8]) {
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

pub(super) struct NativePackedArray {
    interface: Interface,
    kind: PackedArrayKind,
    storage: PackedStorage,
    initialized: bool,
    destroy: sys::GDExtensionPtrDestructor,
    size: sys::GDExtensionPtrBuiltInMethod,
    resize: sys::GDExtensionPtrBuiltInMethod,
    index: MutableIndex,
    index_const: ConstIndex,
}

impl NativePackedArray {
    pub(super) fn from_argument(
        interface: Interface,
        ptrcall_type: AbiPtrcallType,
        value: AbiValueV1,
    ) -> EngineResult<Self> {
        let kind = PackedArrayKind::from_ptrcall(ptrcall_type)
            .ok_or_else(|| EngineError::invalid_argument("invalid Native packed-array contract"))?;
        if value.type_ != kind.value_type() || value.reserved_flags != 0 {
            return Err(EngineError::invalid_argument(
                "Native packed-array argument violates its generated contract",
            ));
        }
        let (pointer, length) = value
            .byte_range(value.type_)
            .ok_or_else(|| EngineError::invalid_argument("invalid Native packed-array range"))?;
        if length > MAX_PACKED_BYTES {
            return Err(EngineError::invalid_argument(
                "Native packed-array argument exceeds the 64 MiB boundary",
            ));
        }
        // SAFETY: Generated wrappers retain borrowed ABI storage for this
        // synchronous call and the range has been bounded above.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let mut result = Self::empty(interface, kind)?;
        if kind == PackedArrayKind::String {
            result.fill_strings(bytes)?;
        } else {
            result.fill_numeric(bytes)?;
        }
        Ok(result)
    }

    pub(super) fn output(interface: Interface, ptrcall_type: AbiPtrcallType) -> EngineResult<Self> {
        let kind = PackedArrayKind::from_ptrcall(ptrcall_type)
            .ok_or_else(|| EngineError::invalid_argument("invalid Native packed-array output"))?;
        Self::empty(interface, kind)
    }

    fn empty(interface: Interface, kind: PackedArrayKind) -> EngineResult<Self> {
        let mut result = Self::uninitialized(interface, kind)?;
        let variant_type = kind.variant_type();
        // SAFETY: The generated type and default-constructor index are from
        // the selected authenticated Godot API.
        let constructor = unsafe { (interface.variant_get_ptr_constructor)(variant_type, 0) }
            .ok_or_else(|| {
                EngineError::unavailable("Godot omitted a Native packed-array constructor")
            })?;
        // SAFETY: Storage is large and aligned for every authenticated
        // standard-precision packed-array layout.
        unsafe { constructor(result.as_mut_ptr(), ptr::null()) };
        result.initialized = true;
        Ok(result)
    }

    pub(super) fn constructor_output(
        interface: Interface,
        ptrcall_type: AbiPtrcallType,
    ) -> EngineResult<Self> {
        let kind = PackedArrayKind::from_ptrcall(ptrcall_type)
            .ok_or_else(|| EngineError::invalid_argument("invalid Native packed-array output"))?;
        Self::uninitialized(interface, kind)
    }

    fn uninitialized(interface: Interface, kind: PackedArrayKind) -> EngineResult<Self> {
        let variant_type = kind.variant_type();
        // SAFETY: The selected official builtin owns one matching destructor.
        let destroy = unsafe { (interface.variant_get_ptr_destructor)(variant_type) };
        if destroy.is_none() {
            return Err(EngineError::unavailable(
                "Godot omitted a Native packed-array destructor",
            ));
        }
        let size_name = GodotStringName::new(&interface, "size")
            .map_err(|error| EngineError::invalid_result(error.to_string()))?;
        let resize_name = GodotStringName::new(&interface, "resize")
            .map_err(|error| EngineError::invalid_result(error.to_string()))?;
        // SAFETY: Method names and hashes are authenticated Godot builtin APIs.
        let size = unsafe {
            (interface.variant_get_ptr_builtin_method)(variant_type, size_name.as_ptr(), SIZE_HASH)
        };
        // SAFETY: See the `size` lookup above.
        let resize = unsafe {
            (interface.variant_get_ptr_builtin_method)(
                variant_type,
                resize_name.as_ptr(),
                RESIZE_HASH,
            )
        };
        if size.is_none() || resize.is_none() {
            return Err(EngineError::unavailable(
                "Godot omitted required Native packed-array methods",
            ));
        }
        let (index_name, index_const_name) = kind.index_symbols();
        let index = resolve_index::<MutableIndex>(interface, index_name);
        let index_const = resolve_index::<ConstIndex>(interface, index_const_name);
        if index.is_none() || index_const.is_none() {
            return Err(EngineError::unavailable(
                "Godot omitted required Native packed-array index operations",
            ));
        }
        let result = Self {
            interface,
            kind,
            storage: PackedStorage([0; 16]),
            initialized: false,
            destroy,
            size,
            resize,
            index,
            index_const,
        };
        Ok(result)
    }

    pub(super) fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    pub(super) fn as_const_ptr(&self) -> sys::GDExtensionConstTypePtr {
        self.storage.0.as_ptr().cast()
    }

    pub(super) fn as_mut_ptr(&mut self) -> sys::GDExtensionTypePtr {
        self.storage.0.as_mut_ptr().cast()
    }

    pub(super) fn into_abi(self) -> EngineResult<AbiValueV1> {
        let value_type = self.kind.value_type();
        let bytes = self.to_bytes()?;
        Ok(crate::module::owned_bytes(value_type, bytes))
    }

    fn fill_numeric(&mut self, bytes: &[u8]) -> EngineResult<()> {
        let width = self
            .kind
            .element_size()
            .expect("numeric packed-array width");
        if bytes.len() % width != 0 {
            return Err(EngineError::invalid_argument(
                "Native packed-array contains a partial element",
            ));
        }
        let count = bytes.len() / width;
        if count > MAX_PACKED_ELEMENTS {
            return Err(EngineError::invalid_argument(
                "Native packed-array exceeds the element boundary",
            ));
        }
        self.resize(count)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let destination = self.mutable_element(0)?;
        // SAFETY: Resize created a contiguous destination with exactly this
        // authenticated element width and byte count.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast(), bytes.len()) };
        Ok(())
    }

    fn fill_strings(&mut self, bytes: &[u8]) -> EngineResult<()> {
        let values = decode_strings(bytes)?;
        let method_name = GodotStringName::new(&self.interface, "push_back")
            .map_err(|error| EngineError::invalid_result(error.to_string()))?;
        // SAFETY: The selected builtin, method and signature hash come from
        // the authenticated official API.
        let push_back = unsafe {
            (self.interface.variant_get_ptr_builtin_method)(
                self.kind.variant_type(),
                method_name.as_ptr(),
                PUSH_BACK_HASH,
            )
        }
        .ok_or_else(|| {
            EngineError::unavailable("Godot omitted PackedStringArray.push_back in Native mode")
        })?;
        for text in values {
            let string = GodotString::new(&self.interface, text)
                .map_err(|error| EngineError::invalid_argument(error.to_string()))?;
            let arguments = [string.as_ptr()];
            let mut failed = 0_u8;
            // SAFETY: Receiver and String are initialized official builtins.
            unsafe {
                push_back(
                    self.as_mut_ptr(),
                    arguments.as_ptr(),
                    ptr::from_mut(&mut failed).cast(),
                    1,
                );
            }
            if failed != 0 {
                return Err(EngineError::invalid_result(
                    "Godot rejected a Native PackedStringArray element",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn to_bytes(&self) -> EngineResult<Vec<u8>> {
        let count = self.len()?;
        if self.kind == PackedArrayKind::String {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(count as u64).to_le_bytes());
            for index in 0..count {
                let value = self.const_element(index)?;
                // SAFETY: The element pointer belongs to this live
                // PackedStringArray and remains initialized during the copy.
                let text = unsafe { GodotString::copy_ptr_to_rust(&self.interface, value) }
                    .map_err(|error| EngineError::invalid_result(error.to_string()))?;
                let required = bytes
                    .len()
                    .checked_add(STRING_HEADER_BYTES)
                    .and_then(|length| length.checked_add(text.len()))
                    .ok_or_else(|| {
                        EngineError::invalid_result("Native PackedStringArray byte size overflowed")
                    })?;
                if required > MAX_PACKED_BYTES {
                    return Err(EngineError::invalid_result(
                        "Native PackedStringArray exceeds the 64 MiB boundary",
                    ));
                }
                bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
                bytes.extend_from_slice(text.as_bytes());
            }
            return Ok(bytes);
        }
        let width = self
            .kind
            .element_size()
            .expect("numeric packed-array width");
        let length = count.checked_mul(width).ok_or_else(|| {
            EngineError::invalid_result("Native packed-array byte size overflowed")
        })?;
        if length > MAX_PACKED_BYTES {
            return Err(EngineError::invalid_result(
                "Native packed-array exceeds the 64 MiB boundary",
            ));
        }
        if length == 0 {
            return Ok(Vec::new());
        }
        let source = self.const_element(0)?;
        let mut bytes = vec![0_u8; length];
        // SAFETY: Godot packed arrays expose contiguous storage; the live
        // element count and authenticated width bound this copy exactly.
        unsafe { ptr::copy_nonoverlapping(source.cast(), bytes.as_mut_ptr(), length) };
        Ok(bytes)
    }

    fn len(&self) -> EngineResult<usize> {
        let size = self
            .size
            .ok_or_else(|| EngineError::unavailable("Native packed-array size is unavailable"))?;
        let mut count = 0_i64;
        // SAFETY: Receiver is initialized and `size` has no arguments.
        unsafe {
            size(
                self.as_const_ptr().cast_mut(),
                ptr::null(),
                ptr::from_mut(&mut count).cast(),
                0,
            );
        }
        let count = usize::try_from(count).map_err(|_| {
            EngineError::invalid_result("Godot returned a negative packed-array size")
        })?;
        if count > MAX_PACKED_ELEMENTS {
            return Err(EngineError::invalid_result(
                "Godot returned a packed-array beyond the element boundary",
            ));
        }
        Ok(count)
    }

    fn resize(&mut self, count: usize) -> EngineResult<()> {
        let count = i64::try_from(count)
            .map_err(|_| EngineError::invalid_argument("packed-array size is out of range"))?;
        let resize = self
            .resize
            .ok_or_else(|| EngineError::unavailable("Native packed-array resize is unavailable"))?;
        let arguments = [ptr::from_ref(&count).cast()];
        let mut error = -1_i64;
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
                "Godot could not resize a Native packed-array",
            ));
        }
        Ok(())
    }

    fn mutable_element(&mut self, index: usize) -> EngineResult<sys::GDExtensionTypePtr> {
        let callback = self.index.ok_or_else(|| {
            EngineError::unavailable("Native packed-array mutable indexing is unavailable")
        })?;
        let index = i64::try_from(index)
            .map_err(|_| EngineError::invalid_argument("packed-array index is out of range"))?;
        // SAFETY: Caller resized this same array above the requested index.
        let value = unsafe { callback(self.as_mut_ptr(), index) };
        (!value.is_null()).then_some(value).ok_or_else(|| {
            EngineError::invalid_result("Godot returned a null mutable packed-array element")
        })
    }

    fn const_element(&self, index: usize) -> EngineResult<sys::GDExtensionConstTypePtr> {
        let callback = self.index_const.ok_or_else(|| {
            EngineError::unavailable("Native packed-array const indexing is unavailable")
        })?;
        let index = i64::try_from(index).map_err(|_| {
            EngineError::invalid_result("packed-array result index is out of range")
        })?;
        // SAFETY: Caller obtained this index from the same live array size.
        let value = unsafe { callback(self.as_const_ptr(), index) };
        (!value.is_null()).then_some(value).ok_or_else(|| {
            EngineError::invalid_result("Godot returned a null packed-array element")
        })
    }
}

impl Drop for NativePackedArray {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.destroy {
                // SAFETY: This storage owns one initialized builtin and drops once.
                unsafe { destroy(self.as_mut_ptr()) };
            }
        }
    }
}

fn decode_strings(bytes: &[u8]) -> EngineResult<Vec<&str>> {
    let count = read_u64(bytes, 0)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| EngineError::invalid_argument("PackedStringArray count is invalid"))?;
    if count > MAX_PACKED_ELEMENTS {
        return Err(EngineError::invalid_argument(
            "PackedStringArray exceeds the element boundary",
        ));
    }
    let mut values = Vec::with_capacity(count);
    let mut offset = STRING_HEADER_BYTES;
    for _ in 0..count {
        let length = read_u64(bytes, offset)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| EngineError::invalid_argument("PackedStringArray length is invalid"))?;
        offset = offset
            .checked_add(STRING_HEADER_BYTES)
            .ok_or_else(|| EngineError::invalid_argument("PackedStringArray offset overflowed"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| EngineError::invalid_argument("PackedStringArray value overflowed"))?;
        let value = core::str::from_utf8(bytes.get(offset..end).ok_or_else(|| {
            EngineError::invalid_argument("PackedStringArray value is truncated")
        })?)
        .map_err(|_| EngineError::invalid_argument("PackedStringArray value is not valid UTF-8"))?;
        values.push(value);
        offset = end;
    }
    if offset != bytes.len() {
        return Err(EngineError::invalid_argument(
            "PackedStringArray contains trailing bytes",
        ));
    }
    Ok(values)
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(STRING_HEADER_BYTES)?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn resolve_index<T>(interface: Interface, name: &[u8]) -> T
where
    T: Copy,
{
    // SAFETY: Every static name selects the matching official packed-array
    // index signature for this exact builtin kind.
    let raw = unsafe { (interface.get_proc_address)(name.as_ptr().cast()) };
    assert_eq!(
        mem::size_of::<sys::GDExtensionInterfaceFunctionPtr>(),
        mem::size_of::<T>()
    );
    // SAFETY: Size equality and the official nullable function-pointer ABI
    // were checked above.
    unsafe { mem::transmute_copy::<sys::GDExtensionInterfaceFunctionPtr, T>(&raw) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_kinds_match_the_stable_project_abi() {
        for (ptrcall, value_type, width) in [
            (
                AbiPtrcallType::PACKED_BYTE_ARRAY,
                AbiValueType::PACKED_BYTE_ARRAY,
                Some(1),
            ),
            (
                AbiPtrcallType::PACKED_INT32_ARRAY,
                AbiValueType::PACKED_INT32_ARRAY,
                Some(4),
            ),
            (
                AbiPtrcallType::PACKED_INT64_ARRAY,
                AbiValueType::PACKED_INT64_ARRAY,
                Some(8),
            ),
            (
                AbiPtrcallType::PACKED_FLOAT32_ARRAY,
                AbiValueType::PACKED_FLOAT32_ARRAY,
                Some(4),
            ),
            (
                AbiPtrcallType::PACKED_FLOAT64_ARRAY,
                AbiValueType::PACKED_FLOAT64_ARRAY,
                Some(8),
            ),
            (
                AbiPtrcallType::PACKED_STRING_ARRAY,
                AbiValueType::PACKED_STRING_ARRAY,
                None,
            ),
            (
                AbiPtrcallType::PACKED_VECTOR2_ARRAY,
                AbiValueType::PACKED_VECTOR2_ARRAY,
                Some(8),
            ),
            (
                AbiPtrcallType::PACKED_VECTOR3_ARRAY,
                AbiValueType::PACKED_VECTOR3_ARRAY,
                Some(12),
            ),
            (
                AbiPtrcallType::PACKED_COLOR_ARRAY,
                AbiValueType::PACKED_COLOR_ARRAY,
                Some(16),
            ),
            (
                AbiPtrcallType::PACKED_VECTOR4_ARRAY,
                AbiValueType::PACKED_VECTOR4_ARRAY,
                Some(16),
            ),
        ] {
            let kind = PackedArrayKind::from_ptrcall(ptrcall).expect("packed kind");
            assert_eq!(kind.value_type(), value_type);
            assert_eq!(kind.element_size(), width);
        }
    }

    #[test]
    fn packed_strings_reject_truncation_utf8_and_trailing_bytes() {
        let mut valid = Vec::new();
        valid.extend_from_slice(&2_u64.to_le_bytes());
        valid.extend_from_slice(&6_u64.to_le_bytes());
        valid.extend_from_slice("你好".as_bytes());
        valid.extend_from_slice(&5_u64.to_le_bytes());
        valid.extend_from_slice(b"Godot");
        assert_eq!(decode_strings(&valid).expect("valid"), ["你好", "Godot"]);

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
