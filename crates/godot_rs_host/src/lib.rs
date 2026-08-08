#![doc = "Godot-loaded Host for the godot-rust script language."]

macro_rules! host_eprintln {
    ($($argument:tt)*) => {
        crate::logging::error(format_args!($($argument)*))
    };
}

macro_rules! host_println {
    ($($argument:tt)*) => {
        crate::logging::module(
            godot_rs_api::abi::AbiLogLevel::Info,
            format_args!($($argument)*),
        )
    };
}

mod callable_value;
mod debugger;
mod dynamic_value;
mod engine_call;
mod entry;
mod file_access;
mod godot_metadata;
mod interface;
mod last_known_good;
mod logging;
mod module_loader;
mod module_value;
mod node_path;
mod node_resolver;
mod packed_array;
mod packed_string_array;
mod profiler;
mod registry;
mod resource_loader;
mod resource_saver;
mod resource_uid;
mod runtime;
mod rust_source;
mod script;
mod script_instance;
mod script_language;
mod script_template;
mod signal_callback;
mod signal_value;
mod signal_wait;
mod string_name;
mod value;
mod variant_codec;
mod version;

pub use entry::godot_rust_init;

/// Validates a trusted Script Mode project module without starting Godot.
///
/// This entry is used by repository and build-daemon diagnostics. Loading a
/// native module executes its platform loader initialization.
#[doc(hidden)]
pub fn validate_project_module(path: &std::path::Path) -> Result<usize, String> {
    // SAFETY: Callers only pass trusted local project build artifacts.
    let generation = unsafe { module_loader::ModuleGeneration::load(path) }
        .map_err(|error| error.to_string())?;
    Ok(generation.script_count())
}

/// Loads one trusted module and exercises its frame-start lifecycle slots.
#[doc(hidden)]
pub fn exercise_project_module_callbacks(
    path: &std::path::Path,
    source_path: &str,
) -> Result<(), String> {
    // SAFETY: Callers only pass trusted local project build artifacts.
    let generation = unsafe { module_loader::ModuleGeneration::load(path) }
        .map_err(|error| error.to_string())?;
    let script = generation
        .script(source_path)
        .ok_or_else(|| format!("module has no script descriptor for `{source_path}`"))?;
    if script.resource_uid() < 0 {
        return Err("module script has no persistent Godot Resource UID".into());
    }
    let signal = (0..script.field_count())
        .find_map(|index| {
            let field = script.field(index)?;
            (field.name() == "counter_changed" && field.is_signal()).then_some(field)
        })
        .ok_or_else(|| "module has no structured `counter_changed` signal".to_owned())?;
    let signal_arguments = signal.signal_arguments().collect::<Vec<_>>();
    if signal_arguments
        != [
            ("old_value", godot_rs_api::abi::AbiValueType::I64),
            ("new_value", godot_rs_api::abi::AbiValueType::I64),
        ]
    {
        return Err("signal `counter_changed` has an unexpected argument schema".into());
    }
    let mut state = script.create_state().map_err(|error| {
        format!(
            "script state creation failed with {:?}: {}",
            error.status, error.message
        )
    })?;
    state.ready().map_err(|error| {
        format!(
            "script `_ready` failed with {:?}: {}",
            error.status, error.message
        )
    })?;
    state.process(1.0 / 60.0).map_err(|error| {
        format!(
            "script `_process` failed with {:?}: {}",
            error.status, error.message
        )
    })?;

    let find_method = |name: &str| {
        (0..script.method_count()).find_map(|index| {
            let method = script.method(index)?;
            (method.name() == name).then_some(method)
        })
    };
    let sum = find_method("sum_values")
        .ok_or_else(|| "module has no reflected `sum_values` method".to_owned())?;
    let output = state
        .call_method(
            &sum,
            &[
                godot_rs_api::abi::AbiValueV1::from_i64(20),
                godot_rs_api::abi::AbiValueV1::from_i64(0),
                godot_rs_api::abi::AbiValueV1::from_i64(22),
            ],
        )
        .map_err(|error| {
            format!(
                "script `sum_values` failed with {:?}: {}",
                error.status, error.message
            )
        })?;
    let output = output.abi();
    if output.type_ != godot_rs_api::abi::AbiValueType::I64 || output.payload[0] as i64 != 42 {
        return Err("script `sum_values` returned an unexpected value".into());
    }
    let counter = (0..script.field_count())
        .find_map(|index| {
            let field = script.field(index)?;
            (field.name() == "counter").then_some(field)
        })
        .ok_or_else(|| "module has no exported `counter` field".to_owned())?;
    let initial_counter = state.get_field(&counter).map_err(|error| {
        format!(
            "field `counter` get failed with {:?}: {}",
            error.status, error.message
        )
    })?;
    if initial_counter.abi() != godot_rs_api::abi::AbiValueV1::from_i64(3) {
        return Err("field `counter` did not expose its generated default".into());
    }
    state
        .set_field(&counter, godot_rs_api::abi::AbiValueV1::from_i64(91))
        .map_err(|error| {
            format!(
                "field `counter` set failed with {:?}: {}",
                error.status, error.message
            )
        })?;
    let updated_counter = state.get_field(&counter).map_err(|error| {
        format!(
            "updated field `counter` get failed with {:?}: {}",
            error.status, error.message
        )
    })?;
    if updated_counter.abi() != godot_rs_api::abi::AbiValueV1::from_i64(91) {
        return Err("field `counter` did not retain its updated value".into());
    }
    Ok(())
}
