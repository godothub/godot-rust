use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;

const MAX_ENVIRONMENT_VARIABLES: usize = 16_384;
const MAX_ENVIRONMENT_ENTRY_BYTES: usize = 1024 * 1024;
const MAX_ENVIRONMENT_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_FINGERPRINT_BYTES: u64 = 1024 * 1024;
const MAX_FINGERPRINT_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FINGERPRINT_FILES: usize = 100_000;

const FIXED_BUILD_VARIABLES: &[&str] = &[
    "AR",
    "CC",
    "CFLAGS",
    "CXX",
    "CXXFLAGS",
    "HOME",
    "LD",
    "LDFLAGS",
    "MACOSX_DEPLOYMENT_TARGET",
    "PATH",
    "GODOT_RS_GODOT",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "SDKROOT",
    "USERPROFILE",
];

const BUILD_VARIABLE_PREFIXES: &[&str] = &[
    "AR_",
    "CARGO_",
    "CC_",
    "CFLAGS_",
    "CXX_",
    "CXXFLAGS_",
    "DEP_",
    "LD_",
    "LDFLAGS_",
    "OPENSSL_",
    "PKG_CONFIG",
    "RUST",
    "VCPKG_",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct EnvironmentSnapshot {
    pub variable_count: usize,
    pub variable_names: Vec<String>,
    pub sha256: String,
}

pub(super) fn snapshot_build_environment(
    artifact: Option<&Path>,
) -> Result<EnvironmentSnapshot, String> {
    let mut names = current_build_variable_names();
    if let Some(artifact) = artifact {
        names.extend(declared_build_script_variables(artifact)?);
    }
    snapshot_named_environment(names)
}

pub(super) fn resnapshot_build_environment(
    recorded: &EnvironmentSnapshot,
) -> Result<EnvironmentSnapshot, String> {
    validate_recorded_names(recorded)?;
    let mut names = current_build_variable_names();
    names.extend(recorded.variable_names.iter().cloned());
    snapshot_named_environment(names)
}

fn snapshot_named_environment(
    names: impl IntoIterator<Item = String>,
) -> Result<EnvironmentSnapshot, String> {
    let names = names.into_iter().collect::<BTreeSet<_>>();
    if names.len() > MAX_ENVIRONMENT_VARIABLES {
        return Err(format!(
            "Build Receipt has more than {MAX_ENVIRONMENT_VARIABLES} environment inputs"
        ));
    }
    let mut total_bytes = 0_usize;
    let mut hasher = Sha256::new();
    for name in &names {
        validate_variable_name(name)?;
        let name_bytes = name.as_bytes();
        let value = std::env::var_os(name);
        let value_bytes = value.as_deref().map(os_bytes);
        let entry_bytes = name_bytes
            .len()
            .checked_add(value_bytes.as_ref().map_or(0, Vec::len))
            .ok_or_else(|| "process environment size overflowed usize".to_owned())?;
        if entry_bytes > MAX_ENVIRONMENT_ENTRY_BYTES {
            return Err(format!(
                "a process environment entry exceeds the {MAX_ENVIRONMENT_ENTRY_BYTES} byte safety limit"
            ));
        }
        total_bytes = total_bytes
            .checked_add(entry_bytes)
            .ok_or_else(|| "process environment size overflowed usize".to_owned())?;
        if total_bytes > MAX_ENVIRONMENT_TOTAL_BYTES {
            return Err(format!(
                "process environment exceeds the {MAX_ENVIRONMENT_TOTAL_BYTES} byte safety limit"
            ));
        }
        hash_field(&mut hasher, name_bytes)?;
        if let Some(value) = value_bytes {
            hasher.update([1]);
            hash_field(&mut hasher, &value)?;
        } else {
            hasher.update([0]);
        }
    }
    let variable_names = names.into_iter().collect::<Vec<_>>();
    Ok(EnvironmentSnapshot {
        variable_count: variable_names.len(),
        variable_names,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn current_build_variable_names() -> BTreeSet<String> {
    let mut names = FIXED_BUILD_VARIABLES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    names.extend(std::env::vars_os().filter_map(|(name, _)| {
        let name = name.to_str()?;
        BUILD_VARIABLE_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
            .then(|| name.to_owned())
    }));
    names
}

fn declared_build_script_variables(artifact: &Path) -> Result<BTreeSet<String>, String> {
    let Some(profile_directory) = artifact.parent() else {
        return Ok(BTreeSet::new());
    };
    let fingerprint_root = profile_directory.join(".fingerprint");
    let root_metadata = match std::fs::symlink_metadata(&fingerprint_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeSet::new());
        }
        Err(error) => {
            return Err(format!(
                "could not scan Cargo fingerprints `{}`: {error}",
                fingerprint_root.display()
            ));
        }
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(format!(
            "Cargo fingerprint root is not an owned directory: {}",
            fingerprint_root.display()
        ));
    }
    let directories = std::fs::read_dir(&fingerprint_root).map_err(|error| {
        format!(
            "could not scan Cargo fingerprints `{}`: {error}",
            fingerprint_root.display()
        )
    })?;
    let mut names = BTreeSet::new();
    let mut file_count = 0_usize;
    let mut total_bytes = 0_u64;
    for directory in directories {
        let directory = directory.map_err(|error| {
            format!(
                "could not read Cargo fingerprint directory `{}`: {error}",
                fingerprint_root.display()
            )
        })?;
        let directory_type = directory.file_type().map_err(|error| {
            format!(
                "could not inspect Cargo fingerprint path `{}`: {error}",
                directory.path().display()
            )
        })?;
        if directory_type.is_symlink() {
            return Err(format!(
                "Cargo fingerprint directory must not be a symbolic link: {}",
                directory.path().display()
            ));
        }
        if !directory_type.is_dir() {
            continue;
        }
        let files = std::fs::read_dir(directory.path()).map_err(|error| {
            format!(
                "could not scan Cargo fingerprint `{}`: {error}",
                directory.path().display()
            )
        })?;
        for file in files {
            let file = file.map_err(|error| {
                format!(
                    "could not read Cargo fingerprint `{}`: {error}",
                    directory.path().display()
                )
            })?;
            let file_name = file.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.starts_with("run-build-script-") || !file_name.ends_with(".json") {
                continue;
            }
            let file_type = file.file_type().map_err(|error| {
                format!(
                    "could not inspect Cargo fingerprint `{}`: {error}",
                    file.path().display()
                )
            })?;
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(format!(
                    "Cargo build-script fingerprint is not a regular file: {}",
                    file.path().display()
                ));
            }
            file_count += 1;
            if file_count > MAX_FINGERPRINT_FILES {
                return Err(format!(
                    "Cargo has more than {MAX_FINGERPRINT_FILES} build-script fingerprints"
                ));
            }
            let metadata = file.metadata().map_err(|error| {
                format!(
                    "could not inspect Cargo fingerprint `{}`: {error}",
                    file.path().display()
                )
            })?;
            if metadata.len() > MAX_FINGERPRINT_BYTES {
                return Err(format!(
                    "Cargo build-script fingerprint is not a bounded regular file: {}",
                    file.path().display()
                ));
            }
            total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| "Cargo fingerprint size overflowed u64".to_owned())?;
            if total_bytes > MAX_FINGERPRINT_TOTAL_BYTES {
                return Err(format!(
                    "Cargo build-script fingerprints exceed the {MAX_FINGERPRINT_TOTAL_BYTES} byte safety limit"
                ));
            }
            collect_fingerprint_variables(&file.path(), &mut names)?;
        }
    }
    Ok(names)
}

fn collect_fingerprint_variables(path: &Path, names: &mut BTreeSet<String>) -> Result<(), String> {
    let source = std::fs::read(path).map_err(|error| {
        format!(
            "could not read Cargo fingerprint `{}`: {error}",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_slice(&source).map_err(|error| {
        format!(
            "Cargo fingerprint contains invalid JSON `{}`: {error}",
            path.display()
        )
    })?;
    for local in value
        .get("local")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(variable) = local
            .get("RerunIfEnvChanged")
            .and_then(|entry| entry.get("var"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        validate_variable_name(variable)?;
        names.insert(variable.to_owned());
    }
    Ok(())
}

fn validate_recorded_names(snapshot: &EnvironmentSnapshot) -> Result<(), String> {
    if snapshot.variable_count != snapshot.variable_names.len()
        || snapshot.variable_names.len() > MAX_ENVIRONMENT_VARIABLES
        || snapshot
            .variable_names
            .windows(2)
            .any(|names| names[0] >= names[1])
    {
        return Err("Build Receipt environment input names are invalid".to_owned());
    }
    for name in &snapshot.variable_names {
        validate_variable_name(name)?;
    }
    Ok(())
}

fn validate_variable_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > MAX_ENVIRONMENT_ENTRY_BYTES
        || name.bytes().any(|byte| matches!(byte, 0 | b'='))
    {
        return Err("Build Receipt environment input name is invalid".to_owned());
    }
    Ok(())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) -> Result<(), String> {
    let length = u64::try_from(value.len())
        .map_err(|_| "process environment field length overflowed u64".to_owned())?;
    hasher.update(length.to_le_bytes());
    hasher.update(value);
    Ok(())
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    value.as_bytes().to_owned()
}

#[cfg(windows)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(not(any(unix, windows)))]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_snapshot_contains_no_plaintext_values() {
        let snapshot = snapshot_build_environment(None).expect("environment snapshot");
        assert!(snapshot.variable_count > 0);
        assert!(snapshot.variable_names.iter().any(|name| name == "PATH"));
        assert_eq!(snapshot.sha256.len(), 64);
        assert!(snapshot.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let encoded = serde_json::to_string(&snapshot).expect("environment JSON");
        assert!(!encoded.contains("PATH="));
        if let Ok(path) = std::env::var("PATH") {
            assert!(!encoded.contains(&path));
        }
        assert_eq!(
            resnapshot_build_environment(&snapshot).expect("repeat snapshot"),
            snapshot
        );
    }

    #[test]
    fn plugin_control_variables_do_not_invalidate_build_receipts() {
        assert!(current_build_variable_names().contains("GODOT_RS_GODOT"));
        assert!(!current_build_variable_names().contains("GODOT_RS_BUILD_CANCEL_FILE"));
    }
}
