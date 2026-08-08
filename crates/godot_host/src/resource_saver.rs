use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};
use godot_api::{
    GDExtensionBool, GDExtensionClassInstancePtr, GDExtensionConstTypePtr,
    GDExtensionMethodBindPtr, GDExtensionTypePtr,
};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::interface::EngineInterface;
use crate::packed_string_array::PackedStringArrayWriter;
use crate::registry::{ClassRegistry, ClassSpec, RegisteredClassId, VirtualMethodSpec};
use crate::resource_uid::{to_text, uid_file_path};
use crate::string_name::StaticStringName;
use crate::value::{LocalGodotString, read_utf8_string};

const OK: i64 = 0;
const ERR_FILE_CANT_WRITE: i64 = 13;
const ERR_FILE_UNRECOGNIZED: i64 = 15;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(crate) struct RustResourceSaver {
    interface: EngineInterface,
    project_settings: usize,
    globalize_path_method: usize,
    resource_saver: usize,
    resource_id_for_path_method: usize,
    packed_strings: PackedStringArrayWriter,
}

pub(crate) fn register_class(registry: &mut ClassRegistry) -> RegisteredClassId {
    registry.register(ClassSpec {
        name: c"GodotRustResourceSaver",
        parent: c"ResourceFormatSaver",
        factory: create_saver_instance,
        dropper: drop_saver_instance,
        virtual_methods: SAVER_VIRTUAL_METHODS,
    })
}

fn create_saver_instance(interface: EngineInterface, _object: *mut c_void) -> *mut c_void {
    let Some(get_method) = interface.classdb_get_method_bind else {
        return ptr::null_mut();
    };
    let Some(get_singleton) = interface.global_get_singleton else {
        return ptr::null_mut();
    };
    let project_settings_class = StaticStringName::new(interface, c"ProjectSettings");
    let globalize_path_name = StaticStringName::new(interface, c"globalize_path");
    let resource_saver_class = StaticStringName::new(interface, c"ResourceSaver");
    let resource_id_for_path_name = StaticStringName::new(interface, c"get_resource_id_for_path");
    // SAFETY: Name and hash match ProjectSettings.globalize_path(String).
    let globalize_path_method = unsafe {
        get_method(
            project_settings_class.as_ptr(),
            globalize_path_name.as_ptr(),
            3_135_753_539,
        )
    };
    // SAFETY: ProjectSettings is an official singleton.
    let project_settings = unsafe { get_singleton(project_settings_class.as_ptr()) };
    // SAFETY: Name and hash match
    // ResourceSaver.get_resource_id_for_path(String, bool).
    let resource_id_for_path_method = unsafe {
        get_method(
            resource_saver_class.as_ptr(),
            resource_id_for_path_name.as_ptr(),
            150_756_522,
        )
    };
    // SAFETY: ResourceSaver is an official singleton.
    let resource_saver = unsafe { get_singleton(resource_saver_class.as_ptr()) };
    let Some(packed_strings) = PackedStringArrayWriter::new(interface) else {
        return ptr::null_mut();
    };
    if globalize_path_method.is_null()
        || project_settings.is_null()
        || resource_id_for_path_method.is_null()
        || resource_saver.is_null()
    {
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(RustResourceSaver {
        interface,
        project_settings: project_settings as usize,
        globalize_path_method: globalize_path_method as usize,
        resource_saver: resource_saver as usize,
        resource_id_for_path_method: resource_id_for_path_method as usize,
        packed_strings,
    }))
    .cast()
}

unsafe fn drop_saver_instance(instance: *mut c_void) {
    // SAFETY: This allocation comes from `create_saver_instance`.
    unsafe { drop(Box::from_raw(instance.cast::<RustResourceSaver>())) };
}

fn saver(instance: GDExtensionClassInstancePtr) -> Option<&'static RustResourceSaver> {
    if instance.is_null() {
        return None;
    }
    // SAFETY: ClassDB supplies the matching live extension instance.
    Some(unsafe { &*instance.cast::<RustResourceSaver>() })
}

impl RustResourceSaver {
    fn globalize_path(&self, value: GDExtensionConstTypePtr) -> Option<String> {
        let mut result = LocalGodotString::new(self.interface, c"")?;
        let arguments = [value];
        let ptrcall = self.interface.object_method_bind_ptrcall?;
        // SAFETY: Bind and argument match
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

    fn resource_id_for_path(&self, path: GDExtensionConstTypePtr) -> Option<i64> {
        let generate: GDExtensionBool = 1;
        let arguments = [
            path,
            (&raw const generate).cast::<c_void>() as GDExtensionConstTypePtr,
        ];
        let mut result = crate::resource_uid::INVALID_RESOURCE_UID;
        let ptrcall = self.interface.object_method_bind_ptrcall?;
        // SAFETY: Bind, singleton, argument storage, and output match
        // ResourceSaver.get_resource_id_for_path(String, bool) -> int.
        unsafe {
            ptrcall(
                self.resource_id_for_path_method as GDExtensionMethodBindPtr,
                self.resource_saver as *mut c_void,
                arguments.as_ptr(),
                (&raw mut result).cast::<c_void>() as GDExtensionTypePtr,
            );
        }
        (result >= 0).then_some(result)
    }
}

fn resource_argument(arguments: *const GDExtensionConstTypePtr) -> Option<*mut c_void> {
    if arguments.is_null() {
        return None;
    }
    // SAFETY: ResourceFormatSaver encodes Ref<Resource> ptrcall arguments as
    // one pointer-to-object slot.
    let resource = unsafe { *arguments };
    if resource.is_null() {
        return None;
    }
    // SAFETY: The argument points to a live Object pointer for this callback.
    let object = unsafe { resource.cast::<*mut c_void>().read() };
    (!object.is_null()).then_some(object)
}

fn path_argument(
    interface: EngineInterface,
    arguments: *const GDExtensionConstTypePtr,
    index: usize,
) -> Option<(GDExtensionConstTypePtr, String)> {
    if arguments.is_null() {
        return None;
    }
    // SAFETY: The caller chooses an index from the official virtual signature.
    let raw = unsafe { *arguments.add(index) };
    read_utf8_string(interface, raw)
        .ok()
        .map(|value| (raw, value))
}

unsafe extern "C" fn recognize(
    _instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let recognized = resource_argument(arguments)
        .and_then(crate::script::source_for_object)
        .is_some();
    write_bool(result, recognized);
}

unsafe extern "C" fn recognize_path(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(saver) = saver(instance) else {
        write_bool(result, false);
        return;
    };
    let recognized = resource_argument(arguments)
        .and_then(crate::script::source_for_object)
        .is_some()
        && path_argument(saver.interface, arguments, 1)
            .is_some_and(|(_, path)| path.ends_with(".rs"));
    write_bool(result, recognized);
}

unsafe extern "C" fn get_recognized_extensions(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(saver) = saver(instance) else {
        return;
    };
    if result.is_null()
        || resource_argument(arguments)
            .and_then(crate::script::source_for_object)
            .is_none()
    {
        saver.packed_strings.write_empty(result);
        return;
    }
    saver.packed_strings.write(result, &["rs"]);
}

unsafe extern "C" fn save(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(saver) = saver(instance) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    let Some(resource) = resource_argument(arguments) else {
        write_i64(result, ERR_FILE_UNRECOGNIZED);
        return;
    };
    let Some(source) = crate::script::source_for_object(resource) else {
        write_i64(result, ERR_FILE_UNRECOGNIZED);
        return;
    };
    let Some((raw_path, resource_path)) = path_argument(saver.interface, arguments, 1) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    if !resource_path.ends_with(".rs") {
        write_i64(result, ERR_FILE_UNRECOGNIZED);
        return;
    }
    let Some(global_path) = saver.globalize_path(raw_path) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    let path = Path::new(&global_path);
    if atomic_write(path, source.as_bytes()).is_err() {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    }
    if !crate::script::set_saved_path(resource, &resource_path) {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    }
    if let Some(uid) = saver.resource_id_for_path(raw_path) {
        let Some(text) = to_text(uid) else {
            write_i64(result, ERR_FILE_CANT_WRITE);
            return;
        };
        if write_uid_file(path, &resource_path, uid, &text).is_err() {
            write_i64(result, ERR_FILE_CANT_WRITE);
            return;
        }
    }
    write_i64(result, OK);
}

unsafe extern "C" fn set_uid(
    instance: GDExtensionClassInstancePtr,
    arguments: *const GDExtensionConstTypePtr,
    result: GDExtensionTypePtr,
) {
    let Some(saver) = saver(instance) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    let Some((raw_path, resource_path)) = path_argument(saver.interface, arguments, 0) else {
        write_i64(result, ERR_FILE_UNRECOGNIZED);
        return;
    };
    if !resource_path.ends_with(".rs") || arguments.is_null() {
        write_i64(result, ERR_FILE_UNRECOGNIZED);
        return;
    }
    // SAFETY: `_set_uid` has an i64 second argument.
    let uid = unsafe { (*arguments.add(1)).cast::<i64>().read() };
    let Some(text) = to_text(uid) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    let Some(global_path) = saver.globalize_path(raw_path) else {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    };
    if write_uid_file(Path::new(&global_path), &resource_path, uid, &text).is_err() {
        write_i64(result, ERR_FILE_CANT_WRITE);
        return;
    }
    write_i64(result, OK);
}

fn write_uid_file(
    global_path: &Path,
    resource_path: &str,
    uid: i64,
    text: &str,
) -> std::io::Result<()> {
    let contents = format!("{text}\n");
    atomic_write(&uid_file_path(global_path), contents.as_bytes())?;
    crate::script::set_resource_uid_for_path(resource_path, uid);
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    let temporary = temporary_path(path);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let outcome = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    let _ = sync_directory(parent);
    outcome
}

fn temporary_path(path: &Path) -> PathBuf {
    let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let name = format!(
        ".{}.godot-rust-{}-{id}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("script"),
        std::process::id()
    );
    path.with_file_name(name)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(core::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(core::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both paths are nul-terminated UTF-16 buffers retained for the
    // synchronous Win32 call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn write_bool(result: GDExtensionTypePtr, value: bool) {
    if !result.is_null() {
        // SAFETY: Godot ptrcall bool is one byte.
        unsafe { result.cast::<u8>().write(u8::from(value)) };
    }
}

fn write_i64(result: GDExtensionTypePtr, value: i64) {
    if !result.is_null() {
        // SAFETY: Godot ptrcall integers and Error values are i64.
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

static SAVER_VIRTUAL_METHODS: &[VirtualMethodSpec] = &[
    virtual_method!(c"_save", 2_794_699_034, save),
    virtual_method!(c"_set_uid", 993_915_709, set_uid),
    virtual_method!(c"_recognize", 3_190_994_482, recognize),
    virtual_method!(
        c"_get_recognized_extensions",
        1_567_505_034,
        get_recognized_extensions
    ),
    virtual_method!(c"_recognize_path", 710_996_192, recognize_path),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn atomic_script_writes_replace_existing_content() {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "godot-rust-resource-saver-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("test directory");
        let path = directory.join("player.rs");
        atomic_write(&path, b"first").expect("first save");
        atomic_write(&path, b"second").expect("replacement save");
        assert_eq!(std::fs::read(&path).expect("saved source"), b"second");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn saver_virtuals_match_the_official_godot_4_4_hashes() {
        assert_eq!(
            SAVER_VIRTUAL_METHODS
                .iter()
                .map(|method| (method.name.to_bytes(), method.hash))
                .collect::<Vec<_>>(),
            [
                (c"_save".to_bytes(), 2_794_699_034),
                (c"_set_uid".to_bytes(), 993_915_709),
                (c"_recognize".to_bytes(), 3_190_994_482),
                (c"_get_recognized_extensions".to_bytes(), 1_567_505_034),
                (c"_recognize_path".to_bytes(), 710_996_192),
            ]
        );
    }
}
