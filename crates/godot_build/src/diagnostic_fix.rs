use crate::managed_fs::atomic_write;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_FIX_FILES: usize = 32;
const MAX_FIX_EDITS: usize = 256;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REPLACEMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticFixEdit {
    pub file_name: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub replacement: String,
    pub expected_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticFixPlan {
    pub edits: Vec<DiagnosticFixEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticFixReport {
    pub changed_files: Vec<String>,
    pub undo: DiagnosticFixPlan,
}

struct PlannedFile {
    path: PathBuf,
    display_path: String,
    original: Vec<u8>,
    replacement: Vec<u8>,
}

pub fn apply_diagnostic_fix(
    project_root: impl AsRef<Path>,
    plan: DiagnosticFixPlan,
) -> Result<DiagnosticFixReport, String> {
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
    if plan.edits.is_empty() || plan.edits.len() > MAX_FIX_EDITS {
        return Err(format!(
            "Rust quick fix must contain between 1 and {MAX_FIX_EDITS} edits"
        ));
    }
    let mut grouped = BTreeMap::<PathBuf, Vec<DiagnosticFixEdit>>::new();
    for edit in plan.edits {
        validate_hash(&edit.expected_sha256)?;
        let path = resolve_source_path(&project_root, &edit.file_name)?;
        grouped.entry(path).or_default().push(edit);
    }
    if grouped.len() > MAX_FIX_FILES {
        return Err(format!(
            "Rust quick fix affects more than {MAX_FIX_FILES} files"
        ));
    }
    let mut files = Vec::with_capacity(grouped.len());
    for (path, edits) in grouped {
        files.push(plan_file(&project_root, path, edits)?);
    }
    for (applied, file) in files.iter().enumerate() {
        if let Err(error) = atomic_write(&file.path, &file.replacement, "Rust quick-fix source") {
            let rollback = rollback_files(&files[..applied]);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    format!("{error}; quick-fix rollback also failed: {rollback_error}")
                }
            });
        }
    }

    let changed_files = files.iter().map(|file| file.display_path.clone()).collect();
    let undo = DiagnosticFixPlan {
        edits: files
            .iter()
            .map(|file| DiagnosticFixEdit {
                file_name: file.display_path.clone(),
                byte_start: 0,
                byte_end: file.replacement.len() as u64,
                replacement: String::from_utf8(file.original.clone())
                    .expect("validated Rust source remains UTF-8"),
                expected_sha256: sha256(&file.replacement),
            })
            .collect(),
    };
    Ok(DiagnosticFixReport {
        changed_files,
        undo,
    })
}

fn plan_file(
    project_root: &Path,
    path: PathBuf,
    mut edits: Vec<DiagnosticFixEdit>,
) -> Result<PlannedFile, String> {
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "could not inspect Rust quick-fix source `{}`: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Rust quick-fix source must be a regular non-symlink file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "Rust quick-fix source exceeds the {MAX_SOURCE_BYTES} byte limit: {}",
            path.display()
        ));
    }
    let original = std::fs::read(&path).map_err(|error| {
        format!(
            "could not read Rust quick-fix source `{}`: {error}",
            path.display()
        )
    })?;
    let source = std::str::from_utf8(&original)
        .map_err(|_| format!("Rust quick-fix source is not UTF-8: {}", path.display()))?;
    let expected_hash = &edits[0].expected_sha256;
    if edits
        .iter()
        .any(|edit| &edit.expected_sha256 != expected_hash)
    {
        return Err(format!(
            "Rust quick fix has conflicting source versions for {}",
            path.display()
        ));
    }
    if sha256(&original) != *expected_hash {
        return Err(format!(
            "Rust quick fix is stale because the source changed: {}",
            path.display()
        ));
    }
    let replacement_bytes = edits
        .iter()
        .map(|edit| edit.replacement.len())
        .try_fold(0_usize, usize::checked_add)
        .filter(|bytes| *bytes <= MAX_REPLACEMENT_BYTES)
        .ok_or_else(|| {
            format!("Rust quick-fix replacements exceed the {MAX_REPLACEMENT_BYTES} byte limit")
        })?;
    let _ = replacement_bytes;
    edits.sort_by_key(|edit| (edit.byte_start, edit.byte_end));
    let mut previous: Option<(usize, usize)> = None;
    for edit in &edits {
        let start = usize::try_from(edit.byte_start)
            .map_err(|_| "Rust quick-fix byte offset exceeds this platform".to_owned())?;
        let end = usize::try_from(edit.byte_end)
            .map_err(|_| "Rust quick-fix byte offset exceeds this platform".to_owned())?;
        if start > end
            || end > original.len()
            || !source.is_char_boundary(start)
            || !source.is_char_boundary(end)
        {
            return Err(format!(
                "Rust quick fix has an invalid UTF-8 byte range in {}",
                path.display()
            ));
        }
        if previous.is_some_and(|(previous_start, previous_end)| {
            start < previous_end || start == previous_start
        }) {
            return Err(format!(
                "Rust quick fix has overlapping edits in {}",
                path.display()
            ));
        }
        previous = Some((start, end));
    }
    let mut replacement = original.clone();
    for edit in edits.iter().rev() {
        let start = edit.byte_start as usize;
        let end = edit.byte_end as usize;
        replacement.splice(start..end, edit.replacement.bytes());
    }
    let display_path = path
        .strip_prefix(project_root)
        .expect("resolved quick-fix source stays in project")
        .to_string_lossy()
        .replace('\\', "/");
    Ok(PlannedFile {
        path,
        display_path,
        original,
        replacement,
    })
}

fn resolve_source_path(project_root: &Path, file_name: &str) -> Result<PathBuf, String> {
    if file_name.is_empty() || file_name.contains('\0') {
        return Err("Rust quick-fix file name is invalid".to_owned());
    }
    let candidate = Path::new(file_name);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        project_root.join(candidate)
    };
    let candidate = candidate.canonicalize().map_err(|error| {
        format!(
            "could not resolve Rust quick-fix source `{}`: {error}",
            candidate.display()
        )
    })?;
    if !candidate.starts_with(project_root) {
        return Err(format!(
            "Rust quick-fix source is outside the Cargo project: {}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

fn rollback_files(files: &[PlannedFile]) -> Result<(), String> {
    let mut errors = Vec::new();
    for file in files.iter().rev() {
        if let Err(error) = atomic_write(&file.path, &file.original, "Rust quick-fix rollback") {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn validate_hash(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Rust quick-fix source hash is invalid".to_owned());
    }
    Ok(())
}

fn sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
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
            let root = std::env::temp_dir().join(format!(
                "godot-rust-diagnostic-fix-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&root).expect("temporary project");
            std::fs::write(root.join("Cargo.toml"), b"[package]\nname='game'\n")
                .expect("Cargo.toml");
            Self(root)
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn machine_edit_is_hash_guarded_and_undoable() {
        let project = TempProject::new();
        let path = project.0.join("player.rs");
        let source = b"let speed: i64 = \"fast\";\n";
        std::fs::write(&path, source).expect("source");
        let report = apply_diagnostic_fix(
            &project.0,
            DiagnosticFixPlan {
                edits: vec![DiagnosticFixEdit {
                    file_name: "player.rs".to_owned(),
                    byte_start: 17,
                    byte_end: 23,
                    replacement: "42".to_owned(),
                    expected_sha256: sha256(source),
                }],
            },
        )
        .expect("apply fix");
        assert_eq!(
            std::fs::read_to_string(&path).expect("updated source"),
            "let speed: i64 = 42;\n"
        );
        apply_diagnostic_fix(&project.0, report.undo).expect("undo");
        assert_eq!(std::fs::read(&path).expect("restored source"), source);
    }

    #[test]
    fn stale_overlapping_and_escaping_edits_are_rejected_without_changes() {
        let project = TempProject::new();
        let path = project.0.join("player.rs");
        let source = b"abcdef\n";
        std::fs::write(&path, source).expect("source");
        let base = DiagnosticFixEdit {
            file_name: "player.rs".to_owned(),
            byte_start: 1,
            byte_end: 3,
            replacement: "x".to_owned(),
            expected_sha256: sha256(source),
        };
        let mut stale = base.clone();
        stale.expected_sha256 = "0".repeat(64);
        assert!(
            apply_diagnostic_fix(&project.0, DiagnosticFixPlan { edits: vec![stale] })
                .expect_err("stale")
                .contains("source changed")
        );
        let mut overlapping = base.clone();
        overlapping.byte_start = 2;
        overlapping.byte_end = 4;
        assert!(
            apply_diagnostic_fix(
                &project.0,
                DiagnosticFixPlan {
                    edits: vec![base, overlapping],
                },
            )
            .expect_err("overlap")
            .contains("overlapping")
        );
        assert_eq!(std::fs::read(&path).expect("unchanged source"), source);
        let outside = project.0.parent().expect("parent").join("outside.rs");
        std::fs::write(&outside, b"outside").expect("outside source");
        let result = apply_diagnostic_fix(
            &project.0,
            DiagnosticFixPlan {
                edits: vec![DiagnosticFixEdit {
                    file_name: outside.to_string_lossy().into_owned(),
                    byte_start: 0,
                    byte_end: 1,
                    replacement: String::new(),
                    expected_sha256: sha256(b"outside"),
                }],
            },
        );
        let _ = std::fs::remove_file(outside);
        assert!(result.expect_err("escape").contains("outside"));
    }
}
