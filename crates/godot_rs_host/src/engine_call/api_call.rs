use core::ffi::c_void;
use core::mem;
use core::ptr;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};

use godot_rs_api::abi::{
    AbiCallResult, AbiGodotApiKind, AbiGodotApiSpecV1, AbiPtrcallType, AbiStatus, AbiValueType,
    AbiValueV1,
};
use godot_rs_api::{
    GDExtensionConstTypePtr, GDExtensionMethodBindPtr, GDExtensionPtrBuiltInMethod,
    GDExtensionPtrConstructor, GDExtensionPtrGetter, GDExtensionPtrIndexedGetter,
    GDExtensionPtrIndexedSetter, GDExtensionPtrKeyedGetter, GDExtensionPtrKeyedSetter,
    GDExtensionPtrOperatorEvaluator, GDExtensionPtrSetter, GDExtensionPtrUtilityFunction,
    GDExtensionTypeFromVariantConstructorFunc, GDExtensionVariantOperator, GDExtensionVariantType,
};

use super::contract::{ApiContract, ValueContract};
use super::value::{NativeGodotRef, NativeValue, NativeValueOutput, ValueError};
use super::{
    EngineCallContext, EngineCallError, decode_native_argument, encode_returned_object,
    own_returned_callable, own_returned_dynamic, own_returned_math, own_returned_packed,
    own_returned_signal, own_returned_utf8, resolve_class_tag, resolve_value_tag,
    retain_returned_object, validate_argument_count,
};
use crate::dynamic_value::{NativeDynamic, builtin_variant_type};
use crate::interface::EngineInterface;
use crate::string_name::OwnedStringName;
use crate::variant_codec::OwnedVariant;

const MAX_CACHED_API_ENTRIES: usize = 16_384;
// Object.notification declares `what` as `int` with `int32` ptrcall metadata.
const NOTIFICATION_POSTINITIALIZE: i32 = 0;
const OBJECT_NOTIFICATION_HASH: i64 = 4_023_243_586;

static API_CACHE: OnceLock<Mutex<HashMap<u64, Arc<ResolvedApi>>>> = OnceLock::new();

#[derive(Clone, Debug)]
struct ResolvedApi {
    contract: ApiContract,
    target: ResolvedTarget,
    base_tag: usize,
    argument_tags: Vec<usize>,
    return_tag: usize,
    object_tag: usize,
}

#[derive(Clone, Copy, Debug)]
enum ResolvedTarget {
    Utility(usize),
    BuiltinConstructor(usize, GDExtensionVariantType),
    BuiltinMethod(usize),
    BuiltinOperator(usize),
    BuiltinGetter(usize),
    BuiltinSetter(usize),
    IndexedGetter(usize),
    IndexedSetter(usize),
    KeyedGetter(usize),
    KeyedSetter(usize),
    BuiltinConstant(GDExtensionVariantType),
    Singleton,
    ObjectConstructor { notification: usize },
}

/// Executes one generated utility, builtin, singleton, or object-construction
/// entry through the official GDExtension interface.
pub(crate) unsafe extern "C" fn call_godot_api_from_module(
    context: *mut c_void,
    spec: *const AbiGodotApiSpecV1,
    base: *const AbiValueV1,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
    updated_base: *mut AbiValueV1,
) -> AbiCallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The implementation validates every ABI pointer and performs
        // a synchronous deep copy of generated metadata before use.
        unsafe {
            call_godot_api(
                context,
                spec,
                base,
                arguments,
                argument_count,
                output,
                updated_base,
            )
        }
    }));
    match outcome {
        Ok(Ok(())) => AbiCallResult::OK,
        Ok(Err(error)) => error.into_abi(),
        Err(_) => AbiCallResult::failure(
            AbiStatus::Panic,
            "godot-rust caught a panic while calling a generated Godot API",
        ),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_godot_api(
    context: *mut c_void,
    spec: *const AbiGodotApiSpecV1,
    base: *const AbiValueV1,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
    updated_base: *mut AbiValueV1,
) -> Result<(), EngineCallError> {
    if output.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot API output pointer is null",
        ));
    }
    if argument_count != 0 && arguments.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot API arguments pointer is null",
        ));
    }
    let Some(interface) = crate::script_instance::active_engine_interface() else {
        return Err(EngineCallError::new(
            AbiStatus::Unsupported,
            "Godot APIs can only be called during a script callback or cooperative task",
        ));
    };
    if context.is_null() {
        return Err(EngineCallError::internal(
            "Host engine-call context is unavailable",
        ));
    }
    // SAFETY: The Host owns this context for the module generation.
    let engine_context = unsafe { &*context.cast::<EngineCallContext>() };
    // SAFETY: Generated static metadata remains readable for this call.
    let contract = unsafe { ApiContract::copy_from_abi(spec) }?;
    let count =
        validate_argument_count(argument_count, contract.arguments.len(), contract.is_vararg)?;
    let resolved = resolve_api(interface, contract)?;
    let profile_owner = resolved.contract.owner_name.as_deref().unwrap_or("Godot");
    let profile_member = resolved
        .contract
        .member_name
        .as_deref()
        .unwrap_or("generated_api");
    let _profile_scope = crate::profiler::ProfileScope::enter_native(profile_owner, profile_member);
    let has_base = resolved.contract.base_value.ptrcall_type != AbiPtrcallType::VOID;
    if has_base == base.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot API receiver does not match its generated contract",
        ));
    }
    if resolved.contract.mutates_base == updated_base.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot API updated receiver slot does not match its contract",
        ));
    }
    let arguments = if count == 0 {
        &[]
    } else {
        // SAFETY: Null was rejected and the count was bounded.
        unsafe { core::slice::from_raw_parts(arguments, count) }
    };
    let mut native_base = if has_base {
        // SAFETY: Presence was validated and this is a synchronous copy.
        let value = unsafe { *base };
        Some(decode_native_argument(
            interface,
            engine_context,
            &resolved.contract.base_value,
            value,
            resolved.base_tag,
            resolved.object_tag,
        )?)
    } else {
        None
    };
    let variant_contract = ValueContract {
        value_type: AbiValueType::VARIANT,
        ptrcall_type: AbiPtrcallType::VARIANT,
        class_name: None,
        typed_array_element: None,
    };
    let mut native_arguments = Vec::with_capacity(arguments.len());
    for (index, value) in arguments.iter().copied().enumerate() {
        let (contract, tag) = if index < resolved.contract.arguments.len() {
            (
                &resolved.contract.arguments[index],
                resolved.argument_tags[index],
            )
        } else {
            (&variant_contract, 0)
        };
        native_arguments.push(decode_native_argument(
            interface,
            engine_context,
            contract,
            value,
            tag,
            resolved.object_tag,
        )?);
    }
    let argument_pointers = native_arguments
        .iter()
        .map(NativeValue::as_const_ptr)
        .collect::<Vec<GDExtensionConstTypePtr>>();
    let mut native_output = NativeValue::empty_output(interface, &resolved.contract.return_value)?;

    match resolved.target {
        ResolvedTarget::Utility(function) => {
            let function =
                // SAFETY: The cached pointer came from the exact utility resolver.
                unsafe { mem::transmute::<usize, GDExtensionPtrUtilityFunction>(function) }
                    .expect("resolved utility function");
            // SAFETY: Generated argument and output contracts select exact
            // native storage for this official utility hash.
            unsafe {
                function(
                    native_output.as_mut_ptr(),
                    argument_pointers.as_ptr(),
                    i32::try_from(argument_pointers.len()).map_err(|_| {
                        EngineCallError::new(
                            AbiStatus::InvalidArgument,
                            "Godot utility argument count exceeds the engine ABI",
                        )
                    })?,
                )
            };
        }
        ResolvedTarget::BuiltinConstructor(function, variant_type) => {
            // SAFETY: The cached pointer came from the constructor resolver.
            let function = unsafe { mem::transmute::<usize, GDExtensionPtrConstructor>(function) }
                .expect("resolved builtin constructor");
            // SAFETY: The placement constructor immediately replaces this
            // exact initialized output storage.
            unsafe { native_output.prepare_builtin_constructor(interface, variant_type)? };
            // SAFETY: The generated constructor index fixes every argument
            // layout and output type.
            unsafe { function(native_output.as_mut_ptr(), argument_pointers.as_ptr()) };
        }
        ResolvedTarget::BuiltinMethod(function) => {
            let function =
                // SAFETY: The cached pointer came from the builtin method resolver.
                unsafe { mem::transmute::<usize, GDExtensionPtrBuiltInMethod>(function) }
                    .expect("resolved builtin method");
            // SAFETY: Static calls use null base; instance calls carry exact
            // builtin storage. Arguments and result match the official hash.
            unsafe {
                function(
                    native_base
                        .as_mut()
                        .map_or(ptr::null_mut(), NativeValue::as_mut_ptr),
                    argument_pointers.as_ptr(),
                    native_output.as_mut_ptr(),
                    i32::try_from(argument_pointers.len()).map_err(|_| {
                        EngineCallError::new(
                            AbiStatus::InvalidArgument,
                            "Godot builtin argument count exceeds the engine ABI",
                        )
                    })?,
                )
            };
        }
        ResolvedTarget::BuiltinOperator(function) => {
            let function =
                // SAFETY: The cached address came from the operator resolver and
                // is invoked with the same generated contract.
                unsafe { mem::transmute::<usize, GDExtensionPtrOperatorEvaluator>(function) }
                    .expect("resolved builtin operator");
            let left = native_base
                .as_ref()
                .expect("validated operator receiver")
                .as_const_ptr();
            let right = native_arguments
                .first()
                .map_or(ptr::null(), NativeValue::as_const_ptr);
            // SAFETY: The operator resolver fixed the left/right/result types.
            unsafe { function(left, right, native_output.as_mut_ptr()) };
        }
        ResolvedTarget::BuiltinGetter(function) => {
            // SAFETY: The cached address came from the builtin getter
            // resolver and is invoked with the same generated contract.
            let function = unsafe { mem::transmute::<usize, GDExtensionPtrGetter>(function) }
                .expect("resolved builtin getter");
            // SAFETY: Receiver/member/result match the generated contract.
            unsafe {
                function(
                    native_base
                        .as_ref()
                        .expect("validated getter receiver")
                        .as_const_ptr(),
                    native_output.as_mut_ptr(),
                )
            };
        }
        ResolvedTarget::BuiltinSetter(function) => {
            // SAFETY: The cached address came from the builtin setter
            // resolver and is invoked with the same generated contract.
            let function = unsafe { mem::transmute::<usize, GDExtensionPtrSetter>(function) }
                .expect("resolved builtin setter");
            // SAFETY: Receiver/member/value match the generated contract.
            unsafe {
                function(
                    native_base
                        .as_mut()
                        .expect("validated setter receiver")
                        .as_mut_ptr(),
                    native_arguments[0].as_const_ptr(),
                )
            };
        }
        ResolvedTarget::IndexedGetter(function) => {
            let function =
                // SAFETY: The cached address came from the indexed getter
                // resolver and is invoked with the same generated contract.
                unsafe { mem::transmute::<usize, GDExtensionPtrIndexedGetter>(function) }
                    .expect("resolved indexed getter");
            let index = native_arguments[0].as_i64().ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot builtin index is not an integer",
                )
            })?;
            // SAFETY: Receiver/index/result follow the indexed contract.
            unsafe {
                function(
                    native_base
                        .as_ref()
                        .expect("validated indexed receiver")
                        .as_const_ptr(),
                    index,
                    native_output.as_mut_ptr(),
                )
            };
        }
        ResolvedTarget::IndexedSetter(function) => {
            let function =
                // SAFETY: The cached address came from the indexed setter
                // resolver and is invoked with the same generated contract.
                unsafe { mem::transmute::<usize, GDExtensionPtrIndexedSetter>(function) }
                    .expect("resolved indexed setter");
            let index = native_arguments[0].as_i64().ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot builtin index is not an integer",
                )
            })?;
            // SAFETY: Receiver/index/value follow the indexed contract.
            unsafe {
                function(
                    native_base
                        .as_mut()
                        .expect("validated indexed receiver")
                        .as_mut_ptr(),
                    index,
                    native_arguments[1].as_const_ptr(),
                )
            };
        }
        ResolvedTarget::KeyedGetter(function) => {
            // SAFETY: The cached address came from the keyed getter resolver
            // and is invoked with the same generated contract.
            let function = unsafe { mem::transmute::<usize, GDExtensionPtrKeyedGetter>(function) }
                .expect("resolved keyed getter");
            // SAFETY: Keyed builtins use Variant keys and return Variants.
            unsafe {
                function(
                    native_base
                        .as_ref()
                        .expect("validated keyed receiver")
                        .as_const_ptr(),
                    native_arguments[0].as_const_ptr(),
                    native_output.as_mut_ptr(),
                )
            };
        }
        ResolvedTarget::KeyedSetter(function) => {
            // SAFETY: The cached address came from the keyed setter resolver
            // and is invoked with the same generated contract.
            let function = unsafe { mem::transmute::<usize, GDExtensionPtrKeyedSetter>(function) }
                .expect("resolved keyed setter");
            // SAFETY: Keyed builtins use Variant keys and values.
            unsafe {
                function(
                    native_base
                        .as_mut()
                        .expect("validated keyed receiver")
                        .as_mut_ptr(),
                    native_arguments[0].as_const_ptr(),
                    native_arguments[1].as_const_ptr(),
                )
            };
        }
        ResolvedTarget::BuiltinConstant(variant_type) => {
            native_output = read_builtin_constant(
                interface,
                variant_type,
                resolved
                    .contract
                    .member_name
                    .as_deref()
                    .expect("validated constant name"),
                &resolved.contract.return_value,
            )?;
        }
        ResolvedTarget::Singleton => {
            let value = get_singleton(interface, &resolved)?;
            // SAFETY: The output slot is live for this synchronous callback.
            unsafe { output.write(value) };
            return Ok(());
        }
        ResolvedTarget::ObjectConstructor { notification } => {
            let value = construct_object(context, interface, &resolved, notification)?;
            // SAFETY: The output slot is live for this synchronous callback.
            unsafe { output.write(value) };
            return Ok(());
        }
    }

    let output_value = encode_native_value(
        context,
        interface,
        engine_context,
        native_output,
        &resolved.contract.return_value,
        resolved.return_tag,
    )?;
    let updated_value = if resolved.contract.mutates_base {
        match encode_native_value(
            context,
            interface,
            engine_context,
            native_base.expect("validated mutable receiver"),
            &resolved.contract.base_value,
            resolved.base_tag,
        ) {
            Ok(value) => Some(value),
            Err(error) => {
                // The first owned result has not crossed the ABI yet. Release
                // it before reporting failure so a partial mutable call cannot
                // leak Host ownership.
                let _ = super::drop_host_value(context, output_value);
                return Err(error.into());
            }
        }
    } else {
        None
    };
    // Write only after both owned outputs have been produced successfully.
    // SAFETY: The project module retained these validated output slots.
    unsafe {
        output.write(output_value);
        if let Some(updated) = updated_value {
            updated_base.write(updated);
        }
    }
    Ok(())
}

fn encode_native_value(
    context: *mut c_void,
    interface: EngineInterface,
    engine_context: &EngineCallContext,
    value: NativeValue,
    contract: &ValueContract,
    class_tag: usize,
) -> Result<AbiValueV1, ValueError> {
    value.into_abi(
        contract,
        NativeValueOutput {
            object_id: |object| encode_returned_object(interface, object, class_tag),
            own_object_ref: |object, owned_ref| {
                retain_returned_object(context, interface, object, owned_ref, class_tag)
            },
            own_text: own_returned_utf8,
            own_math: own_returned_math,
            own_packed: own_returned_packed,
            own_dynamic: |value_type, value| {
                own_returned_dynamic(engine_context, value_type, value)
            },
            own_callable: |value| own_returned_callable(engine_context, value),
            own_signal: own_returned_signal,
        },
    )
}

fn resolve_api(
    interface: EngineInterface,
    contract: ApiContract,
) -> Result<Arc<ResolvedApi>, EngineCallError> {
    let cache = API_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let cache = cache
            .lock()
            .map_err(|_| EngineCallError::internal("Godot API cache is poisoned"))?;
        if let Some(cached) = cache.get(&contract.id) {
            if cached.contract != contract {
                return Err(EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot API contract ID collides with different metadata",
                ));
            }
            return Ok(Arc::clone(cached));
        }
        if cache.len() >= MAX_CACHED_API_ENTRIES {
            return Err(EngineCallError::new(
                AbiStatus::Unsupported,
                "Godot API cache limit has been reached",
            ));
        }
    }
    let resolved = Arc::new(resolve_api_uncached(interface, contract)?);
    let mut cache = cache
        .lock()
        .map_err(|_| EngineCallError::internal("Godot API cache is poisoned"))?;
    if let Some(cached) = cache.get(&resolved.contract.id) {
        if cached.contract != resolved.contract {
            return Err(EngineCallError::new(
                AbiStatus::InvalidArgument,
                "Godot API contract ID collides with different metadata",
            ));
        }
        return Ok(Arc::clone(cached));
    }
    cache.insert(resolved.contract.id, Arc::clone(&resolved));
    Ok(resolved)
}

fn resolve_api_uncached(
    interface: EngineInterface,
    contract: ApiContract,
) -> Result<ResolvedApi, EngineCallError> {
    let owner = contract.owner_name.as_deref();
    let member = contract.member_name.as_deref();
    let variant_type = || {
        builtin_variant_type(owner.expect("validated builtin owner")).ok_or_else(|| {
            EngineCallError::new(
                AbiStatus::Unsupported,
                "generated Godot builtin type is unavailable",
            )
        })
    };
    let member_name = || {
        OwnedStringName::new(interface, member.expect("validated API member"))
            .ok_or_else(|| EngineCallError::internal("could not create Godot API member name"))
    };
    let target = match contract.kind {
        AbiGodotApiKind::UTILITY_FUNCTION => {
            let name = member_name()?;
            let hash = i64::try_from(contract.numeric).map_err(|_| {
                EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot utility hash exceeds the engine ABI",
                )
            })?;
            let get = interface.variant_get_ptr_utility_function.ok_or_else(|| {
                EngineCallError::internal("Godot utility function lookup is unavailable")
            })?;
            // SAFETY: Name and hash come from authenticated generated metadata.
            let function = unsafe { get(name.as_ptr(), hash) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot utility function is unavailable in this engine",
                )
            })?;
            ResolvedTarget::Utility(function as usize)
        }
        AbiGodotApiKind::BUILTIN_CONSTRUCTOR => {
            let type_ = variant_type()?;
            let index = i32::try_from(contract.numeric).map_err(|_| {
                EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot builtin constructor index exceeds the engine ABI",
                )
            })?;
            let get = interface.variant_get_ptr_constructor.ok_or_else(|| {
                EngineCallError::internal("Godot builtin constructor lookup is unavailable")
            })?;
            // SAFETY: Type and index come from authenticated metadata.
            let function = unsafe { get(type_, index) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot builtin constructor is unavailable in this engine",
                )
            })?;
            ResolvedTarget::BuiltinConstructor(function as usize, type_)
        }
        AbiGodotApiKind::BUILTIN_METHOD => {
            let type_ = variant_type()?;
            let name = member_name()?;
            let hash = i64::try_from(contract.numeric).map_err(|_| {
                EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot builtin method hash exceeds the engine ABI",
                )
            })?;
            let get = interface.variant_get_ptr_builtin_method.ok_or_else(|| {
                EngineCallError::internal("Godot builtin method lookup is unavailable")
            })?;
            // SAFETY: Type, name, and hash are authenticated metadata.
            let function = unsafe { get(type_, name.as_ptr(), hash) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot builtin method is unavailable in this engine",
                )
            })?;
            ResolvedTarget::BuiltinMethod(function as usize)
        }
        AbiGodotApiKind::BUILTIN_OPERATOR => {
            let type_ = variant_type()?;
            let operator = u32::try_from(contract.numeric)
                .ok()
                .filter(|operator| *operator < 25)
                .map(GDExtensionVariantOperator)
                .ok_or_else(|| {
                    EngineCallError::new(
                        AbiStatus::InvalidArgument,
                        "Godot builtin operator ordinal is invalid",
                    )
                })?;
            let right_type = contract.arguments.first().map_or(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
                |value| ptrcall_variant_type(value.ptrcall_type),
            );
            let get = interface
                .variant_get_ptr_operator_evaluator
                .ok_or_else(|| {
                    EngineCallError::internal("Godot builtin operator lookup is unavailable")
                })?;
            // SAFETY: Operator and both operand types are generated metadata.
            let function = unsafe { get(operator, type_, right_type) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot builtin operator is unavailable in this engine",
                )
            })?;
            ResolvedTarget::BuiltinOperator(function as usize)
        }
        AbiGodotApiKind::BUILTIN_MEMBER_GETTER => {
            let get = interface.variant_get_ptr_getter.ok_or_else(|| {
                EngineCallError::internal("Godot builtin getter lookup is unavailable")
            })?;
            let name = member_name()?;
            // SAFETY: Type and member come from generated metadata.
            let function = unsafe { get(variant_type()?, name.as_ptr()) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot builtin getter is unavailable in this engine",
                )
            })?;
            ResolvedTarget::BuiltinGetter(function as usize)
        }
        AbiGodotApiKind::BUILTIN_MEMBER_SETTER => {
            let get = interface.variant_get_ptr_setter.ok_or_else(|| {
                EngineCallError::internal("Godot builtin setter lookup is unavailable")
            })?;
            let name = member_name()?;
            // SAFETY: Type and member come from generated metadata.
            let function = unsafe { get(variant_type()?, name.as_ptr()) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot builtin setter is unavailable in this engine",
                )
            })?;
            ResolvedTarget::BuiltinSetter(function as usize)
        }
        AbiGodotApiKind::BUILTIN_INDEXED_GETTER => {
            let get = interface.variant_get_ptr_indexed_getter.ok_or_else(|| {
                EngineCallError::internal("Godot indexed getter lookup is unavailable")
            })?;
            // SAFETY: Type comes from generated metadata.
            let function = unsafe { get(variant_type()?) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot indexed getter is unavailable in this engine",
                )
            })?;
            ResolvedTarget::IndexedGetter(function as usize)
        }
        AbiGodotApiKind::BUILTIN_INDEXED_SETTER => {
            let get = interface.variant_get_ptr_indexed_setter.ok_or_else(|| {
                EngineCallError::internal("Godot indexed setter lookup is unavailable")
            })?;
            // SAFETY: Type comes from generated metadata.
            let function = unsafe { get(variant_type()?) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot indexed setter is unavailable in this engine",
                )
            })?;
            ResolvedTarget::IndexedSetter(function as usize)
        }
        AbiGodotApiKind::BUILTIN_KEYED_GETTER => {
            let get = interface.variant_get_ptr_keyed_getter.ok_or_else(|| {
                EngineCallError::internal("Godot keyed getter lookup is unavailable")
            })?;
            // SAFETY: Type comes from generated metadata.
            let function = unsafe { get(variant_type()?) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot keyed getter is unavailable in this engine",
                )
            })?;
            ResolvedTarget::KeyedGetter(function as usize)
        }
        AbiGodotApiKind::BUILTIN_KEYED_SETTER => {
            let get = interface.variant_get_ptr_keyed_setter.ok_or_else(|| {
                EngineCallError::internal("Godot keyed setter lookup is unavailable")
            })?;
            // SAFETY: Type comes from generated metadata.
            let function = unsafe { get(variant_type()?) }.ok_or_else(|| {
                EngineCallError::new(
                    AbiStatus::Unsupported,
                    "generated Godot keyed setter is unavailable in this engine",
                )
            })?;
            ResolvedTarget::KeyedSetter(function as usize)
        }
        AbiGodotApiKind::BUILTIN_CONSTANT => ResolvedTarget::BuiltinConstant(variant_type()?),
        AbiGodotApiKind::SINGLETON => ResolvedTarget::Singleton,
        AbiGodotApiKind::OBJECT_CONSTRUCTOR => {
            let object = OwnedStringName::new(interface, "Object")
                .ok_or_else(|| EngineCallError::internal("could not create Object class name"))?;
            let notification =
                OwnedStringName::new(interface, "notification").ok_or_else(|| {
                    EngineCallError::internal("could not create Object.notification name")
                })?;
            let get = interface.classdb_get_method_bind.ok_or_else(|| {
                EngineCallError::internal("Godot MethodBind lookup is unavailable")
            })?;
            // Verify that construction support exists now; the actual class
            // is instantiated exactly once when the user requests it.
            let _construct = interface.classdb_construct_object2.ok_or_else(|| {
                EngineCallError::internal("Godot object construction is unavailable")
            })?;
            // SAFETY: Object and method names plus hash are official metadata.
            let notification = unsafe {
                get(
                    object.as_ptr(),
                    notification.as_ptr(),
                    OBJECT_NOTIFICATION_HASH,
                )
            };
            if notification.is_null() {
                return Err(EngineCallError::new(
                    AbiStatus::Unsupported,
                    "Object.notification is unavailable in this engine",
                ));
            }
            ResolvedTarget::ObjectConstructor {
                notification: notification as usize,
            }
        }
        _ => {
            return Err(EngineCallError::new(
                AbiStatus::Unsupported,
                "generated Godot API operation is unavailable",
            ));
        }
    };
    let base_tag = resolve_value_tag(interface, &contract.base_value)?;
    let argument_tags = contract
        .arguments
        .iter()
        .map(|value| resolve_value_tag(interface, value))
        .collect::<Result<Vec<_>, _>>()?;
    let return_tag = resolve_value_tag(interface, &contract.return_value)?;
    let needs_object_tag = contract.arguments.iter().any(|value| {
        matches!(
            value.ptrcall_type,
            AbiPtrcallType::CALLABLE | AbiPtrcallType::SIGNAL
        )
    });
    let object_tag = if needs_object_tag {
        resolve_class_tag(interface, "Object")?
    } else {
        0
    };
    Ok(ResolvedApi {
        contract,
        target,
        base_tag,
        argument_tags,
        return_tag,
        object_tag,
    })
}

fn ptrcall_variant_type(type_: AbiPtrcallType) -> GDExtensionVariantType {
    GDExtensionVariantType(match type_ {
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

fn read_builtin_constant(
    interface: EngineInterface,
    variant_type: GDExtensionVariantType,
    name: &str,
    contract: &ValueContract,
) -> Result<NativeValue, EngineCallError> {
    let name = OwnedStringName::new(interface, name)
        .ok_or_else(|| EngineCallError::internal("could not create builtin constant name"))?;
    let get = interface
        .variant_get_constant_value
        .ok_or_else(|| EngineCallError::internal("Godot builtin constant lookup is unavailable"))?;
    let mut variant = OwnedVariant::uninitialized(interface);
    // SAFETY: The destination is aligned uninitialized Variant storage.
    unsafe { get(variant_type, name.as_ptr(), variant.as_mut_ptr()) };
    variant.mark_initialized();
    if contract.ptrcall_type == AbiPtrcallType::VARIANT {
        return Ok(NativeValue::Dynamic(Box::new(NativeDynamic::Variant(
            variant,
        ))));
    }
    let output_type = ptrcall_variant_type(contract.ptrcall_type);
    let converter = interface.get_variant_to_type_constructor.ok_or_else(|| {
        EngineCallError::internal("Godot Variant conversion lookup is unavailable")
    })?;
    // SAFETY: Return type comes from authenticated constant metadata.
    let converter: GDExtensionTypeFromVariantConstructorFunc = unsafe { converter(output_type) };
    let converter = converter.ok_or_else(|| {
        EngineCallError::new(
            AbiStatus::Unsupported,
            "Godot builtin constant cannot be converted to its generated type",
        )
    })?;
    let mut output = NativeValue::empty_output(interface, contract)?;
    // SAFETY: The conversion placement-constructs the exact output type.
    unsafe { output.prepare_builtin_constructor(interface, output_type)? };
    // SAFETY: Destination and source match the resolver-selected conversion.
    unsafe { converter(output.as_mut_ptr(), variant.as_mut_ptr()) };
    Ok(output)
}

fn get_singleton(
    interface: EngineInterface,
    resolved: &ResolvedApi,
) -> Result<AbiValueV1, EngineCallError> {
    let name = OwnedStringName::new(
        interface,
        resolved
            .contract
            .member_name
            .as_deref()
            .expect("validated singleton name"),
    )
    .ok_or_else(|| EngineCallError::internal("could not create singleton name"))?;
    let get = interface
        .global_get_singleton
        .ok_or_else(|| EngineCallError::internal("Godot singleton lookup is unavailable"))?;
    // SAFETY: The generated singleton name is one initialized StringName.
    let object = unsafe { get(name.as_ptr()) };
    if object.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::Unsupported,
            "generated Godot singleton is unavailable in this engine",
        ));
    }
    encode_returned_object(interface, object, resolved.return_tag)
        .map(AbiValueV1::from_object_id)
        .map_err(Into::into)
}

fn construct_object(
    context: *mut c_void,
    interface: EngineInterface,
    resolved: &ResolvedApi,
    notification: usize,
) -> Result<AbiValueV1, EngineCallError> {
    let class = OwnedStringName::new(
        interface,
        resolved
            .contract
            .owner_name
            .as_deref()
            .expect("validated constructor class"),
    )
    .ok_or_else(|| EngineCallError::internal("could not create Godot class name"))?;
    let construct = interface
        .classdb_construct_object2
        .ok_or_else(|| EngineCallError::internal("Godot object construction is unavailable"))?;
    // SAFETY: Class name comes from the authenticated instantiable class.
    let object = unsafe { construct(class.as_ptr()) };
    if object.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::Unsupported,
            "Godot class construction returned null",
        ));
    }
    let notification: GDExtensionMethodBindPtr = ptr::with_exposed_provenance_mut(notification);
    let ptrcall = interface.object_method_bind_ptrcall.ok_or_else(|| {
        EngineCallError::internal("Godot post-initialize notification is unavailable")
    })?;
    let reversed = 0_u8;
    let arguments: [GDExtensionConstTypePtr; 2] = [
        ptr::from_ref(&NOTIFICATION_POSTINITIALIZE).cast(),
        ptr::from_ref(&reversed).cast(),
    ];
    // SAFETY: MethodBind and arguments match Object.notification(int, bool).
    unsafe { ptrcall(notification, object, arguments.as_ptr(), ptr::null_mut()) };
    match resolved.contract.return_value.ptrcall_type {
        AbiPtrcallType::OBJECT => {
            match encode_returned_object(interface, object, resolved.return_tag) {
                Ok(id) => Ok(AbiValueV1::from_object_id(id)),
                Err(error) => {
                    if let Some(destroy) = interface.object_destroy {
                        // SAFETY: Construction succeeded, the object has not
                        // escaped, and result validation failed.
                        unsafe { destroy(object) };
                    }
                    Err(error.into())
                }
            }
        }
        AbiPtrcallType::REFCOUNTED_OBJECT => {
            let owned = NativeGodotRef::from_object(interface, object)?;
            retain_returned_object(context, interface, object, owned, resolved.return_tag)
                .map_err(Into::into)
        }
        _ => unreachable!("validated object-constructor return type"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_api_calls_fail_closed_and_validate_buffers_first() {
        // SAFETY: Null output is rejected before any other pointer is read.
        let missing_output = unsafe {
            call_godot_api_from_module(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(missing_output.status, AbiStatus::InvalidArgument);

        let mut output = AbiValueV1::NIL;
        // SAFETY: A non-zero argument count requires a non-null argument
        // buffer and is rejected before engine context lookup.
        let missing_arguments = unsafe {
            call_godot_api_from_module(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                1,
                &mut output,
                ptr::null_mut(),
            )
        };
        assert_eq!(missing_arguments.status, AbiStatus::InvalidArgument);

        // SAFETY: With valid empty buffers, the absent active script callback
        // is rejected before the null contract can be read.
        let inactive = unsafe {
            call_godot_api_from_module(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                &mut output,
                ptr::null_mut(),
            )
        };
        assert_eq!(inactive.status, AbiStatus::Unsupported);
        assert_eq!(output, AbiValueV1::NIL);
    }
}
