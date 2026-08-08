use crate::{
    CargoPackageModel, CargoProjectModel, CargoTargetModel, CargoTaskKind, CargoTaskReport,
    GodotRustMode, NativePlatform, PublishedNativeExtension, inspect_cargo_project,
    publish_native_extension, run_native_package_cargo_task,
};
use godot_api::{GODOT_API_ENV, GodotApiVersion, NATIVE_ENTRY_SYMBOL};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NativeBuildPlan {
    pub project_root: PathBuf,
    pub package: CargoPackageModel,
    pub target: CargoTargetModel,
    pub godot_api: GodotApiVersion,
    pub cargo_environment: BTreeMap<String, String>,
    pub entry_symbol: String,
    pub compatibility_minimum: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeProjectPlan {
    pub project: CargoProjectModel,
    pub native: NativeBuildPlan,
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeBuildReport {
    pub project: CargoProjectModel,
    pub native: NativeBuildPlan,
    pub cargo: CargoTaskReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<PublishedNativeExtension>,
}

pub fn plan_native_build(project: &CargoProjectModel) -> Result<NativeBuildPlan, String> {
    let package = project.native_package.clone().ok_or_else(|| {
        let prefix = "Cargo project does not identify exactly one valid Extension Mode package";
        if project.issues.is_empty() {
            prefix.to_owned()
        } else {
            format!("{prefix}: {}", project.issues.join("; "))
        }
    })?;
    if package.godot_rust_mode != Some(GodotRustMode::Extension) {
        return Err(format!(
            "Cargo package `{}` is not configured with mode = \"extension\"",
            package.name
        ));
    }
    let godot_api = package.godot_api.ok_or_else(|| {
        format!(
            "Cargo package `{}` has no validated Godot API target",
            package.name
        )
    })?;
    let [target] = package.cdylib_targets.as_slice() else {
        return Err(format!(
            "Native package `{}` must have exactly one cdylib target",
            package.name
        ));
    };
    let target = target.clone();
    let compatibility_minimum = godot_api.to_string();
    Ok(NativeBuildPlan {
        project_root: project.project_root.clone(),
        package,
        target,
        godot_api,
        cargo_environment: BTreeMap::from([(
            GODOT_API_ENV.to_owned(),
            compatibility_minimum.clone(),
        )]),
        entry_symbol: NATIVE_ENTRY_SYMBOL.to_owned(),
        compatibility_minimum,
    })
}

pub fn inspect_native_build_plan(
    project_root: impl AsRef<Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<NativeProjectPlan, String> {
    let project = inspect_cargo_project(project_root, cargo)?;
    let native = plan_native_build(&project)?;
    Ok(NativeProjectPlan { project, native })
}

pub fn run_native_build(
    project_root: impl AsRef<Path>,
    kind: CargoTaskKind,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<NativeBuildReport, String> {
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let NativeProjectPlan { project, native } =
        inspect_native_build_plan(project_root.as_ref(), Some(&cargo))?;
    let cargo_report = run_native_package_cargo_task(
        project_root.as_ref(),
        kind,
        &native.package,
        native.godot_api,
        Some(&cargo),
    )?;
    debug_assert_eq!(cargo_report.environment, native.cargo_environment);
    let publication = if kind == CargoTaskKind::Build && cargo_report.success {
        let artifact = match cargo_report.artifacts.as_slice() {
            [artifact] => artifact,
            [] => {
                return Err(format!(
                    "Cargo built Native package `{}` successfully but reported no dynamic library artifact",
                    native.package.name
                ));
            }
            artifacts => {
                return Err(format!(
                    "Cargo built Native package `{}` but reported {} dynamic library artifacts; exactly one is required",
                    native.package.name,
                    artifacts.len()
                ));
            }
        };
        Some(publish_native_extension(
            project_root.as_ref(),
            &native,
            artifact,
            NativePlatform::current_debug()?,
        )?)
    } else {
        None
    };
    Ok(NativeBuildReport {
        project,
        native,
        cargo: cargo_report,
        publication,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackageSelectionReason;

    fn native_project(package: CargoPackageModel) -> CargoProjectModel {
        CargoProjectModel {
            project_root: PathBuf::from("/game"),
            workspace_root: PathBuf::from("/game"),
            target_directory: PathBuf::from("/game/target"),
            packages: vec![package.clone()],
            selected_package: None,
            selection_reason: None,
            native_package: Some(package),
            issues: Vec::new(),
            command: vec!["cargo".to_owned(), "metadata".to_owned()],
        }
    }

    fn package() -> CargoPackageModel {
        CargoPackageModel {
            id: "godothub_project 0.1.0".to_owned(),
            name: "godothub_project".to_owned(),
            manifest_path: PathBuf::from("/game/Cargo.toml"),
            workspace_default: true,
            godot_rs_dependency: true,
            godot_rust_enabled: true,
            godot_rust_mode: Some(GodotRustMode::Extension),
            godot_api: Some(GodotApiVersion::new(4, 6)),
            scripts_path: None,
            editor: crate::EditorWorkflowConfig::default(),
            script_mode_configured: false,
            configuration_issues: Vec::new(),
            cdylib_targets: vec![CargoTargetModel {
                name: "godothub_project".to_owned(),
                src_path: PathBuf::from("/game/src/lib.rs"),
            }],
        }
    }

    #[test]
    fn native_plan_derives_the_private_cargo_input_and_descriptor_contract() {
        let plan = plan_native_build(&native_project(package())).expect("native plan");
        assert_eq!(plan.godot_api, GodotApiVersion::new(4, 6));
        assert_eq!(
            plan.cargo_environment
                .get(GODOT_API_ENV)
                .map(String::as_str),
            Some("4.6")
        );
        assert_eq!(plan.entry_symbol, "godot_rs_native_init");
        assert_eq!(plan.compatibility_minimum, "4.6");
        assert_eq!(plan.target.name, "godothub_project");
    }

    #[test]
    fn native_plan_never_falls_back_to_a_script_selection() {
        let mut project = native_project(package());
        project.native_package = None;
        project.selected_package = Some(project.packages[0].clone());
        project.selection_reason = Some(PackageSelectionReason::RootPackage);
        let error = plan_native_build(&project).expect_err("native package required");
        assert!(error.contains("exactly one valid Extension Mode package"));
    }
}
