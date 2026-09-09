use super::check_build_receipt;
use super::evidence::snapshot_build_environment;
use super::format::{
    BUILD_RECEIPT_FILE, BuildReceiptPayload, BuildReceiptReason, StoredBuildReceipt, read_receipt,
    write_receipt,
};
use super::inputs::{collect_script_inventory, snapshot_paths};
use crate::probe_toolchain;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct ReceiptProject {
    root: PathBuf,
    source: PathBuf,
    manifest: PathBuf,
    lockfile: PathBuf,
    cargo_config: PathBuf,
    scripts: PathBuf,
    module: PathBuf,
    validator: PathBuf,
    receipt_path: PathBuf,
}

impl ReceiptProject {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "godot-rust-build-receipt-project-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("temporary project");
        let root = root.canonicalize().expect("canonical temporary project");
        std::fs::write(root.join("project.godot"), b"[application]\n").expect("Godot project");
        let manifest = root.join("Cargo.toml");
        let lockfile = root.join("Cargo.lock");
        let cargo_config = root.join(".cargo/config.toml");
        std::fs::write(
            &manifest,
            b"[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
        )
        .expect("Cargo manifest");
        let scripts = root.join("src/scripts");
        std::fs::create_dir_all(&scripts).expect("script directory");
        let source = scripts.join("player.rs");
        std::fs::write(&source, b"pub struct Player;\n").expect("Rust source");
        std::fs::write(scripts.join("player.rs.uid"), b"uid://player\n").expect("script UID");

        let module_source = b"verified project module";
        let hash = format!("{:x}", Sha256::digest(module_source));
        let build_id = format!("sha256:{hash}");
        let generation = root.join(".godot/rust/builds").join(&hash);
        std::fs::create_dir_all(&generation).expect("generation directory");
        let module = generation.join("project_module.so");
        std::fs::write(&module, module_source).expect("project module");
        std::fs::write(
            generation.join("generation.json"),
            serde_json::to_vec_pretty(&json!({
                "format": 1,
                "build_id": build_id,
                "module_file": "project_module.so",
                "byte_len": module_source.len()
            }))
            .expect("generation JSON"),
        )
        .expect("generation manifest");
        let last_known_good = root.join(".godot/rust/last-known-good.json");
        std::fs::write(
            &last_known_good,
            serde_json::to_vec_pretty(&json!({
                "format": 1,
                "build_id": build_id,
                "module_path": PathBuf::from(".godot/rust/builds")
                    .join(&hash)
                    .join("project_module.so")
            }))
            .expect("Last Known Good JSON"),
        )
        .expect("Last Known Good");
        let validator = root.join("module-check");
        std::fs::write(&validator, b"validator identity").expect("validator");
        let build_service_path = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .expect("current test executable");
        let tracked_inputs = snapshot_paths([
            source.clone(),
            manifest.clone(),
            lockfile.clone(),
            cargo_config.clone(),
            module.clone(),
            generation.join("generation.json"),
            last_known_good,
            validator.clone(),
        ])
        .expect("tracked inputs");
        let receipt = StoredBuildReceipt::new(BuildReceiptPayload {
            project_root: root.clone(),
            package_id: "game 0.1.0".to_owned(),
            package_name: "game".to_owned(),
            manifest_path: manifest.clone(),
            target_name: "game".to_owned(),
            cargo_command: "cargo".to_owned(),
            validator_path: validator.clone(),
            build_service_path,
            build_id,
            module_path: module.clone(),
            tracked_inputs,
            script_inventory: Some(collect_script_inventory(&scripts).expect("script inventory")),
            environment: snapshot_build_environment(None).expect("environment snapshot"),
            toolchain: probe_toolchain(&root, Some(OsStr::new("cargo"))).expect("toolchain report"),
        })
        .expect("Build Receipt");
        let receipt_path = root.join(BUILD_RECEIPT_FILE);
        write_receipt(&receipt_path, &receipt).expect("stored Build Receipt");
        Self {
            root,
            source,
            manifest,
            lockfile,
            cargo_config,
            scripts,
            module,
            validator,
            receipt_path,
        }
    }

    fn check(&self) -> super::BuildReceiptStatus {
        check_build_receipt(&self.root, OsStr::new("cargo"), self.validator.as_os_str())
            .expect("Build Receipt query")
    }
}

impl Drop for ReceiptProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn unchanged_receipt_is_verified() {
    let project = ReceiptProject::new();
    let status = project.check();
    assert!(status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::Verified);
    assert!(status.receipt.is_some());
}

#[test]
fn source_manifest_lockfile_and_config_changes_are_stale() {
    for input in 0..4 {
        let project = ReceiptProject::new();
        let (path, create_parent) = match input {
            0 => (project.source.clone(), false),
            1 => (project.manifest.clone(), false),
            2 => (project.lockfile.clone(), false),
            3 => (project.cargo_config.clone(), true),
            _ => unreachable!("fixed test input range"),
        };
        if create_parent {
            std::fs::create_dir_all(path.parent().expect("configuration parent"))
                .expect("configuration directory");
        }
        std::fs::write(&path, b"changed\n").expect("changed input");
        let status = project.check();
        assert!(!status.fresh, "{} should be stale", path.display());
        assert_eq!(status.reason, BuildReceiptReason::InputChanged);
    }
}

#[test]
fn new_script_and_uid_changes_are_stale() {
    let project = ReceiptProject::new();
    std::fs::write(project.scripts.join("enemy.rs"), b"pub struct Enemy;\n")
        .expect("new Rust script");
    let status = project.check();
    assert!(!status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::ScriptInventoryChanged);

    let project = ReceiptProject::new();
    std::fs::write(project.scripts.join("player.rs.uid"), b"uid://different\n")
        .expect("changed script UID");
    let status = project.check();
    assert!(!status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::ScriptInventoryChanged);
}

#[test]
fn receipt_and_published_module_tampering_are_stale() {
    let project = ReceiptProject::new();
    let mut source: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&project.receipt_path).expect("Build Receipt source"),
    )
    .expect("Build Receipt JSON");
    source["payload"]["package_name"] = json!("tampered");
    std::fs::write(
        &project.receipt_path,
        serde_json::to_vec_pretty(&source).expect("tampered Receipt JSON"),
    )
    .expect("tampered Build Receipt");
    let status = project.check();
    assert!(!status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::Invalid);

    let project = ReceiptProject::new();
    std::fs::write(&project.module, b"tampered module").expect("tampered module");
    let status = project.check();
    assert!(!status.fresh);
    assert!(matches!(
        status.reason,
        BuildReceiptReason::InputChanged | BuildReceiptReason::PublicationChanged
    ));
}

#[test]
fn missing_receipt_and_changed_toolchain_evidence_are_stale() {
    let project = ReceiptProject::new();
    std::fs::remove_file(&project.receipt_path).expect("removed Build Receipt");
    let status = project.check();
    assert!(!status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::Missing);

    let project = ReceiptProject::new();
    let receipt = read_receipt(&project.receipt_path).expect("Build Receipt");
    let mut payload = receipt.payload;
    payload
        .toolchain
        .rustc_version_verbose
        .push_str("-different");
    let changed = StoredBuildReceipt::new(payload).expect("changed toolchain Receipt");
    write_receipt(&project.receipt_path, &changed).expect("changed Build Receipt");
    let status = project.check();
    assert!(!status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::ToolchainChanged);
}

#[test]
fn a_different_validator_cannot_reuse_the_receipt() {
    let project = ReceiptProject::new();
    let other_validator = project.root.join("other-module-check");
    std::fs::write(&other_validator, b"different validator").expect("other validator");
    let status = check_build_receipt(
        &project.root,
        OsStr::new("cargo"),
        other_validator.as_os_str(),
    )
    .expect("Build Receipt query");
    assert!(!status.fresh);
    assert_eq!(status.reason, BuildReceiptReason::ValidatorChanged);
}

#[test]
fn receipt_cannot_resnapshot_edits_made_after_the_build() {
    use crate::{
        CargoPackageModel, CargoProjectModel, CargoTargetModel, EditorWorkflowConfig,
        GodotRustMode, PublishedGeneration,
    };

    let project = ReceiptProject::new();
    let original_receipt = std::fs::read(&project.receipt_path).expect("original receipt");
    let stable_inputs = snapshot_paths([project.source.clone()]).expect("build input snapshot");
    let previous = read_receipt(&project.receipt_path).expect("previous receipt");
    let package = CargoPackageModel {
        id: previous.payload.package_id,
        name: previous.payload.package_name,
        manifest_path: project.manifest.clone(),
        workspace_default: true,
        godot_rs_dependency: true,
        godot_rust_enabled: true,
        godot_rust_mode: Some(GodotRustMode::Script),
        godot_api: None,
        scripts_path: Some(project.scripts.clone()),
        editor: EditorWorkflowConfig::default(),
        script_mode_configured: true,
        configuration_issues: Vec::new(),
        cdylib_targets: vec![CargoTargetModel {
            name: previous.payload.target_name,
            src_path: project.source.clone(),
        }],
    };
    let model = CargoProjectModel {
        project_root: project.root.clone(),
        workspace_root: project.root.clone(),
        target_directory: project.root.join("target"),
        packages: vec![package.clone()],
        selected_package: Some(package.clone()),
        selection_reason: None,
        native_package: None,
        issues: Vec::new(),
        command: Vec::new(),
    };
    let publication = PublishedGeneration {
        build_id: previous.payload.build_id,
        module_path: project.module.clone(),
        last_known_good_path: project.root.join(crate::LAST_KNOWN_GOOD_FILE),
        reused: true,
        validator_stdout: String::new(),
        validator_stderr: String::new(),
    };
    let escaped_source = project.source.to_string_lossy().replace(' ', "\\ ");
    std::fs::write(
        project.module.with_extension("d"),
        format!("module: {escaped_source}\n"),
    )
    .expect("Cargo dependency information");
    std::fs::write(&project.source, b"pub struct EditedPlayer;\n")
        .expect("source edited after build");

    let error = super::record::record_build_receipt_guarded(
        &project.root,
        (&model, &package),
        &project.module,
        &publication,
        OsStr::new("cargo"),
        project.validator.as_os_str(),
        || {
            if super::inputs::verify_snapshots(&stable_inputs)?.is_some() {
                Err("source changed since the build".to_owned())
            } else {
                Ok(())
            }
        },
    )
    .expect_err("stale build cannot acquire a fresh receipt");
    assert!(error.contains("source changed since the build"), "{error}");
    assert_eq!(
        std::fs::read(&project.receipt_path).expect("preserved receipt"),
        original_receipt
    );
    assert!(!project.check().fresh);
}
