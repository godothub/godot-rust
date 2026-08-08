#![cfg_attr(not(feature = "generator"), no_std)]
#![doc = "Versioned Godot APIs and the stable project-module ABI used by godot-rust."]

#[cfg(feature = "generator")]
mod generator;
#[cfg(feature = "generator")]
#[doc(hidden)]
pub use generator::*;

/// Stable project-module ABI shared by the Script Host and Rust SDK.
pub mod abi;

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all
)]
pub mod versions {
    /// Raw Godot 4.4 ABI used by the Script Mode Host baseline.
    pub mod godot_4_4 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.4/raw.rs"
        ));
    }

    /// Raw Godot 4.5 ABI for Extension Mode.
    #[cfg(godot_rs_api_version = "4.5")]
    pub mod godot_4_5 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.5/raw.rs"
        ));
    }

    /// Raw Godot 4.6 ABI for Extension Mode.
    #[cfg(godot_rs_api_version = "4.6")]
    pub mod godot_4_6 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.6/raw.rs"
        ));
    }

    /// Raw Godot 4.7 ABI for Extension Mode.
    #[cfg(godot_rs_api_version = "4.7")]
    pub mod godot_4_7 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.7/raw.rs"
        ));
    }
}

/// Versioned summaries generated from each authenticated official API dump.
pub mod api_snapshots {
    /// Godot 4.4 API metadata.
    #[cfg(godot_rs_api_version = "4.4")]
    pub mod godot_4_4 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.4/api_snapshot.rs"
        ));
    }

    /// Godot 4.5 API metadata.
    #[cfg(godot_rs_api_version = "4.5")]
    pub mod godot_4_5 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.5/api_snapshot.rs"
        ));
    }

    /// Godot 4.6 API metadata.
    #[cfg(godot_rs_api_version = "4.6")]
    pub mod godot_4_6 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.6/api_snapshot.rs"
        ));
    }

    /// Godot 4.7 API metadata.
    #[cfg(godot_rs_api_version = "4.7")]
    pub mod godot_4_7 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/generated/godot-4.7/api_snapshot.rs"
        ));
    }
}

/// Godot Major/Minor selected for this SDK build.
pub const SELECTED_GODOT_API: &str = env!("GODOT_RS_API_GODOT");
/// Major component of [`SELECTED_GODOT_API`].
pub const SELECTED_GODOT_API_MAJOR: u32 = 4;
/// Minor component of [`SELECTED_GODOT_API`].
#[cfg(godot_rs_api_version = "4.4")]
pub const SELECTED_GODOT_API_MINOR: u32 = 4;
/// Minor component of [`SELECTED_GODOT_API`].
#[cfg(godot_rs_api_version = "4.5")]
pub const SELECTED_GODOT_API_MINOR: u32 = 5;
/// Minor component of [`SELECTED_GODOT_API`].
#[cfg(godot_rs_api_version = "4.6")]
pub const SELECTED_GODOT_API_MINOR: u32 = 6;
/// Minor component of [`SELECTED_GODOT_API`].
#[cfg(godot_rs_api_version = "4.7")]
pub const SELECTED_GODOT_API_MINOR: u32 = 7;
/// Godot Major/Minor selected for the Native Extension bindings.
pub const NATIVE_GODOT_API: &str = SELECTED_GODOT_API;
/// Stable Native GDExtension entry symbol shared with generated descriptors.
pub const NATIVE_ENTRY_SYMBOL: &str = "godot_rs_native_init";

/// Raw ABI selected for one Native Extension build, plus stable aliases for
/// interfaces whose official version suffix changes between Godot releases.
pub mod native {
    #[cfg(godot_rs_api_version = "4.4")]
    pub use super::versions::godot_4_4::*;
    #[cfg(godot_rs_api_version = "4.5")]
    pub use super::versions::godot_4_5::*;
    #[cfg(godot_rs_api_version = "4.6")]
    pub use super::versions::godot_4_6::*;
    #[cfg(godot_rs_api_version = "4.7")]
    pub use super::versions::godot_4_7::*;

    #[cfg(godot_rs_api_version = "4.4")]
    pub type GDExtensionClassCreationInfo = GDExtensionClassCreationInfo4;
    #[cfg(any(godot_rs_api_version = "4.5", godot_rs_api_version = "4.6"))]
    pub type GDExtensionClassCreationInfo = GDExtensionClassCreationInfo5;
    #[cfg(godot_rs_api_version = "4.7")]
    pub type GDExtensionClassCreationInfo = GDExtensionClassCreationInfo6;

    pub type GDExtensionClassdbRegisterExtensionClass = unsafe extern "C" fn(
        GDExtensionClassLibraryPtr,
        GDExtensionConstStringNamePtr,
        GDExtensionConstStringNamePtr,
        *const GDExtensionClassCreationInfo,
    );

    #[cfg(godot_rs_api_version = "4.4")]
    pub const CLASSDB_REGISTER_EXTENSION_CLASS_SYMBOL: &[u8] =
        b"classdb_register_extension_class4\0";
    #[cfg(any(godot_rs_api_version = "4.5", godot_rs_api_version = "4.6"))]
    pub const CLASSDB_REGISTER_EXTENSION_CLASS_SYMBOL: &[u8] =
        b"classdb_register_extension_class5\0";
    #[cfg(godot_rs_api_version = "4.7")]
    pub const CLASSDB_REGISTER_EXTENSION_CLASS_SYMBOL: &[u8] =
        b"classdb_register_extension_class6\0";
}

// Keep the fixed 4.4 top-level API consumed by the Script Mode Host.
pub use versions::godot_4_4::*;

/// Generated summary selected for the current Native Extension build.
#[cfg(godot_rs_api_version = "4.4")]
pub use api_snapshots::godot_4_4 as api_snapshot;
#[cfg(godot_rs_api_version = "4.5")]
pub use api_snapshots::godot_4_5 as api_snapshot;
#[cfg(godot_rs_api_version = "4.6")]
pub use api_snapshots::godot_4_6 as api_snapshot;
#[cfg(godot_rs_api_version = "4.7")]
pub use api_snapshots::godot_4_7 as api_snapshot;

/// Audited generation evidence bundled with each supported Raw ABI.
pub mod bundle_manifests {
    pub const GODOT_4_4: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/godot-4.4/bundle.json"
    ));
    pub const GODOT_4_5: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/godot-4.5/bundle.json"
    ));
    pub const GODOT_4_6: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/godot-4.6/bundle.json"
    ));
    pub const GODOT_4_7: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/generated/godot-4.7/bundle.json"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn official_scalar_layout_is_preserved() {
        assert_eq!(size_of::<GDExtensionBool>(), 1);
        assert_eq!(size_of::<GDExtensionInitializationLevel>(), 4);
        assert_eq!(
            size_of::<GDExtensionInterfaceFunctionPtr>(),
            size_of::<usize>()
        );
        assert_eq!(
            GDExtensionInitializationLevel::GDEXTENSION_INITIALIZATION_SCENE.0,
            2
        );
    }

    #[test]
    fn generated_c_enums_are_unknown_value_safe_newtypes() {
        let unknown = GDExtensionInitializationLevel(u32::MAX);
        assert_eq!(unknown.0, u32::MAX);
        assert_eq!(
            size_of::<GDExtensionInitializationLevel>(),
            size_of::<u32>()
        );

        for source in [
            include_str!("../generated/godot-4.4/raw.rs"),
            include_str!("../generated/godot-4.5/raw.rs"),
            include_str!("../generated/godot-4.6/raw.rs"),
            include_str!("../generated/godot-4.7/raw.rs"),
        ] {
            assert!(!source.contains("pub enum GDExtension"));
            assert!(source.contains("pub struct GDExtensionInitializationLevel("));
        }
    }

    #[test]
    fn generated_wide_character_type_matches_the_target_abi() {
        #[cfg(target_os = "windows")]
        assert_eq!(size_of::<wchar_t>(), 2);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(size_of::<wchar_t>(), 4);
    }

    #[test]
    fn initialization_layout_is_c_compatible() {
        let pointer_size = size_of::<usize>();
        let pointer_align = align_of::<usize>();
        let level_size = size_of::<GDExtensionInitializationLevel>();
        let first_pointer_offset = (level_size + pointer_align - 1) & !(pointer_align - 1);
        let expected_size = first_pointer_offset + pointer_size * 3;

        assert_eq!(align_of::<GDExtensionInitialization>(), pointer_align);
        assert_eq!(size_of::<GDExtensionInitialization>(), expected_size);
    }

    #[test]
    fn version_layout_matches_three_u32s_and_a_pointer() {
        let pointer_align = align_of::<usize>();
        let fields_size = size_of::<u32>() * 3;
        let pointer_offset = (fields_size + pointer_align - 1) & !(pointer_align - 1);

        assert_eq!(
            size_of::<GDExtensionGodotVersion>(),
            pointer_offset + size_of::<usize>()
        );
    }

    #[test]
    fn generated_header_matches_the_reviewed_official_source() {
        assert_eq!(
            GODOT_GDEXTENSION_INTERFACE_SHA256,
            "355ff4c6254fdd434ea16d9a8ef0f18e3f95aeb3e3f00d98db10769ece3c7fe5"
        );
        assert!(size_of::<GDExtensionScriptInstanceInfo3>() > size_of::<usize>());
    }

    #[test]
    fn selected_raw_abi_has_the_reviewed_interface_hash() {
        assert_eq!(
            versions::godot_4_4::GODOT_GDEXTENSION_INTERFACE_SHA256,
            "355ff4c6254fdd434ea16d9a8ef0f18e3f95aeb3e3f00d98db10769ece3c7fe5"
        );
        let expected = match NATIVE_GODOT_API {
            "4.4" => "355ff4c6254fdd434ea16d9a8ef0f18e3f95aeb3e3f00d98db10769ece3c7fe5",
            "4.5" => "a40ac4fca0f526910bd0e6afc6da6c169f50801c84d4e29c4ce2891cadc7b550",
            "4.6" => "6228db9a0be9bb154a89911a69ee7ec767f2d40004b3ed5b1d08eebf90bcd16b",
            "4.7" => "640b48188708ba0016f8d7ace9e0e1d3279a41fa1226c59ff3193b15538bd254",
            other => panic!("unexpected selected Godot API {other}"),
        };
        assert_eq!(native::GODOT_GDEXTENSION_INTERFACE_SHA256, expected);

        for manifest in [
            bundle_manifests::GODOT_4_4,
            bundle_manifests::GODOT_4_5,
            bundle_manifests::GODOT_4_6,
            bundle_manifests::GODOT_4_7,
        ] {
            assert!(manifest.contains("\"schema_version\": 2"));
            assert!(manifest.contains("\"raw_ffi_sha256\""));
            assert!(manifest.ends_with('\n'));
        }
    }

    #[test]
    fn selected_initialization_layout_matches_the_host_baseline() {
        assert_eq!(
            size_of::<versions::godot_4_4::GDExtensionInitialization>(),
            size_of::<native::GDExtensionInitialization>()
        );
    }

    #[test]
    fn selected_class_registration_alias_matches_the_official_generation() {
        #[cfg(godot_rs_api_version = "4.4")]
        let (creation_size, symbol) = (
            size_of::<native::GDExtensionClassCreationInfo4>(),
            b"classdb_register_extension_class4\0".as_slice(),
        );
        #[cfg(any(godot_rs_api_version = "4.5", godot_rs_api_version = "4.6"))]
        let (creation_size, symbol) = (
            size_of::<native::GDExtensionClassCreationInfo5>(),
            b"classdb_register_extension_class5\0".as_slice(),
        );
        #[cfg(godot_rs_api_version = "4.7")]
        let (creation_size, symbol) = (
            size_of::<native::GDExtensionClassCreationInfo6>(),
            b"classdb_register_extension_class6\0".as_slice(),
        );
        assert_eq!(
            size_of::<native::GDExtensionClassCreationInfo>(),
            creation_size
        );
        assert_eq!(native::CLASSDB_REGISTER_EXTENSION_CLASS_SYMBOL, symbol);
    }

    #[test]
    fn generated_api_snapshot_matches_the_reviewed_official_dump() {
        let (version, class_count, hash) = match NATIVE_GODOT_API {
            "4.4" => (
                (4, 4, 1),
                952,
                "1136ad8c676034a0d9ac15ec55f1f4c79f300fd645f45a08a129c0254ca95d51",
            ),
            "4.5" => (
                (4, 5, 2),
                971,
                "481ed7dc8efc79e951081187cd5d651d6b34e2365a463f4f12adeab2f63475c8",
            ),
            "4.6" => (
                (4, 6, 3),
                1023,
                "c7a3f647d9a6d6e7f3361d8a88ffc7486a708b59262ab2f0ceae54a3d87df74d",
            ),
            "4.7" => (
                (4, 7, 1),
                1036,
                "c5dbd0c117e67f96bd9fef2c2e2023913e1d750d072ad2417a559e698211800b",
            ),
            other => panic!("unexpected selected Godot API {other}"),
        };

        assert_eq!(api_snapshot::GODOT_API_VERSION, version);
        assert_eq!(api_snapshot::ENGINE_CLASS_COUNT, class_count);
        assert_eq!(api_snapshot::BUILTIN_CLASS_COUNT, 38);
        assert_eq!(api_snapshot::UTILITY_FUNCTION_COUNT, 114);
        assert_eq!(
            api_snapshot::ENGINE_METHOD_COUNT,
            api_snapshot::ENGINE_METHODS.len()
        );
        assert!(core::hint::black_box(api_snapshot::ENGINE_METHODS.len()) > 5_000);
        assert!(api_snapshot::BUILTIN_SIZES.len() > 100);
        assert_eq!(api_snapshot::GODOT_API_SHA256, hash);
    }

    #[test]
    fn selected_api_snapshot_contains_runtime_methods() {
        assert!(api_snapshot::ENGINE_METHODS.iter().any(|method| {
            method.class == "Object" && method.name == "notification" && method.hash.is_some()
        }));
    }

    #[test]
    fn generated_class_index_is_sorted_and_contains_script_extensions() {
        assert!(
            api_snapshot::ENGINE_CLASSES
                .windows(2)
                .all(|pair| pair[0].name < pair[1].name)
        );
        for required in ["Object", "ScriptExtension", "ScriptLanguageExtension"] {
            assert!(
                api_snapshot::ENGINE_CLASSES
                    .binary_search_by_key(&required, |class| class.name)
                    .is_ok(),
                "missing {required}"
            );
        }
    }
}
