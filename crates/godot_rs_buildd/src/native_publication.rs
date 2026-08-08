use crate::managed_fs::{
    atomic_write, create_temporary_directory, ensure_directory, sync_directory,
};
use crate::xcframework::{copy_ios_xcframework, inspect_ios_xcframework};
use crate::{NativeBuildPlan, NativeExtensionDescriptor, NativePlatform};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

pub const NATIVE_DESCRIPTOR_FILE: &str = "godot_rs_project.gdextension";
pub const NATIVE_ARTIFACT_DIRECTORY: &str = ".godot/rust/native";
pub const NATIVE_STATE_FILE: &str = ".godot/rust/native/state.json";

const STATE_FORMAT: u32 = 2;
const GENERATION_FORMAT: u32 = 1;
const MAX_STATE_BYTES: u64 = 1024 * 1024;
const MAX_GENERATION_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_NATIVE_LIBRARY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublishedNativeExtension {
    pub descriptor_path: PathBuf,
    pub state_path: PathBuf,
    pub library_path: PathBuf,
    pub library_resource_path: String,
    pub platform_selector: String,
    pub sha256: String,
    pub byte_len: u64,
    pub reused: bool,
    pub rollback_available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NativePublicationState {
    format: u32,
    package_name: String,
    godot_api: String,
    entry_symbol: String,
    libraries: BTreeMap<String, PublishedLibraryRecord>,
    #[serde(default)]
    descriptor_sha256: Option<String>,
    #[serde(default)]
    previous_descriptor_sha256: Option<String>,
    #[serde(default)]
    pending: Option<PendingNativePublication>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PendingNativePublication {
    platform_selector: String,
    candidate_sha256: String,
    previous_libraries: BTreeMap<String, PublishedLibraryRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PublishedLibraryRecord {
    resource_path: String,
    sha256: String,
    byte_len: u64,
    module_file: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NativeGenerationManifest {
    format: u32,
    sha256: String,
    byte_len: u64,
    module_file: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NativePublicationResolution {
    pub platform_selector: String,
    pub sha256: String,
    pub rolled_back: bool,
}

pub fn publish_native_extension(
    project_root: impl AsRef<Path>,
    plan: &NativeBuildPlan,
    artifact: impl AsRef<Path>,
    platform: NativePlatform,
) -> Result<PublishedNativeExtension, String> {
    let project_root = canonical_project_root(project_root.as_ref())?;
    if project_root != plan.project_root {
        return Err(format!(
            "Native build plan belongs to `{}`, not `{}`",
            plan.project_root.display(),
            project_root.display()
        ));
    }
    let artifact = validate_native_artifact(artifact.as_ref())?;
    let module_file = artifact
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| is_safe_file_name(name))
        .ok_or_else(|| {
            format!(
                "Native artifact has an unsafe or non-UTF-8 file name: {}",
                artifact.display()
            )
        })?
        .to_owned();

    let generated_root = project_root.join(NATIVE_ARTIFACT_DIRECTORY);
    let builds_root = generated_root.join("bin");
    for (directory, purpose) in [
        (project_root.join(".godot"), "Godot managed directory"),
        (project_root.join(".godot/rust"), "Rust managed directory"),
        (generated_root.clone(), "Native generated directory"),
    ] {
        ensure_directory(&directory, purpose)?;
    }
    ensure_directory(&builds_root, "Native library directory")?;
    let descriptor_path = project_root.join(NATIVE_DESCRIPTOR_FILE);
    let state_path = project_root.join(NATIVE_STATE_FILE);
    let mut state = read_or_create_state(&state_path, &descriptor_path, &builds_root, plan)?;

    let temporary = create_temporary_directory(&builds_root, ".tmp")?;
    let staged_library = temporary.join(&module_file);
    let publish_result = (|| {
        copy_library(&artifact, &staged_library)?;
        let (sha256, byte_len) = hash_library(&staged_library)?;
        let generation_manifest = NativeGenerationManifest {
            format: GENERATION_FORMAT,
            sha256: sha256.clone(),
            byte_len,
            module_file: module_file.clone(),
        };
        write_new_json(&temporary.join("generation.json"), &generation_manifest)?;

        let generation_directory = builds_root.join(&sha256);
        let reused = if generation_directory.exists() {
            verify_generation(&generation_directory, &generation_manifest)?;
            true
        } else {
            match std::fs::rename(&temporary, &generation_directory) {
                Ok(()) => false,
                Err(_) if generation_directory.exists() => {
                    verify_generation(&generation_directory, &generation_manifest)?;
                    true
                }
                Err(error) => {
                    return Err(format!(
                        "could not publish immutable Native generation `{}`: {error}",
                        generation_directory.display()
                    ));
                }
            }
        };

        let library_path = generation_directory.join(&module_file);
        let library_resource_path =
            format!("res://{NATIVE_ARTIFACT_DIRECTORY}/bin/{sha256}/{module_file}");
        let platform_selector = platform.selector();
        let previous_libraries = state.pending.as_ref().map_or_else(
            || state.libraries.clone(),
            |pending| pending.previous_libraries.clone(),
        );
        state.libraries.insert(
            platform_selector.clone(),
            PublishedLibraryRecord {
                resource_path: library_resource_path.clone(),
                sha256: sha256.clone(),
                byte_len,
                module_file,
            },
        );
        let rollback_available = !previous_libraries.is_empty();
        state.pending = Some(PendingNativePublication {
            platform_selector: platform_selector.clone(),
            candidate_sha256: sha256.clone(),
            previous_libraries,
        });
        write_state_and_descriptor(&state_path, &descriptor_path, &state, plan)?;
        sync_directory(&generated_root)?;

        Ok(PublishedNativeExtension {
            descriptor_path,
            state_path,
            library_path,
            library_resource_path,
            platform_selector,
            sha256,
            byte_len,
            reused,
            rollback_available,
        })
    })();
    if temporary.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    publish_result
}

fn canonical_project_root(root: &Path) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Native project root `{}`: {error}",
            root.display()
        )
    })?;
    if !root.join("project.godot").is_file() {
        return Err(format!(
            "Native project root does not contain project.godot: {}",
            root.display()
        ));
    }
    Ok(root)
}

fn validate_native_artifact(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect Native artifact `{}`: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Native artifact must not be a symbolic link: {}",
            path.display()
        ));
    }
    if metadata.is_dir() {
        inspect_ios_xcframework(path)?;
        return path.canonicalize().map_err(|error| {
            format!(
                "could not resolve Native XCFramework `{}`: {error}",
                path.display()
            )
        });
    }
    if !metadata.is_file() {
        return Err(format!(
            "Native artifact must be a regular file or iOS XCFramework: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_NATIVE_LIBRARY_BYTES {
        return Err(format!(
            "Native artifact exceeds the {} byte safety limit: {}",
            MAX_NATIVE_LIBRARY_BYTES,
            path.display()
        ));
    }
    if !matches!(
        path.extension().and_then(OsStr::to_str),
        Some("dll" | "dylib" | "so" | "wasm")
    ) {
        return Err(format!(
            "Native artifact is not a supported dynamic library: {}",
            path.display()
        ));
    }
    path.canonicalize().map_err(|error| {
        format!(
            "could not resolve Native artifact `{}`: {error}",
            path.display()
        )
    })
}

fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.bytes().all(|byte| {
            byte == b'_' || byte == b'-' || byte == b'.' || byte.is_ascii_alphanumeric()
        })
}

fn read_or_create_state(
    state_path: &Path,
    descriptor_path: &Path,
    builds_root: &Path,
    plan: &NativeBuildPlan,
) -> Result<NativePublicationState, String> {
    let expected = NativePublicationState {
        format: STATE_FORMAT,
        package_name: plan.package.name.clone(),
        godot_api: plan.godot_api.to_string(),
        entry_symbol: plan.entry_symbol.clone(),
        libraries: BTreeMap::new(),
        descriptor_sha256: None,
        previous_descriptor_sha256: None,
        pending: None,
    };
    let mut state = match std::fs::symlink_metadata(state_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "Native publication state must be a regular non-symlink file: {}",
                state_path.display()
            ));
        }
        Ok(metadata) if metadata.len() > MAX_STATE_BYTES => {
            return Err(format!(
                "Native publication state exceeds the {} byte safety limit",
                MAX_STATE_BYTES
            ));
        }
        Ok(_) => {
            let source = std::fs::read(state_path).map_err(|error| {
                format!(
                    "could not read Native publication state `{}`: {error}",
                    state_path.display()
                )
            })?;
            serde_json::from_slice::<NativePublicationState>(&source).map_err(|error| {
                format!(
                    "Native publication state `{}` is invalid: {error}",
                    state_path.display()
                )
            })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(descriptor_path).is_ok() {
                return Err(format!(
                    "refusing to overwrite an unmanaged `.gdextension`: {}",
                    descriptor_path.display()
                ));
            }
            return Ok(expected);
        }
        Err(error) => {
            return Err(format!(
                "could not inspect Native publication state `{}`: {error}",
                state_path.display()
            ));
        }
    };
    if !matches!(state.format, 1 | STATE_FORMAT) || state.package_name != expected.package_name {
        return Err(format!(
            "existing Native publication state does not belong to package `{}`",
            plan.package.name
        ));
    }
    let legacy_state = state.format == 1;
    state.format = STATE_FORMAT;
    verify_state_libraries(&state, builds_root)?;
    if legacy_state {
        verify_legacy_descriptor(descriptor_path, &state)?;
        state.previous_descriptor_sha256 = None;
        state.pending = None;
    } else {
        recover_descriptor_transaction(state_path, descriptor_path, &mut state)?;
    }
    if state.godot_api != expected.godot_api || state.entry_symbol != expected.entry_symbol {
        state.godot_api = expected.godot_api;
        state.entry_symbol = expected.entry_symbol;
        state.libraries.clear();
        state.pending = None;
    }
    Ok(state)
}

fn verify_legacy_descriptor(
    descriptor_path: &Path,
    state: &NativePublicationState,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(descriptor_path).map_err(|error| {
        format!(
            "could not inspect legacy generated `.gdextension` `{}`: {error}",
            descriptor_path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "legacy generated `.gdextension` must be a regular non-symlink file: {}",
            descriptor_path.display()
        ));
    }
    if metadata.len() > MAX_STATE_BYTES {
        return Err(format!(
            "legacy generated `.gdextension` exceeds the {} byte safety limit",
            MAX_STATE_BYTES
        ));
    }
    let descriptor = std::fs::read(descriptor_path).map_err(|error| {
        format!(
            "could not read legacy generated `.gdextension` `{}`: {error}",
            descriptor_path.display()
        )
    })?;
    let actual = hash_bytes(&descriptor);
    if state.descriptor_sha256.as_deref() != Some(actual.as_str()) {
        return Err(format!(
            "legacy generated `.gdextension` was modified outside godot-rust: {}",
            descriptor_path.display()
        ));
    }
    Ok(())
}

fn verify_state_libraries(
    state: &NativePublicationState,
    builds_root: &Path,
) -> Result<(), String> {
    verify_library_records(&state.libraries, builds_root)?;
    if let Some(pending) = &state.pending {
        verify_library_records(&pending.previous_libraries, builds_root)?;
    }
    Ok(())
}

fn verify_library_records(
    libraries: &BTreeMap<String, PublishedLibraryRecord>,
    builds_root: &Path,
) -> Result<(), String> {
    for (selector, record) in libraries {
        if record.sha256.len() != 64
            || !record.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !is_safe_file_name(&record.module_file)
        {
            return Err(format!(
                "Native publication state has an invalid `{selector}` record"
            ));
        }
        let expected_resource_path = format!(
            "res://{NATIVE_ARTIFACT_DIRECTORY}/bin/{}/{}",
            record.sha256, record.module_file
        );
        if record.resource_path != expected_resource_path {
            return Err(format!(
                "Native publication state has an invalid `{selector}` resource path"
            ));
        }
        verify_generation(
            &builds_root.join(&record.sha256),
            &NativeGenerationManifest {
                format: GENERATION_FORMAT,
                sha256: record.sha256.clone(),
                byte_len: record.byte_len,
                module_file: record.module_file.clone(),
            },
        )?;
    }
    Ok(())
}

fn write_state_and_descriptor(
    state_path: &Path,
    descriptor_path: &Path,
    state: &NativePublicationState,
    plan: &NativeBuildPlan,
) -> Result<(), String> {
    descriptor_for_state(state, plan)?;
    write_current_state_and_descriptor(state_path, descriptor_path, state)
}

fn write_current_state_and_descriptor(
    state_path: &Path,
    descriptor_path: &Path,
    state: &NativePublicationState,
) -> Result<(), String> {
    let descriptor = descriptor_for_state_without_plan(state)?.render()?;
    let descriptor_sha256 = hash_bytes(descriptor.as_bytes());
    let mut prepared_state = state.clone();
    prepared_state.previous_descriptor_sha256 = prepared_state
        .descriptor_sha256
        .filter(|previous| previous != &descriptor_sha256);
    prepared_state.descriptor_sha256 = Some(descriptor_sha256);
    write_state(state_path, &prepared_state)?;
    atomic_write(
        descriptor_path,
        descriptor.as_bytes(),
        "generated `.gdextension`",
    )?;
    prepared_state.previous_descriptor_sha256 = None;
    write_state(state_path, &prepared_state)
}

pub fn commit_native_publication(
    project_root: impl AsRef<Path>,
    platform_selector: &str,
    sha256: &str,
) -> Result<NativePublicationResolution, String> {
    let project_root = canonical_project_root(project_root.as_ref())?;
    let state_path = project_root.join(NATIVE_STATE_FILE);
    let mut state = read_resolution_state(&project_root, &state_path)?;
    validate_pending_candidate(&state, platform_selector, sha256)?;
    state.pending = None;
    write_state(&state_path, &state)?;
    Ok(NativePublicationResolution {
        platform_selector: platform_selector.to_owned(),
        sha256: sha256.to_owned(),
        rolled_back: false,
    })
}

pub fn rollback_native_publication(
    project_root: impl AsRef<Path>,
    platform_selector: &str,
    sha256: &str,
) -> Result<NativePublicationResolution, String> {
    let project_root = canonical_project_root(project_root.as_ref())?;
    let state_path = project_root.join(NATIVE_STATE_FILE);
    let descriptor_path = project_root.join(NATIVE_DESCRIPTOR_FILE);
    let mut state = read_resolution_state(&project_root, &state_path)?;
    validate_pending_candidate(&state, platform_selector, sha256)?;
    let pending = state
        .pending
        .take()
        .expect("validated pending Native publication");
    if pending.previous_libraries.is_empty() {
        state.libraries.clear();
        reject_first_descriptor(&state_path, &descriptor_path, &mut state)?;
    } else {
        state.libraries = pending.previous_libraries;
        write_current_state_and_descriptor(&state_path, &descriptor_path, &state)?;
    }
    Ok(NativePublicationResolution {
        platform_selector: platform_selector.to_owned(),
        sha256: sha256.to_owned(),
        rolled_back: true,
    })
}

fn reject_first_descriptor(
    state_path: &Path,
    descriptor_path: &Path,
    state: &mut NativePublicationState,
) -> Result<(), String> {
    let recorded_hash = state
        .descriptor_sha256
        .as_deref()
        .ok_or_else(|| "Native publication state has no descriptor identity".to_owned())?;
    let metadata = std::fs::symlink_metadata(descriptor_path).map_err(|error| {
        format!(
            "could not inspect rejected `.gdextension` `{}`: {error}",
            descriptor_path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_STATE_BYTES
    {
        return Err(format!(
            "rejected `.gdextension` is not a managed regular file: {}",
            descriptor_path.display()
        ));
    }
    let actual_hash = hash_bytes(&std::fs::read(descriptor_path).map_err(|error| {
        format!(
            "could not read rejected `.gdextension` `{}`: {error}",
            descriptor_path.display()
        )
    })?);
    if actual_hash != recorded_hash {
        return Err(format!(
            "rejected `.gdextension` was modified outside godot-rust: {}",
            descriptor_path.display()
        ));
    }

    // Persist the empty committed set before removal. Recovery can finish the
    // deletion safely if the process stops between these two durable writes.
    write_state(state_path, state)?;
    std::fs::remove_file(descriptor_path).map_err(|error| {
        format!(
            "could not remove rejected `.gdextension` `{}`: {error}",
            descriptor_path.display()
        )
    })?;
    sync_directory(
        descriptor_path
            .parent()
            .expect("project descriptor always has a parent"),
    )?;
    state.descriptor_sha256 = None;
    state.previous_descriptor_sha256 = None;
    write_state(state_path, state)
}

fn read_resolution_state(
    project_root: &Path,
    state_path: &Path,
) -> Result<NativePublicationState, String> {
    let metadata = std::fs::symlink_metadata(state_path).map_err(|error| {
        format!(
            "could not inspect Native publication state `{}`: {error}",
            state_path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Native publication state must be a regular non-symlink file: {}",
            state_path.display()
        ));
    }
    if metadata.len() > MAX_STATE_BYTES {
        return Err(format!(
            "Native publication state exceeds the {} byte safety limit",
            MAX_STATE_BYTES
        ));
    }
    let source = std::fs::read(state_path).map_err(|error| {
        format!(
            "could not read Native publication state `{}`: {error}",
            state_path.display()
        )
    })?;
    let state: NativePublicationState = serde_json::from_slice(&source).map_err(|error| {
        format!(
            "Native publication state `{}` is invalid: {error}",
            state_path.display()
        )
    })?;
    if state.format != STATE_FORMAT {
        return Err(
            "Native publication state must be rebuilt before resolving a reload".to_owned(),
        );
    }
    verify_state_libraries(
        &state,
        &project_root.join(NATIVE_ARTIFACT_DIRECTORY).join("bin"),
    )?;
    Ok(state)
}

fn validate_pending_candidate(
    state: &NativePublicationState,
    platform_selector: &str,
    sha256: &str,
) -> Result<(), String> {
    let pending = state
        .pending
        .as_ref()
        .ok_or_else(|| "Native publication has no pending reload decision".to_owned())?;
    if pending.platform_selector != platform_selector || pending.candidate_sha256 != sha256 {
        return Err("Native reload decision does not match the pending generation".to_owned());
    }
    let current = state
        .libraries
        .get(platform_selector)
        .ok_or_else(|| "pending Native platform is missing from the descriptor".to_owned())?;
    if current.sha256 != sha256 {
        return Err("pending Native generation is no longer current".to_owned());
    }
    Ok(())
}

fn recover_descriptor_transaction(
    state_path: &Path,
    descriptor_path: &Path,
    state: &mut NativePublicationState,
) -> Result<(), String> {
    if state.libraries.is_empty() {
        return recover_empty_descriptor_transaction(state_path, descriptor_path, state);
    }
    let descriptor = descriptor_for_state_without_plan(state)?.render()?;
    let expected_hash = hash_bytes(descriptor.as_bytes());
    let mut state_changed = false;
    match state.descriptor_sha256.as_deref() {
        Some(recorded) if recorded == expected_hash => {}
        Some(_) => {
            return Err(
                "Native publication state descriptor hash does not match its contents".to_owned(),
            );
        }
        None => {
            state.descriptor_sha256 = Some(expected_hash.clone());
            state_changed = true;
        }
    }
    let actual_hash = match std::fs::symlink_metadata(descriptor_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "generated `.gdextension` must be a regular non-symlink file: {}",
                descriptor_path.display()
            ));
        }
        Ok(metadata) if metadata.len() > MAX_STATE_BYTES => {
            return Err(format!(
                "generated `.gdextension` exceeds the {} byte safety limit",
                MAX_STATE_BYTES
            ));
        }
        Ok(_) => Some(hash_bytes(&std::fs::read(descriptor_path).map_err(
            |error| {
                format!(
                    "could not read generated `.gdextension` `{}`: {error}",
                    descriptor_path.display()
                )
            },
        )?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "could not inspect generated `.gdextension` `{}`: {error}",
                descriptor_path.display()
            ));
        }
    };
    let interrupted = state.previous_descriptor_sha256.as_deref();
    match actual_hash.as_deref() {
        Some(actual) if actual == expected_hash => {}
        Some(actual) if interrupted == Some(actual) => {
            atomic_write(
                descriptor_path,
                descriptor.as_bytes(),
                "generated `.gdextension`",
            )?;
        }
        None => {
            atomic_write(
                descriptor_path,
                descriptor.as_bytes(),
                "generated `.gdextension`",
            )?;
        }
        Some(_) => {
            return Err(format!(
                "generated `.gdextension` was modified outside godot-rust: {}",
                descriptor_path.display()
            ));
        }
    }
    if state.previous_descriptor_sha256.take().is_some() {
        state_changed = true;
    }
    if state_changed {
        state.descriptor_sha256 = Some(expected_hash);
        write_state(state_path, state)?;
    }
    Ok(())
}

fn recover_empty_descriptor_transaction(
    state_path: &Path,
    descriptor_path: &Path,
    state: &mut NativePublicationState,
) -> Result<(), String> {
    let descriptor_metadata = std::fs::symlink_metadata(descriptor_path);
    match (state.descriptor_sha256.as_deref(), descriptor_metadata) {
        (Some(recorded), Ok(metadata))
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_STATE_BYTES =>
        {
            let actual = hash_bytes(&std::fs::read(descriptor_path).map_err(|error| {
                format!(
                    "could not read rejected `.gdextension` `{}`: {error}",
                    descriptor_path.display()
                )
            })?);
            if actual != recorded {
                return Err(format!(
                    "rejected `.gdextension` was modified outside godot-rust: {}",
                    descriptor_path.display()
                ));
            }
            std::fs::remove_file(descriptor_path).map_err(|error| {
                format!(
                    "could not finish removing rejected `.gdextension` `{}`: {error}",
                    descriptor_path.display()
                )
            })?;
            sync_directory(
                descriptor_path
                    .parent()
                    .expect("project descriptor always has a parent"),
            )?;
        }
        (Some(_), Err(error)) if error.kind() == io::ErrorKind::NotFound => {}
        (None, Err(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        (None, Ok(_)) => {
            return Err(format!(
                "refusing to remove an unmanaged `.gdextension`: {}",
                descriptor_path.display()
            ));
        }
        (_, Ok(_)) => {
            return Err(format!(
                "rejected `.gdextension` must be a managed regular file: {}",
                descriptor_path.display()
            ));
        }
        (_, Err(error)) => {
            return Err(format!(
                "could not inspect rejected `.gdextension` `{}`: {error}",
                descriptor_path.display()
            ));
        }
    }
    state.descriptor_sha256 = None;
    state.previous_descriptor_sha256 = None;
    state.pending = None;
    write_state(state_path, state)
}

fn descriptor_for_state(
    state: &NativePublicationState,
    plan: &NativeBuildPlan,
) -> Result<NativeExtensionDescriptor, String> {
    if state.godot_api != plan.compatibility_minimum || state.entry_symbol != plan.entry_symbol {
        return Err("Native publication state does not match the active build plan".to_owned());
    }
    descriptor_for_state_without_plan(state)
}

fn descriptor_for_state_without_plan(
    state: &NativePublicationState,
) -> Result<NativeExtensionDescriptor, String> {
    Ok(NativeExtensionDescriptor {
        entry_symbol: state.entry_symbol.clone(),
        compatibility_minimum: state.godot_api.clone(),
        reloadable: true,
        libraries: state
            .libraries
            .iter()
            .map(|(selector, record)| (selector.clone(), record.resource_path.clone()))
            .collect(),
    })
}

fn write_state(path: &Path, state: &NativePublicationState) -> Result<(), String> {
    let mut state_json = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("could not serialize Native publication state: {error}"))?;
    state_json.push(b'\n');
    atomic_write(path, &state_json, "Native publication state")
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn copy_library(source: &Path, destination: &Path) -> Result<(), String> {
    if source.extension().and_then(OsStr::to_str) == Some("xcframework") {
        copy_ios_xcframework(source, destination)?;
        return Ok(());
    }
    let mut source = File::open(source).map_err(|error| {
        format!(
            "could not open Native artifact `{}`: {error}",
            source.display()
        )
    })?;
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| format!("could not create staged Native artifact: {error}"))?;
    let byte_len = io::copy(&mut source, &mut destination)
        .map_err(|error| format!("could not copy Native artifact: {error}"))?;
    if byte_len > MAX_NATIVE_LIBRARY_BYTES {
        return Err(format!(
            "Native artifact exceeds the {} byte safety limit",
            MAX_NATIVE_LIBRARY_BYTES
        ));
    }
    destination
        .sync_all()
        .map_err(|error| format!("could not flush staged Native artifact: {error}"))
}

fn hash_library(path: &Path) -> Result<(String, u64), String> {
    if path.extension().and_then(OsStr::to_str) == Some("xcframework") {
        let identity = inspect_ios_xcframework(path)?;
        return Ok((identity.sha256, identity.byte_len));
    }
    let file = File::open(path)
        .map_err(|error| format!("could not open staged Native artifact: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut byte_len = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("could not hash staged Native artifact: {error}"))?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or_else(|| "Native artifact size overflowed u64".to_owned())?;
        if byte_len > MAX_NATIVE_LIBRARY_BYTES {
            return Err(format!(
                "Native artifact exceeds the {} byte safety limit",
                MAX_NATIVE_LIBRARY_BYTES
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), byte_len))
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("could not create `{}`: {error}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, value)
        .map_err(|error| format!("could not encode `{}`: {error}", path.display()))?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not flush `{}`: {error}", path.display()))
}

fn verify_generation(directory: &Path, expected: &NativeGenerationManifest) -> Result<(), String> {
    let directory_metadata = std::fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "could not inspect Native generation `{}`: {error}",
            directory.display()
        )
    })?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err(format!(
            "Native generation is not an owned directory: {}",
            directory.display()
        ));
    }
    let manifest_path = directory.join("generation.json");
    let manifest_metadata = std::fs::symlink_metadata(&manifest_path).map_err(|error| {
        format!(
            "could not inspect Native generation manifest `{}`: {error}",
            manifest_path.display()
        )
    })?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        return Err(format!(
            "Native generation manifest is not a regular file: {}",
            manifest_path.display()
        ));
    }
    if manifest_metadata.len() > MAX_GENERATION_MANIFEST_BYTES {
        return Err(format!(
            "Native generation manifest exceeds the {} byte safety limit",
            MAX_GENERATION_MANIFEST_BYTES
        ));
    }
    let manifest = std::fs::read(&manifest_path).map_err(|error| {
        format!(
            "could not read Native generation manifest `{}`: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: NativeGenerationManifest =
        serde_json::from_slice(&manifest).map_err(|error| {
            format!(
                "Native generation manifest `{}` is invalid: {error}",
                manifest_path.display()
            )
        })?;
    if &manifest != expected {
        return Err(format!(
            "Native generation metadata does not match `{}`",
            directory.display()
        ));
    }
    let library_path = directory.join(&manifest.module_file);
    let metadata = std::fs::symlink_metadata(&library_path).map_err(|error| {
        format!(
            "could not inspect Native generation library `{}`: {error}",
            library_path.display()
        )
    })?;
    if metadata.file_type().is_symlink()
        || (manifest.module_file.ends_with(".xcframework") && !metadata.is_dir())
        || (!manifest.module_file.ends_with(".xcframework") && !metadata.is_file())
    {
        return Err(format!(
            "Native generation library has the wrong filesystem type: {}",
            library_path.display()
        ));
    }
    let (sha256, byte_len) = hash_library(&library_path)?;
    if sha256 != manifest.sha256 || byte_len != manifest.byte_len {
        return Err(format!(
            "Native generation content does not match `{}`",
            directory.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CargoPackageModel, CargoTargetModel, GodotRustMode, NativeArchitecture, NativeBuildProfile,
        NativeOperatingSystem,
    };
    use godot_rs_api_policy::{GODOT_API_ENV, GodotApiVersion, NATIVE_ENTRY_SYMBOL};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    struct TempProject(PathBuf);

    impl TempProject {
        fn new() -> Self {
            let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "godot-rust-native-publication-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("temporary project");
            std::fs::write(path.join("project.godot"), b"[application]\n").expect("Godot project");
            Self(path.canonicalize().expect("canonical project"))
        }

        fn artifact(&self, content: &[u8]) -> PathBuf {
            let artifact = self.0.join(format!(
                "libgodothub_project.{}",
                dynamic_library_extension()
            ));
            std::fs::write(&artifact, content).expect("artifact");
            artifact
        }

        fn xcframework(&self) -> PathBuf {
            let root = self.0.join("godot_rs_project_module.xcframework");
            for directory in [
                root.join("ios-arm64"),
                root.join("ios-arm64_x86_64-simulator"),
            ] {
                std::fs::create_dir_all(directory).expect("XCFramework slice");
            }
            std::fs::write(
                root.join("Info.plist"),
                br#"<?xml version="1.0"?>
<plist><dict><key>AvailableLibraries</key><array>
<dict><key>SupportedArchitectures</key><array><string>arm64</string></array>
<key>SupportedPlatform</key><string>ios</string></dict>
<dict><key>SupportedArchitectures</key><array><string>arm64</string><string>x86_64</string></array>
<key>SupportedPlatform</key><string>ios</string>
<key>SupportedPlatformVariant</key><string>simulator</string></dict>
</array></dict></plist>
"#,
            )
            .expect("XCFramework Info.plist");
            std::fs::write(root.join("ios-arm64/libproject.dylib"), b"device library")
                .expect("device library");
            std::fs::write(
                root.join("ios-arm64_x86_64-simulator/libproject.dylib"),
                b"simulator library",
            )
            .expect("simulator library");
            root
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn plan(root: &Path, minor: u32) -> NativeBuildPlan {
        let target = CargoTargetModel {
            name: "godothub_project".to_owned(),
            src_path: root.join("src/lib.rs"),
        };
        let godot_api = GodotApiVersion::new(4, minor);
        NativeBuildPlan {
            project_root: root.to_owned(),
            package: CargoPackageModel {
                id: "godothub_project 0.1.0".to_owned(),
                name: "godothub_project".to_owned(),
                manifest_path: root.join("Cargo.toml"),
                workspace_default: true,
                godot_rs_dependency: true,
                godot_rust_enabled: true,
                godot_rust_mode: Some(GodotRustMode::Extension),
                godot_api: Some(godot_api),
                scripts_path: None,
                editor: crate::EditorWorkflowConfig::default(),
                script_mode_configured: false,
                configuration_issues: Vec::new(),
                cdylib_targets: vec![target.clone()],
            },
            target,
            godot_api,
            cargo_environment: BTreeMap::from([(GODOT_API_ENV.to_owned(), godot_api.to_string())]),
            entry_symbol: NATIVE_ENTRY_SYMBOL.to_owned(),
            compatibility_minimum: godot_api.to_string(),
        }
    }

    fn platform(
        operating_system: NativeOperatingSystem,
        profile: NativeBuildProfile,
    ) -> NativePlatform {
        NativePlatform {
            operating_system,
            architecture: NativeArchitecture::X86_64,
            profile,
        }
    }

    #[test]
    fn publication_is_immutable_and_merges_platform_mappings() {
        let project = TempProject::new();
        let artifact = project.artifact(b"native-library");
        let plan = plan(&project.0, 6);
        let first = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            platform(NativeOperatingSystem::Linux, NativeBuildProfile::Debug),
        )
        .expect("first publication");
        assert!(!first.reused);
        assert!(first.library_path.is_file());
        assert!(first.library_path.to_string_lossy().contains(&first.sha256));

        let second = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            platform(NativeOperatingSystem::Windows, NativeBuildProfile::Release),
        )
        .expect("second publication");
        assert!(second.reused);
        let descriptor = std::fs::read_to_string(&second.descriptor_path).expect("descriptor");
        assert!(descriptor.contains("compatibility_minimum = \"4.6\""));
        assert!(!descriptor.contains("compatibility_maximum"));
        assert!(descriptor.contains("linux.debug.x86_64"));
        assert!(descriptor.contains("windows.release.x86_64"));
    }

    #[test]
    fn failed_reload_can_restore_the_committed_generation() {
        let project = TempProject::new();
        let target = NativePlatform::current_debug().expect("test platform");
        let plan = plan(&project.0, 4);
        let first_artifact = project.artifact(b"first-native-generation");
        let first = publish_native_extension(&project.0, &plan, &first_artifact, target)
            .expect("first publication");
        assert!(!first.rollback_available);
        commit_native_publication(&project.0, &first.platform_selector, &first.sha256)
            .expect("commit first generation");

        let second_artifact = project.artifact(b"second-native-generation");
        let second = publish_native_extension(&project.0, &plan, &second_artifact, target)
            .expect("second publication");
        assert!(second.rollback_available);
        let candidate =
            std::fs::read_to_string(&second.descriptor_path).expect("candidate descriptor");
        assert!(candidate.contains(&second.sha256));
        assert!(!candidate.contains(&first.sha256));

        let resolution =
            rollback_native_publication(&project.0, &second.platform_selector, &second.sha256)
                .expect("rollback");
        assert!(resolution.rolled_back);
        let restored =
            std::fs::read_to_string(&second.descriptor_path).expect("restored descriptor");
        assert!(restored.contains(&first.sha256));
        assert!(!restored.contains(&second.sha256));
        assert!(
            commit_native_publication(&project.0, &second.platform_selector, &second.sha256)
                .is_err()
        );
    }

    #[test]
    fn rejected_first_generation_does_not_become_a_rollback_target() {
        let project = TempProject::new();
        let target = NativePlatform::current_debug().expect("test platform");
        let plan = plan(&project.0, 4);
        let first_artifact = project.artifact(b"rejected-first-generation");
        let first = publish_native_extension(&project.0, &plan, &first_artifact, target)
            .expect("first publication");
        assert!(!first.rollback_available);
        rollback_native_publication(&project.0, &first.platform_selector, &first.sha256)
            .expect("reject first generation");
        assert!(!first.descriptor_path.exists());

        let second_artifact = project.artifact(b"replacement-first-generation");
        let second = publish_native_extension(&project.0, &plan, &second_artifact, target)
            .expect("replacement publication");
        assert!(!second.rollback_available);
        let descriptor =
            std::fs::read_to_string(&second.descriptor_path).expect("replacement descriptor");
        assert!(descriptor.contains(&second.sha256));
        assert!(!descriptor.contains(&first.sha256));
    }

    #[test]
    fn superseded_candidates_keep_the_last_committed_rollback_target() {
        let project = TempProject::new();
        let target = NativePlatform::current_debug().expect("test platform");
        let plan = plan(&project.0, 4);
        let committed_artifact = project.artifact(b"committed-generation");
        let committed = publish_native_extension(&project.0, &plan, &committed_artifact, target)
            .expect("committed publication");
        commit_native_publication(&project.0, &committed.platform_selector, &committed.sha256)
            .expect("commit generation");

        let first_candidate_artifact = project.artifact(b"first-candidate");
        publish_native_extension(&project.0, &plan, &first_candidate_artifact, target)
            .expect("first candidate");
        let second_candidate_artifact = project.artifact(b"second-candidate");
        let second_candidate =
            publish_native_extension(&project.0, &plan, &second_candidate_artifact, target)
                .expect("second candidate");
        rollback_native_publication(
            &project.0,
            &second_candidate.platform_selector,
            &second_candidate.sha256,
        )
        .expect("rollback second candidate");
        let restored =
            std::fs::read_to_string(&second_candidate.descriptor_path).expect("descriptor");
        assert!(restored.contains(&committed.sha256));
        assert!(!restored.contains(&second_candidate.sha256));
    }

    #[test]
    fn rollback_rejects_a_tampered_previous_generation() {
        let project = TempProject::new();
        let target = NativePlatform::current_debug().expect("test platform");
        let plan = plan(&project.0, 4);
        let committed_artifact = project.artifact(b"committed-generation");
        let committed = publish_native_extension(&project.0, &plan, &committed_artifact, target)
            .expect("committed publication");
        commit_native_publication(&project.0, &committed.platform_selector, &committed.sha256)
            .expect("commit generation");
        let candidate_artifact = project.artifact(b"candidate-generation");
        let candidate = publish_native_extension(&project.0, &plan, &candidate_artifact, target)
            .expect("candidate publication");
        std::fs::write(&committed.library_path, b"tampered").expect("tamper committed library");
        assert!(
            rollback_native_publication(
                &project.0,
                &candidate.platform_selector,
                &candidate.sha256,
            )
            .is_err()
        );
    }

    #[test]
    fn ios_xcframework_publication_is_content_addressed_and_architecture_neutral() {
        let project = TempProject::new();
        let artifact = project.xcframework();
        let plan = plan(&project.0, 4);
        let publication = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            NativePlatform {
                operating_system: NativeOperatingSystem::Ios,
                architecture: NativeArchitecture::Arm64,
                profile: NativeBuildProfile::Release,
            },
        )
        .expect("iOS publication");
        assert!(publication.library_path.is_dir());
        assert_eq!(publication.platform_selector, "ios.release");
        let descriptor = std::fs::read_to_string(publication.descriptor_path).expect("descriptor");
        assert!(descriptor.contains("ios.release = "));
        assert!(descriptor.contains("godot_rs_project_module.xcframework"));
    }

    #[test]
    fn changing_the_api_target_resets_incompatible_platform_mappings() {
        let project = TempProject::new();
        let artifact = project.artifact(b"native-library");
        publish_native_extension(
            &project.0,
            &plan(&project.0, 6),
            &artifact,
            platform(NativeOperatingSystem::Linux, NativeBuildProfile::Debug),
        )
        .expect("4.6 publication");
        let report = publish_native_extension(
            &project.0,
            &plan(&project.0, 7),
            &artifact,
            platform(NativeOperatingSystem::Windows, NativeBuildProfile::Release),
        )
        .expect("4.7 publication");
        let descriptor = std::fs::read_to_string(report.descriptor_path).expect("descriptor");
        assert!(descriptor.contains("compatibility_minimum = \"4.7\""));
        assert!(!descriptor.contains("linux.debug.x86_64"));
        assert!(descriptor.contains("windows.release.x86_64"));
    }

    #[test]
    fn unmanaged_descriptors_are_never_overwritten() {
        let project = TempProject::new();
        let descriptor = project.0.join(NATIVE_DESCRIPTOR_FILE);
        std::fs::write(&descriptor, b"user-owned").expect("user descriptor");
        let artifact = project.artifact(b"native-library");
        let error = publish_native_extension(
            &project.0,
            &plan(&project.0, 4),
            &artifact,
            NativePlatform::current_debug().expect("test platform"),
        )
        .expect_err("unmanaged descriptor");
        assert!(error.contains("unmanaged"));
        assert_eq!(
            std::fs::read(descriptor).expect("descriptor"),
            b"user-owned"
        );
    }

    #[test]
    fn managed_descriptors_modified_by_users_are_not_silently_overwritten() {
        let project = TempProject::new();
        let artifact = project.artifact(b"native-library");
        let plan = plan(&project.0, 4);
        let first = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            NativePlatform::current_debug().expect("test platform"),
        )
        .expect("publication");
        std::fs::write(&first.descriptor_path, b"user-modified").expect("modify descriptor");
        let error = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            NativePlatform::current_debug().expect("test platform"),
        )
        .expect_err("modified descriptor");
        assert!(error.contains("modified outside godot-rust"));
        assert_eq!(
            std::fs::read(first.descriptor_path).expect("descriptor"),
            b"user-modified"
        );
    }

    #[test]
    fn tampered_immutable_generations_are_rejected() {
        let project = TempProject::new();
        let artifact = project.artifact(b"native-library");
        let plan = plan(&project.0, 5);
        let first = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            NativePlatform::current_debug().expect("test platform"),
        )
        .expect("publication");
        std::fs::write(&first.library_path, b"tampered").expect("tamper");
        let error = publish_native_extension(
            &project.0,
            &plan,
            &artifact,
            NativePlatform::current_debug().expect("test platform"),
        )
        .expect_err("tampered generation");
        assert!(error.contains("content does not match"));
    }

    #[cfg(target_os = "windows")]
    fn dynamic_library_extension() -> &'static str {
        "dll"
    }

    #[cfg(target_os = "macos")]
    fn dynamic_library_extension() -> &'static str {
        "dylib"
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn dynamic_library_extension() -> &'static str {
        "so"
    }
}
