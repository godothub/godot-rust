mod api_call;
mod contract;
pub(crate) mod value;
mod vararg;

use core::ffi::c_void;
use core::ptr;
use std::collections::{HashMap, hash_map::Entry};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};

use godot_api::abi::{
    ABI_VALUE_OWNED_BYTES, ABI_VALUE_OWNED_CALLABLE, ABI_VALUE_OWNED_DYNAMIC_GROUP,
    ABI_VALUE_OWNED_OBJECT_REF, ABI_VALUE_OWNED_UTF8, AbiCallResult, AbiGodotMethodSpecV1,
    AbiStatus, AbiValueType, AbiValueV1, callable_value_ownership_token,
    dynamic_value_ownership_token,
};
use godot_api::{GDExtensionConstTypePtr, GDExtensionMethodBindPtr, GDExtensionObjectPtr};

use self::contract::{ContractError, MethodContract, ValueContract};
use self::value::{NativeGodotRef, NativeValue, NativeValueInput, NativeValueOutput, ValueError};
use crate::callable_value::NativeCallable;
use crate::dynamic_value::{NativeDynamic, set_dynamic_ownership};
use crate::interface::EngineInterface;
use crate::node_path::OwnedNodePath;
use crate::packed_array::OwnedPackedArray;
use crate::signal_value::NativeSignal;
use crate::string_name::OwnedStringName;

pub(crate) use api_call::call_godot_api_from_module;

const MAX_CACHED_METHODS: usize = 65_536;
const MAX_OWNED_RETURN_VALUES: usize = 65_536;
const MAX_METHOD_CALL_ARGUMENTS: usize = 1_024;

static METHOD_CACHE: OnceLock<Mutex<HashMap<u64, Arc<ResolvedMethod>>>> = OnceLock::new();
static OWNED_RETURN_VALUES: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();

pub(crate) struct EngineCallContext {
    refs: Mutex<OwnedGodotRefs>,
    dynamics: Mutex<OwnedDynamicValues>,
    callables: Mutex<OwnedCallableValues>,
}

struct OwnedGodotRefs {
    next_token: u64,
    values: HashMap<u64, NativeGodotRef>,
}

struct OwnedDynamicValues {
    next_token: u64,
    values: HashMap<u64, (usize, NativeDynamic, Vec<u64>)>,
}

struct OwnedCallableValues {
    next_token: u64,
    values: HashMap<u64, (usize, NativeCallable)>,
}

impl EngineCallContext {
    pub(crate) fn new() -> Self {
        Self {
            refs: Mutex::new(OwnedGodotRefs {
                next_token: 1,
                values: HashMap::new(),
            }),
            dynamics: Mutex::new(OwnedDynamicValues {
                next_token: 1,
                values: HashMap::new(),
            }),
            callables: Mutex::new(OwnedCallableValues {
                next_token: 1,
                values: HashMap::new(),
            }),
        }
    }

    fn retain(&self, value: NativeGodotRef) -> Result<u64, ValueError> {
        let mut refs = self.refs.lock().map_err(|_| {
            ValueError::new(
                AbiStatus::Internal,
                "Host RefCounted return registry is poisoned",
            )
        })?;
        if refs.values.len() >= MAX_OWNED_RETURN_VALUES {
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Host RefCounted return limit has been reached",
            ));
        }
        let token = refs.next_token;
        refs.next_token = refs.next_token.checked_add(1).ok_or_else(|| {
            ValueError::new(
                AbiStatus::Unsupported,
                "Host RefCounted return token space is exhausted",
            )
        })?;
        if token == 0 || refs.values.insert(token, value).is_some() {
            return Err(ValueError::new(
                AbiStatus::Internal,
                "Host RefCounted return token collided",
            ));
        }
        Ok(token)
    }

    pub(crate) fn retain_refcounted_object(
        &self,
        interface: EngineInterface,
        object_id: u64,
    ) -> Result<AbiValueV1, ValueError> {
        if object_id == 0 {
            return Ok(AbiValueV1::from_object_id(0));
        }
        let get_instance = interface.object_get_instance_from_id.ok_or_else(|| {
            ValueError::new(
                AbiStatus::Internal,
                "Godot object lookup is unavailable for a Resource property",
            )
        })?;
        // SAFETY: Godot instance IDs are opaque integers accepted by ObjectDB.
        let object = unsafe { get_instance(object_id) };
        if object.is_null() {
            return Err(ValueError::new(
                AbiStatus::StaleHandle,
                "Resource property object no longer exists",
            ));
        }
        let ownership = NativeGodotRef::from_object(interface, object)?;
        let token = self.retain(ownership)?;
        Ok(AbiValueV1 {
            type_: AbiValueType::OBJECT_ID,
            reserved_flags: ABI_VALUE_OWNED_OBJECT_REF,
            payload: [object_id, token],
        })
    }

    fn release(&self, token: u64) -> AbiStatus {
        let Ok(mut refs) = self.refs.lock() else {
            return AbiStatus::Internal;
        };
        let Some(value) = refs.values.remove(&token) else {
            return AbiStatus::InvalidArgument;
        };
        drop(refs);
        drop(value);
        AbiStatus::Ok
    }

    pub(crate) fn retain_dynamic(
        &self,
        value: NativeDynamic,
        callable_tokens: Vec<u64>,
    ) -> Result<u64, ValueError> {
        let mut dynamics = match self.dynamics.lock() {
            Ok(dynamics) => dynamics,
            Err(_) => {
                self.release_callable_tokens(&callable_tokens);
                return Err(ValueError::new(
                    AbiStatus::Internal,
                    "Host dynamic-value registry is poisoned",
                ));
            }
        };
        if dynamics.values.len() >= MAX_OWNED_RETURN_VALUES {
            drop(dynamics);
            self.release_callable_tokens(&callable_tokens);
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Host dynamic-value limit has been reached",
            ));
        }
        let token = dynamics.next_token;
        let Some(next_token) = dynamics.next_token.checked_add(1) else {
            drop(dynamics);
            self.release_callable_tokens(&callable_tokens);
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Host dynamic-value token space is exhausted",
            ));
        };
        if token == 0 || dynamics.values.contains_key(&token) {
            drop(dynamics);
            self.release_callable_tokens(&callable_tokens);
            return Err(ValueError::new(
                AbiStatus::Internal,
                "Host dynamic-value token collided",
            ));
        }
        dynamics.next_token = next_token;
        dynamics.values.insert(token, (1, value, callable_tokens));
        Ok(token)
    }

    fn release_callable_tokens(&self, tokens: &[u64]) {
        for token in tokens {
            let _ = self.release_callable(*token);
        }
    }

    fn clone_dynamic(&self, token: u64) -> AbiStatus {
        let Ok(mut dynamics) = self.dynamics.lock() else {
            return AbiStatus::Internal;
        };
        let Some((references, _, _)) = dynamics.values.get_mut(&token) else {
            return AbiStatus::InvalidArgument;
        };
        let Some(next) = references.checked_add(1) else {
            return AbiStatus::Unsupported;
        };
        *references = next;
        AbiStatus::Ok
    }

    pub(crate) fn release_dynamic(&self, token: u64) -> AbiStatus {
        let Ok(mut dynamics) = self.dynamics.lock() else {
            return AbiStatus::Internal;
        };
        let Some((references, _, _)) = dynamics.values.get_mut(&token) else {
            return AbiStatus::InvalidArgument;
        };
        if *references > 1 {
            *references -= 1;
            return AbiStatus::Ok;
        }
        let Some((_, value, callable_tokens)) = dynamics.values.remove(&token) else {
            return AbiStatus::Internal;
        };
        drop(dynamics);
        drop(value);
        for token in callable_tokens {
            let _ = self.release_callable(token);
        }
        AbiStatus::Ok
    }

    pub(crate) fn retain_callable(&self, value: NativeCallable) -> Result<u64, ValueError> {
        let mut callables = self.callables.lock().map_err(|_| {
            ValueError::new(AbiStatus::Internal, "Host Callable registry is poisoned")
        })?;
        if callables.values.len() >= MAX_OWNED_RETURN_VALUES {
            return Err(ValueError::new(
                AbiStatus::Unsupported,
                "Host Callable limit has been reached",
            ));
        }
        let token = callables.next_token;
        callables.next_token = callables.next_token.checked_add(1).ok_or_else(|| {
            ValueError::new(
                AbiStatus::Unsupported,
                "Host Callable token space is exhausted",
            )
        })?;
        if token == 0 || callables.values.insert(token, (1, value)).is_some() {
            return Err(ValueError::new(
                AbiStatus::Internal,
                "Host Callable token collided",
            ));
        }
        Ok(token)
    }

    fn clone_callable(&self, token: u64) -> AbiStatus {
        let Ok(mut callables) = self.callables.lock() else {
            return AbiStatus::Internal;
        };
        let Some((references, _)) = callables.values.get_mut(&token) else {
            return AbiStatus::InvalidArgument;
        };
        let Some(next) = references.checked_add(1) else {
            return AbiStatus::Unsupported;
        };
        *references = next;
        AbiStatus::Ok
    }

    pub(crate) fn copy_callable(&self, token: u64) -> Result<NativeCallable, ValueError> {
        let callables = self.callables.lock().map_err(|_| {
            ValueError::new(AbiStatus::Internal, "Host Callable registry is poisoned")
        })?;
        callables
            .values
            .get(&token)
            .ok_or_else(|| ValueError::invalid("Host Callable token is invalid"))?
            .1
            .copy_value()
    }

    pub(crate) fn release_callable(&self, token: u64) -> AbiStatus {
        let Ok(mut callables) = self.callables.lock() else {
            return AbiStatus::Internal;
        };
        let Some((references, _)) = callables.values.get_mut(&token) else {
            return AbiStatus::InvalidArgument;
        };
        if *references > 1 {
            *references -= 1;
            return AbiStatus::Ok;
        }
        let Some((_, value)) = callables.values.remove(&token) else {
            return AbiStatus::Internal;
        };
        drop(callables);
        drop(value);
        AbiStatus::Ok
    }
}

struct ResolvedMethod {
    contract: MethodContract,
    method_bind: usize,
    receiver_tag: usize,
    argument_tags: Vec<usize>,
    return_tag: usize,
}

#[derive(Clone, Copy)]
struct EngineCallError {
    status: AbiStatus,
    message: &'static str,
}

impl EngineCallError {
    const fn new(status: AbiStatus, message: &'static str) -> Self {
        Self { status, message }
    }

    const fn internal(message: &'static str) -> Self {
        Self::new(AbiStatus::Internal, message)
    }

    fn into_abi(self) -> AbiCallResult {
        AbiCallResult::failure(self.status, self.message)
    }
}

impl From<ContractError> for EngineCallError {
    fn from(value: ContractError) -> Self {
        Self::new(value.status, value.message)
    }
}

impl From<ValueError> for EngineCallError {
    fn from(value: ValueError) -> Self {
        Self::new(value.status, value.message)
    }
}

fn own_returned_utf8(value_type: AbiValueType, value: String) -> Result<AbiValueV1, ValueError> {
    if !matches!(
        value_type,
        AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH
    ) {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Host attempted to retain a non-text engine value",
        ));
    }
    retain_returned_bytes(
        value_type,
        value.into_bytes().into_boxed_slice(),
        ABI_VALUE_OWNED_UTF8,
    )
}

fn own_returned_math(
    value_type: AbiValueType,
    components: &[f32],
) -> Result<AbiValueV1, ValueError> {
    if !matches!(
        value_type,
        AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION
    ) {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Host attempted to retain an unsupported fixed-layout value",
        ));
    }
    let bytes = components
        .iter()
        .flat_map(|component| component.to_ne_bytes())
        .collect::<Vec<_>>()
        .into_boxed_slice();
    retain_returned_bytes(value_type, bytes, ABI_VALUE_OWNED_BYTES)
}

fn own_returned_packed(value_type: AbiValueType, bytes: Vec<u8>) -> Result<AbiValueV1, ValueError> {
    if crate::packed_array::PackedArrayKind::from_value_type(value_type).is_none() {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Host attempted to retain an unsupported packed-array value",
        ));
    }
    retain_returned_bytes(value_type, bytes.into_boxed_slice(), ABI_VALUE_OWNED_BYTES)
}

fn own_returned_dynamic(
    context: &EngineCallContext,
    value_type: AbiValueType,
    value: NativeDynamic,
) -> Result<AbiValueV1, ValueError> {
    if !matches!(
        value_type,
        AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY
    ) {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Host attempted to retain an unsupported dynamic value",
        ));
    }
    let encoded = value.to_bytes(Some(context))?;
    let mut bytes = encoded.bytes;
    let token = context.retain_dynamic(value, encoded.callable_tokens)?;
    if let Err(error) = set_dynamic_ownership(&mut bytes, token) {
        let _ = context.release_dynamic(token);
        return Err(error);
    }
    match retain_returned_bytes(value_type, bytes.into_boxed_slice(), ABI_VALUE_OWNED_BYTES) {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = context.release_dynamic(token);
            Err(error)
        }
    }
}

fn own_returned_callable(
    context: &EngineCallContext,
    value: NativeCallable,
) -> Result<AbiValueV1, ValueError> {
    let token = context.retain_callable(value)?;
    let bytes = match context
        .copy_callable(token)
        .and_then(|copy| copy.to_bytes(token))
    {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = context.release_callable(token);
            return Err(error);
        }
    };
    match retain_returned_bytes(
        AbiValueType::CALLABLE,
        bytes.into_boxed_slice(),
        ABI_VALUE_OWNED_BYTES,
    ) {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = context.release_callable(token);
            Err(error)
        }
    }
}

fn own_returned_signal(value: NativeSignal) -> Result<AbiValueV1, ValueError> {
    retain_returned_bytes(
        AbiValueType::SIGNAL,
        value.to_bytes()?.into_boxed_slice(),
        ABI_VALUE_OWNED_BYTES,
    )
}

fn retain_returned_bytes(
    value_type: AbiValueType,
    bytes: Box<[u8]>,
    ownership: u32,
) -> Result<AbiValueV1, ValueError> {
    let registry = OWNED_RETURN_VALUES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().map_err(|_| {
        ValueError::new(
            AbiStatus::Internal,
            "Host return-value registry is poisoned",
        )
    })?;
    if registry.len() >= MAX_OWNED_RETURN_VALUES {
        return Err(ValueError::new(
            AbiStatus::Unsupported,
            "Host return-value limit has been reached",
        ));
    }
    let length = bytes.len();
    let pointer = Box::into_raw(bytes) as *mut u8;
    match registry.entry(pointer as usize) {
        Entry::Vacant(entry) => {
            entry.insert(length);
        }
        Entry::Occupied(_) => {
            // SAFETY: A live allocation cannot share its data pointer with
            // another retained allocation. Reconstruct this unexpected
            // allocation so the registry keeps its prior owner.
            unsafe {
                drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
                    pointer, length,
                )));
            }
            return Err(ValueError::new(
                AbiStatus::Internal,
                "Host return-value allocation collided",
            ));
        }
    }
    Ok(AbiValueV1 {
        type_: value_type,
        reserved_flags: ownership,
        payload: [
            pointer as usize as u64,
            u64::try_from(length).expect("supported pointer widths fit the value ABI"),
        ],
    })
}

/// Releases a Host-owned engine result after the project SDK has copied it.
pub(crate) unsafe extern "C" fn drop_host_value_from_module(
    context: *mut c_void,
    value: AbiValueV1,
) -> AbiStatus {
    match catch_unwind(AssertUnwindSafe(|| drop_host_value(context, value))) {
        Ok(status) => status,
        Err(_) => AbiStatus::Panic,
    }
}

pub(crate) unsafe extern "C" fn retain_dynamic_value_from_module(
    context: *mut c_void,
    token: u64,
) -> AbiStatus {
    match catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || token == 0 {
            return AbiStatus::InvalidArgument;
        }
        // SAFETY: The Host table retains this exact context for the complete
        // project-module generation.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        context.clone_dynamic(token)
    })) {
        Ok(status) => status,
        Err(_) => AbiStatus::Panic,
    }
}

pub(crate) unsafe extern "C" fn retain_callable_value_from_module(
    context: *mut c_void,
    token: u64,
) -> AbiStatus {
    match catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || token == 0 {
            return AbiStatus::InvalidArgument;
        }
        // SAFETY: The Host table retains this exact context for the complete
        // project-module generation.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        context.clone_callable(token)
    })) {
        Ok(status) => status,
        Err(_) => AbiStatus::Panic,
    }
}

fn drop_host_value(context: *mut c_void, value: AbiValueV1) -> AbiStatus {
    if value.type_ == AbiValueType::OBJECT_ID
        && value.reserved_flags == ABI_VALUE_OWNED_OBJECT_REF
        && value.payload[0] != 0
        && value.payload[1] != 0
    {
        if context.is_null() {
            return AbiStatus::InvalidArgument;
        }
        // SAFETY: The Host table supplies its retained EngineCallContext as
        // the callback context for the complete module generation.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        return context.release(value.payload[1]);
    }
    if value.reserved_flags == ABI_VALUE_OWNED_DYNAMIC_GROUP
        && matches!(
            value.type_,
            AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY
        )
        && value.payload[0] != 0
        && value.payload[1] == 0
    {
        if context.is_null() {
            return AbiStatus::InvalidArgument;
        }
        // SAFETY: See the object-reference ownership branch above.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        return context.release_dynamic(value.payload[0]);
    }
    if value.type_ == AbiValueType::CALLABLE
        && value.reserved_flags == ABI_VALUE_OWNED_CALLABLE
        && value.payload[0] != 0
        && value.payload[1] == 0
    {
        if context.is_null() {
            return AbiStatus::InvalidArgument;
        }
        // SAFETY: See the object-reference ownership branch above.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        return context.release_callable(value.payload[0]);
    }
    let valid_text = matches!(
        value.type_,
        AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH
    ) && value.reserved_flags == ABI_VALUE_OWNED_UTF8;
    let valid_bytes = matches!(
        value.type_,
        AbiValueType::TRANSFORM2D
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
            | AbiValueType::CALLABLE
            | AbiValueType::SIGNAL
    ) && value.reserved_flags == ABI_VALUE_OWNED_BYTES;
    if !valid_text && !valid_bytes {
        return AbiStatus::InvalidArgument;
    }
    let Ok(address) = usize::try_from(value.payload[0]) else {
        return AbiStatus::InvalidArgument;
    };
    let Ok(length) = usize::try_from(value.payload[1]) else {
        return AbiStatus::InvalidArgument;
    };
    if address == 0 {
        return AbiStatus::InvalidArgument;
    }
    let registry = OWNED_RETURN_VALUES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut registry) = registry.lock() else {
        return AbiStatus::Internal;
    };
    if registry.get(&address).copied() != Some(length) {
        return AbiStatus::InvalidArgument;
    }
    // SAFETY: The registry authenticates this exact bounded allocation.
    let dynamic_token = matches!(
        value.type_,
        AbiValueType::VARIANT | AbiValueType::ARRAY | AbiValueType::DICTIONARY
    )
    // SAFETY: The registry authenticates this exact bounded allocation.
    .then(|| unsafe { core::slice::from_raw_parts(address as *const u8, length) })
    .and_then(dynamic_value_ownership_token);
    let callable_token = (value.type_ == AbiValueType::CALLABLE)
        // SAFETY: The registry authenticates this exact bounded allocation.
        .then(|| unsafe { core::slice::from_raw_parts(address as *const u8, length) })
        .and_then(callable_value_ownership_token);
    if (dynamic_token.is_some() || callable_token.is_some()) && context.is_null() {
        return AbiStatus::InvalidArgument;
    }
    registry.remove(&address);
    drop(registry);
    let slice = ptr::slice_from_raw_parts_mut(address as *mut u8, length);
    // SAFETY: The registry proves this exact pointer and length came from one
    // retained `Box<[u8]>`; removal makes this its only release.
    unsafe { drop(Box::from_raw(slice)) };
    if let Some(token) = dynamic_token {
        // SAFETY: The Host table owns this retained context.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        return context.release_dynamic(token);
    }
    if let Some(token) = callable_token {
        // SAFETY: The Host table owns this retained context.
        let context = unsafe { &*context.cast::<EngineCallContext>() };
        return context.release_callable(token);
    }
    AbiStatus::Ok
}

/// Executes one generated Godot method through the official ptrcall ABI.
pub(crate) unsafe extern "C" fn call_godot_method_from_module(
    context: *mut c_void,
    receiver: u64,
    method: *const AbiGodotMethodSpecV1,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: The ABI callback validates every reachable prefix and
        // performs a synchronous deep copy before retaining metadata.
        unsafe { call_godot_method(context, receiver, method, arguments, argument_count, output) }
    }));
    match outcome {
        Ok(Ok(())) => AbiCallResult::OK,
        Ok(Err(error)) => error.into_abi(),
        Err(_) => AbiCallResult::failure(
            AbiStatus::Panic,
            "godot-rust caught a panic while calling a Godot method",
        ),
    }
}

unsafe fn call_godot_method(
    context: *mut c_void,
    receiver: u64,
    method: *const AbiGodotMethodSpecV1,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
) -> Result<(), EngineCallError> {
    if output.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot method output pointer is null",
        ));
    }
    if argument_count != 0 && arguments.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot method arguments pointer is null",
        ));
    }
    let Some(interface) = crate::script_instance::active_engine_interface() else {
        return Err(EngineCallError::new(
            AbiStatus::Unsupported,
            "Godot methods can only be called during a script callback or cooperative task",
        ));
    };
    if context.is_null() {
        return Err(EngineCallError::internal(
            "Host engine-call context is unavailable",
        ));
    }
    // SAFETY: The Host supplies this retained context for the complete project
    // module generation.
    let engine_context = unsafe { &*context.cast::<EngineCallContext>() };
    // SAFETY: The project module retains the generated metadata for this
    // synchronous callback. The contract is deep-copied before caching.
    let contract = unsafe { MethodContract::copy_from_abi(method) }?;
    let argument_count =
        validate_argument_count(argument_count, contract.arguments.len(), contract.is_vararg)?;
    let resolved = resolve_method(interface, contract)?;
    let _profile_scope = crate::profiler::ProfileScope::enter_native(
        &resolved.contract.class_name,
        &resolved.contract.method_name,
    );
    let builtin_object_tag = resolved
        .contract
        .arguments
        .iter()
        .any(|value| {
            matches!(
                value.ptrcall_type,
                godot_api::abi::AbiPtrcallType::CALLABLE | godot_api::abi::AbiPtrcallType::SIGNAL
            )
        })
        .then(|| resolve_class_tag(interface, "Object"))
        .transpose()?;
    let receiver = if resolved.contract.is_static {
        if receiver != 0 {
            return Err(EngineCallError::new(
                AbiStatus::InvalidArgument,
                "class-level Godot method received an instance receiver",
            ));
        }
        ptr::null_mut()
    } else {
        resolve_object(
            interface,
            receiver,
            resolved.receiver_tag,
            "Godot method receiver no longer exists",
            "Godot method receiver has the wrong class",
        )?
    };
    let arguments = if argument_count == 0 {
        &[]
    } else {
        // SAFETY: Null was rejected and the bounded count exactly matches the
        // validated generated argument contract or its bounded vararg suffix.
        unsafe { core::slice::from_raw_parts(arguments, argument_count) }
    };
    if resolved.contract.is_vararg {
        let output_value = vararg::call(
            interface,
            engine_context,
            &resolved,
            receiver,
            arguments,
            builtin_object_tag.unwrap_or_default(),
        )?;
        // SAFETY: Null was rejected and the SDK retains this output slot for
        // the complete synchronous call.
        unsafe { output.write(output_value) };
        return Ok(());
    }
    let mut native_arguments = Vec::with_capacity(arguments.len());
    for ((value, contract), class_tag) in arguments
        .iter()
        .copied()
        .zip(&resolved.contract.arguments)
        .zip(&resolved.argument_tags)
    {
        native_arguments.push(decode_native_argument(
            interface,
            engine_context,
            contract,
            value,
            *class_tag,
            builtin_object_tag.unwrap_or_default(),
        )?);
    }
    let argument_pointers = native_arguments
        .iter()
        .map(NativeValue::as_const_ptr)
        .collect::<Vec<GDExtensionConstTypePtr>>();
    let mut native_output = NativeValue::empty_output(interface, &resolved.contract.return_value)?;
    let Some(ptrcall) = interface.object_method_bind_ptrcall else {
        return Err(EngineCallError::internal(
            "Godot ptrcall interface is unavailable",
        ));
    };
    // SAFETY: MethodBind and class tags were resolved from the same official
    // ClassDB. Every argument and output pointer targets the exact native
    // storage selected by the authenticated API metadata.
    unsafe {
        ptrcall(
            resolved.method_bind as GDExtensionMethodBindPtr,
            receiver,
            argument_pointers.as_ptr(),
            native_output.as_mut_ptr(),
        );
    }
    let output_value = native_output.into_abi(
        &resolved.contract.return_value,
        NativeValueOutput {
            object_id: |object| encode_returned_object(interface, object, resolved.return_tag),
            own_object_ref: |object, owned_ref| {
                retain_returned_object(context, interface, object, owned_ref, resolved.return_tag)
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
    )?;
    // SAFETY: Null was rejected and the SDK retains the output slot for this
    // synchronous callback.
    unsafe { output.write(output_value) };
    Ok(())
}

fn validate_argument_count(
    argument_count: u32,
    fixed_count: usize,
    is_vararg: bool,
) -> Result<usize, EngineCallError> {
    let argument_count = usize::try_from(argument_count).map_err(|_| {
        EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot method argument count exceeds this platform",
        )
    })?;
    let matches_contract = if is_vararg {
        argument_count >= fixed_count
    } else {
        argument_count == fixed_count
    };
    if argument_count > MAX_METHOD_CALL_ARGUMENTS || !matches_contract {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            "Godot method argument count does not match its generated contract",
        ));
    }
    Ok(argument_count)
}

fn decode_native_argument(
    interface: EngineInterface,
    engine_context: &EngineCallContext,
    contract: &ValueContract,
    value: AbiValueV1,
    class_tag: usize,
    builtin_object_tag: usize,
) -> Result<NativeValue, ValueError> {
    NativeValue::from_abi(
        contract,
        value,
        NativeValueInput {
            resolve_object: |object_id| resolve_nullable_object(interface, object_id, class_tag),
            create_string: |text: &str| {
                crate::value::LocalGodotString::new_utf8(interface, text).ok_or_else(|| {
                    ValueError::new(
                        AbiStatus::InvalidArgument,
                        "Godot String argument could not be encoded",
                    )
                })
            },
            create_string_name: |text: &str| {
                OwnedStringName::new(interface, text).ok_or_else(|| {
                    ValueError::new(
                        AbiStatus::InvalidArgument,
                        "Godot StringName argument could not be encoded",
                    )
                })
            },
            create_node_path: |text: &str| {
                OwnedNodePath::new(interface, text).ok_or_else(|| {
                    ValueError::new(
                        AbiStatus::InvalidArgument,
                        "Godot NodePath argument could not be encoded",
                    )
                })
            },
            create_packed: |value| OwnedPackedArray::from_abi(interface, value),
            create_dynamic: |value| {
                NativeDynamic::from_abi(
                    interface,
                    contract.value_type,
                    value,
                    contract.typed_array_element.as_deref(),
                    Some(engine_context),
                )
            },
            create_callable: |value| {
                NativeCallable::from_abi(interface, value, Some(engine_context), |object_id| {
                    resolve_nullable_object(interface, object_id, builtin_object_tag)
                })
            },
            create_signal: |value| {
                NativeSignal::from_abi(interface, value, |object_id| {
                    resolve_nullable_object(interface, object_id, builtin_object_tag)
                })
            },
        },
    )
}

fn retain_returned_object(
    context: *mut c_void,
    interface: EngineInterface,
    object: GDExtensionObjectPtr,
    owned_ref: NativeGodotRef,
    class_tag: usize,
) -> Result<AbiValueV1, ValueError> {
    let object_id = encode_returned_object(interface, object, class_tag)?;
    if object_id == 0 {
        return Ok(AbiValueV1::from_object_id(0));
    }
    if context.is_null() {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Host RefCounted return context is unavailable",
        ));
    }
    // SAFETY: The Host table retains this EngineCallContext for the complete
    // project-module generation and supplies its exact pointer to callbacks.
    let context = unsafe { &*context.cast::<EngineCallContext>() };
    let token = context.retain(owned_ref)?;
    Ok(AbiValueV1 {
        type_: AbiValueType::OBJECT_ID,
        reserved_flags: ABI_VALUE_OWNED_OBJECT_REF,
        payload: [object_id, token],
    })
}

fn resolve_method(
    interface: EngineInterface,
    contract: MethodContract,
) -> Result<Arc<ResolvedMethod>, EngineCallError> {
    let cache = METHOD_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let cache = cache
            .lock()
            .map_err(|_| EngineCallError::internal("Godot method cache is poisoned"))?;
        if let Some(cached) = cache.get(&contract.id) {
            if cached.contract != contract {
                return Err(EngineCallError::new(
                    AbiStatus::InvalidArgument,
                    "Godot method contract ID collides with different metadata",
                ));
            }
            return Ok(Arc::clone(cached));
        }
        if cache.len() >= MAX_CACHED_METHODS {
            return Err(EngineCallError::new(
                AbiStatus::Unsupported,
                "Godot method cache limit has been reached",
            ));
        }
    }

    let resolved = Arc::new(resolve_uncached(interface, contract)?);
    let mut cache = cache
        .lock()
        .map_err(|_| EngineCallError::internal("Godot method cache is poisoned"))?;
    if let Some(cached) = cache.get(&resolved.contract.id) {
        if cached.contract != resolved.contract {
            return Err(EngineCallError::new(
                AbiStatus::InvalidArgument,
                "Godot method contract ID collides with different metadata",
            ));
        }
        return Ok(Arc::clone(cached));
    }
    cache.insert(resolved.contract.id, Arc::clone(&resolved));
    Ok(resolved)
}

fn resolve_uncached(
    interface: EngineInterface,
    contract: MethodContract,
) -> Result<ResolvedMethod, EngineCallError> {
    let receiver_tag = if contract.is_static {
        0
    } else {
        resolve_class_tag(interface, &contract.class_name)?
    };
    let class_name = OwnedStringName::new(interface, &contract.class_name)
        .ok_or_else(|| EngineCallError::internal("could not create Godot class name"))?;
    let method_name = OwnedStringName::new(interface, &contract.method_name)
        .ok_or_else(|| EngineCallError::internal("could not create Godot method name"))?;
    let Some(get_method_bind) = interface.classdb_get_method_bind else {
        return Err(EngineCallError::internal(
            "Godot MethodBind lookup is unavailable",
        ));
    };
    // SAFETY: Both StringNames are initialized and the hash was range-checked
    // while copying the generated contract.
    let method_bind = unsafe {
        get_method_bind(
            class_name.as_ptr(),
            method_name.as_ptr(),
            contract.method_hash,
        )
    };
    if method_bind.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::Unsupported,
            "generated Godot method is unavailable in this engine",
        ));
    }
    let argument_tags = contract
        .arguments
        .iter()
        .map(|value| resolve_value_tag(interface, value))
        .collect::<Result<Vec<_>, _>>()?;
    let return_tag = resolve_value_tag(interface, &contract.return_value)?;
    Ok(ResolvedMethod {
        contract,
        method_bind: method_bind as usize,
        receiver_tag,
        argument_tags,
        return_tag,
    })
}

fn resolve_value_tag(
    interface: EngineInterface,
    contract: &ValueContract,
) -> Result<usize, EngineCallError> {
    contract
        .class_name
        .as_deref()
        .map_or(Ok(0), |class_name| resolve_class_tag(interface, class_name))
}

fn resolve_class_tag(
    interface: EngineInterface,
    class_name: &str,
) -> Result<usize, EngineCallError> {
    let name = OwnedStringName::new(interface, class_name)
        .ok_or_else(|| EngineCallError::internal("could not create Godot class name"))?;
    let Some(get_class_tag) = interface.classdb_get_class_tag else {
        return Err(EngineCallError::internal(
            "Godot class tag lookup is unavailable",
        ));
    };
    // SAFETY: `name` is an initialized StringName for this synchronous lookup.
    let tag = unsafe { get_class_tag(name.as_ptr()) };
    if tag.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::Unsupported,
            "generated Godot class is unavailable in this engine",
        ));
    }
    Ok(tag as usize)
}

fn resolve_nullable_object(
    interface: EngineInterface,
    object_id: u64,
    class_tag: usize,
) -> Result<GDExtensionObjectPtr, ValueError> {
    if object_id == 0 {
        return Ok(ptr::null_mut());
    }
    resolve_object(
        interface,
        object_id,
        class_tag,
        "Godot object argument no longer exists",
        "Godot object argument has the wrong class",
    )
    .map_err(|error| ValueError::new(error.status, error.message))
}

fn resolve_object(
    interface: EngineInterface,
    object_id: u64,
    class_tag: usize,
    stale_message: &'static str,
    class_message: &'static str,
) -> Result<GDExtensionObjectPtr, EngineCallError> {
    if object_id == 0 || class_tag == 0 {
        return Err(EngineCallError::new(AbiStatus::StaleHandle, stale_message));
    }
    let Some(get_instance) = interface.object_get_instance_from_id else {
        return Err(EngineCallError::internal(
            "Godot object lookup is unavailable",
        ));
    };
    // SAFETY: Object IDs are opaque integers accepted by the official API.
    let object = unsafe { get_instance(object_id) };
    if object.is_null() {
        return Err(EngineCallError::new(AbiStatus::StaleHandle, stale_message));
    }
    let Some(cast_to) = interface.object_cast_to else {
        return Err(EngineCallError::internal(
            "Godot object class validation is unavailable",
        ));
    };
    // SAFETY: The object came from ObjectDB and the tag came from ClassDB.
    let cast = unsafe { cast_to(object, class_tag as *mut c_void) };
    if cast.is_null() {
        return Err(EngineCallError::new(
            AbiStatus::InvalidArgument,
            class_message,
        ));
    }
    Ok(cast)
}

fn encode_returned_object(
    interface: EngineInterface,
    object: GDExtensionObjectPtr,
    class_tag: usize,
) -> Result<u64, ValueError> {
    if object.is_null() {
        return Ok(0);
    }
    let Some(cast_to) = interface.object_cast_to else {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot object class validation is unavailable",
        ));
    };
    // SAFETY: ptrcall returned this Object pointer and the expected tag came
    // from the generated return contract.
    if unsafe { cast_to(object, class_tag as *mut c_void) }.is_null() {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot method returned an object with the wrong class",
        ));
    }
    let Some(get_instance_id) = interface.object_get_instance_id else {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot object identity is unavailable",
        ));
    };
    // SAFETY: The non-null Object pointer was returned by this ptrcall.
    let object_id = unsafe { get_instance_id(object) };
    if object_id == 0 {
        return Err(ValueError::new(
            AbiStatus::Internal,
            "Godot method returned an object without an instance ID",
        ));
    }
    Ok(object_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static OWNED_REF_RELEASES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn mock_ref_get_object(
        _value: godot_api::GDExtensionConstRefPtr,
    ) -> GDExtensionObjectPtr {
        ptr::null_mut()
    }

    unsafe extern "C" fn mock_ref_set_object(
        _value: godot_api::GDExtensionRefPtr,
        object: GDExtensionObjectPtr,
    ) {
        if object.is_null() {
            OWNED_REF_RELEASES.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn method_calls_fail_closed_outside_a_script_callback() {
        let mut output = AbiValueV1::NIL;
        // SAFETY: The callback rejects the absent script scope before reading
        // the intentionally null method contract.
        let result = unsafe {
            call_godot_method_from_module(
                ptr::null_mut(),
                1,
                ptr::null(),
                ptr::null(),
                0,
                &mut output,
            )
        };
        assert_eq!(result.status, AbiStatus::Unsupported);
        assert_eq!(output, AbiValueV1::NIL);
    }

    #[test]
    fn method_calls_validate_output_and_argument_pointers_first() {
        // SAFETY: These inputs intentionally exercise pointer validation
        // without dereferencing any project or Godot memory.
        let missing_output = unsafe {
            call_godot_method_from_module(
                ptr::null_mut(),
                1,
                ptr::null(),
                ptr::null(),
                0,
                ptr::null_mut(),
            )
        };
        assert_eq!(missing_output.status, AbiStatus::InvalidArgument);

        let mut output = AbiValueV1::NIL;
        // SAFETY: The non-zero count intentionally requires a non-null input.
        let missing_arguments = unsafe {
            call_godot_method_from_module(
                ptr::null_mut(),
                1,
                ptr::null(),
                ptr::null(),
                1,
                &mut output,
            )
        };
        assert_eq!(missing_arguments.status, AbiStatus::InvalidArgument);
    }

    #[test]
    fn method_argument_counts_are_exact_or_bounded_varargs() {
        assert_eq!(validate_argument_count(2, 2, false).ok(), Some(2));
        assert!(validate_argument_count(1, 2, false).is_err());
        assert!(validate_argument_count(3, 2, false).is_err());

        assert_eq!(validate_argument_count(2, 2, true).ok(), Some(2));
        assert_eq!(validate_argument_count(3, 2, true).ok(), Some(3));
        assert!(validate_argument_count(1, 2, true).is_err());
        assert!(
            validate_argument_count(
                u32::try_from(MAX_METHOD_CALL_ARGUMENTS + 1).expect("test count"),
                0,
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn host_owned_text_rejects_forged_or_duplicate_releases() {
        let value = own_returned_utf8(AbiValueType::STRING, "你好，Godot".to_owned())
            .unwrap_or_else(|_| panic!("owned result"));
        let pointer = usize::try_from(value.payload[0]).expect("pointer");
        let length = usize::try_from(value.payload[1]).expect("length");
        // SAFETY: The registry retains this exact allocation until release.
        let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
        assert_eq!(core::str::from_utf8(bytes), Ok("你好，Godot"));

        let mut forged = value;
        forged.payload[1] += 1;
        assert_eq!(
            drop_host_value(ptr::null_mut(), forged),
            AbiStatus::InvalidArgument
        );
        assert_eq!(drop_host_value(ptr::null_mut(), value), AbiStatus::Ok);
        assert_eq!(
            drop_host_value(ptr::null_mut(), value),
            AbiStatus::InvalidArgument
        );

        let string_name = own_returned_utf8(AbiValueType::STRING_NAME, "玩家".to_owned())
            .unwrap_or_else(|_| panic!("owned StringName result"));
        assert_eq!(string_name.type_, AbiValueType::STRING_NAME);
        assert_eq!(drop_host_value(ptr::null_mut(), string_name), AbiStatus::Ok);

        let node_path = own_returned_utf8(AbiValueType::NODE_PATH, "../玩家/%武器".to_owned())
            .unwrap_or_else(|_| panic!("owned NodePath result"));
        assert_eq!(node_path.type_, AbiValueType::NODE_PATH);
        assert_eq!(drop_host_value(ptr::null_mut(), node_path), AbiStatus::Ok);
    }

    #[test]
    fn host_owned_math_rejects_forged_or_duplicate_releases() {
        let components = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0];
        let value = own_returned_math(AbiValueType::TRANSFORM3D, &components)
            .unwrap_or_else(|_| panic!("owned Transform3D result"));
        assert_eq!(value.reserved_flags, ABI_VALUE_OWNED_BYTES);
        let (pointer, length) = value
            .byte_range(AbiValueType::TRANSFORM3D)
            .expect("owned component bytes");
        assert_eq!(length, 12 * core::mem::size_of::<f32>());
        // SAFETY: The Host registry retains this exact allocation until its
        // successful release below.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let returned = bytes
            .chunks_exact(core::mem::size_of::<f32>())
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().expect("f32 byte width")))
            .collect::<Vec<_>>();
        assert_eq!(returned, components);

        let mut forged = value;
        forged.payload[1] -= 1;
        assert_eq!(
            drop_host_value(ptr::null_mut(), forged),
            AbiStatus::InvalidArgument
        );
        assert_eq!(drop_host_value(ptr::null_mut(), value), AbiStatus::Ok);
        assert_eq!(
            drop_host_value(ptr::null_mut(), value),
            AbiStatus::InvalidArgument
        );
    }

    #[test]
    fn host_owned_refs_reject_forged_or_duplicate_releases() {
        OWNED_REF_RELEASES.store(0, Ordering::SeqCst);
        let context = EngineCallContext::new();
        let token = context
            .retain(NativeGodotRef::from_functions(
                mock_ref_get_object,
                mock_ref_set_object,
            ))
            .expect("retained Ref token");
        let value = AbiValueV1 {
            type_: AbiValueType::OBJECT_ID,
            reserved_flags: ABI_VALUE_OWNED_OBJECT_REF,
            payload: [42, token],
        };
        let context_pointer = core::ptr::from_ref(&context).cast_mut().cast::<c_void>();

        let mut forged = value;
        forged.payload[1] += 1;
        assert_eq!(
            drop_host_value(context_pointer, forged),
            AbiStatus::InvalidArgument
        );
        assert_eq!(drop_host_value(context_pointer, value), AbiStatus::Ok);
        assert_eq!(OWNED_REF_RELEASES.load(Ordering::SeqCst), 1);
        assert_eq!(
            drop_host_value(context_pointer, value),
            AbiStatus::InvalidArgument
        );
    }
}
