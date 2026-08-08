use crate::project_setup::{ProjectSetupOutcome, configure_project};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_PACKAGE_NAME: &str = "godothub_project";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectKind {
    Uninitialized,
    Package { name: String, workspace_root: bool },
    Workspace { member_count: usize },
    Conflict { messages: Vec<String> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectProbe {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub kind: ProjectKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InitOutcome {
    pub root: PathBuf,
    pub package_name: String,
    pub command: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub setup: ProjectSetupOutcome,
}

#[derive(Debug)]
struct ProjectError(String);

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub fn probe_project(root: impl AsRef<Path>) -> Result<ProjectProbe, String> {
    probe_project_inner(root.as_ref()).map_err(|error| error.to_string())
}

fn probe_project_inner(root: &Path) -> Result<ProjectProbe, ProjectError> {
    let root = root.canonicalize().map_err(|error| {
        ProjectError(format!(
            "could not resolve Godot project root `{}`: {error}",
            root.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(ProjectError(format!(
            "Godot project root is not a directory: {}",
            root.display()
        )));
    }
    if !root.join("project.godot").is_file() {
        return Err(ProjectError(format!(
            "directory does not contain project.godot: {}",
            root.display()
        )));
    }

    let manifest_path = root.join("Cargo.toml");
    let kind = if !manifest_path.exists() {
        let mut messages = Vec::new();
        if root.join("src/lib.rs").exists() {
            messages
                .push("src/lib.rs already exists; initialization will not overwrite it".to_owned());
        }
        if root.join("Cargo.lock").exists() {
            messages.push(
                "Cargo.lock already exists without Cargo.toml; initialization will not remove it"
                    .to_owned(),
            );
        }
        if messages.is_empty() {
            ProjectKind::Uninitialized
        } else {
            ProjectKind::Conflict { messages }
        }
    } else if !manifest_path.is_file() {
        ProjectKind::Conflict {
            messages: vec!["Cargo.toml exists but is not a regular file".to_owned()],
        }
    } else {
        inspect_manifest(&manifest_path)?
    };

    Ok(ProjectProbe {
        root,
        manifest_path,
        kind,
    })
}

fn inspect_manifest(path: &Path) -> Result<ProjectKind, ProjectError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        ProjectError(format!("could not inspect `{}`: {error}", path.display()))
    })?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Ok(ProjectKind::Conflict {
            messages: vec![format!(
                "Cargo.toml exceeds the {} byte safety limit",
                MAX_MANIFEST_BYTES
            )],
        });
    }
    let source = std::fs::read_to_string(path).map_err(|error| {
        ProjectError(format!(
            "could not read `{}` as UTF-8: {error}",
            path.display()
        ))
    })?;
    let manifest = source.parse::<toml::Table>().map_err(|error| {
        ProjectError(format!(
            "Cargo.toml is invalid and was not modified: {error}"
        ))
    })?;
    let package = manifest.get("package").and_then(toml::Value::as_table);
    let workspace = manifest.get("workspace").and_then(toml::Value::as_table);

    if let Some(package) = package {
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            return Ok(ProjectKind::Conflict {
                messages: vec!["[package].name is missing or is not a string".to_owned()],
            });
        };
        return Ok(ProjectKind::Package {
            name: name.to_owned(),
            workspace_root: workspace.is_some(),
        });
    }
    if let Some(workspace) = workspace {
        let member_count = workspace
            .get("members")
            .and_then(toml::Value::as_array)
            .map_or(0, Vec::len);
        return Ok(ProjectKind::Workspace { member_count });
    }
    Ok(ProjectKind::Conflict {
        messages: vec![
            "Cargo.toml contains neither a [package] nor a [workspace] table".to_owned(),
        ],
    })
}

pub fn initialize_project(
    root: impl AsRef<Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<InitOutcome, String> {
    let probe = probe_project(root)?;
    if probe.kind != ProjectKind::Uninitialized {
        return Err(format!(
            "Cargo initialization is not safe for project state: {:?}",
            probe.kind
        ));
    }

    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let src_existed = probe.root.join("src").exists();
    let arguments = init_arguments();
    let output = crate::process::run_command(
        Command::new(&cargo)
            .args(&arguments)
            .current_dir(&probe.root),
        &format!("`{}` init", cargo.to_string_lossy()),
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        rollback_cargo_init(&probe.root, src_existed);
        return Err(format!(
            "cargo init failed with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    let initialized = probe_project(&probe.root)?;
    if !matches!(
        initialized.kind,
        ProjectKind::Package {
            ref name,
            ..
        } if name == DEFAULT_PACKAGE_NAME
    ) {
        rollback_cargo_init(&probe.root, src_existed);
        return Err("cargo init did not create the expected godothub_project package".to_owned());
    }
    let setup = configure_project(&probe.root).map_err(|error| {
        rollback_cargo_init(&probe.root, src_existed);
        format!("Cargo initialized the package, but godot-rust setup failed and was rolled back: {error}")
    })?;

    Ok(InitOutcome {
        root: probe.root,
        package_name: DEFAULT_PACKAGE_NAME.to_owned(),
        command: std::iter::once(cargo.to_string_lossy().into_owned())
            .chain(
                arguments
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned()),
            )
            .collect(),
        stdout,
        stderr,
        setup,
    })
}

fn init_arguments() -> [OsString; 7] {
    [
        OsString::from("init"),
        OsString::from("--lib"),
        OsString::from("--vcs"),
        OsString::from("none"),
        OsString::from("--name"),
        OsString::from(DEFAULT_PACKAGE_NAME),
        OsString::from("."),
    ]
}

fn rollback_cargo_init(root: &Path, src_existed: bool) {
    for path in [
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("src/lib.rs"),
    ] {
        let _ = std::fs::remove_file(path);
    }
    if !src_existed {
        let _ = std::fs::remove_dir(root.join("src"));
    }
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
            let path = std::env::temp_dir().join(format!(
                "godot-rust-buildd-project-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("temporary project directory");
            std::fs::write(path.join("project.godot"), b"[application]\n")
                .expect("project fixture");
            Self(path)
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn empty_godot_project_has_exact_safe_init_plan() {
        let project = TempProject::new();
        let probe = probe_project(&project.0).expect("probe");
        assert_eq!(probe.kind, ProjectKind::Uninitialized);
        assert_eq!(
            init_arguments(),
            [
                "init",
                "--lib",
                "--vcs",
                "none",
                "--name",
                "godothub_project",
                ".",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn existing_source_and_manifests_are_never_treated_as_empty() {
        let project = TempProject::new();
        std::fs::create_dir(project.0.join("src")).expect("src");
        std::fs::write(project.0.join("src/lib.rs"), b"pub fn user_code() {}\n")
            .expect("user source");
        assert!(matches!(
            probe_project(&project.0).expect("probe").kind,
            ProjectKind::Conflict { .. }
        ));

        std::fs::write(
            project.0.join("Cargo.toml"),
            b"[package]\nname = \"existing_game\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        assert_eq!(
            probe_project(&project.0).expect("probe").kind,
            ProjectKind::Package {
                name: "existing_game".to_owned(),
                workspace_root: false,
            }
        );
    }

    #[test]
    fn virtual_workspace_is_reported_without_modification() {
        let project = TempProject::new();
        std::fs::write(
            project.0.join("Cargo.toml"),
            b"[workspace]\nmembers = [\"game\", \"shared\"]\n",
        )
        .expect("manifest");
        assert_eq!(
            probe_project(&project.0).expect("probe").kind,
            ProjectKind::Workspace { member_count: 2 }
        );
        assert!(!project.0.join("Cargo.lock").exists());
    }

    #[test]
    fn local_cargo_initializes_the_fixed_safe_package_shape() {
        let project = TempProject::new();
        let outcome =
            initialize_project(&project.0, None::<&str>).expect("local cargo initialization");
        assert_eq!(outcome.package_name, DEFAULT_PACKAGE_NAME);
        assert_eq!(
            outcome.command[1..],
            [
                "init",
                "--lib",
                "--vcs",
                "none",
                "--name",
                "godothub_project",
                "."
            ]
        );
        let manifest =
            std::fs::read_to_string(project.0.join("Cargo.toml")).expect("generated manifest");
        assert!(manifest.contains("name = \"godothub_project\""));
        assert!(manifest.contains(&format!(
            "godot_rs = \"{}\"",
            outcome.setup.godot_rs_requirement
        )));
        assert!(manifest.contains("crate-type = [\"cdylib\", \"rlib\"]"));
        assert!(manifest.contains("[package.metadata.godot-rust]"));
        assert!(project.0.join("src/lib.rs").is_file());
        assert!(
            std::fs::read_to_string(project.0.join("src/lib.rs"))
                .expect("crate root")
                .contains("mod scripts;")
        );
        assert!(project.0.join("src/scripts/mod.rs").is_file());
        assert!(project.0.join(".cargo/config.toml").is_file());
        assert_eq!(outcome.setup.package_name, DEFAULT_PACKAGE_NAME);
        assert!(!project.0.join(".git").exists());
    }

    #[test]
    fn initialized_package_direct_cargo_uses_the_configured_api() {
        let project = TempProject::new();
        initialize_project(&project.0, None::<&str>).expect("local Cargo initialization");
        let manifest_path = project.0.join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .expect("generated manifest")
            .replace("godot = \"4.4\"", "godot = \"4.7\"");
        std::fs::write(&manifest_path, manifest).expect("4.7 project metadata");
        configure_project(&project.0).expect("synchronize the Cargo API selection");

        let crate_root = project.0.join("src/lib.rs");
        let mut source = std::fs::read_to_string(&crate_root).expect("generated crate root");
        source.push_str(
            "\n#[cfg(test)]\nmod api_selection_tests {\n    #[test]\n    \
             fn direct_cargo_uses_project_api() {\n        \
             assert_eq!(godot_rs::GODOT_API, \"4.7\");\n    }\n}\n",
        );
        std::fs::write(crate_root, source).expect("API selection test");

        let sdk = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("build service crate has a Workspace crates directory")
            .join("godot_rs");
        let mut manifest = std::fs::read_to_string(&manifest_path).expect("generated manifest");
        manifest.push_str(&format!(
            "\n[patch.crates-io]\ngodot_rs = {{ path = {:?} }}\n",
            sdk.to_string_lossy()
        ));
        std::fs::write(&manifest_path, manifest).expect("local SDK patch");
        let output = Command::new("cargo")
            .args(["test", "--offline", "--lib"])
            .env_remove("RUSTFLAGS")
            .env_remove("RUSTDOCFLAGS")
            .current_dir(&project.0)
            .output()
            .expect("direct Cargo test");
        assert!(
            output.status.success(),
            "direct Cargo did not compile the configured Godot API:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
