use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportOperatingSystem {
    Android,
    Ios,
    Linux,
    Macos,
    Web,
    Windows,
}

impl ExportOperatingSystem {
    fn from_godot_name(name: &str) -> Result<Self, String> {
        match name {
            "Android" => Ok(Self::Android),
            "iOS" => Ok(Self::Ios),
            "Linux" => Ok(Self::Linux),
            "macOS" => Ok(Self::Macos),
            "Web" => Ok(Self::Web),
            "Windows" => Ok(Self::Windows),
            other => Err(format!(
                "godot-rust does not support Rust project export for Godot platform `{other}`"
            )),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Web => "web",
            Self::Windows => "windows",
        }
    }

    const fn is_desktop(self) -> bool {
        matches!(self, Self::Linux | Self::Macos | Self::Windows)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportArchitecture {
    Arm32,
    Arm64,
    Universal,
    Wasm32,
    X86_32,
    X86_64,
}

impl ExportArchitecture {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Arm32 => "arm32",
            Self::Arm64 => "arm64",
            Self::Universal => "universal",
            Self::Wasm32 => "wasm32",
            Self::X86_32 => "x86_32",
            Self::X86_64 => "x86_64",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportProfile {
    Debug,
    Release,
}

impl ExportProfile {
    pub fn is_release(self) -> bool {
        self == Self::Release
    }

    pub fn directory_name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExportArtifactTarget {
    pub architecture: ExportArchitecture,
    pub rust_targets: Vec<String>,
    pub module_file: String,
    pub godot_tags: Vec<String>,
    pub staging_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectExportTarget {
    pub operating_system: ExportOperatingSystem,
    pub profile: ExportProfile,
    pub artifacts: Vec<ExportArtifactTarget>,
}

impl ProjectExportTarget {
    pub fn from_godot(platform: &str, features: &[String], is_debug: bool) -> Result<Self, String> {
        let operating_system = ExportOperatingSystem::from_godot_name(platform)?;
        let profile = if is_debug {
            ExportProfile::Debug
        } else {
            ExportProfile::Release
        };
        let artifacts = select_architectures(operating_system, features)?
            .into_iter()
            .map(|architecture| {
                let rust_targets = match (operating_system, architecture) {
                    (ExportOperatingSystem::Ios, ExportArchitecture::Arm64) => vec![
                        "aarch64-apple-ios".to_owned(),
                        "aarch64-apple-ios-sim".to_owned(),
                        "x86_64-apple-ios".to_owned(),
                    ],
                    (_, ExportArchitecture::Universal) => vec![
                        rust_target(operating_system, ExportArchitecture::X86_64)?.to_owned(),
                        rust_target(operating_system, ExportArchitecture::Arm64)?.to_owned(),
                    ],
                    _ => vec![rust_target(operating_system, architecture)?.to_owned()],
                };
                let module_file = module_file(operating_system).to_owned();
                let staging_key = if operating_system == ExportOperatingSystem::Ios {
                    "ios-xcframework".to_owned()
                } else {
                    format!("{}-{}", operating_system.name(), architecture.name())
                };
                let godot_tags = match architecture {
                    ExportArchitecture::Universal => vec!["universal".to_owned()],
                    architecture => vec![architecture.name().to_owned()],
                };
                Ok(ExportArtifactTarget {
                    architecture,
                    rust_targets,
                    module_file,
                    godot_tags,
                    staging_key,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            operating_system,
            profile,
            artifacts,
        })
    }

    pub fn ensure_host_compatible(&self) -> Result<(), String> {
        self.ensure_compatible_with(std::env::consts::OS)
    }

    fn ensure_compatible_with(&self, host_os: &str) -> Result<(), String> {
        let compatible = match self.operating_system {
            ExportOperatingSystem::Ios | ExportOperatingSystem::Macos => host_os == "macos",
            operating_system if operating_system.is_desktop() => host_os == operating_system.name(),
            ExportOperatingSystem::Android | ExportOperatingSystem::Web => {
                matches!(host_os, "linux" | "macos" | "windows")
            }
            _ => false,
        };
        if compatible {
            return Ok(());
        }

        let requirement = match self.operating_system {
            ExportOperatingSystem::Ios => "iOS export requires a macOS editor host with Xcode",
            ExportOperatingSystem::Macos => "macOS export requires a macOS editor host",
            operating_system if operating_system.is_desktop() => {
                return Err(format!(
                    "Rust project export for {} must run on the same operating system; \
                     this editor runs on {host_os}",
                    operating_system.name()
                ));
            }
            _ => "Rust mobile and Web export requires a supported desktop editor host",
        };
        Err(format!("{requirement}; this editor runs on {host_os}"))
    }

    pub const fn needs_native_semantic_validation(&self) -> bool {
        !self.operating_system.is_desktop()
    }
}

fn select_architectures(
    operating_system: ExportOperatingSystem,
    features: &[String],
) -> Result<Vec<ExportArchitecture>, String> {
    let has = |name: &str| features.iter().any(|feature| feature == name);
    if operating_system == ExportOperatingSystem::Macos && has("universal") {
        if !has("x86_64") || !has("arm64") {
            return Err(
                "Godot's macOS Universal export is missing x86_64 or arm64 feature tags".to_owned(),
            );
        }
        return Ok(vec![ExportArchitecture::Universal]);
    }

    let candidates = match operating_system {
        ExportOperatingSystem::Android => &[
            ("arm32", ExportArchitecture::Arm32),
            ("arm64", ExportArchitecture::Arm64),
            ("x86_32", ExportArchitecture::X86_32),
            ("x86_64", ExportArchitecture::X86_64),
        ][..],
        ExportOperatingSystem::Ios => &[("arm64", ExportArchitecture::Arm64)][..],
        ExportOperatingSystem::Linux => &[
            ("arm32", ExportArchitecture::Arm32),
            ("arm64", ExportArchitecture::Arm64),
            ("x86_32", ExportArchitecture::X86_32),
            ("x86_64", ExportArchitecture::X86_64),
        ][..],
        ExportOperatingSystem::Macos => &[
            ("arm64", ExportArchitecture::Arm64),
            ("x86_64", ExportArchitecture::X86_64),
        ][..],
        ExportOperatingSystem::Web => &[("wasm32", ExportArchitecture::Wasm32)][..],
        ExportOperatingSystem::Windows => &[
            ("arm64", ExportArchitecture::Arm64),
            ("x86_32", ExportArchitecture::X86_32),
            ("x86_64", ExportArchitecture::X86_64),
        ][..],
    };
    let selected = candidates
        .iter()
        .filter_map(|(name, architecture)| has(name).then_some(*architecture))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(format!(
            "Godot export platform `{}` has no supported Rust architecture feature",
            operating_system.name()
        ));
    }
    if operating_system != ExportOperatingSystem::Android && selected.len() != 1 {
        return Err(format!(
            "Godot export platform `{}` selected multiple non-Universal architectures",
            operating_system.name()
        ));
    }
    Ok(selected)
}

fn rust_target(
    operating_system: ExportOperatingSystem,
    architecture: ExportArchitecture,
) -> Result<&'static str, String> {
    match (operating_system, architecture) {
        (ExportOperatingSystem::Android, ExportArchitecture::Arm32) => {
            Ok("armv7-linux-androideabi")
        }
        (ExportOperatingSystem::Android, ExportArchitecture::Arm64) => Ok("aarch64-linux-android"),
        (ExportOperatingSystem::Android, ExportArchitecture::X86_32) => Ok("i686-linux-android"),
        (ExportOperatingSystem::Android, ExportArchitecture::X86_64) => Ok("x86_64-linux-android"),
        (ExportOperatingSystem::Ios, ExportArchitecture::Arm64) => Ok("aarch64-apple-ios"),
        (ExportOperatingSystem::Linux, ExportArchitecture::Arm32) => {
            Ok("armv7-unknown-linux-gnueabihf")
        }
        (ExportOperatingSystem::Linux, ExportArchitecture::Arm64) => {
            Ok("aarch64-unknown-linux-gnu")
        }
        (ExportOperatingSystem::Linux, ExportArchitecture::X86_32) => Ok("i686-unknown-linux-gnu"),
        (ExportOperatingSystem::Linux, ExportArchitecture::X86_64) => {
            Ok("x86_64-unknown-linux-gnu")
        }
        (ExportOperatingSystem::Macos, ExportArchitecture::X86_64) => Ok("x86_64-apple-darwin"),
        (ExportOperatingSystem::Macos, ExportArchitecture::Arm64) => Ok("aarch64-apple-darwin"),
        (ExportOperatingSystem::Web, ExportArchitecture::Wasm32) => Ok("wasm32-unknown-emscripten"),
        (ExportOperatingSystem::Windows, ExportArchitecture::X86_32) => Ok("i686-pc-windows-msvc"),
        (ExportOperatingSystem::Windows, ExportArchitecture::X86_64) => {
            Ok("x86_64-pc-windows-msvc")
        }
        (ExportOperatingSystem::Windows, ExportArchitecture::Arm64) => {
            Ok("aarch64-pc-windows-msvc")
        }
        _ => Err("Godot export architecture is not supported by godot-rust".to_owned()),
    }
}

fn module_file(operating_system: ExportOperatingSystem) -> &'static str {
    match operating_system {
        ExportOperatingSystem::Android | ExportOperatingSystem::Linux => {
            "libgodot_rs_project_module.so"
        }
        ExportOperatingSystem::Ios => "godot_rs_project_module.xcframework",
        ExportOperatingSystem::Macos => "libgodot_rs_project_module.dylib",
        ExportOperatingSystem::Web => "godot_rs_project_module.wasm",
        ExportOperatingSystem::Windows => "godot_rs_project_module.dll",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn every_public_export_platform_maps_to_canonical_rust_targets() {
        let cases = [
            (
                "Linux",
                vec!["linux", "x86_64"],
                vec!["x86_64-unknown-linux-gnu"],
            ),
            (
                "Windows",
                vec!["windows", "arm64"],
                vec!["aarch64-pc-windows-msvc"],
            ),
            (
                "Android",
                vec!["android", "arm32", "arm64", "x86_32", "x86_64"],
                vec![
                    "armv7-linux-androideabi",
                    "aarch64-linux-android",
                    "i686-linux-android",
                    "x86_64-linux-android",
                ],
            ),
            (
                "iOS",
                vec!["ios", "arm64"],
                vec![
                    "aarch64-apple-ios",
                    "aarch64-apple-ios-sim",
                    "x86_64-apple-ios",
                ],
            ),
            (
                "Web",
                vec!["web", "wasm32"],
                vec!["wasm32-unknown-emscripten"],
            ),
        ];
        for (platform, feature_values, expected) in cases {
            let target =
                ProjectExportTarget::from_godot(platform, &features(&feature_values), false)
                    .unwrap_or_else(|error| panic!("{platform}: {error}"));
            assert_eq!(
                target
                    .artifacts
                    .iter()
                    .flat_map(|artifact| artifact.rust_targets.iter().map(String::as_str))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn android_keeps_each_enabled_abi_as_a_separate_shared_object() {
        let target = ProjectExportTarget::from_godot(
            "Android",
            &features(&["android", "arm64", "x86_64"]),
            true,
        )
        .expect("Android target");
        assert_eq!(target.artifacts.len(), 2);
        assert_eq!(target.artifacts[0].godot_tags, ["arm64"]);
        assert_eq!(target.artifacts[1].godot_tags, ["x86_64"]);
        assert!(
            target
                .artifacts
                .iter()
                .all(|artifact| artifact.module_file == "libgodot_rs_project_module.so")
        );
    }

    #[test]
    fn macos_universal_expands_to_both_cargo_targets() {
        let target = ProjectExportTarget::from_godot(
            "macOS",
            &features(&["macos", "universal", "x86_64", "arm64"]),
            false,
        )
        .expect("Universal target");
        let artifact = &target.artifacts[0];
        assert_eq!(artifact.architecture, ExportArchitecture::Universal);
        assert_eq!(artifact.staging_key, "macos-universal");
        assert_eq!(artifact.godot_tags, ["universal"]);

        assert!(
            ProjectExportTarget::from_godot(
                "macOS",
                &features(&["macos", "universal", "arm64"]),
                false,
            )
            .expect_err("incomplete Universal features")
            .contains("missing x86_64 or arm64")
        );
    }

    #[test]
    fn ios_builds_device_and_simulator_xcframework_slices() {
        let target = ProjectExportTarget::from_godot("iOS", &features(&["ios", "arm64"]), false)
            .expect("iOS target");
        let artifact = &target.artifacts[0];
        assert_eq!(artifact.architecture, ExportArchitecture::Arm64);
        assert_eq!(artifact.staging_key, "ios-xcframework");
        assert_eq!(
            artifact.rust_targets,
            [
                "aarch64-apple-ios",
                "aarch64-apple-ios-sim",
                "x86_64-apple-ios"
            ]
        );
        assert_eq!(artifact.module_file, "godot_rs_project_module.xcframework");
    }

    #[test]
    fn host_compatibility_allows_cross_toolchains_only_where_supported() {
        let web = ProjectExportTarget::from_godot("Web", &features(&["web", "wasm32"]), false)
            .expect("Web target");
        assert!(web.ensure_compatible_with("linux").is_ok());
        assert!(web.ensure_compatible_with("macos").is_ok());
        assert!(web.ensure_compatible_with("windows").is_ok());

        let ios = ProjectExportTarget::from_godot("iOS", &features(&["ios", "arm64"]), false)
            .expect("iOS target");
        assert!(ios.ensure_compatible_with("macos").is_ok());
        assert!(ios.ensure_compatible_with("linux").is_err());

        let linux = ProjectExportTarget::from_godot("Linux", &features(&["linux", "arm64"]), false)
            .expect("Linux target");
        assert!(linux.ensure_compatible_with("linux").is_ok());
        assert!(linux.ensure_compatible_with("windows").is_err());
    }
}
