use crate::{CargoProjectModel, PublishedGeneration};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) struct ReceiptCandidates {
    pub paths: BTreeSet<PathBuf>,
}

pub(super) fn collect_receipt_candidates(
    project: &CargoProjectModel,
    dependencies: &[PathBuf],
    dep_info_path: PathBuf,
    publication: &PublishedGeneration,
    validator: PathBuf,
    build_service: PathBuf,
) -> Result<ReceiptCandidates, String> {
    let mut candidates = collect_build_candidates(project, dependencies, dep_info_path)?;
    candidates.paths.extend([
        publication.module_path.clone(),
        publication.last_known_good_path.clone(),
        publication
            .module_path
            .parent()
            .ok_or_else(|| "published project module has no generation directory".to_owned())?
            .join("generation.json"),
        validator,
        build_service,
    ]);
    Ok(candidates)
}

pub(crate) fn collect_build_candidates(
    project: &CargoProjectModel,
    dependencies: &[PathBuf],
    dep_info_path: PathBuf,
) -> Result<ReceiptCandidates, String> {
    let project_root = project.project_root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Godot project root `{}`: {error}",
            project.project_root.display()
        )
    })?;
    let workspace_root = project.workspace_root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Cargo workspace root `{}`: {error}",
            project.workspace_root.display()
        )
    })?;
    let mut paths = BTreeSet::from([
        dep_info_path,
        project_root.join("Cargo.toml"),
        project_root.join("Cargo.lock"),
        workspace_root.join("Cargo.toml"),
        workspace_root.join("Cargo.lock"),
    ]);
    paths.extend(dependencies.iter().cloned());

    let mut manifest_directories = BTreeSet::new();
    for dependency in dependencies {
        let Some(parent) = dependency.parent() else {
            continue;
        };
        for ancestor in parent.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            if manifest.is_file() {
                paths.insert(manifest);
                manifest_directories.insert(ancestor.to_owned());
            }
        }
    }
    manifest_directories.extend([project_root.clone(), workspace_root.clone()]);
    for directory in manifest_directories {
        paths.insert(directory.join("build.rs"));
    }

    add_hierarchical_candidates(&mut paths, &project_root);
    if let Some(cargo_home) = cargo_home(&project_root) {
        paths.insert(cargo_home.join("config"));
        paths.insert(cargo_home.join("config.toml"));
    }
    Ok(ReceiptCandidates { paths })
}

fn add_hierarchical_candidates(paths: &mut BTreeSet<PathBuf>, project_root: &Path) {
    for ancestor in project_root.ancestors() {
        paths.insert(ancestor.join(".cargo/config"));
        paths.insert(ancestor.join(".cargo/config.toml"));
        paths.insert(ancestor.join("rust-toolchain"));
        paths.insert(ancestor.join("rust-toolchain.toml"));
    }
}

fn cargo_home(project_root: &Path) -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                project_root.join(path)
            }
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .map(|home| home.join(".cargo"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackageSelectionReason;

    #[test]
    fn candidate_set_covers_missing_configuration_and_build_script_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .canonicalize()
            .expect("canonical workspace");
        let package_root = root.join("crates/godot_rs_buildd");
        let dependency = package_root.join("src/lib.rs");
        let project = CargoProjectModel {
            project_root: root.clone(),
            workspace_root: root.clone(),
            target_directory: root.join("target"),
            packages: Vec::new(),
            selected_package: None,
            selection_reason: Some(PackageSelectionReason::RootPackage),
            native_package: None,
            issues: Vec::new(),
            command: Vec::new(),
        };
        let publication = PublishedGeneration {
            build_id: "sha256:test".to_owned(),
            module_path: root.join(".godot/rust/builds/test/project_module.so"),
            last_known_good_path: root.join(".godot/rust/last-known-good.json"),
            reused: false,
            validator_stdout: String::new(),
            validator_stderr: String::new(),
        };
        let candidates = collect_receipt_candidates(
            &project,
            std::slice::from_ref(&dependency),
            root.join("target/debug/libgame.d"),
            &publication,
            root.join("validator"),
            root.join("buildd"),
        )
        .expect("candidate paths");
        assert!(candidates.paths.contains(&dependency));
        assert!(candidates.paths.contains(&package_root.join("Cargo.toml")));
        assert!(candidates.paths.contains(&package_root.join("build.rs")));
        assert!(candidates.paths.contains(&root.join("Cargo.lock")));
        assert!(candidates.paths.contains(&root.join(".cargo/config.toml")));
    }
}
