use super::evidence::resnapshot_build_environment;
use super::format::{
    BUILD_RECEIPT_FILE, BuildReceiptReason, BuildReceiptStatus, read_receipt, receipt_is_missing,
    stale,
};
use super::inputs::{verify_script_inventory, verify_snapshots};
use crate::{probe_toolchain, verify_last_known_good};
use std::ffi::OsStr;
use std::path::Path;

pub fn check_build_receipt(
    project_root: &Path,
    cargo: &OsStr,
    validator: &OsStr,
) -> Result<BuildReceiptStatus, String> {
    let project_root = project_root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}`: {error}",
            project_root.display()
        )
    })?;
    let receipt_path = project_root.join(BUILD_RECEIPT_FILE);
    if receipt_is_missing(&receipt_path)? {
        return Ok(stale(
            BuildReceiptReason::Missing,
            "no successful Script Mode build has recorded a Build Receipt",
            None,
        ));
    }
    let receipt = match read_receipt(&receipt_path) {
        Ok(receipt) => receipt,
        Err(error) => {
            return Ok(stale(BuildReceiptReason::Invalid, error, None));
        }
    };
    if let Err(error) = receipt.validate() {
        return Ok(stale(BuildReceiptReason::Invalid, error, None));
    }
    let summary = match receipt.summary(receipt_path) {
        Ok(summary) => Some(summary),
        Err(error) => return Ok(stale(BuildReceiptReason::Invalid, error, None)),
    };
    let payload = &receipt.payload;
    if payload.project_root != project_root {
        return Ok(stale(
            BuildReceiptReason::ProjectChanged,
            "Build Receipt belongs to a different project root",
            summary,
        ));
    }
    let Some(cargo_command) = cargo.to_str() else {
        return Ok(stale(
            BuildReceiptReason::CargoCommandChanged,
            "Cargo command is not UTF-8",
            summary,
        ));
    };
    if payload.cargo_command != cargo_command {
        return Ok(stale(
            BuildReceiptReason::CargoCommandChanged,
            "Cargo command differs from the successful build",
            summary,
        ));
    }
    let validator_path = match Path::new(validator).canonicalize() {
        Ok(path) => path,
        Err(error) => {
            return Ok(stale(
                BuildReceiptReason::ValidatorChanged,
                format!("could not resolve the current project-module validator: {error}"),
                summary,
            ));
        }
    };
    if payload.validator_path != validator_path {
        return Ok(stale(
            BuildReceiptReason::ValidatorChanged,
            "project-module validator path differs from the successful build",
            summary,
        ));
    }
    let build_service_path = match std::env::current_exe().and_then(|path| path.canonicalize()) {
        Ok(path) => path,
        Err(error) => {
            return Ok(stale(
                BuildReceiptReason::BuildServiceChanged,
                format!("could not resolve the running build service: {error}"),
                summary,
            ));
        }
    };
    if payload.build_service_path != build_service_path {
        return Ok(stale(
            BuildReceiptReason::BuildServiceChanged,
            "build service path differs from the successful build",
            summary,
        ));
    }
    match resnapshot_build_environment(&payload.environment) {
        Ok(environment) if environment == payload.environment => {}
        Ok(_) => {
            return Ok(stale(
                BuildReceiptReason::EnvironmentChanged,
                "process environment differs from the successful build",
                summary,
            ));
        }
        Err(error) => {
            return Ok(stale(
                BuildReceiptReason::EnvironmentChanged,
                error,
                summary,
            ));
        }
    }
    match probe_toolchain(&project_root, Some(cargo)) {
        Ok(toolchain) if toolchain == payload.toolchain => {}
        Ok(_) => {
            return Ok(stale(
                BuildReceiptReason::ToolchainChanged,
                "Cargo or rustc version differs from the successful build",
                summary,
            ));
        }
        Err(error) => {
            return Ok(stale(BuildReceiptReason::ToolchainChanged, error, summary));
        }
    }
    match verify_snapshots(&payload.tracked_inputs) {
        Ok(None) => {}
        Ok(Some(path)) => {
            return Ok(stale(
                BuildReceiptReason::InputChanged,
                format!("Build Receipt input changed: {}", path.display()),
                summary,
            ));
        }
        Err(error) => {
            return Ok(stale(BuildReceiptReason::InputChanged, error, summary));
        }
    }
    if let Some(inventory) = &payload.script_inventory {
        match verify_script_inventory(inventory) {
            Ok(None) => {}
            Ok(Some(path)) => {
                return Ok(stale(
                    BuildReceiptReason::ScriptInventoryChanged,
                    format!("Rust script inventory changed: {}", path.display()),
                    summary,
                ));
            }
            Err(error) => {
                return Ok(stale(
                    BuildReceiptReason::ScriptInventoryChanged,
                    error,
                    summary,
                ));
            }
        }
    }
    match verify_last_known_good(&project_root) {
        Ok(publication)
            if publication.build_id == payload.build_id
                && publication.module_path == payload.module_path => {}
        Ok(_) => {
            return Ok(stale(
                BuildReceiptReason::PublicationChanged,
                "Last Known Good differs from the successful build",
                summary,
            ));
        }
        Err(error) => {
            return Ok(stale(
                BuildReceiptReason::PublicationChanged,
                error,
                summary,
            ));
        }
    }
    Ok(BuildReceiptStatus {
        fresh: true,
        reason: BuildReceiptReason::Verified,
        detail: "all recorded build inputs and the published module are unchanged".to_owned(),
        receipt: summary,
    })
}
