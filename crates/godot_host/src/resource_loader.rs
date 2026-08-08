use core::ffi::c_void;
use core::ptr;
use godot_api::{
    GDExtensionClassInstancePtr, GDExtensionConstTypePtr, GDExtensionMethodBindPtr,
    GDExtensionTypePtr, GDExtensionVariantFromTypeConstructorFunc, GDExtensionVariantType,
};
use std::collections::HashMap;
use std::path::Path;

use crate::interface::EngineInterface;
use crate::packed_string_array::PackedStringArrayWriter;
use crate::registry::{ClassRegistry, ClassSpec, RegisteredClassId, VirtualMethodSpec};
use crate::resource_uid::{INVALID_RESOURCE_UID, read_uid_file};
use crate::script;
use crate::string_name::StaticStringName;
use crate::value::write_utf8_string;
use crate::value::{LocalGodotString, read_utf8_string, write_latin1_string, write_nil_variant};

const OK: i64 = 0;
const ERR_FILE_CANT_WRITE: i64 = 13;
const ERR_INVALID_DATA: i64 = 30;
const ERR_PARSE_ERROR: i64 = 43;

pub(crate) struct RustResourceLoader {
    interface: EngineInterface,
    script_class: StaticStringName,
    script_type: StaticStringName,
    rust_script_type: StaticStringName,
    notification_method: usize,
    globalize_path_method: usize,
    project_settings: usize,
    packed_strings: PackedStringArrayWriter,
    object_to_variant: GDExtensionVariantFromTypeConstructorFunc,
}

pub(crate) fn register_class(registry: &mut ClassRegistry) -> RegisteredClassId {
    registry.register(ClassSpec {
        name: c"GodotRustResourceLoader",
        parent: c"ResourceFormatLoader",
        factory: create_loader_instance,
        dropper: drop_loader_instance,
        virtual_methods: LOADER_VIRTUAL_METHODS,
    })
}

fn create_loader_instance(interface: EngineInterface, _object: *mut c_void) -> *mut c_void {
    let Some(get_method) = interface.classdb_get_method_bind else {
        return ptr::null_mut();
    };
    let Some(get_variant_constructor) = interface.get_variant_from_type_constructor else {
        return ptr::null_mut();
    };
    let Some(get_singleton) = interface.global_get_singleton else {
        return ptr::null_mut();
    };

    let object_class = StaticStringName::new(interface, c"Object");
    let notification_name = StaticStringName::new(interface, c"notification");
    let project_settings_class = StaticStringName::new(interface, c"ProjectSettings");
    let globalize_path_name = StaticStringName::new(interface, c"globalize_path");

    // SAFETY: All method names and hashes are from the official 4.4 API.
    let notification_method = unsafe {
        get_method(
            object_class.as_ptr(),
            notification_name.as_ptr(),
            4_023_243_586,
        )
    };
    // SAFETY: Name and hash match ProjectSettings.globalize_path(String).
    let globalize_path_method = unsafe {
        get_method(
            project_settings_class.as_ptr(),
            globalize_path_name.as_ptr(),
            3_135_753_539,
        )
    };
    // SAFETY: This singleton name is defined by Godot.
    let project_settings = unsafe { get_singleton(project_settings_class.as_ptr()) };
    let Some(packed_strings) = PackedStringArrayWriter::new(interface) else {
        return ptr::null_mut();
    };
    // SAFETY: Object is an official Variant type.
    let object_to_variant =
        unsafe { get_variant_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT) };

    if notification_method.is_null()
        || globalize_path_method.is_null()
        || project_settings.is_null()
        || object_to_variant.is_none()
    {
        return ptr::null_mut();
    }

    Box::into_raw(Box::new(RustResourceLoader {
        interface,
        script_class: StaticStringName::new(interface, c"GodotRustScript"),
        script_type: StaticStringName::new(interface, c"Script"),
        rust_script_type: StaticStringName::new(interface, c"GodotRustScript"),
        notification_method: notification_method as usize,
        globalize_path_method: globalize_path_method as usize,
        project_settings: project_settings as usize,
        packed_strings,
        object_to_variant,
    }))
    .cast()
}

unsafe fn drop_loader_instance(instance: *mut c_void) {
    // SAFETY: This allocation comes from `create_loader_instance`.
    unsafe { drop(Box::from_raw(instance.cast::<RustResourceLoader>())) };
}

fn loader(instance: GDExtensionClassInstancePtr) -> Option<&'static RustResourceLoader> {
    if instance.is_null() {
        return None;
    }
    // SAFETY: ClassDB supplies the matching live extension instance.
    Some(unsafe { &*instance.cast::<RustResourceLoader>() })
}

impl RustResourceLoader {
    fn path_argument(&self, arguments: *const GDExtensionConstTypePtr) -> Option<String> {
        if arguments.is_null() {
            return None;
        }
        // SAFETY: Every caller passes a virtual whose first argument is String.
        let path = unsafe { *arguments };
        read_utf8_string(self.interface, path).ok()
    }

    fn script_source(
        &self,
        arguments: *const GDExtensionConstTypePtr,
    ) -> Option<(GDExtensionConstTypePtr, String, String)> {
        if arguments.is_null() {
            return None;
        }
        // SAFETY: Every resource query has a String path as its first argument.
        let path = unsafe { *arguments };
        let resource_path = read_utf8_string(self.interface, path).ok()?;
        if !resource_path.ends_with(".rs") {
            return None;
        }
        let source = crate::file_access::read_text(self.interface, path).ok()?;
        Some((path, resource_path, source))
    }

    fn globalize_path(&self, path: GDExtensionConstTypePtr) -> Option<String> {
        let mut result = LocalGodotString::new(self.interface, c"")?;
        let arguments = [path];
        let ptrcall = self.interface.object_method_bind_ptrcall?;
        // SAFETY: Method bind and argument match
        // ProjectSettings.globalize_path(String) -> String.
        unsafe {
            ptrcall(
                self.globalize_path_method as GDExtensionMethodBindPtr,
                self.project_settings as *mut c_void,
                arguments.as_ptr(),
                result.as_mut_ptr(),
            );
        }
        result.to_utf8().ok()
    }

    fn postinitialize(&self, object: *mut c_void) {
        let notification = 0_i64;
        let reversed = 0_u8;
        let arguments: [GDExtensionConstTypePtr; 2] = [
            (&notification as *const i64).cast(),
            (&reversed as *const u8).cast(),
        ];
        let Some(ptrcall) = self.interface.object_method_bind_ptrcall else {
            return;
        };
        // SAFETY: Bind and arguments match Object.notification(int, bool).
        unsafe {
            ptrcall(
                self.notification_method as GDExtensionMethodBindPtr,
                object,
                arguments.as_ptr(),
                ptr::null_mut(),
            );
        }
    }

    fn load_script(&self, path: &str, resource_uid: i64, source: &str, result: GDExtensionTypePtr) {
        if result.is_null() {
            return;
        }
        let Some(construct) = self.interface.classdb_construct_object2 else {
            write_nil_variant(self.interface, result);
            return;
        };
        // SAFETY: The script class is registered before this loader is exposed.
        let script = unsafe { construct(self.script_class.as_ptr()) };
        if script.is_null() {
            write_nil_variant(self.interface, result);
            return;
        }
        self.postinitialize(script);

        if !script::initialize_loaded_script(script, path, resource_uid, source) {
            self.destroy(script);
            write_nil_variant(self.interface, result);
            return;
        }

        let Some(convert) = self.object_to_variant else {
            self.destroy(script);
            write_nil_variant(self.interface, result);
            return;
        };
        let object = script;
        // SAFETY: `result` is uninitialized Variant storage and the raw Object
        // representation is a pointer-to-object-pointer.
        unsafe {
            convert(result, (&object as *const *mut c_void).cast_mut().cast());
        }
    }

    fn destroy(&self, object: *mut c_void) {
        if let Some(destroy) = self.interface.object_destroy {
            // SAFETY: Used only for a just-created object that has not escaped.
            unsafe { destroy(object) };
        }
    }
}

unsafe extern "C" fn get_recognized_extensions(
    instance: GDExtensionClassInstancePtr,
    _arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        return;
    };
    loader.packed_strings.write(result, &["rs"]);
}

unsafe extern "C" fn recognize_path(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let recognized = loader(instance)
        .and_then(|loader| loader.path_argument(arguments))
        .is_some_and(|path| path.ends_with(".rs"));
    write_bool(result, recognized);
}

unsafe extern "C" fn handles_type(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        write_bool(result, false);
        return;
    };
    if arguments.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: This virtual's first argument is StringName.
    let type_name = unsafe { *arguments };
    write_bool(
        result,
        loader.script_type.equals(loader.interface, type_name)
            || loader.rust_script_type.equals(loader.interface, type_name),
    );
}

unsafe extern "C" fn get_resource_type(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        return;
    };
    let resource_type = if loader
        .path_argument(arguments)
        .is_some_and(|path| path.ends_with(".rs"))
    {
        c"GodotRustScript"
    } else {
        c""
    };
    write_latin1_string(loader.interface, result, resource_type);
}

unsafe extern "C" fn get_resource_script_class(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        return;
    };
    let class_name = loader
        .script_source(arguments)
        .and_then(|(_, path, source)| {
            crate::module_loader::active_script(&path)
                .and_then(|script| script.global_name().map(str::to_owned))
                .or_else(|| {
                    let script_type = crate::rust_source::find_script_type(&source)?;
                    crate::rust_source::script_source_attributes(&source, script_type).global_name
                })
        });
    if !write_utf8_string(
        loader.interface,
        result,
        class_name.as_deref().unwrap_or(""),
    ) {
        write_latin1_string(loader.interface, result, c"");
    }
}

unsafe extern "C" fn get_dependencies(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        return;
    };
    let Some((_, _, source)) = loader.script_source(arguments) else {
        loader.packed_strings.write_empty(result);
        return;
    };
    let Some(script_type) = crate::rust_source::find_script_type(&source) else {
        loader.packed_strings.write_empty(result);
        return;
    };
    let add_types = if arguments.is_null() {
        false
    } else {
        // SAFETY: `_get_dependencies` has a bool second argument.
        unsafe { (*arguments.add(1)).cast::<u8>().read() != 0 }
    };
    let attributes = crate::rust_source::script_source_attributes(&source, script_type);
    let dependencies = crate::rust_source::script_dependencies(&source, script_type)
        .into_iter()
        .map(|path| {
            if !add_types {
                return path;
            }
            if attributes.base_script_path.as_deref() == Some(path.as_str()) {
                format!("{path}::GodotRustScript")
            } else {
                format!("{path}::Resource")
            }
        })
        .collect::<Vec<_>>();
    let references = dependencies.iter().map(String::as_str).collect::<Vec<_>>();
    loader.packed_strings.write(result, &references);
}

unsafe extern "C" fn rename_dependencies(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        write_i64(result, ERR_INVALID_DATA);
        return;
    };
    let Some((raw_path, resource_path, source)) = loader.script_source(arguments) else {
        write_i64(result, ERR_INVALID_DATA);
        return;
    };
    let Some(script_type) = crate::rust_source::find_script_type(&source) else {
        write_i64(result, ERR_PARSE_ERROR);
        return;
    };
    if arguments.is_null() {
        write_i64(result, ERR_INVALID_DATA);
        return;
    }
    // SAFETY: `_rename_dependencies` has a Dictionary second argument.
    let dictionary = unsafe { *arguments.add(1) };
    let Ok(renames) = crate::dynamic_value::read_string_dictionary(loader.interface, dictionary)
    else {
        write_i64(result, ERR_INVALID_DATA);
        return;
    };
    let renames = HashMap::from_iter(renames);
    let renamed =
        match crate::rust_source::rename_script_dependencies(&source, script_type, &renames) {
            Ok(Some(source)) => source,
            Ok(None) => {
                write_i64(result, OK);
                return;
            }
            Err(_) => {
                write_i64(result, ERR_INVALID_DATA);
                return;
            }
        };
    let Some(path) = loader.globalize_path(raw_path) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    if crate::resource_saver::atomic_write(Path::new(&path), renamed.as_bytes()).is_err() {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    }
    script::update_source_for_path(&resource_path, &renamed);
    write_i64(result, OK);
}

unsafe extern "C" fn get_resource_uid(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let uid = loader(instance)
        .and_then(|loader| {
            loader
                .script_source(arguments)
                .map(|(path, _, _)| (loader, path))
        })
        .and_then(|(loader, path)| loader.globalize_path(path))
        .and_then(|path| read_uid_file(Path::new(&path)).ok().flatten())
        .unwrap_or(INVALID_RESOURCE_UID);
    write_i64(result, uid);
}

unsafe extern "C" fn get_classes_used(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        return;
    };
    let Some((_, path, source)) = loader.script_source(arguments) else {
        loader.packed_strings.write_empty(result);
        return;
    };
    let classes = crate::module_loader::active_script(&path)
        .map(|script| script.classes_used())
        .unwrap_or_else(|| {
            crate::rust_source::find_script_type(&source)
                .and_then(|script_type| {
                    crate::rust_source::script_source_attributes(&source, script_type).base_class
                })
                .into_iter()
                .collect()
        });
    let references = classes.iter().map(String::as_str).collect::<Vec<_>>();
    loader.packed_strings.write(result, &references);
}

unsafe extern "C" fn exists(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        write_bool(result, false);
        return;
    };
    if arguments.is_null() {
        write_bool(result, false);
        return;
    }
    // SAFETY: This virtual's first argument is String.
    let path = unsafe { *arguments };
    let exists = crate::file_access::exists(loader.interface, path).unwrap_or(false);
    write_bool(result, exists);
}

unsafe extern "C" fn load(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(loader) = loader(instance) else {
        return;
    };
    if arguments.is_null() {
        write_nil_variant(loader.interface, result);
        return;
    }
    // SAFETY: This virtual's first argument is String.
    let path = unsafe { *arguments };
    let Ok(resource_path) = read_utf8_string(loader.interface, path) else {
        write_nil_variant(loader.interface, result);
        return;
    };
    if !crate::file_access::exists(loader.interface, path).unwrap_or(false) {
        write_nil_variant(loader.interface, result);
        return;
    }
    let resource_uid = loader
        .globalize_path(path)
        .and_then(|global_path| {
            read_uid_file(std::path::Path::new(&global_path))
                .ok()
                .flatten()
        })
        .unwrap_or(INVALID_RESOURCE_UID);
    let Ok(source) = crate::file_access::read_text(loader.interface, path) else {
        write_nil_variant(loader.interface, result);
        return;
    };
    loader.load_script(&resource_path, resource_uid, &source, result);
}

fn write_bool(result: GDExtensionTypePtr, value: bool) {
    if !result.is_null() {
        // SAFETY: Godot ptrcall bool is one byte.
        unsafe { result.cast::<u8>().write(u8::from(value)) };
    }
}

fn write_i64(result: GDExtensionTypePtr, value: i64) {
    if !result.is_null() {
        // SAFETY: Godot ptrcall integers and enums are i64.
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

static LOADER_VIRTUAL_METHODS: &[VirtualMethodSpec] = &[
    virtual_method!(
        c"_get_recognized_extensions",
        1_139_954_409,
        get_recognized_extensions
    ),
    virtual_method!(c"_recognize_path", 2_594_487_047, recognize_path),
    virtual_method!(c"_handles_type", 2_619_796_661, handles_type),
    virtual_method!(c"_get_resource_type", 3_135_753_539, get_resource_type),
    virtual_method!(
        c"_get_resource_script_class",
        3_135_753_539,
        get_resource_script_class
    ),
    virtual_method!(c"_get_resource_uid", 1_321_353_865, get_resource_uid),
    virtual_method!(c"_get_dependencies", 6_257_701, get_dependencies),
    virtual_method!(c"_rename_dependencies", 223_715_120, rename_dependencies),
    virtual_method!(c"_exists", 3_927_539_163, exists),
    virtual_method!(c"_get_classes_used", 4_291_131_558, get_classes_used),
    virtual_method!(c"_load", 2_885_906_527, load),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn loader_virtuals_match_the_official_godot_4_4_surface() {
        let expected = [
            (b"_get_recognized_extensions".as_slice(), 1_139_954_409),
            (b"_recognize_path".as_slice(), 2_594_487_047),
            (b"_handles_type".as_slice(), 2_619_796_661),
            (b"_get_resource_type".as_slice(), 3_135_753_539),
            (b"_get_resource_script_class".as_slice(), 3_135_753_539),
            (b"_get_resource_uid".as_slice(), 1_321_353_865),
            (b"_get_dependencies".as_slice(), 6_257_701),
            (b"_rename_dependencies".as_slice(), 223_715_120),
            (b"_exists".as_slice(), 3_927_539_163),
            (b"_get_classes_used".as_slice(), 4_291_131_558),
            (b"_load".as_slice(), 2_885_906_527),
        ];
        let names: HashSet<_> = LOADER_VIRTUAL_METHODS
            .iter()
            .map(|method| method.name.to_bytes())
            .collect();
        assert_eq!(names.len(), LOADER_VIRTUAL_METHODS.len());
        assert_eq!(LOADER_VIRTUAL_METHODS.len(), expected.len());
        for (name, hash) in expected {
            let method = LOADER_VIRTUAL_METHODS
                .iter()
                .find(|method| method.name.to_bytes() == name)
                .unwrap_or_else(|| panic!("missing official loader virtual {name:?}"));
            assert_eq!(method.hash, hash, "wrong hash for {name:?}");
        }
        assert!(
            LOADER_VIRTUAL_METHODS
                .iter()
                .all(|method| method.callback.is_some())
        );
    }
}
