//! Shared Godot API target and official-input provenance policy.

mod source;
mod target;

pub use source::{
    OfficialApiSource, OfficialFileSource, SourceManifestError, load_official_api_source,
    verify_official_input,
};
pub use target::{
    ApiTargetCatalog, GodotApiVersion, GodotApiVersionError, SUPPORTED_API_TARGETS,
    TargetCatalogError, load_target_catalog,
};

/// Internal environment variable used to select one pre-generated Native ABI.
pub const GODOT_API_ENV: &str = "GODOT_RS_GODOT";
/// Stable entry symbol emitted by `godot_rs::gdextension!`.
pub const NATIVE_ENTRY_SYMBOL: &str = "godot_rs_native_init";
