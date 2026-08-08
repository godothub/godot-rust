use godot_rs_api::abi::{
    ABI_DYNAMIC_MAGIC, ABI_DYNAMIC_VERSION, ABI_VALUE_OWNED_BYTES, ABI_VALUE_OWNED_OBJECT_REF,
    ABI_VALUE_OWNED_UTF8, AbiDropValueFn, AbiStatus, AbiValueType, AbiValueV1,
    validate_callable_value, validate_dynamic_value, validate_signal_value,
};

use crate::module_loader::{ModuleCallError, ModuleGeneration};

const MAX_VALUE_TEXT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct ModuleValueOwner {
    _generation: ModuleGeneration,
    drop_value: AbiDropValueFn,
}

pub(crate) struct ModuleValue {
    owner: ModuleValueOwner,
    value: AbiValueV1,
}

impl ModuleValueOwner {
    pub(crate) fn new(generation: ModuleGeneration, drop_value: AbiDropValueFn) -> Self {
        Self {
            _generation: generation,
            drop_value,
        }
    }

    pub(crate) fn engine_call_context(&self) -> &crate::engine_call::EngineCallContext {
        self._generation.engine_call_context()
    }

    pub(crate) fn output(
        &self,
        expected: AbiValueType,
        value: AbiValueV1,
    ) -> Result<ModuleValue, ModuleCallError> {
        let value = ModuleValue {
            owner: self.clone(),
            value,
        };
        value.validate(expected, TextOwnership::Required)?;
        Ok(value)
    }

    pub(crate) fn signal(
        &self,
        expected: AbiValueType,
        value: AbiValueV1,
    ) -> Result<ModuleValue, ModuleCallError> {
        let value = ModuleValue {
            owner: self.clone(),
            value,
        };
        value.validate(expected, TextOwnership::BorrowedOrOwned)?;
        Ok(value)
    }
}

impl ModuleValue {
    pub(crate) fn abi(&self) -> AbiValueV1 {
        self.value
    }

    pub(crate) fn borrowed_abi(&self) -> AbiValueV1 {
        let mut value = self.value;
        if matches!(
            value.reserved_flags,
            ABI_VALUE_OWNED_UTF8 | ABI_VALUE_OWNED_BYTES
        ) {
            value.reserved_flags = 0;
        }
        value
    }

    pub(crate) fn engine_call_context(&self) -> &crate::engine_call::EngineCallContext {
        self.owner.engine_call_context()
    }

    fn validate(
        &self,
        expected: AbiValueType,
        text_ownership: TextOwnership,
    ) -> Result<(), ModuleCallError> {
        if self.value.type_ != expected || !valid_payload(self.value, text_ownership) {
            return Err(ModuleCallError {
                status: AbiStatus::Internal,
                message: "project module returned a value that violates its descriptor".into(),
            });
        }
        Ok(())
    }
}

impl Drop for ModuleValue {
    fn drop(&mut self) {
        if !matches!(
            self.value.reserved_flags,
            ABI_VALUE_OWNED_UTF8 | ABI_VALUE_OWNED_BYTES
        ) {
            return;
        }
        let Some(drop_value) = self.owner.drop_value else {
            host_eprintln!("godot-rust could not release a project-module-owned dynamic value");
            return;
        };
        // SAFETY: The value remains owned by this retained module generation
        // and this Drop consumes it exactly once before the library unloads.
        let status = unsafe { drop_value(self.value) };
        if status != AbiStatus::Ok {
            host_eprintln!("godot-rust project module rejected its owned dynamic value release");
        }
    }
}

#[derive(Clone, Copy)]
enum TextOwnership {
    Required,
    BorrowedOrOwned,
}

pub(crate) fn validate_input(
    expected: AbiValueType,
    value: AbiValueV1,
) -> Result<(), ModuleCallError> {
    if value.type_ != expected || !valid_payload(value, TextOwnership::BorrowedOrOwned) {
        return Err(ModuleCallError {
            status: AbiStatus::InvalidArgument,
            message: "Host value does not match the project field or method descriptor".into(),
        });
    }
    if matches!(
        value.type_,
        AbiValueType::STRING
            | AbiValueType::STRING_NAME
            | AbiValueType::NODE_PATH
            | AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION
            | AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY
            | AbiValueType::VARIANT
            | AbiValueType::ARRAY
            | AbiValueType::DICTIONARY
    ) && value.reserved_flags != 0
    {
        return Err(ModuleCallError {
            status: AbiStatus::InvalidArgument,
            message: "Host dynamic input values must be borrowed for the synchronous project call"
                .into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_module_output(
    expected: AbiValueType,
    value: AbiValueV1,
) -> Result<(), ModuleCallError> {
    if value.type_ != expected || !valid_payload(value, TextOwnership::Required) {
        return Err(ModuleCallError {
            status: AbiStatus::Internal,
            message: "project module returned a value that violates its descriptor".into(),
        });
    }
    Ok(())
}

pub(crate) fn copy_descriptor_value(
    expected: AbiValueType,
    value: AbiValueV1,
) -> Result<HostValue, ModuleCallError> {
    if matches!(expected, AbiValueType::STRING | AbiValueType::STRING_NAME)
        || value.type_ != expected
        || !valid_payload(value, TextOwnership::BorrowedOrOwned)
    {
        return Err(ModuleCallError {
            status: AbiStatus::Internal,
            message: "project descriptor contains an invalid default value".into(),
        });
    }
    Ok(HostValue::Scalar(value))
}

pub(crate) fn copy_fixed_math_descriptor(
    expected: AbiValueType,
    component_bits: &[u32],
) -> Result<HostValue, ModuleCallError> {
    let expected_count = match expected {
        AbiValueType::TRANSFORM2D | AbiValueType::AABB => 6,
        AbiValueType::BASIS => 9,
        AbiValueType::TRANSFORM3D => 12,
        AbiValueType::PROJECTION => 16,
        _ => {
            return Err(ModuleCallError {
                status: AbiStatus::Internal,
                message: "project descriptor uses fixed math storage for an invalid type".into(),
            });
        }
    };
    if component_bits.len() != expected_count {
        return Err(ModuleCallError {
            status: AbiStatus::Internal,
            message: "project descriptor contains an invalid fixed math component count".into(),
        });
    }
    Ok(HostValue::FixedMath {
        type_: expected,
        components: component_bits
            .iter()
            .copied()
            .map(f32::from_bits)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HostValue {
    Scalar(AbiValueV1),
    FixedMath {
        type_: AbiValueType,
        components: Box<[f32]>,
    },
    String(String),
    StringName(String),
    NodePath(String),
    Bytes {
        type_: AbiValueType,
        bytes: Box<[u8]>,
    },
}

impl HostValue {
    pub(crate) fn abi(&self) -> AbiValueV1 {
        match self {
            Self::Scalar(value) => *value,
            Self::FixedMath { type_, components } => {
                AbiValueV1::from_borrowed_f32_components(*type_, components)
            }
            Self::String(value) => AbiValueV1::from_borrowed_utf8(value),
            Self::StringName(value) => AbiValueV1::from_borrowed_string_name(value),
            Self::NodePath(value) => AbiValueV1::from_borrowed_node_path(value),
            Self::Bytes { type_, bytes } => AbiValueV1::from_borrowed_bytes(*type_, bytes),
        }
    }
}

pub(crate) fn empty_property_value(type_: AbiValueType) -> Option<HostValue> {
    if matches!(
        type_,
        AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY
    ) {
        let bytes = if type_ == AbiValueType::PACKED_STRING_ARRAY {
            vec![0_u8; 8].into_boxed_slice()
        } else {
            Box::default()
        };
        return Some(HostValue::Bytes { type_, bytes });
    }
    let node_type = match type_ {
        AbiValueType::ARRAY => 37_u32,
        AbiValueType::DICTIONARY => 38_u32,
        _ => return None,
    };
    let mut bytes = Vec::with_capacity(44);
    bytes.extend_from_slice(&ABI_DYNAMIC_MAGIC);
    bytes.extend_from_slice(&ABI_DYNAMIC_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&node_type.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&8_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    debug_assert!(validate_dynamic_value(type_, &bytes));
    Some(HostValue::Bytes {
        type_,
        bytes: bytes.into_boxed_slice(),
    })
}

fn valid_payload(value: AbiValueV1, text_ownership: TextOwnership) -> bool {
    match value.type_ {
        AbiValueType::NIL => value.reserved_flags == 0 && value.payload == [0; 2],
        AbiValueType::BOOL => {
            value.reserved_flags == 0 && value.payload[0] <= 1 && value.payload[1] == 0
        }
        AbiValueType::I64 | AbiValueType::U64 | AbiValueType::F64 | AbiValueType::RID => {
            value.reserved_flags == 0 && value.payload[1] == 0
        }
        AbiValueType::OBJECT_ID => {
            (value.reserved_flags == 0 && value.payload[1] == 0)
                || (value.reserved_flags == ABI_VALUE_OWNED_OBJECT_REF
                    && value.payload[0] != 0
                    && value.payload[1] != 0)
        }
        AbiValueType::VECTOR2 => value.reserved_flags == 0 && value.payload[1] == 0,
        AbiValueType::VECTOR3 => {
            value.reserved_flags == 0 && value.payload[1] & 0xffff_ffff_0000_0000 == 0
        }
        AbiValueType::COLOR => value.reserved_flags == 0,
        AbiValueType::RECT2
        | AbiValueType::RECT2I
        | AbiValueType::QUATERNION
        | AbiValueType::PLANE
        | AbiValueType::VECTOR4
        | AbiValueType::VECTOR4I => value.reserved_flags == 0,
        AbiValueType::VECTOR2I => value.reserved_flags == 0 && value.payload[1] == 0,
        AbiValueType::VECTOR3I => {
            value.reserved_flags == 0 && value.payload[1] & 0xffff_ffff_0000_0000 == 0
        }
        AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH => {
            let valid_ownership = match text_ownership {
                TextOwnership::Required => value.reserved_flags == ABI_VALUE_OWNED_UTF8,
                TextOwnership::BorrowedOrOwned => {
                    matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_UTF8)
                }
            };
            valid_ownership && utf8(&value).is_ok()
        }
        AbiValueType::TRANSFORM2D
        | AbiValueType::AABB
        | AbiValueType::BASIS
        | AbiValueType::TRANSFORM3D
        | AbiValueType::PROJECTION => {
            let expected_length = match value.type_ {
                AbiValueType::TRANSFORM2D | AbiValueType::AABB => 6 * 4,
                AbiValueType::BASIS => 9 * 4,
                AbiValueType::TRANSFORM3D => 12 * 4,
                AbiValueType::PROJECTION => 16 * 4,
                _ => unreachable!(),
            };
            let valid_ownership = match text_ownership {
                TextOwnership::Required => value.reserved_flags == ABI_VALUE_OWNED_BYTES,
                TextOwnership::BorrowedOrOwned => {
                    matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
                }
            };
            valid_ownership
                && value
                    .byte_range(value.type_)
                    .is_some_and(|(_, length)| length == expected_length)
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
            let valid_ownership = match text_ownership {
                TextOwnership::Required => value.reserved_flags == ABI_VALUE_OWNED_BYTES,
                TextOwnership::BorrowedOrOwned => {
                    matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
                }
            };
            valid_ownership && valid_packed_payload(value)
        }
        AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY => {
            let valid_ownership = match text_ownership {
                TextOwnership::Required => value.reserved_flags == ABI_VALUE_OWNED_BYTES,
                TextOwnership::BorrowedOrOwned => {
                    matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
                }
            };
            let Some((pointer, length)) = value.byte_range(value.type_) else {
                return false;
            };
            if !valid_ownership {
                return false;
            }
            // SAFETY: The native module owns or synchronously borrows this
            // bounded byte range. Validation never retains the slice.
            validate_dynamic_value(value.type_, unsafe {
                core::slice::from_raw_parts(pointer, length)
            })
        }
        AbiValueType::CALLABLE => {
            let valid_ownership = match text_ownership {
                TextOwnership::Required => value.reserved_flags == ABI_VALUE_OWNED_BYTES,
                TextOwnership::BorrowedOrOwned => {
                    matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
                }
            };
            let Some((pointer, length)) = value.byte_range(AbiValueType::CALLABLE) else {
                return false;
            };
            valid_ownership
                // SAFETY: The module owns or synchronously borrows this
                // bounded byte range. Validation never retains it.
                && validate_callable_value(unsafe {
                    core::slice::from_raw_parts(pointer, length)
                })
        }
        AbiValueType::SIGNAL => {
            let valid_ownership = match text_ownership {
                TextOwnership::Required => value.reserved_flags == ABI_VALUE_OWNED_BYTES,
                TextOwnership::BorrowedOrOwned => {
                    matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
                }
            };
            let Some((pointer, length)) = value.byte_range(AbiValueType::SIGNAL) else {
                return false;
            };
            valid_ownership
                // SAFETY: The module owns or synchronously borrows this
                // bounded byte range. Validation never retains it.
                && validate_signal_value(unsafe {
                    core::slice::from_raw_parts(pointer, length)
                })
        }
        _ => false,
    }
}

fn valid_packed_payload(value: AbiValueV1) -> bool {
    let Some((pointer, length)) = value.byte_range(value.type_) else {
        return false;
    };
    if length > MAX_VALUE_TEXT_BYTES {
        return false;
    }
    if value.type_ == AbiValueType::PACKED_STRING_ARRAY {
        // SAFETY: Project modules are trusted native artifacts and the byte
        // count was bounded before constructing this temporary view.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        return valid_packed_strings(bytes);
    }
    let width = match value.type_ {
        AbiValueType::PACKED_BYTE_ARRAY => 1,
        AbiValueType::PACKED_INT32_ARRAY | AbiValueType::PACKED_FLOAT32_ARRAY => 4,
        AbiValueType::PACKED_INT64_ARRAY
        | AbiValueType::PACKED_FLOAT64_ARRAY
        | AbiValueType::PACKED_VECTOR2_ARRAY => 8,
        AbiValueType::PACKED_VECTOR3_ARRAY => 12,
        AbiValueType::PACKED_COLOR_ARRAY | AbiValueType::PACKED_VECTOR4_ARRAY => 16,
        _ => return false,
    };
    length % width == 0
}

fn valid_packed_strings(bytes: &[u8]) -> bool {
    let Some(count) = read_packed_u64(bytes, 0).and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let mut offset = core::mem::size_of::<u64>();
    for _ in 0..count {
        let Some(length) =
            read_packed_u64(bytes, offset).and_then(|value| usize::try_from(value).ok())
        else {
            return false;
        };
        let Some(start) = offset.checked_add(core::mem::size_of::<u64>()) else {
            return false;
        };
        let Some(end) = start.checked_add(length) else {
            return false;
        };
        let Some(value) = bytes.get(start..end) else {
            return false;
        };
        if core::str::from_utf8(value).is_err() {
            return false;
        }
        offset = end;
    }
    offset == bytes.len()
}

fn read_packed_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(core::mem::size_of::<u64>())?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

pub(crate) fn utf8(value: &AbiValueV1) -> Result<&str, ()> {
    let address = usize::try_from(value.payload[0]).map_err(|_| ())?;
    let length = usize::try_from(value.payload[1]).map_err(|_| ())?;
    if address == 0 || length > MAX_VALUE_TEXT_BYTES {
        return Err(());
    }
    // SAFETY: Project modules are trusted native artifacts. The producer
    // promises a live buffer for the documented borrowed or owned lifetime;
    // length is bounded before constructing the slice.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    core::str::from_utf8(bytes).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronous_inputs_accept_only_borrowed_valid_utf8() {
        let text = String::from("你好，Godot");
        let borrowed = AbiValueV1::from_borrowed_utf8(&text);
        assert!(validate_input(AbiValueType::STRING, borrowed).is_ok());

        let mut owned = borrowed;
        owned.reserved_flags = ABI_VALUE_OWNED_UTF8;
        let error = validate_input(AbiValueType::STRING, owned)
            .expect_err("Host inputs must never transfer ownership");
        assert_eq!(error.status, AbiStatus::InvalidArgument);
        assert!(error.message.contains("borrowed"));

        let invalid_bytes = [0xff_u8];
        let invalid = AbiValueV1 {
            type_: AbiValueType::STRING,
            reserved_flags: 0,
            payload: [invalid_bytes.as_ptr() as usize as u64, 1],
        };
        assert!(validate_input(AbiValueType::STRING, invalid).is_err());

        let name = AbiValueV1::from_borrowed_string_name(&text);
        assert!(validate_input(AbiValueType::STRING_NAME, name).is_ok());
        assert!(validate_input(AbiValueType::STRING, name).is_err());
    }

    #[test]
    fn value_payload_validation_is_bounded_and_typed() {
        let oversized = AbiValueV1 {
            type_: AbiValueType::STRING,
            reserved_flags: 0,
            payload: [1, (MAX_VALUE_TEXT_BYTES as u64) + 1],
        };
        assert!(validate_input(AbiValueType::STRING, oversized).is_err());
        assert!(
            validate_input(
                AbiValueType::BOOL,
                AbiValueV1 {
                    type_: AbiValueType::BOOL,
                    reserved_flags: 0,
                    payload: [2, 0],
                },
            )
            .is_err()
        );
        assert!(validate_input(AbiValueType::I64, AbiValueV1::from_i64(-12)).is_ok());
        assert!(
            validate_input(AbiValueType::VECTOR2, AbiValueV1::from_vector2(12.0, -4.0)).is_ok()
        );
        assert!(
            validate_input(
                AbiValueType::VECTOR3,
                AbiValueV1::from_vector3(1.0, 2.0, 3.0)
            )
            .is_ok()
        );
        assert!(
            validate_input(
                AbiValueType::COLOR,
                AbiValueV1::from_color(0.25, 0.5, 0.75, 1.0)
            )
            .is_ok()
        );
        assert!(
            validate_input(
                AbiValueType::VECTOR2I,
                AbiValueV1::from_vector2i(i32::MIN, i32::MAX)
            )
            .is_ok()
        );
        assert!(
            validate_input(AbiValueType::VECTOR3I, AbiValueV1::from_vector3i(-1, 2, -3)).is_ok()
        );
        assert!(validate_input(AbiValueType::RID, AbiValueV1::from_rid(u64::MAX)).is_ok());

        let mut malformed_vector = AbiValueV1::from_vector3(1.0, 2.0, 3.0);
        malformed_vector.payload[1] |= 1_u64 << 63;
        assert!(validate_input(AbiValueType::VECTOR3, malformed_vector).is_err());

        let mut flagged_color = AbiValueV1::from_color(0.25, 0.5, 0.75, 1.0);
        flagged_color.reserved_flags = 1;
        assert!(validate_input(AbiValueType::COLOR, flagged_color).is_err());

        let mut malformed_rid = AbiValueV1::from_rid(42);
        malformed_rid.payload[1] = 1;
        assert!(validate_input(AbiValueType::RID, malformed_rid).is_err());
    }

    #[test]
    fn descriptor_strings_are_copied_into_host_storage() {
        let mut source = String::from("默认文本");
        let copied = HostValue::String(source.clone());
        source.clear();

        assert_eq!(copied, HostValue::String(String::from("默认文本")));
        assert_eq!(utf8(&copied.abi()).expect("copied Host UTF-8"), "默认文本");

        let copied_name = HostValue::StringName(String::from("玩家/生命值"));
        assert_eq!(copied_name.abi().type_, AbiValueType::STRING_NAME);
        assert_eq!(
            utf8(&copied_name.abi()).expect("copied Host StringName"),
            "玩家/生命值"
        );
    }

    #[test]
    fn empty_container_defaults_use_valid_canonical_payloads() {
        let packed_strings =
            empty_property_value(AbiValueType::PACKED_STRING_ARRAY).expect("packed string default");
        let HostValue::Bytes { type_, bytes } = packed_strings else {
            panic!("packed string default must own its encoded bytes");
        };
        assert_eq!(type_, AbiValueType::PACKED_STRING_ARRAY);
        assert_eq!(&*bytes, &[0_u8; 8]);
        assert!(valid_payload(
            HostValue::Bytes { type_, bytes }.abi(),
            TextOwnership::BorrowedOrOwned,
        ));

        for (type_, node_type) in [
            (AbiValueType::ARRAY, 37_u32),
            (AbiValueType::DICTIONARY, 38_u32),
        ] {
            let value = empty_property_value(type_).expect("dynamic container default");
            let HostValue::Bytes {
                type_: actual_type,
                bytes,
            } = value
            else {
                panic!("dynamic container default must own its encoded bytes");
            };
            assert_eq!(actual_type, type_);
            assert_eq!(
                u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
                node_type
            );
            assert!(validate_dynamic_value(type_, &bytes));
        }
    }

    #[test]
    fn fixed_math_defaults_are_copied_into_host_storage() {
        let bits = [
            1.0_f32.to_bits(),
            0.0_f32.to_bits(),
            0.0_f32.to_bits(),
            1.0_f32.to_bits(),
            12.0_f32.to_bits(),
            (-6.0_f32).to_bits(),
        ];
        let copied = copy_fixed_math_descriptor(AbiValueType::TRANSFORM2D, &bits)
            .expect("valid Transform2D default");
        let abi = copied.abi();
        assert_eq!(abi.type_, AbiValueType::TRANSFORM2D);
        assert_eq!(abi.reserved_flags, 0);
        let (pointer, length) = abi
            .byte_range(AbiValueType::TRANSFORM2D)
            .expect("borrowed Host components");
        // SAFETY: `copied` owns this exact component range for the assertion.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let components = bytes
            .chunks_exact(core::mem::size_of::<f32>())
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().expect("f32 byte width")))
            .collect::<Vec<_>>();
        assert_eq!(components, [1.0, 0.0, 0.0, 1.0, 12.0, -6.0]);
        assert!(copy_fixed_math_descriptor(AbiValueType::TRANSFORM2D, &bits[..5]).is_err());
        assert!(copy_fixed_math_descriptor(AbiValueType::COLOR, &bits).is_err());
    }

    #[test]
    fn module_outputs_require_owned_string_storage() {
        let text = String::from("result");
        let borrowed = AbiValueV1::from_borrowed_utf8(&text);
        assert!(!valid_payload(borrowed, TextOwnership::Required));

        let mut owned = borrowed;
        owned.reserved_flags = ABI_VALUE_OWNED_UTF8;
        assert!(valid_payload(owned, TextOwnership::Required));
        assert!(valid_payload(
            AbiValueV1::from_f64(1.5),
            TextOwnership::Required
        ));

        let borrowed_name = AbiValueV1::from_borrowed_string_name(&text);
        assert!(!valid_payload(borrowed_name, TextOwnership::Required));
        let mut owned_name = borrowed_name;
        owned_name.reserved_flags = ABI_VALUE_OWNED_UTF8;
        assert!(valid_payload(owned_name, TextOwnership::Required));
    }
}
