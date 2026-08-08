use core::ffi::c_void;
use core::fmt;
use core::ptr;
use godot_rs_api::{
    GDExtensionConstTypePtr, GDExtensionMethodBindPtr, GDExtensionObjectPtr, GDExtensionTypePtr,
};

use crate::interface::EngineInterface;
use crate::last_known_good;
use crate::module_loader::{self, ModuleGeneration, ModuleLoadError};
use crate::registry::{ClassRegistry, RegistryError};
use crate::resource_loader;
use crate::resource_saver;
use crate::script;
use crate::script_language;
use crate::string_name::StaticStringName;

const OK: i64 = 0;

#[derive(Debug)]
pub(crate) enum RuntimeError {
    Registry(RegistryError),
    MissingSingleton(&'static str),
    MissingMethod {
        class: &'static str,
        method: &'static str,
        hash: i64,
    },
    ScriptLanguageRegistration(i64),
    ProjectModule(ModuleLoadError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => error.fmt(formatter),
            Self::MissingSingleton(singleton) => {
                write!(formatter, "Godot did not expose its {singleton} singleton")
            }
            Self::MissingMethod {
                class,
                method,
                hash,
            } => write!(
                formatter,
                "Godot method `{class}.{method}` with hash {hash} is unavailable"
            ),
            Self::ScriptLanguageRegistration(error) => write!(
                formatter,
                "Godot rejected the Rust script language with Error code {error}"
            ),
            Self::ProjectModule(error) => error.fmt(formatter),
        }
    }
}

impl From<RegistryError> for RuntimeError {
    fn from(value: RegistryError) -> Self {
        Self::Registry(value)
    }
}

impl From<ModuleLoadError> for RuntimeError {
    fn from(value: ModuleLoadError) -> Self {
        Self::ProjectModule(value)
    }
}

pub(crate) struct HostRuntime {
    interface: EngineInterface,
    registry: ClassRegistry,
    engine: usize,
    language: usize,
    unregister_language: usize,
    resource_loader_singleton: usize,
    format_loader: usize,
    remove_format_loader: usize,
    resource_saver_singleton: usize,
    format_saver: usize,
    remove_format_saver: usize,
    language_registered: bool,
    module_generation: Option<ModuleGeneration>,
}

impl HostRuntime {
    pub(crate) fn start(interface: EngineInterface) -> Result<Self, RuntimeError> {
        let mut registry = ClassRegistry::new(interface)?;
        let engine_name = StaticStringName::new(interface, c"Engine");
        let get_singleton = interface
            .global_get_singleton
            .expect("required singleton lookup was loaded");
        // SAFETY: `engine_name` is an initialized official StringName.
        let engine = unsafe { get_singleton(engine_name.as_ptr()) };
        if engine.is_null() {
            return Err(RuntimeError::MissingSingleton("Engine"));
        }

        let register_language = resolve_method(
            interface,
            c"Engine",
            c"register_script_language",
            1_850_254_898,
        )?;
        let unregister_language = resolve_method(
            interface,
            c"Engine",
            c"unregister_script_language",
            1_850_254_898,
        )?;
        let resource_loader_name = StaticStringName::new(interface, c"ResourceLoader");
        // SAFETY: ResourceLoader is an official engine singleton.
        let resource_loader_singleton = unsafe { get_singleton(resource_loader_name.as_ptr()) };
        if resource_loader_singleton.is_null() {
            return Err(RuntimeError::MissingSingleton("ResourceLoader"));
        }
        let add_format_loader = resolve_method(
            interface,
            c"ResourceLoader",
            c"add_resource_format_loader",
            2_896_595_483,
        )?;
        let remove_format_loader = resolve_method(
            interface,
            c"ResourceLoader",
            c"remove_resource_format_loader",
            405_397_102,
        )?;
        let resource_saver_name = StaticStringName::new(interface, c"ResourceSaver");
        // SAFETY: ResourceSaver is an official engine singleton.
        let resource_saver_singleton = unsafe { get_singleton(resource_saver_name.as_ptr()) };
        if resource_saver_singleton.is_null() {
            return Err(RuntimeError::MissingSingleton("ResourceSaver"));
        }
        let add_format_saver = resolve_method(
            interface,
            c"ResourceSaver",
            c"add_resource_format_saver",
            362_894_272,
        )?;
        let remove_format_saver = resolve_method(
            interface,
            c"ResourceSaver",
            c"remove_resource_format_saver",
            3_373_026_878,
        )?;
        let module_generation = if let Some(path) = std::env::var_os("GODOT_RS_PROJECT_MODULE") {
            Some(
                // SAFETY: This opt-in path is produced by the trusted local
                // project build and explicitly selected for this Host process.
                unsafe {
                    ModuleGeneration::load_for_engine(
                        std::path::Path::new(&path),
                        interface.version(),
                    )
                }
                .map_err(RuntimeError::from)?,
            )
        } else {
            match last_known_good::safe_mode_enabled(interface) {
                Ok(true) => {
                    host_eprintln!(
                        "godot-rust: Safe Mode is active; project module loading is disabled"
                    );
                    None
                }
                Err(error) => {
                    host_eprintln!(
                        "godot-rust: project module loading was disabled because Safe Mode state could not be verified: {error}"
                    );
                    None
                }
                #[cfg(target_os = "emscripten")]
                Ok(false) => {
                    // SAFETY: Godot's Web bootstrap preloads the project side
                    // module listed by the export plugin before Host startup.
                    match unsafe { ModuleGeneration::load_exported_for_engine(interface.version()) }
                    {
                        Ok(generation) => Some(generation),
                        Err(error) => {
                            host_eprintln!(
                                "godot-rust: preloaded Web project module could not be loaded: {error}"
                            );
                            None
                        }
                    }
                }
                #[cfg(not(target_os = "emscripten"))]
                Ok(false) => match last_known_good::discover(interface) {
                    Ok(Some(path)) => {
                        // SAFETY: Development modules are constrained to a
                        // content-verified managed generation. Export modules use
                        // the application's exact native-library sidecar path.
                        match unsafe {
                            ModuleGeneration::load_for_engine(&path, interface.version())
                        } {
                            Ok(generation) => Some(generation),
                            Err(error) => {
                                host_eprintln!(
                                    "godot-rust: Rust project module could not be loaded: {error}"
                                );
                                None
                            }
                        }
                    }
                    Ok(None) => None,
                    Err(error) => {
                        host_eprintln!(
                            "godot-rust: Rust project module was not discovered: {error}"
                        );
                        None
                    }
                },
            }
        };

        // Resolve every fallible engine dependency before classes or objects
        // are installed, so an early return cannot leave ClassDB state behind.
        let _script_class = script::register_class(&mut registry);
        let language_class = script_language::register_class(&mut registry);
        let format_loader_class = resource_loader::register_class(&mut registry);
        let format_saver_class = resource_saver::register_class(&mut registry);
        let language = registry.instantiate(language_class)?;
        script::set_language_object(language);

        let error = call_language_method(interface, register_language, engine, language);
        if error != OK {
            script::clear_language_object();
            destroy(interface, language);
            return Err(RuntimeError::ScriptLanguageRegistration(error));
        }
        let format_loader = match registry.instantiate(format_loader_class) {
            Ok(loader) => loader,
            Err(error) => {
                let _ = call_language_method(interface, unregister_language, engine, language);
                script::clear_language_object();
                destroy(interface, language);
                return Err(error.into());
            }
        };
        call_format_loader_method(
            interface,
            add_format_loader,
            resource_loader_singleton,
            format_loader,
            true,
        );
        let format_saver = match registry.instantiate(format_saver_class) {
            Ok(saver) => saver,
            Err(error) => {
                call_format_loader_method(
                    interface,
                    remove_format_loader,
                    resource_loader_singleton,
                    format_loader,
                    false,
                );
                let _ = call_language_method(interface, unregister_language, engine, language);
                script::clear_language_object();
                destroy(interface, language);
                return Err(error.into());
            }
        };
        call_format_loader_method(
            interface,
            add_format_saver,
            resource_saver_singleton,
            format_saver,
            true,
        );
        if let Some(generation) = &module_generation {
            module_loader::set_active_generation(generation.clone());
        }

        Ok(Self {
            interface,
            registry,
            engine: engine as usize,
            language: language as usize,
            unregister_language: unregister_language as usize,
            resource_loader_singleton: resource_loader_singleton as usize,
            format_loader: format_loader as usize,
            remove_format_loader: remove_format_loader as usize,
            resource_saver_singleton: resource_saver_singleton as usize,
            format_saver: format_saver as usize,
            remove_format_saver: remove_format_saver as usize,
            language_registered: true,
            module_generation,
        })
    }

    pub(crate) fn shutdown(&mut self) {
        module_loader::clear_active_generation();
        let format_saver = self.format_saver as GDExtensionObjectPtr;
        if !format_saver.is_null() {
            call_format_loader_method(
                self.interface,
                self.remove_format_saver as GDExtensionMethodBindPtr,
                self.resource_saver_singleton as GDExtensionObjectPtr,
                format_saver,
                false,
            );
            self.format_saver = 0;
        }
        let format_loader = self.format_loader as GDExtensionObjectPtr;
        if !format_loader.is_null() {
            call_format_loader_method(
                self.interface,
                self.remove_format_loader as GDExtensionMethodBindPtr,
                self.resource_loader_singleton as GDExtensionObjectPtr,
                format_loader,
                false,
            );
            self.format_loader = 0;
        }
        let language = self.language as GDExtensionObjectPtr;
        if self.language_registered && !language.is_null() {
            let _ = call_language_method(
                self.interface,
                self.unregister_language as GDExtensionMethodBindPtr,
                self.engine as GDExtensionObjectPtr,
                language,
            );
            self.language_registered = false;
        }
        if !language.is_null() {
            script::clear_language_object();
            destroy(self.interface, language);
            self.language = 0;
        }
        self.registry.unregister_all();
        self.module_generation = None;
    }
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn resolve_method(
    interface: EngineInterface,
    class_name: &'static core::ffi::CStr,
    method_name: &'static core::ffi::CStr,
    hash: i64,
) -> Result<GDExtensionMethodBindPtr, RuntimeError> {
    let class = StaticStringName::new(interface, class_name);
    let method = StaticStringName::new(interface, method_name);
    let get_method = interface
        .classdb_get_method_bind
        .expect("required method bind lookup was loaded");
    // SAFETY: The StringNames are initialized and the hash is from the official
    // Godot 4.4 API.
    let method_bind = unsafe { get_method(class.as_ptr(), method.as_ptr(), hash) };
    if method_bind.is_null() {
        return Err(RuntimeError::MissingMethod {
            class: class_name
                .to_str()
                .expect("Host class names are reviewed ASCII literals"),
            method: method_name
                .to_str()
                .expect("Host method names are reviewed ASCII literals"),
            hash,
        });
    }
    Ok(method_bind)
}

fn call_format_loader_method(
    interface: EngineInterface,
    method: GDExtensionMethodBindPtr,
    singleton: GDExtensionObjectPtr,
    loader: GDExtensionObjectPtr,
    add: bool,
) {
    let loader_argument = loader;
    let at_front = u8::from(add);
    let arguments: [GDExtensionConstTypePtr; 2] = [
        (&loader_argument as *const GDExtensionObjectPtr).cast(),
        (&at_front as *const u8).cast(),
    ];
    let Some(ptrcall) = interface.object_method_bind_ptrcall else {
        return;
    };
    // SAFETY: Both methods accept the encoded Ref<ResourceFormatLoader>; add
    // consumes the additional bool while remove reads only the first element.
    unsafe {
        ptrcall(method, singleton, arguments.as_ptr(), ptr::null_mut());
    }
}

fn call_language_method(
    interface: EngineInterface,
    method: GDExtensionMethodBindPtr,
    engine: GDExtensionObjectPtr,
    language: GDExtensionObjectPtr,
) -> i64 {
    let language_argument = language;
    let arguments: [GDExtensionConstTypePtr; 1] =
        [(&language_argument as *const GDExtensionObjectPtr).cast()];
    let mut error = i64::MIN;
    let ptrcall = interface
        .object_method_bind_ptrcall
        .expect("required ptrcall interface was loaded");
    // SAFETY: The method bind has Engine.(un)register_script_language's exact
    // signature; Object arguments are encoded as pointers-to-object-pointers
    // and Error results as i64 by Godot's official ptrcall ABI.
    unsafe {
        ptrcall(
            method,
            engine,
            arguments.as_ptr(),
            (&mut error as *mut i64).cast::<c_void>() as GDExtensionTypePtr,
        );
    }
    error
}

fn destroy(interface: EngineInterface, object: GDExtensionObjectPtr) {
    if object.is_null() {
        return;
    }
    let destroy = interface
        .object_destroy
        .expect("required object destroy interface was loaded");
    // SAFETY: The Host exclusively owns this non-refcounted language object.
    unsafe { destroy(object) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_errors_explain_the_failed_engine_operation() {
        assert_eq!(
            RuntimeError::MissingSingleton("Engine").to_string(),
            "Godot did not expose its Engine singleton"
        );
        assert_eq!(
            RuntimeError::ScriptLanguageRegistration(22).to_string(),
            "Godot rejected the Rust script language with Error code 22"
        );
    }
}
