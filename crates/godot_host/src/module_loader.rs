use core::ffi::c_void;
use core::fmt;
use core::mem::MaybeUninit;
use core::ptr;
use godot_api::abi::{
    ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA, ABI_FIELD_EXTENSION_NODE_SCHEMA,
    ABI_FIELD_EXTENSION_PROPERTY_SCHEMA, ABI_FIELD_EXTENSION_RELOAD_SCHEMA,
    ABI_FIELD_EXTENSION_SIGNAL_SCHEMA, ABI_METHOD_EXTENSION_ARGUMENT_CLASSES,
    ABI_METHOD_EXTENSION_RETURN_CLASS, ABI_METHOD_EXTENSION_SCHEMA_V1, ABI_METHOD_SCHEMA_VARARG,
    ABI_MINOR, ABI_MODULE_EXTENSION_GODOT_API, ABI_MODULE_EXTENSION_OWNED_VALUES,
    ABI_MODULE_EXTENSION_TASKS, ABI_PROPERTY_HINT_COLOR_NO_ALPHA, ABI_PROPERTY_HINT_ENUM,
    ABI_PROPERTY_HINT_FILE, ABI_PROPERTY_HINT_FLAGS, ABI_PROPERTY_HINT_MULTILINE_TEXT,
    ABI_PROPERTY_HINT_NODE_TYPE, ABI_PROPERTY_HINT_NONE, ABI_PROPERTY_HINT_RANGE,
    ABI_PROPERTY_HINT_RESOURCE_TYPE, ABI_PROPERTY_HINT_TYPE_STRING,
    ABI_PROPERTY_USAGE_NODE_PATH_FROM_SCENE_ROOT, ABI_PROPERTY_USAGE_SCRIPT_DEFAULT,
    ABI_SCRIPT_EXTENSION_BASE_SCRIPT, ABI_SCRIPT_EXTENSION_FIELD_ACCESS,
    ABI_SCRIPT_EXTENSION_GLOBAL_CLASS, ABI_SCRIPT_EXTENSION_RESOURCE_UID, AbiByteSlice,
    AbiByteSliceSlice, AbiCallResult, AbiCallScriptMethodFn, AbiCancelTasksFn, AbiDropValueFn,
    AbiFieldDescriptorV1, AbiFieldKind, AbiFixedMathDefaultV1, AbiGetScriptDescriptorFn,
    AbiGetScriptFieldFn, AbiGodotIntegerDefaultFn, AbiGodotIntegerOptionV1, AbiLifecycleSlot,
    AbiLifecycleTableV1, AbiLogLevel, AbiMethodArgumentSlice, AbiMethodDefaultFn,
    AbiMethodDescriptorV1, AbiMethodExtensionsV1, AbiMethodKind, AbiPollTasksFn, AbiPropertyType,
    AbiReceiverKind, AbiReloadPolicy, AbiRpcConfigV1, AbiScriptDescriptorV1, AbiSetScriptFieldFn,
    AbiSignalArgumentDescriptorV1, AbiStatus, AbiValueType, AbiValueTypeSlice, AbiValueV1,
    HOST_API_SLOT_CALL_GODOT_METHOD, HOST_API_SLOT_CALL_SUPER_METHOD, HOST_API_SLOT_CANCEL_SIGNAL,
    HOST_API_SLOT_CURRENT_OWNER, HOST_API_SLOT_DROP_VALUE, HOST_API_SLOT_EMIT_SIGNAL,
    HOST_API_SLOT_POLL_SIGNAL, HOST_API_SLOT_RETAIN_CALLABLE_VALUE,
    HOST_API_SLOT_RETAIN_DYNAMIC_VALUE, HOST_API_SLOT_WATCH_SIGNAL, HostApiV1,
    MODULE_API_SLOT_CANCEL_TASKS, MODULE_API_SLOT_DROP_VALUE, MODULE_API_SLOT_GODOT_API_MAJOR,
    MODULE_API_SLOT_GODOT_API_MINOR, MODULE_API_SLOT_POLL_TASKS, ModuleApiV1,
    decode_node_field_class, decode_resource_uid_words,
};
use libloading::Library;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use crate::module_value::{self, HostValue, ModuleValue};
use crate::version::EngineVersion;

const MODULE_ENTRY: &[u8] = b"godot_rs_module_entry\0";
const MAX_SCRIPT_COUNT: u32 = 65_536;
const MAX_MEMBER_COUNT: u32 = 65_536;
const MAX_SIGNAL_ARGUMENT_COUNT: usize = 8;
const MAX_ABI_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) enum ModuleLoadError {
    Open(String),
    MissingEntry(String),
    EntryRejected(AbiStatus),
    IncompatibleModule,
    IncompatibleGodotApi {
        module: ModuleGodotApi,
        engine: EngineVersion,
    },
    InvalidModuleTable(String),
    MissingScriptGetter,
    TooManyScripts(u32),
    InvalidDescriptor {
        script: u32,
        message: String,
    },
    DuplicateSourcePath(String),
    DuplicateResourceUid(i64),
}

impl fmt::Display for ModuleLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(message) => write!(formatter, "could not open project module: {message}"),
            Self::MissingEntry(message) => {
                write!(
                    formatter,
                    "project module has no `godot_rs_module_entry`: {message}"
                )
            }
            Self::EntryRejected(status) => {
                write!(
                    formatter,
                    "project module rejected Host ABI with status {status:?}"
                )
            }
            Self::IncompatibleModule => {
                formatter.write_str("project module returned an incompatible ABI table")
            }
            Self::IncompatibleGodotApi { module, engine } => write!(
                formatter,
                "project module requires Godot {}.{}, but the editor is Godot {}.{}.{}",
                module.major, module.minor, engine.major, engine.minor, engine.patch
            ),
            Self::InvalidModuleTable(message) => {
                write!(
                    formatter,
                    "project module returned an invalid ABI table: {message}"
                )
            }
            Self::MissingScriptGetter => {
                formatter.write_str("project module returned no script descriptor getter")
            }
            Self::TooManyScripts(count) => {
                write!(
                    formatter,
                    "project module declares too many scripts: {count}"
                )
            }
            Self::InvalidDescriptor { script, message } => {
                write!(
                    formatter,
                    "project script descriptor {script} is invalid: {message}"
                )
            }
            Self::DuplicateSourcePath(path) => {
                write!(
                    formatter,
                    "project module contains duplicate script path `{path}`"
                )
            }
            Self::DuplicateResourceUid(uid) => {
                write!(
                    formatter,
                    "project module contains duplicate script Resource UID `{uid}`"
                )
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct ModuleGeneration {
    inner: Arc<GenerationInner>,
}

struct GenerationInner {
    shutdown: godot_api::abi::AbiModuleShutdownFn,
    context: usize,
    host: usize,
    host_context: usize,
    drop_value: AbiDropValueFn,
    poll_tasks: AbiPollTasksFn,
    cancel_tasks: AbiCancelTasksFn,
    scripts: Vec<LoadedScript>,
    _library: Library,
}

struct PendingGeneration {
    library: Option<Library>,
    host: Option<Box<HostApiV1>>,
    host_context: Option<Box<crate::engine_call::EngineCallContext>>,
    api: Option<ModuleApiV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModuleGodotApi {
    major: u32,
    minor: u32,
}

#[derive(Debug)]
struct ModuleExtensions {
    drop_value: AbiDropValueFn,
    godot_api: ModuleGodotApi,
    poll_tasks: AbiPollTasksFn,
    cancel_tasks: AbiCancelTasksFn,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedField {
    name: String,
    rust_type: String,
    kind: AbiFieldKind,
    options: String,
    default_value: Option<String>,
    reload: AbiReloadPolicy,
    reload_value_type: Option<AbiValueType>,
    property: Option<LoadedPropertySchema>,
    node: Option<LoadedNodeSchema>,
    signal: Option<LoadedSignalSchema>,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedPropertySchema {
    type_: AbiPropertyType,
    value_type: AbiValueType,
    hint: u32,
    hint_string: String,
    typed_array_element: Option<String>,
    usage: u32,
    group: Option<String>,
    default_value: Option<HostValue>,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedSignalSchema {
    arguments: Vec<LoadedSignalArgument>,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedNodeSchema {
    path: String,
    class_name: String,
    optional: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedSignalArgument {
    name: String,
    type_: AbiValueType,
}

#[derive(Clone, Debug)]
struct LoadedMethod {
    id: u64,
    name: String,
    rust_signature: String,
    kind: AbiMethodKind,
    lifecycle: AbiLifecycleSlot,
    receiver: AbiReceiverKind,
    argument_count: u16,
    argument_types: Vec<AbiValueType>,
    arguments: Vec<LoadedMethodArgument>,
    default_arguments: Vec<AbiMethodDefaultFn>,
    vararg: bool,
    return_type: AbiValueType,
    return_class: Option<String>,
    options: String,
    rpc: Option<AbiRpcConfigV1>,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedMethodArgument {
    name: String,
    type_: AbiValueType,
    class_name: Option<String>,
}

#[derive(Debug)]
struct LoadedScript {
    resource_uid: i64,
    source_path: String,
    name: String,
    global_name: Option<String>,
    base_script: Option<String>,
    base: String,
    tool: bool,
    fields: Vec<LoadedField>,
    methods: Vec<LoadedMethod>,
    create_state: unsafe extern "C" fn(*mut *mut c_void) -> AbiCallResult,
    drop_state: unsafe extern "C" fn(*mut c_void),
    lifecycle: AbiLifecycleTableV1,
    call_method: AbiCallScriptMethodFn,
    get_field_value: AbiGetScriptFieldFn,
    set_field_value: AbiSetScriptFieldFn,
}

#[derive(Clone)]
pub(crate) struct ModuleScript {
    generation: ModuleGeneration,
    index: usize,
}

pub(crate) struct ModuleState {
    script: ModuleScript,
    states: Vec<ModuleScriptState>,
    super_dispatch: Box<SuperDispatchContext>,
}

struct ModuleScriptState {
    script: ModuleScript,
    state: *mut c_void,
}

struct SuperDispatchContext {
    states: Box<[SuperDispatchState]>,
}

struct SuperDispatchState {
    script: ModuleScript,
    state: *mut c_void,
}

#[derive(Clone)]
pub(crate) struct ModuleMethod {
    script: ModuleScript,
    index: usize,
}

#[derive(Clone)]
pub(crate) struct ModuleField {
    script: ModuleScript,
    index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModuleCallError {
    pub(crate) status: AbiStatus,
    pub(crate) message: String,
}

static ACTIVE_GENERATION: OnceLock<RwLock<Option<ModuleGeneration>>> = OnceLock::new();

thread_local! {
    static CURRENT_CALLBACK_SCRIPT_UID: Cell<i64> =
        const { Cell::new(crate::resource_uid::INVALID_RESOURCE_UID) };
    static ACTIVE_SUPER_DISPATCH: Cell<*const SuperDispatchContext> =
        const { Cell::new(ptr::null()) };
}

struct CallbackScriptScope(i64);

struct ActiveSuperDispatchScope(*const SuperDispatchContext);

impl CallbackScriptScope {
    fn enter(resource_uid: i64) -> Self {
        Self(CURRENT_CALLBACK_SCRIPT_UID.with(|current| current.replace(resource_uid)))
    }
}

impl Drop for CallbackScriptScope {
    fn drop(&mut self) {
        CURRENT_CALLBACK_SCRIPT_UID.with(|current| current.set(self.0));
    }
}

impl ActiveSuperDispatchScope {
    fn enter(context: &SuperDispatchContext) -> Self {
        Self(ACTIVE_SUPER_DISPATCH.with(|current| current.replace(ptr::from_ref(context))))
    }
}

impl Drop for ActiveSuperDispatchScope {
    fn drop(&mut self) {
        ACTIVE_SUPER_DISPATCH.with(|current| current.set(self.0));
    }
}

pub(crate) fn current_callback_script_uid() -> i64 {
    CURRENT_CALLBACK_SCRIPT_UID.with(Cell::get)
}

unsafe extern "C" fn call_super_method_from_module(
    _context: *mut c_void,
    method: AbiByteSlice,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    if output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "base method output pointer is null",
        );
    }
    if method.len == 0
        || method.len > MAX_ABI_TEXT_BYTES
        || method.ptr.is_null()
        || (argument_count != 0 && arguments.is_null())
    {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "base method call has invalid input buffers",
        );
    }
    // SAFETY: The SDK retains the method bytes for this synchronous callback.
    let method_bytes = unsafe { core::slice::from_raw_parts(method.ptr, method.len) };
    let Ok(method_name) = core::str::from_utf8(method_bytes) else {
        return AbiCallResult::failure(AbiStatus::InvalidArgument, "base method name is not UTF-8");
    };
    if method_name.as_bytes().contains(&0) {
        return AbiCallResult::failure(AbiStatus::InvalidArgument, "base method name contains NUL");
    }
    let dispatch = ACTIVE_SUPER_DISPATCH.with(Cell::get);
    let current_uid = current_callback_script_uid();
    if dispatch.is_null() || current_uid == crate::resource_uid::INVALID_RESOURCE_UID {
        return AbiCallResult::failure(
            AbiStatus::Unsupported,
            "base methods can only be called during a Rust script callback",
        );
    }
    let arguments = if argument_count == 0 {
        &[]
    } else {
        // SAFETY: Null was rejected and the SDK retains this bounded slice for
        // the complete synchronous callback.
        unsafe { core::slice::from_raw_parts(arguments, argument_count as usize) }
    };
    // SAFETY: ActiveSuperDispatchScope retains this immutable dispatch table
    // for the complete callback. Its project state pointers belong to
    // distinct script states held by the locked ModuleState.
    let dispatch = unsafe { &*dispatch };
    match dispatch.call_super_method(current_uid, method_name, arguments) {
        Ok(value) => {
            // SAFETY: Null was rejected and ownership of the module-produced
            // value transfers synchronously back to the project SDK.
            unsafe { output.write(value) };
            AbiCallResult::OK
        }
        Err(error) => {
            host_eprintln!(
                "godot-rust base method `{method_name}` failed: {}",
                error.message
            );
            AbiCallResult::failure(error.status, "base Rust script method call failed")
        }
    }
}

impl ModuleGeneration {
    pub(crate) fn engine_call_context(&self) -> &crate::engine_call::EngineCallContext {
        // SAFETY: GenerationInner owns this Box for at least as long as every
        // ModuleGeneration clone, and shutdown clears it only in final Drop.
        unsafe { &*(self.inner.host_context as *const crate::engine_call::EngineCallContext) }
    }

    /// Loads and validates one immutable project-module generation.
    ///
    /// # Safety
    ///
    /// Loading a native library runs platform loader initialization code. The
    /// caller must only supply a trusted project build artifact.
    pub(crate) unsafe fn load(path: &Path) -> Result<Self, ModuleLoadError> {
        // SAFETY: The caller accepted native project module execution.
        unsafe { Self::load_inner(path, None) }
    }

    /// Loads a generation and rejects modules generated for a newer Godot API.
    ///
    /// # Safety
    ///
    /// Loading a native library runs platform loader initialization code. The
    /// caller must only supply a trusted project build artifact.
    pub(crate) unsafe fn load_for_engine(
        path: &Path,
        engine: EngineVersion,
    ) -> Result<Self, ModuleLoadError> {
        // SAFETY: The caller accepted native project module execution.
        unsafe { Self::load_inner(path, Some(engine)) }
    }

    unsafe fn load_inner(
        path: &Path,
        engine: Option<EngineVersion>,
    ) -> Result<Self, ModuleLoadError> {
        // SAFETY: The caller accepted native project module execution.
        let library = unsafe { Library::new(path) }
            .map_err(|error| ModuleLoadError::Open(error.to_string()))?;
        // SAFETY: The newly loaded library is retained by the generation and
        // was accepted by the caller as trusted project code.
        unsafe { Self::load_library(library, engine) }
    }

    #[cfg(target_os = "emscripten")]
    pub(crate) unsafe fn load_exported_for_engine(
        engine: EngineVersion,
    ) -> Result<Self, ModuleLoadError> {
        let library = Library::from(libloading::os::unix::Library::this());
        // SAFETY: Godot's Web loader authenticates and preloads every exported
        // side module before it initializes the Script Host. The process-wide
        // symbol table is retained for the full Web application lifetime.
        unsafe { Self::load_library(library, Some(engine)) }
    }

    unsafe fn load_library(
        library: Library,
        engine: Option<EngineVersion>,
    ) -> Result<Self, ModuleLoadError> {
        // SAFETY: The symbol name and function type are fixed by the shared project ABI.
        let entry = unsafe {
            library
                .get::<godot_api::abi::AbiModuleEntryFn>(MODULE_ENTRY)
                .map_err(|error| ModuleLoadError::MissingEntry(error.to_string()))?
        };
        let entry = *entry;

        let mut reserved = [0; 16];
        reserved[HOST_API_SLOT_EMIT_SIGNAL] =
            crate::script_instance::emit_signal_from_module as *const () as usize;
        reserved[HOST_API_SLOT_CURRENT_OWNER] =
            crate::script_instance::current_owner_from_module as *const () as usize;
        reserved[HOST_API_SLOT_CALL_GODOT_METHOD] =
            crate::engine_call::call_godot_method_from_module as *const () as usize;
        reserved[HOST_API_SLOT_DROP_VALUE] =
            crate::engine_call::drop_host_value_from_module as *const () as usize;
        reserved[HOST_API_SLOT_RETAIN_DYNAMIC_VALUE] =
            crate::engine_call::retain_dynamic_value_from_module as *const () as usize;
        reserved[HOST_API_SLOT_RETAIN_CALLABLE_VALUE] =
            crate::engine_call::retain_callable_value_from_module as *const () as usize;
        reserved[godot_api::abi::HOST_API_SLOT_CALL_GODOT_API] =
            crate::engine_call::call_godot_api_from_module as *const () as usize;
        reserved[HOST_API_SLOT_WATCH_SIGNAL] =
            crate::signal_wait::watch_signal_from_module as *const () as usize;
        reserved[HOST_API_SLOT_POLL_SIGNAL] =
            crate::signal_wait::poll_signal_from_module as *const () as usize;
        reserved[HOST_API_SLOT_CANCEL_SIGNAL] =
            crate::signal_wait::cancel_signal_from_module as *const () as usize;
        reserved[HOST_API_SLOT_CALL_SUPER_METHOD] =
            call_super_method_from_module as *const () as usize;
        let host_context = Box::new(crate::engine_call::EngineCallContext::new());
        let host = Box::new(HostApiV1 {
            header: godot_api::abi::AbiHeader::new(HostApiV1::MINIMUM_SIZE),
            context: core::ptr::from_ref(&*host_context).cast_mut().cast(),
            log: Some(host_log),
            reserved,
        });
        let mut output = MaybeUninit::<ModuleApiV1>::zeroed();
        // SAFETY: Both tables are live for this entry call and the module came
        // from the trusted path accepted above.
        let status = unsafe { entry(&*host, output.as_mut_ptr()) };
        if status != AbiStatus::Ok {
            return Err(ModuleLoadError::EntryRejected(status));
        }
        // SAFETY: ABI success requires the module to initialize the table.
        let api = unsafe { output.assume_init() };
        let mut pending = PendingGeneration {
            library: Some(library),
            host: Some(host),
            host_context: Some(host_context),
            api: Some(api),
        };
        let api = pending.api.as_ref().expect("pending API");
        if !api
            .header
            .is_compatible(ModuleApiV1::MINIMUM_SIZE, ABI_MINOR)
        {
            return Err(ModuleLoadError::IncompatibleModule);
        }
        let get_script = api.get_script.ok_or(ModuleLoadError::MissingScriptGetter)?;
        let extensions = validate_module_extensions(api)?;
        if let Some(engine) = engine {
            ensure_module_godot_api(extensions.godot_api, engine)?;
        }
        if api.script_count > MAX_SCRIPT_COUNT {
            return Err(ModuleLoadError::TooManyScripts(api.script_count));
        }
        let scripts = load_scripts(Some(get_script), api.script_count)?;
        validate_script_inheritance(&scripts)?;

        let api = pending.api.take().expect("validated API");
        let host = pending.host.take().expect("validated Host table");
        let host_context = pending
            .host_context
            .take()
            .expect("validated Host callback context");
        let library = pending.library.take().expect("validated library");
        let generation = Self {
            inner: Arc::new(GenerationInner {
                shutdown: api.shutdown,
                context: api.context as usize,
                host: Box::into_raw(host) as usize,
                host_context: Box::into_raw(host_context) as usize,
                drop_value: extensions.drop_value,
                poll_tasks: extensions.poll_tasks,
                cancel_tasks: extensions.cancel_tasks,
                scripts,
                _library: library,
            }),
        };
        generation.validate_method_defaults()?;
        Ok(generation)
    }

    pub(crate) fn script(&self, source_path: &str) -> Option<ModuleScript> {
        self.inner
            .scripts
            .iter()
            .position(|script| script.source_path == source_path)
            .map(|index| ModuleScript {
                generation: self.clone(),
                index,
            })
    }

    pub(crate) fn script_by_uid(&self, resource_uid: i64) -> Option<ModuleScript> {
        self.inner
            .scripts
            .iter()
            .position(|script| script.resource_uid == resource_uid)
            .map(|index| ModuleScript {
                generation: self.clone(),
                index,
            })
    }

    pub(crate) fn script_count(&self) -> usize {
        self.inner.scripts.len()
    }

    pub(crate) fn value_owner(&self) -> crate::module_value::ModuleValueOwner {
        crate::module_value::ModuleValueOwner::new(self.clone(), self.inner.drop_value)
    }

    pub(crate) fn poll_tasks(&self) -> Result<(), ModuleCallError> {
        let Some(callback) = self.inner.poll_tasks else {
            return Ok(());
        };
        // SAFETY: The callback belongs to this retained generation and the
        // ScriptLanguage frame callback runs it on the Godot main thread.
        status_result(unsafe { callback() }, "project task polling failed")
    }

    pub(crate) fn cancel_tasks(&self) -> Result<(), ModuleCallError> {
        let Some(callback) = self.inner.cancel_tasks else {
            return Ok(());
        };
        // SAFETY: The callback belongs to this retained generation and is
        // invoked once before it becomes inactive.
        status_result(unsafe { callback() }, "project task cancellation failed")
    }

    fn validate_method_defaults(&self) -> Result<(), ModuleLoadError> {
        for (script_index, script) in self.inner.scripts.iter().enumerate() {
            let script_handle = ModuleScript {
                generation: self.clone(),
                index: script_index,
            };
            for method_index in 0..script.methods.len() {
                let method = ModuleMethod {
                    script: script_handle.clone(),
                    index: method_index,
                };
                method.default_values().map_err(|error| {
                    invalid(
                        u32::try_from(script_index).unwrap_or(u32::MAX),
                        format!(
                            "method `{}` default value failed with {:?}: {}",
                            method.name(),
                            error.status,
                            error.message
                        ),
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Rejects schema changes that would invalidate metadata already copied
    /// into live Godot ScriptInstance objects.
    ///
    /// Code pointers are intentionally excluded: changing them is the purpose
    /// of a generation switch. Metadata and callback availability must remain
    /// identical until the Host can ask Godot to rebuild live instance
    /// metadata transactionally.
    pub(crate) fn ensure_reload_compatible(&self, candidate: &Self) -> Result<(), String> {
        if self.inner.scripts.len() != candidate.inner.scripts.len() {
            return Err(format!(
                "script count changed from {} to {}",
                self.inner.scripts.len(),
                candidate.inner.scripts.len()
            ));
        }
        for current in &self.inner.scripts {
            let Some(next) = candidate
                .inner
                .scripts
                .iter()
                .find(|script| script.resource_uid == current.resource_uid)
            else {
                return Err(format!(
                    "script `{}` (UID {}) is missing",
                    current.source_path, current.resource_uid
                ));
            };
            ensure_script_reload_compatible(current, next)?;
        }
        Ok(())
    }
}

fn status_result(status: AbiStatus, message: &str) -> Result<(), ModuleCallError> {
    if status == AbiStatus::Ok {
        Ok(())
    } else {
        Err(ModuleCallError {
            status,
            message: message.to_owned(),
        })
    }
}

fn ensure_script_reload_compatible(
    current: &LoadedScript,
    candidate: &LoadedScript,
) -> Result<(), String> {
    if (
        &current.source_path,
        &current.name,
        &current.global_name,
        &current.base_script,
        &current.base,
        current.tool,
    ) != (
        &candidate.source_path,
        &candidate.name,
        &candidate.global_name,
        &candidate.base_script,
        &candidate.base,
        candidate.tool,
    ) {
        return Err(format!(
            "script `{}` changed its path, type, base class, or tool status",
            current.source_path
        ));
    }
    if current.fields != candidate.fields {
        return Err(format!(
            "script `{}` changed its field, property, node, or signal schema",
            current.source_path
        ));
    }
    if current.methods.len() != candidate.methods.len()
        || current
            .methods
            .iter()
            .zip(&candidate.methods)
            .any(|(left, right)| !method_schema_matches(left, right))
    {
        return Err(format!(
            "script `{}` changed its reflected method schema",
            current.source_path
        ));
    }
    if lifecycle_presence(&current.lifecycle) != lifecycle_presence(&candidate.lifecycle) {
        return Err(format!(
            "script `{}` changed its lifecycle callback schema",
            current.source_path
        ));
    }
    Ok(())
}

fn method_schema_matches(left: &LoadedMethod, right: &LoadedMethod) -> bool {
    left.id == right.id
        && left.name == right.name
        && left.rust_signature == right.rust_signature
        && left.kind == right.kind
        && left.lifecycle == right.lifecycle
        && left.receiver == right.receiver
        && left.argument_count == right.argument_count
        && left.argument_types == right.argument_types
        && left.arguments == right.arguments
        && left.default_arguments.len() == right.default_arguments.len()
        && left.vararg == right.vararg
        && left.return_type == right.return_type
        && left.return_class == right.return_class
        && left.options == right.options
        && left.rpc == right.rpc
}

fn lifecycle_presence(table: &AbiLifecycleTableV1) -> [bool; 7] {
    [
        table.enter_tree.is_some(),
        table.ready.is_some(),
        table.process.is_some(),
        table.physics_process.is_some(),
        table.input.is_some(),
        table.unhandled_input.is_some(),
        table.exit_tree.is_some(),
    ]
}

impl Drop for GenerationInner {
    fn drop(&mut self) {
        crate::signal_wait::cancel_context(self.host_context);
        if let Some(shutdown) = self.shutdown {
            // SAFETY: Context and callback were returned by this still-loaded
            // module generation.
            let _ = unsafe { shutdown(self.context as *mut c_void) };
        }
        if self.host != 0 {
            // SAFETY: `host` came from `Box::into_raw` when this generation was
            // finalized, after the module stopped retaining its address.
            unsafe { drop(Box::from_raw(self.host as *mut HostApiV1)) };
            self.host = 0;
        }
        if self.host_context != 0 {
            // SAFETY: `host_context` came from `Box::into_raw` beside the Host
            // table and all module state has already released its owned refs.
            unsafe {
                drop(Box::from_raw(
                    self.host_context as *mut crate::engine_call::EngineCallContext,
                ));
            }
            self.host_context = 0;
        }
    }
}

impl Drop for PendingGeneration {
    fn drop(&mut self) {
        if self.library.is_some() {
            if let Some(api) = &self.api {
                if let Some(shutdown) = api.shutdown {
                    // SAFETY: Pending validation still retains the loaded
                    // library.
                    let _ = unsafe { shutdown(api.context) };
                }
            }
        }
    }
}

fn ensure_module_godot_api(
    module: ModuleGodotApi,
    engine: EngineVersion,
) -> Result<(), ModuleLoadError> {
    if engine.major == module.major && engine.minor >= module.minor {
        Ok(())
    } else {
        Err(ModuleLoadError::IncompatibleGodotApi { module, engine })
    }
}

fn validate_module_extensions(api: &ModuleApiV1) -> Result<ModuleExtensions, ModuleLoadError> {
    let known_extensions = ABI_MODULE_EXTENSION_OWNED_VALUES
        | ABI_MODULE_EXTENSION_GODOT_API
        | ABI_MODULE_EXTENSION_TASKS;
    if api.reserved_flags & !known_extensions != 0 {
        return Err(ModuleLoadError::InvalidModuleTable(
            "unknown extension flags are set".into(),
        ));
    }
    if api.reserved_flags & ABI_MODULE_EXTENSION_GODOT_API == 0 {
        return Err(ModuleLoadError::InvalidModuleTable(
            "module does not declare its minimum Godot API".into(),
        ));
    }
    let major = u32::try_from(api.reserved[MODULE_API_SLOT_GODOT_API_MAJOR]).map_err(|_| {
        ModuleLoadError::InvalidModuleTable("Godot API major does not fit in u32".into())
    })?;
    let minor = u32::try_from(api.reserved[MODULE_API_SLOT_GODOT_API_MINOR]).map_err(|_| {
        ModuleLoadError::InvalidModuleTable("Godot API minor does not fit in u32".into())
    })?;
    if major != 4 || !(4..=7).contains(&minor) {
        return Err(ModuleLoadError::InvalidModuleTable(format!(
            "unsupported module Godot API target {major}.{minor}"
        )));
    }

    let drop_value = if api.reserved_flags & ABI_MODULE_EXTENSION_OWNED_VALUES == 0 {
        if api.reserved[MODULE_API_SLOT_DROP_VALUE] != 0 {
            return Err(ModuleLoadError::InvalidModuleTable(
                "release callback is populated without the owned-values capability".into(),
            ));
        }
        None
    } else {
        if api.reserved[MODULE_API_SLOT_DROP_VALUE] == 0 {
            return Err(ModuleLoadError::InvalidModuleTable(
                "owned values require a release callback".into(),
            ));
        }
        // SAFETY: The advertised ABI extension defines this non-zero slot as
        // `AbiDropValueFn`.
        unsafe {
            core::mem::transmute::<usize, AbiDropValueFn>(api.reserved[MODULE_API_SLOT_DROP_VALUE])
        }
    };
    let (poll_tasks, cancel_tasks) = if api.reserved_flags & ABI_MODULE_EXTENSION_TASKS == 0 {
        if api.reserved[MODULE_API_SLOT_POLL_TASKS] != 0
            || api.reserved[MODULE_API_SLOT_CANCEL_TASKS] != 0
        {
            return Err(ModuleLoadError::InvalidModuleTable(
                "task callbacks are populated without the task capability".into(),
            ));
        }
        (None, None)
    } else {
        if api.reserved[MODULE_API_SLOT_POLL_TASKS] == 0
            || api.reserved[MODULE_API_SLOT_CANCEL_TASKS] == 0
        {
            return Err(ModuleLoadError::InvalidModuleTable(
                "task capability requires poll and cancellation callbacks".into(),
            ));
        }
        // SAFETY: The advertised ABI extension defines these non-zero slots
        // as task callbacks with fixed C signatures.
        unsafe {
            (
                core::mem::transmute::<usize, AbiPollTasksFn>(
                    api.reserved[MODULE_API_SLOT_POLL_TASKS],
                ),
                core::mem::transmute::<usize, AbiCancelTasksFn>(
                    api.reserved[MODULE_API_SLOT_CANCEL_TASKS],
                ),
            )
        }
    };
    if api.reserved.iter().enumerate().any(|(index, value)| {
        !matches!(
            index,
            MODULE_API_SLOT_DROP_VALUE
                | MODULE_API_SLOT_GODOT_API_MAJOR
                | MODULE_API_SLOT_GODOT_API_MINOR
                | MODULE_API_SLOT_POLL_TASKS
                | MODULE_API_SLOT_CANCEL_TASKS
        ) && *value != 0
    }) {
        return Err(ModuleLoadError::InvalidModuleTable(
            "unknown extension slots are populated".into(),
        ));
    }
    Ok(ModuleExtensions {
        drop_value,
        godot_api: ModuleGodotApi { major, minor },
        poll_tasks,
        cancel_tasks,
    })
}

impl ModuleScript {
    pub(crate) fn name(&self) -> &str {
        &self.descriptor().name
    }

    pub(crate) fn source_path(&self) -> &str {
        &self.descriptor().source_path
    }

    pub(crate) fn resource_uid(&self) -> i64 {
        self.descriptor().resource_uid
    }

    pub(crate) fn base(&self) -> &str {
        &self.descriptor().base
    }

    pub(crate) fn global_name(&self) -> Option<&str> {
        self.descriptor().global_name.as_deref()
    }

    pub(crate) fn base_script_path(&self) -> Option<&str> {
        self.descriptor().base_script.as_deref()
    }

    pub(crate) fn base_script(&self) -> Option<Self> {
        self.base_script_path()
            .and_then(|path| self.generation.script(path))
    }

    pub(crate) fn inherits(&self, candidate: &ModuleScript) -> bool {
        let mut current = self.base_script();
        while let Some(script) = current {
            if script.resource_uid() == candidate.resource_uid() {
                return true;
            }
            current = script.base_script();
        }
        false
    }

    fn hierarchy(&self) -> Vec<Self> {
        let mut scripts = Vec::new();
        let mut current = Some(self.clone());
        while let Some(script) = current {
            current = script.base_script();
            scripts.push(script);
        }
        scripts
    }

    pub(crate) fn is_tool(&self) -> bool {
        self.descriptor().tool
    }

    pub(crate) fn has_enter_tree(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.enter_tree.is_some())
    }

    pub(crate) fn has_ready(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.ready.is_some())
    }

    pub(crate) fn has_node_fields(&self) -> bool {
        self.hierarchy().iter().any(|script| {
            script
                .descriptor()
                .fields
                .iter()
                .any(|field| field.node.is_some())
        })
    }

    pub(crate) fn has_exit_tree(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.exit_tree.is_some())
    }

    pub(crate) fn has_process(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.process.is_some())
    }

    pub(crate) fn has_physics_process(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.physics_process.is_some())
    }

    pub(crate) fn has_input(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.input.is_some())
    }

    pub(crate) fn has_unhandled_input(&self) -> bool {
        self.hierarchy()
            .iter()
            .any(|script| script.descriptor().lifecycle.unhandled_input.is_some())
    }

    pub(crate) fn method_count(&self) -> usize {
        self.inherited_methods().len()
    }

    pub(crate) fn field_count(&self) -> usize {
        self.hierarchy()
            .iter()
            .map(|script| script.descriptor().fields.len())
            .sum()
    }

    pub(crate) fn classes_used(&self) -> Vec<String> {
        let mut classes = Vec::new();
        let mut push = |name: &str| {
            if !name.is_empty() && !classes.iter().any(|existing| existing == name) {
                classes.push(name.to_owned());
            }
        };
        push(self.base());
        for index in 0..self.field_count() {
            let Some(field) = self.field(index) else {
                continue;
            };
            if let Some(class) = field.property_object_class() {
                push(class);
            }
            if let Some(class) = field.node_class_name() {
                push(class);
            }
            if let Some(class) = field.typed_array_element() {
                push(class);
            }
        }
        for index in 0..self.method_count() {
            let Some(method) = self.method(index) else {
                continue;
            };
            for argument_index in 0..method.argument_types().len() {
                if let Some(class) = method.argument_class_name(argument_index) {
                    push(class);
                }
            }
            if let Some(class) = method.return_class_name() {
                push(class);
            }
        }
        classes
    }

    pub(crate) fn field(&self, index: usize) -> Option<ModuleField> {
        let mut remaining = index;
        for script in self.hierarchy() {
            if remaining < script.descriptor().fields.len() {
                return Some(ModuleField {
                    script,
                    index: remaining,
                });
            }
            remaining = remaining.saturating_sub(script.descriptor().fields.len());
        }
        None
    }

    pub(crate) fn method(&self, index: usize) -> Option<ModuleMethod> {
        self.inherited_methods().get(index).cloned()
    }

    pub(crate) fn create_state(&self) -> Result<ModuleState, ModuleCallError> {
        let mut states = Vec::new();
        for script in self.hierarchy() {
            let mut state = ptr::null_mut();
            // SAFETY: Callback belongs to this retained module generation and
            // the output slot is writable.
            let result = unsafe { (script.descriptor().create_state)(&mut state) };
            call_result(result)?;
            if state.is_null() {
                return Err(ModuleCallError {
                    status: AbiStatus::Internal,
                    message: "project module returned null script state".into(),
                });
            }
            states.push(ModuleScriptState { script, state });
        }
        let super_dispatch = Box::new(SuperDispatchContext {
            states: states
                .iter()
                .map(|state| SuperDispatchState {
                    script: state.script.clone(),
                    state: state.state,
                })
                .collect(),
        });
        Ok(ModuleState {
            script: self.clone(),
            states,
            super_dispatch,
        })
    }

    fn inherited_methods(&self) -> Vec<ModuleMethod> {
        let mut methods = Vec::new();
        let mut names = HashSet::new();
        for script in self.hierarchy() {
            for (index, method) in script.descriptor().methods.iter().enumerate() {
                if names.insert(method.name.clone()) {
                    methods.push(ModuleMethod {
                        script: script.clone(),
                        index,
                    });
                }
            }
        }
        methods
    }

    fn descriptor(&self) -> &LoadedScript {
        &self.generation.inner.scripts[self.index]
    }
}

impl ModuleState {
    pub(crate) fn source_path(&self) -> &str {
        &self.script.descriptor().source_path
    }

    pub(crate) fn value_owner(&self) -> crate::module_value::ModuleValueOwner {
        self.script.generation.value_owner()
    }

    pub(crate) fn resource_uid(&self) -> i64 {
        self.script.resource_uid()
    }

    pub(crate) fn method_count(&self) -> usize {
        self.script.method_count()
    }

    pub(crate) fn method(&self, index: usize) -> Option<ModuleMethod> {
        self.script.method(index)
    }

    pub(crate) fn field_count(&self) -> usize {
        self.script.field_count()
    }

    pub(crate) fn field(&self, index: usize) -> Option<ModuleField> {
        self.script.field(index)
    }

    pub(crate) fn get_field(&self, field: &ModuleField) -> Result<ModuleValue, ModuleCallError> {
        let state = self.validate_field_handle(field)?;
        let callback = self.states[state]
            .script
            .descriptor()
            .get_field_value
            .ok_or_else(|| ModuleCallError {
                status: AbiStatus::Unsupported,
                message: "project module has no generated field getter".into(),
            })?;
        let field_index = u32::try_from(field.index).map_err(|_| ModuleCallError {
            status: AbiStatus::InvalidArgument,
            message: "field index exceeds the project ABI".into(),
        })?;
        let mut output = AbiValueV1::NIL;
        let _scope = CallbackScriptScope::enter(self.states[state].script.resource_uid());
        // SAFETY: State and field belong to the retained generation and output
        // is a writable fixed-layout value.
        call_result(unsafe { callback(self.states[state].state, field_index, &mut output) })?;
        let expected = field.value_type().ok_or_else(|| ModuleCallError {
            status: AbiStatus::Unsupported,
            message: "field has no runtime value transport".into(),
        })?;
        self.states[state]
            .script
            .generation
            .value_owner()
            .output(expected, output)
    }

    pub(crate) fn set_field(
        &mut self,
        field: &ModuleField,
        value: AbiValueV1,
    ) -> Result<(), ModuleCallError> {
        let state = self.validate_field_handle(field)?;
        let expected = field.value_type().ok_or_else(|| ModuleCallError {
            status: AbiStatus::Unsupported,
            message: "field has no runtime value transport".into(),
        })?;
        module_value::validate_input(expected, value)?;
        let callback = self.states[state]
            .script
            .descriptor()
            .set_field_value
            .ok_or_else(|| ModuleCallError {
                status: AbiStatus::Unsupported,
                message: "project module has no generated field setter".into(),
            })?;
        let field_index = u32::try_from(field.index).map_err(|_| ModuleCallError {
            status: AbiStatus::InvalidArgument,
            message: "field index exceeds the project ABI".into(),
        })?;
        let _scope = CallbackScriptScope::enter(self.states[state].script.resource_uid());
        // SAFETY: State and field belong to the retained generation and the
        // value was validated against the field schema.
        call_result(unsafe { callback(self.states[state].state, field_index, value) })
    }

    fn validate_field_handle(&self, field: &ModuleField) -> Result<usize, ModuleCallError> {
        let state = self
            .states
            .iter()
            .position(|state| state.script.resource_uid() == field.script.resource_uid());
        let current = state.and_then(|state| {
            (field.index < self.states[state].script.descriptor().fields.len()).then(|| {
                ModuleField {
                    script: self.states[state].script.clone(),
                    index: field.index,
                }
            })
        });
        if current.as_ref().is_none_or(|current| {
            current.name() != field.name() || current.value_type() != field.value_type()
        }) {
            return Err(ModuleCallError {
                status: AbiStatus::StaleHandle,
                message: "field descriptor belongs to another module generation".into(),
            });
        }
        Ok(state.expect("validated field state"))
    }

    pub(crate) fn has_enter_tree(&self) -> bool {
        self.script.has_enter_tree()
    }

    pub(crate) fn has_ready(&self) -> bool {
        self.script.has_ready()
    }

    pub(crate) fn has_node_fields(&self) -> bool {
        self.script.has_node_fields()
    }

    pub(crate) fn has_exit_tree(&self) -> bool {
        self.script.has_exit_tree()
    }

    pub(crate) fn has_process(&self) -> bool {
        self.script.has_process()
    }

    pub(crate) fn has_physics_process(&self) -> bool {
        self.script.has_physics_process()
    }

    pub(crate) fn has_input(&self) -> bool {
        self.script.has_input()
    }

    pub(crate) fn has_unhandled_input(&self) -> bool {
        self.script.has_unhandled_input()
    }

    pub(crate) fn enter_tree(&mut self) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.enter_tree {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state) });
            }
        }
        Ok(())
    }

    pub(crate) fn ready(&mut self) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.ready {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state) });
            }
        }
        Ok(())
    }

    pub(crate) fn process(&mut self, delta: f64) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.process {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state, delta) });
            }
        }
        Ok(())
    }

    pub(crate) fn physics_process(&mut self, delta: f64) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.physics_process {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state, delta) });
            }
        }
        Ok(())
    }

    pub(crate) fn input(&mut self, event: u64) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.input {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state, event) });
            }
        }
        Ok(())
    }

    pub(crate) fn unhandled_input(&mut self, event: u64) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.unhandled_input {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state, event) });
            }
        }
        Ok(())
    }

    pub(crate) fn call_method(
        &mut self,
        method: &ModuleMethod,
        arguments: &[AbiValueV1],
    ) -> Result<ModuleValue, ModuleCallError> {
        let state = self
            .states
            .iter()
            .position(|state| state.script.resource_uid() == method.script.resource_uid())
            .ok_or_else(|| ModuleCallError {
                status: AbiStatus::StaleHandle,
                message: "method descriptor belongs to another script state".into(),
            })?;
        let current =
            (method.index < self.states[state].script.descriptor().methods.len()).then(|| {
                ModuleMethod {
                    script: self.states[state].script.clone(),
                    index: method.index,
                }
            });
        let Some(current) = current else {
            return Err(ModuleCallError {
                status: AbiStatus::StaleHandle,
                message: "method descriptor belongs to another module generation".into(),
            });
        };
        let Some(callback) = current.script.descriptor().call_method else {
            return Err(ModuleCallError {
                status: AbiStatus::Unsupported,
                message: "project module has no reflected method callback".into(),
            });
        };
        if current.id() != method.id()
            || current.name() != method.name()
            || current.argument_types() != method.argument_types()
            || current.return_type() != method.return_type()
        {
            return Err(ModuleCallError {
                status: AbiStatus::StaleHandle,
                message: "method descriptor belongs to another module generation".into(),
            });
        }
        let argument_count = u32::try_from(arguments.len()).map_err(|_| ModuleCallError {
            status: AbiStatus::InvalidArgument,
            message: "method argument count exceeds the ABI limit".into(),
        })?;
        let fixed_count = method.argument_types().len();
        if arguments.len() < fixed_count || (!method.is_vararg() && arguments.len() != fixed_count)
        {
            return Err(ModuleCallError {
                status: AbiStatus::InvalidArgument,
                message: "method argument count does not match its descriptor".into(),
            });
        }
        for (index, value) in arguments.iter().copied().enumerate() {
            let expected = method
                .argument_types()
                .get(index)
                .copied()
                .unwrap_or(AbiValueType::VARIANT);
            module_value::validate_input(expected, value)?;
        }
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        let output = self.super_dispatch.invoke_method(
            state,
            &current,
            arguments,
            callback,
            argument_count,
        )?;
        self.states[state]
            .script
            .generation
            .value_owner()
            .output(method.return_type(), output)
    }

    pub(crate) fn exit_tree(&mut self) -> Result<(), ModuleCallError> {
        let _dispatch_scope = ActiveSuperDispatchScope::enter(&self.super_dispatch);
        for state in &mut self.states {
            if let Some(callback) = state.script.descriptor().lifecycle.exit_tree {
                let _scope = CallbackScriptScope::enter(state.script.resource_uid());
                // SAFETY: State and callback were created by the same retained module.
                return call_result(unsafe { callback(state.state) });
            }
        }
        Ok(())
    }
}

impl SuperDispatchContext {
    fn invoke_method(
        &self,
        state_index: usize,
        method: &ModuleMethod,
        arguments: &[AbiValueV1],
        callback: unsafe extern "C" fn(
            *mut c_void,
            u64,
            *const AbiValueV1,
            u32,
            *mut AbiValueV1,
        ) -> AbiCallResult,
        argument_count: u32,
    ) -> Result<AbiValueV1, ModuleCallError> {
        let state = self.states[state_index].state;
        let script_uid = self.states[state_index].script.resource_uid();
        let mut output = AbiValueV1::NIL;
        let _scope = CallbackScriptScope::enter(script_uid);
        // SAFETY: State, method and callback belong to the same retained
        // generation; input and output storage remains live for the call.
        let result = unsafe {
            callback(
                state,
                method.id(),
                arguments.as_ptr(),
                argument_count,
                &mut output,
            )
        };
        if let Err(error) = call_result(result) {
            // Adopt any well-formed owned output written before failure so a
            // malformed callback cannot leak it across repeated calls.
            let _ = self.states[state_index]
                .script
                .generation
                .value_owner()
                .output(method.return_type(), output);
            return Err(error);
        }
        if let Err(error) = module_value::validate_module_output(method.return_type(), output) {
            // Adopt any recognizable project allocation before returning an
            // invalid-output error so malformed callbacks cannot leak.
            let _ = self.states[state_index]
                .script
                .generation
                .value_owner()
                .output(method.return_type(), output);
            return Err(error);
        }
        Ok(output)
    }

    fn call_super_method(
        &self,
        current_script_uid: i64,
        name: &str,
        arguments: &[AbiValueV1],
    ) -> Result<AbiValueV1, ModuleCallError> {
        let current_index = self
            .states
            .iter()
            .position(|state| state.script.resource_uid() == current_script_uid)
            .ok_or_else(|| ModuleCallError {
                status: AbiStatus::StaleHandle,
                message: "the active script is not part of this instance".into(),
            })?;
        let (state_index, method) = self
            .states
            .iter()
            .enumerate()
            .skip(current_index + 1)
            .find_map(|(state_index, state)| {
                state
                    .script
                    .descriptor()
                    .methods
                    .iter()
                    .position(|method| method.name == name)
                    .map(|index| {
                        (
                            state_index,
                            ModuleMethod {
                                script: state.script.clone(),
                                index,
                            },
                        )
                    })
            })
            .ok_or_else(|| ModuleCallError {
                status: AbiStatus::Unsupported,
                message: format!("no base Rust script implements `{name}`"),
            })?;
        let fixed_count = method.argument_types().len();
        let minimum_count = method.minimum_argument_count();
        if arguments.len() < minimum_count || (!method.is_vararg() && arguments.len() > fixed_count)
        {
            return Err(ModuleCallError {
                status: AbiStatus::InvalidArgument,
                message: "base method argument count does not match its descriptor".into(),
            });
        }
        let default_values = if arguments.len() < fixed_count {
            method.default_values()?
        } else {
            Vec::new()
        };
        let mut call_arguments = Vec::with_capacity(arguments.len().max(fixed_count));
        call_arguments.extend_from_slice(arguments);
        if arguments.len() < fixed_count {
            let supplied_default_count = arguments.len().saturating_sub(minimum_count);
            call_arguments.extend(
                default_values
                    .iter()
                    .skip(supplied_default_count)
                    .map(ModuleValue::borrowed_abi),
            );
        }
        for (index, value) in call_arguments.iter().copied().enumerate() {
            let expected = method
                .argument_types()
                .get(index)
                .copied()
                .unwrap_or(AbiValueType::VARIANT);
            module_value::validate_input(expected, value)?;
        }
        let argument_count = u32::try_from(call_arguments.len()).map_err(|_| ModuleCallError {
            status: AbiStatus::InvalidArgument,
            message: "base method argument count exceeds the ABI limit".into(),
        })?;
        if method.kind() == AbiMethodKind::Lifecycle {
            return self.invoke_lifecycle_method(state_index, &method, &call_arguments);
        }
        let callback = method
            .script
            .descriptor()
            .call_method
            .ok_or_else(|| ModuleCallError {
                status: AbiStatus::Unsupported,
                message: "base project module has no reflected method callback".into(),
            })?;
        self.invoke_method(
            state_index,
            &method,
            &call_arguments,
            callback,
            argument_count,
        )
    }

    fn invoke_lifecycle_method(
        &self,
        state_index: usize,
        method: &ModuleMethod,
        arguments: &[AbiValueV1],
    ) -> Result<AbiValueV1, ModuleCallError> {
        let state = self.states[state_index].state;
        let script = self.states[state_index].script.clone();
        let lifecycle = script.descriptor().lifecycle;
        let _scope = CallbackScriptScope::enter(script.resource_uid());
        let result = match method.descriptor().lifecycle {
            AbiLifecycleSlot::EnterTree => lifecycle.enter_tree.map(|callback| {
                // SAFETY: State and callback belong to the same retained module.
                unsafe { callback(state) }
            }),
            AbiLifecycleSlot::Ready => lifecycle.ready.map(|callback| {
                // SAFETY: State and callback belong to the same retained module.
                unsafe { callback(state) }
            }),
            AbiLifecycleSlot::Process => lifecycle.process.map(|callback| {
                // SAFETY: The descriptor validated the sole f64 input and the
                // state and callback belong to the same retained module.
                unsafe { callback(state, f64::from_bits(arguments[0].payload[0])) }
            }),
            AbiLifecycleSlot::PhysicsProcess => lifecycle.physics_process.map(|callback| {
                // SAFETY: The descriptor validated the sole f64 input and the
                // state and callback belong to the same retained module.
                unsafe { callback(state, f64::from_bits(arguments[0].payload[0])) }
            }),
            AbiLifecycleSlot::Input => lifecycle.input.map(|callback| {
                // SAFETY: The descriptor validated the sole Object ID input and
                // the state and callback belong to the same retained module.
                unsafe { callback(state, arguments[0].payload[0]) }
            }),
            AbiLifecycleSlot::UnhandledInput => lifecycle.unhandled_input.map(|callback| {
                // SAFETY: The descriptor validated the sole Object ID input and
                // the state and callback belong to the same retained module.
                unsafe { callback(state, arguments[0].payload[0]) }
            }),
            AbiLifecycleSlot::ExitTree => lifecycle.exit_tree.map(|callback| {
                // SAFETY: State and callback belong to the same retained module.
                unsafe { callback(state) }
            }),
            AbiLifecycleSlot::None => None,
        }
        .ok_or_else(|| ModuleCallError {
            status: AbiStatus::Internal,
            message: format!(
                "base lifecycle method `{}` has no matching callback",
                method.name()
            ),
        })?;
        call_result(result)?;
        Ok(AbiValueV1::NIL)
    }
}

impl ModuleMethod {
    fn descriptor(&self) -> &LoadedMethod {
        &self.script.descriptor().methods[self.index]
    }

    pub(crate) fn id(&self) -> u64 {
        self.descriptor().id
    }

    pub(crate) fn name(&self) -> &str {
        &self.descriptor().name
    }

    pub(crate) fn rust_signature(&self) -> &str {
        &self.descriptor().rust_signature
    }

    pub(crate) fn kind(&self) -> AbiMethodKind {
        self.descriptor().kind
    }

    pub(crate) fn argument_types(&self) -> &[AbiValueType] {
        &self.descriptor().argument_types
    }

    pub(crate) fn minimum_argument_count(&self) -> usize {
        self.argument_types()
            .len()
            .saturating_sub(self.descriptor().default_arguments.len())
    }

    pub(crate) fn is_vararg(&self) -> bool {
        self.descriptor().vararg
    }

    pub(crate) fn default_values(&self) -> Result<Vec<ModuleValue>, ModuleCallError> {
        let owner = self.script.generation.value_owner();
        let start = self.minimum_argument_count();
        self.descriptor()
            .default_arguments
            .iter()
            .copied()
            .zip(&self.argument_types()[start..])
            .map(|(callback, expected)| {
                let callback = callback.ok_or_else(|| ModuleCallError {
                    status: AbiStatus::Internal,
                    message: "method default callback is unavailable".into(),
                })?;
                let mut output = AbiValueV1::NIL;
                // SAFETY: The callback was copied from this retained module
                // generation and receives one writable output slot.
                let result = unsafe { callback(&mut output) };
                if result.status != AbiStatus::Ok {
                    // A malformed module may write an owned value before
                    // reporting failure. Adopt and immediately drop any valid
                    // output so it cannot leak across repeated metadata reads.
                    let _ = owner.output(*expected, output);
                    return Err(call_result(result).expect_err("non-OK callback result"));
                }
                if *expected == AbiValueType::OBJECT_ID && output.payload[0] != 0 {
                    return Err(ModuleCallError {
                        status: AbiStatus::InvalidArgument,
                        message: "method object defaults must be null".into(),
                    });
                }
                owner.output(*expected, output)
            })
            .collect()
    }

    pub(crate) fn engine_call_context(&self) -> &crate::engine_call::EngineCallContext {
        self.script.generation.engine_call_context()
    }

    pub(crate) fn arguments(&self) -> impl ExactSizeIterator<Item = (&str, AbiValueType)> {
        self.descriptor()
            .arguments
            .iter()
            .map(|argument| (argument.name.as_str(), argument.type_))
    }

    pub(crate) fn argument_class_name(&self, index: usize) -> Option<&str> {
        self.descriptor()
            .arguments
            .get(index)?
            .class_name
            .as_deref()
    }

    pub(crate) fn return_type(&self) -> AbiValueType {
        self.descriptor().return_type
    }

    pub(crate) fn return_class_name(&self) -> Option<&str> {
        self.descriptor().return_class.as_deref()
    }

    pub(crate) fn receiver(&self) -> AbiReceiverKind {
        self.descriptor().receiver
    }

    pub(crate) fn rpc_config(&self) -> Option<AbiRpcConfigV1> {
        self.descriptor().rpc
    }
}

impl ModuleField {
    fn descriptor(&self) -> &LoadedField {
        &self.script.descriptor().fields[self.index]
    }

    pub(crate) fn name(&self) -> &str {
        &self.descriptor().name
    }

    pub(crate) fn index(&self) -> u32 {
        u32::try_from(self.index).expect("validated script field count fits u32")
    }

    pub(crate) fn script_resource_uid(&self) -> i64 {
        self.script.resource_uid()
    }

    pub(crate) fn property_type(&self) -> Option<AbiPropertyType> {
        Some(self.descriptor().property.as_ref()?.type_)
    }

    pub(crate) fn property_hint(&self) -> Option<u32> {
        Some(self.descriptor().property.as_ref()?.hint)
    }

    pub(crate) fn property_hint_string(&self) -> Option<&str> {
        Some(&self.descriptor().property.as_ref()?.hint_string)
    }

    pub(crate) fn typed_array_element(&self) -> Option<&str> {
        self.descriptor()
            .property
            .as_ref()?
            .typed_array_element
            .as_deref()
    }

    pub(crate) fn property_object_class(&self) -> Option<&str> {
        let property = self.descriptor().property.as_ref()?;
        (property.type_ == AbiPropertyType::OBJECT
            && matches!(
                property.hint,
                ABI_PROPERTY_HINT_NODE_TYPE | ABI_PROPERTY_HINT_RESOURCE_TYPE
            ))
        .then_some(property.hint_string.as_str())
    }

    pub(crate) fn owns_property_object(&self) -> bool {
        self.descriptor().property.as_ref().is_some_and(|property| {
            property.type_ == AbiPropertyType::OBJECT
                && property.hint == ABI_PROPERTY_HINT_RESOURCE_TYPE
        })
    }

    pub(crate) fn property_usage(&self) -> Option<u32> {
        Some(self.descriptor().property.as_ref()?.usage)
    }

    pub(crate) fn property_group(&self) -> Option<&str> {
        self.descriptor().property.as_ref()?.group.as_deref()
    }

    pub(crate) fn property_default_value(&self) -> Option<HostValue> {
        self.descriptor().property.as_ref()?.default_value.clone()
    }

    pub(crate) fn value_type(&self) -> Option<AbiValueType> {
        if self.is_node() {
            return Some(AbiValueType::OBJECT_ID);
        }
        self.descriptor()
            .property
            .as_ref()
            .map(|property| property.value_type)
            .or(self.descriptor().reload_value_type)
    }

    pub(crate) fn reload_policy(&self) -> AbiReloadPolicy {
        self.descriptor().reload
    }

    pub(crate) fn is_node(&self) -> bool {
        self.descriptor().node.is_some()
    }

    pub(crate) fn node_path(&self) -> Option<&str> {
        Some(&self.descriptor().node.as_ref()?.path)
    }

    pub(crate) fn node_class_name(&self) -> Option<&str> {
        Some(&self.descriptor().node.as_ref()?.class_name)
    }

    pub(crate) fn node_optional(&self) -> Option<bool> {
        Some(self.descriptor().node.as_ref()?.optional)
    }

    pub(crate) fn is_signal(&self) -> bool {
        self.descriptor().signal.is_some()
    }

    pub(crate) fn signal_arguments(&self) -> impl Iterator<Item = (&str, AbiValueType)> {
        self.descriptor()
            .signal
            .as_ref()
            .into_iter()
            .flat_map(|signal| &signal.arguments)
            .map(|argument| (argument.name.as_str(), argument.type_))
    }
}

impl LoadedScript {
    fn validate_owned_schema(&self) {
        debug_assert!(self.resource_uid >= 0);
        debug_assert!(!self.name.is_empty());
        debug_assert!(!self.base.is_empty());
        debug_assert!(is_godot_class_name(&self.base));
        debug_assert!(self.fields.iter().all(|field| {
            let _ = (field.kind, field.reload);
            !field.name.is_empty()
                && !field.rust_type.is_empty()
                && field.options.len() <= MAX_ABI_TEXT_BYTES
                && field
                    .default_value
                    .as_ref()
                    .is_none_or(|value| value.len() <= MAX_ABI_TEXT_BYTES)
                && field.node.as_ref().is_none_or(|node| {
                    !node.path.is_empty()
                        && node.path.len() <= MAX_ABI_TEXT_BYTES
                        && is_godot_class_name(&node.class_name)
                })
        }));
        debug_assert!(self.methods.iter().all(|method| {
            method.id != 0
                && !method.name.is_empty()
                && !method.rust_signature.is_empty()
                && method.argument_types.len() == usize::from(method.argument_count)
                && method.arguments.len() == usize::from(method.argument_count)
                && method
                    .arguments
                    .iter()
                    .zip(&method.argument_types)
                    .all(|(argument, type_)| {
                        !argument.name.is_empty()
                            && argument.type_ == *type_
                            && if *type_ == AbiValueType::OBJECT_ID {
                                argument
                                    .class_name
                                    .as_deref()
                                    .is_some_and(is_godot_class_name)
                            } else if *type_ == AbiValueType::ARRAY {
                                argument
                                    .class_name
                                    .as_deref()
                                    .is_none_or(is_typed_array_element_name)
                            } else {
                                argument.class_name.is_none()
                            }
                    })
                && method
                    .argument_types
                    .iter()
                    .all(|value| value.is_supported())
                && method.return_type.is_supported()
                && if method.return_type == AbiValueType::OBJECT_ID {
                    method
                        .return_class
                        .as_deref()
                        .is_some_and(is_godot_class_name)
                } else if method.return_type == AbiValueType::ARRAY {
                    method
                        .return_class
                        .as_deref()
                        .is_none_or(is_typed_array_element_name)
                } else {
                    method.return_class.is_none()
                }
                && method.options.len() <= MAX_ABI_TEXT_BYTES
        }));
    }
}

impl Drop for ModuleScriptState {
    fn drop(&mut self) {
        // SAFETY: This state was created by its paired callback and the
        // retained script handle keeps the generation loaded.
        unsafe { (self.script.descriptor().drop_state)(self.state) };
        self.state = ptr::null_mut();
    }
}

pub(crate) fn set_active_generation(generation: ModuleGeneration) {
    let active = ACTIVE_GENERATION.get_or_init(|| RwLock::new(None));
    if let Ok(mut active) = active.write() {
        *active = Some(generation);
    }
}

pub(crate) fn clear_active_generation() {
    let active = ACTIVE_GENERATION.get_or_init(|| RwLock::new(None));
    if let Ok(mut active) = active.write() {
        *active = None;
    }
}

pub(crate) fn active_generation() -> Option<ModuleGeneration> {
    ACTIVE_GENERATION
        .get_or_init(|| RwLock::new(None))
        .read()
        .ok()
        .and_then(|generation| generation.clone())
}

pub(crate) fn active_script(source_path: &str) -> Option<ModuleScript> {
    ACTIVE_GENERATION
        .get_or_init(|| RwLock::new(None))
        .read()
        .ok()
        .and_then(|generation| generation.as_ref()?.script(source_path))
}

pub(crate) fn active_script_by_uid(resource_uid: i64) -> Option<ModuleScript> {
    ACTIVE_GENERATION
        .get_or_init(|| RwLock::new(None))
        .read()
        .ok()
        .and_then(|generation| generation.as_ref()?.script_by_uid(resource_uid))
}

fn load_scripts(
    get_script: AbiGetScriptDescriptorFn,
    count: u32,
) -> Result<Vec<LoadedScript>, ModuleLoadError> {
    let get_script = get_script.ok_or(ModuleLoadError::MissingScriptGetter)?;
    let mut scripts = Vec::with_capacity(count as usize);
    let mut paths = HashSet::with_capacity(count as usize);
    let mut resource_uids = HashSet::with_capacity(count as usize);
    let mut global_names = HashSet::new();
    for index in 0..count {
        let mut output = MaybeUninit::<AbiScriptDescriptorV1>::zeroed();
        // SAFETY: Index is within the advertised count and output is writable.
        let status = unsafe { get_script(index, output.as_mut_ptr()) };
        if status != AbiStatus::Ok {
            return Err(invalid(index, format!("getter returned {status:?}")));
        }
        // SAFETY: ABI success requires a fully initialized descriptor.
        let descriptor = unsafe { output.assume_init() };
        let script = copy_script(index, descriptor)?;
        script.validate_owned_schema();
        if !paths.insert(script.source_path.clone()) {
            return Err(ModuleLoadError::DuplicateSourcePath(script.source_path));
        }
        if !resource_uids.insert(script.resource_uid) {
            return Err(ModuleLoadError::DuplicateResourceUid(script.resource_uid));
        }
        if script
            .global_name
            .as_ref()
            .is_some_and(|name| !global_names.insert(name.clone()))
        {
            return Err(invalid(index, "duplicate global Rust class name"));
        }
        scripts.push(script);
    }
    Ok(scripts)
}

fn validate_script_inheritance(scripts: &[LoadedScript]) -> Result<(), ModuleLoadError> {
    let paths = scripts
        .iter()
        .enumerate()
        .map(|(index, script)| (script.source_path.as_str(), index))
        .collect::<HashMap<_, _>>();
    for (index, script) in scripts.iter().enumerate() {
        let mut seen = HashSet::new();
        let mut current = script;
        while let Some(base_path) = current.base_script.as_deref() {
            if !seen.insert(current.source_path.as_str()) {
                return Err(invalid(
                    u32::try_from(index).unwrap_or(u32::MAX),
                    "Rust script inheritance contains a cycle",
                ));
            }
            let Some(base_index) = paths.get(base_path).copied() else {
                return Err(invalid(
                    u32::try_from(index).unwrap_or(u32::MAX),
                    format!("base Rust script `{base_path}` is not part of the project module"),
                ));
            };
            let base = &scripts[base_index];
            if base.base != script.base {
                return Err(invalid(
                    u32::try_from(index).unwrap_or(u32::MAX),
                    format!(
                        "base Rust script `{base_path}` uses Godot base `{}`, expected `{}`",
                        base.base, script.base
                    ),
                ));
            }
            current = base;
        }

        let mut field_names = HashSet::new();
        let mut current = Some(script);
        while let Some(member) = current {
            for field in &member.fields {
                if !field_names.insert(field.name.as_str()) {
                    return Err(invalid(
                        u32::try_from(index).unwrap_or(u32::MAX),
                        format!(
                            "inherited Rust field `{}` is declared more than once",
                            field.name
                        ),
                    ));
                }
            }
            current = member
                .base_script
                .as_deref()
                .and_then(|path| paths.get(path))
                .map(|base| &scripts[*base]);
        }
    }
    Ok(())
}

fn copy_script(
    index: u32,
    descriptor: AbiScriptDescriptorV1,
) -> Result<LoadedScript, ModuleLoadError> {
    if descriptor.struct_size < AbiScriptDescriptorV1::MINIMUM_SIZE {
        return Err(invalid(index, "descriptor is smaller than ABI V1"));
    }
    let known_extensions = ABI_SCRIPT_EXTENSION_FIELD_ACCESS
        | ABI_SCRIPT_EXTENSION_RESOURCE_UID
        | ABI_SCRIPT_EXTENSION_GLOBAL_CLASS
        | ABI_SCRIPT_EXTENSION_BASE_SCRIPT;
    if descriptor.reserved_flags & !known_extensions != 0 || descriptor.reserved_bytes != [0; 7] {
        return Err(invalid(
            index,
            "script descriptor has unknown extension flags",
        ));
    }
    if descriptor.reserved_flags & ABI_SCRIPT_EXTENSION_FIELD_ACCESS == 0
        || descriptor.reserved[0] == 0
        || descriptor.reserved[1] == 0
    {
        return Err(invalid(
            index,
            "script descriptor has no valid field access extension",
        ));
    }
    // SAFETY: The fixed extension slots are emitted from function pointers by
    // the paired SDK in this process. Null and unknown slots were rejected.
    let get_field_value: AbiGetScriptFieldFn =
        unsafe { core::mem::transmute(descriptor.reserved[0]) };
    // SAFETY: See above.
    let set_field_value: AbiSetScriptFieldFn =
        unsafe { core::mem::transmute(descriptor.reserved[1]) };
    if descriptor.reserved_flags & ABI_SCRIPT_EXTENSION_RESOURCE_UID == 0 {
        return Err(invalid(
            index,
            "script descriptor has no valid Resource UID extension",
        ));
    }
    let resource_uid = decode_resource_uid_words([descriptor.reserved[2], descriptor.reserved[3]])
        .ok_or_else(|| invalid(index, "script Resource UID is out of range"))?;
    let global_name = if descriptor.reserved_flags & ABI_SCRIPT_EXTENSION_GLOBAL_CLASS != 0 {
        let name = copy_text(
            index,
            "script.global_name",
            AbiByteSlice {
                ptr: descriptor.reserved[4] as *const u8,
                len: descriptor.reserved[5],
            },
        )?;
        if !is_godot_class_name(&name) {
            return Err(invalid(index, "script global class name is invalid"));
        }
        Some(name)
    } else {
        if descriptor.reserved[4..6] != [0; 2] {
            return Err(invalid(
                index,
                "script global class slots are populated without the extension",
            ));
        }
        None
    };
    let base_script = if descriptor.reserved_flags & ABI_SCRIPT_EXTENSION_BASE_SCRIPT != 0 {
        let path = copy_text(
            index,
            "script.base_script",
            AbiByteSlice {
                ptr: descriptor.reserved[6] as *const u8,
                len: descriptor.reserved[7],
            },
        )?;
        if !is_canonical_rs_path(&path) {
            return Err(invalid(
                index,
                "base Rust script must be a canonical `res://` `.rs` path",
            ));
        }
        Some(path)
    } else {
        if descriptor.reserved[6..] != [0; 2] {
            return Err(invalid(
                index,
                "base Rust script slots are populated without the extension",
            ));
        }
        None
    };
    if descriptor.tool > 1 {
        return Err(invalid(index, "`tool` must be encoded as zero or one"));
    }
    if descriptor.field_count > MAX_MEMBER_COUNT || descriptor.method_count > MAX_MEMBER_COUNT {
        return Err(invalid(index, "field or method count exceeds safety limit"));
    }
    let source_path = copy_text(index, "source_path", descriptor.source_path)?;
    if !is_canonical_rs_path(&source_path) {
        return Err(invalid(index, "source path must be a `res://` `.rs` path"));
    }
    let name = copy_text(index, "name", descriptor.name)?;
    let base = copy_text(index, "base", descriptor.base)?;
    if name.is_empty() || !is_godot_class_name(&base) {
        return Err(invalid(
            index,
            "script name must be non-empty and base must be a Godot class name",
        ));
    }
    let get_field = descriptor
        .get_field
        .ok_or_else(|| invalid(index, "field getter is missing"))?;
    let get_method = descriptor
        .get_method
        .ok_or_else(|| invalid(index, "method getter is missing"))?;
    let create_state = descriptor
        .create_state
        .ok_or_else(|| invalid(index, "state creator is missing"))?;
    let drop_state = descriptor
        .drop_state
        .ok_or_else(|| invalid(index, "state dropper is missing"))?;
    let call_method = descriptor.call_method;

    let mut fields = Vec::with_capacity(descriptor.field_count as usize);
    let mut field_names = HashSet::new();
    for field_index in 0..descriptor.field_count {
        let mut output = MaybeUninit::<AbiFieldDescriptorV1>::zeroed();
        // SAFETY: Index is within the descriptor count and output is writable.
        let status = unsafe { get_field(field_index, output.as_mut_ptr()) };
        if status != AbiStatus::Ok {
            return Err(invalid(
                index,
                format!("field {field_index} returned {status:?}"),
            ));
        }
        // SAFETY: ABI success requires initialized output.
        let field = unsafe { output.assume_init() };
        let known_extensions = ABI_FIELD_EXTENSION_PROPERTY_SCHEMA
            | ABI_FIELD_EXTENSION_SIGNAL_SCHEMA
            | ABI_FIELD_EXTENSION_NODE_SCHEMA
            | ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA
            | ABI_FIELD_EXTENSION_RELOAD_SCHEMA;
        if field.struct_size < AbiFieldDescriptorV1::MINIMUM_SIZE
            || field.reserved_extension_flags & !known_extensions != 0
            || field.reserved_extension_flags.count_ones() > 1
        {
            return Err(invalid(
                index,
                format!("field {field_index} has an incompatible descriptor layout"),
            ));
        }
        if field.reserved_flags != [0; 3] {
            return Err(invalid(
                index,
                format!("field {field_index} reserved flags must be zero"),
            ));
        }
        if field.has_default > 1 {
            return Err(invalid(
                index,
                format!("field {field_index} has an invalid default-value flag"),
            ));
        }
        let name = copy_text(index, "field.name", field.name)?;
        let rust_type = copy_text(index, "field.rust_type", field.rust_type)?;
        let options = copy_text(index, "field.options", field.options)?;
        let default_value = copy_text(index, "field.default_value", field.default_value)?;
        if name.is_empty() || !field_names.insert(name.clone()) {
            return Err(invalid(index, format!("duplicate or empty field `{name}`")));
        }
        if rust_type.is_empty() {
            return Err(invalid(
                index,
                format!("field `{name}` has an empty Rust type"),
            ));
        }
        if field.has_default == 0 && !default_value.is_empty() {
            return Err(invalid(
                index,
                format!("field `{name}` supplies a default without setting its flag"),
            ));
        }
        let property = if field.reserved_extension_flags & ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA
            != 0
        {
            if field.kind != AbiFieldKind::Export {
                return Err(invalid(
                    index,
                    format!("field `{name}` has an invalid Godot integer schema extension"),
                ));
            }
            Some(copy_godot_integer_property(
                index,
                &name,
                &options,
                field.reserved,
            )?)
        } else if field.reserved_extension_flags & ABI_FIELD_EXTENSION_PROPERTY_SCHEMA != 0 {
            if field.kind != AbiFieldKind::Export {
                return Err(invalid(
                    index,
                    format!("field `{name}` has an invalid property schema extension"),
                ));
            }
            let type_ = AbiPropertyType(
                u32::try_from(field.reserved[0])
                    .map_err(|_| invalid(index, format!("field `{name}` type is out of range")))?,
            );
            let hint = u32::try_from(field.reserved[1])
                .map_err(|_| invalid(index, format!("field `{name}` hint is out of range")))?;
            let usage = u32::try_from(field.reserved[2])
                .map_err(|_| invalid(index, format!("field `{name}` usage is out of range")))?;
            let (group, hint_string, typed_array_element) =
                parse_property_options(index, &name, &options)?;
            validate_property_schema(index, &name, type_, hint, usage)?;
            if typed_array_element.is_some()
                && (type_ != AbiPropertyType::ARRAY || hint != ABI_PROPERTY_HINT_TYPE_STRING)
            {
                return Err(invalid(
                    index,
                    format!("export field `{name}` has inconsistent typed Array metadata"),
                ));
            }
            if type_ == AbiPropertyType::OBJECT
                && (!matches!(
                    hint,
                    ABI_PROPERTY_HINT_NODE_TYPE | ABI_PROPERTY_HINT_RESOURCE_TYPE
                ) || !is_godot_class_name(&hint_string))
            {
                return Err(invalid(
                    index,
                    format!("export field `{name}` has invalid Godot object class metadata"),
                ));
            }
            let typed_default = if matches!(
                type_,
                AbiPropertyType::STRING | AbiPropertyType::STRING_NAME | AbiPropertyType::NODE_PATH
            ) {
                if field.reserved[3] != 0 {
                    return Err(invalid(
                        index,
                        format!(
                            "text export field `{name}` must store its UTF-8 default in the field descriptor"
                        ),
                    ));
                }
                Some(if type_ == AbiPropertyType::STRING_NAME {
                    HostValue::StringName(default_value.clone())
                } else if type_ == AbiPropertyType::NODE_PATH {
                    HostValue::NodePath(default_value.clone())
                } else {
                    HostValue::String(default_value.clone())
                })
            } else if field.reserved[3] == 0 {
                module_value::empty_property_value(property_value_type(type_))
            } else if matches!(
                type_,
                AbiPropertyType::TRANSFORM2D
                    | AbiPropertyType::AABB
                    | AbiPropertyType::BASIS
                    | AbiPropertyType::TRANSFORM3D
                    | AbiPropertyType::PROJECTION
            ) {
                // SAFETY: Project modules are trusted native artifacts and
                // large fixed math property types define this slot as a live
                // AbiFixedMathDefaultV1 pointer during descriptor copying.
                let value = unsafe { &*(field.reserved[3] as *const AbiFixedMathDefaultV1) };
                Some(validate_fixed_math_property_default(
                    index, &name, type_, value,
                )?)
            } else {
                // SAFETY: Project modules are trusted native artifacts and
                // the schema flag defines this slot as a live AbiValueV1
                // pointer for the duration of descriptor copying.
                let value = unsafe { *(field.reserved[3] as *const AbiValueV1) };
                Some(validate_property_default(index, &name, type_, value)?)
            };
            Some(LoadedPropertySchema {
                type_,
                value_type: property_value_type(type_),
                hint,
                hint_string,
                typed_array_element,
                usage,
                group,
                default_value: typed_default,
            })
        } else {
            if field.kind == AbiFieldKind::Export {
                return Err(invalid(
                    index,
                    format!("export field `{name}` has no normalized property schema"),
                ));
            }
            None
        };
        let reload_value_type =
            if field.reserved_extension_flags & ABI_FIELD_EXTENSION_RELOAD_SCHEMA != 0 {
                let type_ = AbiValueType(u32::try_from(field.reserved[0]).map_err(|_| {
                    invalid(
                        index,
                        format!("field `{name}` reload value type is out of range"),
                    )
                })?);
                if field.kind != AbiFieldKind::Plain
                    || field.reload != AbiReloadPolicy::Persist
                    || !type_.is_supported()
                    || field.reserved[1..] != [0; 3]
                {
                    return Err(invalid(
                        index,
                        format!("field `{name}` has an invalid reload schema extension"),
                    ));
                }
                Some(type_)
            } else {
                None
            };
        let signal = if field.reserved_extension_flags & ABI_FIELD_EXTENSION_SIGNAL_SCHEMA != 0 {
            if field.kind != AbiFieldKind::Signal || field.reserved[2..] != [0; 2] {
                return Err(invalid(
                    index,
                    format!("field `{name}` has an invalid signal schema extension"),
                ));
            }
            Some(copy_signal_schema(
                index,
                &name,
                field.reserved[0],
                field.reserved[1],
            )?)
        } else {
            if field.kind == AbiFieldKind::Signal {
                return Err(invalid(
                    index,
                    format!("signal field `{name}` has no normalized signal schema"),
                ));
            }
            None
        };
        let node = if field.reserved_extension_flags & ABI_FIELD_EXTENSION_NODE_SCHEMA != 0 {
            if field.kind != AbiFieldKind::Node {
                return Err(invalid(
                    index,
                    format!("field `{name}` has an invalid node schema extension"),
                ));
            }
            Some(copy_node_schema(index, &name, field.reserved)?)
        } else {
            if field.kind == AbiFieldKind::Node {
                return Err(invalid(
                    index,
                    format!("node field `{name}` has no normalized node schema"),
                ));
            }
            None
        };
        if property.is_none()
            && signal.is_none()
            && node.is_none()
            && reload_value_type.is_none()
            && field.reserved != [0; 4]
        {
            return Err(invalid(
                index,
                format!("field `{name}` has non-zero reserved extension slots"),
            ));
        }
        fields.push(LoadedField {
            name,
            rust_type,
            kind: field.kind,
            options,
            default_value: (field.has_default != 0).then_some(default_value),
            reload: field.reload,
            reload_value_type,
            property,
            node,
            signal,
        });
    }

    let mut methods = Vec::with_capacity(descriptor.method_count as usize);
    let mut method_names = HashSet::new();
    let mut method_ids = HashSet::new();
    for method_index in 0..descriptor.method_count {
        let mut output = MaybeUninit::<AbiMethodDescriptorV1>::zeroed();
        // SAFETY: Index is within the descriptor count and output is writable.
        let status = unsafe { get_method(method_index, output.as_mut_ptr()) };
        if status != AbiStatus::Ok {
            return Err(invalid(
                index,
                format!("method {method_index} returned {status:?}"),
            ));
        }
        // SAFETY: ABI success requires initialized output.
        let method = unsafe { output.assume_init() };
        if method.struct_size < AbiMethodDescriptorV1::MINIMUM_SIZE
            || method.reserved_extension_flags
                & !(ABI_METHOD_EXTENSION_ARGUMENT_CLASSES
                    | ABI_METHOD_EXTENSION_RETURN_CLASS
                    | ABI_METHOD_EXTENSION_SCHEMA_V1)
                != 0
        {
            return Err(invalid(
                index,
                format!("method {method_index} has an incompatible descriptor layout"),
            ));
        }
        if method.reserved_flags != 0 {
            return Err(invalid(
                index,
                format!("method {method_index} reserved flags must be zero"),
            ));
        }
        if method.reserved_value_flags != 0 {
            return Err(invalid(
                index,
                format!("method {method_index} value flags must be zero"),
            ));
        }
        let name = copy_text(index, "method.name", method.name)?;
        let rust_signature = copy_text(index, "method.rust_signature", method.rust_signature)?;
        let options = copy_text(index, "method.options", method.options)?;
        let argument_types = copy_value_types(
            index,
            method_index,
            method.argument_types,
            method.argument_count,
        )?;
        if !method.return_type.is_supported() {
            return Err(invalid(
                index,
                format!("method `{name}` has an unsupported return value type"),
            ));
        }
        let extensions = copy_method_extensions(
            index,
            method_index,
            method.reserved_extension_flags,
            method.reserved,
            &argument_types,
            method.return_type,
        )?;
        let arguments = copy_method_arguments(
            index,
            method_index,
            method.arguments,
            &argument_types,
            extensions.argument_classes,
        )?;
        let rpc = copy_rpc_config(index, method_index, method.kind, method.rpc)?;
        if method.id == 0
            || name.is_empty()
            || rust_signature.is_empty()
            || !method_names.insert(name.clone())
            || !method_ids.insert(method.id)
        {
            return Err(invalid(
                index,
                format!("duplicate, empty, or invalid method `{name}`"),
            ));
        }
        methods.push(LoadedMethod {
            id: method.id,
            name,
            rust_signature,
            kind: method.kind,
            lifecycle: method.lifecycle,
            receiver: method.receiver,
            argument_count: method.argument_count,
            argument_types,
            arguments,
            default_arguments: extensions.default_arguments,
            vararg: extensions.vararg,
            return_type: method.return_type,
            return_class: extensions.return_class,
            options,
            rpc,
        });
    }
    validate_lifecycle_schema(index, &methods, descriptor.lifecycle)?;
    if methods
        .iter()
        .any(|method| method.kind != AbiMethodKind::Lifecycle)
        && call_method.is_none()
    {
        return Err(invalid(
            index,
            "reflected methods require a project method callback",
        ));
    }

    Ok(LoadedScript {
        resource_uid,
        source_path,
        name,
        global_name,
        base_script,
        base,
        tool: descriptor.tool != 0,
        fields,
        methods,
        create_state,
        drop_state,
        lifecycle: descriptor.lifecycle,
        call_method,
        get_field_value,
        set_field_value,
    })
}

fn copy_signal_schema(
    script: u32,
    field: &str,
    pointer: usize,
    count: usize,
) -> Result<LoadedSignalSchema, ModuleLoadError> {
    if count > MAX_SIGNAL_ARGUMENT_COUNT {
        return Err(invalid(
            script,
            format!("signal field `{field}` declares too many arguments"),
        ));
    }
    if count == 0 {
        return Ok(LoadedSignalSchema {
            arguments: Vec::new(),
        });
    }
    if pointer == 0 {
        return Err(invalid(
            script,
            format!("signal field `{field}` has a null argument schema"),
        ));
    }
    // SAFETY: Project modules are trusted native artifacts. Count is bounded,
    // and the SDK guarantees this static slice remains live during copying.
    let arguments = unsafe {
        core::slice::from_raw_parts(pointer as *const AbiSignalArgumentDescriptorV1, count)
    };
    let mut names = HashSet::with_capacity(arguments.len());
    let mut copied = Vec::with_capacity(arguments.len());
    for (argument_index, argument) in arguments.iter().enumerate() {
        if argument.reserved_flags != 0
            || !argument.type_.is_supported()
            || argument.type_ == AbiValueType::NIL
        {
            return Err(invalid(
                script,
                format!(
                    "signal field `{field}` argument {argument_index} has invalid type metadata"
                ),
            ));
        }
        let name = copy_text(script, "signal.argument.name", argument.name)?;
        if name.is_empty() || !names.insert(name.clone()) {
            return Err(invalid(
                script,
                format!("signal field `{field}` has an empty or duplicate argument `{name}`"),
            ));
        }
        copied.push(LoadedSignalArgument {
            name,
            type_: argument.type_,
        });
    }
    Ok(LoadedSignalSchema { arguments: copied })
}

fn copy_node_schema(
    script: u32,
    field: &str,
    slots: [usize; 4],
) -> Result<LoadedNodeSchema, ModuleLoadError> {
    let (class_name_length, optional) = decode_node_field_class(slots[3]);
    let path = copy_text(
        script,
        "field.node_path",
        AbiByteSlice {
            ptr: slots[0] as *const u8,
            len: slots[1],
        },
    )?;
    let class_name = copy_text(
        script,
        "field.node_class",
        AbiByteSlice {
            ptr: slots[2] as *const u8,
            len: class_name_length,
        },
    )?;
    if path.is_empty() || path.as_bytes().contains(&0) || !is_godot_class_name(&class_name) {
        return Err(invalid(
            script,
            format!("node field `{field}` has an invalid path or target class"),
        ));
    }
    Ok(LoadedNodeSchema {
        path,
        class_name,
        optional,
    })
}

#[derive(Debug)]
struct CopiedMethodExtensions {
    argument_classes: Vec<Option<String>>,
    return_class: Option<String>,
    default_arguments: Vec<AbiMethodDefaultFn>,
    vararg: bool,
}

fn copy_method_extensions(
    script: u32,
    method: u32,
    extension_flags: u32,
    slots: [usize; 4],
    argument_types: &[AbiValueType],
    return_type: AbiValueType,
) -> Result<CopiedMethodExtensions, ModuleLoadError> {
    if extension_flags & ABI_METHOD_EXTENSION_SCHEMA_V1 == 0 {
        let argument_classes =
            copy_method_argument_classes(script, method, extension_flags, slots, argument_types)?;
        let return_class =
            copy_method_return_class(script, method, extension_flags, slots, return_type)?;
        return Ok(CopiedMethodExtensions {
            argument_classes,
            return_class,
            default_arguments: Vec::new(),
            vararg: false,
        });
    }
    if extension_flags != ABI_METHOD_EXTENSION_SCHEMA_V1
        || slots[0] == 0
        || slots[1] < AbiMethodExtensionsV1::MINIMUM_SIZE as usize
        || slots[2..] != [0; 2]
        || slots[0] % core::mem::align_of::<AbiMethodExtensionsV1>() != 0
    {
        return Err(invalid(
            script,
            format!("method {method} has an invalid versioned extension schema"),
        ));
    }
    // SAFETY: The trusted project module promises a live, aligned extension
    // for this synchronous descriptor copy and the advertised size was
    // checked before reading the fixed V1 prefix.
    let extension = unsafe { (slots[0] as *const AbiMethodExtensionsV1).read() };
    if extension.struct_size < AbiMethodExtensionsV1::MINIMUM_SIZE
        || extension.struct_size as usize > slots[1]
        || extension.reserved_flags & !ABI_METHOD_SCHEMA_VARARG != 0
        || extension.reserved != [0; 4]
    {
        return Err(invalid(
            script,
            format!("method {method} has incompatible versioned metadata"),
        ));
    }
    let argument_classes = copy_method_argument_class_slice(
        script,
        method,
        extension.argument_classes,
        argument_types,
    )?;
    let return_class =
        copy_method_return_class_value(script, method, extension.return_class, return_type)?;
    let default_arguments = copy_method_default_callbacks(
        script,
        method,
        extension.default_arguments.ptr,
        extension.default_arguments.len,
        argument_types.len(),
    )?;
    Ok(CopiedMethodExtensions {
        argument_classes,
        return_class,
        default_arguments,
        vararg: extension.reserved_flags & ABI_METHOD_SCHEMA_VARARG != 0,
    })
}

fn copy_method_default_callbacks(
    script: u32,
    method: u32,
    pointer: *const AbiMethodDefaultFn,
    count: usize,
    argument_count: usize,
) -> Result<Vec<AbiMethodDefaultFn>, ModuleLoadError> {
    if count > argument_count || count > MAX_MEMBER_COUNT as usize {
        return Err(invalid(
            script,
            format!("method {method} declares too many default arguments"),
        ));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err(invalid(
            script,
            format!("method {method} has a null default-argument callback slice"),
        ));
    }
    // SAFETY: Count is bounded and the trusted module keeps this static slice
    // live for the synchronous descriptor copy.
    let callbacks = unsafe { core::slice::from_raw_parts(pointer, count) };
    if callbacks.iter().any(Option::is_none) {
        return Err(invalid(
            script,
            format!("method {method} has a null default-argument callback"),
        ));
    }
    Ok(callbacks.to_vec())
}

fn copy_method_argument_class_slice(
    script: u32,
    method: u32,
    classes: AbiByteSliceSlice,
    expected_types: &[AbiValueType],
) -> Result<Vec<Option<String>>, ModuleLoadError> {
    if classes.len != expected_types.len() || (classes.ptr.is_null() && !expected_types.is_empty())
    {
        return Err(invalid(
            script,
            format!("method {method} argument class metadata has an invalid slice"),
        ));
    }
    if expected_types.is_empty() {
        return Ok(Vec::new());
    }
    // SAFETY: Count is the already bounded method arity and the trusted
    // project module keeps the static slice live for this copy.
    let classes = unsafe { core::slice::from_raw_parts(classes.ptr, classes.len) };
    copy_method_argument_class_values(script, method, classes, expected_types)
}

fn copy_method_argument_class_values(
    script: u32,
    method: u32,
    classes: &[AbiByteSlice],
    expected_types: &[AbiValueType],
) -> Result<Vec<Option<String>>, ModuleLoadError> {
    debug_assert_eq!(classes.len(), expected_types.len());
    let mut copied = Vec::with_capacity(classes.len());
    for (index, (class_name, expected)) in classes.iter().zip(expected_types).enumerate() {
        let class_name = copy_text(script, "method.argument.class_name", *class_name)?;
        if *expected == AbiValueType::OBJECT_ID {
            if !is_godot_class_name(&class_name) {
                return Err(invalid(
                    script,
                    format!("method {method} argument {index} has an invalid Godot class"),
                ));
            }
            copied.push(Some(class_name));
        } else if *expected == AbiValueType::ARRAY && is_typed_array_element_name(&class_name) {
            copied.push(Some(class_name));
        } else if class_name.is_empty() {
            copied.push(None);
        } else {
            return Err(invalid(
                script,
                format!("method {method} argument {index} has unexpected class metadata"),
            ));
        }
    }
    Ok(copied)
}

fn copy_method_argument_classes(
    script: u32,
    method: u32,
    extension_flags: u32,
    slots: [usize; 4],
    expected_types: &[AbiValueType],
) -> Result<Vec<Option<String>>, ModuleLoadError> {
    if extension_flags & ABI_METHOD_EXTENSION_ARGUMENT_CLASSES == 0 {
        if slots[0..2] != [0; 2] {
            return Err(invalid(
                script,
                format!("method {method} has argument class data without its extension flag"),
            ));
        }
        if expected_types.contains(&AbiValueType::OBJECT_ID) {
            return Err(invalid(
                script,
                format!("method {method} object argument has no Godot class metadata"),
            ));
        }
        return Ok(vec![None; expected_types.len()]);
    }
    if slots[1] != expected_types.len() || (slots[0] == 0 && slots[1] != 0) {
        return Err(invalid(
            script,
            format!("method {method} argument class metadata has an invalid slice"),
        ));
    }
    if expected_types.is_empty() {
        return Ok(Vec::new());
    }
    // SAFETY: Project modules are trusted native artifacts. The count matches
    // the already bounded method arity before borrowing the descriptor slice.
    let classes = unsafe { core::slice::from_raw_parts(slots[0] as *const AbiByteSlice, slots[1]) };
    copy_method_argument_class_values(script, method, classes, expected_types)
}

fn copy_method_return_class_value(
    script: u32,
    method: u32,
    value: AbiByteSlice,
    expected_type: AbiValueType,
) -> Result<Option<String>, ModuleLoadError> {
    let class_name = copy_text(script, "method.return.class_name", value)?;
    if class_name.is_empty() {
        if expected_type == AbiValueType::OBJECT_ID {
            return Err(invalid(
                script,
                format!("method {method} object return value has no Godot class metadata"),
            ));
        }
        return Ok(None);
    }
    if expected_type != AbiValueType::OBJECT_ID && expected_type != AbiValueType::ARRAY {
        return Err(invalid(
            script,
            format!("method {method} non-object return value has unexpected type metadata"),
        ));
    }
    let valid = if expected_type == AbiValueType::OBJECT_ID {
        is_godot_class_name(&class_name)
    } else {
        is_typed_array_element_name(&class_name)
    };
    if !valid {
        return Err(invalid(
            script,
            format!("method {method} return value has invalid type metadata"),
        ));
    }
    Ok(Some(class_name))
}

fn copy_method_return_class(
    script: u32,
    method: u32,
    extension_flags: u32,
    slots: [usize; 4],
    expected_type: AbiValueType,
) -> Result<Option<String>, ModuleLoadError> {
    if extension_flags & ABI_METHOD_EXTENSION_RETURN_CLASS == 0 {
        if slots[2..4] != [0; 2] {
            return Err(invalid(
                script,
                format!("method {method} has return class data without its extension flag"),
            ));
        }
        if expected_type == AbiValueType::OBJECT_ID {
            return Err(invalid(
                script,
                format!("method {method} object return value has no Godot class metadata"),
            ));
        }
        return Ok(None);
    }
    copy_method_return_class_value(
        script,
        method,
        AbiByteSlice {
            ptr: slots[2] as *const u8,
            len: slots[3],
        },
        expected_type,
    )
}

fn is_typed_array_element_name(value: &str) -> bool {
    matches!(
        value,
        "Variant"
            | "bool"
            | "int"
            | "float"
            | "String"
            | "Vector2"
            | "Vector2i"
            | "Rect2"
            | "Rect2i"
            | "Vector3"
            | "Vector3i"
            | "Transform2D"
            | "Vector4"
            | "Vector4i"
            | "Plane"
            | "Quaternion"
            | "AABB"
            | "Basis"
            | "Transform3D"
            | "Projection"
            | "Color"
            | "StringName"
            | "NodePath"
            | "RID"
            | "Object"
            | "Callable"
            | "Signal"
            | "Dictionary"
            | "Array"
            | "PackedByteArray"
            | "PackedInt32Array"
            | "PackedInt64Array"
            | "PackedFloat32Array"
            | "PackedFloat64Array"
            | "PackedStringArray"
            | "PackedVector2Array"
            | "PackedVector3Array"
            | "PackedColorArray"
            | "PackedVector4Array"
    ) || is_godot_class_name(value)
}

fn copy_method_arguments(
    script: u32,
    method: u32,
    value: AbiMethodArgumentSlice,
    expected_types: &[AbiValueType],
    class_names: Vec<Option<String>>,
) -> Result<Vec<LoadedMethodArgument>, ModuleLoadError> {
    if value.len != expected_types.len() {
        return Err(invalid(
            script,
            format!("method {method} argument metadata count does not match its value schema"),
        ));
    }
    if value.len == 0 {
        return Ok(Vec::new());
    }
    if value.ptr.is_null() {
        return Err(invalid(
            script,
            format!("method {method} argument metadata has a null pointer"),
        ));
    }
    // SAFETY: The module promises `len` live entries during this getter call;
    // the method count and per-method arity are bounded before this copy.
    let arguments = unsafe { core::slice::from_raw_parts(value.ptr, value.len) };
    let mut names = HashSet::with_capacity(arguments.len());
    let mut copied = Vec::with_capacity(arguments.len());
    for (index, ((argument, expected), class_name)) in arguments
        .iter()
        .zip(expected_types)
        .zip(class_names)
        .enumerate()
    {
        if argument.reserved_flags != 0 || argument.type_ != *expected {
            return Err(invalid(
                script,
                format!("method {method} argument {index} has inconsistent metadata"),
            ));
        }
        let name = copy_text(script, "method.argument.name", argument.name)?;
        if name.is_empty() || !names.insert(name.clone()) {
            return Err(invalid(
                script,
                format!("method {method} has an empty or duplicate argument `{name}`"),
            ));
        }
        copied.push(LoadedMethodArgument {
            name,
            type_: argument.type_,
            class_name,
        });
    }
    Ok(copied)
}

fn copy_rpc_config(
    script: u32,
    method: u32,
    kind: AbiMethodKind,
    rpc: AbiRpcConfigV1,
) -> Result<Option<AbiRpcConfigV1>, ModuleLoadError> {
    if rpc.present > 1
        || rpc.call_local > 1
        || rpc.reserved_bytes != [0; 2]
        || rpc.reserved_flags != 0
        || !rpc.mode.is_supported()
        || !rpc.transfer_mode.is_supported()
    {
        return Err(invalid(
            script,
            format!("method {method} has invalid RPC metadata"),
        ));
    }
    if kind == AbiMethodKind::Rpc {
        if rpc.present == 0 {
            return Err(invalid(
                script,
                format!("RPC method {method} has no RPC configuration"),
            ));
        }
        Ok(Some(rpc))
    } else if rpc == AbiRpcConfigV1::NONE {
        Ok(None)
    } else {
        Err(invalid(
            script,
            format!("non-RPC method {method} declares RPC configuration"),
        ))
    }
}

fn copy_value_types(
    script: u32,
    method: u32,
    value: AbiValueTypeSlice,
    expected_count: u16,
) -> Result<Vec<AbiValueType>, ModuleLoadError> {
    if value.len != usize::from(expected_count) {
        return Err(invalid(
            script,
            format!("method {method} value schema count does not match its argument count"),
        ));
    }
    if value.len == 0 {
        return Ok(Vec::new());
    }
    if value.ptr.is_null() {
        return Err(invalid(
            script,
            format!("method {method} value schema has a null pointer"),
        ));
    }
    // SAFETY: The trusted module promises `len` live entries for the getter
    // call; the count is already bounded by MAX_MEMBER_COUNT.
    let values = unsafe { core::slice::from_raw_parts(value.ptr, value.len) };
    if values.iter().any(|value| !value.is_supported()) {
        return Err(invalid(
            script,
            format!("method {method} uses an unsupported ABI value type"),
        ));
    }
    Ok(values.to_vec())
}

fn validate_lifecycle_schema(
    script: u32,
    methods: &[LoadedMethod],
    table: AbiLifecycleTableV1,
) -> Result<(), ModuleLoadError> {
    let mut expected = [false; 7];
    for method in methods {
        match method.kind {
            AbiMethodKind::Lifecycle => {
                let Some((slot, canonical_name, argument_count)) =
                    lifecycle_shape(method.lifecycle)
                else {
                    return Err(invalid(
                        script,
                        format!("lifecycle method `{}` has no valid slot", method.name),
                    ));
                };
                if method.name != canonical_name
                    || method.argument_count != argument_count
                    || method.receiver != AbiReceiverKind::Mutable
                    || method.return_type != AbiValueType::NIL
                    || method.argument_types.as_slice() != lifecycle_value_types(method.lifecycle)
                    || !method.default_arguments.is_empty()
                    || method.vararg
                {
                    return Err(invalid(
                        script,
                        format!(
                            "lifecycle method `{}` has inconsistent metadata",
                            method.name
                        ),
                    ));
                }
                if expected[slot] {
                    return Err(invalid(
                        script,
                        format!("lifecycle slot `{canonical_name}` is declared twice"),
                    ));
                }
                expected[slot] = true;
            }
            AbiMethodKind::Func | AbiMethodKind::Rpc => {
                if method.lifecycle != AbiLifecycleSlot::None {
                    return Err(invalid(
                        script,
                        format!(
                            "non-lifecycle method `{}` declares a lifecycle slot",
                            method.name
                        ),
                    ));
                }
            }
        }
    }
    let actual = [
        table.enter_tree.is_some(),
        table.ready.is_some(),
        table.process.is_some(),
        table.physics_process.is_some(),
        table.input.is_some(),
        table.unhandled_input.is_some(),
        table.exit_tree.is_some(),
    ];
    if actual != expected {
        return Err(invalid(
            script,
            "lifecycle callback table does not match method descriptors",
        ));
    }
    Ok(())
}

fn lifecycle_value_types(slot: AbiLifecycleSlot) -> &'static [AbiValueType] {
    match slot {
        AbiLifecycleSlot::None
        | AbiLifecycleSlot::EnterTree
        | AbiLifecycleSlot::Ready
        | AbiLifecycleSlot::ExitTree => &[],
        AbiLifecycleSlot::Process | AbiLifecycleSlot::PhysicsProcess => &[AbiValueType::F64],
        AbiLifecycleSlot::Input | AbiLifecycleSlot::UnhandledInput => &[AbiValueType::OBJECT_ID],
    }
}

fn lifecycle_shape(slot: AbiLifecycleSlot) -> Option<(usize, &'static str, u16)> {
    match slot {
        AbiLifecycleSlot::None => None,
        AbiLifecycleSlot::EnterTree => Some((0, "_enter_tree", 0)),
        AbiLifecycleSlot::Ready => Some((1, "_ready", 0)),
        AbiLifecycleSlot::Process => Some((2, "_process", 1)),
        AbiLifecycleSlot::PhysicsProcess => Some((3, "_physics_process", 1)),
        AbiLifecycleSlot::Input => Some((4, "_input", 1)),
        AbiLifecycleSlot::UnhandledInput => Some((5, "_unhandled_input", 1)),
        AbiLifecycleSlot::ExitTree => Some((6, "_exit_tree", 0)),
    }
}

fn copy_godot_integer_property(
    script: u32,
    field: &str,
    group: &str,
    reserved: [usize; 4],
) -> Result<LoadedPropertySchema, ModuleLoadError> {
    if reserved[0] > 1
        || reserved[1] == 0
        || reserved[2] == 0
        || reserved[2] > MAX_MEMBER_COUNT as usize
        || reserved[3] == 0
    {
        return Err(invalid(
            script,
            format!("export field `{field}` has invalid Godot integer metadata"),
        ));
    }
    if group.chars().any(char::is_control) {
        return Err(invalid(
            script,
            format!("export field `{field}` has an invalid property group"),
        ));
    }
    let signed = reserved[0] == 1;
    // SAFETY: Project modules are trusted native artifacts. The specialized
    // extension promises this live static option slice for descriptor copying,
    // and the count was bounded above.
    let options = unsafe {
        core::slice::from_raw_parts(reserved[1] as *const AbiGodotIntegerOptionV1, reserved[2])
    };
    let mut copied = Vec::with_capacity(options.len());
    let mut names = HashSet::with_capacity(options.len());
    for option in options {
        let name = copy_text(script, "Godot integer option", option.name)?;
        if !is_godot_integer_name(&name) || !names.insert(name.clone()) {
            return Err(invalid(
                script,
                format!("export field `{field}` has an invalid or duplicate Godot integer option"),
            ));
        }
        copied.push((name, option.raw));
    }
    let hint_string = godot_integer_hint_string(&copied, signed);
    if hint_string.is_empty() || hint_string.len() > MAX_ABI_TEXT_BYTES {
        return Err(invalid(
            script,
            format!("export field `{field}` generated invalid Inspector options"),
        ));
    }
    // SAFETY: The extension defines slot three as this exact C function
    // pointer and the project module remains loaded throughout the call.
    let default_fn: AbiGodotIntegerDefaultFn = unsafe { core::mem::transmute(reserved[3]) };
    // SAFETY: The generated function has no arguments and returns one raw
    // integer without borrowing project state.
    let default_raw = unsafe { default_fn() };
    let value_type = if signed {
        AbiValueType::I64
    } else {
        AbiValueType::U64
    };
    let default_value = if signed {
        AbiValueV1::from_i64(default_raw as i64)
    } else {
        AbiValueV1::from_u64(default_raw)
    };
    Ok(LoadedPropertySchema {
        type_: AbiPropertyType::INT,
        value_type,
        hint: if signed {
            ABI_PROPERTY_HINT_ENUM
        } else {
            ABI_PROPERTY_HINT_FLAGS
        },
        hint_string,
        typed_array_element: None,
        usage: ABI_PROPERTY_USAGE_SCRIPT_DEFAULT,
        group: (!group.is_empty()).then(|| group.to_owned()),
        default_value: Some(HostValue::Scalar(default_value)),
    })
}

fn is_godot_integer_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn godot_integer_hint_string(options: &[(String, u64)], signed: bool) -> String {
    let visible = options
        .iter()
        .filter(|(name, _)| {
            !signed || !matches!(name.rsplit('_').next(), Some("MAX" | "COUNT" | "ENUM_SIZE"))
        })
        .collect::<Vec<_>>();
    let visible = if visible.is_empty() {
        options.iter().collect::<Vec<_>>()
    } else {
        visible
    };
    let prefix_components = common_integer_prefix(&visible);
    visible
        .into_iter()
        .map(|(name, raw)| {
            let label = name
                .split('_')
                .skip(prefix_components)
                .filter(|component| !component.is_empty())
                .map(friendly_integer_word)
                .collect::<Vec<_>>()
                .join(" ");
            format!("{label}:{}", *raw as i64)
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn common_integer_prefix(options: &[&(String, u64)]) -> usize {
    let Some((first, _)) = options.first().copied() else {
        return 0;
    };
    let first = first.split('_').collect::<Vec<_>>();
    let maximum = options
        .iter()
        .map(|(name, _)| name.split('_').count().saturating_sub(1))
        .min()
        .unwrap_or(0);
    (0..maximum)
        .take_while(|index| {
            options
                .iter()
                .all(|(name, _)| name.split('_').nth(*index) == Some(first[*index]))
        })
        .count()
}

fn friendly_integer_word(word: &str) -> String {
    if word.bytes().any(|byte| byte.is_ascii_digit())
        || matches!(word, "CPU" | "FPS" | "GPU" | "ID" | "IO" | "RID" | "XR")
    {
        return word.to_owned();
    }
    let mut bytes = word.bytes();
    let Some(first) = bytes.next() else {
        return String::new();
    };
    let mut result = String::with_capacity(word.len());
    result.push(char::from(first));
    result.extend(bytes.map(|byte| char::from(byte.to_ascii_lowercase())));
    result
}

fn parse_property_options(
    script: u32,
    field: &str,
    options: &str,
) -> Result<(Option<String>, String, Option<String>), ModuleLoadError> {
    let (encoded, has_array_element) =
        if let Some(encoded) = options.strip_prefix("gdrs-property-v2:") {
            (encoded, true)
        } else if let Some(encoded) = options.strip_prefix("gdrs-property-v1:") {
            (encoded, false)
        } else {
            return Err(invalid(
                script,
                format!("export field `{field}` has an invalid property schema payload"),
            ));
        };
    let Some((group_length, encoded)) = encoded.split_once(':') else {
        return Err(invalid(
            script,
            format!("export field `{field}` property group length is missing"),
        ));
    };
    let Some((hint_length, encoded)) = encoded.split_once(':') else {
        return Err(invalid(
            script,
            format!("export field `{field}` property hint length is missing"),
        ));
    };
    let (array_element_length, values) = if has_array_element {
        let Some((array_element_length, values)) = encoded.split_once(':') else {
            return Err(invalid(
                script,
                format!("export field `{field}` Array element length is missing"),
            ));
        };
        let array_element_length = array_element_length.parse::<usize>().map_err(|_| {
            invalid(
                script,
                format!("export field `{field}` Array element length is invalid"),
            )
        })?;
        (array_element_length, values)
    } else {
        (0, encoded)
    };
    let group_length = group_length.parse::<usize>().map_err(|_| {
        invalid(
            script,
            format!("export field `{field}` property group length is invalid"),
        )
    })?;
    let hint_length = hint_length.parse::<usize>().map_err(|_| {
        invalid(
            script,
            format!("export field `{field}` property hint length is invalid"),
        )
    })?;
    let hint_end = group_length.checked_add(hint_length).ok_or_else(|| {
        invalid(
            script,
            format!("export field `{field}` property payload length overflowed"),
        )
    })?;
    let expected_length = hint_end.checked_add(array_element_length).ok_or_else(|| {
        invalid(
            script,
            format!("export field `{field}` property payload length overflowed"),
        )
    })?;
    if values.len() != expected_length
        || !values.is_char_boundary(group_length)
        || !values.is_char_boundary(hint_end)
    {
        return Err(invalid(
            script,
            format!("export field `{field}` property payload length does not match"),
        ));
    }
    let (group, remaining) = values.split_at(group_length);
    let (hint_string, typed_array_element) = remaining.split_at(hint_length);
    let typed_array_element =
        (!typed_array_element.is_empty()).then(|| typed_array_element.to_owned());
    if typed_array_element
        .as_deref()
        .is_some_and(|element| !is_typed_array_element_name(element))
    {
        return Err(invalid(
            script,
            format!("export field `{field}` has an invalid typed Array element"),
        ));
    }
    Ok((
        (!group.is_empty()).then(|| group.to_owned()),
        hint_string.to_owned(),
        typed_array_element,
    ))
}

fn validate_property_schema(
    script: u32,
    field: &str,
    type_: AbiPropertyType,
    hint: u32,
    usage: u32,
) -> Result<(), ModuleLoadError> {
    if !type_.is_supported() || type_ == AbiPropertyType::NIL {
        return Err(invalid(
            script,
            format!("export field `{field}` has an unsupported property type"),
        ));
    }
    let valid_hint = match hint {
        ABI_PROPERTY_HINT_NONE => true,
        ABI_PROPERTY_HINT_RANGE => {
            matches!(type_, AbiPropertyType::INT | AbiPropertyType::FLOAT)
        }
        ABI_PROPERTY_HINT_ENUM | ABI_PROPERTY_HINT_FLAGS => type_ == AbiPropertyType::INT,
        ABI_PROPERTY_HINT_FILE | ABI_PROPERTY_HINT_MULTILINE_TEXT => {
            type_ == AbiPropertyType::STRING
        }
        ABI_PROPERTY_HINT_COLOR_NO_ALPHA => type_ == AbiPropertyType::COLOR,
        ABI_PROPERTY_HINT_TYPE_STRING => type_ == AbiPropertyType::ARRAY,
        ABI_PROPERTY_HINT_RESOURCE_TYPE | ABI_PROPERTY_HINT_NODE_TYPE => {
            type_ == AbiPropertyType::OBJECT
        }
        _ => false,
    };
    if !valid_hint {
        return Err(invalid(
            script,
            format!("export field `{field}` has an incompatible property hint"),
        ));
    }
    let expected_usage = if hint == ABI_PROPERTY_HINT_NODE_TYPE {
        ABI_PROPERTY_USAGE_SCRIPT_DEFAULT | ABI_PROPERTY_USAGE_NODE_PATH_FROM_SCENE_ROOT
    } else {
        ABI_PROPERTY_USAGE_SCRIPT_DEFAULT
    };
    if usage != expected_usage {
        return Err(invalid(
            script,
            format!("export field `{field}` has unsupported property usage flags"),
        ));
    }
    Ok(())
}

fn validate_property_default(
    script: u32,
    field: &str,
    property_type: AbiPropertyType,
    value: AbiValueV1,
) -> Result<HostValue, ModuleLoadError> {
    let expected = match property_type {
        AbiPropertyType::BOOL => Some(AbiValueType::BOOL),
        AbiPropertyType::INT => Some(AbiValueType::I64),
        AbiPropertyType::FLOAT => Some(AbiValueType::F64),
        AbiPropertyType::VECTOR2 => Some(AbiValueType::VECTOR2),
        AbiPropertyType::VECTOR2I => Some(AbiValueType::VECTOR2I),
        AbiPropertyType::VECTOR3 => Some(AbiValueType::VECTOR3),
        AbiPropertyType::VECTOR3I => Some(AbiValueType::VECTOR3I),
        AbiPropertyType::VECTOR4 => Some(AbiValueType::VECTOR4),
        AbiPropertyType::VECTOR4I => Some(AbiValueType::VECTOR4I),
        AbiPropertyType::RECT2 => Some(AbiValueType::RECT2),
        AbiPropertyType::RECT2I => Some(AbiValueType::RECT2I),
        AbiPropertyType::QUATERNION => Some(AbiValueType::QUATERNION),
        AbiPropertyType::PLANE => Some(AbiValueType::PLANE),
        AbiPropertyType::TRANSFORM2D => Some(AbiValueType::TRANSFORM2D),
        AbiPropertyType::AABB => Some(AbiValueType::AABB),
        AbiPropertyType::BASIS => Some(AbiValueType::BASIS),
        AbiPropertyType::TRANSFORM3D => Some(AbiValueType::TRANSFORM3D),
        AbiPropertyType::PROJECTION => Some(AbiValueType::PROJECTION),
        AbiPropertyType::COLOR => Some(AbiValueType::COLOR),
        AbiPropertyType::OBJECT => Some(AbiValueType::OBJECT_ID),
        AbiPropertyType::STRING
        | AbiPropertyType::STRING_NAME
        | AbiPropertyType::NODE_PATH
        | AbiPropertyType::CALLABLE
        | AbiPropertyType::SIGNAL
        | AbiPropertyType::DICTIONARY
        | AbiPropertyType::ARRAY
        | AbiPropertyType::PACKED_BYTE_ARRAY
        | AbiPropertyType::PACKED_INT32_ARRAY
        | AbiPropertyType::PACKED_INT64_ARRAY
        | AbiPropertyType::PACKED_FLOAT32_ARRAY
        | AbiPropertyType::PACKED_FLOAT64_ARRAY
        | AbiPropertyType::PACKED_STRING_ARRAY
        | AbiPropertyType::PACKED_VECTOR2_ARRAY
        | AbiPropertyType::PACKED_VECTOR3_ARRAY
        | AbiPropertyType::PACKED_COLOR_ARRAY
        | AbiPropertyType::PACKED_VECTOR4_ARRAY => None,
        _ => None,
    };
    let Some(expected) = expected else {
        return Err(invalid(
            script,
            format!("export field `{field}` has an invalid typed default value"),
        ));
    };
    let copied = module_value::copy_descriptor_value(expected, value).map_err(|_| {
        invalid(
            script,
            format!("export field `{field}` has an invalid typed default value"),
        )
    })?;
    if property_type == AbiPropertyType::OBJECT && value.payload != [0, 0] {
        return Err(invalid(
            script,
            format!("export field `{field}` object default must be null"),
        ));
    }
    Ok(copied)
}

fn property_value_type(type_: AbiPropertyType) -> AbiValueType {
    match type_ {
        AbiPropertyType::BOOL => AbiValueType::BOOL,
        AbiPropertyType::INT => AbiValueType::I64,
        AbiPropertyType::FLOAT => AbiValueType::F64,
        AbiPropertyType::STRING => AbiValueType::STRING,
        AbiPropertyType::STRING_NAME => AbiValueType::STRING_NAME,
        AbiPropertyType::NODE_PATH => AbiValueType::NODE_PATH,
        AbiPropertyType::RID => AbiValueType::RID,
        AbiPropertyType::OBJECT => AbiValueType::OBJECT_ID,
        AbiPropertyType::CALLABLE => AbiValueType::CALLABLE,
        AbiPropertyType::SIGNAL => AbiValueType::SIGNAL,
        AbiPropertyType::DICTIONARY => AbiValueType::DICTIONARY,
        AbiPropertyType::ARRAY => AbiValueType::ARRAY,
        AbiPropertyType::PACKED_BYTE_ARRAY => AbiValueType::PACKED_BYTE_ARRAY,
        AbiPropertyType::PACKED_INT32_ARRAY => AbiValueType::PACKED_INT32_ARRAY,
        AbiPropertyType::PACKED_INT64_ARRAY => AbiValueType::PACKED_INT64_ARRAY,
        AbiPropertyType::PACKED_FLOAT32_ARRAY => AbiValueType::PACKED_FLOAT32_ARRAY,
        AbiPropertyType::PACKED_FLOAT64_ARRAY => AbiValueType::PACKED_FLOAT64_ARRAY,
        AbiPropertyType::PACKED_STRING_ARRAY => AbiValueType::PACKED_STRING_ARRAY,
        AbiPropertyType::PACKED_VECTOR2_ARRAY => AbiValueType::PACKED_VECTOR2_ARRAY,
        AbiPropertyType::PACKED_VECTOR3_ARRAY => AbiValueType::PACKED_VECTOR3_ARRAY,
        AbiPropertyType::PACKED_COLOR_ARRAY => AbiValueType::PACKED_COLOR_ARRAY,
        AbiPropertyType::PACKED_VECTOR4_ARRAY => AbiValueType::PACKED_VECTOR4_ARRAY,
        AbiPropertyType::VECTOR2 => AbiValueType::VECTOR2,
        AbiPropertyType::VECTOR2I => AbiValueType::VECTOR2I,
        AbiPropertyType::VECTOR3 => AbiValueType::VECTOR3,
        AbiPropertyType::VECTOR3I => AbiValueType::VECTOR3I,
        AbiPropertyType::VECTOR4 => AbiValueType::VECTOR4,
        AbiPropertyType::VECTOR4I => AbiValueType::VECTOR4I,
        AbiPropertyType::RECT2 => AbiValueType::RECT2,
        AbiPropertyType::RECT2I => AbiValueType::RECT2I,
        AbiPropertyType::QUATERNION => AbiValueType::QUATERNION,
        AbiPropertyType::PLANE => AbiValueType::PLANE,
        AbiPropertyType::TRANSFORM2D => AbiValueType::TRANSFORM2D,
        AbiPropertyType::AABB => AbiValueType::AABB,
        AbiPropertyType::BASIS => AbiValueType::BASIS,
        AbiPropertyType::TRANSFORM3D => AbiValueType::TRANSFORM3D,
        AbiPropertyType::PROJECTION => AbiValueType::PROJECTION,
        AbiPropertyType::COLOR => AbiValueType::COLOR,
        _ => AbiValueType::NIL,
    }
}

fn validate_fixed_math_property_default(
    script: u32,
    field: &str,
    property_type: AbiPropertyType,
    value: &AbiFixedMathDefaultV1,
) -> Result<HostValue, ModuleLoadError> {
    let (expected, component_count) = match property_type {
        AbiPropertyType::TRANSFORM2D => (AbiValueType::TRANSFORM2D, 6),
        AbiPropertyType::AABB => (AbiValueType::AABB, 6),
        AbiPropertyType::BASIS => (AbiValueType::BASIS, 9),
        AbiPropertyType::TRANSFORM3D => (AbiValueType::TRANSFORM3D, 12),
        AbiPropertyType::PROJECTION => (AbiValueType::PROJECTION, 16),
        _ => {
            return Err(invalid(
                script,
                format!("export field `{field}` has an invalid fixed math default"),
            ));
        }
    };
    if value.struct_size < AbiFixedMathDefaultV1::MINIMUM_SIZE
        || value.component_count != component_count
        || value.reserved != [0; 2]
    {
        return Err(invalid(
            script,
            format!("export field `{field}` has an invalid fixed math default"),
        ));
    }
    module_value::copy_fixed_math_descriptor(
        expected,
        &value.component_bits[..component_count as usize],
    )
    .map_err(|_| {
        invalid(
            script,
            format!("export field `{field}` has an invalid fixed math default"),
        )
    })
}

fn is_canonical_rs_path(path: &str) -> bool {
    let Some(relative) = path.strip_prefix("res://") else {
        return false;
    };
    relative.ends_with(".rs")
        && !relative.contains('\\')
        && relative
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_godot_class_name(name: &str) -> bool {
    let mut characters = name.bytes();
    matches!(characters.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && characters.all(|value| value.is_ascii_alphanumeric() || value == b'_')
}

fn copy_text(script: u32, field: &str, value: AbiByteSlice) -> Result<String, ModuleLoadError> {
    if value.len > MAX_ABI_TEXT_BYTES {
        return Err(invalid(script, format!("{field} exceeds size limit")));
    }
    if value.len == 0 {
        return Ok(String::new());
    }
    if value.ptr.is_null() {
        return Err(invalid(script, format!("{field} has a null pointer")));
    }
    // SAFETY: Project modules are trusted native artifacts. Length is bounded
    // before constructing the borrowed slice.
    let bytes = unsafe { core::slice::from_raw_parts(value.ptr, value.len) };
    let text = core::str::from_utf8(bytes)
        .map_err(|_| invalid(script, format!("{field} is not UTF-8")))?;
    Ok(text.to_owned())
}

fn call_result(result: AbiCallResult) -> Result<(), ModuleCallError> {
    if result.status == AbiStatus::Ok {
        return Ok(());
    }
    let message = if result.message.len == 0 {
        String::new()
    } else if result.message.ptr.is_null() || result.message.len > MAX_ABI_TEXT_BYTES {
        "<invalid module diagnostic>".into()
    } else {
        // SAFETY: The callback guarantees the message for the call duration,
        // and the length was bounded above.
        let bytes = unsafe { core::slice::from_raw_parts(result.message.ptr, result.message.len) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    Err(ModuleCallError {
        status: result.status,
        message,
    })
}

fn invalid(script: u32, message: impl Into<String>) -> ModuleLoadError {
    ModuleLoadError::InvalidDescriptor {
        script,
        message: message.into(),
    }
}

unsafe extern "C" fn host_log(
    _context: *mut c_void,
    level: AbiLogLevel,
    target: AbiByteSlice,
    message: AbiByteSlice,
) {
    let _ = std::panic::catch_unwind(|| {
        let target = lossy_text(target);
        let message = lossy_text(message);
        crate::logging::module(
            level,
            format_args!("[godot-rust module/{level:?}] {target}: {message}"),
        );
    });
}

fn lossy_text(value: AbiByteSlice) -> String {
    if value.len == 0 {
        return String::new();
    }
    if value.ptr.is_null() || value.len > MAX_ABI_TEXT_BYTES {
        return "<invalid text>".into();
    }
    // SAFETY: The callback promises a live slice for its duration and length
    // was bounded.
    let bytes = unsafe { core::slice::from_raw_parts(value.ptr, value.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn drop_value(_value: AbiValueV1) -> AbiStatus {
        AbiStatus::Ok
    }

    unsafe extern "C" fn godot_integer_default() -> u64 {
        3
    }

    unsafe extern "C" fn method_default(output: *mut AbiValueV1) -> AbiCallResult {
        if output.is_null() {
            return AbiCallResult::failure(AbiStatus::InvalidArgument, "null default output");
        }
        // SAFETY: Null was rejected and the callback contract provides one
        // writable output slot.
        unsafe { output.write(AbiValueV1::from_i64(22)) };
        AbiCallResult::OK
    }

    fn module_api() -> ModuleApiV1 {
        let mut reserved = [0; 13];
        reserved[MODULE_API_SLOT_GODOT_API_MAJOR] = 4;
        reserved[MODULE_API_SLOT_GODOT_API_MINOR] = 4;
        ModuleApiV1 {
            header: godot_api::abi::AbiHeader::new(ModuleApiV1::MINIMUM_SIZE),
            context: ptr::null_mut(),
            shutdown: None,
            script_count: 0,
            reserved_flags: ABI_MODULE_EXTENSION_GODOT_API,
            get_script: None,
            reserved,
        }
    }

    #[test]
    fn module_value_capability_requires_an_exact_release_slot() {
        let mut api = module_api();
        let extensions = validate_module_extensions(&api).expect("Godot API extension");
        assert!(extensions.drop_value.is_none());
        assert_eq!(extensions.godot_api, ModuleGodotApi { major: 4, minor: 4 });

        api.reserved_flags |= ABI_MODULE_EXTENSION_OWNED_VALUES;
        assert!(
            validate_module_extensions(&api)
                .expect_err("owned values require a callback")
                .to_string()
                .contains("release callback")
        );

        api.reserved[MODULE_API_SLOT_DROP_VALUE] = drop_value as *const () as usize;
        assert!(
            validate_module_extensions(&api)
                .expect("owned value capability")
                .drop_value
                .is_some()
        );

        api.reserved[5] = 1;
        assert!(
            validate_module_extensions(&api)
                .expect_err("unknown slots must remain zero")
                .to_string()
                .contains("unknown extension slots")
        );
    }

    #[test]
    fn module_value_capability_rejects_unadvertised_and_unknown_extensions() {
        let mut api = module_api();
        api.reserved[MODULE_API_SLOT_DROP_VALUE] = drop_value as *const () as usize;
        assert!(
            validate_module_extensions(&api)
                .expect_err("unadvertised callback must fail")
                .to_string()
                .contains("without the owned-values capability")
        );

        api.reserved[MODULE_API_SLOT_DROP_VALUE] = 0;
        api.reserved_flags |= 1 << 31;
        assert!(
            validate_module_extensions(&api)
                .expect_err("unknown capability must fail")
                .to_string()
                .contains("unknown extension flags")
        );
    }

    #[test]
    fn module_godot_api_capability_is_required_and_bounded() {
        let mut api = module_api();
        api.reserved_flags &= !ABI_MODULE_EXTENSION_GODOT_API;
        assert!(
            validate_module_extensions(&api)
                .expect_err("missing Godot API target")
                .to_string()
                .contains("does not declare")
        );

        api.reserved_flags |= ABI_MODULE_EXTENSION_GODOT_API;
        api.reserved[MODULE_API_SLOT_GODOT_API_MINOR] = 8;
        assert!(
            validate_module_extensions(&api)
                .expect_err("unsupported Godot API target")
                .to_string()
                .contains("4.8")
        );

        api.reserved[MODULE_API_SLOT_GODOT_API_MAJOR] = 5;
        api.reserved[MODULE_API_SLOT_GODOT_API_MINOR] = 0;
        assert!(
            validate_module_extensions(&api)
                .expect_err("unsupported Godot major")
                .to_string()
                .contains("5.0")
        );
    }

    #[test]
    fn module_godot_api_must_not_exceed_the_running_engine() {
        let module = ModuleGodotApi { major: 4, minor: 6 };
        assert!(
            ensure_module_godot_api(
                module,
                EngineVersion {
                    major: 4,
                    minor: 6,
                    patch: 0,
                }
            )
            .is_ok()
        );
        assert!(
            ensure_module_godot_api(
                module,
                EngineVersion {
                    major: 4,
                    minor: 7,
                    patch: 1,
                }
            )
            .is_ok()
        );
        assert!(
            ensure_module_godot_api(
                module,
                EngineVersion {
                    major: 4,
                    minor: 5,
                    patch: 3,
                }
            )
            .expect_err("older Godot must reject newer module")
            .to_string()
            .contains("requires Godot 4.6")
        );
        assert!(
            ensure_module_godot_api(
                module,
                EngineVersion {
                    major: 5,
                    minor: 0,
                    patch: 0,
                }
            )
            .is_err()
        );
    }

    unsafe extern "C" fn create_state(output: *mut *mut c_void) -> AbiCallResult {
        if output.is_null() {
            return AbiCallResult::failure(AbiStatus::InvalidArgument, "null output");
        }
        let state = Box::into_raw(Box::new(7_u32)).cast();
        // SAFETY: Test caller supplies a writable output.
        unsafe { output.write(state) };
        AbiCallResult::OK
    }

    unsafe extern "C" fn drop_state(state: *mut c_void) {
        if !state.is_null() {
            // SAFETY: Test state was allocated by `create_state`.
            unsafe { drop(Box::from_raw(state.cast::<u32>())) };
        }
    }

    unsafe extern "C" fn ready(_state: *mut c_void) -> AbiCallResult {
        AbiCallResult::OK
    }

    unsafe extern "C" fn get_field_value(
        state: *mut c_void,
        _index: u32,
        output: *mut AbiValueV1,
    ) -> AbiCallResult {
        if state.is_null() || output.is_null() {
            return AbiCallResult::failure(AbiStatus::InvalidArgument, "null field access");
        }
        // SAFETY: The test pairs this callback with its u32 state.
        let value = unsafe { *state.cast::<u32>() };
        // SAFETY: Null was rejected.
        unsafe { output.write(AbiValueV1::from_f64(f64::from(value))) };
        AbiCallResult::OK
    }

    unsafe extern "C" fn set_field_value(
        state: *mut c_void,
        _index: u32,
        value: AbiValueV1,
    ) -> AbiCallResult {
        if state.is_null() || value.type_ != AbiValueType::F64 {
            return AbiCallResult::failure(AbiStatus::InvalidArgument, "invalid field value");
        }
        // SAFETY: The test pairs this callback with its u32 state.
        unsafe { *state.cast::<u32>() = f64::from_bits(value.payload[0]) as u32 };
        AbiCallResult::OK
    }

    fn script_extension_slots() -> [usize; 8] {
        script_extension_slots_for("uid://c2")
    }

    fn script_extension_slots_for(uid: &str) -> [usize; 8] {
        let uid = crate::resource_uid::parse_text(uid).expect("test UID");
        let words = godot_api::abi::encode_resource_uid_words(uid).expect("test UID words");
        [
            get_field_value as *const () as usize,
            set_field_value as *const () as usize,
            words[0],
            words[1],
            0,
            0,
            0,
            0,
        ]
    }

    fn inheritance_descriptor(
        path: &'static str,
        name: &'static str,
        uid: &'static str,
        base_script: Option<&'static str>,
    ) -> AbiScriptDescriptorV1 {
        let mut reserved = script_extension_slots_for(uid);
        let mut flags = ABI_SCRIPT_EXTENSION_FIELD_ACCESS | ABI_SCRIPT_EXTENSION_RESOURCE_UID;
        if let Some(base_script) = base_script {
            flags |= ABI_SCRIPT_EXTENSION_BASE_SCRIPT;
            reserved[6] = base_script.as_ptr() as usize;
            reserved[7] = base_script.len();
        }
        AbiScriptDescriptorV1 {
            struct_size: AbiScriptDescriptorV1::MINIMUM_SIZE,
            reserved_flags: flags,
            source_path: AbiByteSlice::from_static(path),
            name: AbiByteSlice::from_static(name),
            base: AbiByteSlice::from_static("Node"),
            tool: 0,
            reserved_bytes: [0; 7],
            field_count: 0,
            method_count: 0,
            get_field: Some(get_field),
            get_method: Some(get_method),
            create_state: Some(create_state),
            drop_state: Some(drop_state),
            lifecycle: AbiLifecycleTableV1::EMPTY,
            call_method: None,
            reserved,
        }
    }

    unsafe extern "C" fn get_field(_index: u32, output: *mut AbiFieldDescriptorV1) -> AbiStatus {
        const DEFAULT_SPEED: AbiValueV1 = AbiValueV1::from_f64(240.0);
        if output.is_null() {
            return AbiStatus::InvalidArgument;
        }
        let value = AbiFieldDescriptorV1 {
            struct_size: AbiFieldDescriptorV1::MINIMUM_SIZE,
            reserved_extension_flags: ABI_FIELD_EXTENSION_PROPERTY_SCHEMA,
            name: AbiByteSlice::from_static("speed"),
            rust_type: AbiByteSlice::from_static("f32"),
            kind: godot_api::abi::AbiFieldKind::Export,
            options: AbiByteSlice::from_static("gdrs-property-v1:0:0:"),
            default_value: AbiByteSlice::from_static("240.0"),
            has_default: 1,
            reserved_flags: [0; 3],
            reload: godot_api::abi::AbiReloadPolicy::Default,
            reserved: [
                AbiPropertyType::FLOAT.0 as usize,
                ABI_PROPERTY_HINT_NONE as usize,
                ABI_PROPERTY_USAGE_SCRIPT_DEFAULT as usize,
                (&DEFAULT_SPEED as *const AbiValueV1) as usize,
            ],
        };
        // SAFETY: Null was rejected.
        unsafe { output.write(value) };
        AbiStatus::Ok
    }

    unsafe extern "C" fn get_string_field(
        index: u32,
        output: *mut AbiFieldDescriptorV1,
    ) -> AbiStatus {
        if index != 0 || output.is_null() {
            return AbiStatus::InvalidArgument;
        }
        let value = AbiFieldDescriptorV1 {
            struct_size: AbiFieldDescriptorV1::MINIMUM_SIZE,
            reserved_extension_flags: ABI_FIELD_EXTENSION_PROPERTY_SCHEMA,
            name: AbiByteSlice::from_static("status_text"),
            rust_type: AbiByteSlice::from_static("String"),
            kind: godot_api::abi::AbiFieldKind::Export,
            options: AbiByteSlice::from_static("gdrs-property-v1:0:0:"),
            default_value: AbiByteSlice::from_static("等待构建"),
            has_default: 1,
            reserved_flags: [0; 3],
            reload: godot_api::abi::AbiReloadPolicy::Default,
            reserved: [
                AbiPropertyType::STRING.0 as usize,
                ABI_PROPERTY_HINT_MULTILINE_TEXT as usize,
                ABI_PROPERTY_USAGE_SCRIPT_DEFAULT as usize,
                0,
            ],
        };
        // SAFETY: Index and output were validated.
        unsafe { output.write(value) };
        AbiStatus::Ok
    }

    unsafe extern "C" fn get_string_name_field(
        index: u32,
        output: *mut AbiFieldDescriptorV1,
    ) -> AbiStatus {
        if index != 0 || output.is_null() {
            return AbiStatus::InvalidArgument;
        }
        let value = AbiFieldDescriptorV1 {
            struct_size: AbiFieldDescriptorV1::MINIMUM_SIZE,
            reserved_extension_flags: ABI_FIELD_EXTENSION_PROPERTY_SCHEMA,
            name: AbiByteSlice::from_static("status_name"),
            rust_type: AbiByteSlice::from_static("StringName"),
            kind: godot_api::abi::AbiFieldKind::Export,
            options: AbiByteSlice::from_static("gdrs-property-v1:0:0:"),
            default_value: AbiByteSlice::from_static("玩家/准备"),
            has_default: 1,
            reserved_flags: [0; 3],
            reload: godot_api::abi::AbiReloadPolicy::Default,
            reserved: [
                AbiPropertyType::STRING_NAME.0 as usize,
                ABI_PROPERTY_HINT_NONE as usize,
                ABI_PROPERTY_USAGE_SCRIPT_DEFAULT as usize,
                0,
            ],
        };
        // SAFETY: Index and output were validated.
        unsafe { output.write(value) };
        AbiStatus::Ok
    }

    unsafe extern "C" fn get_reload_field(
        index: u32,
        output: *mut AbiFieldDescriptorV1,
    ) -> AbiStatus {
        if index != 0 || output.is_null() {
            return AbiStatus::InvalidArgument;
        }
        let value = AbiFieldDescriptorV1 {
            struct_size: AbiFieldDescriptorV1::MINIMUM_SIZE,
            reserved_extension_flags: ABI_FIELD_EXTENSION_RELOAD_SCHEMA,
            name: AbiByteSlice::from_static("session_counter"),
            rust_type: AbiByteSlice::from_static("i64"),
            kind: godot_api::abi::AbiFieldKind::Plain,
            options: AbiByteSlice::EMPTY,
            default_value: AbiByteSlice::EMPTY,
            has_default: 0,
            reserved_flags: [0; 3],
            reload: godot_api::abi::AbiReloadPolicy::Persist,
            reserved: [AbiValueType::I64.0 as usize, 0, 0, 0],
        };
        // SAFETY: Index and output were validated.
        unsafe { output.write(value) };
        AbiStatus::Ok
    }

    unsafe extern "C" fn get_method(_index: u32, output: *mut AbiMethodDescriptorV1) -> AbiStatus {
        if output.is_null() {
            return AbiStatus::InvalidArgument;
        }
        let value = AbiMethodDescriptorV1 {
            struct_size: AbiMethodDescriptorV1::MINIMUM_SIZE,
            reserved_extension_flags: 0,
            id: 42,
            name: AbiByteSlice::from_static("_ready"),
            rust_signature: AbiByteSlice::from_static("fn _ready(&mut self)"),
            kind: godot_api::abi::AbiMethodKind::Lifecycle,
            lifecycle: godot_api::abi::AbiLifecycleSlot::Ready,
            receiver: godot_api::abi::AbiReceiverKind::Mutable,
            argument_count: 0,
            reserved_flags: 0,
            options: AbiByteSlice::EMPTY,
            argument_types: AbiValueTypeSlice::EMPTY,
            return_type: AbiValueType::NIL,
            reserved_value_flags: 0,
            arguments: AbiMethodArgumentSlice::EMPTY,
            rpc: AbiRpcConfigV1::NONE,
            reserved: [0; 4],
        };
        // SAFETY: Null was rejected.
        unsafe { output.write(value) };
        AbiStatus::Ok
    }

    #[test]
    fn descriptor_copy_owns_and_validates_module_text() {
        let descriptor = AbiScriptDescriptorV1 {
            struct_size: AbiScriptDescriptorV1::MINIMUM_SIZE,
            reserved_flags: ABI_SCRIPT_EXTENSION_FIELD_ACCESS | ABI_SCRIPT_EXTENSION_RESOURCE_UID,
            source_path: AbiByteSlice::from_static("res://player.rs"),
            name: AbiByteSlice::from_static("Player"),
            base: AbiByteSlice::from_static("Node2D"),
            tool: 0,
            reserved_bytes: [0; 7],
            field_count: 1,
            method_count: 1,
            get_field: Some(get_field),
            get_method: Some(get_method),
            create_state: Some(create_state),
            drop_state: Some(drop_state),
            lifecycle: AbiLifecycleTableV1 {
                ready: Some(ready),
                ..AbiLifecycleTableV1::EMPTY
            },
            call_method: None,
            reserved: script_extension_slots(),
        };

        let script = copy_script(0, descriptor).expect("valid descriptor");
        assert_eq!(script.source_path, "res://player.rs");
        assert_eq!(
            script.resource_uid,
            crate::resource_uid::parse_text("uid://c2").expect("test UID")
        );
        assert_eq!(script.name, "Player");
        assert_eq!(script.base, "Node2D");
        assert_eq!(script.fields[0].name, "speed");
        let property = script.fields[0]
            .property
            .as_ref()
            .expect("normalized property");
        assert_eq!(property.type_, AbiPropertyType::FLOAT);
        assert_eq!(property.hint, ABI_PROPERTY_HINT_NONE);
        assert_eq!(property.usage, ABI_PROPERTY_USAGE_SCRIPT_DEFAULT);
        assert_eq!(
            property.default_value,
            Some(HostValue::Scalar(AbiValueV1::from_f64(240.0)))
        );
        assert_eq!(script.methods[0].id, 42);
        assert_eq!(script.methods[0].name, "_ready");

        let mut missing_uid = descriptor;
        missing_uid.reserved_flags &= !ABI_SCRIPT_EXTENSION_RESOURCE_UID;
        missing_uid.reserved[2..4].fill(0);
        assert!(
            copy_script(0, missing_uid)
                .expect_err("current modules require a stable Resource UID")
                .to_string()
                .contains("Resource UID")
        );
    }

    #[test]
    fn reload_schema_is_loaded_and_method_schema_changes_are_rejected() {
        let descriptor = AbiScriptDescriptorV1 {
            struct_size: AbiScriptDescriptorV1::MINIMUM_SIZE,
            reserved_flags: ABI_SCRIPT_EXTENSION_FIELD_ACCESS | ABI_SCRIPT_EXTENSION_RESOURCE_UID,
            source_path: AbiByteSlice::from_static("res://reloadable.rs"),
            name: AbiByteSlice::from_static("Reloadable"),
            base: AbiByteSlice::from_static("Node"),
            tool: 0,
            reserved_bytes: [0; 7],
            field_count: 1,
            method_count: 1,
            get_field: Some(get_reload_field),
            get_method: Some(get_method),
            create_state: Some(create_state),
            drop_state: Some(drop_state),
            lifecycle: AbiLifecycleTableV1 {
                ready: Some(ready),
                ..AbiLifecycleTableV1::EMPTY
            },
            call_method: None,
            reserved: script_extension_slots(),
        };
        let current = copy_script(0, descriptor).expect("current descriptor");
        let mut candidate = copy_script(0, descriptor).expect("candidate descriptor");
        assert_eq!(current.fields[0].reload_value_type, Some(AbiValueType::I64));
        assert!(ensure_script_reload_compatible(&current, &candidate).is_ok());

        candidate.methods[0].rust_signature = "fn _ready(&mut self) -> bool".into();
        assert!(
            ensure_script_reload_compatible(&current, &candidate)
                .expect_err("method schema change")
                .contains("method schema")
        );
    }

    #[test]
    fn script_inheritance_requires_a_complete_acyclic_project_chain() {
        let base = copy_script(
            0,
            inheritance_descriptor("res://base.rs", "Base", "uid://c2", None),
        )
        .expect("base descriptor");
        let derived = copy_script(
            1,
            inheritance_descriptor(
                "res://derived.rs",
                "Derived",
                "uid://d2",
                Some("res://base.rs"),
            ),
        )
        .expect("derived descriptor");
        validate_script_inheritance(&[base, derived]).expect("valid inheritance");

        let missing = copy_script(
            0,
            inheritance_descriptor(
                "res://missing.rs",
                "Missing",
                "uid://e2",
                Some("res://absent.rs"),
            ),
        )
        .expect("missing-base descriptor");
        assert!(
            validate_script_inheritance(&[missing])
                .expect_err("missing base")
                .to_string()
                .contains("not part of the project module")
        );

        let first = copy_script(
            0,
            inheritance_descriptor(
                "res://first.rs",
                "First",
                "uid://b2",
                Some("res://second.rs"),
            ),
        )
        .expect("first cycle descriptor");
        let second = copy_script(
            1,
            inheritance_descriptor(
                "res://second.rs",
                "Second",
                "uid://c2",
                Some("res://first.rs"),
            ),
        )
        .expect("second cycle descriptor");
        assert!(
            validate_script_inheritance(&[first, second])
                .expect_err("inheritance cycle")
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn string_property_defaults_are_copied_into_host_storage() {
        let descriptor = AbiScriptDescriptorV1 {
            struct_size: AbiScriptDescriptorV1::MINIMUM_SIZE,
            reserved_flags: ABI_SCRIPT_EXTENSION_FIELD_ACCESS | ABI_SCRIPT_EXTENSION_RESOURCE_UID,
            source_path: AbiByteSlice::from_static("res://status.rs"),
            name: AbiByteSlice::from_static("Status"),
            base: AbiByteSlice::from_static("Node"),
            tool: 0,
            reserved_bytes: [0; 7],
            field_count: 1,
            method_count: 0,
            get_field: Some(get_string_field),
            get_method: Some(get_method),
            create_state: Some(create_state),
            drop_state: Some(drop_state),
            lifecycle: AbiLifecycleTableV1::EMPTY,
            call_method: None,
            reserved: script_extension_slots(),
        };
        let script = copy_script(0, descriptor).expect("String property descriptor");
        let property = script.fields[0]
            .property
            .as_ref()
            .expect("normalized String property");
        assert_eq!(property.type_, AbiPropertyType::STRING);
        assert_eq!(property.hint, ABI_PROPERTY_HINT_MULTILINE_TEXT);
        assert_eq!(
            property.default_value,
            Some(HostValue::String(String::from("等待构建")))
        );

        let string_name_descriptor = AbiScriptDescriptorV1 {
            source_path: AbiByteSlice::from_static("res://status_name.rs"),
            name: AbiByteSlice::from_static("StatusName"),
            get_field: Some(get_string_name_field),
            ..descriptor
        };
        let script =
            copy_script(0, string_name_descriptor).expect("StringName property descriptor");
        let property = script.fields[0]
            .property
            .as_ref()
            .expect("normalized StringName property");
        assert_eq!(property.type_, AbiPropertyType::STRING_NAME);
        assert_eq!(property.hint, ABI_PROPERTY_HINT_NONE);
        assert_eq!(
            property.default_value,
            Some(HostValue::StringName(String::from("玩家/准备")))
        );
    }

    #[test]
    fn invalid_paths_and_duplicate_method_ids_are_rejected() {
        let mut descriptor = AbiScriptDescriptorV1 {
            struct_size: AbiScriptDescriptorV1::MINIMUM_SIZE,
            reserved_flags: ABI_SCRIPT_EXTENSION_FIELD_ACCESS | ABI_SCRIPT_EXTENSION_RESOURCE_UID,
            source_path: AbiByteSlice::from_static("/tmp/player.rs"),
            name: AbiByteSlice::from_static("Player"),
            base: AbiByteSlice::from_static("Node"),
            tool: 0,
            reserved_bytes: [0; 7],
            field_count: 0,
            method_count: 0,
            get_field: Some(get_field),
            get_method: Some(get_method),
            create_state: Some(create_state),
            drop_state: Some(drop_state),
            lifecycle: AbiLifecycleTableV1::EMPTY,
            call_method: None,
            reserved: script_extension_slots(),
        };
        assert!(
            copy_script(0, descriptor)
                .expect_err("absolute filesystem path must fail")
                .to_string()
                .contains("res://")
        );

        descriptor.source_path = AbiByteSlice::from_static("res://player.rs");
        descriptor.method_count = 2;
        descriptor.lifecycle.ready = Some(ready);
        assert!(
            copy_script(0, descriptor)
                .expect_err("duplicate generated methods must fail")
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn invalid_property_schema_is_rejected_before_state_creation() {
        assert_eq!(
            parse_property_options(0, "checkpoints", "gdrs-property-v2:4:2:7:Data5:Vector2")
                .expect("valid typed Array property payload"),
            (
                Some(String::from("Data")),
                String::from("5:"),
                Some(String::from("Vector2")),
            )
        );
        assert_eq!(
            parse_property_options(0, "speed", "gdrs-property-v1:4:5:Datam/s²")
                .expect("legacy property payload remains supported"),
            (Some(String::from("Data")), String::from("m/s²"), None)
        );
        assert!(
            parse_property_options(0, "speed", "unversioned")
                .expect_err("unversioned property payload must fail")
                .to_string()
                .contains("payload")
        );
        assert!(
            parse_property_options(0, "speed", "gdrs-property-v1:2:1:a")
                .expect_err("incorrect payload lengths must fail")
                .to_string()
                .contains("length")
        );
        assert!(
            parse_property_options(0, "checkpoints", "gdrs-property-v2:0:2:8:5:Bad/Type")
                .expect_err("invalid typed Array element must fail")
                .to_string()
                .contains("element")
        );
        assert!(
            validate_property_schema(
                0,
                "speed",
                AbiPropertyType::FLOAT,
                ABI_PROPERTY_HINT_FLAGS,
                ABI_PROPERTY_USAGE_SCRIPT_DEFAULT,
            )
            .expect_err("float flags hint must fail")
            .to_string()
            .contains("hint")
        );
        assert!(
            validate_property_default(
                0,
                "speed",
                AbiPropertyType::FLOAT,
                AbiValueV1::from_i64(240),
            )
            .expect_err("mismatched typed default must fail")
            .to_string()
            .contains("default")
        );
        assert!(
            validate_property_schema(
                0,
                "accent_color",
                AbiPropertyType::COLOR,
                ABI_PROPERTY_HINT_COLOR_NO_ALPHA,
                ABI_PROPERTY_USAGE_SCRIPT_DEFAULT,
            )
            .is_ok()
        );
        assert!(
            validate_property_schema(
                0,
                "speed",
                AbiPropertyType::FLOAT,
                ABI_PROPERTY_HINT_COLOR_NO_ALPHA,
                ABI_PROPERTY_USAGE_SCRIPT_DEFAULT,
            )
            .expect_err("Color hint on a float must fail")
            .to_string()
            .contains("hint")
        );
        assert!(
            validate_property_default(
                0,
                "target_position",
                AbiPropertyType::VECTOR3,
                AbiValueV1::from_vector3(4.0, 8.0, 12.0),
            )
            .is_ok()
        );
    }

    #[test]
    fn godot_integer_schema_generates_friendly_typed_inspector_options() {
        let enum_options = [
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("PROCESS_MODE_INHERIT"),
                raw: 0,
            },
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("PROCESS_MODE_WHEN_PAUSED"),
                raw: 2,
            },
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("PROCESS_MODE_ALWAYS"),
                raw: 3,
            },
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("PROCESS_MODE_MAX"),
                raw: 4,
            },
        ];
        let schema = copy_godot_integer_property(
            0,
            "worker_mode",
            "Behavior",
            [
                1,
                enum_options.as_ptr() as usize,
                enum_options.len(),
                godot_integer_default as *const () as usize,
            ],
        )
        .expect("valid generated enum schema");
        assert_eq!(schema.type_, AbiPropertyType::INT);
        assert_eq!(schema.value_type, AbiValueType::I64);
        assert_eq!(schema.hint, ABI_PROPERTY_HINT_ENUM);
        assert_eq!(schema.hint_string, "Inherit:0,When Paused:2,Always:3");
        assert_eq!(schema.group.as_deref(), Some("Behavior"));
        assert_eq!(
            schema.default_value,
            Some(HostValue::Scalar(AbiValueV1::from_i64(3)))
        );

        let flag_options = [
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("FLAG_PROCESS_THREAD_MESSAGES"),
                raw: 1,
            },
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("FLAG_PROCESS_THREAD_MESSAGES_PHYSICS"),
                raw: 2,
            },
            AbiGodotIntegerOptionV1 {
                name: AbiByteSlice::from_static("FLAG_PROCESS_THREAD_MESSAGES_ALL"),
                raw: 3,
            },
        ];
        let schema = copy_godot_integer_property(
            0,
            "thread_messages",
            "",
            [
                0,
                flag_options.as_ptr() as usize,
                flag_options.len(),
                godot_integer_default as *const () as usize,
            ],
        )
        .expect("valid generated bitfield schema");
        assert_eq!(schema.value_type, AbiValueType::U64);
        assert_eq!(schema.hint, ABI_PROPERTY_HINT_FLAGS);
        assert_eq!(
            schema.hint_string,
            "Messages:1,Messages Physics:2,Messages All:3"
        );
        assert_eq!(
            schema.default_value,
            Some(HostValue::Scalar(AbiValueV1::from_u64(3)))
        );
    }

    #[test]
    fn node_schema_is_deep_copied_and_validated() {
        let mut path = String::from("Target");
        let mut class_name = String::from("Sprite2D");
        let node = copy_node_schema(
            0,
            "sprite",
            [
                path.as_ptr() as usize,
                path.len(),
                class_name.as_ptr() as usize,
                godot_api::abi::encode_node_field_class(class_name.len(), true)
                    .expect("class length"),
            ],
        )
        .expect("valid node schema");
        path.replace_range(.., "Source");
        class_name.replace_range(.., "Camera2D");
        assert_eq!(node.path, "Target");
        assert_eq!(node.class_name, "Sprite2D");
        assert!(node.optional);

        let invalid_path = "Bad\0Path";
        assert!(
            copy_node_schema(
                0,
                "sprite",
                [
                    invalid_path.as_ptr() as usize,
                    invalid_path.len(),
                    class_name.as_ptr() as usize,
                    godot_api::abi::encode_node_field_class(class_name.len(), false)
                        .expect("class length"),
                ],
            )
            .expect_err("embedded nul must fail")
            .to_string()
            .contains("invalid path")
        );
    }

    #[test]
    fn signal_schema_is_deep_copied_and_validated() {
        const ARGUMENTS: [AbiSignalArgumentDescriptorV1; 2] = [
            AbiSignalArgumentDescriptorV1 {
                name: AbiByteSlice::from_static("old_value"),
                type_: AbiValueType::I64,
                reserved_flags: 0,
            },
            AbiSignalArgumentDescriptorV1 {
                name: AbiByteSlice::from_static("new_value"),
                type_: AbiValueType::I64,
                reserved_flags: 0,
            },
        ];
        let signal = copy_signal_schema(0, "changed", ARGUMENTS.as_ptr() as usize, ARGUMENTS.len())
            .expect("valid signal schema");
        assert_eq!(signal.arguments.len(), 2);
        assert_eq!(signal.arguments[0].name, "old_value");
        assert_eq!(signal.arguments[1].type_, AbiValueType::I64);

        const DUPLICATE: [AbiSignalArgumentDescriptorV1; 2] = [ARGUMENTS[0], ARGUMENTS[0]];
        assert!(
            copy_signal_schema(0, "changed", DUPLICATE.as_ptr() as usize, DUPLICATE.len(),)
                .expect_err("duplicate signal arguments must fail")
                .to_string()
                .contains("duplicate")
        );
        assert!(
            copy_signal_schema(
                0,
                "changed",
                ARGUMENTS.as_ptr() as usize,
                MAX_SIGNAL_ARGUMENT_COUNT + 1,
            )
            .expect_err("oversized signal schemas must fail")
            .to_string()
            .contains("too many")
        );
    }

    #[test]
    fn method_object_classes_are_deep_copied_and_type_checked() {
        const CLASSES: [AbiByteSlice; 2] = [AbiByteSlice::from_static("Node"), AbiByteSlice::EMPTY];
        let types = [AbiValueType::OBJECT_ID, AbiValueType::I64];
        let classes = copy_method_argument_classes(
            0,
            3,
            ABI_METHOD_EXTENSION_ARGUMENT_CLASSES,
            [CLASSES.as_ptr() as usize, CLASSES.len(), 0, 0],
            &types,
        )
        .expect("valid object class metadata");
        assert_eq!(classes, [Some("Node".to_owned()), None]);

        const TYPED_ARRAY_CLASSES: [AbiByteSlice; 2] = [
            AbiByteSlice::from_static("Vector2"),
            AbiByteSlice::from_static("Image"),
        ];
        let typed_array_types = [AbiValueType::ARRAY, AbiValueType::ARRAY];
        assert_eq!(
            copy_method_argument_classes(
                0,
                3,
                ABI_METHOD_EXTENSION_ARGUMENT_CLASSES,
                [
                    TYPED_ARRAY_CLASSES.as_ptr() as usize,
                    TYPED_ARRAY_CLASSES.len(),
                    0,
                    0,
                ],
                &typed_array_types,
            )
            .expect("valid typed Array metadata"),
            [Some("Vector2".to_owned()), Some("Image".to_owned())]
        );

        const WRONG: [AbiByteSlice; 2] = [AbiByteSlice::EMPTY, AbiByteSlice::from_static("Node")];
        assert!(
            copy_method_argument_classes(
                0,
                3,
                ABI_METHOD_EXTENSION_ARGUMENT_CLASSES,
                [WRONG.as_ptr() as usize, WRONG.len(), 0, 0],
                &types,
            )
            .expect_err("class metadata must match object positions")
            .to_string()
            .contains("Godot class")
        );

        let mut return_class = String::from("Node");
        let copied = copy_method_return_class(
            0,
            3,
            ABI_METHOD_EXTENSION_RETURN_CLASS,
            [0, 0, return_class.as_ptr() as usize, return_class.len()],
            AbiValueType::OBJECT_ID,
        )
        .expect("valid object return class metadata");
        return_class.replace_range(.., "Nope");
        assert_eq!(copied.as_deref(), Some("Node"));
        assert_eq!(
            copy_method_return_class(
                0,
                3,
                ABI_METHOD_EXTENSION_RETURN_CLASS,
                [0, 0, b"Vector2".as_ptr() as usize, 7],
                AbiValueType::ARRAY,
            )
            .expect("typed Array return metadata")
            .as_deref(),
            Some("Vector2")
        );
        assert!(
            copy_method_return_class(
                0,
                3,
                ABI_METHOD_EXTENSION_RETURN_CLASS,
                [0, 0, b"not a type".as_ptr() as usize, 10],
                AbiValueType::ARRAY,
            )
            .expect_err("invalid typed Array metadata must fail")
            .to_string()
            .contains("invalid type metadata")
        );
        assert!(
            copy_method_return_class(0, 3, 0, [0; 4], AbiValueType::OBJECT_ID)
                .expect_err("object return values require class metadata")
                .to_string()
                .contains("no Godot class metadata")
        );
        assert!(
            copy_method_return_class(
                0,
                3,
                ABI_METHOD_EXTENSION_RETURN_CLASS,
                [0, 0, b"Node".as_ptr() as usize, 4],
                AbiValueType::I64,
            )
            .expect_err("scalar return values reject class metadata")
            .to_string()
            .contains("non-object")
        );
    }

    #[test]
    fn versioned_method_schema_validates_defaults_and_varargs() {
        let classes = [AbiByteSlice::from_static("Node"), AbiByteSlice::EMPTY];
        let defaults = [Some(
            method_default as unsafe extern "C" fn(*mut AbiValueV1) -> AbiCallResult,
        )];
        let mut extension = AbiMethodExtensionsV1 {
            struct_size: AbiMethodExtensionsV1::MINIMUM_SIZE,
            reserved_flags: ABI_METHOD_SCHEMA_VARARG,
            argument_classes: AbiByteSliceSlice {
                ptr: classes.as_ptr(),
                len: classes.len(),
            },
            return_class: AbiByteSlice::from_static("Node"),
            default_arguments: godot_api::abi::AbiMethodDefaultFnSlice {
                ptr: defaults.as_ptr(),
                len: defaults.len(),
            },
            reserved: [0; 4],
        };
        let copied = copy_method_extensions(
            0,
            4,
            ABI_METHOD_EXTENSION_SCHEMA_V1,
            [
                ptr::from_ref(&extension) as usize,
                AbiMethodExtensionsV1::MINIMUM_SIZE as usize,
                0,
                0,
            ],
            &[AbiValueType::OBJECT_ID, AbiValueType::I64],
            AbiValueType::OBJECT_ID,
        )
        .expect("valid versioned method schema");
        assert_eq!(copied.argument_classes, [Some("Node".to_owned()), None]);
        assert_eq!(copied.return_class.as_deref(), Some("Node"));
        assert_eq!(copied.default_arguments.len(), 1);
        assert!(copied.vararg);

        extension.default_arguments.len = 3;
        assert!(
            copy_method_extensions(
                0,
                4,
                ABI_METHOD_EXTENSION_SCHEMA_V1,
                [
                    ptr::from_ref(&extension) as usize,
                    AbiMethodExtensionsV1::MINIMUM_SIZE as usize,
                    0,
                    0,
                ],
                &[AbiValueType::OBJECT_ID, AbiValueType::I64],
                AbiValueType::OBJECT_ID,
            )
            .expect_err("defaults cannot exceed fixed arguments")
            .to_string()
            .contains("too many default")
        );

        extension.default_arguments.len = 1;
        extension.reserved_flags |= 1 << 31;
        assert!(
            copy_method_extensions(
                0,
                4,
                ABI_METHOD_EXTENSION_SCHEMA_V1,
                [
                    ptr::from_ref(&extension) as usize,
                    AbiMethodExtensionsV1::MINIMUM_SIZE as usize,
                    0,
                    0,
                ],
                &[AbiValueType::OBJECT_ID, AbiValueType::I64],
                AbiValueType::OBJECT_ID,
            )
            .expect_err("unknown method schema flags must fail")
            .to_string()
            .contains("incompatible versioned metadata")
        );
    }
}
