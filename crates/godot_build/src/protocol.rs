use crate::{
    CargoTaskKind, DependencyChange, DiagnosticFixPlan, ProjectExportEnvironment,
    apply_dependency_change, apply_diagnostic_fix, build_and_publish, build_project_for_export,
    check_build_receipt, commit_native_publication, configure_project, initialize_project,
    inspect_cargo_project, inspect_native_build_plan, install_rust_target, list_dependencies,
    preview_dependency_change, probe_project, probe_toolchain, publish_validated_generation,
    rollback_native_publication, run_cargo_task, run_native_build, run_project_cargo_task,
    select_workspace_package,
};
use godot_api::GodotApiVersion;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_HEX_REQUEST_BYTES: usize = MAX_REQUEST_BYTES * 2;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum BuilddRequest {
    Probe {
        id: u64,
        root: PathBuf,
    },
    Initialize {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
    },
    Configure {
        id: u64,
        root: PathBuf,
    },
    Toolchain {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
    },
    InstallRustTarget {
        id: u64,
        root: PathBuf,
        target: String,
    },
    Metadata {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
    },
    SelectWorkspace {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        package: String,
    },
    Cargo {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        kind: CargoTaskKind,
    },
    NativePlan {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
    },
    NativeCargo {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        kind: CargoTaskKind,
    },
    NativeCommit {
        id: u64,
        root: PathBuf,
        platform_selector: String,
        sha256: String,
    },
    NativeRollback {
        id: u64,
        root: PathBuf,
        platform_selector: String,
        sha256: String,
    },
    ProjectCargo {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        kind: CargoTaskKind,
        validator: Option<PathBuf>,
    },
    ApplySuggestion {
        id: u64,
        root: PathBuf,
        plan: DiagnosticFixPlan,
    },
    DependencyList {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
    },
    DependencyPreview {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        change: DependencyChange,
    },
    DependencyApply {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        expected_sha256: String,
        change: DependencyChange,
    },
    ProjectExport {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        platform: String,
        features: Vec<String>,
        is_debug: bool,
        runtime_godot: GodotApiVersion,
        android_sdk: Option<PathBuf>,
        validator: PathBuf,
    },
    Publish {
        id: u64,
        root: PathBuf,
        artifact: PathBuf,
        validator: PathBuf,
    },
    Build {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        validator: PathBuf,
    },
    BuildReceipt {
        id: u64,
        root: PathBuf,
        cargo: Option<PathBuf>,
        validator: PathBuf,
    },
}

impl BuilddRequest {
    fn id(&self) -> u64 {
        match self {
            Self::Probe { id, .. }
            | Self::Initialize { id, .. }
            | Self::Configure { id, .. }
            | Self::Toolchain { id, .. }
            | Self::InstallRustTarget { id, .. }
            | Self::Metadata { id, .. }
            | Self::SelectWorkspace { id, .. }
            | Self::Cargo { id, .. }
            | Self::NativePlan { id, .. }
            | Self::NativeCargo { id, .. }
            | Self::NativeCommit { id, .. }
            | Self::NativeRollback { id, .. }
            | Self::ProjectCargo { id, .. }
            | Self::ApplySuggestion { id, .. }
            | Self::DependencyList { id, .. }
            | Self::DependencyPreview { id, .. }
            | Self::DependencyApply { id, .. }
            | Self::ProjectExport { id, .. }
            | Self::Publish { id, .. }
            | Self::Build { id, .. }
            | Self::BuildReceipt { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BuilddResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn handle_request(request: BuilddRequest) -> BuilddResponse {
    let id = request.id();
    let result = match request {
        BuilddRequest::Probe { root, .. } => probe_project(root).and_then(to_json_value),
        BuilddRequest::Initialize { root, cargo, .. } => {
            initialize_project(root, cargo.as_deref()).and_then(to_json_value)
        }
        BuilddRequest::Configure { root, .. } => configure_project(root).and_then(to_json_value),
        BuilddRequest::Toolchain { root, cargo, .. } => {
            probe_toolchain(root, cargo.as_deref()).and_then(to_json_value)
        }
        BuilddRequest::InstallRustTarget { root, target, .. } => {
            install_rust_target(root, &target).and_then(to_json_value)
        }
        BuilddRequest::Metadata { root, cargo, .. } => {
            inspect_cargo_project(root, cargo.as_deref()).and_then(to_json_value)
        }
        BuilddRequest::SelectWorkspace {
            root,
            cargo,
            package,
            ..
        } => select_workspace_package(root, &package, cargo.as_deref()).and_then(to_json_value),
        BuilddRequest::Cargo {
            root, cargo, kind, ..
        } => run_cargo_task(root, kind, cargo.as_deref()).and_then(to_json_value),
        BuilddRequest::NativePlan { root, cargo, .. } => {
            inspect_native_build_plan(root, cargo.as_deref()).and_then(to_json_value)
        }
        BuilddRequest::NativeCargo {
            root, cargo, kind, ..
        } => run_native_build(root, kind, cargo.as_deref()).and_then(to_json_value),
        BuilddRequest::NativeCommit {
            root,
            platform_selector,
            sha256,
            ..
        } => commit_native_publication(root, &platform_selector, &sha256).and_then(to_json_value),
        BuilddRequest::NativeRollback {
            root,
            platform_selector,
            sha256,
            ..
        } => rollback_native_publication(root, &platform_selector, &sha256).and_then(to_json_value),
        BuilddRequest::ProjectCargo {
            root,
            cargo,
            kind,
            validator,
            ..
        } => run_project_cargo_task(root, kind, cargo.as_deref(), validator.as_deref())
            .and_then(to_json_value),
        BuilddRequest::ApplySuggestion { root, plan, .. } => {
            apply_diagnostic_fix(root, plan).and_then(to_json_value)
        }
        BuilddRequest::DependencyList { root, cargo, .. } => {
            list_dependencies(root, cargo.as_deref()).and_then(to_json_value)
        }
        BuilddRequest::DependencyPreview {
            root,
            cargo,
            change,
            ..
        } => preview_dependency_change(root, change, cargo.as_deref()).and_then(to_json_value),
        BuilddRequest::DependencyApply {
            root,
            cargo,
            expected_sha256,
            change,
            ..
        } => apply_dependency_change(root, &expected_sha256, change, cargo.as_deref())
            .and_then(to_json_value),
        BuilddRequest::ProjectExport {
            root,
            cargo,
            platform,
            features,
            is_debug,
            runtime_godot,
            android_sdk,
            validator,
            ..
        } => build_project_for_export(
            root,
            &platform,
            &features,
            is_debug,
            ProjectExportEnvironment {
                runtime_godot,
                android_sdk,
            },
            cargo.as_deref(),
            validator,
        )
        .and_then(to_json_value),
        BuilddRequest::Publish {
            root,
            artifact,
            validator,
            ..
        } => publish_validated_generation(root, artifact, validator).and_then(to_json_value),
        BuilddRequest::Build {
            root,
            cargo,
            validator,
            ..
        } => build_and_publish(root, cargo.as_deref(), validator).and_then(to_json_value),
        BuilddRequest::BuildReceipt {
            root,
            cargo,
            validator,
            ..
        } => {
            let cargo = cargo
                .as_deref()
                .map_or_else(|| std::ffi::OsStr::new("cargo"), Path::as_os_str);
            check_build_receipt(&root, cargo, validator.as_os_str()).and_then(to_json_value)
        }
    };
    match result {
        Ok(result) => BuilddResponse {
            id,
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(error) => BuilddResponse {
            id,
            ok: false,
            result: None,
            error: Some(error),
        },
    }
}

pub fn handle_json_request(line: &str) -> BuilddResponse {
    if line.len() > MAX_REQUEST_BYTES {
        return BuilddResponse {
            id: 0,
            ok: false,
            result: None,
            error: Some(format!(
                "build daemon request exceeds {} bytes",
                MAX_REQUEST_BYTES
            )),
        };
    }
    match serde_json::from_str::<BuilddRequest>(line) {
        Ok(request) => handle_request(request),
        Err(error) => BuilddResponse {
            id: 0,
            ok: false,
            result: None,
            error: Some(format!("invalid build daemon request: {error}")),
        },
    }
}

pub fn decode_hex_request(encoded: &str) -> Result<String, String> {
    if encoded.len() > MAX_HEX_REQUEST_BYTES {
        return Err(format!(
            "hex-encoded build daemon request exceeds {} bytes",
            MAX_HEX_REQUEST_BYTES
        ));
    }
    if encoded.len() % 2 != 0 {
        return Err("hex-encoded build daemon request has an odd length".to_owned());
    }
    let mut decoded = Vec::with_capacity(encoded.len() / 2);
    for pair in encoded.as_bytes().chunks_exact(2) {
        let high = decode_hex_digit(pair[0])
            .ok_or_else(|| "hex-encoded build daemon request is invalid".to_owned())?;
        let low = decode_hex_digit(pair[1])
            .ok_or_else(|| "hex-encoded build daemon request is invalid".to_owned())?;
        decoded.push((high << 4) | low);
    }
    String::from_utf8(decoded)
        .map_err(|_| "hex-encoded build daemon request is not UTF-8".to_owned())
}

fn decode_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn to_json_value(value: impl Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value)
        .map_err(|error| format!("could not serialize build daemon response: {error}"))
}

pub fn serve(reader: impl BufRead, mut writer: impl Write) -> Result<(), String> {
    for line in reader.lines() {
        let line = line.map_err(|error| format!("could not read build daemon request: {error}"))?;
        if line.is_empty() {
            continue;
        }
        let response = handle_json_request(&line);
        serde_json::to_writer(&mut writer, &response)
            .map_err(|error| format!("could not write build daemon response: {error}"))?;
        writer
            .write_all(b"\n")
            .and_then(|()| writer.flush())
            .map_err(|error| format!("could not flush build daemon response: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn malformed_requests_return_an_error_without_stopping_the_daemon() {
        let input = b"{bad json}\n{\"command\":\"probe\",\"id\":7,\"root\":\"/missing\"}\n";
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output).expect("serve");
        let responses = String::from_utf8(output).expect("UTF-8 output");
        let values = responses
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("response JSON"))
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["id"], 0);
        assert_eq!(values[0]["ok"], false);
        assert_eq!(values[1]["id"], 7);
        assert_eq!(values[1]["ok"], false);
    }

    #[test]
    fn one_shot_json_uses_the_same_error_contract() {
        let response = handle_json_request("{bad json}");
        assert_eq!(response.id, 0);
        assert!(!response.ok);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("invalid build daemon request"))
        );
    }

    #[test]
    fn configure_requests_keep_their_identity_on_setup_errors() {
        let request = serde_json::json!({
            "command": "configure",
            "id": 23,
            "root": "/definitely/missing/godot-rust-project"
        });
        let response = handle_json_request(&request.to_string());
        assert_eq!(response.id, 23);
        assert!(!response.ok);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("could not resolve Godot project root"))
        );
    }

    #[test]
    fn workspace_selection_requests_preserve_unicode_package_names() {
        let request = serde_json::json!({
            "command": "select_workspace",
            "id": 24,
            "root": "/项目",
            "package": "游戏-runtime"
        });
        let decoded =
            serde_json::from_value::<BuilddRequest>(request).expect("Workspace selection request");
        let BuilddRequest::SelectWorkspace { id, package, .. } = decoded else {
            panic!("unexpected request variant");
        };
        assert_eq!(id, 24);
        assert_eq!(package, "游戏-runtime");
    }

    #[test]
    fn command_line_hex_transport_preserves_json_quotes_and_utf8_paths() {
        use std::fmt::Write as _;

        let request = r#"{"command":"probe","id":7,"root":"/项目"}"#;
        let encoded = request.as_bytes().iter().fold(
            String::with_capacity(request.len() * 2),
            |mut encoded, byte| {
                write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
                encoded
            },
        );
        assert_eq!(decode_hex_request(&encoded).expect("hex request"), request);
        assert!(decode_hex_request("0").is_err());
        assert!(decode_hex_request("zz").is_err());
    }

    #[test]
    fn missing_build_receipt_is_a_stale_result_not_a_protocol_failure() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let request = serde_json::json!({
            "command": "build_receipt",
            "id": 19,
            "root": root,
            "validator": root.join("missing-validator")
        });
        let response = handle_json_request(&request.to_string());
        assert!(response.ok);
        let result = response.result.expect("stale Receipt result");
        assert_eq!(result["fresh"], false);
        assert_eq!(result["reason"], "missing");
    }

    #[test]
    fn export_requests_preserve_godot_target_information() {
        let request = serde_json::json!({
            "command": "project_export",
            "id": 41,
            "root": "/missing",
            "platform": "macOS",
            "features": ["macos", "universal", "x86_64", "arm64", "release"],
            "is_debug": false,
            "runtime_godot": "4.6",
            "android_sdk": null,
            "validator": "/missing-validator"
        });
        let decoded =
            serde_json::from_value::<BuilddRequest>(request).expect("Project Export request");
        let BuilddRequest::ProjectExport {
            id,
            platform,
            features,
            is_debug,
            runtime_godot,
            ..
        } = decoded
        else {
            panic!("unexpected request variant");
        };
        assert_eq!(id, 41);
        assert_eq!(platform, "macOS");
        assert_eq!(
            features,
            ["macos", "universal", "x86_64", "arm64", "release"]
        );
        assert!(!is_debug);
        assert_eq!(runtime_godot, GodotApiVersion::new(4, 6));
    }

    #[test]
    fn rust_target_install_requests_preserve_only_the_exact_target() {
        let request = serde_json::json!({
            "command": "install_rust_target",
            "id": 42,
            "root": "/project",
            "target": "aarch64-linux-android"
        });
        let decoded =
            serde_json::from_value::<BuilddRequest>(request).expect("target install request");
        let BuilddRequest::InstallRustTarget { id, target, .. } = decoded else {
            panic!("unexpected request variant");
        };
        assert_eq!(id, 42);
        assert_eq!(target, "aarch64-linux-android");
    }
}
