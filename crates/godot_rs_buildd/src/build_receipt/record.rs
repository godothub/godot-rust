use super::candidates::collect_receipt_candidates;
use super::dep_info::read_artifact_dep_info;
use super::evidence::snapshot_build_environment;
use super::format::{
    BUILD_RECEIPT_FILE, BuildReceiptPayload, BuildReceiptSummary, StoredBuildReceipt, write_receipt,
};
use super::inputs::{
    collect_script_inventory, snapshot_paths, verify_script_inventory, verify_snapshots,
};
use crate::{
    CargoPackageModel, CargoProjectModel, PublishedGeneration, probe_toolchain,
    verify_last_known_good,
};
use std::ffi::OsStr;
use std::path::Path;

pub fn record_build_receipt(
    project_root: &Path,
    project: &CargoProjectModel,
    package: &CargoPackageModel,
    artifact: &Path,
    publication: &PublishedGeneration,
    cargo: &OsStr,
    validator: &OsStr,
) -> Result<BuildReceiptSummary, String> {
    let project_root = project_root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}`: {error}",
            project_root.display()
        )
    })?;
    let cargo_command = cargo
        .to_str()
        .ok_or_else(|| "Cargo command is not UTF-8; Build Receipt is unavailable".to_owned())?
        .to_owned();
    let validator_path = Path::new(validator).canonicalize().map_err(|error| {
        format!(
            "could not resolve project-module validator `{}`: {error}",
            Path::new(validator).display()
        )
    })?;
    let build_service_path = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("could not resolve the running build service: {error}"))?;
    let verified_publication = verify_last_known_good(&project_root)?;
    if verified_publication.build_id != publication.build_id
        || verified_publication.module_path != publication.module_path
    {
        return Err("published project module changed before Receipt creation".to_owned());
    }

    let dep_info = read_artifact_dep_info(artifact, &project_root)?;
    let candidates = collect_receipt_candidates(
        project,
        &dep_info.dependencies,
        dep_info.path,
        publication,
        validator_path.clone(),
        build_service_path.clone(),
    )?;
    let mut candidate_paths = candidates.paths;
    candidate_paths.insert(package.manifest_path.clone());
    let tracked_inputs = snapshot_paths(candidate_paths)?;
    let script_inventory = package
        .scripts_path
        .as_deref()
        .map(collect_script_inventory)
        .transpose()?;
    let [target] = package.cdylib_targets.as_slice() else {
        return Err(format!(
            "Script Mode package `{}` must have exactly one cdylib target",
            package.name
        ));
    };
    let payload = BuildReceiptPayload {
        project_root: project_root.clone(),
        package_id: package.id.clone(),
        package_name: package.name.clone(),
        manifest_path: package.manifest_path.clone(),
        target_name: target.name.clone(),
        cargo_command,
        validator_path,
        build_service_path,
        build_id: publication.build_id.clone(),
        module_path: publication.module_path.clone(),
        tracked_inputs,
        script_inventory,
        environment: snapshot_build_environment(Some(artifact))?,
        toolchain: probe_toolchain(&project_root, Some(cargo))?,
    };
    let receipt = StoredBuildReceipt::new(payload)?;

    if let Some(path) = verify_snapshots(&receipt.payload.tracked_inputs)? {
        return Err(format!(
            "Build Receipt input changed while the Receipt was being created: {}",
            path.display()
        ));
    }
    if let Some(inventory) = &receipt.payload.script_inventory {
        if let Some(path) = verify_script_inventory(inventory)? {
            return Err(format!(
                "Rust script inventory changed while the Build Receipt was being created: {}",
                path.display()
            ));
        }
    }
    let verified_publication = verify_last_known_good(&project_root)?;
    if verified_publication.build_id != receipt.payload.build_id
        || verified_publication.module_path != receipt.payload.module_path
    {
        return Err(
            "published project module changed while the Receipt was being created".to_owned(),
        );
    }

    let receipt_path = project_root.join(BUILD_RECEIPT_FILE);
    write_receipt(&receipt_path, &receipt)?;
    receipt.summary(receipt_path)
}
