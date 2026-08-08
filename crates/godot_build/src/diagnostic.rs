use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Error,
    Warning,
    FailureNote,
    Note,
    Help,
    Unknown,
}

impl DiagnosticLevel {
    fn from_cargo(value: &str) -> Self {
        match value {
            "error" => Self::Error,
            "warning" => Self::Warning,
            "failure-note" => Self::FailureNote,
            "note" => Self::Note,
            "help" => Self::Help,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticSpan {
    pub file_name: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub line_start: u64,
    pub line_end: u64,
    pub column_start: u64,
    pub column_end: u64,
    pub is_primary: bool,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticReplacement {
    pub file_name: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub replacement: String,
    pub expected_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticSuggestion {
    pub message: String,
    pub applicability: String,
    pub replacements: Vec<DiagnosticReplacement>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CargoDiagnostic {
    pub package_id: Option<String>,
    pub target_name: Option<String>,
    pub level: DiagnosticLevel,
    pub code: Option<String>,
    pub message: String,
    pub rendered: Option<String>,
    pub spans: Vec<DiagnosticSpan>,
    pub suggestions: Vec<DiagnosticSuggestion>,
}

pub(crate) fn parse_compiler_message(
    value: &serde_json::Value,
    project_root: &Path,
) -> Option<CargoDiagnostic> {
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let spans = message
        .get("spans")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_span)
        .collect();
    let suggestions = std::iter::once(message)
        .chain(
            message
                .get("children")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|message| parse_suggestion(message, project_root))
        .collect();
    Some(CargoDiagnostic {
        package_id: value
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        target_name: value
            .pointer("/target/name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        level: DiagnosticLevel::from_cargo(
            message
                .get("level")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        ),
        code: message
            .pointer("/code/code")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        message: message.get("message")?.as_str()?.to_owned(),
        rendered: message
            .get("rendered")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        spans,
        suggestions,
    })
}

fn parse_span(value: &serde_json::Value) -> Option<DiagnosticSpan> {
    Some(DiagnosticSpan {
        file_name: value.get("file_name")?.as_str()?.to_owned(),
        byte_start: value.get("byte_start")?.as_u64()?,
        byte_end: value.get("byte_end")?.as_u64()?,
        line_start: value.get("line_start")?.as_u64()?,
        line_end: value.get("line_end")?.as_u64()?,
        column_start: value.get("column_start")?.as_u64()?,
        column_end: value.get("column_end")?.as_u64()?,
        is_primary: value.get("is_primary")?.as_bool()?,
        label: value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

fn parse_suggestion(
    message: &serde_json::Value,
    project_root: &Path,
) -> Option<DiagnosticSuggestion> {
    let spans = message.get("spans")?.as_array()?;
    let suggested = spans
        .iter()
        .filter(|span| {
            !span
                .get("suggested_replacement")
                .is_none_or(serde_json::Value::is_null)
        })
        .collect::<Vec<_>>();
    if suggested.is_empty() {
        return None;
    }
    let applicability = suggested
        .first()?
        .get("suggestion_applicability")?
        .as_str()?;
    if suggested.iter().any(|span| {
        span.get("suggestion_applicability")
            .and_then(serde_json::Value::as_str)
            != Some(applicability)
    }) {
        return None;
    }
    let replacements = suggested
        .into_iter()
        .map(|span| parse_replacement(span, project_root))
        .collect::<Option<Vec<_>>>()?;
    Some(DiagnosticSuggestion {
        message: message
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Apply rustc suggestion")
            .to_owned(),
        applicability: applicability.to_owned(),
        replacements,
    })
}

fn parse_replacement(
    span: &serde_json::Value,
    project_root: &Path,
) -> Option<DiagnosticReplacement> {
    let file_name = span.get("file_name")?.as_str()?;
    let path = resolve_source_path(project_root, file_name)?;
    let source = std::fs::read(&path).ok()?;
    let byte_start = span.get("byte_start")?.as_u64()?;
    let byte_end = span.get("byte_end")?.as_u64()?;
    let range = usize::try_from(byte_start).ok()?..usize::try_from(byte_end).ok()?;
    if range.start > range.end || range.end > source.len() {
        return None;
    }
    Some(DiagnosticReplacement {
        file_name: file_name.to_owned(),
        byte_start,
        byte_end,
        replacement: span.get("suggested_replacement")?.as_str()?.to_owned(),
        expected_sha256: format!("{:x}", Sha256::digest(&source)),
    })
}

fn resolve_source_path(project_root: &Path, file_name: &str) -> Option<PathBuf> {
    let root = project_root.canonicalize().ok()?;
    let candidate = Path::new(file_name);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let metadata = std::fs::symlink_metadata(&candidate).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let candidate = candidate.canonicalize().ok()?;
    candidate.starts_with(&root).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn cargo_json_diagnostic_keeps_source_navigation() {
        let value = serde_json::json!({
            "reason": "compiler-message",
            "package_id": "path+file:///game#0.1.0",
            "target": { "name": "game" },
            "message": {
                "message": "mismatched types",
                "code": { "code": "E0308" },
                "level": "error",
                "rendered": "error[E0308]: mismatched types",
                "spans": [{
                    "file_name": "src/scripts/player.rs",
                    "byte_start": 20,
                    "byte_end": 23,
                    "line_start": 4,
                    "line_end": 4,
                    "column_start": 9,
                    "column_end": 12,
                    "is_primary": true,
                    "label": "expected i64"
                }]
            }
        });
        let directory = std::env::temp_dir();
        let diagnostic = parse_compiler_message(&value, &directory).expect("compiler message");
        assert_eq!(diagnostic.level, DiagnosticLevel::Error);
        assert_eq!(diagnostic.code.as_deref(), Some("E0308"));
        assert_eq!(diagnostic.spans[0].line_start, 4);
        assert!(diagnostic.spans[0].is_primary);
        assert!(diagnostic.suggestions.is_empty());
    }

    #[test]
    fn machine_applicable_children_keep_hash_guarded_replacements() {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "godot-rust-diagnostic-parser-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary directory");
        let source = b"let count = 1;\n";
        std::fs::write(directory.join("player.rs"), source).expect("source");
        let value = serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": "unused variable",
                "level": "warning",
                "spans": [],
                "children": [{
                    "message": "prefix it with an underscore",
                    "spans": [{
                        "file_name": "player.rs",
                        "byte_start": 4,
                        "byte_end": 9,
                        "line_start": 1,
                        "line_end": 1,
                        "column_start": 5,
                        "column_end": 10,
                        "is_primary": true,
                        "label": null,
                        "suggested_replacement": "_count",
                        "suggestion_applicability": "MachineApplicable"
                    }]
                }]
            }
        });
        let diagnostic = parse_compiler_message(&value, &directory).expect("diagnostic");
        let suggestion = &diagnostic.suggestions[0];
        assert_eq!(suggestion.applicability, "MachineApplicable");
        assert_eq!(suggestion.replacements[0].replacement, "_count");
        assert_eq!(
            suggestion.replacements[0].expected_sha256,
            format!("{:x}", Sha256::digest(source))
        );
        std::fs::remove_dir_all(directory).expect("remove temporary directory");
    }
}
