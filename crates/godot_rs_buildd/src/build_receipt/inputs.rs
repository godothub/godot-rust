use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

const MAX_TRACKED_INPUTS: usize = 100_000;
const MAX_TRACKED_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TRACKED_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TrackedInput {
    pub(super) path: PathBuf,
    pub(super) state: InputState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum InputState {
    Missing,
    Present { byte_len: u64, sha256: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct ScriptInventory {
    pub root: PathBuf,
    pub entries: Vec<TrackedInput>,
}

pub(crate) fn snapshot_paths(
    paths: impl IntoIterator<Item = PathBuf>,
) -> Result<Vec<TrackedInput>, String> {
    let mut snapshots = BTreeMap::new();
    let mut total_bytes = 0_u64;
    for path in paths {
        if snapshots.len() >= MAX_TRACKED_INPUTS {
            return Err(format!(
                "Build Receipt has more than {MAX_TRACKED_INPUTS} tracked inputs"
            ));
        }
        let snapshot = snapshot_path(&path)?;
        if let InputState::Present { byte_len, .. } = &snapshot.state {
            total_bytes = total_bytes
                .checked_add(*byte_len)
                .ok_or_else(|| "Build Receipt input size overflowed u64".to_owned())?;
            if total_bytes > MAX_TRACKED_TOTAL_BYTES {
                return Err(format!(
                    "Build Receipt inputs exceed the {MAX_TRACKED_TOTAL_BYTES} byte safety limit"
                ));
            }
        }
        snapshots.insert(snapshot.path.clone(), snapshot);
    }
    Ok(snapshots.into_values().collect())
}

pub(crate) fn verify_snapshots(recorded: &[TrackedInput]) -> Result<Option<PathBuf>, String> {
    validate_snapshot_shape(recorded)?;
    let paths = recorded
        .iter()
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    let current = snapshot_paths(paths)?;
    Ok(recorded
        .iter()
        .zip(current)
        .find_map(|(expected, actual)| (expected != &actual).then(|| expected.path.clone())))
}

pub(super) fn collect_script_inventory(root: &Path) -> Result<ScriptInventory, String> {
    let root = root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Rust script directory `{}`: {error}",
            root.display()
        )
    })?;
    let mut paths = Vec::new();
    collect_script_files(&root, &mut paths)?;
    Ok(ScriptInventory {
        root,
        entries: snapshot_paths(paths)?,
    })
}

pub(super) fn verify_script_inventory(
    recorded: &ScriptInventory,
) -> Result<Option<PathBuf>, String> {
    if !recorded.root.is_absolute() {
        return Err("Build Receipt script directory is not absolute".to_owned());
    }
    validate_snapshot_shape(&recorded.entries)?;
    let current = collect_script_inventory(&recorded.root)?;
    if current == *recorded {
        return Ok(None);
    }
    let changed = recorded
        .entries
        .iter()
        .zip(&current.entries)
        .find_map(|(expected, actual)| (expected != actual).then(|| expected.path.clone()))
        .or_else(|| {
            recorded
                .entries
                .get(current.entries.len())
                .map(|input| input.path.clone())
        })
        .or_else(|| {
            current
                .entries
                .get(recorded.entries.len())
                .map(|input| input.path.clone())
        })
        .unwrap_or_else(|| recorded.root.clone());
    Ok(Some(changed))
}

fn snapshot_path(path: &Path) -> Result<TrackedInput, String> {
    if !path.is_absolute() {
        return Err(format!(
            "Build Receipt input is not absolute: {}",
            path.display()
        ));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(TrackedInput {
                path: path.to_owned(),
                state: InputState::Missing,
            });
        }
        Err(error) => {
            return Err(format!(
                "could not inspect Build Receipt input `{}`: {error}",
                path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Build Receipt input is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_TRACKED_FILE_BYTES {
        return Err(format!(
            "Build Receipt input exceeds the {MAX_TRACKED_FILE_BYTES} byte safety limit: {}",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "could not resolve Build Receipt input `{}`: {error}",
            path.display()
        )
    })?;
    let (sha256, byte_len) = hash_file(&canonical)?;
    Ok(TrackedInput {
        path: canonical,
        state: InputState::Present { byte_len, sha256 },
    })
}

fn hash_file(path: &Path) -> Result<(String, u64), String> {
    let file = File::open(path).map_err(|error| {
        format!(
            "could not open Build Receipt input `{}`: {error}",
            path.display()
        )
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let mut byte_len = 0_u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            format!(
                "could not hash Build Receipt input `{}`: {error}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or_else(|| "Build Receipt input size overflowed u64".to_owned())?;
        if byte_len > MAX_TRACKED_FILE_BYTES {
            return Err(format!(
                "Build Receipt input exceeds the {MAX_TRACKED_FILE_BYTES} byte safety limit: {}",
                path.display()
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), byte_len))
}

fn validate_snapshot_shape(recorded: &[TrackedInput]) -> Result<(), String> {
    if recorded.len() > MAX_TRACKED_INPUTS {
        return Err(format!(
            "Build Receipt has more than {MAX_TRACKED_INPUTS} tracked inputs"
        ));
    }
    let mut previous = None;
    let mut total_bytes = 0_u64;
    for input in recorded {
        if !input.path.is_absolute() {
            return Err(format!(
                "Build Receipt input is not absolute: {}",
                input.path.display()
            ));
        }
        if previous.is_some_and(|path: &PathBuf| path >= &input.path) {
            return Err("Build Receipt inputs are not strictly sorted".to_owned());
        }
        if let InputState::Present { byte_len, sha256 } = &input.state {
            if *byte_len > MAX_TRACKED_FILE_BYTES
                || sha256.len() != 64
                || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!(
                    "Build Receipt input metadata is invalid: {}",
                    input.path.display()
                ));
            }
            total_bytes = total_bytes
                .checked_add(*byte_len)
                .ok_or_else(|| "Build Receipt input size overflowed u64".to_owned())?;
        }
        previous = Some(&input.path);
    }
    if total_bytes > MAX_TRACKED_TOTAL_BYTES {
        return Err(format!(
            "Build Receipt inputs exceed the {MAX_TRACKED_TOTAL_BYTES} byte safety limit"
        ));
    }
    Ok(())
}

fn collect_script_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if output.len() > MAX_TRACKED_INPUTS {
        return Err(format!(
            "Rust script directory has more than {MAX_TRACKED_INPUTS} tracked files"
        ));
    }
    let entries = std::fs::read_dir(directory).map_err(|error| {
        format!(
            "could not scan Rust script directory `{}`: {error}",
            directory.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "could not read Rust script directory `{}`: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "could not inspect Rust script path `{}`: {error}",
                path.display()
            )
        })?;
        if file_type.is_symlink() {
            return Err(format!(
                "Build Receipt does not follow symbolic links in the Rust script directory: {}",
                path.display()
            ));
        }
        if file_type.is_dir() {
            collect_script_files(&path, output)?;
        } else if file_type.is_file() && is_script_inventory_file(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_script_inventory_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    (name.ends_with(".rs") && name != "mod.rs") || name.ends_with(".rs.uid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "godot-rust-build-receipt-inputs-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("temporary directory");
            Self(path.canonicalize().expect("canonical temporary directory"))
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn present_and_missing_inputs_are_stable_and_detect_changes() {
        let directory = TempDirectory::new();
        let present = directory.0.join("present.rs");
        let missing = directory.0.join("missing.rs");
        std::fs::write(&present, b"one").expect("present input");
        let snapshots =
            snapshot_paths([missing.clone(), present.clone()]).expect("input snapshots");
        assert_eq!(snapshots[0].path, missing);
        assert_eq!(snapshots[0].state, InputState::Missing);
        assert_eq!(verify_snapshots(&snapshots).expect("verification"), None);

        std::fs::write(&present, b"two").expect("changed input");
        assert_eq!(
            verify_snapshots(&snapshots).expect("changed verification"),
            Some(present)
        );
    }

    #[test]
    fn script_inventory_detects_new_sources_and_uid_changes() {
        let directory = TempDirectory::new();
        let source = directory.0.join("player.rs");
        let uid = directory.0.join("player.rs.uid");
        std::fs::write(&source, b"pub struct Player;").expect("script");
        std::fs::write(&uid, b"uid://one\n").expect("script UID");
        std::fs::write(directory.0.join("mod.rs"), b"generated").expect("generated index");
        let inventory = collect_script_inventory(&directory.0).expect("script inventory");
        assert_eq!(inventory.entries.len(), 2);
        assert_eq!(
            verify_script_inventory(&inventory).expect("inventory verification"),
            None
        );

        std::fs::write(&uid, b"uid://two\n").expect("changed UID");
        assert_eq!(
            verify_script_inventory(&inventory).expect("changed inventory"),
            Some(uid)
        );
    }
}
