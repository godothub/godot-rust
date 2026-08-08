use core::ffi::c_void;
use core::ptr;

use godot_api::abi::{
    ABI_VALUE_OWNED_BYTES, AbiPtrcallType, AbiStatus, AbiValueType, AbiValueV1,
    callable_value_ownership_token, dynamic_value_ownership_token,
};
use godot_api::{
    GDExtensionCallError, GDExtensionCallErrorType, GDExtensionConstVariantPtr,
    GDExtensionMethodBindPtr, GDExtensionObjectPtr,
};

use super::contract::ValueContract;
use super::value::NativeGodotRef;
use super::{EngineCallContext, EngineCallError, ResolvedMethod};
use crate::callable_value::CallableCallBacking;
use crate::dynamic_value::DynamicCallBacking;
use crate::interface::EngineInterface;
use crate::variant_codec::{OwnedVariant, VariantCodec, VariantDecodeBacking};

pub(super) fn call(
    interface: EngineInterface,
    context: &EngineCallContext,
    resolved: &ResolvedMethod,
    receiver: GDExtensionObjectPtr,
    arguments: &[AbiValueV1],
    callable_object_tag: usize,
) -> Result<AbiValueV1, EngineCallError> {
    let codec = VariantCodec::new(interface)
        .ok_or_else(|| EngineCallError::internal("Godot Variant codecs are unavailable"))?;
    let fixed_count = resolved.contract.arguments.len();
    let mut variants = Vec::with_capacity(arguments.len());
    for (index, value) in arguments.iter().copied().enumerate() {
        let typed_array_element = if index < fixed_count {
            let contract = &resolved.contract.arguments[index];
            let class_tag = resolved.argument_tags[index];
            let _validated = super::decode_native_argument(
                interface,
                context,
                contract,
                value,
                class_tag,
                callable_object_tag,
            )?;
            contract.typed_array_element.as_deref()
        } else {
            if value.type_ != AbiValueType::VARIANT || value.reserved_flags != 0 {
                return Err(EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot variable argument is not a Variant",
                ));
            }
            None
        };
        variants.push(
            OwnedVariant::from_abi_with_context(&codec, value, typed_array_element, Some(context))
                .map_err(|_| {
                    EngineCallError::new(
                        AbiStatus::InvalidArgument,
                        "Godot variable argument could not be converted to a Variant",
                    )
                })?,
        );
    }
    let pointers = variants
        .iter()
        .map(OwnedVariant::as_ptr)
        .collect::<Vec<GDExtensionConstVariantPtr>>();
    let argument_count = i64::try_from(pointers.len()).map_err(|_| {
        EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot variable argument count exceeds the engine ABI",
        )
    })?;
    let argument_pointer = if pointers.is_empty() {
        ptr::null()
    } else {
        pointers.as_ptr()
    };
    let Some(call) = interface.object_method_bind_call else {
        return Err(EngineCallError::internal(
            "Godot Variant MethodBind calls are unavailable",
        ));
    };
    let mut output = OwnedVariant::uninitialized(interface);
    let mut call_error = GDExtensionCallError {
        error: GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
        argument: 0,
        expected: 0,
    };
    // SAFETY: MethodBind, receiver, argument Variants, output storage, and
    // error storage all remain live for this synchronous official call.
    unsafe {
        call(
            resolved.method_bind as GDExtensionMethodBindPtr,
            receiver,
            argument_pointer,
            argument_count,
            output.as_mut_ptr(),
            ptr::from_mut(&mut call_error),
        );
    }
    // Godot constructs the Variant return slot even when the call reports a
    // dispatch error, so it must always receive its paired destructor.
    output.mark_initialized();
    check_call_error(call_error)?;

    let mut strings = Vec::new();
    let mut math = Vec::new();
    let mut packed = Vec::new();
    let mut dynamic = Vec::<DynamicCallBacking>::new();
    let mut callable = Vec::<CallableCallBacking>::new();
    let value = codec
        .decode(
            output.as_ptr(),
            resolved.contract.return_value.value_type,
            VariantDecodeBacking {
                strings: &mut strings,
                math: &mut math,
                packed: &mut packed,
                dynamic: &mut dynamic,
                callable: &mut callable,
                dynamic_context: Some(context),
            },
        )
        .map_err(|_| {
            EngineCallError::new(
                AbiStatus::Internal,
                "Godot variable-argument method returned the wrong type",
            )
        })?;
    own_decoded_return(
        interface,
        context,
        &resolved.contract.return_value,
        resolved.return_tag,
        value,
    )
}

fn check_call_error(error: GDExtensionCallError) -> Result<(), EngineCallError> {
    let status = error.error;
    if status == GDExtensionCallErrorType::GDEXTENSION_CALL_OK {
        return Ok(());
    }
    let (status, message) =
        if status == GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_METHOD {
            (
                AbiStatus::Unsupported,
                "Godot variable-argument method is unavailable",
            )
        } else if status == GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_ARGUMENT
            || status == GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_MANY_ARGUMENTS
            || status == GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_FEW_ARGUMENTS
            || status == GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL
        {
            (
                AbiStatus::InvalidArgument,
                "Godot rejected a variable-argument method argument",
            )
        } else if status == GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_METHOD_NOT_CONST {
            (
                AbiStatus::CallbackFailed,
                "Godot rejected a non-const variable-argument method call",
            )
        } else {
            (
                AbiStatus::Internal,
                "Godot returned an unknown variable-argument call error",
            )
        };
    Err(EngineCallError::new(status, message))
}

fn own_decoded_return(
    interface: EngineInterface,
    context: &EngineCallContext,
    contract: &ValueContract,
    return_tag: usize,
    value: AbiValueV1,
) -> Result<AbiValueV1, EngineCallError> {
    match (contract.value_type, contract.ptrcall_type) {
        (AbiValueType::STRING, AbiPtrcallType::STRING)
        | (AbiValueType::STRING_NAME, AbiPtrcallType::STRING_NAME)
        | (AbiValueType::NODE_PATH, AbiPtrcallType::NODE_PATH) => {
            let text = crate::module_value::utf8(&value).map_err(|_| {
                EngineCallError::internal("Godot returned invalid text through Variant call")
            })?;
            super::own_returned_utf8(contract.value_type, text.to_owned()).map_err(Into::into)
        }
        (
            AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION,
            _,
        ) => {
            let components = borrowed_f32_components(value, contract.value_type)?;
            super::own_returned_math(contract.value_type, &components).map_err(Into::into)
        }
        (
            AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY,
            _,
        ) => {
            let bytes = copied_bytes(value, contract.value_type)?;
            super::own_returned_packed(contract.value_type, bytes).map_err(Into::into)
        }
        (AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY, _) => {
            own_dynamic_return(context, value)
        }
        (AbiValueType::CALLABLE, AbiPtrcallType::CALLABLE) => own_callable_return(context, value),
        (AbiValueType::SIGNAL, AbiPtrcallType::SIGNAL) => {
            let bytes = copied_bytes(value, AbiValueType::SIGNAL)?;
            super::retain_returned_bytes(
                AbiValueType::SIGNAL,
                bytes.into_boxed_slice(),
                ABI_VALUE_OWNED_BYTES,
            )
            .map_err(Into::into)
        }
        (AbiValueType::OBJECT_ID, AbiPtrcallType::OBJECT) => {
            validate_returned_object(interface, value.payload[0], return_tag)?;
            Ok(value)
        }
        (AbiValueType::OBJECT_ID, AbiPtrcallType::REFCOUNTED_OBJECT) => {
            let object_id = value.payload[0];
            if object_id == 0 {
                return Ok(AbiValueV1::from_object_id(0));
            }
            let object = validate_returned_object(interface, object_id, return_tag)?;
            let owned = NativeGodotRef::from_object(interface, object)?;
            let context_pointer = ptr::from_ref(context).cast_mut().cast::<c_void>();
            super::retain_returned_object(context_pointer, interface, object, owned, return_tag)
                .map_err(Into::into)
        }
        _ => Ok(value),
    }
}

fn validate_returned_object(
    interface: EngineInterface,
    object_id: u64,
    return_tag: usize,
) -> Result<GDExtensionObjectPtr, EngineCallError> {
    if object_id == 0 {
        return Ok(ptr::null_mut());
    }
    super::resolve_object(
        interface,
        object_id,
        return_tag,
        "Godot variable-argument method returned a stale object",
        "Godot variable-argument method returned an object with the wrong class",
    )
}

fn own_dynamic_return(
    context: &EngineCallContext,
    value: AbiValueV1,
) -> Result<AbiValueV1, EngineCallError> {
    let bytes = copied_bytes(value, value.type_)?;
    let token = dynamic_value_ownership_token(&bytes).ok_or_else(|| {
        EngineCallError::internal("Godot dynamic Variant return has no ownership token")
    })?;
    let status = context.clone_dynamic(token);
    if status != AbiStatus::Ok {
        return Err(EngineCallError::new(
            status,
            "Godot dynamic Variant ownership could not be retained",
        ));
    }
    match super::retain_returned_bytes(value.type_, bytes.into_boxed_slice(), ABI_VALUE_OWNED_BYTES)
    {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = context.release_dynamic(token);
            Err(error.into())
        }
    }
}

fn own_callable_return(
    context: &EngineCallContext,
    value: AbiValueV1,
) -> Result<AbiValueV1, EngineCallError> {
    let bytes = copied_bytes(value, AbiValueType::CALLABLE)?;
    let token = callable_value_ownership_token(&bytes).ok_or_else(|| {
        EngineCallError::internal("Godot Callable Variant return has no ownership token")
    })?;
    let status = context.clone_callable(token);
    if status != AbiStatus::Ok {
        return Err(EngineCallError::new(
            status,
            "Godot Callable ownership could not be retained",
        ));
    }
    match super::retain_returned_bytes(
        AbiValueType::CALLABLE,
        bytes.into_boxed_slice(),
        ABI_VALUE_OWNED_BYTES,
    ) {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = context.release_callable(token);
            Err(error.into())
        }
    }
}

fn copied_bytes(value: AbiValueV1, expected: AbiValueType) -> Result<Vec<u8>, EngineCallError> {
    let (pointer, length) = value.byte_range(expected).ok_or_else(|| {
        EngineCallError::internal("Godot Variant return has invalid byte storage")
    })?;
    // SAFETY: The decoder-owned backing for this exact bounded range remains
    // live until this synchronous copy completes.
    Ok(unsafe { core::slice::from_raw_parts(pointer, length) }.to_vec())
}

fn borrowed_f32_components(
    value: AbiValueV1,
    expected: AbiValueType,
) -> Result<Vec<f32>, EngineCallError> {
    let bytes = copied_bytes(value, expected)?;
    if bytes.len() % core::mem::size_of::<f32>() != 0 {
        return Err(EngineCallError::internal(
            "Godot fixed-layout Variant return has invalid storage",
        ));
    }
    Ok(bytes
        .chunks_exact(core::mem::size_of::<f32>())
        .map(|chunk| f32::from_ne_bytes(chunk.try_into().expect("f32 byte width")))
        .collect())
}
