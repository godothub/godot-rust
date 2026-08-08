use godot_api::GodotApiVersion;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
    workspace_members: Vec<String>,
    workspace_default_members: Vec<String>,
    workspace_root: PathBuf,
    target_directory: PathBuf,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadataPackage {
    id: String,
    name: String,
    manifest_path: PathBuf,
    #[serde(default)]
    dependencies: Vec<CargoMetadataDependency>,
    #[serde(default)]
    metadata: serde_json::Value,
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadataDependency {
    name: String,
    rename: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    optional: bool,
    target: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadataTarget {
    name: String,
    crate_types: Vec<String>,
    src_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageSelectionReason {
    WorkspaceConfiguration,
    RootPackage,
    PackageConfiguration,
    WorkspaceDefault,
    OnlyCdylib,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GodotRustMode {
    Script,
    Extension,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CargoTargetModel {
    pub name: String,
    pub src_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EditorWorkflowConfig {
    pub auto_check: bool,
    pub build_before_play: bool,
    pub auto_build_on_idle: bool,
}

impl Default for EditorWorkflowConfig {
    fn default() -> Self {
        Self {
            auto_check: true,
            build_before_play: true,
            auto_build_on_idle: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CargoPackageModel {
    pub id: String,
    pub name: String,
    pub manifest_path: PathBuf,
    pub workspace_default: bool,
    pub godot_rs_dependency: bool,
    pub godot_rust_enabled: bool,
    pub godot_rust_mode: Option<GodotRustMode>,
    pub godot_api: Option<GodotApiVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scripts_path: Option<PathBuf>,
    pub editor: EditorWorkflowConfig,
    pub script_mode_configured: bool,
    pub configuration_issues: Vec<String>,
    pub cdylib_targets: Vec<CargoTargetModel>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CargoProjectModel {
    pub project_root: PathBuf,
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub packages: Vec<CargoPackageModel>,
    pub selected_package: Option<CargoPackageModel>,
    pub selection_reason: Option<PackageSelectionReason>,
    pub native_package: Option<CargoPackageModel>,
    pub issues: Vec<String>,
    pub command: Vec<String>,
}

pub fn inspect_cargo_project(
    project_root: impl AsRef<Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<CargoProjectModel, String> {
    let project_root = project_root.as_ref().canonicalize().map_err(|error| {
        format!(
            "could not resolve Cargo project root `{}`: {error}",
            project_root.as_ref().display()
        )
    })?;
    if !project_root.join("Cargo.toml").is_file() {
        return Err(format!(
            "Cargo project has no Cargo.toml: {}",
            project_root.display()
        ));
    }
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let arguments = ["metadata", "--format-version", "1", "--no-deps"];
    let output = crate::process::run_command(
        Command::new(&cargo)
            .args(arguments)
            .current_dir(&project_root),
        &format!("`{}` metadata", cargo.to_string_lossy()),
    )?;
    if output.stdout.len() > MAX_METADATA_BYTES || output.stderr.len() > MAX_METADATA_BYTES {
        return Err(format!(
            "Cargo metadata output exceeded the {} byte safety limit",
            MAX_METADATA_BYTES
        ));
    }
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Cargo returned invalid metadata JSON: {error}"))?;
    build_project_model(
        project_root,
        metadata,
        std::iter::once(cargo.to_string_lossy().into_owned())
            .chain(arguments.map(str::to_owned))
            .collect(),
    )
}

fn build_project_model(
    project_root: PathBuf,
    metadata: CargoMetadata,
    command: Vec<String>,
) -> Result<CargoProjectModel, String> {
    let members = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let defaults = metadata
        .workspace_default_members
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut packages = metadata
        .packages
        .into_iter()
        .filter(|package| members.contains(package.id.as_str()))
        .map(|package| {
            let configuration = parse_package_configuration(&package.metadata);
            let godot_rs_dependency = package.dependencies.iter().any(|dependency| {
                dependency.name == "godot_rs"
                    && dependency.kind.is_none()
                    && !dependency.optional
                    && dependency.target.is_none()
                    && dependency
                        .rename
                        .as_deref()
                        .is_none_or(|name| name == "godot_rs")
            });
            CargoPackageModel {
                workspace_default: defaults.contains(package.id.as_str()),
                godot_rs_dependency,
                godot_rust_enabled: configuration.enabled,
                godot_rust_mode: configuration.mode,
                godot_api: configuration.godot_api,
                scripts_path: configuration.scripts_path.map(|path| {
                    package
                        .manifest_path
                        .parent()
                        .expect("Cargo package manifest has a parent directory")
                        .join(path)
                }),
                editor: configuration.editor,
                script_mode_configured: configuration.enabled
                    && configuration.mode == Some(GodotRustMode::Script),
                configuration_issues: configuration.issues,
                cdylib_targets: package
                    .targets
                    .into_iter()
                    .filter(|target| target.crate_types.iter().any(|kind| kind == "cdylib"))
                    .map(|target| CargoTargetModel {
                        name: target.name,
                        src_path: target.src_path,
                    })
                    .collect(),
                id: package.id,
                name: package.name,
                manifest_path: package.manifest_path,
            }
        })
        .collect::<Vec<_>>();
    packages.sort_by(|left, right| {
        left.manifest_path
            .cmp(&right.manifest_path)
            .then_with(|| left.name.cmp(&right.name))
    });

    let root_manifest = project_root.join("Cargo.toml");
    let configured_package = metadata
        .metadata
        .pointer("/godot-rust/package")
        .and_then(serde_json::Value::as_str);
    let mut issues = package_shape_issues(&packages);
    issues.extend(packages.iter().flat_map(|package| {
        package
            .configuration_issues
            .iter()
            .map(|issue| format!("package `{}`: {issue}", package.name))
    }));
    let native_packages = packages
        .iter()
        .filter(|package| {
            package.godot_rust_enabled && package.godot_rust_mode == Some(GodotRustMode::Extension)
        })
        .collect::<Vec<_>>();
    let native_package = match native_packages.as_slice() {
        [package] if is_native_buildable(package) => Some((*package).clone()),
        [_] => None,
        [] => None,
        _ => {
            issues.push(
                "multiple packages declare [package.metadata.godot-rust] Extension Mode; \
                 exactly one enabled Native package is allowed"
                    .to_owned(),
            );
            None
        }
    };
    let selection = if let Some(name) = configured_package {
        packages
            .iter()
            .find(|package| {
                package.name == name && package.godot_rust_mode == Some(GodotRustMode::Extension)
            })
            .map_or_else(
                || {
                    select_named_package(&packages, name, &mut issues)
                        .map(|package| (package, PackageSelectionReason::WorkspaceConfiguration))
                },
                |_| None,
            )
    } else if let Some(package) = packages
        .iter()
        .find(|package| package.manifest_path == root_manifest)
    {
        if package.godot_rust_mode == Some(GodotRustMode::Extension) {
            None
        } else {
            select_if_publishable(package, &mut issues)
                .map(|package| (package, PackageSelectionReason::RootPackage))
        }
    } else {
        let configured = packages
            .iter()
            .filter(|package| package.script_mode_configured)
            .collect::<Vec<_>>();
        match configured.as_slice() {
            [package] => select_if_publishable(package, &mut issues)
                .map(|package| (package, PackageSelectionReason::PackageConfiguration)),
            [] => {
                let defaults = packages
                    .iter()
                    .filter(|package| package.workspace_default && is_publishable(package))
                    .collect::<Vec<_>>();
                if defaults.is_empty() {
                    select_candidates(
                        packages
                            .iter()
                            .filter(|package| is_publishable(package))
                            .collect(),
                        "multiple Workspace packages produce Script Mode cdylibs",
                        &mut issues,
                    )
                    .map(|package| (package, PackageSelectionReason::OnlyCdylib))
                } else {
                    select_candidates(
                        defaults,
                        "multiple default Workspace packages produce Script Mode cdylibs",
                        &mut issues,
                    )
                    .map(|package| (package, PackageSelectionReason::WorkspaceDefault))
                }
            }
            _ => {
                issues.push(
                    "multiple packages declare [package.metadata.godot-rust] Script Mode; set \
                 [workspace.metadata.godot-rust].package"
                        .to_owned(),
                );
                None
            }
        }
    };
    if selection.is_none() && !packages.iter().any(is_publishable) && native_packages.is_empty() {
        issues.push(
            "no Workspace package has exactly one cdylib target; Script Mode requires \
             [lib] crate-type = [\"cdylib\", \"rlib\"]"
                .to_owned(),
        );
    }
    let (selected_package, selection_reason) = selection
        .map(|(package, reason)| (Some(package.clone()), Some(reason)))
        .unwrap_or((None, None));

    Ok(CargoProjectModel {
        project_root,
        workspace_root: metadata.workspace_root,
        target_directory: metadata.target_directory,
        packages,
        selected_package,
        selection_reason,
        native_package,
        issues,
        command,
    })
}

struct ParsedPackageConfiguration {
    enabled: bool,
    mode: Option<GodotRustMode>,
    godot_api: Option<GodotApiVersion>,
    scripts_path: Option<PathBuf>,
    editor: EditorWorkflowConfig,
    issues: Vec<String>,
}

fn parse_package_configuration(metadata: &serde_json::Value) -> ParsedPackageConfiguration {
    let Some(configuration_value) = metadata.get("godot-rust") else {
        return ParsedPackageConfiguration {
            enabled: true,
            mode: None,
            godot_api: None,
            scripts_path: None,
            editor: EditorWorkflowConfig::default(),
            issues: Vec::new(),
        };
    };
    let Some(configuration) = configuration_value.as_object() else {
        return ParsedPackageConfiguration {
            enabled: true,
            mode: None,
            godot_api: None,
            scripts_path: None,
            editor: EditorWorkflowConfig::default(),
            issues: vec!["package.metadata.godot-rust must be a TOML table".to_owned()],
        };
    };
    let mut issues = Vec::new();
    let enabled = configuration
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if configuration
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        issues.push("package.metadata.godot-rust.enabled must be a boolean".to_owned());
    }
    let mode_value = configuration
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("script");
    let mode = match mode_value {
        "script" => Some(GodotRustMode::Script),
        "extension" => Some(GodotRustMode::Extension),
        "gdextension" => {
            issues.push(
                "package.metadata.godot-rust.mode `gdextension` was renamed to `extension`"
                    .to_owned(),
            );
            None
        }
        _ => None,
    };
    if mode.is_none() && mode_value != "gdextension" {
        issues.push("package.metadata.godot-rust.mode must be `script` or `extension`".to_owned());
    }

    let godot_value = configuration.get("godot");
    let godot_api = godot_value
        .and_then(serde_json::Value::as_str)
        .and_then(|value| match value.parse() {
            Ok(version) => Some(version),
            Err(error) => {
                issues.push(format!(
                    "package.metadata.godot-rust.godot is invalid: {error}"
                ));
                None
            }
        });
    if godot_value.is_some_and(|value| !value.is_string()) {
        issues.push("package.metadata.godot-rust.godot must be a string".to_owned());
    }
    match mode {
        Some(GodotRustMode::Extension) if enabled && godot_value.is_none() => issues.push(
            "Extension Mode requires `godot = \"4.4\"`, `\"4.5\"`, `\"4.6\"`, or `\"4.7\"`"
                .to_owned(),
        ),
        _ => {}
    }
    let scripts_value = configuration.get("scripts");
    let scripts_path = scripts_value
        .and_then(serde_json::Value::as_str)
        .and_then(|value| match validate_scripts_path(value) {
            Ok(path) => Some(path),
            Err(error) => {
                issues.push(error);
                None
            }
        });
    if scripts_value.is_some_and(|value| !value.is_string()) {
        issues.push("package.metadata.godot-rust.scripts must be a string".to_owned());
    }
    let scripts_path = match mode {
        Some(GodotRustMode::Script) if enabled => {
            Some(scripts_path.unwrap_or_else(|| PathBuf::from("src/scripts")))
        }
        Some(GodotRustMode::Extension) if scripts_value.is_some() => {
            issues
                .push("Extension Mode must not set package.metadata.godot-rust.scripts".to_owned());
            None
        }
        _ => None,
    };
    let editor = EditorWorkflowConfig {
        auto_check: parse_boolean_setting(configuration, "auto-check", true, &mut issues),
        build_before_play: parse_boolean_setting(
            configuration,
            "build-before-play",
            true,
            &mut issues,
        ),
        auto_build_on_idle: parse_boolean_setting(
            configuration,
            "auto-build-on-idle",
            false,
            &mut issues,
        ),
    };

    ParsedPackageConfiguration {
        enabled,
        mode,
        godot_api,
        scripts_path,
        editor,
        issues,
    }
}

fn parse_boolean_setting(
    configuration: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: bool,
    issues: &mut Vec<String>,
) -> bool {
    let Some(value) = configuration.get(key) else {
        return default;
    };
    match value.as_bool() {
        Some(value) => value,
        None => {
            issues.push(format!(
                "package.metadata.godot-rust.{key} must be a boolean"
            ));
            default
        }
    }
}

fn validate_scripts_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || path
            .components()
            .all(|component| component == Component::CurDir)
    {
        return Err(
            "package.metadata.godot-rust.scripts must be a non-empty relative directory without `..`"
                .to_owned(),
        );
    }
    Ok(path.to_path_buf())
}

fn is_publishable(package: &CargoPackageModel) -> bool {
    package.godot_rust_enabled
        && package.godot_rust_mode != Some(GodotRustMode::Extension)
        && package.configuration_issues.is_empty()
        && package.cdylib_targets.len() == 1
}

fn is_native_buildable(package: &CargoPackageModel) -> bool {
    package.godot_rust_enabled
        && package.godot_rust_mode == Some(GodotRustMode::Extension)
        && package.godot_api.is_some()
        && package.configuration_issues.is_empty()
        && package.cdylib_targets.len() == 1
}

fn select_if_publishable<'a>(
    package: &'a CargoPackageModel,
    issues: &mut Vec<String>,
) -> Option<&'a CargoPackageModel> {
    if is_publishable(package) {
        Some(package)
    } else if !package.godot_rust_enabled {
        issues.push(format!(
            "root package `{}` has godot-rust disabled",
            package.name
        ));
        None
    } else if package.godot_rust_mode == Some(GodotRustMode::Extension) {
        issues.push(format!(
            "root package `{}` is configured for Extension Mode and cannot be loaded as a Script Mode project module",
            package.name
        ));
        None
    } else if !package.configuration_issues.is_empty() {
        None
    } else {
        issues.push(format!(
            "root package `{}` must have exactly one cdylib target; found {}",
            package.name,
            package.cdylib_targets.len()
        ));
        None
    }
}

fn select_named_package<'a>(
    packages: &'a [CargoPackageModel],
    name: &str,
    issues: &mut Vec<String>,
) -> Option<&'a CargoPackageModel> {
    let matches = packages
        .iter()
        .filter(|package| package.name == name)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [package] if is_publishable(package) => Some(package),
        [package] if !package.godot_rust_enabled => {
            issues.push(format!(
                "configured package `{name}` has godot-rust disabled"
            ));
            None
        }
        [package] if package.godot_rust_mode == Some(GodotRustMode::Extension) => {
            issues.push(format!(
                "configured package `{name}` uses Extension Mode, not Script Mode"
            ));
            None
        }
        [package] if !package.configuration_issues.is_empty() => None,
        [package] => {
            issues.push(format!(
                "configured package `{name}` must have exactly one cdylib target; found {}",
                package.cdylib_targets.len()
            ));
            None
        }
        [] => {
            issues.push(format!(
                "configured package `{name}` is not a Workspace member"
            ));
            None
        }
        _ => {
            issues.push(format!(
                "configured package name `{name}` is ambiguous in this Workspace"
            ));
            None
        }
    }
}

fn select_candidates<'a>(
    matches: Vec<&'a CargoPackageModel>,
    ambiguous_message: &str,
    issues: &mut Vec<String>,
) -> Option<&'a CargoPackageModel> {
    match matches.as_slice() {
        [package] => Some(package),
        [] => None,
        _ => {
            issues.push(format!(
                "{ambiguous_message}; set [workspace.metadata.godot-rust].package"
            ));
            None
        }
    }
}

fn package_shape_issues(packages: &[CargoPackageModel]) -> Vec<String> {
    packages
        .iter()
        .filter(|package| package.cdylib_targets.len() > 1)
        .map(|package| {
            format!(
                "package `{}` has {} cdylib targets; exactly one is required",
                package.name,
                package.cdylib_targets.len()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_package_is_selected_without_workspace_configuration() {
        let root = PathBuf::from("/game");
        let model = build_project_model(
            root.clone(),
            metadata_fixture(
                serde_json::json!(null),
                vec![package_fixture(
                    "game 0.1.0",
                    "godothub_project",
                    "/game/Cargo.toml",
                    serde_json::json!(null),
                    true,
                )],
                &["game 0.1.0"],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert_eq!(
            model
                .selected_package
                .as_ref()
                .map(|package| package.name.as_str()),
            Some("godothub_project")
        );
        assert_eq!(
            model.selection_reason,
            Some(PackageSelectionReason::RootPackage)
        );
        assert!(
            model
                .selected_package
                .as_ref()
                .is_some_and(|package| package.godot_rs_dependency)
        );
    }

    #[test]
    fn optional_or_renamed_sdk_dependencies_are_not_reported_as_ready() {
        for dependency in [
            CargoMetadataDependency {
                name: "godot_rs".to_owned(),
                rename: None,
                kind: None,
                optional: true,
                target: None,
            },
            CargoMetadataDependency {
                name: "godot_rs".to_owned(),
                rename: Some("other_name".to_owned()),
                kind: None,
                optional: false,
                target: None,
            },
        ] {
            let mut package = package_fixture(
                "game 0.1.0",
                "game",
                "/game/Cargo.toml",
                serde_json::json!(null),
                true,
            );
            package.dependencies = vec![dependency];
            let model = build_project_model(
                PathBuf::from("/game"),
                metadata_fixture(serde_json::json!(null), vec![package], &["game 0.1.0"]),
                Vec::new(),
            )
            .expect("project model");
            assert!(
                !model
                    .selected_package
                    .as_ref()
                    .expect("root package")
                    .godot_rs_dependency
            );
        }
    }

    #[test]
    fn virtual_workspace_uses_explicit_package_configuration() {
        let model = build_project_model(
            PathBuf::from("/game"),
            metadata_fixture(
                serde_json::json!({"godot-rust": {"package": "game_runtime"}}),
                vec![
                    package_fixture(
                        "editor 0.1.0",
                        "editor_tools",
                        "/game/editor/Cargo.toml",
                        serde_json::json!(null),
                        true,
                    ),
                    package_fixture(
                        "runtime 0.1.0",
                        "game_runtime",
                        "/game/runtime/Cargo.toml",
                        serde_json::json!(null),
                        true,
                    ),
                ],
                &["editor 0.1.0", "runtime 0.1.0"],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert_eq!(
            model
                .selected_package
                .as_ref()
                .map(|package| package.name.as_str()),
            Some("game_runtime")
        );
        assert_eq!(
            model.selection_reason,
            Some(PackageSelectionReason::WorkspaceConfiguration)
        );
    }

    #[test]
    fn ambiguous_virtual_workspace_requires_an_explicit_package() {
        let model = build_project_model(
            PathBuf::from("/game"),
            metadata_fixture(
                serde_json::json!(null),
                vec![
                    package_fixture(
                        "one 0.1.0",
                        "one",
                        "/game/one/Cargo.toml",
                        serde_json::json!(null),
                        true,
                    ),
                    package_fixture(
                        "two 0.1.0",
                        "two",
                        "/game/two/Cargo.toml",
                        serde_json::json!(null),
                        true,
                    ),
                ],
                &["one 0.1.0", "two 0.1.0"],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert!(model.selected_package.is_none());
        assert!(
            model
                .issues
                .iter()
                .any(|issue| { issue.contains("[workspace.metadata.godot-rust].package") })
        );
    }

    #[test]
    fn package_metadata_can_select_one_script_mode_member() {
        let model = build_project_model(
            PathBuf::from("/game"),
            metadata_fixture(
                serde_json::json!(null),
                vec![
                    package_fixture(
                        "tool 0.1.0",
                        "tool",
                        "/game/tool/Cargo.toml",
                        serde_json::json!(null),
                        true,
                    ),
                    package_fixture(
                        "runtime 0.1.0",
                        "runtime",
                        "/game/runtime/Cargo.toml",
                        serde_json::json!({"godot-rust": {"mode": "script"}}),
                        true,
                    ),
                ],
                &[],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert_eq!(
            model
                .selected_package
                .as_ref()
                .map(|package| package.name.as_str()),
            Some("runtime")
        );
        assert_eq!(
            model.selection_reason,
            Some(PackageSelectionReason::PackageConfiguration)
        );
    }

    #[test]
    fn extension_mode_requires_one_supported_major_minor_target() {
        let valid = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "extension", "godot": "4.6"}
        }));
        assert_eq!(valid.mode, Some(GodotRustMode::Extension));
        assert_eq!(valid.godot_api, Some(GodotApiVersion::new(4, 6)));
        assert!(valid.issues.is_empty());

        let patch = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "extension", "godot": "4.6.3"}
        }));
        assert!(patch.godot_api.is_none());
        assert!(
            patch
                .issues
                .iter()
                .any(|issue| issue.contains("contains a patch version"))
        );

        let missing = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "extension"}
        }));
        assert!(
            missing
                .issues
                .iter()
                .any(|issue| issue.contains("requires `godot"))
        );

        assert_eq!(
            serde_json::to_value(GodotRustMode::Extension).expect("serialize mode"),
            "extension"
        );
        let renamed = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "gdextension", "godot": "4.4"}
        }));
        assert!(renamed.mode.is_none());
        assert_eq!(
            renamed.issues,
            ["package.metadata.godot-rust.mode `gdextension` was renamed to `extension`"]
        );
    }

    #[test]
    fn script_mode_accepts_a_supported_api_target() {
        let configuration = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "script", "godot": "4.7"}
        }));
        assert_eq!(configuration.mode, Some(GodotRustMode::Script));
        assert_eq!(configuration.godot_api, Some(GodotApiVersion::new(4, 7)));
        assert!(configuration.issues.is_empty());
    }

    #[test]
    fn script_directory_is_relative_to_the_selected_package() {
        let model = build_project_model(
            PathBuf::from("/game"),
            metadata_fixture(
                serde_json::json!(null),
                vec![package_fixture(
                    "runtime 0.1.0",
                    "runtime",
                    "/game/runtime/Cargo.toml",
                    serde_json::json!({
                        "godot-rust": {
                            "mode": "script",
                            "scripts": "src/gameplay"
                        }
                    }),
                    true,
                )],
                &["runtime 0.1.0"],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert_eq!(
            model
                .selected_package
                .as_ref()
                .and_then(|package| package.scripts_path.as_deref()),
            Some(Path::new("/game/runtime/src/gameplay"))
        );

        let default = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "script"}
        }));
        assert_eq!(
            default.scripts_path.as_deref(),
            Some(Path::new("src/scripts"))
        );
    }

    #[test]
    fn editor_workflow_defaults_and_overrides_are_typed() {
        let default = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "script"}
        }));
        assert_eq!(default.editor, EditorWorkflowConfig::default());

        let configured = parse_package_configuration(&serde_json::json!({
            "godot-rust": {
                "mode": "script",
                "auto-check": false,
                "build-before-play": false,
                "auto-build-on-idle": true
            }
        }));
        assert_eq!(
            configured.editor,
            EditorWorkflowConfig {
                auto_check: false,
                build_before_play: false,
                auto_build_on_idle: true,
            }
        );
        assert!(configured.issues.is_empty());

        let malformed = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "script", "auto-check": "yes"}
        }));
        assert!(malformed.editor.auto_check);
        assert!(
            malformed
                .issues
                .iter()
                .any(|issue| issue.contains("auto-check must be a boolean"))
        );
    }

    #[test]
    fn unsafe_or_native_script_directories_are_rejected() {
        let traversal = parse_package_configuration(&serde_json::json!({
            "godot-rust": {"mode": "script", "scripts": "../outside"}
        }));
        assert!(
            traversal
                .issues
                .iter()
                .any(|issue| issue.contains("relative directory"))
        );

        let native = parse_package_configuration(&serde_json::json!({
            "godot-rust": {
                "mode": "extension",
                "godot": "4.4",
                "scripts": "src/scripts"
            }
        }));
        assert!(
            native
                .issues
                .iter()
                .any(|issue| issue.contains("must not set"))
        );
        assert!(native.scripts_path.is_none());
    }

    #[test]
    fn malformed_plugin_metadata_is_reported_without_guessing() {
        let configuration = parse_package_configuration(&serde_json::json!({
            "godot-rust": "extension"
        }));
        assert!(configuration.mode.is_none());
        assert_eq!(
            configuration.issues,
            ["package.metadata.godot-rust must be a TOML table"]
        );
    }

    #[test]
    fn extension_root_is_selected_only_as_a_native_package() {
        let model = build_project_model(
            PathBuf::from("/game"),
            metadata_fixture(
                serde_json::json!(null),
                vec![package_fixture(
                    "game 0.1.0",
                    "godothub_project",
                    "/game/Cargo.toml",
                    serde_json::json!({
                        "godot-rust": {"mode": "extension", "godot": "4.7"}
                    }),
                    true,
                )],
                &["game 0.1.0"],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert!(model.selected_package.is_none());
        assert_eq!(
            model
                .native_package
                .as_ref()
                .map(|package| package.name.as_str()),
            Some("godothub_project")
        );
        assert_eq!(
            model.packages[0].godot_api,
            Some(GodotApiVersion::new(4, 7))
        );
        assert!(model.issues.is_empty());
    }

    #[test]
    fn multiple_native_packages_are_rejected_without_guessing() {
        let model = build_project_model(
            PathBuf::from("/game"),
            metadata_fixture(
                serde_json::json!(null),
                vec![
                    package_fixture(
                        "one 0.1.0",
                        "one",
                        "/game/one/Cargo.toml",
                        serde_json::json!({
                            "godot-rust": {"mode": "extension", "godot": "4.4"}
                        }),
                        true,
                    ),
                    package_fixture(
                        "two 0.1.0",
                        "two",
                        "/game/two/Cargo.toml",
                        serde_json::json!({
                            "godot-rust": {"mode": "extension", "godot": "4.5"}
                        }),
                        true,
                    ),
                ],
                &[],
            ),
            Vec::new(),
        )
        .expect("project model");
        assert!(model.native_package.is_none());
        assert!(
            model
                .issues
                .iter()
                .any(|issue| issue.contains("exactly one enabled Native package"))
        );
    }

    fn metadata_fixture(
        metadata: serde_json::Value,
        packages: Vec<CargoMetadataPackage>,
        defaults: &[&str],
    ) -> CargoMetadata {
        CargoMetadata {
            workspace_members: packages.iter().map(|package| package.id.clone()).collect(),
            workspace_default_members: defaults.iter().map(|id| (*id).to_owned()).collect(),
            workspace_root: PathBuf::from("/game"),
            target_directory: PathBuf::from("/game/target"),
            packages,
            metadata,
        }
    }

    fn package_fixture(
        id: &str,
        name: &str,
        manifest: &str,
        metadata: serde_json::Value,
        cdylib: bool,
    ) -> CargoMetadataPackage {
        CargoMetadataPackage {
            id: id.to_owned(),
            name: name.to_owned(),
            manifest_path: PathBuf::from(manifest),
            dependencies: vec![CargoMetadataDependency {
                name: "godot_rs".to_owned(),
                rename: None,
                kind: None,
                optional: false,
                target: None,
            }],
            metadata,
            targets: vec![CargoMetadataTarget {
                name: name.to_owned(),
                crate_types: if cdylib {
                    vec!["cdylib".to_owned(), "rlib".to_owned()]
                } else {
                    vec!["lib".to_owned()]
                },
                src_path: PathBuf::from(format!("{manifest}/src/lib.rs")),
            }],
        }
    }
}
