extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::error::Error;
use core::fmt;
use core::str::FromStr;

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Internal environment variable used to select one pre-generated Godot API.
pub const GODOT_API_ENV: &str = "GODOT_RS_GODOT";

/// Default Godot API target used when a project does not select one explicitly.
pub const DEFAULT_GODOT_API: GodotApiVersion = GodotApiVersion::new(4, 4);

/// Godot feature releases for which bindings can be generated.
pub const SUPPORTED_API_TARGETS: [GodotApiVersion; 4] = [
    GodotApiVersion::new(4, 4),
    GodotApiVersion::new(4, 5),
    GodotApiVersion::new(4, 6),
    GodotApiVersion::new(4, 7),
];

/// One Godot Major/Minor API generation target.
///
/// Patch versions are deliberately absent because Godot does not introduce a
/// new GDExtension API level in maintenance releases.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GodotApiVersion {
    major: u32,
    minor: u32,
}

impl GodotApiVersion {
    /// Creates a version without performing support-policy validation.
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Returns the major component.
    #[must_use]
    pub const fn major(self) -> u32 {
        self.major
    }

    /// Returns the minor component.
    #[must_use]
    pub const fn minor(self) -> u32 {
        self.minor
    }

    /// Returns whether this release is currently a supported generation target.
    #[must_use]
    pub fn is_supported(self) -> bool {
        SUPPORTED_API_TARGETS.contains(&self)
    }
}

impl fmt::Display for GodotApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for GodotApiVersion {
    type Err = GodotApiVersionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let components = value.split('.').collect::<Vec<_>>();
        if components.len() == 3 {
            return Err(GodotApiVersionError::PatchVersion(value.into()));
        }
        if components.len() != 2
            || components.iter().any(|component| {
                component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return Err(GodotApiVersionError::InvalidFormat(value.into()));
        }

        let version = Self::new(
            components[0]
                .parse()
                .map_err(|_| GodotApiVersionError::InvalidFormat(value.into()))?,
            components[1]
                .parse()
                .map_err(|_| GodotApiVersionError::InvalidFormat(value.into()))?,
        );
        if !version.is_supported() {
            return Err(GodotApiVersionError::Unsupported(version));
        }
        Ok(version)
    }
}

#[cfg(feature = "serde")]
impl Serialize for GodotApiVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for GodotApiVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Failure while parsing a user-facing Godot API target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GodotApiVersionError {
    /// The value was not exactly `Major.Minor`.
    InvalidFormat(String),
    /// A maintenance version was supplied where an API target was expected.
    PatchVersion(String),
    /// The syntactically valid target is outside the supported matrix.
    Unsupported(GodotApiVersion),
}

impl fmt::Display for GodotApiVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(value) => {
                write!(
                    formatter,
                    "invalid Godot API target `{value}`; expected `4.4` through `4.7`"
                )
            }
            Self::PatchVersion(value) => write!(
                formatter,
                "Godot API target `{value}` contains a patch version; use only its Major.Minor value"
            ),
            Self::Unsupported(version) => write!(
                formatter,
                "unsupported Godot API target `{version}`; supported targets are 4.4, 4.5, 4.6, and 4.7"
            ),
        }
    }
}

impl Error for GodotApiVersionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn supported_versions_parse_without_patch_components() {
        for expected in SUPPORTED_API_TARGETS {
            let parsed = expected
                .to_string()
                .parse::<GodotApiVersion>()
                .expect("supported target should parse");
            assert_eq!(parsed, expected);
        }
    }

    #[cfg(feature = "generator")]
    #[test]
    fn serde_uses_the_user_facing_major_minor_value() {
        #[derive(Debug, Deserialize, Serialize)]
        struct Wrapper {
            version: GodotApiVersion,
        }

        let encoded = serde_json::to_string(&Wrapper {
            version: GodotApiVersion::new(4, 6),
        })
        .expect("version should serialize");
        assert_eq!(encoded, r#"{"version":"4.6"}"#);
        let decoded =
            serde_json::from_str::<Wrapper>(&encoded).expect("version should deserialize");
        assert_eq!(decoded.version, GodotApiVersion::new(4, 6));
    }

    #[test]
    fn patch_and_unknown_versions_are_rejected() {
        assert!(matches!(
            "4.7.1".parse::<GodotApiVersion>(),
            Err(GodotApiVersionError::PatchVersion(_))
        ));
        assert!(matches!(
            "4.8".parse::<GodotApiVersion>(),
            Err(GodotApiVersionError::Unsupported(_))
        ));
        assert!(matches!(
            "v4.4".parse::<GodotApiVersion>(),
            Err(GodotApiVersionError::InvalidFormat(_))
        ));
    }
}
