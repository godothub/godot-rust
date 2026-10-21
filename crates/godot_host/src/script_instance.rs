use core::ffi::c_void;
mod connections;
mod reload;
mod typed;
use core::mem::size_of;
use core::ptr;
use godot_api::abi::{
    ABI_PROPERTY_USAGE_GROUP, ABI_PROPERTY_USAGE_STORAGE, AbiCallResult, AbiMethodKind, AbiStatus,
    AbiValueType, AbiValueV1,
};
use godot_api::{
    GDExtensionBool, GDExtensionCallError, GDExtensionCallErrorType, GDExtensionConstStringNamePtr,
    GDExtensionConstTypePtr, GDExtensionConstVariantPtr, GDExtensionInt, GDExtensionMethodBindPtr,
    GDExtensionMethodInfo, GDExtensionObjectPtr, GDExtensionPropertyInfo,
    GDExtensionScriptInstanceDataPtr, GDExtensionScriptInstanceInfo3,
    GDExtensionScriptInstancePropertyStateAdd, GDExtensionScriptInstancePtr,
    GDExtensionScriptLanguagePtr, GDExtensionStringPtr, GDExtensionTypePtr, GDExtensionVariantPtr,
    GDExtensionVariantType,
};
use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::CString;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
pub(crate) use typed::{call_script_member_from_module, validate_object_from_module};

use crate::godot_metadata::{
    PROPERTY_USAGE_DEFAULT, method_flags, property_variant_type, variant_type,
};
use crate::interface::EngineInterface;
use crate::module_loader::{
    ModuleCallError, ModuleField, ModuleGeneration, ModuleMethod, ModuleState,
};
use crate::module_value::ModuleValueOwner;
use crate::string_name::{OwnedStringName, StaticStringName};
use crate::value::LocalGodotString;
use crate::variant_codec::{OwnedVariant, VariantCodec, VariantDecodeBacking, VariantTypeMismatch};

const MAX_SIGNAL_ARGUMENTS: usize = 8;
const OBJECT_EMIT_SIGNAL_HASH: i64 = 4_047_867_050;
const OS_HAS_FEATURE_HASH: i64 = 3_927_539_163;
const SINGLE_PRECISION_VARIANT_SIZE: usize = 24;
const DOUBLE_PRECISION_VARIANT_SIZE: usize = 40;
const INSTANCE_FAILURE_LIMIT: u8 = 3;
const INSTANCE_FUSED: u8 = 1 << 7;
const INSTANCE_FAILURE_COUNT: u8 = !INSTANCE_FUSED;

static LIVE_INSTANCES: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();

thread_local! {
    static ACTIVE_SCRIPT_INSTANCE: Cell<*const RustScriptInstance> =
        const { Cell::new(ptr::null()) };
    static ACTIVE_ENGINE_INTERFACE: Cell<Option<EngineInterface>> =
        const { Cell::new(None) };
}

#[derive(Default)]
struct InstanceIdentity {
    address: AtomicUsize,
    godot_instance: AtomicUsize,
}

// Godot retains this wrapper across reloads. Each callback owns a snapshot of
// one complete generation, including every pointer returned in its metadata.
struct InstanceSlot {
    current: Mutex<Arc<RustScriptInstance>>,
    property_lists: Mutex<Vec<Arc<RustScriptInstance>>>,
    method_lists: Mutex<Vec<Arc<RustScriptInstance>>>,
}

struct RustScriptInstance {
    interface: Option<EngineInterface>,
    codec: Option<VariantCodec>,
    owner: usize,
    owner_id: u64,
    script: usize,
    language: usize,
    source_path: String,
    script_uids: Vec<i64>,
    identity: Arc<InstanceIdentity>,
    faults: InstanceFaults,
    names: LifecycleMethodNames,
    available: LifecycleAvailability,
    properties: Vec<ReflectedProperty>,
    property_infos: Vec<GDExtensionPropertyInfo>,
    methods: Vec<ReflectedMethod>,
    method_infos: Vec<GDExtensionMethodInfo>,
    signals: Vec<ReflectedSignal>,
    emit_signal_method: usize,
    _empty_metadata_name: Option<Box<OwnedStringName>>,
    _empty_metadata_hint: Option<Box<LocalGodotString>>,
    state: Option<Mutex<ModuleState>>,
    // ScriptInstance owns its Script resource; Godot's Object only retains
    // the ScriptInstance. Keep metadata alive until callback state is dropped.
    _script_ref: Option<crate::engine_call::value::NativeGodotRef>,
}

struct ReflectedMethod {
    name: OwnedStringName,
    method: ModuleMethod,
    _argument_names: Vec<OwnedStringName>,
    _argument_type_names: Vec<OwnedStringName>,
    _argument_type_hints: Vec<LocalGodotString>,
    return_type_name: OwnedStringName,
    return_type_hint: LocalGodotString,
    argument_infos: Vec<GDExtensionPropertyInfo>,
    default_arguments: ContiguousVariants,
}

/// Owns the exact contiguous `Variant` layout consumed by Godot's
/// `MethodInfo(GDExtensionMethodInfo)` adapter.
///
/// Despite the public C field being declared as an array of Variant pointers,
/// Godot 4.4-4.7 casts the field itself to `Variant *` and indexes contiguous
/// values. Keep this ScriptInstance-only compatibility storage separate from
/// normal GDExtension calls, which correctly use pointer arrays.
struct ContiguousVariants {
    interface: EngineInterface,
    storage: Vec<u64>,
    stride: usize,
    initialized: usize,
}

impl ContiguousVariants {
    fn empty(interface: EngineInterface) -> Self {
        Self {
            interface,
            storage: Vec::new(),
            stride: SINGLE_PRECISION_VARIANT_SIZE,
            initialized: 0,
        }
    }

    fn with_capacity(interface: EngineInterface, stride: usize, capacity: usize) -> Option<Self> {
        if !matches!(
            stride,
            SINGLE_PRECISION_VARIANT_SIZE | DOUBLE_PRECISION_VARIANT_SIZE
        ) {
            return None;
        }
        let words = stride
            .checked_mul(capacity)?
            .checked_div(size_of::<u64>())?;
        Some(Self {
            interface,
            storage: vec![0; words],
            stride,
            initialized: 0,
        })
    }

    fn push(
        &mut self,
        codec: &VariantCodec,
        value: AbiValueV1,
        typed_array_element: Option<&str>,
        context: Option<&crate::engine_call::EngineCallContext>,
    ) -> Result<(), ()> {
        if self.initialized >= self.capacity() {
            return Err(());
        }
        let output = self.variant_ptr(self.initialized);
        codec.construct_with_context(value, output, typed_array_element, context)?;
        self.initialized += 1;
        Ok(())
    }

    fn capacity(&self) -> usize {
        self.storage
            .len()
            .saturating_mul(size_of::<u64>())
            .checked_div(self.stride)
            .unwrap_or_default()
    }

    fn len(&self) -> usize {
        self.initialized
    }

    fn as_ffi_ptr(&mut self) -> *mut GDExtensionVariantPtr {
        self.storage.as_mut_ptr().cast()
    }

    fn get(&self, index: usize) -> Option<GDExtensionConstVariantPtr> {
        if index >= self.initialized {
            return None;
        }
        // SAFETY: Every index below `initialized` refers to a live Variant in
        // the fixed-size buffer, and immutable access cannot move it.
        Some(unsafe {
            self.storage
                .as_ptr()
                .cast::<u8>()
                .add(index * self.stride)
                .cast()
        })
    }

    fn variant_ptr(&mut self, index: usize) -> GDExtensionVariantPtr {
        // SAFETY: Storage is allocated for `capacity * stride` bytes, both
        // supported strides preserve Variant's eight-byte alignment, and
        // callers validate the index against capacity.
        unsafe {
            self.storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(index * self.stride)
                .cast()
        }
    }
}

impl Drop for ContiguousVariants {
    fn drop(&mut self) {
        let Some(destroy) = self.interface.variant_destroy else {
            return;
        };
        for index in 0..self.initialized {
            let variant = self.variant_ptr(index);
            // SAFETY: Each slot below `initialized` was successfully
            // constructed exactly once and remains owned by this buffer.
            unsafe { destroy(variant) };
        }
    }
}

struct ReflectedProperty {
    name: OwnedStringName,
    hint_string: LocalGodotString,
    field: Option<ModuleField>,
    type_: GDExtensionVariantType,
    hint: u32,
    usage: u32,
}

struct ReflectedSignal {
    script_resource_uid: i64,
    field_index: u32,
    name: OwnedStringName,
    argument_types: Vec<AbiValueType>,
}

struct ActiveScriptScope {
    previous: *const RustScriptInstance,
}

struct ActiveEngineInterfaceScope {
    previous: Option<EngineInterface>,
}

#[derive(Default)]
struct InstanceFaults {
    state: AtomicU8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultDisposition {
    Report,
    Suppress,
    Fused,
}

impl InstanceFaults {
    fn is_fused(&self) -> bool {
        self.state.load(Ordering::Acquire) & INSTANCE_FUSED != 0
    }

    fn record_success(&self) {
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current & INSTANCE_FUSED == 0).then_some(0)
            });
    }

    fn record_failure(&self, status: AbiStatus) -> FaultDisposition {
        loop {
            let current = self.state.load(Ordering::Acquire);
            if current & INSTANCE_FUSED != 0 {
                return FaultDisposition::Suppress;
            }
            let failures = current & INSTANCE_FAILURE_COUNT;
            let next_failures = failures.saturating_add(1);
            let fuse = status == AbiStatus::Panic || next_failures >= INSTANCE_FAILURE_LIMIT;
            let next = if fuse {
                INSTANCE_FUSED | next_failures.min(INSTANCE_FAILURE_COUNT)
            } else {
                next_failures
            };
            if self
                .state
                .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return if fuse {
                    FaultDisposition::Fused
                } else if failures == 0 {
                    FaultDisposition::Report
                } else {
                    FaultDisposition::Suppress
                };
            }
        }
    }
}

impl ActiveScriptScope {
    fn enter(instance: &RustScriptInstance) -> Self {
        let previous =
            ACTIVE_SCRIPT_INSTANCE.with(|active| active.replace(ptr::from_ref(instance)));
        Self { previous }
    }
}

impl Drop for ActiveScriptScope {
    fn drop(&mut self) {
        ACTIVE_SCRIPT_INSTANCE.with(|active| active.set(self.previous));
    }
}

impl Drop for ActiveEngineInterfaceScope {
    fn drop(&mut self) {
        ACTIVE_ENGINE_INTERFACE.with(|active| active.set(self.previous));
    }
}

struct LifecycleMethodNames {
    enter_tree: Option<StaticStringName>,
    ready: Option<StaticStringName>,
    process: Option<StaticStringName>,
    physics_process: Option<StaticStringName>,
    input: Option<StaticStringName>,
    unhandled_input: Option<StaticStringName>,
    exit_tree: Option<StaticStringName>,
}

struct LifecycleAvailability {
    enter_tree: bool,
    ready: bool,
    process: bool,
    physics_process: bool,
    input: bool,
    unhandled_input: bool,
    exit_tree: bool,
}

fn property_info(
    type_: AbiValueType,
    name: &OwnedStringName,
    type_name: &OwnedStringName,
    type_hint: &LocalGodotString,
    has_type_metadata: bool,
    empty_name: &OwnedStringName,
    empty_hint: &LocalGodotString,
) -> GDExtensionPropertyInfo {
    let typed_array = type_ == AbiValueType::ARRAY && has_type_metadata;
    GDExtensionPropertyInfo {
        type_: variant_type(type_),
        name: name.as_ptr().cast_mut(),
        class_name: if typed_array {
            empty_name.as_ptr().cast_mut()
        } else {
            type_name.as_ptr().cast_mut()
        },
        hint: u32::from(typed_array) * 31,
        hint_string: if typed_array {
            type_hint.as_ptr().cast_mut()
        } else {
            empty_hint.as_ptr().cast_mut()
        },
        usage: PROPERTY_USAGE_DEFAULT
            | if type_ == AbiValueType::VARIANT {
                1 << 17
            } else {
                0
            },
    }
}

fn variant_storage_stride(interface: EngineInterface) -> Option<usize> {
    let get_singleton = interface.global_get_singleton?;
    let os_name = StaticStringName::new(interface, c"OS");
    // SAFETY: OS is an official Godot singleton and the StringName remains
    // initialized for this lookup.
    let os = unsafe { get_singleton(os_name.as_ptr()) };
    if os.is_null() {
        return None;
    }
    let has_feature =
        crate::runtime::resolve_method(interface, c"OS", c"has_feature", OS_HAS_FEATURE_HASH)
            .ok()?;
    let feature = LocalGodotString::new(interface, c"double")?;
    let arguments: [GDExtensionConstTypePtr; 1] = [feature.as_ptr()];
    let ptrcall = interface.object_method_bind_ptrcall?;
    let mut uses_double_precision: GDExtensionBool = 0;
    // SAFETY: The resolved method is OS.has_feature(String) -> bool and every
    // pointer refers to live storage of the matching engine type.
    unsafe {
        ptrcall(
            has_feature,
            os,
            arguments.as_ptr(),
            ptr::from_mut(&mut uses_double_precision).cast(),
        );
    }
    Some(if uses_double_precision != 0 {
        DOUBLE_PRECISION_VARIANT_SIZE
    } else {
        SINGLE_PRECISION_VARIANT_SIZE
    })
}

/// Creates the Godot-owned wrapper around one Rust script instance.
///
/// The callback table is static because Godot stores its address for the whole
/// `ScriptInstanceExtension` lifetime instead of copying it.
pub(crate) fn create(
    interface: EngineInterface,
    script: GDExtensionObjectPtr,
    owner: GDExtensionObjectPtr,
    language: GDExtensionScriptLanguagePtr,
    state: ModuleState,
) -> GDExtensionScriptInstancePtr {
    if script.is_null() || owner.is_null() || language.is_null() {
        return ptr::null_mut();
    }
    let Some(create) = interface.script_instance_create3 else {
        return ptr::null_mut();
    };
    let Some(instance) = prepare_instance(
        interface,
        script,
        owner,
        language,
        state,
        Arc::new(InstanceIdentity::default()),
    ) else {
        return ptr::null_mut();
    };
    let data = into_instance_data(instance);
    // SAFETY: `INSTANCE_INFO` has static storage, `data` owns a strong reference,
    // and Godot will release it exactly once through `free_func`.
    let instance = unsafe { create(&INSTANCE_INFO, data) };
    if instance.is_null() {
        // SAFETY: Godot did not accept ownership when creation failed.
        unsafe { drop(Arc::from_raw(data.cast::<InstanceSlot>())) };
    } else if let Ok(mut instances) = LIVE_INSTANCES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    {
        // The new Godot ScriptInstance owns this stable wrapper.
        if let Some(payload) = self::data(data) {
            payload
                .identity
                .godot_instance
                .store(instance as usize, Ordering::Release);
        }
        instances.insert(data as usize);
    }
    instance
}

fn prepare_instance(
    interface: EngineInterface,
    script: GDExtensionObjectPtr,
    owner: GDExtensionObjectPtr,
    language: GDExtensionScriptLanguagePtr,
    state: ModuleState,
    identity: Arc<InstanceIdentity>,
) -> Option<RustScriptInstance> {
    let codec = VariantCodec::new(interface)?;
    let get_instance_id = interface.object_get_instance_id?;
    // SAFETY: Godot passes the live owner while creating its script instance.
    let owner_id = unsafe { get_instance_id(owner) };
    let script_ref =
        crate::engine_call::value::NativeGodotRef::from_object(interface, script).ok()?;
    let source_path = state.source_path().to_owned();
    let emit_signal_method = crate::runtime::resolve_method(
        interface,
        c"Object",
        c"emit_signal",
        OBJECT_EMIT_SIGNAL_HASH,
    )
    .ok()?;
    let available = LifecycleAvailability {
        enter_tree: state.has_enter_tree(),
        ready: state.has_ready() || state.has_node_fields(),
        process: state.has_process(),
        physics_process: state.has_physics_process(),
        input: state.has_input(),
        unhandled_input: state.has_unhandled_input(),
        exit_tree: state.has_exit_tree(),
    };
    let empty_metadata_name = OwnedStringName::new(interface, "").map(Box::new)?;
    let empty_metadata_hint = LocalGodotString::new(interface, c"").map(Box::new)?;
    let value_owner = state.value_owner();
    let mut signals = Vec::new();
    for index in 0..state.field_count() {
        let field = state.field(index)?;
        if !field.is_signal() {
            continue;
        }
        let name = OwnedStringName::new(interface, field.name())?;
        signals.push(ReflectedSignal {
            script_resource_uid: field.script_resource_uid(),
            field_index: field.index(),
            name,
            argument_types: field.signal_arguments().map(|(_, type_)| type_).collect(),
        });
    }
    let mut properties = Vec::with_capacity(state.field_count().saturating_mul(2));
    let mut current_group: Option<String> = None;
    for index in 0..state.field_count() {
        let field = state.field(index)?;
        let (Some(type_), Some(hint), Some(hint_string), Some(usage)) = (
            field.property_type().and_then(property_variant_type),
            field.property_hint(),
            field.property_hint_string(),
            field.property_usage(),
        ) else {
            continue;
        };
        let next_group = field.property_group();
        if current_group.as_deref() != next_group {
            let name = OwnedStringName::new(interface, next_group.unwrap_or(""))?;
            let hint_string = LocalGodotString::new_utf8(interface, "")?;
            properties.push(ReflectedProperty {
                name,
                hint_string,
                field: None,
                type_: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
                hint: 0,
                usage: ABI_PROPERTY_USAGE_GROUP,
            });
            current_group = next_group.map(str::to_owned);
        }
        let name = OwnedStringName::new(interface, field.name())?;
        let hint_string = LocalGodotString::new_utf8(interface, hint_string)?;
        properties.push(ReflectedProperty {
            name,
            hint_string,
            field: Some(field),
            type_,
            hint,
            usage,
        });
    }
    let property_infos = properties
        .iter_mut()
        .map(|property| GDExtensionPropertyInfo {
            type_: property.type_,
            name: property.name.as_ptr().cast_mut(),
            class_name: empty_metadata_name.as_ptr().cast_mut(),
            hint: property.hint,
            hint_string: property.hint_string.as_mut_ptr(),
            usage: property.usage,
        })
        .collect();
    let mut methods = Vec::with_capacity(state.method_count());
    let mut default_variant_stride = None;
    for index in 0..state.method_count() {
        let method = state.method(index)?;
        if method.kind() == AbiMethodKind::Lifecycle {
            continue;
        }
        let name = OwnedStringName::new(interface, method.name())?;
        let argument_names = method
            .arguments()
            .map(|(name, _)| OwnedStringName::new(interface, name))
            .collect::<Option<Vec<_>>>()?;
        let argument_type_names = (0..method.argument_types().len())
            .map(|index| {
                OwnedStringName::new(
                    interface,
                    method.argument_class_name(index).unwrap_or_default(),
                )
            })
            .collect::<Option<Vec<_>>>()?;
        let argument_type_hints = (0..method.argument_types().len())
            .map(|index| {
                LocalGodotString::new_utf8(
                    interface,
                    method.argument_class_name(index).unwrap_or_default(),
                )
            })
            .collect::<Option<Vec<_>>>()?;
        let return_type_name =
            OwnedStringName::new(interface, method.return_class_name().unwrap_or_default())?;
        let return_type_hint =
            LocalGodotString::new_utf8(interface, method.return_class_name().unwrap_or_default())?;
        let argument_infos = method
            .arguments()
            .zip(&argument_names)
            .zip(&argument_type_names)
            .zip(&argument_type_hints)
            .enumerate()
            .map(|(index, ((((_, type_), name), type_name), type_hint))| {
                property_info(
                    type_,
                    name,
                    type_name,
                    type_hint,
                    method.argument_class_name(index).is_some(),
                    &empty_metadata_name,
                    &empty_metadata_hint,
                )
            })
            .collect();
        let default_start = method.minimum_argument_count();
        let default_values = method.default_values().ok()?;
        let mut default_arguments = if default_values.is_empty() {
            ContiguousVariants::empty(interface)
        } else {
            let stride = match default_variant_stride {
                Some(stride) => stride,
                None => {
                    let stride = variant_storage_stride(interface)?;
                    default_variant_stride = Some(stride);
                    stride
                }
            };
            ContiguousVariants::with_capacity(interface, stride, default_values.len())?
        };
        for (offset, value) in default_values.iter().enumerate() {
            let argument_index = default_start + offset;
            let typed_array_element = (method.argument_types()[argument_index]
                == AbiValueType::ARRAY)
                .then(|| method.argument_class_name(argument_index))
                .flatten();
            if default_arguments
                .push(
                    &codec,
                    value.borrowed_abi(),
                    typed_array_element,
                    Some(value_owner.engine_call_context()),
                )
                .is_err()
            {
                return None;
            }
        }
        methods.push(ReflectedMethod {
            name,
            method,
            _argument_names: argument_names,
            _argument_type_names: argument_type_names,
            _argument_type_hints: argument_type_hints,
            return_type_name,
            return_type_hint,
            argument_infos,
            default_arguments,
        });
    }
    let method_infos = methods
        .iter_mut()
        .map(|method| GDExtensionMethodInfo {
            name: method.name.as_ptr().cast_mut(),
            return_value: property_info(
                method.method.return_type(),
                &empty_metadata_name,
                &method.return_type_name,
                &method.return_type_hint,
                method.method.return_class_name().is_some(),
                &empty_metadata_name,
                &empty_metadata_hint,
            ),
            flags: method_flags(method.method.receiver(), method.method.is_vararg()),
            id: 0,
            argument_count: u32::try_from(method.argument_infos.len()).unwrap_or(u32::MAX),
            arguments: if method.argument_infos.is_empty() {
                ptr::null_mut()
            } else {
                method.argument_infos.as_mut_ptr()
            },
            default_argument_count: u32::try_from(method.default_arguments.len())
                .unwrap_or(u32::MAX),
            default_arguments: if method.default_arguments.len() == 0 {
                ptr::null_mut()
            } else {
                method.default_arguments.as_ffi_ptr()
            },
        })
        .collect();
    Some(RustScriptInstance {
        interface: Some(interface),
        codec: Some(codec),
        owner: owner as usize,
        owner_id,
        script: script as usize,
        language: language as usize,
        source_path,
        script_uids: state.script_uids(),
        identity,
        faults: InstanceFaults::default(),
        names: LifecycleMethodNames {
            enter_tree: Some(StaticStringName::new(interface, c"_enter_tree")),
            ready: Some(StaticStringName::new(interface, c"_ready")),
            process: Some(StaticStringName::new(interface, c"_process")),
            physics_process: Some(StaticStringName::new(interface, c"_physics_process")),
            input: Some(StaticStringName::new(interface, c"_input")),
            unhandled_input: Some(StaticStringName::new(interface, c"_unhandled_input")),
            exit_tree: Some(StaticStringName::new(interface, c"_exit_tree")),
        },
        available,
        properties,
        property_infos,
        methods,
        method_infos,
        signals,
        emit_signal_method: emit_signal_method as usize,
        _empty_metadata_name: Some(empty_metadata_name),
        _empty_metadata_hint: Some(empty_metadata_hint),
        state: Some(Mutex::new(state)),
        _script_ref: Some(script_ref),
    })
}

fn into_instance_data(instance: RustScriptInstance) -> GDExtensionScriptInstanceDataPtr {
    let identity = instance.identity.clone();
    // Arc shares lifetime across reentrant engine callbacks, not thread access
    // to Godot values.
    #[allow(clippy::arc_with_non_send_sync)]
    let slot = Arc::new(InstanceSlot {
        current: Mutex::new(Arc::new(instance)),
        property_lists: Mutex::new(Vec::new()),
        method_lists: Mutex::new(Vec::new()),
    });
    let pointer = Arc::into_raw(slot).cast_mut().cast();
    identity.address.store(pointer as usize, Ordering::Release);
    pointer
}

fn slot(instance: GDExtensionScriptInstanceDataPtr) -> Option<Arc<InstanceSlot>> {
    if instance.is_null() {
        return None;
    }
    let pointer = instance.cast::<InstanceSlot>();
    // SAFETY: Godot owns a strong reference throughout callback entry. Retain
    // the stable wrapper even if user code removes its script during this call.
    Some(unsafe {
        Arc::increment_strong_count(pointer);
        Arc::from_raw(pointer)
    })
}

fn data(instance: GDExtensionScriptInstanceDataPtr) -> Option<Arc<RustScriptInstance>> {
    let slot = slot(instance)?;
    let current = slot.current.lock().ok()?;
    Some(current.clone())
}

pub(crate) fn belongs_to_script(
    instance: GDExtensionScriptInstanceDataPtr,
    script: GDExtensionObjectPtr,
) -> bool {
    data(instance)
        .map(|instance| instance.script == script as usize)
        .unwrap_or(false)
}

#[derive(Debug)]
pub(crate) enum GenerationReloadError {
    Busy,
    Incompatible(String),
    Rejected(String),
}

impl core::fmt::Display for GenerationReloadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Busy => {
                formatter.write_str("a Rust script callback or background task is still active")
            }
            Self::Incompatible(message) | Self::Rejected(message) => formatter.write_str(message),
        }
    }
}

pub(crate) use reload::install_generation;

unsafe extern "C" fn set(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
    value: GDExtensionConstVariantPtr,
) -> GDExtensionBool {
    let Some(instance) = data(instance) else {
        return 0;
    };
    let _scope = ActiveScriptScope::enter(&instance);
    if instance.faults.is_fused() {
        return 0;
    }
    let Some(property) = resolve_property(Some(&instance), name) else {
        return 0;
    };
    let Some(field) = property.field.as_ref() else {
        return 0;
    };
    let Some(expected) = field.value_type() else {
        return 0;
    };
    let Some(codec) = instance.codec.as_ref() else {
        return 0;
    };
    let Some(state) = &instance.state else {
        return 0;
    };
    let mut state = match state.try_lock() {
        Ok(state) => state,
        Err(TryLockError::WouldBlock) => {
            instance.record_failure(
                field.name(),
                AbiStatus::ReentrantCall,
                "synchronous mutable property re-entry was rejected",
            );
            return 0;
        }
        Err(TryLockError::Poisoned(_)) => {
            instance.host_failed(field.name(), "script state lock is poisoned");
            return 0;
        }
    };
    let mut string_backing = Vec::new();
    let mut math_backing = Vec::new();
    let mut packed_backing = Vec::new();
    let mut dynamic_backing = Vec::new();
    let mut callable_backing = Vec::new();
    let value_owner = state.value_owner();
    let dynamic_context = Some(value_owner.engine_call_context());
    let Ok(mut value) = codec.decode(
        value,
        expected,
        VariantDecodeBacking {
            strings: &mut string_backing,
            math: &mut math_backing,
            packed: &mut packed_backing,
            dynamic: &mut dynamic_backing,
            callable: &mut callable_backing,
            dynamic_context,
        },
    ) else {
        return 0;
    };
    if expected == AbiValueType::OBJECT_ID {
        let Some(class_name) = field.property_object_class() else {
            return 0;
        };
        if !codec.object_is_class(value.payload[0], class_name) {
            return 0;
        }
        if field.owns_property_object() {
            let (Some(context), Some(interface)) = (dynamic_context, instance.interface) else {
                return 0;
            };
            let Ok(owned) = context.retain_refcounted_object(interface, value.payload[0]) else {
                return 0;
            };
            value = owned;
        }
    }
    match state.set_field(field, value) {
        Ok(()) => 1,
        Err(error) => {
            instance.callback_failed(field.name(), &error);
            0
        }
    }
}

unsafe extern "C" fn get(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
    result: GDExtensionVariantPtr,
) -> GDExtensionBool {
    let Some(instance) = data(instance) else {
        return 0;
    };
    let _scope = ActiveScriptScope::enter(&instance);
    if instance.faults.is_fused() {
        return 0;
    }
    let Some(property) = resolve_property(Some(&instance), name) else {
        return 0;
    };
    let Some(field) = property.field.as_ref() else {
        return 0;
    };
    let Some(state) = &instance.state else {
        return 0;
    };
    let value = match state.try_lock() {
        Ok(state) => match state.get_field(field) {
            Ok(value) => Some(value),
            Err(error) => {
                instance.callback_failed(field.name(), &error);
                None
            }
        },
        Err(TryLockError::WouldBlock) => {
            instance.record_failure(
                field.name(),
                AbiStatus::ReentrantCall,
                "synchronous property re-entry was rejected",
            );
            None
        }
        Err(TryLockError::Poisoned(_)) => {
            instance.host_failed(field.name(), "script state lock is poisoned");
            None
        }
    };
    let Some((value, codec)) = value.zip(instance.codec.as_ref()) else {
        return 0;
    };
    if codec
        .encode_with_context(
            value.borrowed_abi(),
            result,
            field.typed_array_element(),
            Some(value.engine_call_context()),
        )
        .is_err()
    {
        instance.host_failed(field.name(), "could not encode the exported property value");
        return 0;
    }
    1
}

unsafe extern "C" fn get_property_list(
    instance: GDExtensionScriptInstanceDataPtr,
    count: *mut u32,
) -> *const GDExtensionPropertyInfo {
    if !count.is_null() {
        // SAFETY: Godot supplies a writable count output.
        unsafe { count.write(0) };
    }
    let Some(slot) = slot(instance) else {
        return ptr::null();
    };
    let Ok(current) = slot.current.lock() else {
        return ptr::null();
    };
    if current.property_infos.is_empty() {
        return ptr::null();
    }
    let Ok(mut lists) = slot.property_lists.lock() else {
        return ptr::null();
    };
    if !count.is_null() {
        // SAFETY: The metadata count is bounded when the module is loaded.
        unsafe { count.write(current.property_infos.len() as u32) };
    }
    let pointer = current.property_infos.as_ptr();
    lists.push(current.clone());
    pointer
}

unsafe extern "C" fn free_property_list(
    instance: GDExtensionScriptInstanceDataPtr,
    list: *const GDExtensionPropertyInfo,
    _count: u32,
) {
    let Some(slot) = slot(instance) else { return };
    let released = {
        let Ok(mut lists) = slot.property_lists.lock() else {
            return;
        };
        lists
            .iter()
            .position(|payload| payload.property_infos.as_ptr() == list)
            .map(|index| lists.swap_remove(index))
    };
    // State destructors may call Godot; release them outside the lease lock.
    drop(released);
}

unsafe extern "C" fn get_class_category(
    _instance: GDExtensionScriptInstanceDataPtr,
    _category: *mut GDExtensionPropertyInfo,
) -> GDExtensionBool {
    0
}

unsafe extern "C" fn property_can_revert(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
) -> GDExtensionBool {
    let instance = data(instance);
    u8::from(
        resolve_property(instance.as_deref(), name)
            .and_then(|property| property.field.as_ref())
            .and_then(ModuleField::property_default_value)
            .is_some(),
    )
}

unsafe extern "C" fn property_get_revert(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
    result: GDExtensionVariantPtr,
) -> GDExtensionBool {
    let Some(instance) = data(instance) else {
        return 0;
    };
    let Some(field) =
        resolve_property(Some(&instance), name).and_then(|property| property.field.as_ref())
    else {
        return 0;
    };
    let Some(value) = field.property_default_value() else {
        return 0;
    };
    u8::from(instance.codec.as_ref().is_some_and(|codec| {
        codec
            .encode_with_array_type(value.abi(), result, field.typed_array_element())
            .is_ok()
    }))
}

unsafe extern "C" fn get_owner(instance: GDExtensionScriptInstanceDataPtr) -> GDExtensionObjectPtr {
    data(instance)
        .map(|instance| instance.owner as GDExtensionObjectPtr)
        .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn get_property_state(
    instance: GDExtensionScriptInstanceDataPtr,
    add: GDExtensionScriptInstancePropertyStateAdd,
    userdata: *mut c_void,
) {
    let (Some(instance), Some(add)) = (data(instance), add) else {
        return;
    };
    let _scope = ActiveScriptScope::enter(&instance);
    if instance.faults.is_fused() {
        return;
    }
    let Some(codec) = instance.codec.as_ref() else {
        return;
    };
    let Some(state) = &instance.state else {
        return;
    };
    let values = match state.try_lock() {
        Ok(state) => instance
            .properties
            .iter()
            .filter(|property| property.usage & ABI_PROPERTY_USAGE_STORAGE != 0)
            .filter_map(|property| {
                let value = state.get_field(property.field.as_ref()?).ok()?;
                Some((property, value))
            })
            .collect::<Vec<_>>(),
        Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => return,
    };
    for (property, value) in values {
        let typed_array_element = property
            .field
            .as_ref()
            .and_then(ModuleField::typed_array_element);
        let Ok(value) = OwnedVariant::from_abi_with_context(
            codec,
            value.borrowed_abi(),
            typed_array_element,
            Some(value.engine_call_context()),
        ) else {
            continue;
        };
        // SAFETY: Name and temporary Variant remain live for this synchronous
        // Godot-owned property-state callback.
        unsafe { add(property.name.as_ptr(), value.as_ptr(), userdata) };
    }
}

unsafe extern "C" fn get_method_list(
    instance: GDExtensionScriptInstanceDataPtr,
    count: *mut u32,
) -> *const GDExtensionMethodInfo {
    if !count.is_null() {
        // SAFETY: Godot supplies a writable count output.
        unsafe { count.write(0) };
    }
    let Some(slot) = slot(instance) else {
        return ptr::null();
    };
    let Ok(current) = slot.current.lock() else {
        return ptr::null();
    };
    if current.method_infos.is_empty() {
        return ptr::null();
    }
    let Ok(mut lists) = slot.method_lists.lock() else {
        return ptr::null();
    };
    if !count.is_null() {
        // SAFETY: The metadata count is bounded when the module is loaded.
        unsafe { count.write(current.method_infos.len() as u32) };
    }
    let pointer = current.method_infos.as_ptr();
    lists.push(current.clone());
    pointer
}

unsafe extern "C" fn free_method_list(
    instance: GDExtensionScriptInstanceDataPtr,
    list: *const GDExtensionMethodInfo,
    _count: u32,
) {
    let Some(slot) = slot(instance) else { return };
    let released = {
        let Ok(mut lists) = slot.method_lists.lock() else {
            return;
        };
        lists
            .iter()
            .position(|payload| payload.method_infos.as_ptr() == list)
            .map(|index| lists.swap_remove(index))
    };
    // State destructors may call Godot; release them outside the lease lock.
    drop(released);
}

unsafe extern "C" fn get_property_type(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
    is_valid: *mut GDExtensionBool,
) -> GDExtensionVariantType {
    let instance = data(instance);
    let property = resolve_property(instance.as_deref(), name);
    write_bool(is_valid, property.is_some());
    property
        .map(|property| property.type_)
        .unwrap_or(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL)
}

unsafe extern "C" fn validate_property(
    _instance: GDExtensionScriptInstanceDataPtr,
    _property: *mut GDExtensionPropertyInfo,
) -> GDExtensionBool {
    0
}

unsafe extern "C" fn has_method(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
) -> GDExtensionBool {
    let instance = data(instance);
    u8::from(resolve_method(instance.as_deref(), name).is_some())
}

unsafe extern "C" fn get_method_argument_count(
    instance: GDExtensionScriptInstanceDataPtr,
    name: GDExtensionConstStringNamePtr,
    is_valid: *mut GDExtensionBool,
) -> GDExtensionInt {
    let instance = data(instance);
    let method = resolve_method(instance.as_deref(), name);
    let valid = method.is_some();
    write_bool(is_valid, valid);
    method.map(|method| method.argument_count()).unwrap_or(0)
}

unsafe extern "C" fn call(
    instance: GDExtensionScriptInstanceDataPtr,
    method: GDExtensionConstStringNamePtr,
    arguments: *const GDExtensionConstVariantPtr,
    argument_count: GDExtensionInt,
    result: GDExtensionVariantPtr,
    error: *mut GDExtensionCallError,
) {
    let Some(instance) = data(instance) else {
        write_call_error(
            error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
            0,
            0,
        );
        return;
    };
    if instance.faults.is_fused() {
        instance.finish_failed_call(result, error);
        return;
    }
    let Some(method) = resolve_method(Some(&instance), method) else {
        write_call_error(
            error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_METHOD,
            0,
            0,
        );
        return;
    };
    let callback_name = method.name();
    let Ok(supplied_count) = usize::try_from(argument_count) else {
        write_call_error(
            error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_FEW_ARGUMENTS,
            0,
            i32::try_from(method.minimum_argument_count()).unwrap_or(i32::MAX),
        );
        return;
    };
    let expected_count = usize::try_from(method.argument_count()).unwrap_or(usize::MAX);
    let minimum_count = method.minimum_argument_count();
    if supplied_count < minimum_count {
        write_call_error(
            error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_FEW_ARGUMENTS,
            i32::try_from(supplied_count).unwrap_or(i32::MAX),
            i32::try_from(minimum_count).unwrap_or(i32::MAX),
        );
        return;
    }
    if !method.is_vararg() && supplied_count > expected_count {
        write_call_error(
            error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_MANY_ARGUMENTS,
            i32::try_from(expected_count).unwrap_or(i32::MAX),
            i32::try_from(expected_count).unwrap_or(i32::MAX),
        );
        return;
    }
    let prepared = match prepare_call(&instance, method, arguments, supplied_count) {
        Ok(prepared) => prepared,
        Err(mismatch) => {
            write_call_error(
                error,
                GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_ARGUMENT,
                mismatch.index,
                mismatch.expected.0 as i32,
            );
            return;
        }
    };
    let Some(state) = &instance.state else {
        write_call_error(
            error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
            0,
            0,
        );
        return;
    };
    let returned_array_type = match &prepared.target {
        PreparedTarget::Reflected(method) if method.return_type() == AbiValueType::ARRAY => {
            method.return_class_name().map(str::to_owned)
        }
        _ => None,
    };
    let _debug_scope = crate::debugger::CallbackScope::enter(
        &instance.source_path,
        callback_name,
        instance.identity.address.load(Ordering::Acquire),
        || match &prepared.target {
            PreparedTarget::Reflected(method) => method
                .arguments()
                .zip(prepared.arguments.as_slice())
                .filter_map(|((name, _), value)| {
                    debug_value_text(*value).map(|value| (name.to_owned(), value))
                })
                .collect(),
            PreparedTarget::Lifecycle(_) => prepared
                .arguments
                .as_slice()
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    let name = match callback_name {
                        "_process" | "_physics_process" => "delta".to_owned(),
                        "_input" | "_unhandled_input" => "event".to_owned(),
                        _ => format!("arg{index}"),
                    };
                    debug_value_text(*value).map(|value| (name, value))
                })
                .collect(),
        },
    );
    let _profile_scope = crate::profiler::ProfileScope::enter(&instance.source_path, callback_name);
    let outcome = match state.try_lock() {
        Ok(mut state) => {
            let _script_scope = ActiveScriptScope::enter(&instance);
            match prepared.target {
                PreparedTarget::Lifecycle(invocation) => match invocation {
                    LifecycleInvocation::EnterTree => state.enter_tree(),
                    LifecycleInvocation::Ready => {
                        let interface = instance
                            .interface
                            .expect("live ScriptInstance retains its engine interface");
                        let owner = instance.owner as GDExtensionObjectPtr;
                        crate::node_resolver::inject_node_fields(interface, owner, &mut state)
                            .and_then(|()| state.ready())
                    }
                    LifecycleInvocation::Process(delta) => state.process(delta),
                    LifecycleInvocation::PhysicsProcess(delta) => state.physics_process(delta),
                    LifecycleInvocation::Input(event) => state.input(event),
                    LifecycleInvocation::UnhandledInput(event) => state.unhandled_input(event),
                    LifecycleInvocation::ExitTree => state.exit_tree(),
                }
                .map(|()| None),
                PreparedTarget::Reflected(method) => state
                    .call_method(&method, prepared.arguments.as_slice())
                    .map(Some),
            }
        }
        Err(TryLockError::WouldBlock) => {
            instance.record_failure(
                callback_name,
                AbiStatus::ReentrantCall,
                "synchronous mutable script re-entry was rejected",
            );
            instance.finish_failed_call(result, error);
            return;
        }
        Err(TryLockError::Poisoned(_)) => {
            instance.host_failed(callback_name, "script state lock is poisoned");
            instance.finish_failed_call(result, error);
            return;
        }
    };
    match outcome {
        Ok(Some(value)) => {
            let encoded = instance.codec.as_ref().is_some_and(|codec| {
                codec
                    .encode_with_context(
                        value.borrowed_abi(),
                        result,
                        returned_array_type.as_deref(),
                        Some(value.engine_call_context()),
                    )
                    .is_ok()
            });
            if !encoded {
                instance.host_failed(
                    callback_name,
                    "could not encode the reflected method return value",
                );
                instance.finish_failed_call(result, error);
                return;
            }
            instance.callback_succeeded();
        }
        Ok(None) => instance.callback_succeeded(),
        Err(module_error) => {
            instance.callback_failed(callback_name, &module_error);
            instance.finish_failed_call(result, error);
            return;
        }
    }
    write_call_error(error, GDExtensionCallErrorType::GDEXTENSION_CALL_OK, 0, 0);
}

/// Emits a project-module signal on the ScriptInstance currently inside a
/// lifecycle or reflected method callback.
pub(crate) unsafe extern "C" fn emit_signal_from_module(
    _context: *mut c_void,
    signal_index: u32,
    arguments: *const AbiValueV1,
    argument_count: u32,
) -> AbiCallResult {
    if argument_count as usize > MAX_SIGNAL_ARGUMENTS {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "signal argument count exceeds the Host limit",
        );
    }
    if argument_count != 0 && arguments.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "signal arguments pointer is null",
        );
    }
    let instance = ACTIVE_SCRIPT_INSTANCE.with(Cell::get);
    if instance.is_null() {
        return AbiCallResult::failure(
            AbiStatus::Unsupported,
            "signals can only be emitted during a script callback",
        );
    }
    // SAFETY: The active scope is installed from a live ScriptInstance and is
    // restored before that callback can return or release the instance.
    let instance = unsafe { &*instance };
    let arguments = if argument_count == 0 {
        &[]
    } else {
        // SAFETY: Null was rejected and the project SDK retains this scalar
        // slice for the complete synchronous callback.
        unsafe { core::slice::from_raw_parts(arguments, argument_count as usize) }
    };
    instance.emit_signal(signal_index, arguments)
}

/// Returns the live engine interface and owner for the current script callback.
///
/// The raw Object pointer must only be used synchronously on this thread.
pub(crate) fn active_engine_context() -> Option<(EngineInterface, GDExtensionObjectPtr)> {
    let instance = ACTIVE_SCRIPT_INSTANCE.with(Cell::get);
    if instance.is_null() {
        return None;
    }
    // SAFETY: ActiveScriptScope installs a live ScriptInstance for the
    // duration of one synchronous callback on this thread.
    let instance = unsafe { &*instance };
    let interface = instance.interface?;
    let get_instance = interface.object_get_instance_from_id?;
    // SAFETY: ObjectDB resolves the cached identity without dereferencing an
    // owner that user code may have freed earlier in the callback.
    let owner = unsafe { get_instance(instance.owner_id) };
    (!owner.is_null()).then_some((interface, owner))
}

pub(crate) fn active_engine_interface() -> Option<EngineInterface> {
    active_engine_context()
        .map(|(interface, _owner)| interface)
        .or_else(|| ACTIVE_ENGINE_INTERFACE.with(Cell::get))
}

pub(crate) fn with_engine_interface<T>(
    interface: EngineInterface,
    callback: impl FnOnce() -> T,
) -> T {
    let previous = ACTIVE_ENGINE_INTERFACE.with(|active| active.replace(Some(interface)));
    let _scope = ActiveEngineInterfaceScope { previous };
    callback()
}

fn retained_live_instance(instance_address: usize) -> Option<Arc<RustScriptInstance>> {
    let instances = LIVE_INSTANCES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .ok()?;
    if !instances.contains(&instance_address) {
        return None;
    }
    // The registry lock keeps Godot's reference alive while `data` retains an
    // independent reference. Release the lock before invoking user getters:
    // a getter may synchronously remove its script and call `free_func`.
    data(instance_address as GDExtensionScriptInstanceDataPtr)
}

pub(crate) fn write_debug_members(instance_address: usize, result: GDExtensionTypePtr) -> bool {
    let Some(instance) = retained_live_instance(instance_address) else {
        return false;
    };
    let _scope = ActiveScriptScope::enter(&instance);
    let (Some(interface), Some(codec), Some(state)) = (
        instance.interface,
        instance.codec.as_ref(),
        instance.state.as_ref(),
    ) else {
        return false;
    };
    let Ok(state) = state.try_lock() else {
        return false;
    };
    let Ok(mut dictionary) = crate::dynamic_value::OwnedDictionary::empty(interface) else {
        return false;
    };
    for index in 0..state.field_count() {
        let Some(field) = state.field(index) else {
            continue;
        };
        let Ok(value) = state.get_field(&field) else {
            continue;
        };
        let (Ok(key), Ok(value)) = (
            OwnedVariant::from_abi(codec, AbiValueV1::from_borrowed_utf8(field.name())),
            OwnedVariant::from_abi_with_context(
                codec,
                value.borrowed_abi(),
                field.typed_array_element(),
                Some(value.engine_call_context()),
            ),
        ) else {
            continue;
        };
        if dictionary.insert(&key, &value).is_err() {
            return false;
        }
    }
    dictionary.write_copy(result).is_ok()
}

pub(crate) fn debug_member_expression(instance_address: usize, expression: &str) -> Option<String> {
    let instance = retained_live_instance(instance_address)?;
    let _scope = ActiveScriptScope::enter(&instance);
    let state = instance.state.as_ref()?.try_lock().ok()?;
    for index in 0..state.field_count() {
        let field = state.field(index)?;
        if field.name() != expression {
            continue;
        }
        let value = state.get_field(&field).ok()?;
        return debug_value_text(value.borrowed_abi());
    }
    None
}

pub(crate) fn debug_script_instance(instance_address: usize) -> GDExtensionScriptInstancePtr {
    let Ok(instances) = LIVE_INSTANCES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
    else {
        return ptr::null_mut();
    };
    if !instances.contains(&instance_address) {
        return ptr::null_mut();
    }
    // The registry lock keeps the wrapper alive during this debugger query.
    data(instance_address as GDExtensionScriptInstanceDataPtr)
        .map(|instance| {
            instance.identity.godot_instance.load(Ordering::Acquire) as GDExtensionScriptInstancePtr
        })
        .unwrap_or(ptr::null_mut())
}

fn debug_value_text(value: AbiValueV1) -> Option<String> {
    Some(match value.type_ {
        AbiValueType::NIL => "null".to_owned(),
        AbiValueType::BOOL => (value.payload[0] != 0).to_string(),
        AbiValueType::I64 => (value.payload[0] as i64).to_string(),
        AbiValueType::U64 => value.payload[0].to_string(),
        AbiValueType::F64 => f64::from_bits(value.payload[0]).to_string(),
        AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH => {
            crate::module_value::utf8(&value).ok()?.to_owned()
        }
        AbiValueType::OBJECT_ID => format!("Object({})", value.payload[0]),
        _ => format!("{:?}", value.type_),
    })
}

/// Returns the Object ID owned by the current project-script callback.
pub(crate) unsafe extern "C" fn current_owner_from_module(
    _context: *mut c_void,
    output: *mut u64,
) -> AbiCallResult {
    if output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "current owner output pointer is null",
        );
    }
    let Some((interface, owner)) = active_engine_context() else {
        return AbiCallResult::failure(
            AbiStatus::Unsupported,
            "the current Godot object is only available during a script callback",
        );
    };
    let Some(get_instance_id) = interface.object_get_instance_id else {
        return AbiCallResult::failure(AbiStatus::Internal, "Godot object identity is unavailable");
    };
    // SAFETY: active_engine_context resolved a live owner from ObjectDB for
    // this synchronous call; retaining the script alone does not own the node.
    let object_id = unsafe { get_instance_id(owner) };
    if object_id == 0 {
        return AbiCallResult::failure(
            AbiStatus::CallbackFailed,
            "Godot returned an invalid current object ID",
        );
    }
    // SAFETY: Null was rejected and the SDK retains the output slot for this
    // synchronous callback.
    unsafe { output.write(object_id) };
    AbiCallResult::OK
}

impl RustScriptInstance {
    fn callback_succeeded(&self) {
        self.faults.record_success();
    }

    fn callback_failed(&self, callback: &str, error: &ModuleCallError) {
        self.record_failure(callback, error.status, &error.message);
    }

    fn host_failed(&self, callback: &str, message: &str) {
        self.record_failure(callback, AbiStatus::Internal, message);
    }

    fn finish_failed_call(&self, result: GDExtensionVariantPtr, error: *mut GDExtensionCallError) {
        let encoded_nil = self.codec.as_ref().is_some_and(|codec| {
            codec
                .encode_with_context(AbiValueV1::NIL, result, None, None)
                .is_ok()
        });
        write_call_error(
            error,
            if encoded_nil {
                GDExtensionCallErrorType::GDEXTENSION_CALL_OK
            } else {
                GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_METHOD
            },
            0,
            0,
        );
    }

    fn record_failure(&self, callback: &str, status: AbiStatus, message: &str) {
        let disposition = self.faults.record_failure(status);
        match disposition {
            FaultDisposition::Report => {
                self.report_script_error(callback, status, message, false);
            }
            FaultDisposition::Fused => {
                self.report_script_error(callback, status, message, true);
            }
            FaultDisposition::Suppress => {}
        }
    }

    fn report_script_error(&self, callback: &str, status: AbiStatus, message: &str, fused: bool) {
        let description = format!("Rust script callback `{callback}` failed");
        let details = if fused {
            format!(
                "{status:?}: {message}. This script instance was disabled after a panic or {INSTANCE_FAILURE_LIMIT} consecutive callback failures."
            )
        } else {
            format!("{status:?}: {message}")
        };
        crate::debugger::record_error(format!("{description}: {details}"));
        let Some(interface) = self.interface else {
            host_eprintln!("godot-rust: {description}: {details}");
            return;
        };
        let Some(report) = interface.print_script_error_with_message else {
            host_eprintln!("godot-rust: {description}: {details}");
            return;
        };
        let description = sanitized_c_string(&description);
        let details = sanitized_c_string(&details);
        let callback = sanitized_c_string(callback);
        let source_path = sanitized_c_string(&self.source_path);
        let line = crate::debugger::frames()
            .first()
            .and_then(|frame| i32::try_from(frame.line).ok())
            .unwrap_or_default();
        // SAFETY: All C strings are live and nul-terminated for this
        // synchronous official Godot debugger call.
        unsafe {
            report(
                description.as_ptr(),
                details.as_ptr(),
                callback.as_ptr(),
                source_path.as_ptr(),
                line,
                1,
            );
        }
    }

    fn emit_signal(&self, signal_index: u32, arguments: &[AbiValueV1]) -> AbiCallResult {
        let Some(signal) = self.signals.iter().find(|signal| {
            signal.field_index == signal_index
                && signal.script_resource_uid == crate::module_loader::current_callback_script_uid()
        }) else {
            return AbiCallResult::failure(
                AbiStatus::InvalidArgument,
                "signal field index is not declared by this script",
            );
        };
        if arguments.len() != signal.argument_types.len() {
            return AbiCallResult::failure(
                AbiStatus::InvalidArgument,
                "signal arguments do not match the declared schema",
            );
        }
        if arguments
            .iter()
            .zip(&signal.argument_types)
            .any(|(value, expected)| value.type_ != *expected)
        {
            return AbiCallResult::failure(
                AbiStatus::InvalidArgument,
                "signal argument type does not match the declared schema",
            );
        }
        let (Some(interface), Some(codec)) = (self.interface, self.codec.as_ref()) else {
            return AbiCallResult::failure(AbiStatus::Internal, "signal Host state is unavailable");
        };
        let Some(value_owner) =
            crate::module_loader::active_generation().map(|generation| generation.value_owner())
        else {
            return AbiCallResult::failure(
                AbiStatus::Internal,
                "signal value owner is unavailable",
            );
        };
        let Ok(name) = OwnedVariant::from_string_name(codec, &signal.name) else {
            return AbiCallResult::failure(AbiStatus::Internal, "could not encode the signal name");
        };
        let mut variants: [Option<OwnedVariant>; MAX_SIGNAL_ARGUMENTS + 1] =
            core::array::from_fn(|_| None);
        variants[0] = Some(name);
        for (index, (value, expected)) in arguments
            .iter()
            .copied()
            .zip(signal.argument_types.iter().copied())
            .enumerate()
        {
            let argument = match value_owner.signal(expected, value) {
                Ok(value) => value,
                Err(error) => {
                    return AbiCallResult::failure(
                        error.status,
                        "signal argument violates its declared value contract",
                    );
                }
            };
            let Ok(variant) = OwnedVariant::from_abi_with_context(
                codec,
                argument.borrowed_abi(),
                None,
                Some(argument.engine_call_context()),
            ) else {
                return AbiCallResult::failure(
                    AbiStatus::InvalidArgument,
                    "signal argument has an invalid payload",
                );
            };
            variants[index + 1] = Some(variant);
        }
        let count = arguments.len() + 1;
        let mut argument_pointers = [ptr::null(); MAX_SIGNAL_ARGUMENTS + 1];
        for (pointer, variant) in argument_pointers.iter_mut().zip(&variants).take(count) {
            *pointer = variant
                .as_ref()
                .expect("initialized signal argument")
                .as_ptr();
        }
        let Some(call) = interface.object_method_bind_call else {
            return AbiCallResult::failure(
                AbiStatus::Internal,
                "Godot object method calls are unavailable",
            );
        };
        let owner = interface
            .object_get_instance_from_id
            .map_or(ptr::null_mut(), |lookup| {
                // SAFETY: Resolve the identity without accessing an expired owner.
                unsafe { lookup(self.owner_id) }
            });
        if self.emit_signal_method == 0 || owner.is_null() {
            return AbiCallResult::failure(
                AbiStatus::Internal,
                "signal method or owning Godot object is unavailable",
            );
        }
        let mut result = OwnedVariant::uninitialized(interface);
        let mut error = GDExtensionCallError {
            error: GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
            argument: 0,
            expected: 0,
        };
        // SAFETY: MethodBind, owner and all argument Variants belong to this
        // live ScriptInstance. Godot initializes the return Variant.
        unsafe {
            call(
                self.emit_signal_method as GDExtensionMethodBindPtr,
                owner,
                argument_pointers.as_ptr(),
                count as GDExtensionInt,
                result.as_mut_ptr(),
                &mut error,
            );
        }
        result.mark_initialized();
        if error.error != GDExtensionCallErrorType::GDEXTENSION_CALL_OK {
            return AbiCallResult::failure(
                AbiStatus::CallbackFailed,
                "Godot rejected Object.emit_signal",
            );
        }
        AbiCallResult::OK
    }
}

unsafe extern "C" fn notification(
    _instance: GDExtensionScriptInstanceDataPtr,
    _what: i32,
    _reversed: GDExtensionBool,
) {
}

unsafe extern "C" fn to_string(
    _instance: GDExtensionScriptInstanceDataPtr,
    is_valid: *mut GDExtensionBool,
    _result: GDExtensionStringPtr,
) {
    write_bool(is_valid, false);
}

unsafe extern "C" fn refcount_incremented(_instance: GDExtensionScriptInstanceDataPtr) {}

unsafe extern "C" fn refcount_decremented(
    _instance: GDExtensionScriptInstanceDataPtr,
) -> GDExtensionBool {
    // Rust script state does not keep a separate garbage-collected ownership
    // edge. Let Godot destroy the RefCounted owner when its native reference
    // count reaches zero.
    1
}

unsafe extern "C" fn get_script(
    instance: GDExtensionScriptInstanceDataPtr,
) -> GDExtensionObjectPtr {
    data(instance)
        .map(|instance| instance.script as GDExtensionObjectPtr)
        .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn is_placeholder(
    _instance: GDExtensionScriptInstanceDataPtr,
) -> GDExtensionBool {
    0
}

unsafe extern "C" fn get_language(
    instance: GDExtensionScriptInstanceDataPtr,
) -> GDExtensionScriptLanguagePtr {
    data(instance)
        .map(|instance| instance.language as GDExtensionScriptLanguagePtr)
        .unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn free(instance: GDExtensionScriptInstanceDataPtr) {
    if !instance.is_null() {
        let instances = LIVE_INSTANCES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock();
        // A poisoned registry must still be repaired before releasing the
        // engine's reference; otherwise a stale address could later be dereferenced
        // by generation migration.
        let mut instances = instances.unwrap_or_else(|poisoned| poisoned.into_inner());
        instances.remove(&(instance as usize));
        drop(instances);
        // SAFETY: Godot releases the reference created by `into_instance_data`
        // exactly once. Active callbacks retain their own strong references.
        unsafe {
            let slot = Arc::from_raw(instance.cast::<InstanceSlot>());
            let current = slot
                .current
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            current.identity.godot_instance.store(0, Ordering::Release);
            drop(current);
            drop(slot);
        };
    }
}

fn write_bool(output: *mut GDExtensionBool, value: bool) {
    if !output.is_null() {
        // SAFETY: This helper is only called with writable ABI output slots.
        unsafe { output.write(GDExtensionBool::from(value)) };
    }
}

fn sanitized_c_string(value: &str) -> CString {
    CString::new(value).unwrap_or_else(|_| {
        CString::new(value.replace('\0', "\u{fffd}"))
            .expect("replacement text contains no nul bytes")
    })
}

#[derive(Clone, Copy)]
enum LifecycleMethod {
    EnterTree,
    Ready,
    Process,
    PhysicsProcess,
    Input,
    UnhandledInput,
    ExitTree,
}

impl LifecycleMethod {
    const fn argument_count(self) -> GDExtensionInt {
        match self {
            Self::EnterTree | Self::Ready | Self::ExitTree => 0,
            Self::Process | Self::PhysicsProcess | Self::Input | Self::UnhandledInput => 1,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::EnterTree => "_enter_tree",
            Self::Ready => "_ready",
            Self::Process => "_process",
            Self::PhysicsProcess => "_physics_process",
            Self::Input => "_input",
            Self::UnhandledInput => "_unhandled_input",
            Self::ExitTree => "_exit_tree",
        }
    }
}

#[derive(Clone, Copy)]
enum ResolvedMethod<'a> {
    Lifecycle(LifecycleMethod),
    Reflected(&'a ReflectedMethod),
}

impl<'a> ResolvedMethod<'a> {
    fn name(self) -> &'a str {
        match self {
            Self::Lifecycle(method) => method.name(),
            Self::Reflected(method) => method.method.name(),
        }
    }

    fn argument_count(self) -> GDExtensionInt {
        match self {
            Self::Lifecycle(method) => method.argument_count(),
            Self::Reflected(method) => {
                i64::try_from(method.method.argument_types().len()).unwrap_or(i64::MAX)
            }
        }
    }

    fn minimum_argument_count(self) -> usize {
        match self {
            Self::Lifecycle(method) => {
                usize::try_from(method.argument_count()).unwrap_or(usize::MAX)
            }
            Self::Reflected(method) => method.method.minimum_argument_count(),
        }
    }

    fn is_vararg(self) -> bool {
        matches!(self, Self::Reflected(method) if method.method.is_vararg())
    }
}

enum LifecycleInvocation {
    EnterTree,
    Ready,
    Process(f64),
    PhysicsProcess(f64),
    Input(u64),
    UnhandledInput(u64),
    ExitTree,
}

struct PreparedCall {
    target: PreparedTarget,
    arguments: MethodArgumentValues,
    _value_owner: Option<ModuleValueOwner>,
    _string_backing: Vec<String>,
    _math_backing: Vec<Box<[f32]>>,
    _packed_backing: Vec<Box<[u8]>>,
    _dynamic_backing: Vec<crate::dynamic_value::DynamicCallBacking>,
    _callable_backing: Vec<crate::callable_value::CallableCallBacking>,
}

enum PreparedTarget {
    Lifecycle(LifecycleInvocation),
    Reflected(ModuleMethod),
}

const INLINE_METHOD_ARGUMENTS: usize = 8;

enum MethodArgumentValues {
    Inline {
        values: [AbiValueV1; INLINE_METHOD_ARGUMENTS],
        len: usize,
    },
    Heap(Vec<AbiValueV1>),
}

impl MethodArgumentValues {
    fn new(count: usize) -> Self {
        if count <= INLINE_METHOD_ARGUMENTS {
            Self::Inline {
                values: [AbiValueV1::NIL; INLINE_METHOD_ARGUMENTS],
                len: count,
            }
        } else {
            Self::Heap(vec![AbiValueV1::NIL; count])
        }
    }

    fn as_mut_slice(&mut self) -> &mut [AbiValueV1] {
        match self {
            Self::Inline { values, len } => &mut values[..*len],
            Self::Heap(values) => values,
        }
    }

    fn as_slice(&self) -> &[AbiValueV1] {
        match self {
            Self::Inline { values, len } => &values[..*len],
            Self::Heap(values) => values,
        }
    }
}

#[derive(Clone, Copy)]
struct ArgumentMismatch {
    index: i32,
    expected: GDExtensionVariantType,
}

fn resolve_property(
    instance: Option<&RustScriptInstance>,
    name: GDExtensionConstStringNamePtr,
) -> Option<&ReflectedProperty> {
    let instance = instance?;
    if name.is_null() {
        return None;
    }
    instance
        .properties
        .iter()
        .find(|property| property.field.is_some() && property.name.equals(name))
}

fn resolve_method(
    instance: Option<&RustScriptInstance>,
    name: GDExtensionConstStringNamePtr,
) -> Option<ResolvedMethod<'_>> {
    let instance = instance?;
    let interface = instance.interface?;
    if name.is_null() {
        return None;
    }
    if instance.available.enter_tree
        && instance
            .names
            .enter_tree
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::EnterTree))
    } else if instance.available.ready
        && instance
            .names
            .ready
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::Ready))
    } else if instance.available.process
        && instance
            .names
            .process
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::Process))
    } else if instance.available.physics_process
        && instance
            .names
            .physics_process
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::PhysicsProcess))
    } else if instance.available.input
        && instance
            .names
            .input
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::Input))
    } else if instance.available.unhandled_input
        && instance
            .names
            .unhandled_input
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::UnhandledInput))
    } else if instance.available.exit_tree
        && instance
            .names
            .exit_tree
            .as_ref()
            .is_some_and(|expected| expected.equals(interface, name))
    {
        Some(ResolvedMethod::Lifecycle(LifecycleMethod::ExitTree))
    } else {
        instance
            .methods
            .iter()
            .find(|method| method.name.equals(name))
            .map(ResolvedMethod::Reflected)
    }
}

fn prepare_call(
    instance: &RustScriptInstance,
    method: ResolvedMethod<'_>,
    arguments: *const GDExtensionConstVariantPtr,
    supplied_count: usize,
) -> Result<PreparedCall, ArgumentMismatch> {
    match method {
        ResolvedMethod::Lifecycle(method) => decode_invocation(instance, method, arguments)
            .map(|invocation| PreparedCall {
                target: PreparedTarget::Lifecycle(invocation),
                arguments: MethodArgumentValues::new(0),
                _value_owner: None,
                _string_backing: Vec::new(),
                _math_backing: Vec::new(),
                _packed_backing: Vec::new(),
                _dynamic_backing: Vec::new(),
                _callable_backing: Vec::new(),
            })
            .map_err(|error| ArgumentMismatch {
                index: 0,
                expected: error.expected,
            }),
        ResolvedMethod::Reflected(reflected) => {
            let method = &reflected.method;
            let Some(codec) = instance.codec.as_ref() else {
                return Err(ArgumentMismatch {
                    index: 0,
                    expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
                });
            };
            let fixed_count = method.argument_types().len();
            let total_count = supplied_count.max(fixed_count);
            let mut values = MethodArgumentValues::new(total_count);
            let mut string_backing = Vec::new();
            let mut math_backing = Vec::new();
            let mut packed_backing = Vec::new();
            let mut dynamic_backing = Vec::new();
            let mut callable_backing = Vec::new();
            let Some(value_owner) = crate::module_loader::active_generation()
                .map(|generation| generation.value_owner())
            else {
                return Err(ArgumentMismatch {
                    index: 0,
                    expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
                });
            };
            let dynamic_context = Some(value_owner.engine_call_context());
            for (index, output) in values.as_mut_slice().iter_mut().enumerate() {
                let expected = method
                    .argument_types()
                    .get(index)
                    .copied()
                    .unwrap_or(AbiValueType::VARIANT);
                let argument = if index < supplied_count {
                    if arguments.is_null() {
                        return Err(ArgumentMismatch {
                            index: index as i32,
                            expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
                        });
                    }
                    // SAFETY: The supplied arity was validated before
                    // preparing this call.
                    unsafe { *arguments.add(index) }
                } else {
                    let default_index = index.saturating_sub(method.minimum_argument_count());
                    let Some(default) = reflected.default_arguments.get(default_index) else {
                        return Err(ArgumentMismatch {
                            index: index as i32,
                            expected: variant_type(expected),
                        });
                    };
                    default
                };
                if argument.is_null() {
                    return Err(ArgumentMismatch {
                        index: index as i32,
                        expected: variant_type(expected),
                    });
                }
                let decoded = codec
                    .decode(
                        argument,
                        expected,
                        VariantDecodeBacking {
                            strings: &mut string_backing,
                            math: &mut math_backing,
                            packed: &mut packed_backing,
                            dynamic: &mut dynamic_backing,
                            callable: &mut callable_backing,
                            dynamic_context,
                        },
                    )
                    .map_err(|error| ArgumentMismatch {
                        index: index as i32,
                        expected: error.expected,
                    })?;
                if index < fixed_count
                    && expected == AbiValueType::OBJECT_ID
                    && method.argument_class_name(index).is_some_and(|class_name| {
                        !codec.object_is_class(decoded.payload[0], class_name)
                    })
                {
                    return Err(ArgumentMismatch {
                        index: index as i32,
                        expected: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
                    });
                }
                *output = decoded;
            }
            Ok(PreparedCall {
                target: PreparedTarget::Reflected(method.clone()),
                arguments: values,
                _value_owner: Some(value_owner),
                _string_backing: string_backing,
                _math_backing: math_backing,
                _packed_backing: packed_backing,
                _dynamic_backing: dynamic_backing,
                _callable_backing: callable_backing,
            })
        }
    }
}

fn decode_invocation(
    instance: &RustScriptInstance,
    method: LifecycleMethod,
    arguments: *const GDExtensionConstVariantPtr,
) -> Result<LifecycleInvocation, VariantTypeMismatch> {
    let argument = || {
        if arguments.is_null() {
            return None;
        }
        // SAFETY: The validated method arity requires one live argument slot.
        Some(unsafe { *arguments })
    };
    match method {
        LifecycleMethod::EnterTree => Ok(LifecycleInvocation::EnterTree),
        LifecycleMethod::Ready => Ok(LifecycleInvocation::Ready),
        LifecycleMethod::ExitTree => Ok(LifecycleInvocation::ExitTree),
        LifecycleMethod::Process | LifecycleMethod::PhysicsProcess => {
            let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT;
            let codec = instance
                .codec
                .as_ref()
                .ok_or(VariantTypeMismatch { expected })?;
            let delta = codec.read_f64(argument().ok_or(VariantTypeMismatch { expected })?)?;
            Ok(match method {
                LifecycleMethod::Process => LifecycleInvocation::Process(delta),
                LifecycleMethod::PhysicsProcess => LifecycleInvocation::PhysicsProcess(delta),
                _ => unreachable!("matched process lifecycle"),
            })
        }
        LifecycleMethod::Input | LifecycleMethod::UnhandledInput => {
            let expected = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT;
            let codec = instance
                .codec
                .as_ref()
                .ok_or(VariantTypeMismatch { expected })?;
            let event =
                codec.read_object_id(argument().ok_or(VariantTypeMismatch { expected })?)?;
            Ok(match method {
                LifecycleMethod::Input => LifecycleInvocation::Input(event),
                LifecycleMethod::UnhandledInput => LifecycleInvocation::UnhandledInput(event),
                _ => unreachable!("matched input lifecycle"),
            })
        }
    }
}

fn write_call_error(
    output: *mut GDExtensionCallError,
    kind: GDExtensionCallErrorType,
    argument: i32,
    expected: i32,
) {
    if !output.is_null() {
        // SAFETY: Godot supplies a writable call error output.
        unsafe {
            output.write(GDExtensionCallError {
                error: kind,
                argument,
                expected,
            })
        };
    }
}

static INSTANCE_INFO: GDExtensionScriptInstanceInfo3 = GDExtensionScriptInstanceInfo3 {
    set_func: Some(set),
    get_func: Some(get),
    get_property_list_func: Some(get_property_list),
    free_property_list_func: Some(free_property_list),
    get_class_category_func: Some(get_class_category),
    property_can_revert_func: Some(property_can_revert),
    property_get_revert_func: Some(property_get_revert),
    get_owner_func: Some(get_owner),
    get_property_state_func: Some(get_property_state),
    get_method_list_func: Some(get_method_list),
    free_method_list_func: Some(free_method_list),
    get_property_type_func: Some(get_property_type),
    validate_property_func: Some(validate_property),
    has_method_func: Some(has_method),
    get_method_argument_count_func: Some(get_method_argument_count),
    call_func: Some(call),
    notification_func: Some(notification),
    to_string_func: Some(to_string),
    refcount_incremented_func: Some(refcount_incremented),
    refcount_decremented_func: Some(refcount_decremented),
    get_script_func: Some(get_script),
    is_placeholder_func: Some(is_placeholder),
    set_fallback_func: Some(set),
    get_fallback_func: Some(get),
    get_language_func: Some(get_language),
    free_func: Some(free),
};

#[cfg(test)]
mod tests {
    use super::*;

    fn test_instance() -> RustScriptInstance {
        RustScriptInstance {
            interface: None,
            codec: None,
            owner: 1,
            owner_id: 1,
            script: 2,
            language: 3,
            source_path: "res://minimal.rs".into(),
            script_uids: Vec::new(),
            identity: Arc::new(InstanceIdentity::default()),
            faults: InstanceFaults::default(),
            names: LifecycleMethodNames {
                enter_tree: None,
                ready: None,
                process: None,
                physics_process: None,
                input: None,
                unhandled_input: None,
                exit_tree: None,
            },
            available: LifecycleAvailability {
                enter_tree: false,
                ready: false,
                process: false,
                physics_process: false,
                input: false,
                unhandled_input: false,
                exit_tree: false,
            },
            properties: Vec::new(),
            property_infos: Vec::new(),
            methods: Vec::new(),
            method_infos: Vec::new(),
            signals: Vec::new(),
            emit_signal_method: 0,
            _empty_metadata_name: None,
            _empty_metadata_hint: None,
            state: None,
            _script_ref: None,
        }
    }

    #[test]
    fn metadata_leases_keep_retired_generations_alive_until_each_list_is_returned() {
        let mut instance = test_instance();
        instance.property_infos.push(GDExtensionPropertyInfo {
            type_: GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
            name: ptr::null_mut(),
            class_name: ptr::null_mut(),
            hint: 0,
            hint_string: ptr::null_mut(),
            usage: 2,
        });
        let pointer = into_instance_data(instance);
        let active = data(pointer).expect("old callback snapshot");
        let old_lifetime = Arc::downgrade(&active);
        let mut count = 0;
        // SAFETY: The live fixture owns its list; Godot member names are not
        // inspected in this test of the get/free ownership protocol.
        let (first, second) = unsafe {
            (
                get_property_list(pointer, &mut count),
                get_property_list(pointer, &mut count),
            )
        };
        assert_eq!(count, 1);
        let slot = slot(pointer).expect("stable slot");
        let mut next = test_instance();
        next.source_path = "res://replacement.rs".into();
        next.identity = active.identity.clone();
        #[allow(clippy::arc_with_non_send_sync)]
        {
            *slot.current.lock().unwrap() = Arc::new(next);
        }
        drop(active);
        assert_eq!(data(pointer).unwrap().source_path, "res://replacement.rs");
        // SAFETY: Both leases still retain the same old metadata allocation.
        unsafe {
            assert_eq!((*first).usage, 2);
            free_property_list(pointer, second, count);
            assert!(old_lifetime.upgrade().is_some());
            assert_eq!(
                (*first).type_,
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT
            );
            free_property_list(pointer, first, count);
        }
        assert!(old_lifetime.upgrade().is_none());
        // SAFETY: Release Godot's single reference after returning both lists.
        unsafe {
            free(pointer);
        }
    }

    #[test]
    fn refcounted_script_owner_can_die_at_zero_native_references() {
        // SAFETY: This callback does not dereference its opaque instance
        // argument.
        assert_eq!(unsafe { refcount_decremented(ptr::null_mut()) }, 1);
    }

    #[test]
    fn signal_emission_requires_an_active_script_callback() {
        // SAFETY: A zero-argument emission permits a null argument pointer and
        // this test intentionally has no active ScriptInstance scope.
        let result = unsafe { emit_signal_from_module(ptr::null_mut(), 0, ptr::null(), 0) };
        assert_eq!(result.status, AbiStatus::Unsupported);
    }

    #[test]
    fn current_owner_rejects_invalid_or_inactive_calls() {
        // SAFETY: These inputs intentionally exercise validation before any
        // output pointer or ScriptInstance is dereferenced.
        let null_output = unsafe { current_owner_from_module(ptr::null_mut(), ptr::null_mut()) };
        assert_eq!(null_output.status, AbiStatus::InvalidArgument);

        let mut output = u64::MAX;
        // SAFETY: `output` is writable and this thread intentionally has no
        // active project-script callback.
        let inactive = unsafe { current_owner_from_module(ptr::null_mut(), &mut output) };
        assert_eq!(inactive.status, AbiStatus::Unsupported);
        assert_eq!(output, u64::MAX);
    }

    #[test]
    fn instance_metadata_survives_removal_until_the_active_callback_returns() {
        let data = into_instance_data(test_instance());
        let mut property_count = u32::MAX;
        let mut method_count = u32::MAX;
        let mut valid = 1;
        LIVE_INSTANCES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .expect("live instance registry")
            .insert(data as usize);
        let active_callback =
            retained_live_instance(data as usize).expect("retained debugger instance");
        let lifetime = Arc::downgrade(&active_callback);

        // SAFETY: The callbacks receive the live test instance and writable
        // local output slots expected by their ABI contracts.
        unsafe {
            assert!((get_property_list(data, &mut property_count)).is_null());
            assert!((get_method_list(data, &mut method_count)).is_null());
            assert_eq!(
                get_property_type(data, ptr::null(), &mut valid),
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL
            );
            assert_eq!(get_owner(data) as usize, 1);
            assert_eq!(get_script(data) as usize, 2);
            assert_eq!(get_language(data) as usize, 3);
            assert!(belongs_to_script(data, 2_usize as GDExtensionObjectPtr));
            assert!(!belongs_to_script(data, 4_usize as GDExtensionObjectPtr));
            free(data);
        }

        assert!(retained_live_instance(data as usize).is_none());
        assert_eq!(active_callback.source_path, "res://minimal.rs");
        assert_eq!(lifetime.strong_count(), 1);
        drop(active_callback);
        assert!(lifetime.upgrade().is_none());

        assert_eq!(property_count, 0);
        assert_eq!(method_count, 0);
        assert_eq!(valid, 0);
    }

    #[test]
    fn unknown_calls_return_an_explicit_error() {
        let data = into_instance_data(test_instance());
        let mut error = GDExtensionCallError {
            error: GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
            argument: -1,
            expected: -1,
        };

        // SAFETY: The callback does not dereference instance, method, arguments,
        // or result for an unknown method; `error` is writable.
        unsafe {
            call(
                data,
                ptr::null(),
                ptr::null(),
                0,
                ptr::null_mut(),
                &mut error,
            );
            free(data);
        };

        assert_eq!(
            error.error,
            GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_METHOD
        );
        assert_eq!(error.argument, 0);
        assert_eq!(error.expected, 0);
    }

    #[test]
    fn lifecycle_method_arities_match_godot_callbacks() {
        assert_eq!(LifecycleMethod::EnterTree.argument_count(), 0);
        assert_eq!(LifecycleMethod::Ready.argument_count(), 0);
        assert_eq!(LifecycleMethod::ExitTree.argument_count(), 0);
        assert_eq!(LifecycleMethod::Process.argument_count(), 1);
        assert_eq!(LifecycleMethod::PhysicsProcess.argument_count(), 1);
        assert_eq!(LifecycleMethod::Input.argument_count(), 1);
        assert_eq!(LifecycleMethod::UnhandledInput.argument_count(), 1);
    }

    #[test]
    fn repeated_callback_failures_fuse_only_the_affected_instance() {
        let first = InstanceFaults::default();
        let second = InstanceFaults::default();

        assert_eq!(
            first.record_failure(AbiStatus::CallbackFailed),
            FaultDisposition::Report
        );
        assert_eq!(
            first.record_failure(AbiStatus::CallbackFailed),
            FaultDisposition::Suppress
        );
        assert_eq!(
            first.record_failure(AbiStatus::CallbackFailed),
            FaultDisposition::Fused
        );
        assert!(first.is_fused());
        assert!(!second.is_fused());
        assert_eq!(
            first.record_failure(AbiStatus::CallbackFailed),
            FaultDisposition::Suppress
        );
    }

    #[test]
    fn successful_callbacks_reset_failures_but_panics_fuse_immediately() {
        let faults = InstanceFaults::default();
        assert_eq!(
            faults.record_failure(AbiStatus::Internal),
            FaultDisposition::Report
        );
        assert_eq!(
            faults.record_failure(AbiStatus::Internal),
            FaultDisposition::Suppress
        );
        faults.record_success();
        assert_eq!(
            faults.record_failure(AbiStatus::Internal),
            FaultDisposition::Report
        );
        assert_eq!(
            faults.record_failure(AbiStatus::Panic),
            FaultDisposition::Fused
        );
        faults.record_success();
        assert!(faults.is_fused());
    }

    #[test]
    fn debugger_text_replaces_embedded_nul_bytes() {
        let value = sanitized_c_string("method\0name");
        assert_eq!(value.to_str().expect("sanitized UTF-8"), "method�name");
    }
}
