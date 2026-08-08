use crate::{
    CargoTaskKind, GodotRustMode, NativeBuildReport, ProjectBuildReport, build_and_publish,
    inspect_cargo_project, run_native_build, run_package_cargo_task, sync_configured_script_index,
};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ProjectCargoOutcome {
    Script { report: ProjectBuildReport },
    Extension { report: NativeBuildReport },
}

pub fn run_project_cargo_task(
    project_root: impl AsRef<Path>,
    kind: CargoTaskKind,
    cargo: Option<impl AsRef<OsStr>>,
    validator: Option<impl AsRef<OsStr>>,
) -> Result<ProjectCargoOutcome, String> {
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let project = inspect_cargo_project(project_root.as_ref(), Some(&cargo))?;
    let native_configured = project.packages.iter().any(|package| {
        package.godot_rust_enabled && package.godot_rust_mode == Some(GodotRustMode::Extension)
    });
    if native_configured {
        return run_native_build(project_root, kind, Some(&cargo))
            .map(|report| ProjectCargoOutcome::Extension { report });
    }

    if kind == CargoTaskKind::Build {
        let validator = validator.ok_or_else(|| {
            "Script Mode Build requires the project-module validator path".to_owned()
        })?;
        return build_and_publish(project_root, Some(&cargo), validator)
            .map(|report| ProjectCargoOutcome::Script { report });
    }

    let package = project.selected_package.clone().ok_or_else(|| {
        if project.issues.is_empty() {
            "Cargo project does not identify a Script Mode package".to_owned()
        } else {
            format!(
                "Cargo project does not identify a Script Mode package: {}",
                project.issues.join("; ")
            )
        }
    })?;
    let script_index = sync_configured_script_index(project_root.as_ref(), &package)?;
    let cargo_report = run_package_cargo_task(project_root, kind, &package, Some(&cargo))?;
    Ok(ProjectCargoOutcome::Script {
        report: ProjectBuildReport {
            project,
            package,
            script_index,
            cargo: cargo_report,
            publication: None,
            receipt: None,
            receipt_issue: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_mode_names_match_cargo_metadata_values() {
        let script = serde_json::to_value(ProjectCargoOutcome::Script {
            report: dummy_report(),
        })
        .expect("Script outcome");
        assert_eq!(script["mode"], "script");
    }

    fn dummy_report() -> ProjectBuildReport {
        use crate::{
            CargoPackageModel, CargoProjectModel, CargoTargetModel, PackageSelectionReason,
        };
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let target = CargoTargetModel {
            name: "game".to_owned(),
            src_path: PathBuf::from("/game/src/lib.rs"),
        };
        let package = CargoPackageModel {
            id: "game 0.1.0".to_owned(),
            name: "game".to_owned(),
            manifest_path: PathBuf::from("/game/Cargo.toml"),
            workspace_default: true,
            godot_rs_dependency: true,
            godot_rust_enabled: true,
            godot_rust_mode: Some(GodotRustMode::Script),
            godot_api: None,
            scripts_path: Some(PathBuf::from("/game/src/scripts")),
            editor: crate::EditorWorkflowConfig::default(),
            script_mode_configured: true,
            configuration_issues: Vec::new(),
            cdylib_targets: vec![target],
        };
        ProjectBuildReport {
            project: CargoProjectModel {
                project_root: PathBuf::from("/game"),
                workspace_root: PathBuf::from("/game"),
                target_directory: PathBuf::from("/game/target"),
                packages: vec![package.clone()],
                selected_package: Some(package.clone()),
                selection_reason: Some(PackageSelectionReason::RootPackage),
                native_package: None,
                issues: Vec::new(),
                command: Vec::new(),
            },
            package,
            script_index: None,
            cargo: crate::CargoTaskReport {
                kind: CargoTaskKind::Check,
                root: PathBuf::from("/game"),
                success: true,
                exit_code: Some(0),
                command: Vec::new(),
                environment: BTreeMap::new(),
                diagnostics: Vec::new(),
                artifacts: Vec::new(),
                messages: Vec::new(),
                stderr: String::new(),
            },
            publication: None,
            receipt: None,
            receipt_issue: None,
        }
    }
}
