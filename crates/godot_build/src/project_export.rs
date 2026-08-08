use crate::managed_fs::{
    create_temporary_directory, ensure_directory, replace_file, sync_directory,
};
use crate::module_artifact::{
    MAX_MODULE_BYTES, copy_module, ensure_regular_module, hash_module,
    validate_cross_target_module, validate_cross_target_module_entry, validate_with_program,
};
use crate::xcframework::{create_ios_xcframework, inspect_ios_xcframework};
use crate::{
    CargoTaskKind, CargoTaskReport, ExportArchitecture, ExportArtifactTarget,
    ExportOperatingSystem, GodotRustMode, NATIVE_DESCRIPTOR_FILE, NativePlatform,
    ProjectExportTarget, ScriptIndexReport, commit_native_publication, inspect_cargo_project,
    plan_native_build, publish_native_extension, run_native_package_cargo_build,
    run_package_cargo_build, run_package_cargo_task, sync_configured_script_index,
};
use godot_api::GodotApiVersion;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_TOOL_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExportedProjectModule {
    pub build_id: String,
    pub architecture: ExportArchitecture,
    pub rust_targets: Vec<String>,
    pub godot_tags: Vec<String>,
    pub module_path: PathBuf,
    pub byte_len: u64,
    pub validator_stdout: String,
    pub validator_stderr: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectExportReport {
    pub package_name: String,
    pub mode: GodotRustMode,
    pub godot_api: GodotApiVersion,
    pub runtime_godot: GodotApiVersion,
    pub target: ProjectExportTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_index: Option<ScriptIndexReport>,
    pub cargo: Vec<CargoTaskReport>,
    pub modules: Vec<ExportedProjectModule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_descriptor_resource_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectExportEnvironment {
    pub runtime_godot: GodotApiVersion,
    pub android_sdk: Option<PathBuf>,
}

pub fn build_project_for_export(
    project_root: impl AsRef<Path>,
    platform: &str,
    features: &[String],
    is_debug: bool,
    environment: ProjectExportEnvironment,
    cargo: Option<impl AsRef<OsStr>>,
    validator: impl AsRef<OsStr>,
) -> Result<ProjectExportReport, String> {
    let runtime_godot = environment.runtime_godot;
    let target = ProjectExportTarget::from_godot(platform, features, is_debug)?;
    target.ensure_host_compatible()?;
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let project = inspect_cargo_project(project_root.as_ref(), Some(&cargo))?;
    if project.native_package.is_some() {
        return build_native_project_for_export(
            project_root.as_ref(),
            project,
            target,
            environment,
            cargo,
        );
    }
    let package = project.selected_package.clone().ok_or_else(|| {
        if project.issues.is_empty() {
            "Cargo project does not identify a Rust script package for export".to_owned()
        } else {
            format!(
                "Cargo project does not identify a Rust script package for export: {}",
                project.issues.join("; ")
            )
        }
    })?;
    if package.godot_rust_mode != Some(GodotRustMode::Script) {
        return Err("Godot project export requires a configured Rust script package".to_owned());
    }
    let godot_api = package
        .godot_api
        .unwrap_or_else(|| GodotApiVersion::new(4, 4));
    if runtime_godot < godot_api {
        return Err(format!(
            "Cargo package `{}` requires Godot {godot_api}, but this export runs from Godot \
             {runtime_godot}",
            package.name
        ));
    }
    let script_index = sync_configured_script_index(project_root.as_ref(), &package)?;
    let mut cargo_reports = Vec::new();
    let semantic_validation = if target.needs_native_semantic_validation() {
        let report = run_package_cargo_task(
            project_root.as_ref(),
            CargoTaskKind::Build,
            &package,
            Some(&cargo),
        )?;
        let success = report.success;
        let artifact = success
            .then(|| exact_artifact(&package.name, "editor host", &report))
            .transpose()?;
        cargo_reports.push(report);
        if !success {
            return Ok(ProjectExportReport {
                package_name: package.name,
                mode: GodotRustMode::Script,
                godot_api,
                runtime_godot,
                target,
                script_index,
                cargo: cargo_reports,
                modules: Vec::new(),
                native_descriptor_resource_path: None,
            });
        }
        let artifact = artifact.expect("successful native build has an artifact");
        Some(validate_with_program(&artifact, validator.as_ref())?)
    } else {
        None
    };

    let mut artifact_sets = Vec::with_capacity(target.artifacts.len());
    for artifact_target in &target.artifacts {
        let mut artifacts = Vec::with_capacity(artifact_target.rust_targets.len());
        for rust_target in &artifact_target.rust_targets {
            let cargo_report = run_package_cargo_build(
                project_root.as_ref(),
                &package,
                rust_target,
                target.profile.is_release(),
                runtime_godot,
                environment.android_sdk.as_deref(),
                Some(&cargo),
            )?;
            let success = cargo_report.success;
            if success {
                artifacts.push(exact_artifact(&package.name, rust_target, &cargo_report)?);
            }
            cargo_reports.push(cargo_report);
            if !success {
                return Ok(ProjectExportReport {
                    package_name: package.name,
                    mode: GodotRustMode::Script,
                    godot_api,
                    runtime_godot,
                    target,
                    script_index,
                    cargo: cargo_reports,
                    modules: Vec::new(),
                    native_descriptor_resource_path: None,
                });
            }
        }
        artifact_sets.push(artifacts);
    }

    let mut modules = Vec::with_capacity(target.artifacts.len());
    for (artifact_target, artifacts) in target.artifacts.iter().zip(&artifact_sets) {
        modules.push(publish_export_module(
            project_root.as_ref(),
            &target,
            artifact_target,
            artifacts,
            validator.as_ref(),
            semantic_validation.as_ref(),
        )?);
    }
    Ok(ProjectExportReport {
        package_name: package.name,
        mode: GodotRustMode::Script,
        godot_api,
        runtime_godot,
        target,
        script_index,
        cargo: cargo_reports,
        modules,
        native_descriptor_resource_path: None,
    })
}

fn build_native_project_for_export(
    project_root: &Path,
    project: crate::CargoProjectModel,
    target: ProjectExportTarget,
    environment: ProjectExportEnvironment,
    cargo: OsString,
) -> Result<ProjectExportReport, String> {
    let plan = plan_native_build(&project)?;
    if environment.runtime_godot < plan.godot_api {
        return Err(format!(
            "Native package `{}` requires Godot {}, but this export runs from Godot {}",
            plan.package.name, plan.godot_api, environment.runtime_godot
        ));
    }
    let mut cargo_reports = Vec::new();
    let mut artifact_sets = Vec::with_capacity(target.artifacts.len());
    for artifact_target in &target.artifacts {
        let mut artifacts = Vec::with_capacity(artifact_target.rust_targets.len());
        for rust_target in &artifact_target.rust_targets {
            let report = run_native_package_cargo_build(
                project_root,
                &plan.package,
                rust_target,
                target.profile.is_release(),
                environment.runtime_godot,
                environment.android_sdk.as_deref(),
                Some(&cargo),
            )?;
            let success = report.success;
            if success {
                artifacts.push(exact_artifact(&plan.package.name, rust_target, &report)?);
            }
            cargo_reports.push(report);
            if !success {
                return Ok(ProjectExportReport {
                    package_name: plan.package.name,
                    mode: GodotRustMode::Extension,
                    godot_api: plan.godot_api,
                    runtime_godot: environment.runtime_godot,
                    target,
                    script_index: None,
                    cargo: cargo_reports,
                    modules: Vec::new(),
                    native_descriptor_resource_path: None,
                });
            }
        }
        artifact_sets.push(artifacts);
    }

    let staging_root = project_root.join(".godot/rust/native-export-staging");
    ensure_directory(&project_root.join(".godot"), "Godot managed directory")?;
    ensure_directory(&project_root.join(".godot/rust"), "Rust managed directory")?;
    ensure_directory(&staging_root, "Native export staging directory")?;
    let temporary = create_temporary_directory(&staging_root, ".tmp")?;
    let result = (|| {
        let mut candidates = Vec::with_capacity(target.artifacts.len());
        for (index, (artifact_target, artifacts)) in
            target.artifacts.iter().zip(&artifact_sets).enumerate()
        {
            let (candidate, validator_stdout, validator_stderr) = match artifacts.as_slice() {
                [device, simulator_arm64, simulator_x86_64]
                    if target.operating_system == ExportOperatingSystem::Ios =>
                {
                    let candidate = temporary.join(&artifact_target.module_file);
                    let (stdout, stderr) = validate_ios_slices(
                        artifacts,
                        &plan.entry_symbol,
                        target.operating_system,
                    )?;
                    create_ios_xcframework(device, simulator_arm64, simulator_x86_64, &candidate)?;
                    (candidate, stdout, stderr)
                }
                [artifact] => {
                    let (stdout, stderr) = validate_cross_target_module_entry(
                        artifact,
                        target.operating_system,
                        artifact_target.architecture,
                        &plan.entry_symbol,
                    )?;
                    (artifact.clone(), stdout, stderr)
                }
                [first, second]
                    if artifact_target.architecture == ExportArchitecture::Universal =>
                {
                    let candidate = temporary.join(format!("universal-{index}.dylib"));
                    merge_macos_universal(first, second, &candidate)?;
                    let (stdout, stderr) = validate_cross_target_module_entry(
                        &candidate,
                        target.operating_system,
                        artifact_target.architecture,
                        &plan.entry_symbol,
                    )?;
                    (candidate, stdout, stderr)
                }
                _ => {
                    return Err("Native export received an unsupported artifact set".to_owned());
                }
            };
            candidates.push((
                artifact_target,
                candidate,
                (validator_stdout, validator_stderr),
            ));
        }

        let mut modules = Vec::with_capacity(candidates.len());
        let mut descriptor_resource_path = None;
        for (artifact_target, candidate, (validator_stdout, validator_stderr)) in candidates {
            let platform = NativePlatform::from_export_target(
                target.operating_system,
                artifact_target.architecture,
                target.profile,
            );
            let publication = publish_native_extension(project_root, &plan, &candidate, platform)?;
            commit_native_publication(
                project_root,
                &publication.platform_selector,
                &publication.sha256,
            )?;
            descriptor_resource_path = Some(project_resource_path(
                &plan.project_root,
                &publication.descriptor_path,
            )?);
            modules.push(ExportedProjectModule {
                build_id: format!("sha256:{}", publication.sha256),
                architecture: artifact_target.architecture,
                rust_targets: artifact_target.rust_targets.clone(),
                godot_tags: artifact_target.godot_tags.clone(),
                module_path: publication.library_path,
                byte_len: publication.byte_len,
                validator_stdout,
                validator_stderr,
            });
        }
        Ok(ProjectExportReport {
            package_name: plan.package.name,
            mode: GodotRustMode::Extension,
            godot_api: plan.godot_api,
            runtime_godot: environment.runtime_godot,
            target,
            script_index: None,
            cargo: cargo_reports,
            modules,
            native_descriptor_resource_path: descriptor_resource_path,
        })
    })();
    if temporary.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

fn project_resource_path(project_root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(project_root).map_err(|_| {
        format!(
            "Native descriptor is outside the Godot project: {}",
            path.display()
        )
    })?;
    let components = relative
        .components()
        .map(|component| {
            let std::path::Component::Normal(component) = component else {
                return Err(format!(
                    "Native descriptor has an unsafe project-relative path: {}",
                    path.display()
                ));
            };
            component.to_str().ok_or_else(|| {
                format!(
                    "Native descriptor path is not valid UTF-8: {}",
                    path.display()
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components != [NATIVE_DESCRIPTOR_FILE] {
        return Err(format!(
            "Native descriptor is not at the managed project path: {}",
            path.display()
        ));
    }
    Ok(format!("res://{}", components.join("/")))
}

fn exact_artifact(
    package_name: &str,
    rust_target: &str,
    report: &CargoTaskReport,
) -> Result<PathBuf, String> {
    match report.artifacts.as_slice() {
        [artifact] => Ok(artifact.clone()),
        [] => Err(format!(
            "Cargo built `{package_name}` for `{rust_target}` successfully but reported no \
             dynamic library artifact"
        )),
        artifacts => Err(format!(
            "Cargo built `{package_name}` for `{rust_target}` but reported {} dynamic library \
             artifacts; exactly one is required",
            artifacts.len()
        )),
    }
}

fn publish_export_module(
    project_root: &Path,
    target: &ProjectExportTarget,
    artifact_target: &ExportArtifactTarget,
    artifacts: &[PathBuf],
    validator: &OsStr,
    semantic_validation: Option<&(String, String)>,
) -> Result<ExportedProjectModule, String> {
    if artifacts.len() != artifact_target.rust_targets.len() {
        return Err(format!(
            "Rust export expected {} target artifact(s), found {}",
            artifact_target.rust_targets.len(),
            artifacts.len()
        ));
    }
    if target.operating_system == ExportOperatingSystem::Ios {
        return publish_ios_export_module(
            project_root,
            target,
            artifact_target,
            artifacts,
            semantic_validation,
        );
    }
    publish_export_module_with(
        project_root,
        target,
        artifact_target,
        |candidate| match artifacts {
            [artifact] => copy_module(artifact, candidate),
            [first, second] if artifact_target.architecture == ExportArchitecture::Universal => {
                merge_macos_universal(first, second, candidate)
            }
            _ => Err("Rust export received an unsupported artifact set".to_owned()),
        },
        |candidate| {
            if let Some((semantic_stdout, semantic_stderr)) = semantic_validation {
                let (structural_stdout, structural_stderr) = validate_cross_target_module(
                    candidate,
                    target.operating_system,
                    artifact_target.architecture,
                )?;
                return Ok((
                    format!(
                        "native ABI validation:\n{semantic_stdout}cross-target validation:\n\
                         {structural_stdout}"
                    ),
                    format!("{semantic_stderr}{structural_stderr}"),
                ));
            }
            validate_with_program(candidate, validator)
        },
    )
}

fn publish_ios_export_module(
    project_root: &Path,
    target: &ProjectExportTarget,
    artifact_target: &ExportArtifactTarget,
    artifacts: &[PathBuf],
    semantic_validation: Option<&(String, String)>,
) -> Result<ExportedProjectModule, String> {
    let [device, simulator_arm64, simulator_x86_64] = artifacts else {
        return Err(format!(
            "iOS Rust export expected device ARM64 and Simulator ARM64/x86_64 artifacts, found {}",
            artifacts.len()
        ));
    };
    let project_root = canonical_export_project_root(project_root)?;
    let profile_root = export_profile_root(&project_root, target, artifact_target)?;
    let temporary = create_temporary_directory(&profile_root, ".tmp")?;
    let candidate = temporary.join(&artifact_target.module_file);
    let result = (|| {
        let (structural_stdout, structural_stderr) =
            validate_ios_slices(artifacts, "godot_rs_module_entry", target.operating_system)?;
        let identity =
            create_ios_xcframework(device, simulator_arm64, simulator_x86_64, &candidate)?;
        let generation_directory = profile_root.join(&identity.sha256);
        let module_path = generation_directory.join(&artifact_target.module_file);
        if generation_directory.exists() {
            let existing = inspect_ios_xcframework(&module_path)?;
            if existing != identity {
                return Err(format!(
                    "existing iOS XCFramework generation does not match its content address: {}",
                    generation_directory.display()
                ));
            }
        } else {
            std::fs::rename(&temporary, &generation_directory).map_err(|error| {
                format!(
                    "could not publish immutable iOS XCFramework generation `{}`: {error}",
                    generation_directory.display()
                )
            })?;
            sync_directory(&profile_root)?;
        }
        let (semantic_stdout, semantic_stderr) = semantic_validation
            .map(|(stdout, stderr)| (stdout.as_str(), stderr.as_str()))
            .unwrap_or(("", ""));
        Ok(ExportedProjectModule {
            build_id: format!("sha256:{}", identity.sha256),
            architecture: artifact_target.architecture,
            rust_targets: artifact_target.rust_targets.clone(),
            godot_tags: artifact_target.godot_tags.clone(),
            module_path,
            byte_len: identity.byte_len,
            validator_stdout: format!(
                "native ABI validation:\n{semantic_stdout}iOS slice validation:\n\
                 {structural_stdout}XCFramework validation:\nvalidated {} files\n",
                identity.file_count
            ),
            validator_stderr: format!("{semantic_stderr}{structural_stderr}"),
        })
    })();
    if temporary.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

fn validate_ios_slices(
    artifacts: &[PathBuf],
    expected_entry: &str,
    operating_system: ExportOperatingSystem,
) -> Result<(String, String), String> {
    let [device, simulator_arm64, simulator_x86_64] = artifacts else {
        return Err("iOS XCFramework validation requires exactly three Cargo artifacts".to_owned());
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    for (label, artifact, architecture) in [
        ("device ARM64", device, ExportArchitecture::Arm64),
        (
            "Simulator ARM64",
            simulator_arm64,
            ExportArchitecture::Arm64,
        ),
        (
            "Simulator x86_64",
            simulator_x86_64,
            ExportArchitecture::X86_64,
        ),
    ] {
        let (slice_stdout, slice_stderr) = validate_cross_target_module_entry(
            artifact,
            operating_system,
            architecture,
            expected_entry,
        )?;
        stdout.push_str(label);
        stdout.push_str(": ");
        stdout.push_str(&slice_stdout);
        stderr.push_str(&slice_stderr);
    }
    Ok((stdout, stderr))
}

fn canonical_export_project_root(project_root: &Path) -> Result<PathBuf, String> {
    let project_root = project_root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}`: {error}",
            project_root.display()
        )
    })?;
    if !project_root.join("project.godot").is_file() {
        return Err(format!(
            "Godot project root has no project.godot: {}",
            project_root.display()
        ));
    }
    Ok(project_root)
}

fn export_profile_root(
    project_root: &Path,
    target: &ProjectExportTarget,
    artifact_target: &ExportArtifactTarget,
) -> Result<PathBuf, String> {
    let godot_root = project_root.join(".godot");
    let rust_root = godot_root.join("rust");
    let exports_root = rust_root.join("exports");
    let target_root = exports_root.join(&artifact_target.staging_key);
    let profile_root = target_root.join(target.profile.directory_name());
    for (directory, purpose) in [
        (&godot_root, "Godot managed directory"),
        (&rust_root, "Rust managed directory"),
        (&exports_root, "Rust export directory"),
        (&target_root, "Rust export target directory"),
        (&profile_root, "Rust export profile directory"),
    ] {
        ensure_directory(directory, purpose)?;
    }
    Ok(profile_root)
}

fn publish_export_module_with<W, V>(
    project_root: &Path,
    target: &ProjectExportTarget,
    artifact_target: &ExportArtifactTarget,
    write_candidate: W,
    validate: V,
) -> Result<ExportedProjectModule, String>
where
    W: FnOnce(&Path) -> Result<(), String>,
    V: FnOnce(&Path) -> Result<(String, String), String>,
{
    let project_root = project_root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}`: {error}",
            project_root.display()
        )
    })?;
    if !project_root.join("project.godot").is_file() {
        return Err(format!(
            "Godot project root has no project.godot: {}",
            project_root.display()
        ));
    }
    let godot_root = project_root.join(".godot");
    let rust_root = godot_root.join("rust");
    let exports_root = rust_root.join("exports");
    let target_root = exports_root.join(&artifact_target.staging_key);
    let profile_root = target_root.join(target.profile.directory_name());
    for (directory, purpose) in [
        (&godot_root, "Godot managed directory"),
        (&rust_root, "Rust managed directory"),
        (&exports_root, "Rust export directory"),
        (&target_root, "Rust export target directory"),
        (&profile_root, "Rust export profile directory"),
    ] {
        ensure_directory(directory, purpose)?;
    }

    let temporary = create_temporary_directory(&profile_root, ".tmp")?;
    let candidate = temporary.join(&artifact_target.module_file);
    let result = (|| {
        write_candidate(&candidate)?;
        ensure_regular_module(&candidate)?;
        let (hash, byte_len) = hash_module(&candidate)?;
        let (validator_stdout, validator_stderr) = validate(&candidate)?;
        let module_path = profile_root.join(&artifact_target.module_file);
        validate_replaceable_destination(&module_path)?;
        replace_file(&candidate, &module_path).map_err(|error| {
            format!(
                "could not atomically publish Rust export module `{}`: {error}",
                module_path.display()
            )
        })?;
        sync_directory(&profile_root)?;
        Ok(ExportedProjectModule {
            build_id: format!("sha256:{hash}"),
            architecture: artifact_target.architecture,
            rust_targets: artifact_target.rust_targets.clone(),
            godot_tags: artifact_target.godot_tags.clone(),
            module_path,
            byte_len,
            validator_stdout,
            validator_stderr,
        })
    })();
    if temporary.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

fn validate_replaceable_destination(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(format!(
            "refusing to replace Rust export module that is not a regular file: {}",
            path.display()
        )),
        Ok(metadata) if metadata.len() > MAX_MODULE_BYTES => Err(format!(
            "existing Rust export module exceeds the {} byte safety limit",
            MAX_MODULE_BYTES
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not inspect existing Rust export module `{}`: {error}",
            path.display()
        )),
    }
}

fn merge_macos_universal(x86_64: &Path, arm64: &Path, destination: &Path) -> Result<(), String> {
    ensure_regular_module(x86_64)?;
    ensure_regular_module(arm64)?;
    let output = crate::process::run_command(
        Command::new("lipo")
            .args(["-create", "-output"])
            .arg(destination)
            .arg(x86_64)
            .arg(arm64),
        "`lipo` for macOS Universal export",
    )?;
    if output.stdout.len() > MAX_TOOL_OUTPUT_BYTES || output.stderr.len() > MAX_TOOL_OUTPUT_BYTES {
        return Err(format!(
            "`lipo` output exceeded the {} byte safety limit",
            MAX_TOOL_OUTPUT_BYTES
        ));
    }
    if !output.status.success() {
        return Err(format!(
            "`lipo` could not create the macOS Universal Rust module with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    ensure_regular_module(destination)
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
                "godot-rust-project-export-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&root).expect("temporary project");
            std::fs::write(root.join("project.godot"), b"[application]\n").expect("project.godot");
            Self(root)
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn linux_target(profile: crate::ExportProfile) -> ProjectExportTarget {
        ProjectExportTarget {
            operating_system: crate::ExportOperatingSystem::Linux,
            profile,
            artifacts: vec![ExportArtifactTarget {
                architecture: crate::ExportArchitecture::X86_64,
                rust_targets: vec!["x86_64-unknown-linux-gnu".to_owned()],
                module_file: "libgodot_rs_project_module.so".to_owned(),
                godot_tags: vec!["x86_64".to_owned()],
                staging_key: "linux-x86_64".to_owned(),
            }],
        }
    }

    #[test]
    fn validated_exports_use_profile_specific_atomic_paths() {
        let project = TempProject::new();
        let target = linux_target(crate::ExportProfile::Release);
        let first = publish_export_module_with(
            &project.0,
            &target,
            &target.artifacts[0],
            |candidate| {
                std::fs::write(candidate, b"first release module")
                    .map_err(|error| error.to_string())
            },
            |_| Ok(("validated".to_owned(), String::new())),
        )
        .expect("first export");
        let canonical_project = project.0.canonicalize().expect("canonical project");
        assert_eq!(
            first.module_path,
            canonical_project
                .join(".godot/rust/exports/linux-x86_64/release")
                .join("libgodot_rs_project_module.so")
        );
        assert_eq!(first.godot_tags, ["x86_64"]);
        assert_eq!(
            std::fs::read(&first.module_path).expect("published module"),
            b"first release module"
        );

        let second = publish_export_module_with(
            &project.0,
            &target,
            &target.artifacts[0],
            |candidate| {
                std::fs::write(candidate, b"second release module")
                    .map_err(|error| error.to_string())
            },
            |_| Ok(("validated again".to_owned(), String::new())),
        )
        .expect("replacement export");
        assert_ne!(first.build_id, second.build_id);
        assert_eq!(
            std::fs::read(&second.module_path).expect("replaced module"),
            b"second release module"
        );
    }

    #[test]
    fn validation_failure_never_replaces_a_previous_export() {
        let project = TempProject::new();
        let target = linux_target(crate::ExportProfile::Debug);
        let first = publish_export_module_with(
            &project.0,
            &target,
            &target.artifacts[0],
            |candidate| {
                std::fs::write(candidate, b"known good export").map_err(|error| error.to_string())
            },
            |_| Ok((String::new(), String::new())),
        )
        .expect("known good export");
        assert!(
            publish_export_module_with(
                &project.0,
                &target,
                &target.artifacts[0],
                |candidate| {
                    std::fs::write(candidate, b"rejected export").map_err(|error| error.to_string())
                },
                |_| Err("ABI descriptor rejected".to_owned()),
            )
            .expect_err("rejected export")
            .contains("ABI descriptor rejected")
        );
        assert_eq!(
            std::fs::read(first.module_path).expect("retained export"),
            b"known good export"
        );
    }

    #[test]
    fn native_descriptor_protocol_uses_one_project_resource_path() {
        let project = TempProject::new();
        assert_eq!(
            project_resource_path(&project.0, &project.0.join(crate::NATIVE_DESCRIPTOR_FILE))
                .expect("managed descriptor"),
            "res://godot_rs_project.gdextension"
        );
        assert!(
            project_resource_path(
                &project.0,
                &project.0.join("nested/godot_rs_project.gdextension")
            )
            .expect_err("nested descriptor")
            .contains("managed project path")
        );
        assert!(
            project_resource_path(
                &project.0,
                &project
                    .0
                    .parent()
                    .expect("parent")
                    .join("outside.gdextension")
            )
            .expect_err("outside descriptor")
            .contains("outside the Godot project")
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_export_directories_must_not_be_symbolic_links() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new();
        let target = linux_target(crate::ExportProfile::Release);
        let outside = project.0.join("outside");
        std::fs::create_dir(&outside).expect("outside directory");
        std::fs::create_dir(project.0.join(".godot")).expect(".godot");
        symlink(&outside, project.0.join(".godot/rust")).expect("managed directory link");
        let error = publish_export_module_with(
            &project.0,
            &target,
            &target.artifacts[0],
            |_| Ok(()),
            |_| Ok((String::new(), String::new())),
        )
        .expect_err("symbolic link");
        assert!(error.contains("must not be a symbolic link"));
    }
}
