use crate::{
    BuildReceiptSummary, CargoPackageModel, CargoProjectModel, CargoTaskKind, CargoTaskReport,
    PublishedGeneration, ScriptIndexReport, inspect_cargo_project, record_build_receipt,
    run_package_cargo_task, sync_configured_script_index,
};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct ProjectBuildReport {
    pub project: CargoProjectModel,
    pub package: CargoPackageModel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_index: Option<ScriptIndexReport>,
    pub cargo: CargoTaskReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<PublishedGeneration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<BuildReceiptSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_issue: Option<String>,
}

pub fn build_and_publish(
    project_root: impl AsRef<Path>,
    cargo: Option<impl AsRef<OsStr>>,
    validator: impl AsRef<OsStr>,
) -> Result<ProjectBuildReport, String> {
    let project_root = project_root.as_ref();
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let validator = validator.as_ref().to_owned();
    let project = inspect_cargo_project(project_root, Some(&cargo))?;
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
    let script_index = sync_configured_script_index(project_root, &package)?;
    let cargo_report =
        run_package_cargo_task(project_root, CargoTaskKind::Build, &package, Some(&cargo))?;
    if !cargo_report.success {
        return Ok(ProjectBuildReport {
            project,
            package,
            script_index,
            cargo: cargo_report,
            publication: None,
            receipt: None,
            receipt_issue: None,
        });
    }

    let artifact = exact_project_artifact(&package, &cargo_report)?;
    let dep_info = crate::build_receipt::read_artifact_dep_info(artifact, project_root)?;
    let candidates = crate::build_receipt::collect_build_candidates(
        &project,
        &dep_info.dependencies,
        dep_info.path,
    )?;
    let stable_inputs = crate::build_receipt::snapshot_paths(candidates.paths)?;

    let confirmed_report =
        run_package_cargo_task(project_root, CargoTaskKind::Build, &package, Some(&cargo))?;
    if !confirmed_report.success {
        return Ok(ProjectBuildReport {
            project,
            package,
            script_index,
            cargo: confirmed_report,
            publication: None,
            receipt: None,
            receipt_issue: None,
        });
    }
    if let Some(path) = crate::build_receipt::verify_snapshots(&stable_inputs)? {
        return Err(format!(
            "Rust build input changed while Cargo was confirming the project module: {}. The \
             candidate was not published; build again after edits finish",
            path.display()
        ));
    }
    let artifact = exact_project_artifact(&package, &confirmed_report)?;
    let publication = crate::publication::publish_validated_generation_guarded(
        project_root,
        artifact,
        &validator,
        || match crate::build_receipt::verify_snapshots(&stable_inputs)? {
            Some(path) => Err(format!(
                "Rust build input changed while the project module was being validated: {}. The \
                 candidate was not activated; build again after edits finish",
                path.display()
            )),
            None => Ok(()),
        },
    )
    .map(Some)?;
    let (receipt, receipt_issue) = match record_build_receipt(
        project_root,
        &project,
        &package,
        artifact,
        publication
            .as_ref()
            .expect("successful publication is present"),
        &cargo,
        &validator,
    ) {
        Ok(receipt) => (Some(receipt), None),
        Err(error) => (None, Some(error)),
    };
    Ok(ProjectBuildReport {
        project,
        package,
        script_index,
        cargo: confirmed_report,
        publication,
        receipt,
        receipt_issue,
    })
}

fn exact_project_artifact<'a>(
    package: &CargoPackageModel,
    cargo_report: &'a CargoTaskReport,
) -> Result<&'a Path, String> {
    let artifact = match cargo_report.artifacts.as_slice() {
        [artifact] => artifact,
        [] => {
            return Err(format!(
                "Cargo built `{}` successfully but reported no dynamic library artifact",
                package.name
            ));
        }
        artifacts => {
            return Err(format!(
                "Cargo built `{}` but reported {} dynamic library artifacts; exactly one is required",
                package.name,
                artifacts.len()
            ));
        }
    };
    Ok(artifact)
}
