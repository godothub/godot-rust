use godot_api::{GodotApiVersion, GodotApiVersionError, SUPPORTED_API_TARGETS};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

/// Checked repository policy for Host and Native Extension API targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiTargetCatalog {
    host_baseline: GodotApiVersion,
    native_targets: Vec<GodotApiVersion>,
}

impl ApiTargetCatalog {
    /// Returns the single API baseline used to build the Script Mode Host.
    #[must_use]
    pub const fn host_baseline(&self) -> GodotApiVersion {
        self.host_baseline
    }

    /// Returns the independently generated Extension Mode targets.
    #[must_use]
    pub fn native_targets(&self) -> &[GodotApiVersion] {
        &self.native_targets
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTargetCatalog {
    schema_version: u32,
    host_baseline: String,
    native_targets: Vec<String>,
    source_repository: String,
    sources: BTreeMap<String, toml::Value>,
}

/// Failure while loading or validating `godot-api.toml`.
#[derive(Debug)]
pub enum TargetCatalogError {
    /// File system failure.
    Io(std::io::Error),
    /// TOML syntax or shape failure.
    Toml(toml::de::Error),
    /// Repository support-policy failure.
    Invalid(String),
}

impl fmt::Display for TargetCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "failed to read API target catalog: {error}"),
            Self::Toml(error) => write!(formatter, "failed to parse API target catalog: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid API target catalog: {message}"),
        }
    }
}

impl Error for TargetCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Toml(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<std::io::Error> for TargetCatalogError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<toml::de::Error> for TargetCatalogError {
    fn from(error: toml::de::Error) -> Self {
        Self::Toml(error)
    }
}

/// Loads and validates the repository's complete API target policy.
pub fn load_target_catalog(path: &Path) -> Result<ApiTargetCatalog, TargetCatalogError> {
    let source = fs::read_to_string(path)?;
    parse_target_catalog(&source)
}

fn parse_target_catalog(source: &str) -> Result<ApiTargetCatalog, TargetCatalogError> {
    let raw: RawTargetCatalog = toml::from_str(source)?;
    if raw.schema_version != 1 {
        return Err(TargetCatalogError::Invalid(format!(
            "unsupported schema version {}; expected 1",
            raw.schema_version
        )));
    }

    let host_baseline = raw
        .host_baseline
        .parse()
        .map_err(|error: GodotApiVersionError| TargetCatalogError::Invalid(error.to_string()))?;
    let native_targets = raw
        .native_targets
        .iter()
        .map(|value| {
            value.parse().map_err(|error: GodotApiVersionError| {
                TargetCatalogError::Invalid(error.to_string())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if host_baseline != GodotApiVersion::new(4, 4) {
        return Err(TargetCatalogError::Invalid(format!(
            "Script Mode Host baseline must be 4.4, found {host_baseline}"
        )));
    }
    if native_targets != SUPPORTED_API_TARGETS {
        let actual = native_targets
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(TargetCatalogError::Invalid(format!(
            "native targets must be exactly `4.4, 4.5, 4.6, 4.7` in ascending order, found `{actual}`"
        )));
    }
    if raw.source_repository != "https://github.com/godotengine/godot" {
        return Err(TargetCatalogError::Invalid(format!(
            "source repository must be the official Godot repository, found `{}`",
            raw.source_repository
        )));
    }
    let expected_sources = raw.native_targets.into_iter().collect::<Vec<_>>();
    let actual_sources = raw.sources.into_keys().collect::<Vec<_>>();
    if actual_sources != expected_sources {
        return Err(TargetCatalogError::Invalid(format!(
            "source entries must match native targets, found `{}`",
            actual_sources.join(", ")
        )));
    }

    Ok(ApiTargetCatalog {
        host_baseline,
        native_targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CATALOG: &str = r#"
schema_version = 1
host_baseline = "4.4"
native_targets = ["4.4", "4.5", "4.6", "4.7"]
source_repository = "https://github.com/godotengine/godot"

[sources."4.4"]
[sources."4.5"]
[sources."4.6"]
[sources."4.7"]
"#;

    #[test]
    fn complete_catalog_is_accepted() {
        let catalog = parse_target_catalog(VALID_CATALOG).expect("catalog should validate");
        assert_eq!(catalog.host_baseline(), GodotApiVersion::new(4, 4));
        assert_eq!(catalog.native_targets(), SUPPORTED_API_TARGETS);
    }

    #[test]
    fn missing_or_reordered_targets_are_rejected() {
        let missing = VALID_CATALOG.replace(", \"4.6\"", "");
        assert!(matches!(
            parse_target_catalog(&missing),
            Err(TargetCatalogError::Invalid(_))
        ));

        let reordered = VALID_CATALOG.replace(
            "[\"4.4\", \"4.5\", \"4.6\", \"4.7\"]",
            "[\"4.5\", \"4.4\", \"4.6\", \"4.7\"]",
        );
        assert!(matches!(
            parse_target_catalog(&reordered),
            Err(TargetCatalogError::Invalid(_))
        ));

        let missing_source = VALID_CATALOG.replace("[sources.\"4.6\"]\n", "");
        assert!(matches!(
            parse_target_catalog(&missing_source),
            Err(TargetCatalogError::Invalid(_))
        ));
    }
}
