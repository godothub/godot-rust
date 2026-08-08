//! Independent parser, validator, and generator for Godot's official API JSON.

mod bundle;
mod diff;
mod policy;
mod snapshot;
mod sys;

pub use bundle::{
    API_SNAPSHOT_FILE, BUNDLE_MANIFEST_FILE, BindingBundle, BindingBundleManifest, RAW_FFI_FILE,
    generate_binding_bundle, verify_binding_bundle,
};
pub use diff::{ApiDiff, diff_api};
pub use godot_api::{
    ApiArgument, ApiClass, ApiClassConstant, ApiEnum, ApiEnumValue, ApiHeader, ApiInventory,
    ApiMethod, ApiProperty, ApiReturnValue, ApiSignal, ApiSingleton, BuiltinClass,
    BuiltinClassOffsetConfiguration, BuiltinClassOffsets, BuiltinClassSize,
    BuiltinClassSizeConfiguration, BuiltinConstant, BuiltinConstructor, BuiltinEnum, BuiltinMember,
    BuiltinMemberOffset, BuiltinMethod, BuiltinOperator, ExtensionApi, GlobalConstant,
    NativeStructure, UtilityFunction,
};
pub use godot_api::{
    ApiCoverageDisposition, ApiCoverageEntry, EngineApiGenerationError, EngineApiGenerationReport,
    ExpectedApiVersion, LoadError, LoadedApi, UnsupportedEngineType, ValidationIssue,
    analyze_engine_api, generate_engine_api, load_api, validate_api, verify_engine_api_coverage,
};
pub use godot_api::{GodotApiVersion, GodotApiVersionError, SUPPORTED_API_TARGETS};
pub use policy::{
    ApiTargetCatalog, OfficialApiSource, OfficialFileSource, SourceManifestError,
    TargetCatalogError, load_official_api_source, load_target_catalog, verify_official_input,
};
pub use snapshot::generate_api_snapshot;
pub use sys::generate_raw_ffi;
