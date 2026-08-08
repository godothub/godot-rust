use super::evidence::EnvironmentSnapshot;
use super::inputs::{ScriptInventory, TrackedInput};
use crate::ToolchainReport;
use crate::managed_fs::atomic_write;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};

pub const BUILD_RECEIPT_FILE: &str = ".godot/rust/build-receipt.json";

const BUILD_RECEIPT_FORMAT: u32 = 2;
const MAX_BUILD_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildReceiptSummary {
    pub receipt_id: String,
    pub build_id: String,
    pub input_count: usize,
    pub receipt_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildReceiptReason {
    Verified,
    Missing,
    Invalid,
    ProjectChanged,
    CargoCommandChanged,
    ValidatorChanged,
    BuildServiceChanged,
    EnvironmentChanged,
    ToolchainChanged,
    InputChanged,
    ScriptInventoryChanged,
    PublicationChanged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildReceiptStatus {
    pub fresh: bool,
    pub reason: BuildReceiptReason,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<BuildReceiptSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct BuildReceiptPayload {
    pub project_root: PathBuf,
    pub package_id: String,
    pub package_name: String,
    pub manifest_path: PathBuf,
    pub target_name: String,
    pub cargo_command: String,
    pub validator_path: PathBuf,
    pub build_service_path: PathBuf,
    pub build_id: String,
    pub module_path: PathBuf,
    pub tracked_inputs: Vec<TrackedInput>,
    pub script_inventory: Option<ScriptInventory>,
    pub environment: EnvironmentSnapshot,
    pub toolchain: ToolchainReport,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct StoredBuildReceipt {
    pub format: u32,
    pub receipt_id: String,
    pub payload: BuildReceiptPayload,
}

impl StoredBuildReceipt {
    pub fn new(payload: BuildReceiptPayload) -> Result<Self, String> {
        Ok(Self {
            format: BUILD_RECEIPT_FORMAT,
            receipt_id: payload_id(&payload)?,
            payload,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != BUILD_RECEIPT_FORMAT {
            return Err(format!(
                "Build Receipt format {} is unsupported",
                self.format
            ));
        }
        let expected = payload_id(&self.payload)?;
        if self.receipt_id != expected {
            return Err("Build Receipt content does not match its Receipt ID".to_owned());
        }
        Ok(())
    }

    pub fn summary(&self, receipt_path: PathBuf) -> Result<BuildReceiptSummary, String> {
        let script_count = self
            .payload
            .script_inventory
            .as_ref()
            .map_or(0, |inventory| inventory.entries.len());
        let input_count = self
            .payload
            .tracked_inputs
            .len()
            .checked_add(script_count)
            .ok_or_else(|| "Build Receipt input count overflowed usize".to_owned())?;
        Ok(BuildReceiptSummary {
            receipt_id: self.receipt_id.clone(),
            build_id: self.payload.build_id.clone(),
            input_count,
            receipt_path,
        })
    }
}

pub(super) fn write_receipt(
    receipt_path: &Path,
    receipt: &StoredBuildReceipt,
) -> Result<(), String> {
    let mut encoded = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("could not encode Build Receipt: {error}"))?;
    encoded.push(b'\n');
    if encoded.len() as u64 > MAX_BUILD_RECEIPT_BYTES {
        return Err(format!(
            "Build Receipt exceeds the {MAX_BUILD_RECEIPT_BYTES} byte safety limit"
        ));
    }
    atomic_write(receipt_path, &encoded, "Build Receipt")
}

pub(super) fn read_receipt(receipt_path: &Path) -> Result<StoredBuildReceipt, String> {
    let metadata = std::fs::symlink_metadata(receipt_path).map_err(|error| {
        format!(
            "could not inspect Build Receipt `{}`: {error}",
            receipt_path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Build Receipt is not a regular file: {}",
            receipt_path.display()
        ));
    }
    if metadata.len() > MAX_BUILD_RECEIPT_BYTES {
        return Err(format!(
            "Build Receipt exceeds the {MAX_BUILD_RECEIPT_BYTES} byte safety limit"
        ));
    }
    let source = std::fs::read(receipt_path).map_err(|error| {
        format!(
            "could not read Build Receipt `{}`: {error}",
            receipt_path.display()
        )
    })?;
    serde_json::from_slice(&source)
        .map_err(|error| format!("Build Receipt contains invalid JSON: {error}"))
}

pub(super) fn receipt_is_missing(receipt_path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(receipt_path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!(
            "could not inspect Build Receipt `{}`: {error}",
            receipt_path.display()
        )),
    }
}

fn payload_id(payload: &BuildReceiptPayload) -> Result<String, String> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|error| format!("could not encode Build Receipt payload: {error}"))?;
    if encoded.len() as u64 > MAX_BUILD_RECEIPT_BYTES {
        return Err(format!(
            "Build Receipt payload exceeds the {MAX_BUILD_RECEIPT_BYTES} byte safety limit"
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(&encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub(super) fn stale(
    reason: BuildReceiptReason,
    detail: impl Into<String>,
    receipt: Option<BuildReceiptSummary>,
) -> BuildReceiptStatus {
    BuildReceiptStatus {
        fresh: false,
        reason,
        detail: detail.into(),
        receipt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_id_detects_payload_tampering() {
        let mut receipt = StoredBuildReceipt::new(sample_payload()).expect("Build Receipt");
        receipt.payload.package_name = "changed".to_owned();
        assert!(
            receipt
                .validate()
                .expect_err("tampering must fail")
                .contains("Receipt ID")
        );
    }

    fn sample_payload() -> BuildReceiptPayload {
        BuildReceiptPayload {
            project_root: PathBuf::from("/project"),
            package_id: "game 0.1.0".to_owned(),
            package_name: "game".to_owned(),
            manifest_path: PathBuf::from("/project/Cargo.toml"),
            target_name: "game".to_owned(),
            cargo_command: "cargo".to_owned(),
            validator_path: PathBuf::from("/plugin/module-check"),
            build_service_path: PathBuf::from("/plugin/buildd"),
            build_id: format!("sha256:{}", "a".repeat(64)),
            module_path: PathBuf::from("/project/.godot/rust/builds/a/project_module.so"),
            tracked_inputs: Vec::new(),
            script_inventory: None,
            environment: EnvironmentSnapshot {
                variable_count: 1,
                variable_names: vec!["PATH".to_owned()],
                sha256: "b".repeat(64),
            },
            toolchain: ToolchainReport {
                project_root: PathBuf::from("/project"),
                cargo_command: "cargo".to_owned(),
                cargo_version_verbose: "cargo 1.85".to_owned(),
                rustc_version_verbose: "rustc 1.85".to_owned(),
            },
        }
    }
}
