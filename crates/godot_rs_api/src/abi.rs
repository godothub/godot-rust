//! Stable C ABI shared by the godot-rust Host and Script Mode modules.

use core::ffi::c_void;

/// Identifies a godot-rust ABI table.
pub const ABI_MAGIC: [u8; 8] = *b"GDRSABI\0";

/// Magic prefix for recursively encoded [`AbiValueType::VARIANT`],
/// [`AbiValueType::ARRAY`], and [`AbiValueType::DICTIONARY`] payloads.
pub const ABI_DYNAMIC_MAGIC: [u8; 8] = *b"GDRSVAR\0";

/// Current dynamic-value wire revision.
pub const ABI_DYNAMIC_VERSION: u16 = 2;

/// Magic prefix for stable [`AbiValueType::CALLABLE`] payloads.
pub const ABI_CALLABLE_MAGIC: [u8; 8] = *b"GDRSCAL\0";

/// Current Callable wire revision.
pub const ABI_CALLABLE_VERSION: u16 = 1;

/// Magic prefix for stable [`AbiValueType::SIGNAL`] payloads.
pub const ABI_SIGNAL_MAGIC: [u8; 8] = *b"GDRSSIG\0";

/// Current Signal wire revision.
pub const ABI_SIGNAL_VERSION: u16 = 1;

/// Callable-header flag indicating a Host-owned native Callable token.
pub const ABI_CALLABLE_OWNED: u16 = 1 << 0;

/// Root-header flag indicating that the Host retains the encoded native value.
pub const ABI_DYNAMIC_ROOT_OWNED: u16 = 1 << 0;

/// Maximum byte length accepted for one dynamic value.
pub const ABI_DYNAMIC_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Maximum number of entries accepted in one dynamic container.
pub const ABI_DYNAMIC_MAX_ELEMENTS: usize = 1_000_000;

/// Maximum recursive dynamic-container depth.
pub const ABI_DYNAMIC_MAX_DEPTH: usize = 64;

/// Host-owned dynamic group released through the Host value-drop callback.
pub const ABI_VALUE_OWNED_DYNAMIC_GROUP: u32 = 1 << 3;
/// Host-owned native Callable released through the Host value-drop callback.
pub const ABI_VALUE_OWNED_CALLABLE: u32 = 1 << 4;

/// Current incompatible ABI generation.
pub const ABI_MAJOR: u16 = 2;

/// Current backward-compatible ABI revision.
pub const ABI_MINOR: u16 = 35;

/// Header at the beginning of every versioned ABI table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiHeader {
    /// Fixed [`ABI_MAGIC`] value.
    pub magic: [u8; 8],
    /// Total table size known by the producer.
    pub struct_size: u32,
    /// Incompatible ABI generation.
    pub abi_major: u16,
    /// Backward-compatible ABI revision.
    pub abi_minor: u16,
}

impl AbiHeader {
    /// Creates a header for an ABI table of `struct_size` bytes.
    #[must_use]
    pub const fn new(struct_size: u32) -> Self {
        Self {
            magic: ABI_MAGIC,
            struct_size,
            abi_major: ABI_MAJOR,
            abi_minor: ABI_MINOR,
        }
    }

    /// Validates compatibility with a consumer requiring `minimum_size` and
    /// `minimum_minor`.
    #[must_use]
    pub fn is_compatible(self, minimum_size: u32, minimum_minor: u16) -> bool {
        self.magic == ABI_MAGIC
            && self.abi_major == ABI_MAJOR
            && self.abi_minor >= minimum_minor
            && self.struct_size >= minimum_size
    }
}

/// Borrowed bytes valid only for the duration documented by the surrounding call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiByteSlice {
    /// First byte, or null when `len` is zero.
    pub ptr: *const u8,
    /// Number of bytes.
    pub len: usize,
}

impl AbiByteSlice {
    /// Empty byte slice.
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    /// Borrows a static Rust string as ABI bytes.
    #[must_use]
    pub const fn from_static(value: &'static str) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// Stable value kind used by reflected method calls.
///
/// This is an integer newtype instead of an FFI enum so an invalid module
/// cannot create an invalid Rust enum discriminant at the ABI boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct AbiValueType(pub u32);

impl AbiValueType {
    pub const NIL: Self = Self(0);
    pub const BOOL: Self = Self(1);
    pub const I64: Self = Self(2);
    pub const F64: Self = Self(3);
    pub const OBJECT_ID: Self = Self(4);
    pub const U64: Self = Self(5);
    pub const STRING: Self = Self(6);
    pub const VECTOR2: Self = Self(7);
    pub const VECTOR3: Self = Self(8);
    pub const COLOR: Self = Self(9);
    pub const VECTOR2I: Self = Self(10);
    pub const VECTOR3I: Self = Self(11);
    pub const RID: Self = Self(12);
    pub const STRING_NAME: Self = Self(13);
    pub const NODE_PATH: Self = Self(14);
    pub const RECT2: Self = Self(15);
    pub const RECT2I: Self = Self(16);
    pub const QUATERNION: Self = Self(17);
    pub const PLANE: Self = Self(18);
    pub const VECTOR4: Self = Self(19);
    pub const VECTOR4I: Self = Self(20);
    pub const TRANSFORM2D: Self = Self(21);
    pub const AABB: Self = Self(22);
    pub const BASIS: Self = Self(23);
    pub const TRANSFORM3D: Self = Self(24);
    pub const PROJECTION: Self = Self(25);
    pub const PACKED_BYTE_ARRAY: Self = Self(26);
    pub const PACKED_INT32_ARRAY: Self = Self(27);
    pub const PACKED_INT64_ARRAY: Self = Self(28);
    pub const PACKED_FLOAT32_ARRAY: Self = Self(29);
    pub const PACKED_FLOAT64_ARRAY: Self = Self(30);
    pub const PACKED_STRING_ARRAY: Self = Self(31);
    pub const PACKED_VECTOR2_ARRAY: Self = Self(32);
    pub const PACKED_VECTOR3_ARRAY: Self = Self(33);
    pub const PACKED_COLOR_ARRAY: Self = Self(34);
    pub const PACKED_VECTOR4_ARRAY: Self = Self(35);
    pub const VARIANT: Self = Self(36);
    pub const ARRAY: Self = Self(37);
    pub const DICTIONARY: Self = Self(38);
    pub const CALLABLE: Self = Self(39);
    pub const SIGNAL: Self = Self(40);

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self.0, 0..=40)
    }
}

/// Validates the complete stable encoding of one Callable.
#[must_use]
pub fn validate_callable_value(bytes: &[u8]) -> bool {
    const HEADER_BYTES: usize = 32;
    if bytes.len() < HEADER_BYTES
        || bytes.len() > ABI_DYNAMIC_MAX_BYTES
        || bytes[..8] != ABI_CALLABLE_MAGIC
        || read_dynamic_u16(bytes, 8) != Some(ABI_CALLABLE_VERSION)
    {
        return false;
    }
    let Some(flags) = read_dynamic_u16(bytes, 10) else {
        return false;
    };
    let Some(token) = read_dynamic_u64(bytes, 12) else {
        return false;
    };
    if !matches!((flags, token), (0, 0) | (ABI_CALLABLE_OWNED, 1..=u64::MAX)) {
        return false;
    }
    let Some(method_length) = read_dynamic_u32(bytes, 28).map(|value| value as usize) else {
        return false;
    };
    let Some(method_end) = HEADER_BYTES.checked_add(method_length) else {
        return false;
    };
    method_end == bytes.len()
        && core::str::from_utf8(&bytes[HEADER_BYTES..method_end])
            .is_ok_and(|method| !method.as_bytes().contains(&0))
}

/// Returns the Host token encoded in a validated Callable payload.
#[must_use]
pub fn callable_value_ownership_token(bytes: &[u8]) -> Option<u64> {
    validate_callable_value(bytes)
        .then(|| {
            (read_dynamic_u16(bytes, 10) == Some(ABI_CALLABLE_OWNED))
                .then(|| read_dynamic_u64(bytes, 12))
                .flatten()
        })
        .flatten()
}

/// Validates the canonical stable encoding of one Godot Signal.
#[must_use]
pub fn validate_signal_value(bytes: &[u8]) -> bool {
    const HEADER_BYTES: usize = 24;
    if bytes.len() < HEADER_BYTES
        || bytes.len() > ABI_DYNAMIC_MAX_BYTES
        || bytes[..8] != ABI_SIGNAL_MAGIC
        || read_dynamic_u16(bytes, 8) != Some(ABI_SIGNAL_VERSION)
        || read_dynamic_u16(bytes, 10) != Some(0)
    {
        return false;
    }
    let Some(object_id) = read_dynamic_u64(bytes, 12) else {
        return false;
    };
    let Some(name_length) = read_dynamic_u32(bytes, 20).map(|value| value as usize) else {
        return false;
    };
    let Some(name_end) = HEADER_BYTES.checked_add(name_length) else {
        return false;
    };
    if name_end != bytes.len() {
        return false;
    }
    let name = &bytes[HEADER_BYTES..name_end];
    core::str::from_utf8(name).is_ok_and(|name| {
        !name.as_bytes().contains(&0)
            && matches!(
                (object_id, name.is_empty()),
                (0, true) | (1..=u64::MAX, false)
            )
    })
}

/// Validates the complete recursive encoding used by the dynamic-value ABI.
///
/// `VARIANT` accepts any supported Godot value at the root. `ARRAY` and
/// `DICTIONARY` additionally require the corresponding root container type.
/// The validator performs no allocation and rejects trailing bytes,
/// non-zero extension flags, excessive nesting, oversized containers, invalid
/// UTF-8, and malformed packed-array payloads.
#[must_use]
pub fn validate_dynamic_value(expected: AbiValueType, bytes: &[u8]) -> bool {
    const ROOT_HEADER_BYTES: usize = 20;
    if !matches!(
        expected,
        AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY
    ) || bytes.len() < ROOT_HEADER_BYTES
        || bytes.len() > ABI_DYNAMIC_MAX_BYTES
        || bytes[..8] != ABI_DYNAMIC_MAGIC
        || read_dynamic_u16(bytes, 8) != Some(ABI_DYNAMIC_VERSION)
    {
        return false;
    }
    let Some(root_flags) = read_dynamic_u16(bytes, 10) else {
        return false;
    };
    let Some(token) = read_dynamic_u64(bytes, 12) else {
        return false;
    };
    if !matches!(
        (root_flags, token),
        (0, 0) | (ABI_DYNAMIC_ROOT_OWNED, 1..=u64::MAX)
    ) {
        return false;
    }
    let mut offset = ROOT_HEADER_BYTES;
    let Some(root_type) = validate_dynamic_node(bytes, &mut offset, 0) else {
        return false;
    };
    let expected_root = match expected {
        AbiValueType::VARIANT => None,
        AbiValueType::ARRAY => Some(AbiValueType::ARRAY.0),
        AbiValueType::DICTIONARY => Some(AbiValueType::DICTIONARY.0),
        _ => return false,
    };
    expected_root.is_none_or(|expected| root_type == expected) && offset == bytes.len()
}

/// Returns the Host ownership-group token encoded in a validated dynamic root.
#[must_use]
pub fn dynamic_value_ownership_token(bytes: &[u8]) -> Option<u64> {
    validate_dynamic_value(AbiValueType::VARIANT, bytes)
        .then(|| {
            (read_dynamic_u16(bytes, 10) == Some(ABI_DYNAMIC_ROOT_OWNED))
                .then(|| read_dynamic_u64(bytes, 12))
                .flatten()
        })
        .flatten()
}

/// Visits every Host-owned Callable token nested in a validated dynamic value.
///
/// Returns `false` when the wire value is invalid or the visitor rejects a
/// token. Repeated Callable nodes are visited independently because each node
/// carries one ownership reference.
pub fn visit_dynamic_callable_tokens(bytes: &[u8], mut visitor: impl FnMut(u64) -> bool) -> bool {
    if !validate_dynamic_value(AbiValueType::VARIANT, bytes) {
        return false;
    }
    let mut offset = 20;
    visit_dynamic_callable_node(bytes, &mut offset, 0, &mut visitor) && offset == bytes.len()
}

fn visit_dynamic_callable_node(
    bytes: &[u8],
    offset: &mut usize,
    depth: usize,
    visitor: &mut impl FnMut(u64) -> bool,
) -> bool {
    if depth > ABI_DYNAMIC_MAX_DEPTH {
        return false;
    }
    let Some(header_end) = offset.checked_add(16) else {
        return false;
    };
    let Some(header) = bytes.get(*offset..header_end) else {
        return false;
    };
    let type_ = u32::from_le_bytes(header[..4].try_into().expect("u32 width"));
    let Ok(length) = usize::try_from(u64::from_le_bytes(
        header[8..16].try_into().expect("u64 width"),
    )) else {
        return false;
    };
    let payload_start = header_end;
    let Some(payload_end) = payload_start.checked_add(length) else {
        return false;
    };
    let Some(payload) = bytes.get(payload_start..payload_end) else {
        return false;
    };
    *offset = payload_end;

    if type_ == AbiValueType::CALLABLE.0 {
        return callable_value_ownership_token(payload).is_none_or(&mut *visitor);
    }
    let entries = if type_ == AbiValueType::ARRAY.0 {
        1
    } else if type_ == AbiValueType::DICTIONARY.0 {
        2
    } else {
        return true;
    };
    let Some(count_bytes) = payload.get(..8) else {
        return false;
    };
    let Ok(count) = usize::try_from(u64::from_le_bytes(
        count_bytes.try_into().expect("u64 width"),
    )) else {
        return false;
    };
    let Some(nodes) = count.checked_mul(entries) else {
        return false;
    };
    *offset = payload_start + 8;
    for _ in 0..nodes {
        if !visit_dynamic_callable_node(bytes, offset, depth + 1, visitor) {
            return false;
        }
    }
    *offset == payload_end
}

fn validate_dynamic_node(bytes: &[u8], offset: &mut usize, depth: usize) -> Option<u32> {
    const NODE_HEADER_BYTES: usize = 16;
    if depth > ABI_DYNAMIC_MAX_DEPTH {
        return None;
    }
    let header_end = offset.checked_add(NODE_HEADER_BYTES)?;
    let header = bytes.get(*offset..header_end)?;
    let type_ = u32::from_le_bytes(header[..4].try_into().ok()?);
    let flags = u32::from_le_bytes(header[4..8].try_into().ok()?);
    let length = usize::try_from(u64::from_le_bytes(header[8..16].try_into().ok()?)).ok()?;
    if flags != 0 {
        return None;
    }
    let payload_start = header_end;
    let payload_end = payload_start.checked_add(length)?;
    let payload = bytes.get(payload_start..payload_end)?;
    *offset = payload_end;

    let valid = match type_ {
        0 => payload.is_empty(),
        1 => payload.len() == 1 && payload[0] <= 1,
        2..=4 | 12 => payload.len() == 8,
        6 | 13 | 14 => core::str::from_utf8(payload).is_ok(),
        7 | 10 => payload.len() == 8,
        8 | 11 => payload.len() == 12,
        9 | 15..=20 => payload.len() == 16,
        21 | 22 => payload.len() == 24,
        23 => payload.len() == 36,
        24 => payload.len() == 48,
        25 => payload.len() == 64,
        26 => true,
        27 | 29 => payload.len() % 4 == 0,
        28 | 30 | 32 => payload.len() % 8 == 0,
        31 => validate_dynamic_packed_strings(payload),
        33 => payload.len() % 12 == 0,
        34 | 35 => payload.len() % 16 == 0,
        value if value == AbiValueType::CALLABLE.0 => validate_callable_value(payload),
        value if value == AbiValueType::SIGNAL.0 => validate_signal_value(payload),
        value if value == AbiValueType::ARRAY.0 => {
            validate_dynamic_container(bytes, payload_start, payload_end, offset, depth, false)
        }
        value if value == AbiValueType::DICTIONARY.0 => {
            validate_dynamic_container(bytes, payload_start, payload_end, offset, depth, true)
        }
        _ => false,
    };
    valid.then_some(type_)
}

fn validate_dynamic_container(
    bytes: &[u8],
    payload_start: usize,
    payload_end: usize,
    offset: &mut usize,
    depth: usize,
    dictionary: bool,
) -> bool {
    let Some(count_bytes) = bytes.get(payload_start..payload_start.saturating_add(8)) else {
        return false;
    };
    let Ok(count) = usize::try_from(u64::from_le_bytes(
        count_bytes.try_into().expect("dynamic count width"),
    )) else {
        return false;
    };
    if count > ABI_DYNAMIC_MAX_ELEMENTS {
        return false;
    }
    let values_per_entry = if dictionary { 2 } else { 1 };
    let Some(node_count) = count.checked_mul(values_per_entry) else {
        return false;
    };
    // Every value has at least one 16-byte node header. Reject impossible
    // counts before recursing so hostile input cannot amplify parser work.
    let Some(minimum_bytes) = node_count.checked_mul(16) else {
        return false;
    };
    if payload_end.saturating_sub(payload_start.saturating_add(8)) < minimum_bytes {
        return false;
    }
    *offset = payload_start + 8;
    for _ in 0..node_count {
        if validate_dynamic_node(bytes, offset, depth + 1).is_none() {
            return false;
        }
    }
    *offset == payload_end
}

fn validate_dynamic_packed_strings(bytes: &[u8]) -> bool {
    let Some(count_bytes) = bytes.get(..8) else {
        return false;
    };
    let Ok(count) = usize::try_from(u64::from_le_bytes(
        count_bytes.try_into().expect("packed string count width"),
    )) else {
        return false;
    };
    if count > ABI_DYNAMIC_MAX_ELEMENTS || bytes.len().saturating_sub(8) < count.saturating_mul(8) {
        return false;
    }
    let mut offset = 8_usize;
    for _ in 0..count {
        let Some(length_end) = offset.checked_add(8) else {
            return false;
        };
        let Some(length_bytes) = bytes.get(offset..length_end) else {
            return false;
        };
        let Ok(length) = usize::try_from(u64::from_le_bytes(
            length_bytes.try_into().expect("packed string length width"),
        )) else {
            return false;
        };
        let Some(end) = length_end.checked_add(length) else {
            return false;
        };
        let Some(text) = bytes.get(length_end..end) else {
            return false;
        };
        if core::str::from_utf8(text).is_err() {
            return false;
        }
        offset = end;
    }
    offset == bytes.len()
}

fn read_dynamic_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_dynamic_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_dynamic_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// Exact native storage consumed by Godot's ptrcall ABI.
///
/// The project ABI transports normalized values, while this descriptor
/// preserves metadata such as `int32`, `uint64`, `float`, and `double` from
/// the authenticated official API.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct AbiPtrcallType(pub u32);

impl AbiPtrcallType {
    pub const VOID: Self = Self(0);
    pub const BOOL: Self = Self(1);
    pub const I8: Self = Self(2);
    pub const I16: Self = Self(3);
    pub const I32: Self = Self(4);
    pub const I64: Self = Self(5);
    pub const U8: Self = Self(6);
    pub const U16: Self = Self(7);
    pub const U32: Self = Self(8);
    pub const U64: Self = Self(9);
    pub const F32: Self = Self(10);
    pub const F64: Self = Self(11);
    pub const OBJECT: Self = Self(12);
    pub const VECTOR2: Self = Self(13);
    pub const VECTOR3: Self = Self(14);
    pub const COLOR: Self = Self(15);
    pub const STRING: Self = Self(16);
    pub const VECTOR2I: Self = Self(17);
    pub const VECTOR3I: Self = Self(18);
    pub const RID: Self = Self(19);
    /// Return-only storage for Godot's one-pointer `Ref<T>` wrapper.
    pub const REFCOUNTED_OBJECT: Self = Self(20);
    pub const STRING_NAME: Self = Self(21);
    pub const NODE_PATH: Self = Self(22);
    pub const RECT2: Self = Self(23);
    pub const RECT2I: Self = Self(24);
    pub const QUATERNION: Self = Self(25);
    pub const PLANE: Self = Self(26);
    pub const VECTOR4: Self = Self(27);
    pub const VECTOR4I: Self = Self(28);
    pub const TRANSFORM2D: Self = Self(29);
    pub const AABB: Self = Self(30);
    pub const BASIS: Self = Self(31);
    pub const TRANSFORM3D: Self = Self(32);
    pub const PROJECTION: Self = Self(33);
    pub const PACKED_BYTE_ARRAY: Self = Self(34);
    pub const PACKED_INT32_ARRAY: Self = Self(35);
    pub const PACKED_INT64_ARRAY: Self = Self(36);
    pub const PACKED_FLOAT32_ARRAY: Self = Self(37);
    pub const PACKED_FLOAT64_ARRAY: Self = Self(38);
    pub const PACKED_STRING_ARRAY: Self = Self(39);
    pub const PACKED_VECTOR2_ARRAY: Self = Self(40);
    pub const PACKED_VECTOR3_ARRAY: Self = Self(41);
    pub const PACKED_COLOR_ARRAY: Self = Self(42);
    pub const PACKED_VECTOR4_ARRAY: Self = Self(43);
    pub const VARIANT: Self = Self(44);
    pub const ARRAY: Self = Self(45);
    pub const DICTIONARY: Self = Self(46);
    pub const CALLABLE: Self = Self(47);
    pub const SIGNAL: Self = Self(48);

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self.0, 0..=48)
    }
}

/// Borrowed method argument type list.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiValueTypeSlice {
    pub ptr: *const AbiValueType,
    pub len: usize,
}

impl AbiValueTypeSlice {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    #[must_use]
    pub const fn from_static(value: &'static [AbiValueType]) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// Fixed-size value transported between Host and project module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiValueV1 {
    pub type_: AbiValueType,
    pub reserved_flags: u32,
    pub payload: [u64; 2],
}

/// [`AbiValueV1::reserved_flags`] marks a UTF-8 buffer owned by its producer.
///
/// Project-module outputs are released through [`AbiDropValueFn`]. Host
/// outputs are released through [`AbiDropHostValueFn`].
pub const ABI_VALUE_OWNED_UTF8: u32 = 1 << 0;
/// [`AbiValueV1`] carries one Host-retained RefCounted object token.
pub const ABI_VALUE_OWNED_OBJECT_REF: u32 = 1 << 1;
/// [`AbiValueV1`] points to a Host- or module-owned fixed-layout byte buffer.
pub const ABI_VALUE_OWNED_BYTES: u32 = 1 << 2;

impl AbiValueV1 {
    pub const NIL: Self = Self {
        type_: AbiValueType::NIL,
        reserved_flags: 0,
        payload: [0; 2],
    };

    #[must_use]
    pub const fn from_bool(value: bool) -> Self {
        Self {
            type_: AbiValueType::BOOL,
            reserved_flags: 0,
            payload: [value as u64, 0],
        }
    }

    #[must_use]
    pub const fn from_i64(value: i64) -> Self {
        Self {
            type_: AbiValueType::I64,
            reserved_flags: 0,
            payload: [value as u64, 0],
        }
    }

    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self {
            type_: AbiValueType::U64,
            reserved_flags: 0,
            payload: [value, 0],
        }
    }

    #[must_use]
    pub const fn from_f64(value: f64) -> Self {
        Self {
            type_: AbiValueType::F64,
            reserved_flags: 0,
            payload: [value.to_bits(), 0],
        }
    }

    #[must_use]
    pub const fn from_object_id(value: u64) -> Self {
        Self {
            type_: AbiValueType::OBJECT_ID,
            reserved_flags: 0,
            payload: [value, 0],
        }
    }

    /// Transports the exact opaque eight-byte representation of a Godot RID.
    #[must_use]
    pub const fn from_rid(value: u64) -> Self {
        Self {
            type_: AbiValueType::RID,
            reserved_flags: 0,
            payload: [value, 0],
        }
    }

    /// Borrows one UTF-8 string for a synchronous ABI call.
    ///
    /// The producer must keep `value` alive until the consumer has copied it.
    #[must_use]
    pub fn from_borrowed_utf8(value: &str) -> Self {
        Self {
            type_: AbiValueType::STRING,
            reserved_flags: 0,
            payload: [value.as_ptr() as usize as u64, value.len() as u64],
        }
    }

    /// Borrows one UTF-8 `StringName` spelling for a synchronous ABI call.
    ///
    /// The Host interns the spelling before entering Godot's ptrcall ABI.
    #[must_use]
    pub fn from_borrowed_string_name(value: &str) -> Self {
        Self {
            type_: AbiValueType::STRING_NAME,
            reserved_flags: 0,
            payload: [value.as_ptr() as usize as u64, value.len() as u64],
        }
    }

    /// Borrows one UTF-8 `NodePath` spelling for a synchronous ABI call.
    ///
    /// The Host constructs the native Godot value before entering ptrcall.
    #[must_use]
    pub fn from_borrowed_node_path(value: &str) -> Self {
        Self {
            type_: AbiValueType::NODE_PATH,
            reserved_flags: 0,
            payload: [value.as_ptr() as usize as u64, value.len() as u64],
        }
    }

    /// Borrows fixed-layout f32 components for one synchronous ABI call.
    ///
    /// The producer must keep the component slice alive until the consumer
    /// has copied it.
    #[must_use]
    pub fn from_borrowed_f32_components(type_: AbiValueType, value: &[f32]) -> Self {
        Self {
            type_,
            reserved_flags: 0,
            payload: [
                value.as_ptr() as usize as u64,
                core::mem::size_of_val(value) as u64,
            ],
        }
    }

    /// Borrows one validated packed-array encoding for a synchronous ABI call.
    #[must_use]
    pub fn from_borrowed_bytes(type_: AbiValueType, value: &[u8]) -> Self {
        Self {
            type_,
            reserved_flags: 0,
            payload: [value.as_ptr() as usize as u64, value.len() as u64],
        }
    }

    /// Returns the raw borrowed or owned byte range for an exact value type.
    ///
    /// The caller must establish that the producer keeps the range readable
    /// for the complete copy operation.
    #[must_use]
    pub fn byte_range(self, expected: AbiValueType) -> Option<(*const u8, usize)> {
        if self.type_ != expected || !matches!(self.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES) {
            return None;
        }
        let address = usize::try_from(self.payload[0]).ok()?;
        let length = usize::try_from(self.payload[1]).ok()?;
        (address != 0).then_some((address as *const u8, length))
    }

    /// Packs one Godot-style two-dimensional vector without native padding.
    #[must_use]
    pub const fn from_vector2(x: f32, y: f32) -> Self {
        Self {
            type_: AbiValueType::VECTOR2,
            reserved_flags: 0,
            payload: [pack_f32_pair(x, y), 0],
        }
    }

    /// Packs one Godot-style three-dimensional vector without native padding.
    #[must_use]
    pub const fn from_vector3(x: f32, y: f32, z: f32) -> Self {
        Self {
            type_: AbiValueType::VECTOR3,
            reserved_flags: 0,
            payload: [pack_f32_pair(x, y), z.to_bits() as u64],
        }
    }

    /// Packs one RGBA color as four exact IEEE-754 components.
    #[must_use]
    pub const fn from_color(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::from_f32x4(AbiValueType::COLOR, r, g, b, a)
    }

    /// Packs one Godot two-dimensional integer vector.
    #[must_use]
    pub const fn from_vector2i(x: i32, y: i32) -> Self {
        Self {
            type_: AbiValueType::VECTOR2I,
            reserved_flags: 0,
            payload: [pack_i32_pair(x, y), 0],
        }
    }

    /// Packs one Godot three-dimensional integer vector.
    #[must_use]
    pub const fn from_vector3i(x: i32, y: i32, z: i32) -> Self {
        Self {
            type_: AbiValueType::VECTOR3I,
            reserved_flags: 0,
            payload: [pack_i32_pair(x, y), z as u32 as u64],
        }
    }

    /// Packs one Godot floating-point rectangle.
    #[must_use]
    pub const fn from_rect2(position_x: f32, position_y: f32, size_x: f32, size_y: f32) -> Self {
        Self::from_f32x4(AbiValueType::RECT2, position_x, position_y, size_x, size_y)
    }

    /// Packs one Godot integer rectangle.
    #[must_use]
    pub const fn from_rect2i(position_x: i32, position_y: i32, size_x: i32, size_y: i32) -> Self {
        Self::from_i32x4(AbiValueType::RECT2I, position_x, position_y, size_x, size_y)
    }

    /// Packs one Godot quaternion.
    #[must_use]
    pub const fn from_quaternion(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self::from_f32x4(AbiValueType::QUATERNION, x, y, z, w)
    }

    /// Packs one Godot plane.
    #[must_use]
    pub const fn from_plane(normal_x: f32, normal_y: f32, normal_z: f32, d: f32) -> Self {
        Self::from_f32x4(AbiValueType::PLANE, normal_x, normal_y, normal_z, d)
    }

    /// Packs one Godot four-dimensional floating-point vector.
    #[must_use]
    pub const fn from_vector4(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self::from_f32x4(AbiValueType::VECTOR4, x, y, z, w)
    }

    /// Packs one Godot four-dimensional integer vector.
    #[must_use]
    pub const fn from_vector4i(x: i32, y: i32, z: i32, w: i32) -> Self {
        Self::from_i32x4(AbiValueType::VECTOR4I, x, y, z, w)
    }

    const fn from_f32x4(
        type_: AbiValueType,
        first: f32,
        second: f32,
        third: f32,
        fourth: f32,
    ) -> Self {
        Self {
            type_,
            reserved_flags: 0,
            payload: [pack_f32_pair(first, second), pack_f32_pair(third, fourth)],
        }
    }

    const fn from_i32x4(
        type_: AbiValueType,
        first: i32,
        second: i32,
        third: i32,
        fourth: i32,
    ) -> Self {
        Self {
            type_,
            reserved_flags: 0,
            payload: [pack_i32_pair(first, second), pack_i32_pair(third, fourth)],
        }
    }

    /// Decodes a validated two-dimensional vector payload.
    #[must_use]
    pub fn vector2(self) -> Option<[f32; 2]> {
        (self.type_ == AbiValueType::VECTOR2 && self.reserved_flags == 0 && self.payload[1] == 0)
            .then(|| unpack_f32_pair(self.payload[0]))
    }

    /// Decodes a validated three-dimensional vector payload.
    #[must_use]
    pub fn vector3(self) -> Option<[f32; 3]> {
        (self.type_ == AbiValueType::VECTOR3
            && self.reserved_flags == 0
            && self.payload[1] >> 32 == 0)
            .then(|| {
                let [x, y] = unpack_f32_pair(self.payload[0]);
                [x, y, f32::from_bits(self.payload[1] as u32)]
            })
    }

    /// Decodes a validated RGBA color payload.
    #[must_use]
    pub fn color(self) -> Option<[f32; 4]> {
        self.f32x4(AbiValueType::COLOR)
    }

    /// Decodes a validated two-dimensional integer vector payload.
    #[must_use]
    pub fn vector2i(self) -> Option<[i32; 2]> {
        (self.type_ == AbiValueType::VECTOR2I && self.reserved_flags == 0 && self.payload[1] == 0)
            .then(|| unpack_i32_pair(self.payload[0]))
    }

    /// Decodes a validated three-dimensional integer vector payload.
    #[must_use]
    pub fn vector3i(self) -> Option<[i32; 3]> {
        (self.type_ == AbiValueType::VECTOR3I
            && self.reserved_flags == 0
            && self.payload[1] >> 32 == 0)
            .then(|| {
                let [x, y] = unpack_i32_pair(self.payload[0]);
                [x, y, self.payload[1] as u32 as i32]
            })
    }

    /// Decodes a validated floating-point rectangle payload.
    #[must_use]
    pub fn rect2(self) -> Option<[f32; 4]> {
        self.f32x4(AbiValueType::RECT2)
    }

    /// Decodes a validated integer rectangle payload.
    #[must_use]
    pub fn rect2i(self) -> Option<[i32; 4]> {
        self.i32x4(AbiValueType::RECT2I)
    }

    /// Decodes a validated quaternion payload.
    #[must_use]
    pub fn quaternion(self) -> Option<[f32; 4]> {
        self.f32x4(AbiValueType::QUATERNION)
    }

    /// Decodes a validated plane payload.
    #[must_use]
    pub fn plane(self) -> Option<[f32; 4]> {
        self.f32x4(AbiValueType::PLANE)
    }

    /// Decodes a validated four-dimensional floating-point vector payload.
    #[must_use]
    pub fn vector4(self) -> Option<[f32; 4]> {
        self.f32x4(AbiValueType::VECTOR4)
    }

    /// Decodes a validated four-dimensional integer vector payload.
    #[must_use]
    pub fn vector4i(self) -> Option<[i32; 4]> {
        self.i32x4(AbiValueType::VECTOR4I)
    }

    fn f32x4(self, expected: AbiValueType) -> Option<[f32; 4]> {
        (self.type_ == expected && self.reserved_flags == 0).then(|| {
            let [first, second] = unpack_f32_pair(self.payload[0]);
            let [third, fourth] = unpack_f32_pair(self.payload[1]);
            [first, second, third, fourth]
        })
    }

    fn i32x4(self, expected: AbiValueType) -> Option<[i32; 4]> {
        (self.type_ == expected && self.reserved_flags == 0).then(|| {
            let [first, second] = unpack_i32_pair(self.payload[0]);
            let [third, fourth] = unpack_i32_pair(self.payload[1]);
            [first, second, third, fourth]
        })
    }

    /// Decodes a validated opaque Godot RID representation.
    #[must_use]
    pub const fn rid(self) -> Option<u64> {
        if self.type_.0 == AbiValueType::RID.0 && self.reserved_flags == 0 && self.payload[1] == 0 {
            Some(self.payload[0])
        } else {
            None
        }
    }
}

const fn pack_f32_pair(first: f32, second: f32) -> u64 {
    first.to_bits() as u64 | ((second.to_bits() as u64) << 32)
}

fn unpack_f32_pair(value: u64) -> [f32; 2] {
    [
        f32::from_bits(value as u32),
        f32::from_bits((value >> 32) as u32),
    ]
}

const fn pack_i32_pair(first: i32, second: i32) -> u64 {
    first as u32 as u64 | ((second as u32 as u64) << 32)
}

fn unpack_i32_pair(value: u64) -> [i32; 2] {
    [value as u32 as i32, (value >> 32) as u32 as i32]
}

/// One generated argument or return-value contract for an engine ptrcall.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiGodotValueSpecV1 {
    /// Normalized type transported across the Host/project boundary.
    pub value_type: AbiValueType,
    /// Exact C storage selected by the official API's `type` and `meta`.
    pub ptrcall_type: AbiPtrcallType,
    /// Godot ClassDB name for Object values; empty for scalar values.
    pub class_name: AbiByteSlice,
    /// Must remain zero until a compatible extension is defined.
    pub reserved_flags: u32,
    /// Reserved alignment and compatible-growth fields.
    pub reserved: [usize; 2],
}

/// [`AbiGodotValueSpecV1::class_name`] stores a typed-Array element spelling.
pub const ABI_GODOT_VALUE_TYPED_ARRAY: u32 = 1 << 0;

/// [`AbiGodotMethodSpecV1`] describes a class-level method with no receiver.
pub const ABI_GODOT_METHOD_STATIC: u32 = 1 << 0;
/// [`AbiGodotMethodSpecV1`] uses Variant-call storage for trailing arguments.
pub const ABI_GODOT_METHOD_VARARG: u32 = 1 << 1;

impl AbiGodotValueSpecV1 {
    pub const NIL: Self = Self {
        value_type: AbiValueType::NIL,
        ptrcall_type: AbiPtrcallType::VOID,
        class_name: AbiByteSlice::EMPTY,
        reserved_flags: 0,
        reserved: [0; 2],
    };
}

/// Borrowed generated argument contract list.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiGodotValueSpecSlice {
    pub ptr: *const AbiGodotValueSpecV1,
    pub len: usize,
}

impl AbiGodotValueSpecSlice {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    #[must_use]
    pub const fn from_static(value: &'static [AbiGodotValueSpecV1]) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// Authenticated Godot method metadata emitted into the project SDK.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiGodotMethodSpecV1 {
    pub struct_size: u32,
    pub reserved_flags: u32,
    /// Stable hash of class, method, signature hash, and value contracts.
    pub id: u64,
    pub class_name: AbiByteSlice,
    pub method_name: AbiByteSlice,
    pub method_hash: u64,
    pub arguments: AbiGodotValueSpecSlice,
    pub return_value: AbiGodotValueSpecV1,
    pub reserved: [usize; 4],
}

impl AbiGodotMethodSpecV1 {
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// Operation selected by [`AbiGodotApiSpecV1`].
///
/// This is a transparent integer instead of a Rust enum so a newer project
/// module can be rejected cleanly by an older Host without constructing an
/// invalid enum discriminant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AbiGodotApiKind(pub u32);

impl AbiGodotApiKind {
    pub const UTILITY_FUNCTION: Self = Self(1);
    pub const BUILTIN_CONSTRUCTOR: Self = Self(2);
    pub const BUILTIN_METHOD: Self = Self(3);
    pub const BUILTIN_OPERATOR: Self = Self(4);
    pub const BUILTIN_MEMBER_GETTER: Self = Self(5);
    pub const BUILTIN_MEMBER_SETTER: Self = Self(6);
    pub const BUILTIN_INDEXED_GETTER: Self = Self(7);
    pub const BUILTIN_INDEXED_SETTER: Self = Self(8);
    pub const BUILTIN_KEYED_GETTER: Self = Self(9);
    pub const BUILTIN_KEYED_SETTER: Self = Self(10);
    pub const BUILTIN_CONSTANT: Self = Self(11);
    pub const SINGLETON: Self = Self(12);
    pub const OBJECT_CONSTRUCTOR: Self = Self(13);

    #[must_use]
    pub const fn is_supported(self) -> bool {
        self.0 >= Self::UTILITY_FUNCTION.0 && self.0 <= Self::OBJECT_CONSTRUCTOR.0
    }
}

/// [`AbiGodotApiSpecV1`] describes a static builtin method.
pub const ABI_GODOT_API_STATIC: u32 = 1 << 0;
/// [`AbiGodotApiSpecV1`] describes a const builtin method.
pub const ABI_GODOT_API_CONST: u32 = 1 << 1;
/// [`AbiGodotApiSpecV1`] accepts a trailing Variant argument slice.
pub const ABI_GODOT_API_VARARG: u32 = 1 << 2;
/// The operation updates its builtin receiver and requires `updated_base`.
pub const ABI_GODOT_API_MUTATES_BASE: u32 = 1 << 3;

/// Authenticated metadata for one non-MethodBind Godot API entry.
///
/// The same transport covers utility functions, builtin constructors,
/// methods, operators, members, indexing and keyed access, builtin constants,
/// engine singletons, and Object construction. Generated metadata fixes the
/// exact native storage contract on both sides of the Host/project boundary.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiGodotApiSpecV1 {
    pub struct_size: u32,
    pub reserved_flags: u32,
    /// Stable hash of the entry identity and every value contract.
    pub id: u64,
    pub kind: AbiGodotApiKind,
    /// Builtin/class/singleton type; empty only for utility functions.
    pub owner_name: AbiByteSlice,
    /// Function/method/member/constant/singleton name; empty for constructors
    /// and indexed/keyed access.
    pub member_name: AbiByteSlice,
    /// Official method/function hash, constructor index, or operator ordinal.
    pub numeric: u64,
    /// Receiver storage. `NIL` for operations without a builtin receiver.
    pub base_value: AbiGodotValueSpecV1,
    pub arguments: AbiGodotValueSpecSlice,
    pub return_value: AbiGodotValueSpecV1,
    pub reserved: [usize; 4],
}

impl AbiGodotApiSpecV1 {
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// ABI-safe status returned by fallible callbacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiStatus {
    /// Operation completed successfully.
    Ok = 0,
    /// Input was invalid.
    InvalidArgument = 1,
    /// Requested capability is not supported by this ABI revision.
    Unsupported = 2,
    /// A handle referred to an unloaded module generation.
    StaleHandle = 3,
    /// A mutable script call attempted synchronous re-entry.
    ReentrantCall = 4,
    /// Rust code panicked behind an ABI boundary.
    Panic = 5,
    /// Internal failure with details available through Host diagnostics.
    Internal = 6,
    /// User script callback returned an error.
    CallbackFailed = 7,
}

/// Status and optional static diagnostic returned by project callbacks.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiCallResult {
    /// Machine-readable outcome.
    pub status: AbiStatus,
    /// UTF-8 details borrowed from the module for the duration of the call.
    pub message: AbiByteSlice,
}

impl AbiCallResult {
    /// Successful callback result.
    pub const OK: Self = Self {
        status: AbiStatus::Ok,
        message: AbiByteSlice::EMPTY,
    };

    /// Builds an allocation-free user callback failure.
    #[must_use]
    pub const fn callback_failed(message: &'static str) -> Self {
        Self {
            status: AbiStatus::CallbackFailed,
            message: AbiByteSlice::from_static(message),
        }
    }

    /// Builds a result for an ABI validation or runtime failure.
    #[must_use]
    pub const fn failure(status: AbiStatus, message: &'static str) -> Self {
        Self {
            status,
            message: AbiByteSlice::from_static(message),
        }
    }
}

/// Field role in a Script Mode descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiFieldKind {
    Plain = 0,
    Export = 1,
    Node = 2,
    Signal = 3,
}

/// Field descriptor reserved slots contain a normalized Godot property schema.
pub const ABI_FIELD_EXTENSION_PROPERTY_SCHEMA: u32 = 1 << 0;
pub const ABI_FIELD_EXTENSION_SIGNAL_SCHEMA: u32 = 1 << 1;
/// Field descriptor reserved slots contain a generated node path and class.
///
/// Slots zero and one are the path pointer and byte length. Slot two is the
/// class-name pointer. Slot three stores `(class_name_length << 1) | optional`.
pub const ABI_FIELD_EXTENSION_NODE_SCHEMA: u32 = 1 << 2;

/// Field `reserved` contains generated Godot enum/bitfield metadata.
///
/// Slot zero is one for a signed enum and zero for an unsigned bitfield,
/// slots one and two contain a pointer and count for
/// [`AbiGodotIntegerOptionV1`], and slot three contains an
/// [`AbiGodotIntegerDefaultFn`] pointer.
pub const ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA: u32 = 1 << 3;
/// The field can be transported solely for module-generation migration.
///
/// This extension is used by ordinary Rust fields marked
/// `#[reload(persist)]`. Slot zero stores the corresponding [`AbiValueType`]
/// discriminant. Exported properties already carry the same information in
/// their property schema and do not set this flag.
pub const ABI_FIELD_EXTENSION_RELOAD_SCHEMA: u32 = 1 << 4;

/// One official value in a generated Godot enum or bitfield.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiGodotIntegerOptionV1 {
    pub name: AbiByteSlice,
    pub raw: u64,
}

/// Returns the raw default for one generated Godot integer property.
pub type AbiGodotIntegerDefaultFn = unsafe extern "C" fn() -> u64;
/// Low bit stored in node schema slot three for `Option<NodeRef<T>>`.
pub const ABI_NODE_FIELD_OPTIONAL: usize = 1;

/// Packs the node target class length and optional flag into ABI slot three.
#[must_use]
pub const fn encode_node_field_class(class_name_length: usize, optional: bool) -> Option<usize> {
    match class_name_length.checked_mul(2) {
        Some(length) => Some(length | optional as usize),
        None => None,
    }
}

/// Unpacks the node target class length and optional flag from ABI slot three.
#[must_use]
pub const fn decode_node_field_class(encoded: usize) -> (usize, bool) {
    (
        encoded >> 1,
        encoded & ABI_NODE_FIELD_OPTIONAL == ABI_NODE_FIELD_OPTIONAL,
    )
}

/// One named signal argument copied from a project module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiSignalArgumentDescriptorV1 {
    pub name: AbiByteSlice,
    pub type_: AbiValueType,
    pub reserved_flags: u32,
}

/// Fixed-size math default embedded in static project property metadata.
///
/// Components use native Godot order and store IEEE-754 bits so descriptor
/// equality remains exact, including signed zero and NaN payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiFixedMathDefaultV1 {
    pub struct_size: u32,
    pub component_count: u32,
    pub component_bits: [u32; 16],
    pub reserved: [usize; 2],
}

impl AbiFixedMathDefaultV1 {
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;

    #[must_use]
    pub const fn new(component_count: u32, component_bits: [u32; 16]) -> Self {
        Self {
            struct_size: Self::MINIMUM_SIZE,
            component_count,
            component_bits,
            reserved: [0; 2],
        }
    }
}

/// Godot Variant type used by an Inspector property.
///
/// This is kept separate from [`AbiValueType`]: property metadata may describe
/// types whose runtime value transport has not been implemented yet.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct AbiPropertyType(pub u32);

impl AbiPropertyType {
    pub const NIL: Self = Self(0);
    pub const BOOL: Self = Self(1);
    pub const INT: Self = Self(2);
    pub const FLOAT: Self = Self(3);
    pub const STRING: Self = Self(4);
    pub const VECTOR2: Self = Self(5);
    pub const VECTOR2I: Self = Self(6);
    pub const RECT2: Self = Self(7);
    pub const RECT2I: Self = Self(8);
    pub const VECTOR3: Self = Self(9);
    pub const VECTOR3I: Self = Self(10);
    pub const TRANSFORM2D: Self = Self(11);
    pub const VECTOR4: Self = Self(12);
    pub const VECTOR4I: Self = Self(13);
    pub const PLANE: Self = Self(14);
    pub const QUATERNION: Self = Self(15);
    pub const AABB: Self = Self(16);
    pub const BASIS: Self = Self(17);
    pub const TRANSFORM3D: Self = Self(18);
    pub const PROJECTION: Self = Self(19);
    pub const COLOR: Self = Self(20);
    pub const STRING_NAME: Self = Self(21);
    pub const NODE_PATH: Self = Self(22);
    pub const RID: Self = Self(23);
    pub const OBJECT: Self = Self(24);
    pub const CALLABLE: Self = Self(25);
    pub const SIGNAL: Self = Self(26);
    pub const DICTIONARY: Self = Self(27);
    pub const ARRAY: Self = Self(28);
    pub const PACKED_BYTE_ARRAY: Self = Self(29);
    pub const PACKED_INT32_ARRAY: Self = Self(30);
    pub const PACKED_INT64_ARRAY: Self = Self(31);
    pub const PACKED_FLOAT32_ARRAY: Self = Self(32);
    pub const PACKED_FLOAT64_ARRAY: Self = Self(33);
    pub const PACKED_STRING_ARRAY: Self = Self(34);
    pub const PACKED_VECTOR2_ARRAY: Self = Self(35);
    pub const PACKED_VECTOR3_ARRAY: Self = Self(36);
    pub const PACKED_COLOR_ARRAY: Self = Self(37);
    pub const PACKED_VECTOR4_ARRAY: Self = Self(38);

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(
            self,
            Self::NIL
                | Self::BOOL
                | Self::INT
                | Self::FLOAT
                | Self::STRING
                | Self::VECTOR2
                | Self::VECTOR2I
                | Self::RECT2
                | Self::RECT2I
                | Self::VECTOR3
                | Self::VECTOR3I
                | Self::TRANSFORM2D
                | Self::VECTOR4
                | Self::VECTOR4I
                | Self::PLANE
                | Self::QUATERNION
                | Self::AABB
                | Self::BASIS
                | Self::TRANSFORM3D
                | Self::PROJECTION
                | Self::COLOR
                | Self::STRING_NAME
                | Self::NODE_PATH
                | Self::RID
                | Self::OBJECT
                | Self::CALLABLE
                | Self::SIGNAL
                | Self::DICTIONARY
                | Self::ARRAY
                | Self::PACKED_BYTE_ARRAY
                | Self::PACKED_INT32_ARRAY
                | Self::PACKED_INT64_ARRAY
                | Self::PACKED_FLOAT32_ARRAY
                | Self::PACKED_FLOAT64_ARRAY
                | Self::PACKED_STRING_ARRAY
                | Self::PACKED_VECTOR2_ARRAY
                | Self::PACKED_VECTOR3_ARRAY
                | Self::PACKED_COLOR_ARRAY
                | Self::PACKED_VECTOR4_ARRAY
        )
    }
}

/// Godot `PropertyHint` values used by the first Inspector schema revision.
pub const ABI_PROPERTY_HINT_NONE: u32 = 0;
pub const ABI_PROPERTY_HINT_RANGE: u32 = 1;
pub const ABI_PROPERTY_HINT_ENUM: u32 = 2;
pub const ABI_PROPERTY_HINT_FLAGS: u32 = 6;
pub const ABI_PROPERTY_HINT_FILE: u32 = 13;
pub const ABI_PROPERTY_HINT_RESOURCE_TYPE: u32 = 17;
pub const ABI_PROPERTY_HINT_MULTILINE_TEXT: u32 = 18;
pub const ABI_PROPERTY_HINT_COLOR_NO_ALPHA: u32 = 21;
pub const ABI_PROPERTY_HINT_TYPE_STRING: u32 = 23;
pub const ABI_PROPERTY_HINT_ARRAY_TYPE: u32 = 31;
pub const ABI_PROPERTY_HINT_NODE_TYPE: u32 = 34;
pub const ABI_PROPERTY_HINT_DICTIONARY_TYPE: u32 = 38;

/// Godot `PropertyUsageFlags` values used by Script properties.
pub const ABI_PROPERTY_USAGE_STORAGE: u32 = 1 << 1;
pub const ABI_PROPERTY_USAGE_EDITOR: u32 = 1 << 2;
pub const ABI_PROPERTY_USAGE_GROUP: u32 = 1 << 6;
pub const ABI_PROPERTY_USAGE_SCRIPT_VARIABLE: u32 = 1 << 12;
pub const ABI_PROPERTY_USAGE_NODE_PATH_FROM_SCENE_ROOT: u32 = 1 << 22;
pub const ABI_PROPERTY_USAGE_DEFAULT: u32 = ABI_PROPERTY_USAGE_STORAGE | ABI_PROPERTY_USAGE_EDITOR;
pub const ABI_PROPERTY_USAGE_SCRIPT_DEFAULT: u32 =
    ABI_PROPERTY_USAGE_DEFAULT | ABI_PROPERTY_USAGE_SCRIPT_VARIABLE;

/// State policy used while changing module generations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiReloadPolicy {
    Default = 0,
    Persist = 1,
    Skip = 2,
}

/// One field copied out of a project module descriptor.
///
/// Each extension flag defines the meaning of the reserved slots. For the
/// ordinary property-schema extension, slot three points to a static
/// [`AbiValueV1`] scalar default. String properties keep this slot zero and
/// store their raw UTF-8 default in [`Self::default_value`]. Specialized
/// extensions document their layouts alongside their extension flags.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiFieldDescriptorV1 {
    pub struct_size: u32,
    pub reserved_extension_flags: u32,
    pub name: AbiByteSlice,
    pub rust_type: AbiByteSlice,
    pub kind: AbiFieldKind,
    pub options: AbiByteSlice,
    pub default_value: AbiByteSlice,
    pub has_default: u8,
    pub reserved_flags: [u8; 3],
    pub reload: AbiReloadPolicy,
    pub reserved: [usize; 4],
}

impl AbiFieldDescriptorV1 {
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// Method role at the Godot boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiMethodKind {
    Lifecycle = 0,
    Func = 1,
    Rpc = 2,
}

/// Well-known lifecycle slot. `None` is used by non-lifecycle methods.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiLifecycleSlot {
    None = -1,
    EnterTree = 0,
    Ready = 1,
    Process = 2,
    PhysicsProcess = 3,
    Input = 4,
    UnhandledInput = 5,
    ExitTree = 6,
}

/// Borrow kind required before invoking one script method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiReceiverKind {
    Shared = 0,
    Mutable = 1,
    Static = 2,
}

/// One named reflected method argument.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiMethodArgumentDescriptorV1 {
    pub name: AbiByteSlice,
    pub type_: AbiValueType,
    pub reserved_flags: u32,
}

/// Borrowed reflected method argument metadata.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiMethodArgumentSlice {
    pub ptr: *const AbiMethodArgumentDescriptorV1,
    pub len: usize,
}

impl AbiMethodArgumentSlice {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    #[must_use]
    pub const fn from_static(value: &'static [AbiMethodArgumentDescriptorV1]) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// Multiplayer authority rule for one RPC method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AbiRpcMode(pub u32);

impl AbiRpcMode {
    pub const AUTHORITY: Self = Self(0);
    pub const ANY_PEER: Self = Self(1);

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self.0, 0..=1)
    }
}

/// Multiplayer transport rule for one RPC method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AbiRpcTransferMode(pub u32);

impl AbiRpcTransferMode {
    pub const UNRELIABLE: Self = Self(0);
    pub const UNRELIABLE_ORDERED: Self = Self(1);
    pub const RELIABLE: Self = Self(2);

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self.0, 0..=2)
    }
}

/// Structured Godot RPC settings copied with one method descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiRpcConfigV1 {
    pub present: u8,
    pub call_local: u8,
    pub reserved_bytes: [u8; 2],
    pub mode: AbiRpcMode,
    pub transfer_mode: AbiRpcTransferMode,
    pub channel: u32,
    pub reserved_flags: u32,
}

impl AbiRpcConfigV1 {
    pub const NONE: Self = Self {
        present: 0,
        call_local: 0,
        reserved_bytes: [0; 2],
        mode: AbiRpcMode::AUTHORITY,
        transfer_mode: AbiRpcTransferMode::UNRELIABLE,
        channel: 0,
        reserved_flags: 0,
    };
}

/// Method `reserved[0..2]` contain a pointer and length for one
/// [`AbiByteSlice`] class name per argument.
pub const ABI_METHOD_EXTENSION_ARGUMENT_CLASSES: u32 = 1 << 0;

/// Method `reserved[2..4]` contain the pointer and length of the Godot class
/// returned by an object-valued method.
pub const ABI_METHOD_EXTENSION_RETURN_CLASS: u32 = 1 << 1;

/// Method `reserved[0]` points to an [`AbiMethodExtensionsV1`] and
/// `reserved[1]` contains its byte size. The remaining reserved slots are
/// zero. This layout replaces the two legacy class-name extensions when set.
pub const ABI_METHOD_EXTENSION_SCHEMA_V1: u32 = 1 << 2;

/// The reflected method accepts additional trailing Godot `Variant` values.
pub const ABI_METHOD_SCHEMA_VARARG: u32 = 1 << 0;

/// Produces one trailing reflected-method default value.
///
/// The callback must initialize `output` on success. The returned value uses
/// the same ownership rules as a reflected method result.
pub type AbiMethodDefaultFn =
    Option<unsafe extern "C" fn(output: *mut AbiValueV1) -> AbiCallResult>;

/// Borrowed callback slice used only while copying a method descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiMethodDefaultFnSlice {
    pub ptr: *const AbiMethodDefaultFn,
    pub len: usize,
}

impl AbiMethodDefaultFnSlice {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    #[must_use]
    pub const fn from_static(value: &'static [AbiMethodDefaultFn]) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// Borrowed byte-slice slice used only while copying a method descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AbiByteSliceSlice {
    pub ptr: *const AbiByteSlice,
    pub len: usize,
}

impl AbiByteSliceSlice {
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    #[must_use]
    pub const fn from_static(value: &'static [AbiByteSlice]) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// Extensible reflected-method metadata copied synchronously by the Host.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiMethodExtensionsV1 {
    pub struct_size: u32,
    pub reserved_flags: u32,
    pub argument_classes: AbiByteSliceSlice,
    pub return_class: AbiByteSlice,
    pub default_arguments: AbiMethodDefaultFnSlice,
    pub reserved: [usize; 4],
}

impl AbiMethodExtensionsV1 {
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// One method copied out of a project module descriptor.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiMethodDescriptorV1 {
    pub struct_size: u32,
    pub reserved_extension_flags: u32,
    pub id: u64,
    pub name: AbiByteSlice,
    pub rust_signature: AbiByteSlice,
    pub kind: AbiMethodKind,
    pub lifecycle: AbiLifecycleSlot,
    pub receiver: AbiReceiverKind,
    pub argument_count: u16,
    pub reserved_flags: u16,
    pub options: AbiByteSlice,
    pub argument_types: AbiValueTypeSlice,
    pub return_type: AbiValueType,
    pub reserved_value_flags: u32,
    pub arguments: AbiMethodArgumentSlice,
    pub rpc: AbiRpcConfigV1,
    pub reserved: [usize; 4],
}

impl AbiMethodDescriptorV1 {
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// Lifecycle callbacks use fixed signatures so the Host never calls a Rust ABI.
pub type AbiLifecycle0Fn = Option<unsafe extern "C" fn(state: *mut c_void) -> AbiCallResult>;
pub type AbiLifecycleF64Fn =
    Option<unsafe extern "C" fn(state: *mut c_void, value: f64) -> AbiCallResult>;
/// Input callbacks receive a Godot Object instance ID, never an engine pointer.
pub type AbiLifecycleInputFn =
    Option<unsafe extern "C" fn(state: *mut c_void, event: u64) -> AbiCallResult>;

/// Direct lifecycle slots cached by the Host for one script.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiLifecycleTableV1 {
    pub enter_tree: AbiLifecycle0Fn,
    pub ready: AbiLifecycle0Fn,
    pub process: AbiLifecycleF64Fn,
    pub physics_process: AbiLifecycleF64Fn,
    pub input: AbiLifecycleInputFn,
    pub unhandled_input: AbiLifecycleInputFn,
    pub exit_tree: AbiLifecycle0Fn,
}

impl AbiLifecycleTableV1 {
    /// Empty table used before scanning generated method descriptors.
    pub const EMPTY: Self = Self {
        enter_tree: None,
        ready: None,
        process: None,
        physics_process: None,
        input: None,
        unhandled_input: None,
        exit_tree: None,
    };
}

pub type AbiGetFieldDescriptorFn =
    Option<unsafe extern "C" fn(index: u32, output: *mut AbiFieldDescriptorV1) -> AbiStatus>;
pub type AbiGetMethodDescriptorFn =
    Option<unsafe extern "C" fn(index: u32, output: *mut AbiMethodDescriptorV1) -> AbiStatus>;
pub type AbiCreateScriptStateFn =
    Option<unsafe extern "C" fn(output: *mut *mut c_void) -> AbiCallResult>;
pub type AbiDropScriptStateFn = Option<unsafe extern "C" fn(state: *mut c_void)>;
pub type AbiCallScriptMethodFn = Option<
    unsafe extern "C" fn(
        state: *mut c_void,
        method_id: u64,
        arguments: *const AbiValueV1,
        argument_count: u32,
        output: *mut AbiValueV1,
    ) -> AbiCallResult,
>;
pub const ABI_SCRIPT_EXTENSION_FIELD_ACCESS: u32 = 1 << 0;
/// Script descriptor reserved slots contain one persistent Godot Resource UID.
pub const ABI_SCRIPT_EXTENSION_RESOURCE_UID: u32 = 1 << 1;
/// Script descriptor slots four and five contain a global class name pointer
/// and UTF-8 byte length.
pub const ABI_SCRIPT_EXTENSION_GLOBAL_CLASS: u32 = 1 << 2;
/// Script descriptor slots six and seven contain the canonical source path of
/// another Rust script extended by this script.
pub const ABI_SCRIPT_EXTENSION_BASE_SCRIPT: u32 = 1 << 3;
pub type AbiGetScriptFieldFn = Option<
    unsafe extern "C" fn(
        state: *mut c_void,
        field_index: u32,
        output: *mut AbiValueV1,
    ) -> AbiCallResult,
>;
pub type AbiSetScriptFieldFn = Option<
    unsafe extern "C" fn(state: *mut c_void, field_index: u32, value: AbiValueV1) -> AbiCallResult,
>;

/// One script descriptor copied out of the project module.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct AbiScriptDescriptorV1 {
    pub struct_size: u32,
    pub reserved_flags: u32,
    pub source_path: AbiByteSlice,
    pub name: AbiByteSlice,
    pub base: AbiByteSlice,
    pub tool: u8,
    pub reserved_bytes: [u8; 7],
    pub field_count: u32,
    pub method_count: u32,
    pub get_field: AbiGetFieldDescriptorFn,
    pub get_method: AbiGetMethodDescriptorFn,
    pub create_state: AbiCreateScriptStateFn,
    pub drop_state: AbiDropScriptStateFn,
    pub lifecycle: AbiLifecycleTableV1,
    pub call_method: AbiCallScriptMethodFn,
    pub reserved: [usize; 8],
}

impl AbiScriptDescriptorV1 {
    /// Minimum script descriptor size understood by ABI revision 1.
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// Splits one valid Resource UID across two pointer-width-independent words.
#[must_use]
pub const fn encode_resource_uid_words(uid: i64) -> Option<[usize; 2]> {
    if uid < 0 {
        return None;
    }
    let uid = uid as u64;
    Some([
        (uid & u32::MAX as u64) as usize,
        (uid >> u32::BITS) as usize,
    ])
}

/// Reassembles a Resource UID stored by [`encode_resource_uid_words`].
#[must_use]
pub fn decode_resource_uid_words(words: [usize; 2]) -> Option<i64> {
    let low = u32::try_from(words[0]).ok()?;
    let high = u32::try_from(words[1]).ok()?;
    let uid = u64::from(low) | (u64::from(high) << u32::BITS);
    (uid <= i64::MAX as u64).then_some(uid as i64)
}

pub type AbiGetScriptDescriptorFn =
    Option<unsafe extern "C" fn(index: u32, output: *mut AbiScriptDescriptorV1) -> AbiStatus>;

/// Log severity understood by the Host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum AbiLogLevel {
    /// Diagnostic details.
    Debug = 0,
    /// Informational event.
    Info = 1,
    /// Recoverable concern.
    Warning = 2,
    /// User-visible error.
    Error = 3,
}

/// Host logging callback.
pub type AbiLogFn = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        level: AbiLogLevel,
        target: AbiByteSlice,
        message: AbiByteSlice,
    ),
>;

/// Emits one generated signal from the currently executing script instance.
pub type AbiEmitSignalFn = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        signal_index: u32,
        arguments: *const AbiValueV1,
        argument_count: u32,
    ) -> AbiCallResult,
>;

/// Returns the Godot Object ID owned by the currently executing Rust script.
pub type AbiCurrentOwnerFn =
    Option<unsafe extern "C" fn(context: *mut c_void, output: *mut u64) -> AbiCallResult>;

/// Executes one generated, non-vararg Godot MethodBind through ptrcall.
pub type AbiCallGodotMethodFn = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        receiver: u64,
        method: *const AbiGodotMethodSpecV1,
        arguments: *const AbiValueV1,
        argument_count: u32,
        output: *mut AbiValueV1,
    ) -> AbiCallResult,
>;

/// Executes one generated non-MethodBind Godot API entry.
///
/// `base` is null for receiver-free operations. `updated_base` is required
/// only when [`ABI_GODOT_API_MUTATES_BASE`] is present; the Host writes an
/// owned snapshot after the call so the project module can replace its Rust
/// value without sharing engine-native layouts.
pub type AbiCallGodotApiFn = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        spec: *const AbiGodotApiSpecV1,
        base: *const AbiValueV1,
        arguments: *const AbiValueV1,
        argument_count: u32,
        output: *mut AbiValueV1,
        updated_base: *mut AbiValueV1,
    ) -> AbiCallResult,
>;

/// Releases one Host-owned dynamic ABI value returned to the project module.
pub type AbiDropHostValueFn =
    Option<unsafe extern "C" fn(context: *mut c_void, value: AbiValueV1) -> AbiStatus>;

/// Connects a one-shot Host callback to a Godot Signal.
///
/// The returned token is scoped to the exact Host context and must be polled
/// or cancelled before the project-module generation is unloaded.
pub type AbiWatchSignalFn = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        signal: AbiValueV1,
        output_token: *mut u64,
    ) -> AbiCallResult,
>;

/// Tests whether a watched Godot Signal has emitted.
pub type AbiPollSignalFn = Option<
    unsafe extern "C" fn(context: *mut c_void, token: u64, output_fired: *mut u8) -> AbiCallResult,
>;

/// Disconnects and releases a pending Godot Signal watch.
pub type AbiCancelSignalFn =
    Option<unsafe extern "C" fn(context: *mut c_void, token: u64) -> AbiStatus>;

/// Calls the first implementation of `method` above the currently executing
/// Rust script in its declared script-inheritance chain.
pub type AbiCallSuperMethodFn = Option<
    unsafe extern "C" fn(
        context: *mut c_void,
        method: AbiByteSlice,
        arguments: *const AbiValueV1,
        argument_count: u32,
        output: *mut AbiValueV1,
    ) -> AbiCallResult,
>;

/// [`HostApiV1::reserved`] slot containing [`AbiEmitSignalFn`].
pub const HOST_API_SLOT_EMIT_SIGNAL: usize = 0;
/// [`HostApiV1::reserved`] slot containing [`AbiCurrentOwnerFn`].
pub const HOST_API_SLOT_CURRENT_OWNER: usize = 1;
/// [`HostApiV1::reserved`] slot containing [`AbiCallGodotMethodFn`].
pub const HOST_API_SLOT_CALL_GODOT_METHOD: usize = 2;
/// [`HostApiV1::reserved`] slot containing [`AbiDropHostValueFn`].
pub const HOST_API_SLOT_DROP_VALUE: usize = 3;
/// Host table slot containing [`AbiRetainDynamicValueFn`].
pub const HOST_API_SLOT_RETAIN_DYNAMIC_VALUE: usize = 4;
/// Host table slot containing [`AbiRetainCallableValueFn`].
pub const HOST_API_SLOT_RETAIN_CALLABLE_VALUE: usize = 5;
/// Host table slot containing [`AbiCallGodotApiFn`].
pub const HOST_API_SLOT_CALL_GODOT_API: usize = 6;
/// Host table slot containing [`AbiWatchSignalFn`].
pub const HOST_API_SLOT_WATCH_SIGNAL: usize = 7;
/// Host table slot containing [`AbiPollSignalFn`].
pub const HOST_API_SLOT_POLL_SIGNAL: usize = 8;
/// Host table slot containing [`AbiCancelSignalFn`].
pub const HOST_API_SLOT_CANCEL_SIGNAL: usize = 9;
/// Host table slot containing [`AbiCallSuperMethodFn`].
pub const HOST_API_SLOT_CALL_SUPER_METHOD: usize = 10;

/// Adds one project-owned reference to a Host dynamic ownership group.
pub type AbiRetainDynamicValueFn =
    Option<unsafe extern "C" fn(context: *mut c_void, token: u64) -> AbiStatus>;

/// Adds one project-owned reference to a Host native Callable.
pub type AbiRetainCallableValueFn =
    Option<unsafe extern "C" fn(context: *mut c_void, token: u64) -> AbiStatus>;

/// Host functions exposed to a Script Mode project module.
#[repr(C)]
pub struct HostApiV1 {
    /// Version and table size.
    pub header: AbiHeader,
    /// Opaque Host-owned callback context.
    pub context: *mut c_void,
    /// Emits a structured Host diagnostic.
    pub log: AbiLogFn,
    /// Reserved zeroed slots for compatible growth.
    pub reserved: [usize; 16],
}

impl HostApiV1 {
    /// Minimum table size understood by this SDK.
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// Module shutdown callback.
pub type AbiModuleShutdownFn = Option<unsafe extern "C" fn(context: *mut c_void) -> AbiStatus>;

/// Releases one project-module-owned dynamic ABI value.
pub type AbiDropValueFn = Option<unsafe extern "C" fn(value: AbiValueV1) -> AbiStatus>;

/// [`ModuleApiV1::reserved_flags`] advertises owned UTF-8 values.
pub const ABI_MODULE_EXTENSION_OWNED_VALUES: u32 = 1 << 0;
/// [`ModuleApiV1::reserved_flags`] advertises the minimum Godot API target.
pub const ABI_MODULE_EXTENSION_GODOT_API: u32 = 1 << 1;
/// Module table exposes a main-thread cooperative task poll callback.
pub const ABI_MODULE_EXTENSION_TASKS: u32 = 1 << 2;
/// [`ModuleApiV1::reserved`] slot containing [`AbiDropValueFn`].
pub const MODULE_API_SLOT_DROP_VALUE: usize = 0;
/// [`ModuleApiV1::reserved`] slot containing the minimum Godot API major.
pub const MODULE_API_SLOT_GODOT_API_MAJOR: usize = 1;
/// [`ModuleApiV1::reserved`] slot containing the minimum Godot API minor.
pub const MODULE_API_SLOT_GODOT_API_MINOR: usize = 2;
/// Module table slot containing [`AbiPollTasksFn`].
pub const MODULE_API_SLOT_POLL_TASKS: usize = 3;
/// Module table slot containing [`AbiCancelTasksFn`].
pub const MODULE_API_SLOT_CANCEL_TASKS: usize = 4;
/// Polls project futures once at the Godot frame safe point.
pub type AbiPollTasksFn = Option<unsafe extern "C" fn() -> AbiStatus>;
/// Cancels every cooperative task before a generation becomes inactive.
pub type AbiCancelTasksFn = Option<unsafe extern "C" fn() -> AbiStatus>;

/// Project module table returned to the Host.
#[repr(C)]
pub struct ModuleApiV1 {
    /// Version and table size.
    pub header: AbiHeader,
    /// Opaque project-module-owned context.
    pub context: *mut c_void,
    /// Graceful shutdown callback called before unloading a generation.
    pub shutdown: AbiModuleShutdownFn,
    /// Number of scripts compiled into this project generation.
    pub script_count: u32,
    /// Reserved alignment and future flags.
    pub reserved_flags: u32,
    /// Copies one script descriptor into Host-owned memory.
    pub get_script: AbiGetScriptDescriptorFn,
    /// Reserved zeroed slots for compatible growth.
    pub reserved: [usize; 13],
}

impl ModuleApiV1 {
    /// Minimum table size understood by this Host.
    pub const MINIMUM_SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// Symbol exported by every Script Mode project module.
pub type AbiModuleEntryFn =
    unsafe extern "C" fn(host: *const HostApiV1, module: *mut ModuleApiV1) -> AbiStatus;

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use core::mem::{align_of, size_of};
    use std::vec::Vec;

    #[test]
    fn header_rejects_incompatible_values() {
        let valid = AbiHeader::new(size_of::<HostApiV1>() as u32);
        assert!(valid.is_compatible(HostApiV1::MINIMUM_SIZE, ABI_MINOR));

        let mut invalid = valid;
        invalid.magic = [0; 8];
        assert!(!invalid.is_compatible(HostApiV1::MINIMUM_SIZE, ABI_MINOR));

        let mut invalid = valid;
        invalid.abi_major += 1;
        assert!(!invalid.is_compatible(HostApiV1::MINIMUM_SIZE, ABI_MINOR));

        let mut invalid = valid;
        invalid.struct_size -= 1;
        assert!(!invalid.is_compatible(HostApiV1::MINIMUM_SIZE, ABI_MINOR));
    }

    #[test]
    fn tables_have_c_pointer_alignment() {
        assert_eq!(align_of::<HostApiV1>(), align_of::<usize>());
        assert_eq!(align_of::<ModuleApiV1>(), align_of::<usize>());
        assert!(size_of::<HostApiV1>() >= size_of::<AbiHeader>());
        assert!(size_of::<ModuleApiV1>() >= size_of::<AbiHeader>());
        assert_eq!(size_of::<AbiGetScriptFieldFn>(), size_of::<usize>());
        assert_eq!(size_of::<AbiSetScriptFieldFn>(), size_of::<usize>());
        assert_eq!(size_of::<AbiEmitSignalFn>(), size_of::<usize>());
        assert_eq!(size_of::<AbiCurrentOwnerFn>(), size_of::<usize>());
        assert_eq!(size_of::<AbiCallGodotMethodFn>(), size_of::<usize>());
        assert_eq!(size_of::<AbiDropHostValueFn>(), size_of::<usize>());
        assert_eq!(size_of::<AbiDropValueFn>(), size_of::<usize>());
    }

    #[test]
    fn byte_slice_is_two_machine_words() {
        assert_eq!(size_of::<AbiByteSlice>(), size_of::<usize>() * 2);
        assert_eq!(size_of::<AbiValueTypeSlice>(), size_of::<usize>() * 2);
        assert_eq!(size_of::<AbiMethodArgumentSlice>(), size_of::<usize>() * 2);
    }

    #[test]
    fn godot_integer_option_has_stable_c_layout() {
        assert_eq!(
            align_of::<AbiGodotIntegerOptionV1>(),
            align_of::<AbiByteSlice>().max(align_of::<u64>())
        );
        assert!(size_of::<AbiGodotIntegerOptionV1>() >= size_of::<AbiByteSlice>() + 8);

        let option = AbiGodotIntegerOptionV1 {
            name: AbiByteSlice::from_static("PROCESS_MODE_ALWAYS"),
            raw: 3,
        };
        assert_eq!(option.name.len, 19);
        assert_eq!(option.raw, 3);
    }

    #[test]
    fn fixed_math_defaults_have_versioned_exact_bit_storage() {
        let value = AbiFixedMathDefaultV1::new(
            2,
            [
                (-0.0_f32).to_bits(),
                f32::NAN.to_bits(),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        );
        assert_eq!(value.struct_size, AbiFixedMathDefaultV1::MINIMUM_SIZE);
        assert_eq!(value.component_count, 2);
        assert_eq!(value.component_bits[0], (-0.0_f32).to_bits());
        assert_eq!(value.component_bits[1], f32::NAN.to_bits());
        assert_eq!(value.reserved, [0; 2]);
    }

    #[test]
    fn script_descriptor_uses_only_c_abi_fields() {
        assert_eq!(
            AbiScriptDescriptorV1::MINIMUM_SIZE as usize,
            size_of::<AbiScriptDescriptorV1>()
        );
        assert_eq!(align_of::<AbiScriptDescriptorV1>(), align_of::<usize>());
        assert!(size_of::<AbiLifecycleTableV1>() >= size_of::<usize>() * 7);
        assert_eq!(
            AbiFieldDescriptorV1::MINIMUM_SIZE as usize,
            size_of::<AbiFieldDescriptorV1>()
        );
        assert_eq!(
            AbiMethodDescriptorV1::MINIMUM_SIZE as usize,
            size_of::<AbiMethodDescriptorV1>()
        );
    }

    #[test]
    fn resource_uid_values_round_trip_through_reserved_words() {
        let uid = 6_145_787_935_417_040_623;
        let words = encode_resource_uid_words(uid).expect("valid UID");
        assert_eq!(decode_resource_uid_words(words), Some(uid));
        assert_eq!(encode_resource_uid_words(-1), None);
    }

    #[test]
    fn static_diagnostics_are_borrowed_without_allocation() {
        let result = AbiCallResult::callback_failed("broken callback");
        assert_eq!(result.status, AbiStatus::CallbackFailed);
        assert_eq!(result.message.len, 15);
        assert!(!result.message.ptr.is_null());
    }

    #[test]
    fn reflected_values_have_fixed_c_layout_and_round_trip_bits() {
        assert_eq!(size_of::<AbiValueType>(), size_of::<u32>());
        assert_eq!(align_of::<AbiValueV1>(), align_of::<u64>());
        assert_eq!(size_of::<AbiValueV1>(), 24);

        let integer = AbiValueV1::from_i64(-42);
        assert_eq!(integer.type_, AbiValueType::I64);
        assert_eq!(integer.payload[0] as i64, -42);

        let float = AbiValueV1::from_f64(12.5);
        assert_eq!(float.type_, AbiValueType::F64);
        assert_eq!(f64::from_bits(float.payload[0]), 12.5);
        assert_eq!(AbiValueV1::from_u64(u64::MAX).payload[0], u64::MAX);
        assert!(AbiValueType::OBJECT_ID.is_supported());
        assert!(AbiValueType::U64.is_supported());
        assert!(!AbiValueType(99).is_supported());
        assert!(AbiPtrcallType::VECTOR2.is_supported());
        assert!(AbiPtrcallType::VECTOR3.is_supported());
        assert!(AbiPtrcallType::COLOR.is_supported());
        assert!(AbiPtrcallType::STRING.is_supported());
        assert!(AbiPtrcallType::STRING_NAME.is_supported());
        assert!(AbiPtrcallType::NODE_PATH.is_supported());
        assert!(AbiPtrcallType::VECTOR2I.is_supported());
        assert!(AbiPtrcallType::VECTOR3I.is_supported());
        assert!(AbiPtrcallType::VECTOR4.is_supported());
        assert!(AbiPtrcallType::VECTOR4I.is_supported());
        assert!(AbiPtrcallType::RECT2.is_supported());
        assert!(AbiPtrcallType::RECT2I.is_supported());
        assert!(AbiPtrcallType::QUATERNION.is_supported());
        assert!(AbiPtrcallType::PLANE.is_supported());
        assert!(AbiPtrcallType::RID.is_supported());
        assert!(AbiPtrcallType::REFCOUNTED_OBJECT.is_supported());
        assert!(!AbiPtrcallType(99).is_supported());
        assert!(AbiRpcMode::ANY_PEER.is_supported());
        assert!(AbiRpcTransferMode::RELIABLE.is_supported());
    }

    #[test]
    fn borrowed_utf8_values_preserve_pointer_and_length() {
        let text = "你好，Godot";
        let value = AbiValueV1::from_borrowed_utf8(text);
        assert_eq!(value.type_, AbiValueType::STRING);
        assert_eq!(value.reserved_flags, 0);
        assert_eq!(value.payload[0], text.as_ptr() as usize as u64);
        assert_eq!(value.payload[1], text.len() as u64);

        let path = AbiValueV1::from_borrowed_node_path("../玩家/%武器");
        assert_eq!(path.type_, AbiValueType::NODE_PATH);
        assert_eq!(path.reserved_flags, 0);
        assert_eq!(path.payload[0], "../玩家/%武器".as_ptr() as usize as u64);
        assert_eq!(path.payload[1], "../玩家/%武器".len() as u64);
    }

    #[test]
    fn math_values_round_trip_exact_components() {
        let vector2 = AbiValueV1::from_vector2(-1.25, 2.5);
        assert_eq!(vector2.vector2(), Some([-1.25, 2.5]));
        assert_eq!(vector2.payload[1], 0);

        let vector3 = AbiValueV1::from_vector3(1.0, -2.0, 3.5);
        assert_eq!(vector3.vector3(), Some([1.0, -2.0, 3.5]));
        assert_eq!(vector3.payload[1] >> 32, 0);

        let color = AbiValueV1::from_color(0.1, 0.2, 0.3, 0.4);
        assert_eq!(color.color(), Some([0.1, 0.2, 0.3, 0.4]));

        let vector2i = AbiValueV1::from_vector2i(i32::MIN, i32::MAX);
        assert_eq!(vector2i.vector2i(), Some([i32::MIN, i32::MAX]));
        let vector3i = AbiValueV1::from_vector3i(-1, 2, i32::MIN);
        assert_eq!(vector3i.vector3i(), Some([-1, 2, i32::MIN]));
        assert_eq!(
            AbiValueV1::from_vector4(1.0, 2.0, 3.0, 4.0).vector4(),
            Some([1.0, 2.0, 3.0, 4.0])
        );
        assert_eq!(
            AbiValueV1::from_vector4i(-1, 2, -3, 4).vector4i(),
            Some([-1, 2, -3, 4])
        );
        assert_eq!(
            AbiValueV1::from_rect2(-1.0, 2.0, 30.0, 40.0).rect2(),
            Some([-1.0, 2.0, 30.0, 40.0])
        );
        assert_eq!(
            AbiValueV1::from_rect2i(-1, 2, 30, 40).rect2i(),
            Some([-1, 2, 30, 40])
        );
        assert_eq!(
            AbiValueV1::from_quaternion(0.0, 0.5, 0.0, 0.75).quaternion(),
            Some([0.0, 0.5, 0.0, 0.75])
        );
        assert_eq!(
            AbiValueV1::from_plane(0.0, 1.0, 0.0, 12.0).plane(),
            Some([0.0, 1.0, 0.0, 12.0])
        );

        let mut malformed = vector3;
        malformed.payload[1] |= 1_u64 << 63;
        assert_eq!(malformed.vector3(), None);
        assert!(AbiValueType::VECTOR2.is_supported());
        assert!(AbiValueType::VECTOR3.is_supported());
        assert!(AbiValueType::COLOR.is_supported());
        assert!(AbiValueType::VECTOR2I.is_supported());
        assert!(AbiValueType::VECTOR3I.is_supported());
        assert!(AbiValueType::VECTOR4.is_supported());
        assert!(AbiValueType::VECTOR4I.is_supported());
        assert!(AbiValueType::RECT2.is_supported());
        assert!(AbiValueType::RECT2I.is_supported());
        assert!(AbiValueType::QUATERNION.is_supported());
        assert!(AbiValueType::PLANE.is_supported());
    }

    #[test]
    fn rid_values_preserve_all_opaque_bits() {
        let rid = AbiValueV1::from_rid(u64::MAX);
        assert_eq!(rid.rid(), Some(u64::MAX));
        assert!(AbiValueType::RID.is_supported());

        let mut malformed = rid;
        malformed.payload[1] = 1;
        assert_eq!(malformed.rid(), None);
    }

    #[test]
    fn callable_wire_rejects_forged_ownership_and_noncanonical_text() {
        fn callable(flags: u16, token: u64, method: &[u8]) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&ABI_CALLABLE_MAGIC);
            bytes.extend_from_slice(&ABI_CALLABLE_VERSION.to_le_bytes());
            bytes.extend_from_slice(&flags.to_le_bytes());
            bytes.extend_from_slice(&token.to_le_bytes());
            bytes.extend_from_slice(&42_u64.to_le_bytes());
            bytes.extend_from_slice(&(method.len() as u32).to_le_bytes());
            bytes.extend_from_slice(method);
            bytes
        }

        let standard = callable(0, 0, b"_ready");
        assert!(validate_callable_value(&standard));
        assert_eq!(callable_value_ownership_token(&standard), None);

        let owned = callable(ABI_CALLABLE_OWNED, 7, "玩家回调".as_bytes());
        assert!(validate_callable_value(&owned));
        assert_eq!(callable_value_ownership_token(&owned), Some(7));
        assert!(!validate_callable_value(&callable(0, 7, b"_ready")));
        assert!(!validate_callable_value(&callable(
            ABI_CALLABLE_OWNED,
            0,
            b"_ready"
        )));
        assert!(!validate_callable_value(&callable(2, 7, b"_ready")));
        assert!(!validate_callable_value(&callable(0, 0, b"bad\0method")));

        let mut trailing = standard;
        trailing.push(0);
        assert!(!validate_callable_value(&trailing));
    }

    #[test]
    fn signal_wire_requires_a_canonical_object_and_name_pair() {
        fn signal(object_id: u64, name: &[u8]) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&ABI_SIGNAL_MAGIC);
            bytes.extend_from_slice(&ABI_SIGNAL_VERSION.to_le_bytes());
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&object_id.to_le_bytes());
            bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
            bytes.extend_from_slice(name);
            bytes
        }

        assert!(validate_signal_value(&signal(0, b"")));
        assert!(validate_signal_value(&signal(42, "已完成".as_bytes())));
        assert!(!validate_signal_value(&signal(0, b"ready")));
        assert!(!validate_signal_value(&signal(42, b"")));
        assert!(!validate_signal_value(&signal(42, b"bad\0name")));

        let mut wrong_version = signal(42, b"ready");
        wrong_version[8..10].copy_from_slice(&2_u16.to_le_bytes());
        assert!(!validate_signal_value(&wrong_version));

        let mut trailing = signal(42, b"ready");
        trailing.push(0);
        assert!(!validate_signal_value(&trailing));
        assert!(AbiValueType::SIGNAL.is_supported());
        assert!(AbiPtrcallType::SIGNAL.is_supported());
    }

    #[test]
    fn dynamic_wire_accepts_a_nested_canonical_signal() {
        let mut signal = Vec::new();
        signal.extend_from_slice(&ABI_SIGNAL_MAGIC);
        signal.extend_from_slice(&ABI_SIGNAL_VERSION.to_le_bytes());
        signal.extend_from_slice(&0_u16.to_le_bytes());
        signal.extend_from_slice(&42_u64.to_le_bytes());
        signal.extend_from_slice(&5_u32.to_le_bytes());
        signal.extend_from_slice(b"ready");

        let mut node = Vec::new();
        node.extend_from_slice(&AbiValueType::SIGNAL.0.to_le_bytes());
        node.extend_from_slice(&0_u32.to_le_bytes());
        node.extend_from_slice(&(signal.len() as u64).to_le_bytes());
        node.extend_from_slice(&signal);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ABI_DYNAMIC_MAGIC);
        bytes.extend_from_slice(&ABI_DYNAMIC_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&node);
        assert!(validate_dynamic_value(AbiValueType::VARIANT, &bytes));
    }

    #[test]
    fn dynamic_callable_visitor_preserves_each_nested_ownership_reference() {
        fn callable(token: u64) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&ABI_CALLABLE_MAGIC);
            bytes.extend_from_slice(&ABI_CALLABLE_VERSION.to_le_bytes());
            bytes.extend_from_slice(&ABI_CALLABLE_OWNED.to_le_bytes());
            bytes.extend_from_slice(&token.to_le_bytes());
            bytes.extend_from_slice(&42_u64.to_le_bytes());
            bytes.extend_from_slice(&6_u32.to_le_bytes());
            bytes.extend_from_slice(b"_ready");
            bytes
        }

        fn node(type_: AbiValueType, payload: &[u8]) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&type_.0.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            bytes.extend_from_slice(payload);
            bytes
        }

        let callable = node(AbiValueType::CALLABLE, &callable(7));
        let mut array_payload = Vec::new();
        array_payload.extend_from_slice(&2_u64.to_le_bytes());
        array_payload.extend_from_slice(&callable);
        array_payload.extend_from_slice(&callable);
        let root = node(AbiValueType::ARRAY, &array_payload);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ABI_DYNAMIC_MAGIC);
        bytes.extend_from_slice(&ABI_DYNAMIC_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&root);

        let mut tokens = Vec::new();
        assert!(visit_dynamic_callable_tokens(&bytes, |token| {
            tokens.push(token);
            true
        }));
        assert_eq!(tokens, [7, 7]);

        let mut visits = 0;
        assert!(!visit_dynamic_callable_tokens(&bytes, |_| {
            visits += 1;
            false
        }));
        assert_eq!(visits, 1);
    }

    #[test]
    fn generated_method_contract_layout_is_pointer_width_stable() {
        assert_eq!(size_of::<AbiPtrcallType>(), size_of::<u32>());
        assert_eq!(align_of::<AbiGodotValueSpecV1>(), align_of::<usize>());
        assert_eq!(align_of::<AbiGodotMethodSpecV1>(), align_of::<usize>());
        assert!(AbiPtrcallType::OBJECT.is_supported());
        assert!(AbiPtrcallType::REFCOUNTED_OBJECT.is_supported());
        assert_eq!(ABI_VALUE_OWNED_UTF8 & ABI_VALUE_OWNED_OBJECT_REF, 0);
        assert!(!AbiPtrcallType(99).is_supported());
        assert_eq!(
            AbiGodotMethodSpecV1::MINIMUM_SIZE as usize,
            size_of::<AbiGodotMethodSpecV1>()
        );
        assert_eq!(size_of::<AbiGodotApiKind>(), size_of::<u32>());
        assert_eq!(align_of::<AbiGodotApiSpecV1>(), align_of::<usize>());
        assert_eq!(
            AbiGodotApiSpecV1::MINIMUM_SIZE as usize,
            size_of::<AbiGodotApiSpecV1>()
        );
        assert!(AbiGodotApiKind::UTILITY_FUNCTION.is_supported());
        assert!(AbiGodotApiKind::OBJECT_CONSTRUCTOR.is_supported());
        assert!(!AbiGodotApiKind(0).is_supported());
        assert!(!AbiGodotApiKind(14).is_supported());
        assert_eq!(
            ABI_GODOT_API_STATIC
                | ABI_GODOT_API_CONST
                | ABI_GODOT_API_VARARG
                | ABI_GODOT_API_MUTATES_BASE,
            0b1111
        );
    }

    #[test]
    fn reflected_method_extensions_have_a_bounded_c_layout() {
        assert_eq!(
            size_of::<AbiMethodDefaultFn>(),
            size_of::<*const core::ffi::c_void>()
        );
        assert_eq!(size_of::<AbiMethodDefaultFnSlice>(), size_of::<usize>() * 2);
        assert_eq!(size_of::<AbiByteSliceSlice>(), size_of::<usize>() * 2);
        assert_eq!(
            size_of::<AbiMethodExtensionsV1>(),
            size_of::<u32>() * 2 + size_of::<usize>() * 10
        );
        assert_eq!(
            AbiMethodExtensionsV1::MINIMUM_SIZE as usize,
            size_of::<AbiMethodExtensionsV1>()
        );
        assert_eq!(align_of::<AbiMethodExtensionsV1>(), align_of::<usize>());
        assert_eq!(
            ABI_METHOD_EXTENSION_SCHEMA_V1
                & (ABI_METHOD_EXTENSION_ARGUMENT_CLASSES | ABI_METHOD_EXTENSION_RETURN_CLASS),
            0
        );
    }

    #[test]
    fn node_field_class_length_and_optionality_share_one_slot() {
        let required = encode_node_field_class(12, false).expect("required node");
        let optional = encode_node_field_class(12, true).expect("optional node");
        assert_eq!(decode_node_field_class(required), (12, false));
        assert_eq!(decode_node_field_class(optional), (12, true));
        assert_eq!(optional, required | ABI_NODE_FIELD_OPTIONAL);
        assert!(encode_node_field_class(usize::MAX, false).is_none());
    }
}
