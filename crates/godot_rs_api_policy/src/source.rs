use crate::{GodotApiVersion, SUPPORTED_API_TARGETS};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

const GODOT_REPOSITORY: &str = "https://github.com/godotengine/godot";

/// One immutable official file used as code-generator input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfficialFileSource {
    filename: String,
    url: Option<String>,
    acquisition: Option<String>,
    generator_version: Option<String>,
    sha256: String,
}

impl OfficialFileSource {
    /// Returns the expected local filename.
    #[must_use]
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Returns the exact official download URL, when the file is stored in Git.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Returns the official editor command used to generate the file, when applicable.
    #[must_use]
    pub fn acquisition(&self) -> Option<&str> {
        self.acquisition.as_deref()
    }

    /// Returns the exact official editor version used to generate the file.
    #[must_use]
    pub fn generator_version(&self) -> Option<&str> {
        self.generator_version.as_deref()
    }

    /// Returns the required lowercase SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Audited official API inputs for one Godot Major/Minor generation target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfficialApiSource {
    target: GodotApiVersion,
    godot_tag: String,
    gdextension_interface: OfficialFileSource,
    gdextension_interface_json: Option<OfficialFileSource>,
    extension_api: OfficialFileSource,
}

impl OfficialApiSource {
    /// Returns the generation target represented by this manifest.
    #[must_use]
    pub const fn target(&self) -> GodotApiVersion {
        self.target
    }

    /// Returns the immutable Godot source tag used by direct inputs.
    #[must_use]
    pub fn godot_tag(&self) -> &str {
        &self.godot_tag
    }

    /// Returns the C header consumed by Raw FFI generation.
    #[must_use]
    pub const fn gdextension_interface(&self) -> &OfficialFileSource {
        &self.gdextension_interface
    }

    /// Returns the low-level JSON source of truth used by Godot 4.6 and newer.
    #[must_use]
    pub const fn gdextension_interface_json(&self) -> Option<&OfficialFileSource> {
        self.gdextension_interface_json.as_ref()
    }

    /// Returns the high-level class and builtin API JSON.
    #[must_use]
    pub const fn extension_api(&self) -> &OfficialFileSource {
        &self.extension_api
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOfficialApiCatalog {
    schema_version: u32,
    host_baseline: String,
    native_targets: Vec<String>,
    source_repository: String,
    sources: BTreeMap<String, RawOfficialApiSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOfficialApiSource {
    godot_tag: String,
    gdextension_interface: RawOfficialFileSource,
    gdextension_interface_json: Option<RawOfficialFileSource>,
    extension_api: RawOfficialFileSource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOfficialFileSource {
    filename: String,
    url: Option<String>,
    acquisition: Option<String>,
    generator_version: Option<String>,
    sha256: String,
}

/// Failure while loading or authenticating an official API source.
#[derive(Debug)]
pub enum SourceManifestError {
    /// File system failure.
    Io(std::io::Error),
    /// TOML syntax or shape failure.
    Toml(toml::de::Error),
    /// Source provenance or policy failure.
    Invalid(String),
    /// The exact input bytes do not match their audited digest.
    HashMismatch {
        /// Human-readable input filename.
        filename: String,
        /// Digest committed in `godot-api.toml`.
        expected: String,
        /// Digest calculated from the supplied file.
        actual: String,
    },
}

impl fmt::Display for SourceManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "failed to read official API source: {error}"),
            Self::Toml(error) => write!(formatter, "failed to parse official API source: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid official API source: {message}"),
            Self::HashMismatch {
                filename,
                expected,
                actual,
            } => write!(
                formatter,
                "{filename} SHA-256 mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl Error for SourceManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Toml(error) => Some(error),
            Self::Invalid(_) | Self::HashMismatch { .. } => None,
        }
    }
}

impl From<std::io::Error> for SourceManifestError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<toml::de::Error> for SourceManifestError {
    fn from(error: toml::de::Error) -> Self {
        Self::Toml(error)
    }
}

/// Loads the official source catalog and selects the requested target.
pub fn load_official_api_source(
    path: &Path,
    expected: GodotApiVersion,
) -> Result<OfficialApiSource, SourceManifestError> {
    let source = fs::read_to_string(path)?;
    parse_official_api_source(&source, expected)
}

/// Verifies exact input bytes against their committed source digest.
pub fn verify_official_input(
    path: &Path,
    source: &OfficialFileSource,
) -> Result<(), SourceManifestError> {
    let bytes = fs::read(path)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != source.sha256 {
        return Err(SourceManifestError::HashMismatch {
            filename: source.filename.clone(),
            expected: source.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn parse_official_api_source(
    source: &str,
    expected: GodotApiVersion,
) -> Result<OfficialApiSource, SourceManifestError> {
    let mut catalog: RawOfficialApiCatalog = toml::from_str(source)?;
    let expected_targets = SUPPORTED_API_TARGETS
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if catalog.schema_version != 1 {
        return Err(SourceManifestError::Invalid(format!(
            "unsupported schema version {}; expected 1",
            catalog.schema_version
        )));
    }
    if catalog.host_baseline != "4.4" {
        return Err(SourceManifestError::Invalid(format!(
            "Script Mode Host baseline must be 4.4, found `{}`",
            catalog.host_baseline
        )));
    }
    if catalog.native_targets != expected_targets {
        return Err(SourceManifestError::Invalid(
            "native targets must be exactly 4.4 through 4.7 in ascending order".into(),
        ));
    }
    if catalog.source_repository != GODOT_REPOSITORY {
        return Err(SourceManifestError::Invalid(format!(
            "source repository must be `{GODOT_REPOSITORY}`, found `{}`",
            catalog.source_repository
        )));
    }
    let actual_sources = catalog.sources.keys().cloned().collect::<Vec<_>>();
    if actual_sources != expected_targets {
        return Err(SourceManifestError::Invalid(format!(
            "source entries must match native targets, found `{}`",
            actual_sources.join(", ")
        )));
    }
    let target_key = expected.to_string();
    let raw = catalog.sources.remove(&target_key).ok_or_else(|| {
        SourceManifestError::Invalid(format!(
            "official API source for Godot {expected} is missing"
        ))
    })?;
    let expected_tag = format!("{expected}-stable");
    if raw.godot_tag != expected_tag {
        return Err(SourceManifestError::Invalid(format!(
            "Godot tag must be `{expected_tag}`, found `{}`",
            raw.godot_tag
        )));
    }

    let tag_prefix = format!(
        "https://raw.githubusercontent.com/godotengine/godot/{}/",
        raw.godot_tag
    );
    let gdextension_interface = validate_file_source(
        "gdextension_interface",
        raw.gdextension_interface,
        "gdextension_interface.h",
        &tag_prefix,
    )?;
    let gdextension_interface_json = raw
        .gdextension_interface_json
        .map(|input| {
            validate_file_source(
                "gdextension_interface_json",
                input,
                "gdextension_interface.json",
                &tag_prefix,
            )
        })
        .transpose()?;
    let extension_api = validate_file_source(
        "extension_api",
        raw.extension_api,
        "extension_api.json",
        &tag_prefix,
    )?;

    if expected.minor() >= 6 && gdextension_interface_json.is_none() {
        return Err(SourceManifestError::Invalid(format!(
            "Godot {expected} requires the official gdextension_interface.json source of truth"
        )));
    }
    if expected.minor() < 6 && gdextension_interface.url().is_none() {
        return Err(SourceManifestError::Invalid(format!(
            "Godot {expected} must pin gdextension_interface.h directly from its official source tag"
        )));
    }
    if extension_api.acquisition().is_none() || extension_api.generator_version().is_none() {
        return Err(SourceManifestError::Invalid(
            "extension_api.json must record its editor acquisition command and generator version"
                .into(),
        ));
    }
    if !extension_api
        .generator_version()
        .is_some_and(|version| version.starts_with(&target_key))
    {
        return Err(SourceManifestError::Invalid(format!(
            "extension_api.json generator version must start with `{expected}`"
        )));
    }

    Ok(OfficialApiSource {
        target: expected,
        godot_tag: raw.godot_tag,
        gdextension_interface,
        gdextension_interface_json,
        extension_api,
    })
}

fn validate_file_source(
    label: &str,
    raw: RawOfficialFileSource,
    expected_filename: &str,
    official_tag_prefix: &str,
) -> Result<OfficialFileSource, SourceManifestError> {
    if raw.filename != expected_filename {
        return Err(SourceManifestError::Invalid(format!(
            "{label}.filename must be `{expected_filename}`, found `{}`",
            raw.filename
        )));
    }
    if raw.url.is_none() && raw.acquisition.is_none() {
        return Err(SourceManifestError::Invalid(format!(
            "{label} must provide an official URL or acquisition command"
        )));
    }
    if let Some(url) = raw.url.as_deref() {
        if !url.starts_with(official_tag_prefix) {
            return Err(SourceManifestError::Invalid(format!(
                "{label}.url must use the selected official Godot tag, found `{url}`"
            )));
        }
    }
    if !is_lowercase_sha256(&raw.sha256) {
        return Err(SourceManifestError::Invalid(format!(
            "{label}.sha256 must be exactly 64 lowercase hexadecimal characters"
        )));
    }
    if raw.acquisition.as_deref().is_some_and(str::is_empty) {
        return Err(SourceManifestError::Invalid(format!(
            "{label}.acquisition cannot be empty"
        )));
    }
    if raw.generator_version.as_deref().is_some_and(str::is_empty) {
        return Err(SourceManifestError::Invalid(format!(
            "{label}.generator_version cannot be empty"
        )));
    }

    Ok(OfficialFileSource {
        filename: raw.filename,
        url: raw.url,
        acquisition: raw.acquisition,
        generator_version: raw.generator_version,
        sha256: raw.sha256,
    })
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_entry(version: &str, header_origin: &str, interface_json: &str) -> String {
        format!(
            r#"
[sources."{version}"]
godot_tag = "{version}-stable"

[sources."{version}".gdextension_interface]
filename = "gdextension_interface.h"
{header_origin}
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

{interface_json}
[sources."{version}".extension_api]
filename = "extension_api.json"
acquisition = "godot --headless --dump-extension-api"
generator_version = "{version}.stable.official.example"
sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#
        )
    }

    fn manifest(version: &str, header_origin: &str, interface_json: &str) -> String {
        let mut entries = String::new();
        for target in ["4.4", "4.5", "4.6", "4.7"] {
            if target == version {
                entries.push_str(&source_entry(target, header_origin, interface_json));
            } else if target == "4.4" || target == "4.5" {
                entries.push_str(&source_entry(
                    target,
                    &format!(
                        r#"url = "https://raw.githubusercontent.com/godotengine/godot/{target}-stable/core/extension/gdextension_interface.h""#
                    ),
                    "",
                ));
            } else {
                entries.push_str(&source_entry(
                    target,
                    r#"acquisition = "godot --headless --dump-gdextension-interface""#,
                    &format!(
                        r#"
[sources."{target}".gdextension_interface_json]
filename = "gdextension_interface.json"
url = "https://raw.githubusercontent.com/godotengine/godot/{target}-stable/core/extension/gdextension_interface.json"
sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
"#
                    ),
                ));
            }
        }
        format!(
            r#"
schema_version = 1
host_baseline = "4.4"
native_targets = ["4.4", "4.5", "4.6", "4.7"]
source_repository = "https://github.com/godotengine/godot"
{entries}
"#
        )
    }

    #[test]
    fn legacy_header_and_generated_json_sources_validate() {
        let source_44 = manifest(
            "4.4",
            r#"url = "https://raw.githubusercontent.com/godotengine/godot/4.4-stable/core/extension/gdextension_interface.h""#,
            "",
        );
        let parsed = parse_official_api_source(&source_44, GodotApiVersion::new(4, 4))
            .expect("4.4 source should validate");
        assert!(parsed.gdextension_interface().url().is_some());
        assert!(parsed.gdextension_interface_json().is_none());

        let source_47 = manifest(
            "4.7",
            r#"acquisition = "godot --headless --dump-gdextension-interface""#,
            r#"
[sources."4.7".gdextension_interface_json]
filename = "gdextension_interface.json"
url = "https://raw.githubusercontent.com/godotengine/godot/4.7-stable/core/extension/gdextension_interface.json"
sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
"#,
        );
        let parsed = parse_official_api_source(&source_47, GodotApiVersion::new(4, 7))
            .expect("4.7 source should validate");
        assert!(parsed.gdextension_interface_json().is_some());
    }

    #[test]
    fn unofficial_urls_are_rejected_for_the_selected_target() {
        let source = manifest(
            "4.5",
            r#"url = "https://example.com/gdextension_interface.h""#,
            "",
        );
        assert!(matches!(
            parse_official_api_source(&source, GodotApiVersion::new(4, 5)),
            Err(SourceManifestError::Invalid(_))
        ));
    }

    #[test]
    fn newer_targets_require_low_level_json_source_of_truth() {
        let source = manifest(
            "4.6",
            r#"acquisition = "godot --headless --dump-gdextension-interface""#,
            "",
        );
        let error = parse_official_api_source(&source, GodotApiVersion::new(4, 6))
            .expect_err("4.6 source without interface JSON should fail");
        assert!(error.to_string().contains("source of truth"));
    }
}
