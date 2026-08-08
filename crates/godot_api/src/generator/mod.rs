//! Build-time parser, validator, and high-level binding generator.

mod engine_api;
mod model;
mod validate;

use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub use engine_api::{
    ApiCoverageDisposition, ApiCoverageEntry, EngineApiGenerationError, EngineApiGenerationReport,
    UnsupportedEngineType, analyze_engine_api, generate_engine_api, verify_engine_api_coverage,
};
pub use model::{
    ApiArgument, ApiClass, ApiClassConstant, ApiEnum, ApiEnumValue, ApiHeader, ApiInventory,
    ApiMethod, ApiProperty, ApiReturnValue, ApiSignal, ApiSingleton, BuiltinClass,
    BuiltinClassOffsetConfiguration, BuiltinClassOffsets, BuiltinClassSize,
    BuiltinClassSizeConfiguration, BuiltinConstant, BuiltinConstructor, BuiltinEnum, BuiltinMember,
    BuiltinMemberOffset, BuiltinMethod, BuiltinOperator, ExtensionApi, GlobalConstant,
    NativeStructure, UtilityFunction,
};
pub use validate::{ExpectedApiVersion, ValidationIssue, validate_api};

const BUNDLED_APIS: [(&str, &str, &str); 4] = [
    (
        "4.4",
        "metadata/godot-4.4/extension_api.json",
        "1136ad8c676034a0d9ac15ec55f1f4c79f300fd645f45a08a129c0254ca95d51",
    ),
    (
        "4.5",
        "metadata/godot-4.5/extension_api.json",
        "481ed7dc8efc79e951081187cd5d651d6b34e2365a463f4f12adeab2f63475c8",
    ),
    (
        "4.6",
        "metadata/godot-4.6/extension_api.json",
        "c7a3f647d9a6d6e7f3361d8a88ffc7486a708b59262ab2f0ceae54a3d87df74d",
    ),
    (
        "4.7",
        "metadata/godot-4.7/extension_api.json",
        "c5dbd0c117e67f96bd9fef2c2e2023913e1d750d072ad2417a559e698211800b",
    ),
];

/// Parsed API plus the digest of the exact official JSON bytes.
#[derive(Debug)]
pub struct LoadedApi {
    /// Parsed official API.
    pub api: ExtensionApi,
    /// Lowercase SHA-256 of the raw input.
    pub sha256: String,
}

/// Failure while reading or parsing an official API JSON file.
#[derive(Debug)]
pub enum LoadError {
    /// File system failure.
    Io(std::io::Error),
    /// JSON syntax or type failure.
    Json(serde_json::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "failed to read API JSON: {error}"),
            Self::Json(error) => write!(formatter, "failed to parse API JSON: {error}"),
        }
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
        }
    }
}

impl From<std::io::Error> for LoadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for LoadError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Loads and hashes one exact official `extension_api.json`.
pub fn load_api(path: &Path) -> Result<LoadedApi, LoadError> {
    load_api_bytes(&fs::read(path)?)
}

/// Parses and hashes exact official API bytes.
pub fn load_api_bytes(bytes: &[u8]) -> Result<LoadedApi, LoadError> {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let api = serde_json::from_slice(bytes)?;
    Ok(LoadedApi { api, sha256 })
}

/// Returns the authenticated API input bundled for one Godot target.
pub fn bundled_api(target: &str) -> Result<(PathBuf, &'static str), String> {
    let (_, relative, sha256) = BUNDLED_APIS
        .iter()
        .find(|(version, _, _)| *version == target)
        .ok_or_else(|| format!("unsupported Godot API target `{target}`"))?;
    Ok((Path::new(env!("CARGO_MANIFEST_DIR")).join(relative), sha256))
}

/// Generates and verifies the complete high-level API for one bundled target.
pub fn generate_bundled_engine_api(
    target: &str,
) -> Result<(String, EngineApiGenerationReport), Box<dyn Error>> {
    let (path, expected_sha256) = bundled_api(target)?;
    let loaded = load_api(&path)?;
    if loaded.sha256 != expected_sha256 {
        return Err(format!(
            "bundled Godot {target} API digest mismatch: expected {expected_sha256}, found {}",
            loaded.sha256
        )
        .into());
    }
    let minor = target
        .strip_prefix("4.")
        .ok_or_else(|| format!("invalid Godot API target `{target}`"))?
        .parse::<u32>()?;
    let issues = validate_api(&loaded.api, ExpectedApiVersion { major: 4, minor });
    if !issues.is_empty() {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n- ");
        return Err(format!("bundled Godot {target} API validation failed:\n- {details}").into());
    }
    let report = analyze_engine_api(&loaded.api)?;
    verify_engine_api_coverage(&report)?;
    let source = generate_engine_api(&loaded.api, &loaded.sha256)?;
    Ok((source, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_api_is_authenticated_and_complete() {
        for target in ["4.4", "4.5", "4.6", "4.7"] {
            let (source, report) =
                generate_bundled_engine_api(target).expect("authenticated complete API");
            assert!(source.starts_with("// @generated by godot_codegen; DO NOT EDIT."));
            assert!(report.total_official_entries > 0);
            assert_eq!(report.methods_with_unsupported_types, 0);
            assert_eq!(report.unsupported_virtual_methods, 0);
        }
    }
}
