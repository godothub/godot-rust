#[cfg(not(any(target_os = "android", target_os = "emscripten", target_os = "ios")))]
use godot_rs_api::{GDExtensionBool, GDExtensionTypePtr};
use godot_rs_api::{GDExtensionConstTypePtr, GDExtensionMethodBindPtr};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use crate::interface::EngineInterface;
use crate::runtime::resolve_method;
use crate::string_name::StaticStringName;
use crate::value::LocalGodotString;

const LAST_KNOWN_GOOD_FILE: &str = ".godot/rust/last-known-good.json";
const SAFE_MODE_FILE: &str = ".godot/rust/safe-mode";
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_MODULE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
#[cfg(target_os = "windows")]
const EXPORTED_MODULE_FILE: &str = "godot_rs_project_module.dll";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const EXPORTED_MODULE_FILE: &str = "libgodot_rs_project_module.dylib";
#[cfg(all(
    unix,
    not(any(target_os = "emscripten", target_os = "ios", target_os = "macos"))
))]
const EXPORTED_MODULE_FILE: &str = "libgodot_rs_project_module.so";

#[derive(Debug, Deserialize)]
struct LastKnownGood {
    format: u32,
    build_id: String,
    module_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredModule {
    pub(crate) build_id: String,
    pub(crate) path: PathBuf,
}

pub(crate) struct Watcher {
    project_root: PathBuf,
    observed_build_id: Option<String>,
}

impl Watcher {
    pub(crate) fn new(interface: EngineInterface) -> Result<Self, String> {
        let project_root = globalize_project_root(interface)?
            .canonicalize()
            .map_err(|error| format!("could not resolve Godot project root: {error}"))?;
        if !project_root.is_dir() {
            return Err(format!(
                "Godot project root is not a filesystem directory: {}",
                project_root.display()
            ));
        }
        let observed_build_id = read_last_known_good(&project_root)?.map(|record| record.build_id);
        Ok(Self {
            project_root,
            observed_build_id,
        })
    }

    /// Returns each content-addressed generation at most once.
    ///
    /// The Build ID is marked observed before hashing or loading so a corrupt
    /// record cannot trigger expensive retries every editor frame. Publishing
    /// a new build produces a new hash and therefore a new attempt.
    pub(crate) fn poll(&mut self) -> Result<Option<DiscoveredModule>, String> {
        if safe_mode_at(&self.project_root)? {
            return Ok(None);
        }
        let Some(record) = read_last_known_good(&self.project_root)? else {
            self.observed_build_id = None;
            return Ok(None);
        };
        if self.observed_build_id.as_deref() == Some(&record.build_id) {
            return Ok(None);
        }
        self.observed_build_id = Some(record.build_id.clone());
        resolve_last_known_good(&self.project_root, record).map(Some)
    }
}

#[cfg(any(target_os = "android", target_os = "emscripten", target_os = "ios"))]
pub(crate) fn safe_mode_enabled(_interface: EngineInterface) -> Result<bool, String> {
    // Exported mobile and Web applications do not expose a writable desktop
    // project directory. Their code is already fixed by the signed/package
    // export, so the editor-only Safe Mode marker does not apply.
    Ok(false)
}

#[cfg(not(any(target_os = "android", target_os = "emscripten", target_os = "ios")))]
pub(crate) fn safe_mode_enabled(interface: EngineInterface) -> Result<bool, String> {
    if !engine_is_editor_hint(interface)? {
        return Ok(false);
    }
    let root = globalize_project_root(interface)?;
    let project_root = root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}` while checking Safe Mode: {error}",
            root.display()
        )
    })?;
    safe_mode_at(&project_root)
}

#[cfg(not(any(target_os = "android", target_os = "emscripten", target_os = "ios")))]
fn engine_is_editor_hint(interface: EngineInterface) -> Result<bool, String> {
    let get_singleton = interface
        .global_get_singleton
        .ok_or_else(|| "Godot did not expose global singleton lookup".to_owned())?;
    let engine_name = StaticStringName::new(interface, c"Engine");
    // SAFETY: Engine is an official Godot singleton.
    let engine = unsafe { get_singleton(engine_name.as_ptr()) };
    if engine.is_null() {
        return Err("Godot did not expose its Engine singleton".to_owned());
    }
    let method = resolve_method(interface, c"Engine", c"is_editor_hint", 36_873_697)
        .map_err(|error| error.to_string())?;
    let ptrcall = interface
        .object_method_bind_ptrcall
        .ok_or_else(|| "Godot did not expose method bind ptrcall".to_owned())?;
    let mut output: GDExtensionBool = 0;
    // SAFETY: Method bind and receiver match Engine.is_editor_hint() -> bool,
    // and output points to one writable GDExtensionBool.
    unsafe {
        ptrcall(
            method as GDExtensionMethodBindPtr,
            engine,
            core::ptr::null(),
            (&raw mut output).cast::<core::ffi::c_void>() as GDExtensionTypePtr,
        );
    }
    if output > 1 {
        return Err(format!(
            "Godot returned an invalid editor-hint boolean value: {output}"
        ));
    }
    Ok(output != 0)
}

fn safe_mode_at(project_root: &Path) -> Result<bool, String> {
    let path = project_root.join(SAFE_MODE_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(format!(
            "Safe Mode marker must be a regular file: {}",
            path.display()
        )),
        Ok(metadata) if metadata.len() > 1024 => Err(format!(
            "Safe Mode marker exceeds the 1024-byte safety limit: {}",
            path.display()
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "could not inspect Safe Mode marker `{}`: {error}",
            path.display()
        )),
    }
}

#[cfg(target_os = "android")]
pub(crate) fn discover(_interface: EngineInterface) -> Result<Option<PathBuf>, String> {
    // Android's native loader exposes packaged .so files by soname rather
    // than as files beside the process executable.
    Ok(Some(PathBuf::from(EXPORTED_MODULE_FILE)))
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
pub(crate) fn discover(interface: EngineInterface) -> Result<Option<PathBuf>, String> {
    let root = globalize_project_root(interface)?;
    let project_root_issue = match root.canonicalize() {
        Ok(project_root) if project_root.is_dir() => {
            if let Some(module) = find_module(&project_root)? {
                return Ok(Some(module));
            }
            None
        }
        Ok(project_root) => Some(format!(
            "Godot project root is not a filesystem directory: {}",
            project_root.display()
        )),
        Err(error) => Some(format!(
            "could not resolve Godot project root `{}`: {error}",
            root.display()
        )),
    };
    if let Some(module) = find_exported_module_from_executable()? {
        return Ok(Some(module));
    }
    if let Some(issue) = project_root_issue {
        return Err(issue);
    }
    Ok(None)
}

pub(crate) fn globalize_project_root(interface: EngineInterface) -> Result<PathBuf, String> {
    let get_singleton = interface
        .global_get_singleton
        .ok_or_else(|| "Godot did not expose global singleton lookup".to_owned())?;
    let project_settings_name = StaticStringName::new(interface, c"ProjectSettings");
    // SAFETY: ProjectSettings is an official Godot singleton.
    let project_settings = unsafe { get_singleton(project_settings_name.as_ptr()) };
    if project_settings.is_null() {
        return Err("Godot did not expose its ProjectSettings singleton".to_owned());
    }
    let globalize_path = resolve_method(
        interface,
        c"ProjectSettings",
        c"globalize_path",
        3_135_753_539,
    )
    .map_err(|error| error.to_string())?;
    let input = LocalGodotString::new(interface, c"res://")
        .ok_or_else(|| "could not create Godot project path String".to_owned())?;
    let mut output = LocalGodotString::new(interface, c"")
        .ok_or_else(|| "could not create Godot path output String".to_owned())?;
    let arguments: [GDExtensionConstTypePtr; 1] = [input.as_ptr()];
    let ptrcall = interface
        .object_method_bind_ptrcall
        .ok_or_else(|| "Godot did not expose method bind ptrcall".to_owned())?;
    // SAFETY: Method bind, receiver, input, and output match
    // ProjectSettings.globalize_path(String) -> String.
    unsafe {
        ptrcall(
            globalize_path as GDExtensionMethodBindPtr,
            project_settings,
            arguments.as_ptr(),
            output.as_mut_ptr(),
        );
    }
    let root = output
        .to_utf8()
        .map_err(|error| format!("could not decode Godot project root: {error}"))?;
    Ok(PathBuf::from(root))
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
fn find_module(project_root: &Path) -> Result<Option<PathBuf>, String> {
    let project_root = project_root
        .canonicalize()
        .map_err(|error| format!("could not resolve Godot project root: {error}"))?;
    if let Some(module) = find_last_known_good(&project_root)? {
        return Ok(Some(module));
    }
    find_exported_module(&project_root)
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
fn find_last_known_good(project_root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(record) = read_last_known_good(project_root)? else {
        return Ok(None);
    };
    resolve_last_known_good(project_root, record).map(|module| Some(module.path))
}

fn read_last_known_good(project_root: &Path) -> Result<Option<LastKnownGood>, String> {
    let record_path = project_root.join(LAST_KNOWN_GOOD_FILE);
    let metadata = match std::fs::symlink_metadata(&record_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not inspect Last Known Good `{}`: {error}",
                record_path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Last Known Good is not a regular file: {}",
            record_path.display()
        ));
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(format!(
            "Last Known Good exceeds the {} byte safety limit",
            MAX_RECORD_BYTES
        ));
    }
    let source = std::fs::read(&record_path).map_err(|error| {
        format!(
            "could not read Last Known Good `{}`: {error}",
            record_path.display()
        )
    })?;
    let record: LastKnownGood = serde_json::from_slice(&source).map_err(|error| {
        format!(
            "Last Known Good is invalid JSON `{}`: {error}",
            record_path.display()
        )
    })?;
    if record.format != 1 {
        return Err(format!(
            "Last Known Good format {} is unsupported",
            record.format
        ));
    }
    parse_build_id(&record.build_id)?;
    validate_relative_module_path(&record.module_path)?;
    Ok(Some(record))
}

fn resolve_last_known_good(
    project_root: &Path,
    record: LastKnownGood,
) -> Result<DiscoveredModule, String> {
    let expected_hash = parse_build_id(&record.build_id)?;
    let module_path = project_root.join(&record.module_path);
    let module_metadata = std::fs::symlink_metadata(&module_path).map_err(|error| {
        format!(
            "could not inspect Last Known Good module `{}`: {error}",
            module_path.display()
        )
    })?;
    if module_metadata.file_type().is_symlink() || !module_metadata.is_file() {
        return Err(format!(
            "Last Known Good module is not a regular file: {}",
            module_path.display()
        ));
    }
    if module_metadata.len() > MAX_MODULE_BYTES {
        return Err(format!(
            "Last Known Good module exceeds the {} byte safety limit",
            MAX_MODULE_BYTES
        ));
    }
    let canonical_module = module_path.canonicalize().map_err(|error| {
        format!(
            "could not resolve Last Known Good module `{}`: {error}",
            module_path.display()
        )
    })?;
    if !canonical_module.starts_with(project_root) {
        return Err("Last Known Good module escapes the Godot project root".to_owned());
    }
    let actual_hash = hash_file(&canonical_module)?;
    if actual_hash != expected_hash {
        return Err(format!(
            "Last Known Good module hash mismatch: expected {expected_hash}, found {actual_hash}"
        ));
    }
    Ok(DiscoveredModule {
        build_id: record.build_id,
        path: canonical_module,
    })
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
fn find_exported_module(project_root: &Path) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "macos")]
    let directory = {
        let contents = project_root.parent();
        if project_root.file_name().and_then(|name| name.to_str()) != Some("Resources")
            || contents
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some("Contents")
        {
            return Ok(None);
        }
        contents
            .expect("validated macOS Contents directory")
            .join("Frameworks")
    };
    #[cfg(not(target_os = "macos"))]
    let directory = project_root.to_owned();

    find_exported_module_in(&directory, EXPORTED_MODULE_FILE)
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
fn find_exported_module_from_executable() -> Result<Option<PathBuf>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve the exported game executable: {error}"))?;
    find_exported_module_for_executable(&executable)
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
fn find_exported_module_for_executable(executable: &Path) -> Result<Option<PathBuf>, String> {
    let executable = executable.canonicalize().map_err(|error| {
        format!(
            "could not resolve exported game executable `{}`: {error}",
            executable.display()
        )
    })?;
    let executable_directory = executable.parent().ok_or_else(|| {
        format!(
            "exported game executable has no parent directory: {}",
            executable.display()
        )
    })?;
    #[cfg(target_os = "macos")]
    let module_directory = {
        if executable_directory
            .file_name()
            .and_then(|name| name.to_str())
            != Some("MacOS")
        {
            return Ok(None);
        }
        let contents = executable_directory.parent().ok_or_else(|| {
            format!(
                "macOS exported game has no Contents directory: {}",
                executable.display()
            )
        })?;
        if contents.file_name().and_then(|name| name.to_str()) != Some("Contents") {
            return Ok(None);
        }
        contents.join("Frameworks")
    };
    #[cfg(target_os = "ios")]
    let module_directory = executable_directory.join("Frameworks");
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    let module_directory = executable_directory.to_owned();

    #[cfg(target_os = "ios")]
    let result = find_ios_exported_module(&module_directory);
    #[cfg(not(target_os = "ios"))]
    let result = find_exported_module_in(&module_directory, EXPORTED_MODULE_FILE);
    result
}

#[cfg(target_os = "ios")]
fn find_ios_exported_module(directory: &Path) -> Result<Option<PathBuf>, String> {
    if let Some(module) = find_exported_module_in(directory, EXPORTED_MODULE_FILE)? {
        return Ok(Some(module));
    }
    let framework = directory.join("libgodot_rs_project_module.framework");
    find_exported_module_in(&framework, "libgodot_rs_project_module")
}

#[cfg(not(any(target_os = "android", target_os = "emscripten")))]
fn find_exported_module_in(directory: &Path, module_file: &str) -> Result<Option<PathBuf>, String> {
    let module_path = directory.join(module_file);
    let metadata = match std::fs::symlink_metadata(&module_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not inspect exported Rust module `{}`: {error}",
                module_path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "exported Rust module is not a regular file: {}",
            module_path.display()
        ));
    }
    if metadata.len() > MAX_MODULE_BYTES {
        return Err(format!(
            "exported Rust module exceeds the {} byte safety limit",
            MAX_MODULE_BYTES
        ));
    }
    let directory = directory.canonicalize().map_err(|error| {
        format!(
            "could not resolve exported Rust module directory `{}`: {error}",
            directory.display()
        )
    })?;
    let canonical_module = module_path.canonicalize().map_err(|error| {
        format!(
            "could not resolve exported Rust module `{}`: {error}",
            module_path.display()
        )
    })?;
    if canonical_module.parent() != Some(directory.as_path()) {
        return Err("exported Rust module escapes its application directory".to_owned());
    }
    Ok(Some(canonical_module))
}

fn parse_build_id(build_id: &str) -> Result<&str, String> {
    let hash = build_id
        .strip_prefix("sha256:")
        .ok_or_else(|| "Last Known Good Build ID must start with `sha256:`".to_owned())?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Last Known Good Build ID is not canonical lowercase SHA-256".to_owned());
    }
    Ok(hash)
}

fn validate_relative_module_path(path: &Path) -> Result<(), String> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(
            "Last Known Good module path must contain only relative normal components".to_owned(),
        );
    }
    let expected_prefix = Path::new(".godot/rust/builds");
    if !path.starts_with(expected_prefix) {
        return Err("Last Known Good module path is outside `.godot/rust/builds`".to_owned());
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|error| format!("could not open Last Known Good module: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("could not hash Last Known Good module: {error}"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| "Last Known Good module size overflowed u64".to_owned())?;
        if total > MAX_MODULE_BYTES {
            return Err(format!(
                "Last Known Good module exceeds the {} byte safety limit",
                MAX_MODULE_BYTES
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempProject(PathBuf);

    impl TempProject {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "godot-rust-host-last-known-good-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join(".godot/rust/builds/build"))
                .expect("temporary build directory");
            Self(root)
        }

        fn write_generation(&self, content: &[u8]) -> String {
            let module = self.0.join(".godot/rust/builds/build/project_module.so");
            std::fs::write(&module, content).expect("module fixture");
            hash_file(&module).expect("module hash")
        }

        fn write_record(&self, hash: &str, module_path: &str) {
            let record = serde_json::json!({
                "format": 1,
                "build_id": format!("sha256:{hash}"),
                "module_path": module_path,
            });
            std::fs::write(
                self.0.join(LAST_KNOWN_GOOD_FILE),
                serde_json::to_vec(&record).expect("record JSON"),
            )
            .expect("record fixture");
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn valid_relative_generation_is_hash_verified() {
        let project = TempProject::new();
        let hash = project.write_generation(b"trusted module");
        project.write_record(&hash, ".godot/rust/builds/build/project_module.so");
        assert_eq!(
            find_module(&project.0).expect("record"),
            Some(
                project
                    .0
                    .join(".godot/rust/builds/build/project_module.so")
                    .canonicalize()
                    .expect("canonical module"),
            )
        );
    }

    #[test]
    fn safe_mode_requires_a_small_regular_marker() {
        let project = TempProject::new();
        assert!(!safe_mode_at(&project.0).expect("missing marker"));
        let marker = project.0.join(SAFE_MODE_FILE);
        std::fs::write(&marker, b"godot-rust safe mode\n").expect("Safe Mode marker");
        assert!(safe_mode_at(&project.0).expect("regular marker"));
        std::fs::write(&marker, vec![0_u8; 1025]).expect("oversized marker");
        assert!(
            safe_mode_at(&project.0)
                .expect_err("oversized marker")
                .contains("1024-byte")
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_mode_marker_cannot_be_a_symbolic_link() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let target = project.0.join(".godot/rust/safe-mode-target");
        std::fs::write(&target, b"enabled").expect("marker target");
        symlink(&target, project.0.join(SAFE_MODE_FILE)).expect("marker symlink");
        assert!(
            safe_mode_at(&project.0)
                .expect_err("symlink marker")
                .contains("regular file")
        );
    }

    #[test]
    fn traversal_and_modified_modules_are_rejected() {
        let project = TempProject::new();
        let hash = project.write_generation(b"first module");
        project.write_record(&hash, "../outside.so");
        assert!(
            find_module(&project.0)
                .expect_err("path traversal")
                .contains("relative normal components")
        );

        project.write_record(&hash, ".godot/rust/builds/build/project_module.so");
        std::fs::write(
            project.0.join(".godot/rust/builds/build/project_module.so"),
            b"modified module",
        )
        .expect("tamper fixture");
        assert!(
            find_module(&project.0)
                .expect_err("hash mismatch")
                .contains("hash mismatch")
        );
    }

    #[test]
    fn watcher_attempts_each_build_id_once_and_accepts_the_next_generation() {
        let project = TempProject::new();
        let mut watcher = Watcher {
            project_root: project.0.canonicalize().expect("project root"),
            observed_build_id: None,
        };
        assert!(watcher.poll().expect("empty project").is_none());

        let first = project.write_generation(b"first generation");
        project.write_record(&first, ".godot/rust/builds/build/project_module.so");
        let discovered = watcher
            .poll()
            .expect("first generation")
            .expect("new generation");
        assert_eq!(discovered.build_id, format!("sha256:{first}"));
        assert!(watcher.poll().expect("unchanged generation").is_none());

        let invalid = "0".repeat(64);
        project.write_record(&invalid, ".godot/rust/builds/build/project_module.so");
        assert!(
            watcher
                .poll()
                .expect_err("hash mismatch")
                .contains("hash mismatch")
        );
        assert!(
            watcher
                .poll()
                .expect("failed generation is not retried")
                .is_none()
        );

        let second = project.write_generation(b"second generation");
        project.write_record(&second, ".godot/rust/builds/build/project_module.so");
        assert_eq!(
            watcher
                .poll()
                .expect("second generation")
                .expect("new build id")
                .build_id,
            format!("sha256:{second}")
        );
    }

    #[test]
    #[ignore = "long-running release stability gate"]
    fn ten_thousand_generation_records_remain_fail_closed() {
        let project = TempProject::new();
        let mut watcher = Watcher {
            project_root: project.0.canonicalize().expect("project root"),
            observed_build_id: None,
        };
        for cycle in 0_u32..10_000 {
            let contents = cycle.to_le_bytes();
            let hash = project.write_generation(&contents);
            if cycle % 97 == 0 {
                let invalid = "0".repeat(64);
                project.write_record(&invalid, ".godot/rust/builds/build/project_module.so");
                assert!(
                    watcher
                        .poll()
                        .expect_err("tampered generation must fail")
                        .contains("hash mismatch")
                );
                assert!(
                    watcher
                        .poll()
                        .expect("failed generation is attempted once")
                        .is_none()
                );
            }
            project.write_record(&hash, ".godot/rust/builds/build/project_module.so");
            let generation = watcher
                .poll()
                .expect("valid generation record")
                .expect("new generation");
            assert_eq!(generation.build_id, format!("sha256:{hash}"));
            assert_eq!(
                std::fs::read(generation.path).expect("generation contents"),
                contents
            );
            assert!(
                watcher
                    .poll()
                    .expect("unchanged generation is not retried")
                    .is_none()
            );
        }
    }

    #[test]
    fn exact_exported_module_is_discovered_without_a_development_record() {
        let project = TempProject::new();
        let export_directory = project.0.join("export");
        std::fs::create_dir(&export_directory).expect("export directory");
        let module = export_directory.join(EXPORTED_MODULE_FILE);
        std::fs::write(&module, b"exported Rust module").expect("exported module");
        assert_eq!(
            find_exported_module_in(&export_directory, EXPORTED_MODULE_FILE)
                .expect("export discovery"),
            Some(module.canonicalize().expect("canonical exported module"))
        );
        assert_eq!(
            find_exported_module_in(&export_directory, "other-module.so").expect("missing export"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn exported_module_symbolic_links_are_rejected() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let export_directory = project.0.join("export");
        std::fs::create_dir(&export_directory).expect("export directory");
        let outside = project.0.join("outside-module");
        std::fs::write(&outside, b"outside module").expect("outside module");
        symlink(&outside, export_directory.join(EXPORTED_MODULE_FILE))
            .expect("exported module symbolic link");
        assert!(
            find_exported_module_in(&export_directory, EXPORTED_MODULE_FILE)
                .expect_err("symbolic link")
                .contains("not a regular file")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_exports_resolve_the_frameworks_sidecar() {
        let project = TempProject::new();
        let resources = project.0.join("Game.app/Contents/Resources");
        let frameworks = project.0.join("Game.app/Contents/Frameworks");
        std::fs::create_dir_all(&resources).expect("Resources directory");
        std::fs::create_dir_all(&frameworks).expect("Frameworks directory");
        let module = frameworks.join(EXPORTED_MODULE_FILE);
        std::fs::write(&module, b"macOS exported module").expect("exported module");
        assert_eq!(
            find_exported_module(&resources.canonicalize().expect("Resources path"))
                .expect("macOS export discovery"),
            Some(module.canonicalize().expect("canonical exported module"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_exported_executable_resolves_the_frameworks_sidecar() {
        let project = TempProject::new();
        let executable = project.0.join("Game.app/Contents/MacOS/Game");
        let frameworks = project.0.join("Game.app/Contents/Frameworks");
        std::fs::create_dir_all(executable.parent().expect("MacOS directory"))
            .expect("MacOS directory");
        std::fs::create_dir_all(&frameworks).expect("Frameworks directory");
        std::fs::write(&executable, b"game executable").expect("game executable");
        let module = frameworks.join(EXPORTED_MODULE_FILE);
        std::fs::write(&module, b"project module").expect("project module");
        assert_eq!(
            find_exported_module_for_executable(&executable).expect("executable discovery"),
            Some(module.canonicalize().expect("canonical project module"))
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn desktop_executable_resolves_its_module_sidecar() {
        let project = TempProject::new();
        let export_directory = project.0.join("export");
        std::fs::create_dir(&export_directory).expect("export directory");
        let executable = export_directory.join("game");
        std::fs::write(&executable, b"game executable").expect("game executable");
        let module = export_directory.join(EXPORTED_MODULE_FILE);
        std::fs::write(&module, b"project module").expect("project module");
        assert_eq!(
            find_exported_module_for_executable(&executable).expect("executable discovery"),
            Some(module.canonicalize().expect("canonical project module"))
        );
    }
}
