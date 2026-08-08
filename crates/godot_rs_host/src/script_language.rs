use core::ffi::{CStr, c_void};
use core::ptr;
use godot_rs_api::GDExtensionMethodBindPtr;
use godot_rs_api::abi::AbiValueV1;
use godot_rs_api::{
    GDExtensionClassInstancePtr, GDExtensionConstTypePtr, GDExtensionTypePtr,
    GDExtensionVariantType,
};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::interface::EngineInterface;
use crate::packed_string_array::{PackedStringArrayReader, PackedStringArrayWriter};
use crate::registry::{ClassRegistry, ClassSpec, RegisteredClassId, VirtualMethodSpec};
use crate::string_name::{OwnedStringName, StaticStringName};
use crate::value::read_utf8_string;

const ERR_UNAVAILABLE: i64 = 2;
const ERR_CANT_OPEN: i64 = 19;
const SCRIPT_NAME_CASING_SNAKE_CASE: i64 = 2;
const COMPLETION_KIND_CLASS: i64 = 0;
const COMPLETION_KIND_FUNCTION: i64 = 1;
const COMPLETION_KIND_VARIABLE: i64 = 3;
const COMPLETION_KIND_PLAIN_TEXT: i64 = 9;
const COMPLETION_LOCATION_LOCAL: i64 = 0;
const LOOKUP_RESULT_SCRIPT_LOCATION: i64 = 0;

pub(crate) struct RustScriptLanguage {
    interface: EngineInterface,
    packed_strings: PackedStringArrayWriter,
    packed_string_reader: PackedStringArrayReader,
    script_class: StaticStringName,
    notification_method: usize,
    callback_context: Mutex<Option<CallbackContext>>,
    reload_monitor: Mutex<ReloadMonitor>,
}

struct CallbackContext {
    function_name: String,
    script_type: String,
}

struct ReloadMonitor {
    watcher: Option<crate::last_known_good::Watcher>,
    pending: Option<(String, crate::module_loader::ModuleGeneration)>,
    next_poll: Instant,
}

const RELOAD_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) fn register_class(registry: &mut ClassRegistry) -> RegisteredClassId {
    registry.register(ClassSpec {
        name: c"GodotRustLanguage",
        parent: c"ScriptLanguageExtension",
        factory: create_language_instance,
        dropper: drop_language_instance,
        virtual_methods: LANGUAGE_VIRTUAL_METHODS,
    })
}

fn create_language_instance(interface: EngineInterface, _object: *mut c_void) -> *mut c_void {
    let Some(packed_strings) = PackedStringArrayWriter::new(interface) else {
        return ptr::null_mut();
    };
    let Some(packed_string_reader) = PackedStringArrayReader::new(interface) else {
        return ptr::null_mut();
    };
    let object_class = StaticStringName::new(interface, c"Object");
    let notification_name = StaticStringName::new(interface, c"notification");
    let Some(get_method_bind) = interface.classdb_get_method_bind else {
        return ptr::null_mut();
    };
    // SAFETY: Names and signature hash match Object.notification(int, bool).
    let notification_method = unsafe {
        get_method_bind(
            object_class.as_ptr(),
            notification_name.as_ptr(),
            4_023_243_586,
        )
    };
    if notification_method.is_null() {
        return ptr::null_mut();
    }

    let watcher = if is_editor_hint(interface)
        && !crate::last_known_good::safe_mode_enabled(interface).unwrap_or(true)
    {
        match crate::last_known_good::Watcher::new(interface) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                host_eprintln!("godot-rust: editor module reload is unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    Box::into_raw(Box::new(RustScriptLanguage {
        interface,
        packed_strings,
        packed_string_reader,
        script_class: StaticStringName::new(interface, c"GodotRustScript"),
        notification_method: notification_method as usize,
        callback_context: Mutex::new(None),
        reload_monitor: Mutex::new(ReloadMonitor {
            watcher,
            pending: None,
            next_poll: Instant::now(),
        }),
    }))
    .cast()
}

fn is_editor_hint(interface: EngineInterface) -> bool {
    let (Some(get_singleton), Some(ptrcall)) = (
        interface.global_get_singleton,
        interface.object_method_bind_ptrcall,
    ) else {
        return false;
    };
    let engine_name = StaticStringName::new(interface, c"Engine");
    // SAFETY: Engine is an official singleton and the StringName is live.
    let engine = unsafe { get_singleton(engine_name.as_ptr()) };
    if engine.is_null() {
        return false;
    }
    let Ok(method) =
        crate::runtime::resolve_method(interface, c"Engine", c"is_editor_hint", 36_873_697)
    else {
        return false;
    };
    let mut result = 0_u8;
    // SAFETY: The bind matches Engine.is_editor_hint() -> bool and the result
    // points to Godot's one-byte ptrcall bool representation.
    unsafe {
        ptrcall(
            method,
            engine,
            ptr::null(),
            ptr::from_mut(&mut result).cast(),
        );
    }
    result != 0
}

unsafe fn drop_language_instance(instance: *mut c_void) {
    // SAFETY: This pointer is allocated by `create_language_instance` for the
    // matching registered class and is freed exactly once by ClassDB.
    unsafe { drop(Box::from_raw(instance.cast::<RustScriptLanguage>())) };
}

fn language(instance: GDExtensionClassInstancePtr) -> Option<&'static RustScriptLanguage> {
    if instance.is_null() {
        return None;
    }
    // SAFETY: Every virtual call for this class receives the instance returned
    // by `create_language_instance`; ClassDB keeps it alive for the call.
    Some(unsafe { &*instance.cast::<RustScriptLanguage>() })
}

fn write_string(
    instance: GDExtensionClassInstancePtr,
    result: GDExtensionTypePtr,
    value: &'static CStr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    crate::value::write_latin1_string(language.interface, result, value);
}

fn write_default_builtin(
    instance: GDExtensionClassInstancePtr,
    result: GDExtensionTypePtr,
    type_: GDExtensionVariantType,
) {
    let Some(language) = language(instance) else {
        return;
    };
    crate::value::write_default_builtin(language.interface, result, type_);
}

fn write_string_list(
    instance: GDExtensionClassInstancePtr,
    result: GDExtensionTypePtr,
    values: &[&str],
) {
    let Some(language) = language(instance) else {
        return;
    };
    language.packed_strings.write(result, values);
}

fn write_bool(result: GDExtensionTypePtr, value: bool) {
    if !result.is_null() {
        // SAFETY: Godot encodes ptrcall bool values as one byte.
        unsafe { result.cast::<u8>().write(u8::from(value)) };
    }
}

fn write_i64(result: GDExtensionTypePtr, value: i64) {
    if !result.is_null() {
        // SAFETY: Godot encodes ptrcall integers as i64, including methods
        // whose API metadata narrows the C++ value to int32.
        unsafe { result.cast::<i64>().write(value) };
    }
}

unsafe extern "C" fn get_name(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string(instance, result, c"Rust");
}

unsafe extern "C" fn get_type(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string(instance, result, c"Rust");
}

unsafe extern "C" fn get_extension(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string(instance, result, c"rs");
}

unsafe extern "C" fn create_script(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    let script = construct_script(language);
    if result.is_null() {
        if let Some(destroy) = language.interface.object_destroy {
            if !script.is_null() {
                // SAFETY: This object has not escaped to Godot because the
                // return storage is null.
                unsafe { destroy(script) };
            }
        }
        return;
    }
    // SAFETY: Object results use one pointer-sized slot.
    unsafe { result.cast::<*mut c_void>().write(script) };
}

fn construct_script(language: &RustScriptLanguage) -> *mut c_void {
    let Some(construct) = language.interface.classdb_construct_object2 else {
        return ptr::null_mut();
    };
    // SAFETY: The class is registered before the language class is instantiated.
    let script = unsafe { construct(language.script_class.as_ptr()) };
    if script.is_null() {
        return ptr::null_mut();
    }

    let notification = 0_i64;
    let reversed = 0_u8;
    let arguments: [GDExtensionConstTypePtr; 2] = [
        (&notification as *const i64).cast(),
        (&reversed as *const u8).cast(),
    ];
    if let Some(ptrcall) = language.interface.object_method_bind_ptrcall {
        // SAFETY: The bind and encoded arguments match Object.notification.
        unsafe {
            ptrcall(
                language.notification_method as GDExtensionMethodBindPtr,
                script,
                arguments.as_ptr(),
                ptr::null_mut(),
            );
        }
    }
    script
}

unsafe extern "C" fn make_template(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if result.is_null() {
        return;
    }
    // SAFETY: Object results use one pointer-sized slot.
    unsafe { result.cast::<*mut c_void>().write(ptr::null_mut()) };
    if arguments.is_null() {
        return;
    }
    // SAFETY: `_make_template` has exactly three official String arguments.
    let template = unsafe { *arguments };
    // SAFETY: See above.
    let class_name = unsafe { *arguments.add(1) };
    // SAFETY: See above.
    let base_class = unsafe { *arguments.add(2) };
    let Ok(template) = read_utf8_string(language.interface, template) else {
        return;
    };
    let Ok(class_name) = read_utf8_string(language.interface, class_name) else {
        return;
    };
    let Ok(base_class) = read_utf8_string(language.interface, base_class) else {
        return;
    };
    let source = crate::script_template::render(&template, &class_name, &base_class);
    let script = construct_script(language);
    if script.is_null() {
        return;
    }
    if !crate::script::initialize_new_script(script, &source) {
        if let Some(destroy) = language.interface.object_destroy {
            // SAFETY: The object has not been returned to Godot.
            unsafe { destroy(script) };
        }
        return;
    }
    // SAFETY: Object results use one pointer-sized slot.
    unsafe { result.cast::<*mut c_void>().write(script) };
}

unsafe extern "C" fn get_recognized_extensions(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    language.packed_strings.write(result, &["rs"]);
}

macro_rules! default_builtin_callback {
    ($name:ident, $variant:ident) => {
        unsafe extern "C" fn $name(
            instance: GDExtensionClassInstancePtr,
            _args: *const GDExtensionConstTypePtr,
            result: GDExtensionTypePtr,
        ) {
            write_default_builtin(instance, result, GDExtensionVariantType::$variant);
        }
    };
}

default_builtin_callback!(return_empty_array, GDEXTENSION_VARIANT_TYPE_ARRAY);
default_builtin_callback!(return_empty_dictionary, GDEXTENSION_VARIANT_TYPE_DICTIONARY);

unsafe extern "C" fn return_false(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_bool(result, false);
}

unsafe extern "C" fn return_true(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_bool(result, true);
}

unsafe extern "C" fn validate(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
        return;
    }
    // SAFETY: The first two official arguments are String values. The
    // remaining validation flags do not affect Rust syntax parsing.
    let source = unsafe { *arguments };
    // SAFETY: See above.
    let path = unsafe { *arguments.add(1) };
    let Ok(source) = read_utf8_string(language.interface, source) else {
        return;
    };
    let path = read_utf8_string(language.interface, path).unwrap_or_default();
    let functions = validation_function_entries(&source);
    let syntax_error = syn::parse_file(&source).err().map(|error| {
        let start = error.span().start();
        (
            i64::try_from(start.line).unwrap_or(i64::MAX),
            i64::try_from(start.column.saturating_add(1)).unwrap_or(i64::MAX),
            error.to_string(),
        )
    });
    let issues = syntax_error
        .as_ref()
        .map(|(line, column, message)| {
            vec![crate::godot_metadata::ValidationIssue {
                path: &path,
                line: *line,
                column: *column,
                message,
            }]
        })
        .unwrap_or_default();
    if !crate::godot_metadata::write_validation_result(
        language.interface,
        result,
        syntax_error.is_none(),
        &functions,
        &issues,
    ) {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

fn validation_function_entries(source: &str) -> Vec<String> {
    crate::rust_source::function_declarations(source)
        .into_iter()
        .map(|(name, line)| format!("{name}:{}", line.saturating_add(1)))
        .collect()
}

unsafe extern "C" fn validate_path(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        crate::value::write_utf8_string(language.interface, result, "Rust script path is missing");
        return;
    }
    // SAFETY: The virtual receives exactly one String path.
    let path = unsafe { *arguments };
    let message = match read_utf8_string(language.interface, path) {
        Ok(path) if path.starts_with("res://") && path.ends_with(".rs") => "",
        Ok(_) => "Rust scripts must use a res:// path ending in .rs",
        Err(_) => "Rust script path is not valid UTF-8",
    };
    crate::value::write_utf8_string(language.interface, result, message);
}

unsafe extern "C" fn complete_code(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        return;
    }
    // SAFETY: The first official argument is the current source String.
    let source = unsafe { *arguments };
    let Ok(source) = read_utf8_string(language.interface, source) else {
        return;
    };
    let functions = crate::rust_source::function_declarations(&source);
    let identifiers = crate::rust_source::identifiers(&source);
    let mut owned = Vec::<(i64, String, String, i64)>::new();
    for keyword in crate::rust_source::HIGHLIGHT_WORDS {
        owned.push((
            COMPLETION_KIND_PLAIN_TEXT,
            (*keyword).to_owned(),
            (*keyword).to_owned(),
            COMPLETION_LOCATION_LOCAL,
        ));
    }
    for identifier in identifiers {
        let kind = if functions.iter().any(|(name, _)| name == &identifier) {
            COMPLETION_KIND_FUNCTION
        } else if identifier
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_uppercase)
        {
            COMPLETION_KIND_CLASS
        } else {
            COMPLETION_KIND_VARIABLE
        };
        owned.push((
            kind,
            identifier.clone(),
            identifier,
            COMPLETION_LOCATION_LOCAL,
        ));
    }
    let options = owned
        .iter()
        .map(
            |(kind, display, insert_text, location)| crate::godot_metadata::CompletionOption {
                kind: *kind,
                display,
                insert_text,
                location: *location,
            },
        )
        .collect::<Vec<_>>();
    if !crate::godot_metadata::write_completion_result(
        language.interface,
        result,
        &options,
        false,
        "",
    ) {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn lookup_code(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        return;
    }
    // SAFETY: The first three official arguments are source, symbol, and path
    // String values. The owner Object is not needed for source-local lookup.
    let source = unsafe { *arguments };
    // SAFETY: See above.
    let symbol = unsafe { *arguments.add(1) };
    // SAFETY: See above.
    let path = unsafe { *arguments.add(2) };
    let (Ok(source), Ok(symbol), Ok(path)) = (
        read_utf8_string(language.interface, source),
        read_utf8_string(language.interface, symbol),
        read_utf8_string(language.interface, path),
    ) else {
        return;
    };
    let symbol = symbol
        .rsplit("::")
        .next()
        .unwrap_or(&symbol)
        .trim_matches(|character: char| !character.is_alphanumeric() && character != '_');
    let location = crate::rust_source::find_declaration_line(&source, symbol)
        .and_then(|line| i64::try_from(line).ok())
        .unwrap_or(-1);
    let lookup = crate::godot_metadata::LookupResult {
        result: if location >= 0 { 0 } else { ERR_UNAVAILABLE },
        type_: LOOKUP_RESULT_SCRIPT_LOCATION,
        script_path: &path,
        location,
        class_name: "",
        class_member: symbol,
        description: if location >= 0 {
            "Rust source declaration"
        } else {
            ""
        },
    };
    if !crate::godot_metadata::write_lookup_result(language.interface, result, &lookup) {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn reload_scripts(
    _instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    crate::script::reload_all_sources_from_disk();
}

unsafe extern "C" fn overrides_external_editor(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_bool(result, configured_external_editor().is_some());
}

unsafe extern "C" fn handles_global_class_type(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        write_bool(result, false);
        return;
    };
    if arguments.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: The virtual receives exactly one String argument.
    let type_name = unsafe { *arguments };
    write_bool(
        result,
        read_utf8_string(language.interface, type_name).is_ok_and(|name| name == "GodotRustScript"),
    );
}

unsafe extern "C" fn get_global_class_name(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
        return;
    }
    // SAFETY: The virtual receives exactly one String path.
    let path = unsafe { *arguments };
    let Ok(path) = read_utf8_string(language.interface, path) else {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
        return;
    };
    let Some(script) = crate::module_loader::active_script(&path) else {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
        return;
    };
    let Some(global_name) = script.global_name() else {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
        return;
    };
    let Some(codec) = crate::variant_codec::VariantCodec::new(language.interface) else {
        return;
    };
    let Ok(mut dictionary) = crate::dynamic_value::OwnedDictionary::empty(language.interface)
    else {
        return;
    };
    let base_type = script
        .base_script()
        .and_then(|base| base.global_name().map(str::to_owned))
        .unwrap_or_else(|| script.base().to_owned());
    let attributes = crate::script::source_for_path(&path)
        .map(|source| crate::rust_source::script_source_attributes(&source, script.name()))
        .unwrap_or_default();
    let string_entries = [
        ("name", global_name),
        ("base_type", base_type.as_str()),
        ("icon_path", attributes.icon_path.as_deref().unwrap_or("")),
    ];
    for (key, value) in string_entries {
        let (Ok(key), Ok(value)) = (
            crate::variant_codec::OwnedVariant::from_abi(
                &codec,
                AbiValueV1::from_borrowed_utf8(key),
            ),
            crate::variant_codec::OwnedVariant::from_abi(
                &codec,
                AbiValueV1::from_borrowed_utf8(value),
            ),
        ) else {
            return;
        };
        if dictionary.insert(&key, &value).is_err() {
            return;
        }
    }
    for (key, value) in [
        ("is_abstract", attributes.abstract_),
        ("is_tool", script.is_tool()),
    ] {
        let (Ok(key), Ok(value)) = (
            crate::variant_codec::OwnedVariant::from_abi(
                &codec,
                AbiValueV1::from_borrowed_utf8(key),
            ),
            crate::variant_codec::OwnedVariant::from_abi(&codec, AbiValueV1::from_bool(value)),
        ) else {
            return;
        };
        if dictionary.insert(&key, &value).is_err() {
            return;
        }
    }
    if dictionary.write_copy(result).is_err() {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn get_reserved_words(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string_list(instance, result, crate::rust_source::HIGHLIGHT_WORDS);
}

unsafe extern "C" fn is_control_flow_keyword(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        write_bool(result, false);
        return;
    };
    if arguments.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: The official virtual has exactly one String argument.
    let keyword = unsafe { *arguments };
    let is_control_flow = read_utf8_string(language.interface, keyword)
        .is_ok_and(|keyword| crate::rust_source::is_control_flow_word(&keyword));
    write_bool(result, is_control_flow);
}

unsafe extern "C" fn get_comment_delimiters(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string_list(instance, result, crate::rust_source::COMMENT_DELIMITERS);
}

unsafe extern "C" fn get_doc_comment_delimiters(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string_list(instance, result, crate::rust_source::DOC_COMMENT_DELIMITERS);
}

unsafe extern "C" fn get_string_delimiters(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_string_list(instance, result, crate::rust_source::STRING_DELIMITERS);
}

fn debug_frame(arguments: *const GDExtensionConstTypePtr) -> Option<crate::debugger::DebugFrame> {
    if arguments.is_null() {
        return None;
    }
    // SAFETY: Every stack-level virtual has an integer first argument.
    let level = unsafe { (*arguments).cast::<i64>().read() };
    let level = usize::try_from(level).ok()?;
    crate::debugger::frames().get(level).cloned()
}

unsafe extern "C" fn debug_get_error(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if !crate::value::write_utf8_string(language.interface, result, &crate::debugger::error()) {
        write_string(instance, result, c"");
    }
}

unsafe extern "C" fn debug_get_stack_level_count(
    _instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_i64(
        result,
        i64::try_from(crate::debugger::frames().len()).unwrap_or(i64::MAX),
    );
}

unsafe extern "C" fn debug_get_stack_level_line(
    _instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_i64(result, debug_frame(arguments).map_or(0, |frame| frame.line));
}

fn write_debug_frame_text(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
    select: impl FnOnce(&crate::debugger::DebugFrame) -> &str,
) {
    let Some(language) = language(instance) else {
        return;
    };
    let frame = debug_frame(arguments);
    let value = frame.as_ref().map(select).unwrap_or("");
    if !crate::value::write_utf8_string(language.interface, result, value) {
        write_string(instance, result, c"");
    }
}

unsafe extern "C" fn debug_get_stack_level_function(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_debug_frame_text(instance, arguments, result, |frame| &frame.function);
}

unsafe extern "C" fn debug_get_stack_level_source(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_debug_frame_text(instance, arguments, result, |frame| &frame.source);
}

unsafe extern "C" fn debug_get_stack_level_locals(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    let wrote = debug_frame(arguments).is_some_and(|frame| {
        crate::godot_metadata::write_debug_string_values(language.interface, result, &frame.locals)
    });
    if !wrote {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn debug_get_stack_level_members(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let wrote = debug_frame(arguments)
        .is_some_and(|frame| crate::script_instance::write_debug_members(frame.instance, result));
    if !wrote {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        );
    }
}

unsafe extern "C" fn debug_get_stack_level_instance(
    _instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    if result.is_null() {
        return;
    }
    let value = debug_frame(arguments)
        .map(|frame| crate::script_instance::debug_script_instance(frame.instance))
        .unwrap_or(ptr::null_mut());
    // SAFETY: Native pointer returns use one pointer-sized output slot.
    unsafe { result.cast::<*mut c_void>().write(value) };
}

unsafe extern "C" fn debug_parse_stack_level_expression(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    let value = if arguments.is_null() {
        None
    } else {
        let frame = debug_frame(arguments);
        // SAFETY: This virtual's second argument is a live String.
        let expression = unsafe { *arguments.add(1) };
        frame.and_then(|frame| {
            read_utf8_string(language.interface, expression)
                .ok()
                .and_then(|expression| {
                    frame
                        .locals
                        .iter()
                        .find_map(|(name, value)| (name == &expression).then(|| value.clone()))
                        .or_else(|| {
                            crate::script_instance::debug_member_expression(
                                frame.instance,
                                &expression,
                            )
                        })
                })
        })
    };
    if !crate::value::write_utf8_string(language.interface, result, value.as_deref().unwrap_or(""))
    {
        write_string(instance, result, c"");
    }
}

unsafe extern "C" fn debug_get_current_stack_info(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if !crate::godot_metadata::write_debug_stack_info(
        language.interface,
        result,
        &crate::debugger::frames(),
    ) {
        write_default_builtin(
            instance,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        );
    }
}

unsafe extern "C" fn profiling_start(
    _instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    crate::profiler::start();
}

unsafe extern "C" fn profiling_stop(
    _instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    crate::profiler::stop();
}

unsafe extern "C" fn profiling_set_save_native_calls(
    _instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    let enabled = if arguments.is_null() {
        false
    } else {
        // SAFETY: This virtual has one bool argument.
        unsafe { (*arguments).cast::<u8>().read() != 0 }
    };
    crate::profiler::set_save_native_calls(enabled);
}

#[repr(C)]
struct GodotProfilingInfo {
    signature: usize,
    call_count: u64,
    total_time: u64,
    self_time: u64,
    internal_time: u64,
}

fn write_profiling_data(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
    entries: Vec<crate::profiler::ProfileEntry>,
) {
    let Some(language) = language(instance) else {
        write_i64(result, 0);
        return;
    };
    if arguments.is_null() {
        write_i64(result, 0);
        return;
    }
    // Native pointer arguments are transported as their target address, not
    // as pointer-to-pointer storage (Godot `GDExtensionPtr<T>` semantics).
    // SAFETY: The first official argument points to `info_max` writable
    // ScriptLanguage::ProfilingInfo records.
    let output = unsafe { *arguments }
        .cast_mut()
        .cast::<GodotProfilingInfo>();
    // SAFETY: The second official argument is an integer ptrcall slot.
    let maximum = unsafe { (*arguments.add(1)).cast::<i64>().read() };
    let maximum = usize::try_from(maximum.max(0)).unwrap_or(usize::MAX);
    if output.is_null() || maximum == 0 {
        write_i64(result, 0);
        return;
    }
    let count = entries.len().min(maximum);
    let Some(get_destructor) = language.interface.variant_get_ptr_destructor else {
        write_i64(result, 0);
        return;
    };
    // SAFETY: StringName is an official Variant builtin type.
    let Some(destroy_string_name) =
        (unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME) })
    else {
        write_i64(result, 0);
        return;
    };
    let Some(construct_string_name) = language.interface.string_name_new_with_utf8_chars_and_len
    else {
        write_i64(result, 0);
        return;
    };
    for (index, entry) in entries.into_iter().take(count).enumerate() {
        // SAFETY: `index` is below the engine-provided capacity.
        let target = unsafe { output.add(index) };
        // SAFETY: Godot default-constructs each ProfilingInfo StringName.
        unsafe {
            destroy_string_name(ptr::addr_of_mut!((*target).signature).cast());
            construct_string_name(
                ptr::addr_of_mut!((*target).signature).cast(),
                entry.signature.as_ptr().cast(),
                i64::try_from(entry.signature.len()).unwrap_or(i64::MAX),
            );
            (*target).call_count = entry.call_count;
            (*target).total_time = entry.total_time;
            (*target).self_time = entry.self_time;
            (*target).internal_time = entry.internal_time;
        }
    }
    write_i64(result, i64::try_from(count).unwrap_or(i64::MAX));
}

unsafe extern "C" fn profiling_get_accumulated_data(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_profiling_data(instance, arguments, result, crate::profiler::accumulated());
}

unsafe extern "C" fn profiling_get_frame_data(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_profiling_data(instance, arguments, result, crate::profiler::frame());
}

unsafe extern "C" fn open_in_external_editor(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        write_i64(result, ERR_CANT_OPEN);
        return;
    };
    if arguments.is_null() {
        write_i64(result, ERR_CANT_OPEN);
        return;
    }
    // SAFETY: The official virtual transports Script as an Object pointer,
    // followed by line and column in ptrcall integer storage.
    let script_storage = unsafe { *arguments };
    // SAFETY: See above.
    let line_storage = unsafe { *arguments.add(1) };
    // SAFETY: See above.
    let column_storage = unsafe { *arguments.add(2) };
    if script_storage.is_null() || line_storage.is_null() || column_storage.is_null() {
        write_i64(result, ERR_CANT_OPEN);
        return;
    }
    // SAFETY: Each pointer has the matching official ptrcall representation.
    let script = unsafe { script_storage.cast::<*mut c_void>().read() };
    // SAFETY: Godot represents ptrcall integers in an i64 slot.
    let line = unsafe { line_storage.cast::<i64>().read() }.max(0) + 1;
    // SAFETY: See above.
    let column = unsafe { column_storage.cast::<i64>().read() }.max(0) + 1;
    let Some(resource_path) = crate::script::path_for_object(script) else {
        write_i64(result, ERR_CANT_OPEN);
        return;
    };
    let Some(relative) = resource_path.strip_prefix("res://") else {
        write_i64(result, ERR_CANT_OPEN);
        return;
    };
    let Ok(root) = crate::last_known_good::globalize_project_root(language.interface) else {
        write_i64(result, ERR_CANT_OPEN);
        return;
    };
    let source = root.join(relative);
    let Ok(root) = root.canonicalize() else {
        write_i64(result, ERR_CANT_OPEN);
        return;
    };
    let Ok(source) = source.canonicalize() else {
        write_i64(result, ERR_CANT_OPEN);
        return;
    };
    if !source.starts_with(&root) || !source.is_file() {
        write_i64(result, ERR_CANT_OPEN);
        return;
    }
    let Some(editor) = configured_external_editor() else {
        write_i64(result, ERR_UNAVAILABLE);
        return;
    };
    let opened = spawn_editor(&editor, &source, line, column);
    write_i64(result, if opened { 0 } else { ERR_UNAVAILABLE });
}

fn configured_external_editor() -> Option<PathBuf> {
    let editor = PathBuf::from(std::env::var_os("GODOT_RS_EXTERNAL_EDITOR")?);
    (editor.is_absolute() && editor.is_file()).then_some(editor)
}

fn spawn_editor(editor: &Path, source: &Path, line: i64, column: i64) -> bool {
    let name = editor
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let location = format!("{}:{line}:{column}", source.display());
    let mut command = Command::new(editor);
    if name.contains("code") {
        command.arg("--goto").arg(location);
    } else if name.contains("zed") {
        command.arg(location);
    } else if name.contains("rustrover") {
        command.arg("--line").arg(line.to_string()).arg(source);
    } else {
        command.arg(source);
    }
    command.spawn().is_ok()
}

unsafe extern "C" fn return_snake_case(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_i64(result, SCRIPT_NAME_CASING_SNAKE_CASE);
}

unsafe extern "C" fn find_function(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    write_i64(result, -1);
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        return;
    }
    // SAFETY: The official virtual has exactly two String arguments.
    let function = unsafe { *arguments };
    // SAFETY: See above.
    let source = unsafe { *arguments.add(1) };
    let Ok(function) = read_utf8_string(language.interface, function) else {
        return;
    };
    let Ok(source) = read_utf8_string(language.interface, source) else {
        return;
    };
    if let Some(line) = crate::rust_source::find_function_line(&source, &function) {
        let Ok(line) = i64::try_from(line) else {
            return;
        };
        write_i64(result, line);
        return;
    }
    let Some(script_type) = crate::rust_source::find_script_type(&source) else {
        return;
    };
    let Ok(mut context) = language.callback_context.lock() else {
        return;
    };
    *context = Some(CallbackContext {
        function_name: function,
        script_type: script_type.to_owned(),
    });
}

unsafe extern "C" fn make_function(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    crate::value::write_utf8_string(language.interface, result, "");
    if arguments.is_null() {
        return;
    }
    // SAFETY: The official virtual arguments are String, String, and
    // PackedStringArray.
    let class_name = unsafe { *arguments };
    // SAFETY: See above.
    let function_name = unsafe { *arguments.add(1) };
    // SAFETY: See above.
    let function_arguments = unsafe { *arguments.add(2) };
    let Ok(class_name) = read_utf8_string(language.interface, class_name) else {
        return;
    };
    let Ok(function_name) = read_utf8_string(language.interface, function_name) else {
        return;
    };
    let Some(function_arguments) = language.packed_string_reader.read(function_arguments) else {
        return;
    };
    let context = language
        .callback_context
        .lock()
        .ok()
        .and_then(|mut context| context.take());
    let script_type = context
        .filter(|context| context.function_name == function_name)
        .map(|context| context.script_type)
        .or_else(|| (!class_name.is_empty()).then_some(class_name))
        .unwrap_or_else(|| String::from("<unknown>"));
    let source = crate::signal_callback::render(
        &script_type,
        &function_name,
        &function_arguments,
        |class_name| is_godot_class(language.interface, class_name),
    )
    .unwrap_or_else(|error| {
        host_eprintln!("godot-rust signal callback generation failed: {error}");
        crate::signal_callback::render_failure(&script_type, &function_name, &error)
    });
    crate::value::write_utf8_string(language.interface, result, &source);
}

fn is_godot_class(interface: EngineInterface, class_name: &str) -> bool {
    let Some(class_name) = OwnedStringName::new(interface, class_name) else {
        return false;
    };
    let Some(get_class_tag) = interface.classdb_get_class_tag else {
        return false;
    };
    // SAFETY: The StringName remains initialized for this synchronous lookup.
    !unsafe { get_class_tag(class_name.as_ptr()) }.is_null()
}

unsafe extern "C" fn auto_indent_code(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    if arguments.is_null() {
        write_string(instance, result, c"");
        return;
    }
    // SAFETY: The official virtual arguments are String, int32, and int32;
    // ptrcall transports both integer metadata widths in i64 slots.
    let source = unsafe { *arguments };
    // SAFETY: See above.
    let from_line = unsafe { *arguments.add(1) };
    // SAFETY: See above.
    let to_line = unsafe { *arguments.add(2) };
    let Ok(source) = read_utf8_string(language.interface, source) else {
        write_string(instance, result, c"");
        return;
    };
    // SAFETY: Both integer argument pointers are live for this virtual call.
    let (from_line, to_line) = unsafe { (*from_line.cast::<i64>(), *to_line.cast::<i64>()) };
    let indented = crate::rust_source::auto_indent(&source, from_line, to_line);
    crate::value::write_utf8_string(language.interface, result, &indented);
}

unsafe extern "C" fn no_op(
    _instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
}

unsafe extern "C" fn frame(
    instance: GDExtensionClassInstancePtr,
    _args: *const GDExtensionConstTypePtr,
    _result: GDExtensionTypePtr,
) {
    let Some(language) = language(instance) else {
        return;
    };
    crate::profiler::next_frame();
    if let Some(generation) = crate::module_loader::active_generation() {
        let result = crate::script_instance::with_engine_interface(language.interface, || {
            generation.poll_tasks()
        });
        if let Err(error) = result {
            host_eprintln!(
                "godot-rust: cooperative task polling failed with {:?}: {}",
                error.status,
                error.message
            );
        }
    }
    let Ok(mut monitor) = language.reload_monitor.try_lock() else {
        return;
    };
    let now = Instant::now();
    if now < monitor.next_poll {
        return;
    }
    monitor.next_poll = now + RELOAD_POLL_INTERVAL;

    if let Some((build_id, candidate)) = monitor.pending.take() {
        match crate::script_instance::install_generation(candidate.clone()) {
            Ok(instances) => {
                host_println!(
                    "godot-rust: activated Rust build {build_id} and migrated {instances} live instance(s)"
                );
            }
            Err(crate::script_instance::GenerationReloadError::Busy) => {
                monitor.pending = Some((build_id, candidate));
            }
            Err(error) => {
                host_eprintln!(
                    "godot-rust: rejected Rust build {build_id}; keeping old code: {error}"
                );
            }
        }
        return;
    }

    let discovered = match monitor.watcher.as_mut().map(|watcher| watcher.poll()) {
        Some(Ok(Some(discovered))) => discovered,
        Some(Ok(None)) | None => return,
        Some(Err(error)) => {
            host_eprintln!("godot-rust: could not inspect the new Rust build: {error}");
            return;
        }
    };
    // SAFETY: Last Known Good constrains this path to a content-verified,
    // immutable project build selected by the local build service.
    let candidate = match unsafe {
        crate::module_loader::ModuleGeneration::load_for_engine(
            &discovered.path,
            language.interface.version(),
        )
    } {
        Ok(candidate) => candidate,
        Err(error) => {
            host_eprintln!(
                "godot-rust: rejected Rust build {}; keeping old code: {error}",
                discovered.build_id
            );
            return;
        }
    };
    match crate::script_instance::install_generation(candidate.clone()) {
        Ok(instances) => {
            host_println!(
                "godot-rust: activated Rust build {} and migrated {instances} live instance(s)",
                discovered.build_id
            );
        }
        Err(crate::script_instance::GenerationReloadError::Busy) => {
            monitor.pending = Some((discovered.build_id, candidate));
        }
        Err(error) => {
            host_eprintln!(
                "godot-rust: rejected Rust build {}; keeping old code: {error}",
                discovered.build_id
            );
        }
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

static LANGUAGE_VIRTUAL_METHODS: &[VirtualMethodSpec] = &[
    virtual_method!(c"_get_name", 201_670_096, get_name),
    virtual_method!(c"_init", 3_218_959_716, no_op),
    virtual_method!(c"_get_type", 201_670_096, get_type),
    virtual_method!(c"_get_extension", 201_670_096, get_extension),
    virtual_method!(c"_finish", 3_218_959_716, no_op),
    virtual_method!(c"_get_reserved_words", 1_139_954_409, get_reserved_words),
    virtual_method!(
        c"_is_control_flow_keyword",
        3_927_539_163,
        is_control_flow_keyword
    ),
    virtual_method!(
        c"_get_comment_delimiters",
        1_139_954_409,
        get_comment_delimiters
    ),
    virtual_method!(
        c"_get_doc_comment_delimiters",
        1_139_954_409,
        get_doc_comment_delimiters
    ),
    virtual_method!(
        c"_get_string_delimiters",
        1_139_954_409,
        get_string_delimiters
    ),
    virtual_method!(c"_make_template", 3_583_744_548, make_template),
    virtual_method!(
        c"_get_built_in_templates",
        3_147_814_860,
        return_empty_array
    ),
    virtual_method!(c"_is_using_templates", 2_240_911_060, return_false),
    virtual_method!(c"_validate", 1_697_887_509, validate),
    virtual_method!(c"_validate_path", 3_135_753_539, validate_path),
    virtual_method!(c"_create_script", 1_981_248_198, create_script),
    virtual_method!(c"_has_named_classes", 36_873_697, return_true),
    virtual_method!(c"_supports_builtin_mode", 36_873_697, return_false),
    virtual_method!(c"_supports_documentation", 36_873_697, return_true),
    virtual_method!(c"_can_inherit_from_file", 36_873_697, return_true),
    virtual_method!(c"_find_function", 2_878_152_881, find_function),
    virtual_method!(c"_make_function", 1_243_061_914, make_function),
    virtual_method!(c"_can_make_function", 36_873_697, return_true),
    virtual_method!(
        c"_open_in_external_editor",
        552_845_695,
        open_in_external_editor
    ),
    virtual_method!(
        c"_overrides_external_editor",
        2_240_911_060,
        overrides_external_editor
    ),
    virtual_method!(
        c"_preferred_file_name_casing",
        2_969_522_789,
        return_snake_case
    ),
    virtual_method!(c"_complete_code", 950_756_616, complete_code),
    virtual_method!(c"_lookup_code", 3_143_837_309, lookup_code),
    virtual_method!(c"_auto_indent_code", 2_531_480_354, auto_indent_code),
    virtual_method!(c"_add_global_constant", 3_776_071_444, no_op),
    virtual_method!(c"_add_named_global_constant", 3_776_071_444, no_op),
    virtual_method!(c"_remove_named_global_constant", 3_304_788_590, no_op),
    virtual_method!(c"_thread_enter", 3_218_959_716, no_op),
    virtual_method!(c"_thread_exit", 3_218_959_716, no_op),
    virtual_method!(c"_debug_get_error", 201_670_096, debug_get_error),
    virtual_method!(
        c"_debug_get_stack_level_count",
        3_905_245_786,
        debug_get_stack_level_count
    ),
    virtual_method!(
        c"_debug_get_stack_level_line",
        923_996_154,
        debug_get_stack_level_line
    ),
    virtual_method!(
        c"_debug_get_stack_level_function",
        844_755_477,
        debug_get_stack_level_function
    ),
    virtual_method!(
        c"_debug_get_stack_level_source",
        844_755_477,
        debug_get_stack_level_source
    ),
    virtual_method!(
        c"_debug_get_stack_level_locals",
        335_235_777,
        debug_get_stack_level_locals
    ),
    virtual_method!(
        c"_debug_get_stack_level_members",
        335_235_777,
        debug_get_stack_level_members
    ),
    virtual_method!(
        c"_debug_get_stack_level_instance",
        3_744_713_108,
        debug_get_stack_level_instance
    ),
    virtual_method!(
        c"_debug_get_globals",
        4_123_630_098,
        return_empty_dictionary
    ),
    virtual_method!(
        c"_debug_parse_stack_level_expression",
        1_135_811_067,
        debug_parse_stack_level_expression
    ),
    virtual_method!(
        c"_debug_get_current_stack_info",
        2_915_620_761,
        debug_get_current_stack_info
    ),
    virtual_method!(c"_reload_all_scripts", 3_218_959_716, reload_scripts),
    virtual_method!(c"_reload_scripts", 3_156_113_851, reload_scripts),
    virtual_method!(c"_reload_tool_script", 1_957_307_671, reload_scripts),
    virtual_method!(
        c"_get_recognized_extensions",
        1_139_954_409,
        get_recognized_extensions
    ),
    virtual_method!(c"_get_public_functions", 3_995_934_104, return_empty_array),
    virtual_method!(
        c"_get_public_constants",
        3_102_165_223,
        return_empty_dictionary
    ),
    virtual_method!(
        c"_get_public_annotations",
        3_995_934_104,
        return_empty_array
    ),
    virtual_method!(c"_profiling_start", 3_218_959_716, profiling_start),
    virtual_method!(c"_profiling_stop", 3_218_959_716, profiling_stop),
    virtual_method!(
        c"_profiling_set_save_native_calls",
        2_586_408_642,
        profiling_set_save_native_calls
    ),
    virtual_method!(
        c"_profiling_get_accumulated_data",
        50_157_827,
        profiling_get_accumulated_data
    ),
    virtual_method!(
        c"_profiling_get_frame_data",
        50_157_827,
        profiling_get_frame_data
    ),
    virtual_method!(c"_frame", 3_218_959_716, frame),
    virtual_method!(
        c"_handles_global_class_type",
        3_927_539_163,
        handles_global_class_type
    ),
    virtual_method!(
        c"_get_global_class_name",
        2_248_993_622,
        get_global_class_name
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_language_virtual_has_a_unique_name() {
        let mut names = HashSet::new();
        for method in LANGUAGE_VIRTUAL_METHODS {
            assert!(
                names.insert(method.name.to_bytes()),
                "duplicate virtual {:?}",
                method.name
            );
            assert!(method.callback.is_some());
        }
        assert_eq!(names.len(), 60);
    }

    #[test]
    fn validation_functions_use_godots_one_based_navigation_format() {
        let source = "fn first() {}\n\nfn second() {}\n";
        assert_eq!(
            validation_function_entries(source),
            vec!["first:1".to_owned(), "second:3".to_owned()]
        );
        assert!(validation_function_entries(source).iter().all(|entry| {
            entry
                .rsplit_once(':')
                .is_some_and(|(_, line)| line.parse::<usize>().is_ok_and(|line| line > 0))
        }));
    }

    #[test]
    fn bootstrap_virtuals_match_the_official_hashes() {
        for (name, hash) in [
            (c"_get_name".to_bytes(), 201_670_096),
            (c"_get_type".to_bytes(), 201_670_096),
            (c"_get_extension".to_bytes(), 201_670_096),
            (c"_get_recognized_extensions".to_bytes(), 1_139_954_409),
        ] {
            assert!(
                LANGUAGE_VIRTUAL_METHODS
                    .iter()
                    .any(|method| { method.name.to_bytes() == name && method.hash == hash })
            );
        }
    }

    #[test]
    fn editor_keyword_virtuals_use_real_callbacks() {
        for (name, callback) in [
            (
                c"_get_reserved_words".to_bytes(),
                get_reserved_words as *const () as usize,
            ),
            (
                c"_is_control_flow_keyword".to_bytes(),
                is_control_flow_keyword as *const () as usize,
            ),
        ] {
            let method = LANGUAGE_VIRTUAL_METHODS
                .iter()
                .find(|method| method.name.to_bytes() == name)
                .expect("editor virtual must be registered");
            assert_eq!(
                method
                    .callback
                    .map(|callback| callback as *const () as usize),
                Some(callback)
            );
        }
    }

    #[test]
    fn editor_delimiter_virtuals_use_real_callbacks() {
        for (name, callback) in [
            (
                c"_get_comment_delimiters".to_bytes(),
                get_comment_delimiters as *const () as usize,
            ),
            (
                c"_get_doc_comment_delimiters".to_bytes(),
                get_doc_comment_delimiters as *const () as usize,
            ),
            (
                c"_get_string_delimiters".to_bytes(),
                get_string_delimiters as *const () as usize,
            ),
        ] {
            let method = LANGUAGE_VIRTUAL_METHODS
                .iter()
                .find(|method| method.name.to_bytes() == name)
                .expect("editor virtual must be registered");
            assert_eq!(
                method
                    .callback
                    .map(|callback| callback as *const () as usize),
                Some(callback)
            );
        }
    }

    #[test]
    fn external_editor_override_is_opt_in() {
        let method = LANGUAGE_VIRTUAL_METHODS
            .iter()
            .find(|method| method.name.to_bytes() == c"_overrides_external_editor".to_bytes())
            .expect("external editor override virtual must be registered");
        assert_eq!(
            method
                .callback
                .map(|callback| callback as *const () as usize),
            Some(overrides_external_editor as *const () as usize)
        );
    }

    #[test]
    fn function_navigation_uses_the_rust_scanner() {
        let method = LANGUAGE_VIRTUAL_METHODS
            .iter()
            .find(|method| method.name.to_bytes() == c"_find_function".to_bytes())
            .expect("function finder must be registered");
        assert_eq!(
            method
                .callback
                .map(|callback| callback as *const () as usize),
            Some(find_function as *const () as usize)
        );
        assert_eq!(
            crate::rust_source::find_function_line(
                "#[script]\nimpl Player {\n\tfn _ready(&mut self) {}\n}",
                "_ready"
            ),
            Some(2)
        );
    }

    #[test]
    fn signal_callback_virtuals_use_the_rust_generator() {
        for (name, callback) in [
            (
                c"_make_function".to_bytes(),
                make_function as *const () as usize,
            ),
            (
                c"_can_make_function".to_bytes(),
                return_true as *const () as usize,
            ),
        ] {
            let method = LANGUAGE_VIRTUAL_METHODS
                .iter()
                .find(|method| method.name.to_bytes() == name)
                .expect("signal callback virtual must be registered");
            assert_eq!(
                method
                    .callback
                    .map(|callback| callback as *const () as usize),
                Some(callback)
            );
        }
    }

    #[test]
    fn automatic_indentation_returns_source_through_a_real_callback() {
        let method = LANGUAGE_VIRTUAL_METHODS
            .iter()
            .find(|method| method.name.to_bytes() == c"_auto_indent_code".to_bytes())
            .expect("auto indentation must be registered");
        assert_eq!(
            method
                .callback
                .map(|callback| callback as *const () as usize),
            Some(auto_indent_code as *const () as usize)
        );
        assert_eq!(
            crate::rust_source::auto_indent("impl Player {\nfn ready() {}\n}", 1, 1),
            "impl Player {\n\tfn ready() {}\n}"
        );
    }

    #[test]
    fn debugger_and_profiler_virtuals_use_runtime_callbacks() {
        for (name, callback) in [
            (
                c"_debug_get_error".to_bytes(),
                debug_get_error as *const () as usize,
            ),
            (
                c"_debug_get_stack_level_locals".to_bytes(),
                debug_get_stack_level_locals as *const () as usize,
            ),
            (
                c"_debug_get_stack_level_members".to_bytes(),
                debug_get_stack_level_members as *const () as usize,
            ),
            (
                c"_profiling_get_accumulated_data".to_bytes(),
                profiling_get_accumulated_data as *const () as usize,
            ),
            (
                c"_profiling_get_frame_data".to_bytes(),
                profiling_get_frame_data as *const () as usize,
            ),
        ] {
            let method = LANGUAGE_VIRTUAL_METHODS
                .iter()
                .find(|method| method.name.to_bytes() == name)
                .expect("runtime virtual must be registered");
            assert_eq!(
                method
                    .callback
                    .map(|callback| callback as *const () as usize),
                Some(callback)
            );
        }
        assert_eq!(core::mem::size_of::<GodotProfilingInfo>(), 40);
        assert_eq!(core::mem::align_of::<GodotProfilingInfo>(), 8);
    }
}
