use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use godot_api::abi::AbiMethodKind;
use godot_api::{
    GDExtensionClassInstancePtr, GDExtensionConstStringNamePtr, GDExtensionConstTypePtr,
    GDExtensionTypePtr, GDExtensionVariantType,
};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock, Weak};

use crate::godot_metadata;
use crate::interface::EngineInterface;
use crate::module_loader::{self, ModuleField, ModuleMethod, ModuleScript};
use crate::registry::{ClassRegistry, ClassSpec, RegisteredClassId, VirtualMethodSpec};
use crate::script_instance;
use crate::string_name::{OwnedStringName, StaticStringName};
use crate::value::{
    LocalGodotString, read_utf8_string, write_default_builtin, write_latin1_string,
    write_nil_variant, write_string_name, write_utf8_string, write_utf8_string_name,
};
use crate::variant_codec::VariantCodec;

static LANGUAGE_OBJECT: AtomicUsize = AtomicUsize::new(0);
static SCRIPTS_BY_OBJECT: OnceLock<RwLock<HashMap<usize, Weak<RustScript>>>> = OnceLock::new();

pub(crate) struct RustScript {
    interface: EngineInterface,
    object: usize,
    path: RwLock<String>,
    resource_uid: AtomicI64,
    source: RwLock<String>,
    has_source: AtomicBool,
    enter_tree_name: StaticStringName,
    ready_name: StaticStringName,
    process_name: StaticStringName,
    physics_process_name: StaticStringName,
    input_name: StaticStringName,
    unhandled_input_name: StaticStringName,
    exit_tree_name: StaticStringName,
}

pub(crate) fn register_class(registry: &mut ClassRegistry) -> RegisteredClassId {
    registry.register(ClassSpec {
        name: c"GodotRustScript",
        parent: c"ScriptExtension",
        factory: create_script_instance,
        dropper: drop_script_instance,
        virtual_methods: SCRIPT_VIRTUAL_METHODS,
    })
}

pub(crate) fn set_language_object(object: *mut c_void) {
    LANGUAGE_OBJECT.store(object as usize, Ordering::Release);
}

pub(crate) fn clear_language_object() {
    LANGUAGE_OBJECT.store(0, Ordering::Release);
}

fn create_script_instance(interface: EngineInterface, object: *mut c_void) -> *mut c_void {
    let script = Arc::new(RustScript {
        interface,
        object: object as usize,
        path: RwLock::new(String::new()),
        resource_uid: AtomicI64::new(crate::resource_uid::INVALID_RESOURCE_UID),
        source: RwLock::new(String::new()),
        has_source: AtomicBool::new(false),
        enter_tree_name: StaticStringName::new(interface, c"_enter_tree"),
        ready_name: StaticStringName::new(interface, c"_ready"),
        process_name: StaticStringName::new(interface, c"_process"),
        physics_process_name: StaticStringName::new(interface, c"_physics_process"),
        input_name: StaticStringName::new(interface, c"_input"),
        unhandled_input_name: StaticStringName::new(interface, c"_unhandled_input"),
        exit_tree_name: StaticStringName::new(interface, c"_exit_tree"),
    });
    if let Ok(mut scripts) = scripts_by_object().write() {
        scripts.insert(object as usize, Arc::downgrade(&script));
    }
    Arc::into_raw(script).cast_mut().cast()
}

unsafe fn drop_script_instance(instance: *mut c_void) {
    // SAFETY: The pointer comes from `Arc::into_raw` in
    // `create_script_instance`, and ClassDB frees it exactly once.
    let script = unsafe { Arc::from_raw(instance.cast::<RustScript>()) };
    if let Ok(mut scripts) = scripts_by_object().write() {
        scripts.remove(&script.object);
    }
    drop(script);
}

fn script(instance: GDExtensionClassInstancePtr) -> Option<&'static RustScript> {
    if instance.is_null() {
        return None;
    }
    // SAFETY: ClassDB supplies the matching live Rust extension instance.
    Some(unsafe { &*instance.cast::<RustScript>() })
}

fn scripts_by_object() -> &'static RwLock<HashMap<usize, Weak<RustScript>>> {
    SCRIPTS_BY_OBJECT.get_or_init(|| RwLock::new(HashMap::new()))
}

fn script_object_by_path(path: &str) -> Option<*mut c_void> {
    scripts_by_object()
        .read()
        .ok()?
        .values()
        .filter_map(Weak::upgrade)
        .find(|script| script_path(script).as_deref() == Some(path))
        .map(|script| script.object as *mut c_void)
}

fn load_script_ref(
    interface: EngineInterface,
    path: &str,
) -> Option<crate::engine_call::value::NativeGodotRef> {
    let get_singleton = interface.global_get_singleton?;
    let get_method = interface.classdb_get_method_bind?;
    let ptrcall = interface.object_method_bind_ptrcall?;
    let class = StaticStringName::new(interface, c"ResourceLoader");
    let method = StaticStringName::new(interface, c"load");
    // SAFETY: ResourceLoader is an official singleton in every supported
    // Godot version.
    let loader = unsafe { get_singleton(class.as_ptr()) };
    // SAFETY: Name and hash match ResourceLoader.load in Godot 4.4.
    let load = unsafe { get_method(class.as_ptr(), method.as_ptr(), 3_358_495_409) };
    if loader.is_null() || load.is_null() {
        return None;
    }
    let path = LocalGodotString::new_utf8(interface, path)?;
    let type_hint = LocalGodotString::new(interface, c"Script")?;
    let cache_mode = 1_i64;
    let arguments: [GDExtensionConstTypePtr; 3] = [
        path.as_ptr(),
        type_hint.as_ptr(),
        ptr::from_ref(&cache_mode).cast(),
    ];
    let mut output = crate::engine_call::value::NativeGodotRef::empty(interface).ok()?;
    // SAFETY: The bind, singleton, arguments, and Ref<Resource> output match
    // the official ResourceLoader.load ptrcall contract.
    unsafe { ptrcall(load, loader, arguments.as_ptr(), output.as_mut_ptr()) };
    (!output.object().is_null()).then_some(output)
}

pub(crate) fn initialize_loaded_script(
    object: *mut c_void,
    path: &str,
    resource_uid: i64,
    source: &str,
) -> bool {
    let script = scripts_by_object()
        .read()
        .ok()
        .and_then(|scripts| scripts.get(&(object as usize)).and_then(Weak::upgrade));
    let Some(script) = script else {
        return false;
    };
    let Ok(mut target_path) = script.path.write() else {
        return false;
    };
    let Ok(mut target_source) = script.source.write() else {
        return false;
    };
    path.clone_into(&mut target_path);
    source.clone_into(&mut target_source);
    script.resource_uid.store(resource_uid, Ordering::Release);
    script.has_source.store(true, Ordering::Release);
    true
}

pub(crate) fn initialize_new_script(object: *mut c_void, source: &str) -> bool {
    initialize_loaded_script(
        object,
        "",
        crate::resource_uid::INVALID_RESOURCE_UID,
        source,
    )
}

pub(crate) fn source_for_object(object: *mut c_void) -> Option<String> {
    let script = scripts_by_object()
        .read()
        .ok()
        .and_then(|scripts| scripts.get(&(object as usize)).and_then(Weak::upgrade))?;
    script.source.read().ok().map(|source| source.clone())
}

pub(crate) fn path_for_object(object: *mut c_void) -> Option<String> {
    let script = scripts_by_object()
        .read()
        .ok()
        .and_then(|scripts| scripts.get(&(object as usize)).and_then(Weak::upgrade))?;
    script_path(&script)
}

pub(crate) fn source_for_path(path: &str) -> Option<String> {
    scripts_by_object()
        .read()
        .ok()?
        .values()
        .filter_map(Weak::upgrade)
        .find(|script| script_path(script).as_deref() == Some(path))
        .and_then(|script| script.source.read().ok().map(|source| source.clone()))
}

pub(crate) fn update_source_for_path(path: &str, source: &str) -> usize {
    let scripts = scripts_by_object()
        .read()
        .ok()
        .map(|scripts| {
            scripts
                .values()
                .filter_map(Weak::upgrade)
                .filter(|script| script_path(script).as_deref() == Some(path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    scripts
        .iter()
        .filter(|script| {
            script.source.write().is_ok_and(|mut target| {
                source.clone_into(&mut target);
                script.has_source.store(true, Ordering::Release);
                true
            })
        })
        .count()
}

pub(crate) fn set_saved_path(object: *mut c_void, path: &str) -> bool {
    let script = scripts_by_object()
        .read()
        .ok()
        .and_then(|scripts| scripts.get(&(object as usize)).and_then(Weak::upgrade));
    let Some(script) = script else {
        return false;
    };
    let Ok(mut target) = script.path.write() else {
        return false;
    };
    path.clone_into(&mut target);
    true
}

pub(crate) fn set_resource_uid_for_path(path: &str, resource_uid: i64) {
    let scripts = scripts_by_object()
        .read()
        .ok()
        .map(|scripts| {
            scripts
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for script in scripts {
        let matches = script
            .path
            .read()
            .ok()
            .is_some_and(|script_path| script_path.as_str() == path);
        if matches {
            script.resource_uid.store(resource_uid, Ordering::Release);
        }
    }
}

fn script_path(script: &RustScript) -> Option<String> {
    script
        .path
        .read()
        .ok()
        .filter(|path| !path.is_empty())
        .map(|path| path.clone())
}

fn reload_source_from_disk(script: &RustScript) -> Result<(), String> {
    let resource_path =
        script_path(script).ok_or_else(|| "Rust script has no resource path".to_owned())?;
    let relative = resource_path
        .strip_prefix("res://")
        .ok_or_else(|| "Rust script path is outside res://".to_owned())?;
    let root = crate::last_known_good::globalize_project_root(script.interface)?
        .canonicalize()
        .map_err(|error| format!("could not resolve Godot project root: {error}"))?;
    let path = root.join(relative);
    let path = path
        .canonicalize()
        .map_err(|error| format!("could not resolve Rust script `{resource_path}`: {error}"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(format!(
            "Rust script path escapes the project or is not a file: {resource_path}"
        ));
    }
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read Rust script `{resource_path}`: {error}"))?;
    let mut target = script
        .source
        .write()
        .map_err(|_| "Rust script source lock is unavailable".to_owned())?;
    source.clone_into(&mut target);
    script.has_source.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn reload_all_sources_from_disk() -> usize {
    let scripts = scripts_by_object()
        .read()
        .ok()
        .map(|scripts| {
            scripts
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    scripts
        .iter()
        .filter(|script| reload_source_from_disk(script).is_ok())
        .count()
}

fn compiled_script(script: &RustScript) -> Option<ModuleScript> {
    let path = script_path(script)?;
    let resource_uid = script.resource_uid.load(Ordering::Acquire);
    if resource_uid >= 0 {
        return module_loader::active_script_by_uid(resource_uid);
    }
    module_loader::active_script(&path)
}

fn source_attributes(
    script: &RustScript,
    compiled: &ModuleScript,
) -> crate::rust_source::ScriptSourceAttributes {
    script.source.read().ok().map_or_else(
        crate::rust_source::ScriptSourceAttributes::default,
        |source| crate::rust_source::script_source_attributes(&source, compiled.name()),
    )
}

unsafe extern "C" fn editor_can_reload_from_file(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_bool(result, true);
}

unsafe extern "C" fn has_source_code(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let value = script(instance)
        .map(|script| script.has_source.load(Ordering::Acquire))
        .unwrap_or(false);
    write_bool(result, value);
}

unsafe extern "C" fn get_source_code(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Ok(source) = script.source.read() else {
        write_latin1_string(script.interface, result, c"");
        return;
    };
    if !write_utf8_string(script.interface, result, &source) {
        write_latin1_string(script.interface, result, c"");
    }
}

unsafe extern "C" fn set_source_code(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    if arguments.is_null() {
        return;
    }
    // SAFETY: This virtual has exactly one official String argument.
    let source = unsafe { *arguments };
    let Ok(source) = read_utf8_string(script.interface, source) else {
        return;
    };
    let Ok(mut target) = script.source.write() else {
        return;
    };
    *target = source;
    script.has_source.store(true, Ordering::Release);
}

unsafe extern "C" fn reload(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let error = script(instance)
        .map(|script| {
            if script_path(script).is_some() {
                reload_source_from_disk(script).map_or(19_i64, |()| 0_i64)
            } else if script.has_source.load(Ordering::Acquire) {
                0_i64
            } else {
                3_i64 // ERR_UNCONFIGURED
            }
        })
        .unwrap_or(3_i64);
    write_i64(result, error);
}

unsafe extern "C" fn is_valid(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let valid = script(instance)
        .map(|script| script.has_source.load(Ordering::Acquire))
        .unwrap_or(false);
    write_bool(result, valid);
}

unsafe extern "C" fn can_instantiate(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let valid = script(instance)
        .map(|script| {
            script.has_source.load(Ordering::Acquire)
                && LANGUAGE_OBJECT.load(Ordering::Acquire) != 0
                && compiled_script(script)
                    .is_some_and(|compiled| !source_attributes(script, &compiled).abstract_)
        })
        .unwrap_or(false);
    write_bool(result, valid);
}

unsafe extern "C" fn get_language(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    if !result.is_null() {
        let language = LANGUAGE_OBJECT.load(Ordering::Acquire) as *mut c_void;
        // SAFETY: Godot encodes Object returns as one object pointer.
        unsafe { result.cast::<*mut c_void>().write(language) };
    }
}

unsafe extern "C" fn get_instance_base_type(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    if let Some(script) = script(instance) {
        let Some(compiled) = compiled_script(script) else {
            write_string_name(script.interface, result, c"");
            return;
        };
        if !write_utf8_string_name(script.interface, result, compiled.base()) {
            write_string_name(script.interface, result, c"");
        }
    }
}

unsafe extern "C" fn get_global_name(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Some(name) =
        compiled_script(script).and_then(|script| script.global_name().map(str::to_owned))
    else {
        write_string_name(script.interface, result, c"");
        return;
    };
    if !write_utf8_string_name(script.interface, result, &name) {
        write_string_name(script.interface, result, c"");
    }
}

unsafe extern "C" fn get_class_icon_path(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let icon_path = compiled_script(script)
        .map(|compiled| source_attributes(script, &compiled))
        .and_then(|attributes| attributes.icon_path);
    if !write_utf8_string(script.interface, result, icon_path.as_deref().unwrap_or("")) {
        write_latin1_string(script.interface, result, c"");
    }
}

unsafe extern "C" fn is_tool(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let value = script(instance)
        .and_then(compiled_script)
        .is_some_and(|compiled| compiled.is_tool());
    write_bool(result, value);
}

unsafe extern "C" fn is_abstract(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let value = script(instance)
        .and_then(|script| {
            compiled_script(script).map(|compiled| source_attributes(script, &compiled).abstract_)
        })
        .unwrap_or(false);
    write_bool(result, value);
}

unsafe extern "C" fn get_documentation(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let (Some(compiled), Ok(source)) = (compiled_script(script), script.source.read()) else {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
        return;
    };
    if !godot_metadata::write_script_documentation(script.interface, result, &compiled, &source) {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
    }
}

unsafe extern "C" fn has_method(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        write_bool(result, false);
        return;
    };
    if arguments.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: This virtual's first argument is StringName.
    let name = unsafe { *arguments };
    let Some(compiled) = compiled_script(script) else {
        write_bool(result, false);
        return;
    };
    write_bool(
        result,
        lifecycle_argument_count(script, &compiled, name).is_some(),
    );
}

unsafe extern "C" fn has_static_method(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let value = script(instance).is_some_and(|script| {
        if arguments.is_null() {
            return false;
        }
        // SAFETY: This virtual's first argument is StringName.
        let name = unsafe { *arguments };
        compiled_script(script)
            .and_then(|compiled| find_method(script, &compiled, name))
            .is_some_and(|method| method.receiver() == godot_api::abi::AbiReceiverKind::Static)
    });
    write_bool(result, value);
}

unsafe extern "C" fn script_method_argument_count(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        write_i64(result, -1);
        return;
    };
    if arguments.is_null() {
        write_i64(result, -1);
        return;
    }
    // SAFETY: This virtual's first argument is StringName.
    let name = unsafe { *arguments };
    let count = compiled_script(script)
        .and_then(|compiled| lifecycle_argument_count(script, &compiled, name))
        .unwrap_or(-1);
    write_i64(result, count);
}

unsafe extern "C" fn get_rpc_config(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Some(compiled) = compiled_script(script) else {
        write_nil_variant(script.interface, result);
        return;
    };
    if !godot_metadata::write_rpc_config(script.interface, &compiled, result) {
        write_nil_variant(script.interface, result);
    }
}

unsafe extern "C" fn get_script_method_list(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Some(compiled) = compiled_script(script) else {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
        return;
    };
    if !godot_metadata::write_method_list(script.interface, &compiled, result) {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
    }
}

unsafe extern "C" fn get_method_info(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let method = if arguments.is_null() {
        None
    } else {
        // SAFETY: This virtual's first argument is StringName.
        let name = unsafe { *arguments };
        compiled_script(script).and_then(|compiled| find_method(script, &compiled, name))
    };
    let wrote_method = method
        .as_ref()
        .is_some_and(|method| godot_metadata::write_method_info(script.interface, method, result));
    if !wrote_method {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn get_script_property_list(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Some(compiled) = compiled_script(script) else {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
        return;
    };
    if !godot_metadata::write_property_list(script.interface, &compiled, result) {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
    }
}

unsafe extern "C" fn update_exports(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let (Ok(method), Some(ptrcall)) = (
        crate::runtime::resolve_method(
            script.interface,
            c"Resource",
            c"emit_changed",
            3_218_959_716,
        ),
        script.interface.object_method_bind_ptrcall,
    ) else {
        return;
    };
    // SAFETY: The extension object derives from Resource and the official
    // Resource.emit_changed() method takes no arguments or return storage.
    unsafe {
        ptrcall(
            method,
            script.object as *mut c_void,
            ptr::null(),
            ptr::null_mut(),
        );
    }
}

unsafe extern "C" fn get_member_line(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_i64(result, -1);
    let Some(script) = script(instance) else {
        return;
    };
    if arguments.is_null() {
        return;
    }
    // SAFETY: This virtual's first argument is StringName.
    let requested = unsafe { *arguments };
    let Some(compiled) = compiled_script(script) else {
        return;
    };
    let mut name = None;
    for index in 0..compiled.field_count() {
        let Some(field) = compiled.field(index) else {
            continue;
        };
        let Some(candidate) = OwnedStringName::new(script.interface, field.name()) else {
            continue;
        };
        if candidate.equals(requested) {
            name = Some(field.name().to_owned());
            break;
        }
    }
    if name.is_none() {
        for index in 0..compiled.method_count() {
            let Some(method) = compiled.method(index) else {
                continue;
            };
            let Some(candidate) = OwnedStringName::new(script.interface, method.name()) else {
                continue;
            };
            if candidate.equals(requested) {
                name = Some(method.name().to_owned());
                break;
            }
        }
    }
    let Ok(source) = script.source.read() else {
        return;
    };
    if name.is_none() {
        for constant in crate::rust_source::script_constants(&source, compiled.name()) {
            let Some(candidate) = OwnedStringName::new(script.interface, &constant.name) else {
                continue;
            };
            if candidate.equals(requested) {
                write_i64(result, i64::try_from(constant.line).unwrap_or(-1));
                return;
            }
        }
    }
    let Some(name) = name else {
        return;
    };
    let Some(line) = crate::rust_source::find_identifier_line(&source, &name)
        .and_then(|line| i64::try_from(line).ok())
    else {
        return;
    };
    write_i64(result, line);
}

unsafe extern "C" fn get_constants(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let (Some(compiled), Ok(source)) = (compiled_script(script), script.source.read()) else {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
        return;
    };
    let constants = crate::rust_source::script_constants(&source, compiled.name());
    if !godot_metadata::write_script_constants(script.interface, result, &constants) {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn get_members(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Some(compiled) = compiled_script(script) else {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
        return;
    };
    if !godot_metadata::write_script_members(script.interface, result, &compiled) {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
    }
}

unsafe extern "C" fn has_script_signal(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let value = script(instance).is_some_and(|script| {
        if arguments.is_null() {
            return false;
        }
        // SAFETY: This virtual's first argument is StringName.
        let name = unsafe { *arguments };
        compiled_script(script)
            .and_then(|compiled| find_field(script, &compiled, name))
            .is_some_and(|field| field.is_signal())
    });
    write_bool(result, value);
}

unsafe extern "C" fn get_script_signal_list(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let Some(compiled) = compiled_script(script) else {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
        return;
    };
    if !godot_metadata::write_signal_list(script.interface, &compiled, result) {
        write_default_builtin(
            script.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
    }
}

unsafe extern "C" fn has_property_default_value(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let value = script(instance).is_some_and(|script| {
        if arguments.is_null() {
            return false;
        }
        // SAFETY: This virtual's first argument is StringName.
        let name = unsafe { *arguments };
        compiled_script(script)
            .and_then(|compiled| find_field(script, &compiled, name))
            .and_then(|field| field.property_default_value())
            .is_some()
    });
    write_bool(result, value);
}

unsafe extern "C" fn get_property_default_value(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        return;
    };
    let field = if arguments.is_null() {
        None
    } else {
        // SAFETY: This virtual's first argument is StringName.
        let name = unsafe { *arguments };
        compiled_script(script).and_then(|compiled| find_field(script, &compiled, name))
    };
    let wrote_default = field.is_some_and(|field| {
        let Some(value) = field.property_default_value() else {
            return false;
        };
        VariantCodec::new(script.interface).is_some_and(|codec| {
            codec
                .construct_with_context(value.abi(), result, field.typed_array_element(), None)
                .is_ok()
        })
    });
    if !wrote_default {
        write_nil_variant(script.interface, result);
    }
}

fn find_method(
    script: &RustScript,
    compiled: &ModuleScript,
    name: GDExtensionConstStringNamePtr,
) -> Option<ModuleMethod> {
    (0..compiled.method_count()).find_map(|index| {
        let method = compiled.method(index)?;
        let method_name = OwnedStringName::new(script.interface, method.name())?;
        method_name.equals(name).then_some(method)
    })
}

fn find_field(
    script: &RustScript,
    compiled: &ModuleScript,
    name: GDExtensionConstStringNamePtr,
) -> Option<ModuleField> {
    (0..compiled.field_count()).find_map(|index| {
        let field = compiled.field(index)?;
        let field_name = OwnedStringName::new(script.interface, field.name())?;
        field_name.equals(name).then_some(field)
    })
}

fn lifecycle_argument_count(
    script: &RustScript,
    compiled: &ModuleScript,
    name: GDExtensionConstStringNamePtr,
) -> Option<i64> {
    let interface = script.interface;
    let zero_argument = (compiled.has_enter_tree()
        && script.enter_tree_name.equals(interface, name))
        || (compiled.has_ready() && script.ready_name.equals(interface, name))
        || (compiled.has_exit_tree() && script.exit_tree_name.equals(interface, name));
    if zero_argument {
        Some(0)
    } else if (compiled.has_process() && script.process_name.equals(interface, name))
        || (compiled.has_physics_process() && script.physics_process_name.equals(interface, name))
        || (compiled.has_input() && script.input_name.equals(interface, name))
        || (compiled.has_unhandled_input() && script.unhandled_input_name.equals(interface, name))
    {
        Some(1)
    } else {
        (0..compiled.method_count()).find_map(|index| {
            let method = compiled.method(index)?;
            if method.kind() == AbiMethodKind::Lifecycle {
                return None;
            }
            let method_name = OwnedStringName::new(interface, method.name())?;
            method_name
                .equals(name)
                .then(|| i64::try_from(method.argument_types().len()).ok())
                .flatten()
        })
    }
}

unsafe extern "C" fn get_base_script(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        write_null(result);
        return;
    };
    let Some(path) =
        compiled_script(script).and_then(|compiled| compiled.base_script_path().map(str::to_owned))
    else {
        write_null(result);
        return;
    };
    let mut loaded = None;
    let object = script_object_by_path(&path).unwrap_or_else(|| {
        loaded = load_script_ref(script.interface, &path);
        loaded
            .as_ref()
            .map(crate::engine_call::value::NativeGodotRef::object)
            .unwrap_or(ptr::null_mut())
    });
    let Some(set_object) = script.interface.ref_set_object else {
        write_null(result);
        return;
    };
    if !result.is_null() {
        // SAFETY: Godot provides one initialized Ref<Script> return slot and
        // `object` is null or a live Script returned by ResourceLoader.
        unsafe { set_object(result.cast(), object) };
    }
}

unsafe extern "C" fn inherits_script(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let inherited = (|| {
        let current = compiled_script(script(instance)?)?;
        if arguments.is_null() {
            return None;
        }
        // SAFETY: This virtual's first argument is an encoded Object pointer.
        let storage = unsafe { *arguments };
        if storage.is_null() {
            return None;
        }
        // SAFETY: Object ptrcall arguments point to a pointer-sized Object slot.
        let object = unsafe { *storage.cast::<*mut c_void>() };
        let candidate = scripts_by_object()
            .read()
            .ok()?
            .get(&(object as usize))
            .and_then(Weak::upgrade)?;
        let candidate = compiled_script(&candidate)?;
        Some(current.inherits(&candidate))
    })()
    .unwrap_or(false);
    write_bool(result, inherited);
}

unsafe extern "C" fn placeholder_instance_create(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        write_null(result);
        return;
    };
    if arguments.is_null() {
        write_null(result);
        return;
    }
    // SAFETY: This virtual's first argument is an encoded Object pointer.
    let owner_storage = unsafe { *arguments };
    if owner_storage.is_null() {
        write_null(result);
        return;
    }
    // SAFETY: Object ptrcall arguments point to a pointer-sized Object slot.
    let owner = unsafe { *owner_storage.cast::<*mut c_void>() };
    let language = LANGUAGE_OBJECT.load(Ordering::Acquire) as *mut c_void;
    if language.is_null() || owner.is_null() {
        write_null(result);
        return;
    }
    let Some(create) = script.interface.placeholder_script_instance_create else {
        write_null(result);
        return;
    };
    // SAFETY: Language, script, and owner are live Godot Objects. Godot owns
    // the returned placeholder through its ScriptInstance lifecycle.
    let placeholder = unsafe { create(language, script.object as *mut c_void, owner) };
    if !result.is_null() {
        // SAFETY: GDExtensionPtr<void> returns use one pointer-sized slot.
        unsafe { result.cast::<*mut c_void>().write(placeholder) };
    }
}

unsafe extern "C" fn runtime_instance_create(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        write_null(result);
        return;
    };
    if arguments.is_null() || !script.has_source.load(Ordering::Acquire) {
        write_null(result);
        return;
    }
    // SAFETY: This virtual's first argument is an encoded Object pointer.
    let owner_storage = unsafe { *arguments };
    if owner_storage.is_null() {
        write_null(result);
        return;
    }
    // SAFETY: Object ptrcall arguments point to a pointer-sized Object slot.
    let owner = unsafe { *owner_storage.cast::<*mut c_void>() };
    let language = LANGUAGE_OBJECT.load(Ordering::Acquire) as *mut c_void;
    let Some(module_script) = compiled_script(script) else {
        write_null(result);
        return;
    };
    let Ok(module_state) = module_script.create_state() else {
        write_null(result);
        return;
    };
    let instance = script_instance::create(
        script.interface,
        script.object as *mut c_void,
        owner,
        language,
        module_state,
    );
    if !result.is_null() {
        // SAFETY: The virtual returns one opaque ScriptInstance pointer.
        unsafe { result.cast::<*mut c_void>().write(instance) };
    }
}

unsafe extern "C" fn instance_has(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(script) = script(instance) else {
        write_bool(result, false);
        return;
    };
    if arguments.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: This virtual's first argument is an encoded Object pointer.
    let object_storage = unsafe { *arguments };
    if object_storage.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: Object ptrcall arguments point to a pointer-sized Object slot.
    let object = unsafe { *object_storage.cast::<*mut c_void>() };
    let language = LANGUAGE_OBJECT.load(Ordering::Acquire) as *mut c_void;
    let Some(get_instance) = script.interface.object_get_script_instance else {
        write_bool(result, false);
        return;
    };
    let found = if object.is_null() || language.is_null() {
        false
    } else {
        // SAFETY: Both arguments are live Godot Objects for this virtual call.
        let instance = unsafe { get_instance(object, language) };
        script_instance::belongs_to_script(instance, script.object as *mut c_void)
    };
    write_bool(result, found);
}

unsafe extern "C" fn return_true(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_bool(result, true);
}

fn write_null(result: GDExtensionTypePtr) {
    if !result.is_null() {
        // SAFETY: Used only for pointer-like virtual return types.
        unsafe { result.cast::<*mut c_void>().write(ptr::null_mut()) };
    }
}

unsafe extern "C" fn no_op(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
}

fn write_bool(result: GDExtensionTypePtr, value: bool) {
    if !result.is_null() {
        // SAFETY: Godot encodes ptrcall bool as one byte.
        unsafe { result.cast::<u8>().write(u8::from(value)) };
    }
}

fn write_i64(result: GDExtensionTypePtr, value: i64) {
    if !result.is_null() {
        // SAFETY: Godot encodes ptrcall integers and enums as i64.
        unsafe { result.cast::<i64>().write(value) };
    }
}

macro_rules! virtual_method {
    ($name:literal, $hash:literal, $callback:ident) => {
        VirtualMethodSpec {
            name: $name,
            hash: $hash,
            callback: Some($callback),
        }
    };
}

static SCRIPT_VIRTUAL_METHODS: &[VirtualMethodSpec] = &[
    virtual_method!(
        c"_editor_can_reload_from_file",
        2_240_911_060,
        editor_can_reload_from_file
    ),
    virtual_method!(c"_placeholder_erased", 1_286_410_249, no_op),
    virtual_method!(c"_can_instantiate", 36_873_697, can_instantiate),
    virtual_method!(c"_get_base_script", 278_624_046, get_base_script),
    virtual_method!(c"_get_global_name", 2_002_593_661, get_global_name),
    virtual_method!(c"_inherits_script", 3_669_307_804, inherits_script),
    virtual_method!(
        c"_get_instance_base_type",
        2_002_593_661,
        get_instance_base_type
    ),
    virtual_method!(c"_instance_create", 1_107_568_780, runtime_instance_create),
    virtual_method!(
        c"_placeholder_instance_create",
        1_107_568_780,
        placeholder_instance_create
    ),
    virtual_method!(c"_instance_has", 397_768_994, instance_has),
    virtual_method!(c"_has_source_code", 36_873_697, has_source_code),
    virtual_method!(c"_get_source_code", 201_670_096, get_source_code),
    virtual_method!(c"_set_source_code", 83_702_148, set_source_code),
    virtual_method!(c"_reload", 1_413_768_114, reload),
    virtual_method!(c"_get_doc_class_name", 2_002_593_661, get_global_name),
    virtual_method!(c"_get_documentation", 3_995_934_104, get_documentation),
    virtual_method!(c"_get_class_icon_path", 201_670_096, get_class_icon_path),
    virtual_method!(c"_has_method", 2_619_796_661, has_method),
    virtual_method!(c"_has_static_method", 2_619_796_661, has_static_method),
    virtual_method!(
        c"_get_script_method_argument_count",
        2_760_726_917,
        script_method_argument_count
    ),
    virtual_method!(c"_get_method_info", 4_028_089_122, get_method_info),
    virtual_method!(c"_is_tool", 36_873_697, is_tool),
    virtual_method!(c"_is_valid", 36_873_697, is_valid),
    virtual_method!(c"_is_abstract", 36_873_697, is_abstract),
    virtual_method!(c"_get_language", 3_096_237_657, get_language),
    virtual_method!(c"_has_script_signal", 2_619_796_661, has_script_signal),
    virtual_method!(
        c"_get_script_signal_list",
        3_995_934_104,
        get_script_signal_list
    ),
    virtual_method!(
        c"_has_property_default_value",
        2_619_796_661,
        has_property_default_value
    ),
    virtual_method!(
        c"_get_property_default_value",
        2_760_726_917,
        get_property_default_value
    ),
    virtual_method!(c"_update_exports", 3_218_959_716, update_exports),
    virtual_method!(
        c"_get_script_method_list",
        3_995_934_104,
        get_script_method_list
    ),
    virtual_method!(
        c"_get_script_property_list",
        3_995_934_104,
        get_script_property_list
    ),
    virtual_method!(c"_get_member_line", 2_458_036_349, get_member_line),
    virtual_method!(c"_get_constants", 3_102_165_223, get_constants),
    virtual_method!(c"_get_members", 3_995_934_104, get_members),
    virtual_method!(c"_is_placeholder_fallback_enabled", 36_873_697, return_true),
    virtual_method!(c"_get_rpc_config", 1_214_101_251, get_rpc_config),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_script_virtual_has_a_unique_name() {
        let names: HashSet<_> = SCRIPT_VIRTUAL_METHODS
            .iter()
            .map(|method| method.name.to_bytes())
            .collect();
        assert_eq!(names.len(), SCRIPT_VIRTUAL_METHODS.len());
        assert_eq!(names.len(), 37);
        assert!(
            SCRIPT_VIRTUAL_METHODS
                .iter()
                .all(|method| method.callback.is_some())
        );
    }

    #[test]
    fn source_virtuals_match_the_official_hashes() {
        for (name, hash) in [
            (c"_has_source_code".to_bytes(), 36_873_697),
            (c"_get_source_code".to_bytes(), 201_670_096),
            (c"_set_source_code".to_bytes(), 83_702_148),
            (c"_reload".to_bytes(), 1_413_768_114),
        ] {
            assert!(
                SCRIPT_VIRTUAL_METHODS
                    .iter()
                    .any(|method| { method.name.to_bytes() == name && method.hash == hash })
            );
        }
    }
}
