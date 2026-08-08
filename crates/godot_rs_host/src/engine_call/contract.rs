use godot_rs_api::abi::{
    ABI_GODOT_API_CONST, ABI_GODOT_API_MUTATES_BASE, ABI_GODOT_API_STATIC, ABI_GODOT_API_VARARG,
    ABI_GODOT_METHOD_STATIC, ABI_GODOT_METHOD_VARARG, ABI_GODOT_VALUE_TYPED_ARRAY, AbiGodotApiKind,
    AbiGodotApiSpecV1, AbiGodotMethodSpecV1, AbiGodotValueSpecV1, AbiPtrcallType, AbiStatus,
    AbiValueType,
};

const MAX_METHOD_ARGUMENTS: usize = 32;
const MAX_NAME_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ApiContract {
    pub(super) id: u64,
    pub(super) kind: AbiGodotApiKind,
    pub(super) is_static: bool,
    pub(super) is_const: bool,
    pub(super) is_vararg: bool,
    pub(super) mutates_base: bool,
    pub(super) owner_name: Option<String>,
    pub(super) member_name: Option<String>,
    pub(super) numeric: u64,
    pub(super) base_value: ValueContract,
    pub(super) arguments: Vec<ValueContract>,
    pub(super) return_value: ValueContract,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MethodContract {
    pub(super) id: u64,
    pub(super) is_static: bool,
    pub(super) is_vararg: bool,
    pub(super) class_name: String,
    pub(super) method_name: String,
    pub(super) method_hash: i64,
    pub(super) arguments: Vec<ValueContract>,
    pub(super) return_value: ValueContract,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ValueContract {
    pub(super) value_type: AbiValueType,
    pub(super) ptrcall_type: AbiPtrcallType,
    pub(super) class_name: Option<String>,
    pub(super) typed_array_element: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ContractError {
    pub(super) status: AbiStatus,
    pub(super) message: &'static str,
}

impl ContractError {
    const fn invalid(message: &'static str) -> Self {
        Self {
            status: AbiStatus::InvalidArgument,
            message,
        }
    }

    const fn unsupported(message: &'static str) -> Self {
        Self {
            status: AbiStatus::Unsupported,
            message,
        }
    }
}

impl MethodContract {
    /// Deep-copies one generated method contract from a project module.
    ///
    /// # Safety
    ///
    /// `raw` and its borrowed slices must remain readable for this call.
    pub(super) unsafe fn copy_from_abi(
        raw: *const AbiGodotMethodSpecV1,
    ) -> Result<Self, ContractError> {
        if raw.is_null() {
            return Err(ContractError::invalid("Godot method contract is null"));
        }
        // SAFETY: Null was rejected and the caller promises a readable ABI prefix.
        let raw = unsafe { &*raw };
        if raw.struct_size < AbiGodotMethodSpecV1::MINIMUM_SIZE {
            return Err(ContractError::unsupported(
                "Godot method contract is newer than this Host",
            ));
        }
        if raw.reserved_flags & !(ABI_GODOT_METHOD_STATIC | ABI_GODOT_METHOD_VARARG) != 0
            || raw.reserved != [0; 4]
        {
            return Err(ContractError::unsupported(
                "Godot method contract uses unsupported extensions",
            ));
        }
        if raw.id == 0 {
            return Err(ContractError::invalid(
                "Godot method contract ID must not be zero",
            ));
        }
        // SAFETY: The project module promises readable ABI memory for this
        // call; `copy_name` enforces the pointer and length limits.
        let class_name = unsafe { copy_name(raw.class_name.ptr, raw.class_name.len, "class") }?;
        // SAFETY: The same generated contract retains its method-name bytes
        // through this synchronous deep copy.
        let method_name = unsafe { copy_name(raw.method_name.ptr, raw.method_name.len, "method") }?;
        let method_hash = i64::try_from(raw.method_hash)
            .map_err(|_| ContractError::invalid("Godot method hash exceeds the official ABI"))?;
        if raw.arguments.len > MAX_METHOD_ARGUMENTS {
            return Err(ContractError::invalid(
                "Godot method argument count exceeds the Host limit",
            ));
        }
        if raw.arguments.len != 0 && raw.arguments.ptr.is_null() {
            return Err(ContractError::invalid(
                "Godot method argument contracts pointer is null",
            ));
        }
        let arguments = if raw.arguments.len == 0 {
            &[]
        } else {
            // SAFETY: Null and the bounded length were validated above.
            unsafe { core::slice::from_raw_parts(raw.arguments.ptr, raw.arguments.len) }
        };
        let arguments = arguments
            .iter()
            .map(|argument| copy_value_contract(argument, false))
            .collect::<Result<Vec<_>, _>>()?;
        let return_value = copy_value_contract(&raw.return_value, true)?;

        Ok(Self {
            id: raw.id,
            is_static: raw.reserved_flags & ABI_GODOT_METHOD_STATIC != 0,
            is_vararg: raw.reserved_flags & ABI_GODOT_METHOD_VARARG != 0,
            class_name,
            method_name,
            method_hash,
            arguments,
            return_value,
        })
    }
}

impl ApiContract {
    /// Deep-copies and validates one generated non-MethodBind API contract.
    ///
    /// # Safety
    ///
    /// `raw` and all borrowed slices reachable from it must be readable for
    /// this synchronous call.
    pub(super) unsafe fn copy_from_abi(
        raw: *const AbiGodotApiSpecV1,
    ) -> Result<Self, ContractError> {
        if raw.is_null() {
            return Err(ContractError::invalid("Godot API contract is null"));
        }
        // SAFETY: Null was rejected and the caller promises a readable prefix.
        let raw = unsafe { &*raw };
        if raw.struct_size < AbiGodotApiSpecV1::MINIMUM_SIZE {
            return Err(ContractError::unsupported(
                "Godot API contract is newer than this Host",
            ));
        }
        if !raw.kind.is_supported() || raw.reserved != [0; 4] {
            return Err(ContractError::unsupported(
                "Godot API contract uses an unsupported operation",
            ));
        }
        let allowed_flags = ABI_GODOT_API_STATIC
            | ABI_GODOT_API_CONST
            | ABI_GODOT_API_VARARG
            | ABI_GODOT_API_MUTATES_BASE;
        if raw.reserved_flags & !allowed_flags != 0 {
            return Err(ContractError::unsupported(
                "Godot API contract uses unsupported extensions",
            ));
        }
        if raw.id == 0 {
            return Err(ContractError::invalid(
                "Godot API contract ID must not be zero",
            ));
        }
        // SAFETY: The enclosing spec was validated and these slices are
        // synchronously copied before the project callback returns.
        let owner_name = unsafe { copy_optional_name(raw.owner_name, "API owner") }?;
        // SAFETY: The enclosing spec was validated and these slices are
        // synchronously copied before the project callback returns.
        let member_name = unsafe { copy_optional_name(raw.member_name, "API member") }?;
        if raw.arguments.len > MAX_METHOD_ARGUMENTS {
            return Err(ContractError::invalid(
                "Godot API argument count exceeds the Host limit",
            ));
        }
        if raw.arguments.len != 0 && raw.arguments.ptr.is_null() {
            return Err(ContractError::invalid(
                "Godot API argument contracts pointer is null",
            ));
        }
        let arguments = if raw.arguments.len == 0 {
            &[]
        } else {
            // SAFETY: Null and the bounded length were checked above.
            unsafe { core::slice::from_raw_parts(raw.arguments.ptr, raw.arguments.len) }
        };
        let arguments = arguments
            .iter()
            .map(|argument| copy_value_contract(argument, false))
            .collect::<Result<Vec<_>, _>>()?;
        let base_value = copy_value_contract(&raw.base_value, true)?;
        let return_value = copy_value_contract(&raw.return_value, true)?;
        let is_static = raw.reserved_flags & ABI_GODOT_API_STATIC != 0;
        let is_const = raw.reserved_flags & ABI_GODOT_API_CONST != 0;
        let is_vararg = raw.reserved_flags & ABI_GODOT_API_VARARG != 0;
        let mutates_base = raw.reserved_flags & ABI_GODOT_API_MUTATES_BASE != 0;
        validate_api_shape(
            raw.kind,
            owner_name.as_deref(),
            member_name.as_deref(),
            &base_value,
            &arguments,
            &return_value,
            is_static,
            is_const,
            is_vararg,
            mutates_base,
        )?;
        Ok(Self {
            id: raw.id,
            kind: raw.kind,
            is_static,
            is_const,
            is_vararg,
            mutates_base,
            owner_name,
            member_name,
            numeric: raw.numeric,
            base_value,
            arguments,
            return_value,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_api_shape(
    kind: AbiGodotApiKind,
    owner: Option<&str>,
    member: Option<&str>,
    base: &ValueContract,
    arguments: &[ValueContract],
    return_value: &ValueContract,
    is_static: bool,
    is_const: bool,
    is_vararg: bool,
    mutates_base: bool,
) -> Result<(), ContractError> {
    let has_base = base.ptrcall_type != AbiPtrcallType::VOID;
    let returns_void = return_value.ptrcall_type == AbiPtrcallType::VOID;
    let valid = match kind {
        AbiGodotApiKind::UTILITY_FUNCTION => {
            owner.is_none() && member.is_some() && !has_base && is_static && !mutates_base
        }
        AbiGodotApiKind::BUILTIN_CONSTRUCTOR => {
            owner.is_some()
                && member.is_none()
                && !has_base
                && !returns_void
                && is_static
                && !is_vararg
                && !mutates_base
        }
        AbiGodotApiKind::BUILTIN_METHOD => {
            owner.is_some()
                && member.is_some()
                && (is_static != has_base)
                && (is_const || is_static || mutates_base)
                && (!mutates_base || has_base)
        }
        AbiGodotApiKind::BUILTIN_OPERATOR => {
            owner.is_some()
                && member.is_some()
                && has_base
                && !is_static
                && !is_vararg
                && !mutates_base
                && arguments.len() <= 1
        }
        AbiGodotApiKind::BUILTIN_MEMBER_GETTER => {
            owner.is_some()
                && member.is_some()
                && has_base
                && arguments.is_empty()
                && !returns_void
                && is_const
                && !mutates_base
        }
        AbiGodotApiKind::BUILTIN_MEMBER_SETTER => {
            owner.is_some()
                && member.is_some()
                && has_base
                && arguments.len() == 1
                && returns_void
                && mutates_base
        }
        AbiGodotApiKind::BUILTIN_INDEXED_GETTER => {
            owner.is_some()
                && member.is_none()
                && has_base
                && arguments.len() == 1
                && !returns_void
                && is_const
                && !mutates_base
        }
        AbiGodotApiKind::BUILTIN_INDEXED_SETTER => {
            owner.is_some()
                && member.is_none()
                && has_base
                && arguments.len() == 2
                && returns_void
                && mutates_base
        }
        AbiGodotApiKind::BUILTIN_KEYED_GETTER => {
            owner.is_some()
                && member.is_none()
                && has_base
                && arguments.len() == 1
                && !returns_void
                && is_const
                && !mutates_base
        }
        AbiGodotApiKind::BUILTIN_KEYED_SETTER => {
            owner.is_some()
                && member.is_none()
                && has_base
                && arguments.len() == 2
                && returns_void
                && mutates_base
        }
        AbiGodotApiKind::BUILTIN_CONSTANT => {
            owner.is_some()
                && member.is_some()
                && !has_base
                && arguments.is_empty()
                && !returns_void
                && is_static
                && !mutates_base
        }
        AbiGodotApiKind::SINGLETON => {
            owner.is_some()
                && member.is_some()
                && !has_base
                && arguments.is_empty()
                && return_value.ptrcall_type == AbiPtrcallType::OBJECT
                && is_static
                && !mutates_base
        }
        AbiGodotApiKind::OBJECT_CONSTRUCTOR => {
            owner.is_some()
                && member.is_none()
                && !has_base
                && arguments.is_empty()
                && matches!(
                    return_value.ptrcall_type,
                    AbiPtrcallType::OBJECT | AbiPtrcallType::REFCOUNTED_OBJECT
                )
                && is_static
                && !mutates_base
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ContractError::invalid(
            "Godot API contract fields do not match its operation",
        ))
    }
}

pub(super) fn copy_value_contract(
    raw: &AbiGodotValueSpecV1,
    allow_void: bool,
) -> Result<ValueContract, ContractError> {
    if raw.reserved != [0; 2] {
        return Err(ContractError::unsupported(
            "Godot value contract uses unsupported extensions",
        ));
    }
    if !raw.value_type.is_supported() || !raw.ptrcall_type.is_supported() {
        return Err(ContractError::unsupported(
            "Godot value contract uses an unsupported type",
        ));
    }
    let metadata = if raw.class_name.len == 0 {
        if !raw.class_name.ptr.is_null() {
            return Err(ContractError::invalid(
                "empty Godot class name has a non-null pointer",
            ));
        }
        None
    } else {
        // SAFETY: `copy_name` validates the pointer and bounded byte length.
        Some(unsafe { copy_name(raw.class_name.ptr, raw.class_name.len, "value class") }?)
    };
    let (class_name, typed_array_element) = match raw.reserved_flags {
        0 => (metadata, None),
        ABI_GODOT_VALUE_TYPED_ARRAY if metadata.is_some() => (None, metadata),
        ABI_GODOT_VALUE_TYPED_ARRAY => {
            return Err(ContractError::invalid(
                "typed Godot Array contract has no element type",
            ));
        }
        _ => {
            return Err(ContractError::unsupported(
                "Godot value contract uses unsupported extensions",
            ));
        }
    };

    let valid = match (raw.value_type, raw.ptrcall_type) {
        (AbiValueType::NIL, AbiPtrcallType::VOID) => allow_void && class_name.is_none(),
        (AbiValueType::BOOL, AbiPtrcallType::BOOL) => class_name.is_none(),
        (
            AbiValueType::I64,
            AbiPtrcallType::I8 | AbiPtrcallType::I16 | AbiPtrcallType::I32 | AbiPtrcallType::I64,
        ) => class_name.is_none(),
        (
            AbiValueType::U64,
            AbiPtrcallType::U8 | AbiPtrcallType::U16 | AbiPtrcallType::U32 | AbiPtrcallType::U64,
        ) => class_name.is_none(),
        (AbiValueType::F64, AbiPtrcallType::F32 | AbiPtrcallType::F64) => class_name.is_none(),
        (AbiValueType::VECTOR2, AbiPtrcallType::VECTOR2)
        | (AbiValueType::VECTOR2I, AbiPtrcallType::VECTOR2I)
        | (AbiValueType::VECTOR3, AbiPtrcallType::VECTOR3)
        | (AbiValueType::VECTOR3I, AbiPtrcallType::VECTOR3I)
        | (AbiValueType::VECTOR4, AbiPtrcallType::VECTOR4)
        | (AbiValueType::VECTOR4I, AbiPtrcallType::VECTOR4I)
        | (AbiValueType::RECT2, AbiPtrcallType::RECT2)
        | (AbiValueType::RECT2I, AbiPtrcallType::RECT2I)
        | (AbiValueType::QUATERNION, AbiPtrcallType::QUATERNION)
        | (AbiValueType::PLANE, AbiPtrcallType::PLANE)
        | (AbiValueType::TRANSFORM2D, AbiPtrcallType::TRANSFORM2D)
        | (AbiValueType::AABB, AbiPtrcallType::AABB)
        | (AbiValueType::BASIS, AbiPtrcallType::BASIS)
        | (AbiValueType::TRANSFORM3D, AbiPtrcallType::TRANSFORM3D)
        | (AbiValueType::PROJECTION, AbiPtrcallType::PROJECTION)
        | (AbiValueType::PACKED_BYTE_ARRAY, AbiPtrcallType::PACKED_BYTE_ARRAY)
        | (AbiValueType::PACKED_INT32_ARRAY, AbiPtrcallType::PACKED_INT32_ARRAY)
        | (AbiValueType::PACKED_INT64_ARRAY, AbiPtrcallType::PACKED_INT64_ARRAY)
        | (AbiValueType::PACKED_FLOAT32_ARRAY, AbiPtrcallType::PACKED_FLOAT32_ARRAY)
        | (AbiValueType::PACKED_FLOAT64_ARRAY, AbiPtrcallType::PACKED_FLOAT64_ARRAY)
        | (AbiValueType::PACKED_STRING_ARRAY, AbiPtrcallType::PACKED_STRING_ARRAY)
        | (AbiValueType::PACKED_VECTOR2_ARRAY, AbiPtrcallType::PACKED_VECTOR2_ARRAY)
        | (AbiValueType::PACKED_VECTOR3_ARRAY, AbiPtrcallType::PACKED_VECTOR3_ARRAY)
        | (AbiValueType::PACKED_COLOR_ARRAY, AbiPtrcallType::PACKED_COLOR_ARRAY)
        | (AbiValueType::PACKED_VECTOR4_ARRAY, AbiPtrcallType::PACKED_VECTOR4_ARRAY)
        | (AbiValueType::VARIANT, AbiPtrcallType::VARIANT)
        | (AbiValueType::DICTIONARY, AbiPtrcallType::DICTIONARY)
        | (AbiValueType::CALLABLE, AbiPtrcallType::CALLABLE)
        | (AbiValueType::SIGNAL, AbiPtrcallType::SIGNAL)
        | (AbiValueType::COLOR, AbiPtrcallType::COLOR)
        | (AbiValueType::RID, AbiPtrcallType::RID) => class_name.is_none(),
        (AbiValueType::STRING, AbiPtrcallType::STRING) => class_name.is_none(),
        (AbiValueType::STRING_NAME, AbiPtrcallType::STRING_NAME) => class_name.is_none(),
        (AbiValueType::NODE_PATH, AbiPtrcallType::NODE_PATH) => class_name.is_none(),
        (AbiValueType::ARRAY, AbiPtrcallType::ARRAY) => class_name.is_none(),
        (AbiValueType::OBJECT_ID, AbiPtrcallType::OBJECT) => class_name.is_some(),
        (AbiValueType::OBJECT_ID, AbiPtrcallType::REFCOUNTED_OBJECT) => {
            allow_void && class_name.is_some()
        }
        _ => false,
    };
    if !valid {
        return Err(ContractError::invalid(
            "Godot value and ptrcall types do not form a valid contract",
        ));
    }
    if typed_array_element.is_some()
        && !matches!(
            (raw.value_type, raw.ptrcall_type),
            (AbiValueType::ARRAY, AbiPtrcallType::ARRAY)
        )
    {
        return Err(ContractError::invalid(
            "typed-Array metadata is attached to a non-Array value",
        ));
    }

    Ok(ValueContract {
        value_type: raw.value_type,
        ptrcall_type: raw.ptrcall_type,
        class_name,
        typed_array_element,
    })
}

unsafe fn copy_optional_name(
    raw: godot_rs_api::abi::AbiByteSlice,
    kind: &'static str,
) -> Result<Option<String>, ContractError> {
    if raw.len == 0 {
        if raw.ptr.is_null() {
            return Ok(None);
        }
        return Err(ContractError::invalid(
            "empty Godot API name has a non-null pointer",
        ));
    }
    // SAFETY: The caller promises this generated slice is readable.
    unsafe { copy_name(raw.ptr, raw.len, kind) }.map(Some)
}

unsafe fn copy_name(
    pointer: *const u8,
    length: usize,
    kind: &'static str,
) -> Result<String, ContractError> {
    if length == 0 {
        return Err(ContractError::invalid(match kind {
            "class" => "Godot method class name is empty",
            "method" => "Godot method name is empty",
            _ => "Godot value class name is empty",
        }));
    }
    if length > MAX_NAME_BYTES {
        return Err(ContractError::invalid(
            "Godot API name exceeds the Host limit",
        ));
    }
    if pointer.is_null() {
        return Err(ContractError::invalid("Godot API name pointer is null"));
    }
    // SAFETY: The caller promises readable ABI memory and the length is bounded.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    let value = core::str::from_utf8(bytes)
        .map_err(|_| ContractError::invalid("Godot API name is not valid UTF-8"))?;
    if value.as_bytes().contains(&0) {
        return Err(ContractError::invalid("Godot API name contains a nul byte"));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use godot_rs_api::abi::{
        ABI_GODOT_API_CONST, ABI_GODOT_API_MUTATES_BASE, ABI_GODOT_API_STATIC, AbiByteSlice,
        AbiGodotApiKind, AbiGodotApiSpecV1, AbiGodotValueSpecSlice,
    };

    const BOOL: AbiGodotValueSpecV1 = AbiGodotValueSpecV1 {
        value_type: AbiValueType::BOOL,
        ptrcall_type: AbiPtrcallType::BOOL,
        class_name: AbiByteSlice::EMPTY,
        reserved_flags: 0,
        reserved: [0; 2],
    };

    fn method(arguments: &'static [AbiGodotValueSpecV1]) -> AbiGodotMethodSpecV1 {
        AbiGodotMethodSpecV1 {
            struct_size: AbiGodotMethodSpecV1::MINIMUM_SIZE,
            reserved_flags: 0,
            id: 42,
            class_name: AbiByteSlice::from_static("Node"),
            method_name: AbiByteSlice::from_static("set_process"),
            method_hash: 2_583_456_009,
            arguments: AbiGodotValueSpecSlice::from_static(arguments),
            return_value: AbiGodotValueSpecV1::NIL,
            reserved: [0; 4],
        }
    }

    fn api(
        kind: AbiGodotApiKind,
        flags: u32,
        owner: AbiByteSlice,
        member: AbiByteSlice,
        base: AbiGodotValueSpecV1,
        arguments: &'static [AbiGodotValueSpecV1],
        return_value: AbiGodotValueSpecV1,
    ) -> AbiGodotApiSpecV1 {
        AbiGodotApiSpecV1 {
            struct_size: AbiGodotApiSpecV1::MINIMUM_SIZE,
            reserved_flags: flags,
            id: 84,
            kind,
            owner_name: owner,
            member_name: member,
            numeric: 42,
            base_value: base,
            arguments: AbiGodotValueSpecSlice::from_static(arguments),
            return_value,
            reserved: [0; 4],
        }
    }

    #[test]
    fn generated_api_contracts_validate_operation_specific_shapes() {
        let utility = api(
            AbiGodotApiKind::UTILITY_FUNCTION,
            ABI_GODOT_API_STATIC | ABI_GODOT_API_CONST,
            AbiByteSlice::EMPTY,
            AbiByteSlice::from_static("is_instance_valid"),
            AbiGodotValueSpecV1::NIL,
            &[],
            BOOL,
        );
        // SAFETY: The complete local contract and static names are readable.
        let copied = unsafe { ApiContract::copy_from_abi(&utility) }.expect("utility contract");
        assert_eq!(copied.kind, AbiGodotApiKind::UTILITY_FUNCTION);
        assert_eq!(copied.member_name.as_deref(), Some("is_instance_valid"));
        assert!(copied.is_static);

        let mut setter = api(
            AbiGodotApiKind::BUILTIN_MEMBER_SETTER,
            ABI_GODOT_API_MUTATES_BASE,
            AbiByteSlice::from_static("Vector2"),
            AbiByteSlice::from_static("x"),
            AbiGodotValueSpecV1 {
                value_type: AbiValueType::VECTOR2,
                ptrcall_type: AbiPtrcallType::VECTOR2,
                class_name: AbiByteSlice::EMPTY,
                reserved_flags: 0,
                reserved: [0; 2],
            },
            &[AbiGodotValueSpecV1 {
                value_type: AbiValueType::F64,
                ptrcall_type: AbiPtrcallType::F64,
                class_name: AbiByteSlice::EMPTY,
                reserved_flags: 0,
                reserved: [0; 2],
            }],
            AbiGodotValueSpecV1::NIL,
        );
        // SAFETY: The complete local contract and static metadata are readable.
        let copied = unsafe { ApiContract::copy_from_abi(&setter) }.expect("setter contract");
        assert!(copied.mutates_base);

        setter.reserved_flags = 0;
        let error =
            // SAFETY: The malformed local contract remains readable.
            unsafe { ApiContract::copy_from_abi(&setter) }.expect_err("mutation flag required");
        assert_eq!(error.status, AbiStatus::InvalidArgument);
    }

    #[test]
    fn valid_contract_is_deep_copied() {
        let raw = method(&[BOOL]);
        // SAFETY: `raw` and its static argument metadata are readable.
        let copied = unsafe { MethodContract::copy_from_abi(&raw) }.expect("valid contract");
        assert_eq!(copied.id, 42);
        assert!(!copied.is_static);
        assert!(!copied.is_vararg);
        assert_eq!(copied.class_name, "Node");
        assert_eq!(copied.arguments[0].ptrcall_type, AbiPtrcallType::BOOL);
        assert_eq!(
            copied.return_value,
            ValueContract {
                value_type: AbiValueType::NIL,
                ptrcall_type: AbiPtrcallType::VOID,
                class_name: None,
                typed_array_element: None,
            }
        );
    }

    #[test]
    fn invalid_type_pairs_and_reserved_extensions_are_rejected() {
        let raw = method(&[AbiGodotValueSpecV1 {
            value_type: AbiValueType::I64,
            ptrcall_type: AbiPtrcallType::U64,
            class_name: AbiByteSlice::EMPTY,
            reserved_flags: 0,
            reserved: [0; 2],
        }]);
        // SAFETY: `raw` and its promoted static argument metadata are readable.
        let error = unsafe { MethodContract::copy_from_abi(&raw) }.expect_err("invalid pair");
        assert_eq!(error.status, AbiStatus::InvalidArgument);

        let mut raw = method(&[]);
        raw.reserved[0] = 1;
        // SAFETY: `raw` is a readable local contract.
        let error = unsafe { MethodContract::copy_from_abi(&raw) }.expect_err("reserved slot");
        assert_eq!(error.status, AbiStatus::Unsupported);

        let mut raw = method(&[]);
        raw.reserved_flags = 1 << 31;
        // SAFETY: `raw` is a readable local contract.
        let error = unsafe { MethodContract::copy_from_abi(&raw) }.expect_err("unknown flag");
        assert_eq!(error.status, AbiStatus::Unsupported);
    }

    #[test]
    fn static_method_contracts_preserve_receiver_free_dispatch() {
        let mut raw = method(&[]);
        raw.reserved_flags = ABI_GODOT_METHOD_STATIC;
        // SAFETY: `raw` is a readable local contract.
        let copied = unsafe { MethodContract::copy_from_abi(&raw) }.expect("static contract");
        assert!(copied.is_static);
    }

    #[test]
    fn vararg_method_contracts_select_variant_call_dispatch() {
        let mut raw = method(&[BOOL]);
        raw.reserved_flags = ABI_GODOT_METHOD_VARARG;
        // SAFETY: `raw` and its static argument metadata are readable.
        let copied = unsafe { MethodContract::copy_from_abi(&raw) }.expect("vararg contract");
        assert!(copied.is_vararg);
        assert!(!copied.is_static);
        assert_eq!(copied.arguments.len(), 1);
    }

    #[test]
    fn math_value_contracts_require_the_exact_native_builtin() {
        for (value_type, ptrcall_type) in [
            (AbiValueType::VECTOR2, AbiPtrcallType::VECTOR2),
            (AbiValueType::VECTOR2I, AbiPtrcallType::VECTOR2I),
            (AbiValueType::VECTOR3, AbiPtrcallType::VECTOR3),
            (AbiValueType::VECTOR3I, AbiPtrcallType::VECTOR3I),
            (AbiValueType::VECTOR4, AbiPtrcallType::VECTOR4),
            (AbiValueType::VECTOR4I, AbiPtrcallType::VECTOR4I),
            (AbiValueType::RECT2, AbiPtrcallType::RECT2),
            (AbiValueType::RECT2I, AbiPtrcallType::RECT2I),
            (AbiValueType::QUATERNION, AbiPtrcallType::QUATERNION),
            (AbiValueType::PLANE, AbiPtrcallType::PLANE),
            (AbiValueType::TRANSFORM2D, AbiPtrcallType::TRANSFORM2D),
            (AbiValueType::AABB, AbiPtrcallType::AABB),
            (AbiValueType::BASIS, AbiPtrcallType::BASIS),
            (AbiValueType::TRANSFORM3D, AbiPtrcallType::TRANSFORM3D),
            (AbiValueType::PROJECTION, AbiPtrcallType::PROJECTION),
            (AbiValueType::COLOR, AbiPtrcallType::COLOR),
        ] {
            let raw = AbiGodotValueSpecV1 {
                value_type,
                ptrcall_type,
                class_name: AbiByteSlice::EMPTY,
                reserved_flags: 0,
                reserved: [0; 2],
            };
            let copied = copy_value_contract(&raw, false).expect("valid math contract");
            assert_eq!(copied.value_type, value_type);
            assert_eq!(copied.ptrcall_type, ptrcall_type);
        }

        let mismatched = AbiGodotValueSpecV1 {
            value_type: AbiValueType::VECTOR2,
            ptrcall_type: AbiPtrcallType::VECTOR3,
            class_name: AbiByteSlice::EMPTY,
            reserved_flags: 0,
            reserved: [0; 2],
        };
        assert!(copy_value_contract(&mismatched, false).is_err());
    }

    #[test]
    fn text_value_contracts_use_their_exact_owned_builtin_types() {
        for (value_type, ptrcall_type) in [
            (AbiValueType::STRING, AbiPtrcallType::STRING),
            (AbiValueType::STRING_NAME, AbiPtrcallType::STRING_NAME),
            (AbiValueType::NODE_PATH, AbiPtrcallType::NODE_PATH),
        ] {
            let raw = AbiGodotValueSpecV1 {
                value_type,
                ptrcall_type,
                class_name: AbiByteSlice::EMPTY,
                reserved_flags: 0,
                reserved: [0; 2],
            };
            let copied = copy_value_contract(&raw, false).expect("valid text contract");
            assert_eq!(copied.value_type, value_type);
            assert_eq!(copied.ptrcall_type, ptrcall_type);
        }
    }

    #[test]
    fn rid_value_contract_requires_exact_opaque_storage() {
        let raw = AbiGodotValueSpecV1 {
            value_type: AbiValueType::RID,
            ptrcall_type: AbiPtrcallType::RID,
            class_name: AbiByteSlice::EMPTY,
            reserved_flags: 0,
            reserved: [0; 2],
        };
        let copied = copy_value_contract(&raw, false).expect("valid RID contract");
        assert_eq!(copied.value_type, AbiValueType::RID);
        assert_eq!(copied.ptrcall_type, AbiPtrcallType::RID);
    }

    #[test]
    fn refcounted_object_storage_is_return_only() {
        let raw = AbiGodotValueSpecV1 {
            value_type: AbiValueType::OBJECT_ID,
            ptrcall_type: AbiPtrcallType::REFCOUNTED_OBJECT,
            class_name: AbiByteSlice::from_static("Resource"),
            reserved_flags: 0,
            reserved: [0; 2],
        };
        let copied = copy_value_contract(&raw, true).expect("valid RefCounted return");
        assert_eq!(copied.ptrcall_type, AbiPtrcallType::REFCOUNTED_OBJECT);
        assert!(copy_value_contract(&raw, false).is_err());
    }
}
