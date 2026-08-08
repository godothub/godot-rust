use crate::NativeBuildPlan;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeOperatingSystem {
    Android,
    Ios,
    Linux,
    Macos,
    Web,
    Windows,
}

impl NativeOperatingSystem {
    const fn godot_feature(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Web => "web",
            Self::Windows => "windows",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeArchitecture {
    X86_64,
    Arm64,
    X86_32,
    Arm32,
    Universal,
    Wasm32,
}

impl NativeArchitecture {
    const fn godot_feature(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm64 => "arm64",
            Self::X86_32 => "x86_32",
            Self::Arm32 => "arm32",
            Self::Universal => "universal",
            Self::Wasm32 => "wasm32",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeBuildProfile {
    Debug,
    Release,
}

impl NativeBuildProfile {
    const fn godot_feature(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct NativePlatform {
    pub operating_system: NativeOperatingSystem,
    pub architecture: NativeArchitecture,
    pub profile: NativeBuildProfile,
}

impl NativePlatform {
    pub fn current_debug() -> Result<Self, String> {
        Self::from_os_and_arch(
            std::env::consts::OS,
            std::env::consts::ARCH,
            NativeBuildProfile::Debug,
        )
    }

    pub fn from_rust_target_triple(
        target: &str,
        profile: NativeBuildProfile,
    ) -> Result<Self, String> {
        let architecture = target
            .split('-')
            .next()
            .ok_or_else(|| "Rust target triple is empty".to_owned())?;
        let operating_system =
            if target.ends_with("-linux-android") || target.ends_with("-linux-androideabi") {
                "android"
            } else if target.contains("windows") {
                "windows"
            } else if matches!(
                target,
                "aarch64-apple-ios" | "aarch64-apple-ios-sim" | "x86_64-apple-ios"
            ) {
                "ios"
            } else if target.contains("linux") {
                "linux"
            } else if target.contains("darwin") {
                "macos"
            } else if target == "wasm32-unknown-emscripten" {
                "web"
            } else {
                return Err(format!(
                    "Native Extension publication does not support Rust target `{target}`"
                ));
            };
        Self::from_os_and_arch(operating_system, architecture, profile)
    }

    fn from_os_and_arch(
        operating_system: &str,
        architecture: &str,
        profile: NativeBuildProfile,
    ) -> Result<Self, String> {
        let operating_system = match operating_system {
            "android" => NativeOperatingSystem::Android,
            "ios" => NativeOperatingSystem::Ios,
            "linux" => NativeOperatingSystem::Linux,
            "macos" => NativeOperatingSystem::Macos,
            "web" => NativeOperatingSystem::Web,
            "windows" => NativeOperatingSystem::Windows,
            other => {
                return Err(format!(
                    "Native Extension publication does not support operating system `{other}`"
                ));
            }
        };
        let architecture = match architecture {
            "x86_64" => NativeArchitecture::X86_64,
            "aarch64" => NativeArchitecture::Arm64,
            "x86" | "i686" => NativeArchitecture::X86_32,
            "arm" | "armv7" => NativeArchitecture::Arm32,
            "universal" => NativeArchitecture::Universal,
            "wasm32" => NativeArchitecture::Wasm32,
            other => {
                return Err(format!(
                    "Native Extension publication does not support architecture `{other}`"
                ));
            }
        };
        Ok(Self {
            operating_system,
            architecture,
            profile,
        })
    }

    pub fn selector(self) -> String {
        if self.operating_system == NativeOperatingSystem::Ios {
            return format!(
                "{}.{}",
                self.operating_system.godot_feature(),
                self.profile.godot_feature()
            );
        }
        format!(
            "{}.{}.{}",
            self.operating_system.godot_feature(),
            self.profile.godot_feature(),
            self.architecture.godot_feature()
        )
    }

    pub fn from_export_target(
        operating_system: crate::ExportOperatingSystem,
        architecture: crate::ExportArchitecture,
        profile: crate::ExportProfile,
    ) -> Self {
        let operating_system = match operating_system {
            crate::ExportOperatingSystem::Android => NativeOperatingSystem::Android,
            crate::ExportOperatingSystem::Ios => NativeOperatingSystem::Ios,
            crate::ExportOperatingSystem::Linux => NativeOperatingSystem::Linux,
            crate::ExportOperatingSystem::Macos => NativeOperatingSystem::Macos,
            crate::ExportOperatingSystem::Web => NativeOperatingSystem::Web,
            crate::ExportOperatingSystem::Windows => NativeOperatingSystem::Windows,
        };
        let architecture = match architecture {
            crate::ExportArchitecture::Arm32 => NativeArchitecture::Arm32,
            crate::ExportArchitecture::Arm64 => NativeArchitecture::Arm64,
            crate::ExportArchitecture::Universal => NativeArchitecture::Universal,
            crate::ExportArchitecture::Wasm32 => NativeArchitecture::Wasm32,
            crate::ExportArchitecture::X86_32 => NativeArchitecture::X86_32,
            crate::ExportArchitecture::X86_64 => NativeArchitecture::X86_64,
        };
        let profile = match profile {
            crate::ExportProfile::Debug => NativeBuildProfile::Debug,
            crate::ExportProfile::Release => NativeBuildProfile::Release,
        };
        Self {
            operating_system,
            architecture,
            profile,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NativeExtensionDescriptor {
    pub entry_symbol: String,
    pub compatibility_minimum: String,
    pub reloadable: bool,
    pub libraries: BTreeMap<String, String>,
}

impl NativeExtensionDescriptor {
    pub fn from_plan(plan: &NativeBuildPlan) -> Self {
        Self {
            entry_symbol: plan.entry_symbol.clone(),
            compatibility_minimum: plan.compatibility_minimum.clone(),
            reloadable: true,
            libraries: BTreeMap::new(),
        }
    }

    pub fn set_library(
        &mut self,
        platform: NativePlatform,
        resource_path: impl Into<String>,
    ) -> Result<(), String> {
        let resource_path = resource_path.into();
        validate_resource_path(&resource_path)?;
        self.libraries.insert(platform.selector(), resource_path);
        Ok(())
    }

    pub fn render(&self) -> Result<String, String> {
        if self.entry_symbol.is_empty()
            || !self
                .entry_symbol
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            return Err("Native entry symbol must be a non-empty C identifier".to_owned());
        }
        if self
            .compatibility_minimum
            .parse::<godot_api::GodotApiVersion>()
            .is_err()
        {
            return Err(format!(
                "Native compatibility minimum is invalid: {}",
                self.compatibility_minimum
            ));
        }
        if self.libraries.is_empty() {
            return Err("Native `.gdextension` requires at least one platform library".to_owned());
        }

        let mut output = String::new();
        output.push_str("[configuration]\n\n");
        output.push_str(&format!(
            "entry_symbol = \"{}\"\n",
            escape_config_string(&self.entry_symbol)
        ));
        output.push_str(&format!(
            "compatibility_minimum = \"{}\"\n",
            escape_config_string(&self.compatibility_minimum)
        ));
        output.push_str(if self.reloadable {
            "reloadable = true\n"
        } else {
            "reloadable = false\n"
        });
        output.push_str("\n[libraries]\n");
        for (selector, resource_path) in &self.libraries {
            validate_selector(selector)?;
            validate_resource_path(resource_path)?;
            output.push_str(&format!(
                "\n{selector} = \"{}\"\n",
                escape_config_string(resource_path)
            ));
        }
        Ok(output)
    }
}

fn validate_selector(selector: &str) -> Result<(), String> {
    if selector.is_empty()
        || !selector
            .bytes()
            .all(|byte| byte == b'_' || byte == b'.' || byte.is_ascii_alphanumeric())
    {
        return Err(format!(
            "Native platform selector contains unsupported characters: `{selector}`"
        ));
    }
    Ok(())
}

fn validate_resource_path(path: &str) -> Result<(), String> {
    if !path.starts_with("res://")
        || path.contains('\\')
        || path.contains('\n')
        || path.contains('\r')
        || path.split('/').any(|component| component == "..")
    {
        return Err(format!(
            "Native library path must be a safe `res://` resource path: `{path}`"
        ));
    }
    Ok(())
}

fn escape_config_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CargoPackageModel, CargoTargetModel, GodotRustMode};
    use godot_api::GodotApiVersion;
    use std::path::PathBuf;

    fn plan() -> NativeBuildPlan {
        let target = CargoTargetModel {
            name: "godothub_project".to_owned(),
            src_path: PathBuf::from("/game/src/lib.rs"),
        };
        NativeBuildPlan {
            project_root: PathBuf::from("/game"),
            package: CargoPackageModel {
                id: "godothub_project 0.1.0".to_owned(),
                name: "godothub_project".to_owned(),
                manifest_path: PathBuf::from("/game/Cargo.toml"),
                workspace_default: true,
                godot_rs_dependency: true,
                godot_rust_enabled: true,
                godot_rust_mode: Some(GodotRustMode::Extension),
                godot_api: Some(GodotApiVersion::new(4, 6)),
                scripts_path: None,
                editor: crate::EditorWorkflowConfig::default(),
                script_mode_configured: false,
                configuration_issues: Vec::new(),
                cdylib_targets: vec![target.clone()],
            },
            target,
            godot_api: GodotApiVersion::new(4, 6),
            cargo_environment: BTreeMap::from([("GODOT_RS_GODOT".to_owned(), "4.6".to_owned())]),
            entry_symbol: "godot_rs_native_init".to_owned(),
            compatibility_minimum: "4.6".to_owned(),
        }
    }

    #[test]
    fn rust_target_triples_map_to_exact_godot_features() {
        assert_eq!(
            NativePlatform::from_rust_target_triple(
                "x86_64-unknown-linux-gnu",
                NativeBuildProfile::Debug
            )
            .expect("Linux target")
            .selector(),
            "linux.debug.x86_64"
        );
        assert_eq!(
            NativePlatform::from_rust_target_triple(
                "aarch64-apple-darwin",
                NativeBuildProfile::Release
            )
            .expect("macOS target")
            .selector(),
            "macos.release.arm64"
        );
        assert_eq!(
            NativePlatform::from_rust_target_triple(
                "x86_64-pc-windows-msvc",
                NativeBuildProfile::Release
            )
            .expect("Windows target")
            .selector(),
            "windows.release.x86_64"
        );
        assert_eq!(
            NativePlatform::from_rust_target_triple(
                "aarch64-linux-android",
                NativeBuildProfile::Release
            )
            .expect("Android target")
            .selector(),
            "android.release.arm64"
        );
        assert_eq!(
            NativePlatform::from_rust_target_triple(
                "aarch64-apple-ios",
                NativeBuildProfile::Release
            )
            .expect("iOS target")
            .selector(),
            "ios.release"
        );
        assert_eq!(
            NativePlatform::from_rust_target_triple(
                "wasm32-unknown-emscripten",
                NativeBuildProfile::Debug
            )
            .expect("Web target")
            .selector(),
            "web.debug.wasm32"
        );
    }

    #[test]
    fn ios_xcframework_selector_covers_device_and_simulator_architectures() {
        for architecture in [NativeArchitecture::Arm64, NativeArchitecture::X86_64] {
            assert_eq!(
                NativePlatform {
                    operating_system: NativeOperatingSystem::Ios,
                    architecture,
                    profile: NativeBuildProfile::Debug,
                }
                .selector(),
                "ios.debug"
            );
        }
    }

    #[test]
    fn descriptor_uses_a_minimum_and_never_emits_a_maximum() {
        let mut descriptor = NativeExtensionDescriptor::from_plan(&plan());
        descriptor
            .set_library(
                NativePlatform {
                    operating_system: NativeOperatingSystem::Linux,
                    architecture: NativeArchitecture::X86_64,
                    profile: NativeBuildProfile::Debug,
                },
                "res://rust/bin/libgame.so",
            )
            .expect("library mapping");
        let output = descriptor.render().expect("descriptor");
        assert!(output.contains("entry_symbol = \"godot_rs_native_init\""));
        assert!(output.contains("compatibility_minimum = \"4.6\""));
        assert!(output.contains("reloadable = true"));
        assert!(!output.contains("compatibility_maximum"));
        assert!(output.contains("linux.debug.x86_64 = \"res://rust/bin/libgame.so\""));
    }

    #[test]
    fn unsafe_resource_paths_and_empty_mappings_are_rejected() {
        let mut descriptor = NativeExtensionDescriptor::from_plan(&plan());
        assert!(
            descriptor
                .set_library(
                    NativePlatform::current_debug().expect("test platform"),
                    "res://rust/../escape.so"
                )
                .is_err()
        );
        assert!(descriptor.render().is_err());
    }

    #[test]
    fn every_export_platform_maps_to_the_descriptor_feature_vocabulary() {
        let cases = [
            (
                crate::ExportOperatingSystem::Android,
                crate::ExportArchitecture::Arm32,
                "android.release.arm32",
            ),
            (
                crate::ExportOperatingSystem::Ios,
                crate::ExportArchitecture::Arm64,
                "ios.release",
            ),
            (
                crate::ExportOperatingSystem::Linux,
                crate::ExportArchitecture::X86_64,
                "linux.release.x86_64",
            ),
            (
                crate::ExportOperatingSystem::Macos,
                crate::ExportArchitecture::Universal,
                "macos.release.universal",
            ),
            (
                crate::ExportOperatingSystem::Web,
                crate::ExportArchitecture::Wasm32,
                "web.release.wasm32",
            ),
            (
                crate::ExportOperatingSystem::Windows,
                crate::ExportArchitecture::X86_32,
                "windows.release.x86_32",
            ),
        ];
        for (operating_system, architecture, expected) in cases {
            assert_eq!(
                NativePlatform::from_export_target(
                    operating_system,
                    architecture,
                    crate::ExportProfile::Release,
                )
                .selector(),
                expected
            );
        }
    }
}
