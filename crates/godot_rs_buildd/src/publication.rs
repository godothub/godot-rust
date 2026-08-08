use crate::managed_fs::{
    create_temporary_directory, ensure_directory, replace_file, sync_directory,
};
use crate::module_artifact::{
    copy_module, ensure_regular_module, hash_module, validate_with_program,
};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const LAST_KNOWN_GOOD_FILE: &str = ".godot/rust/last-known-good.json";

const GENERATION_FORMAT: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct GenerationManifest {
    format: u32,
    build_id: String,
    module_file: String,
    byte_len: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LastKnownGood {
    format: u32,
    build_id: String,
    module_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublishedGeneration {
    pub build_id: String,
    pub module_path: PathBuf,
    pub last_known_good_path: PathBuf,
    pub reused: bool,
    pub validator_stdout: String,
    pub validator_stderr: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublishedGenerationIdentity {
    pub build_id: String,
    pub module_path: PathBuf,
    pub last_known_good_path: PathBuf,
}

pub fn publish_validated_generation(
    project_root: impl AsRef<Path>,
    artifact: impl AsRef<Path>,
    validator: impl AsRef<OsStr>,
) -> Result<PublishedGeneration, String> {
    publish_validated_generation_with(project_root.as_ref(), artifact.as_ref(), |module| {
        validate_with_program(module, validator.as_ref())
    })
}

pub(crate) fn publish_validated_generation_guarded(
    project_root: &Path,
    artifact: &Path,
    validator: &OsStr,
    before_commit: impl FnOnce() -> Result<(), String>,
) -> Result<PublishedGeneration, String> {
    publish_validated_generation_with_guard(
        project_root,
        artifact,
        |module| validate_with_program(module, validator),
        before_commit,
    )
}

pub fn verify_last_known_good(
    project_root: impl AsRef<Path>,
) -> Result<PublishedGenerationIdentity, String> {
    let project_root = canonical_project_root(project_root.as_ref())?;
    let managed_root = project_root.join(".godot/rust");
    let builds_root = managed_root.join("builds");
    for (path, purpose) in [
        (project_root.join(".godot"), "Godot managed directory"),
        (managed_root, "Rust managed directory"),
        (builds_root.clone(), "Rust generation directory"),
    ] {
        verify_owned_directory(&path, purpose)?;
    }

    let last_known_good_path = project_root.join(LAST_KNOWN_GOOD_FILE);
    let source = read_bounded_regular_file(
        &last_known_good_path,
        MAX_MANIFEST_BYTES,
        "Last Known Good record",
    )?;
    let last_known_good: LastKnownGood = serde_json::from_slice(&source).map_err(|error| {
        format!(
            "Last Known Good record is invalid `{}`: {error}",
            last_known_good_path.display()
        )
    })?;
    if last_known_good.format != GENERATION_FORMAT {
        return Err(format!(
            "Last Known Good format {} is unsupported",
            last_known_good.format
        ));
    }
    let hash = build_id_hash(&last_known_good.build_id)?;
    validate_managed_relative_path(&last_known_good.module_path)?;
    let extension = last_known_good
        .module_path
        .extension()
        .and_then(OsStr::to_str)
        .filter(|extension| matches!(*extension, "dll" | "dylib" | "so"))
        .ok_or_else(|| {
            format!(
                "Last Known Good module has an unsupported extension: {}",
                last_known_good.module_path.display()
            )
        })?;
    let expected_relative = PathBuf::from(".godot")
        .join("rust")
        .join("builds")
        .join(hash)
        .join(format!("project_module.{extension}"));
    if last_known_good.module_path != expected_relative {
        return Err(format!(
            "Last Known Good module is outside its content-addressed generation: {}",
            last_known_good.module_path.display()
        ));
    }

    let generation_directory = builds_root.join(hash);
    let manifest_path = generation_directory.join("generation.json");
    let manifest_source =
        read_bounded_regular_file(&manifest_path, MAX_MANIFEST_BYTES, "generation manifest")?;
    let manifest: GenerationManifest =
        serde_json::from_slice(&manifest_source).map_err(|error| {
            format!(
                "generation manifest is invalid `{}`: {error}",
                manifest_path.display()
            )
        })?;
    if manifest.build_id != last_known_good.build_id
        || manifest.module_file != format!("project_module.{extension}")
    {
        return Err(format!(
            "Last Known Good does not match generation metadata `{}`",
            manifest_path.display()
        ));
    }
    verify_generation(&generation_directory, &manifest)?;

    Ok(PublishedGenerationIdentity {
        build_id: last_known_good.build_id,
        module_path: project_root.join(expected_relative),
        last_known_good_path,
    })
}

fn publish_validated_generation_with<F>(
    project_root: &Path,
    artifact: &Path,
    validate: F,
) -> Result<PublishedGeneration, String>
where
    F: FnOnce(&Path) -> Result<(String, String), String>,
{
    publish_validated_generation_with_guard(project_root, artifact, validate, || Ok(()))
}

fn publish_validated_generation_with_guard<F, G>(
    project_root: &Path,
    artifact: &Path,
    validate: F,
    before_commit: G,
) -> Result<PublishedGeneration, String>
where
    F: FnOnce(&Path) -> Result<(String, String), String>,
    G: FnOnce() -> Result<(), String>,
{
    let project_root = canonical_project_root(project_root)?;
    let artifact = artifact.canonicalize().map_err(|error| {
        format!(
            "could not resolve project module artifact `{}`: {error}",
            artifact.display()
        )
    })?;
    ensure_regular_module(&artifact)?;

    let managed_root = project_root.join(".godot/rust");
    let builds_root = managed_root.join("builds");
    ensure_directory(&project_root.join(".godot"), "managed build directory")?;
    ensure_directory(&managed_root, "managed build directory")?;
    ensure_directory(&builds_root, "managed build directory")?;

    let extension = artifact
        .extension()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            format!(
                "project module artifact has no UTF-8 dynamic-library extension: {}",
                artifact.display()
            )
        })?;
    if !matches!(extension, "dll" | "dylib" | "so") {
        return Err(format!(
            "project module artifact extension must be .dll, .dylib, or .so: {}",
            artifact.display()
        ));
    }
    let module_file = format!("project_module.{extension}");
    let temporary = create_temporary_directory(&builds_root, ".tmp")?;
    let staged_module = temporary.join(&module_file);

    let stage_result = (|| {
        copy_module(&artifact, &staged_module)?;
        let (hash, byte_len) = hash_module(&staged_module)?;
        let build_id = format!("sha256:{hash}");
        let manifest = GenerationManifest {
            format: GENERATION_FORMAT,
            build_id: build_id.clone(),
            module_file: module_file.clone(),
            byte_len,
        };
        write_json_file(&temporary.join("generation.json"), &manifest)?;

        let generation_directory = builds_root.join(&hash);
        let reused = if generation_directory.exists() {
            verify_generation(&generation_directory, &manifest)?;
            true
        } else {
            match std::fs::rename(&temporary, &generation_directory) {
                Ok(()) => false,
                Err(_) if generation_directory.exists() => {
                    verify_generation(&generation_directory, &manifest)?;
                    true
                }
                Err(error) => {
                    return Err(format!(
                        "could not publish immutable module generation `{}`: {error}",
                        generation_directory.display()
                    ));
                }
            }
        };
        let module_path = generation_directory.join(&module_file);
        let (validator_stdout, validator_stderr) = validate(&module_path)?;
        before_commit()?;

        let relative_module_path = module_path
            .strip_prefix(&project_root)
            .expect("managed generation remains inside project root")
            .to_owned();
        let last_known_good = LastKnownGood {
            format: GENERATION_FORMAT,
            build_id: build_id.clone(),
            module_path: relative_module_path,
        };
        let last_known_good_path = project_root.join(LAST_KNOWN_GOOD_FILE);
        replace_json_file(&last_known_good_path, &last_known_good)?;
        sync_directory(&managed_root)?;

        Ok(PublishedGeneration {
            build_id,
            module_path,
            last_known_good_path,
            reused,
            validator_stdout,
            validator_stderr,
        })
    })();

    if temporary.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    stage_result
}

fn canonical_project_root(root: &Path) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}`: {error}",
            root.display()
        )
    })?;
    if !root.join("project.godot").is_file() {
        return Err(format!(
            "directory does not contain project.godot: {}",
            root.display()
        ));
    }
    Ok(root)
}

fn verify_owned_directory(path: &Path, purpose: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {purpose} `{}`: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "{purpose} is not an owned directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn read_bounded_regular_file(path: &Path, limit: u64, purpose: &str) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {purpose} `{}`: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{purpose} is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > limit {
        return Err(format!("{purpose} exceeds the {limit} byte safety limit"));
    }
    std::fs::read(path)
        .map_err(|error| format!("could not read {purpose} `{}`: {error}", path.display()))
}

fn build_id_hash(build_id: &str) -> Result<&str, String> {
    let hash = build_id
        .strip_prefix("sha256:")
        .filter(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
        .ok_or_else(|| "Last Known Good Build ID is not a canonical SHA-256 digest".to_owned())?;
    Ok(hash)
}

fn validate_managed_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "Last Known Good module path is not a safe relative path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_generation(directory: &Path, expected: &GenerationManifest) -> Result<(), String> {
    let directory_metadata = std::fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "could not inspect existing generation directory `{}`: {error}",
            directory.display()
        )
    })?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err(format!(
            "existing generation path is not an owned directory: {}",
            directory.display()
        ));
    }
    let manifest_path = directory.join("generation.json");
    let metadata = std::fs::symlink_metadata(&manifest_path).map_err(|error| {
        format!(
            "could not inspect existing generation manifest `{}`: {error}",
            manifest_path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "existing generation manifest is not a regular file: {}",
            manifest_path.display()
        ));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "existing generation manifest exceeds the {} byte safety limit",
            MAX_MANIFEST_BYTES
        ));
    }
    let manifest = std::fs::read(&manifest_path).map_err(|error| {
        format!(
            "could not read existing generation manifest `{}`: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: GenerationManifest = serde_json::from_slice(&manifest).map_err(|error| {
        format!(
            "existing generation manifest is invalid `{}`: {error}",
            manifest_path.display()
        )
    })?;
    if &manifest != expected {
        return Err(format!(
            "existing immutable generation metadata does not match `{}`",
            directory.display()
        ));
    }
    let module = directory.join(&manifest.module_file);
    let module_metadata = std::fs::symlink_metadata(&module).map_err(|error| {
        format!(
            "could not inspect existing generation module `{}`: {error}",
            module.display()
        )
    })?;
    if module_metadata.file_type().is_symlink() || !module_metadata.is_file() {
        return Err(format!(
            "existing generation module is not a regular file: {}",
            module.display()
        ));
    }
    let (hash, byte_len) = hash_module(&module)?;
    if manifest.build_id != format!("sha256:{hash}") || manifest.byte_len != byte_len {
        return Err(format!(
            "existing immutable generation content does not match `{}`",
            directory.display()
        ));
    }
    Ok(())
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<(), String> {
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

fn replace_json_file(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("last-known-good path has no parent: {}", path.display()))?;
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".last-known-good-{}-{id}.tmp", std::process::id()));
    write_json_file(&temporary, value)?;
    if let Err(error) = replace_file(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "could not atomically publish Last Known Good `{}`: {error}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempProject(PathBuf);

    impl TempProject {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "godot-rust-buildd-publication-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("temporary project directory");
            std::fs::write(path.join("project.godot"), b"[application]\n")
                .expect("project fixture");
            Self(path)
        }

        fn artifact(&self, name: &str, content: &[u8]) -> PathBuf {
            let artifact = self
                .0
                .join(format!("{name}.{}", dynamic_library_extension()));
            std::fs::write(&artifact, content).expect("artifact fixture");
            artifact
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn validated_artifact_becomes_immutable_last_known_good() {
        let project = TempProject::new();
        let artifact = project.artifact("candidate", b"valid module");
        let report = publish_validated_generation_with(&project.0, &artifact, |module| {
            assert_eq!(
                std::fs::read(module).expect("staged module"),
                b"valid module"
            );
            Ok(("validated".to_owned(), String::new()))
        })
        .expect("publication");

        assert!(report.build_id.starts_with("sha256:"));
        assert!(report.module_path.is_file());
        assert_eq!(report.validator_stdout, "validated");
        let current = read_last_known_good(&project.0);
        assert_eq!(current.build_id, report.build_id);
        assert!(current.module_path.is_relative());

        let reused = publish_validated_generation_with(&project.0, &artifact, |_| {
            Ok((String::new(), String::new()))
        })
        .expect("reused publication");
        assert!(reused.reused);
        assert_eq!(reused.module_path, report.module_path);

        let verified = verify_last_known_good(&project.0).expect("verified Last Known Good");
        assert_eq!(verified.build_id, report.build_id);
        assert_eq!(verified.module_path, report.module_path);
        assert_eq!(
            verified.last_known_good_path,
            project
                .0
                .canonicalize()
                .expect("canonical project")
                .join(LAST_KNOWN_GOOD_FILE)
        );
    }

    #[test]
    fn rejected_candidate_does_not_replace_last_known_good() {
        let project = TempProject::new();
        let first = project.artifact("first", b"first valid module");
        let first = publish_validated_generation_with(&project.0, &first, |_| {
            Ok((String::new(), String::new()))
        })
        .expect("first publication");

        let rejected = project.artifact("rejected", b"broken module");
        let error = publish_validated_generation_with(&project.0, &rejected, |_| {
            Err("ABI mismatch".to_owned())
        })
        .expect_err("rejected publication");
        assert!(error.contains("ABI mismatch"));
        assert_eq!(read_last_known_good(&project.0).build_id, first.build_id);
        assert!(
            project
                .0
                .join(".godot/rust/builds")
                .read_dir()
                .expect("build directory")
                .all(|entry| !entry
                    .expect("build entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tmp-"))
        );
    }

    #[test]
    fn failed_precommit_guard_does_not_replace_last_known_good() {
        let project = TempProject::new();
        let first = project.artifact("first", b"first valid module");
        let first = publish_validated_generation_with(&project.0, &first, |_| {
            Ok((String::new(), String::new()))
        })
        .expect("first publication");

        let changed = project.artifact("changed", b"changed while building");
        let error = publish_validated_generation_with_guard(
            &project.0,
            &changed,
            |_| Ok(("validated".to_owned(), String::new())),
            || Err("source input changed".to_owned()),
        )
        .expect_err("guarded publication");
        assert!(error.contains("source input changed"));
        assert_eq!(read_last_known_good(&project.0).build_id, first.build_id);
    }

    #[test]
    fn last_known_good_verification_rejects_module_tampering() {
        let project = TempProject::new();
        let artifact = project.artifact("candidate", b"valid module");
        let report = publish_validated_generation_with(&project.0, &artifact, |_| {
            Ok((String::new(), String::new()))
        })
        .expect("publication");
        std::fs::write(&report.module_path, b"tampered module").expect("tampered module");

        let error = verify_last_known_good(&project.0).expect_err("tampering must be rejected");
        assert!(error.contains("content does not match"));
    }

    #[test]
    fn last_known_good_verification_rejects_noncanonical_paths() {
        let project = TempProject::new();
        let managed_root = project.0.join(".godot/rust");
        std::fs::create_dir_all(managed_root.join("builds")).expect("managed directory");
        std::fs::write(
            managed_root.join("last-known-good.json"),
            br#"{
                "format": 1,
                "build_id": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "module_path": "../outside.dylib"
            }"#,
        )
        .expect("Last Known Good record");

        let error =
            verify_last_known_good(&project.0).expect_err("path traversal must be rejected");
        assert!(error.contains("safe relative path"));
    }

    fn read_last_known_good(root: &Path) -> LastKnownGood {
        let source =
            std::fs::read(root.join(LAST_KNOWN_GOOD_FILE)).expect("last-known-good record");
        serde_json::from_slice(&source).expect("last-known-good JSON")
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
