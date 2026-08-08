#![doc = "Cargo project and build orchestration for the godot-rust editor plugin."]

mod build;
mod build_receipt;
mod cargo_task;
mod dependency;
mod diagnostic;
mod diagnostic_fix;
mod export_target;
mod gdextension;
mod managed_fs;
mod metadata;
mod module_artifact;
mod native;
mod native_publication;
mod process;
mod project;
mod project_export;
mod project_setup;
mod project_task;
mod protocol;
mod publication;
mod script_index;
mod toolchain;
mod xcframework;

pub use build::{ProjectBuildReport, build_and_publish};
pub use build_receipt::{
    BUILD_RECEIPT_FILE, BuildReceiptReason, BuildReceiptStatus, BuildReceiptSummary,
    check_build_receipt, record_build_receipt,
};
pub use cargo_task::{
    CargoTaskKind, CargoTaskReport, run_cargo_task, run_native_package_cargo_build,
    run_native_package_cargo_task, run_package_cargo_build, run_package_cargo_task,
};
pub use dependency::{
    DependencyApplyReport, DependencyChange, DependencyKind, DependencyListReport,
    DependencyOperation, DependencyPreview, DependencyRecord, DependencySourceKind,
    apply_dependency_change, list_dependencies, preview_dependency_change,
};
pub use diagnostic::{
    CargoDiagnostic, DiagnosticLevel, DiagnosticReplacement, DiagnosticSpan, DiagnosticSuggestion,
};
pub use diagnostic_fix::{
    DiagnosticFixEdit, DiagnosticFixPlan, DiagnosticFixReport, apply_diagnostic_fix,
};
pub use export_target::{
    ExportArchitecture, ExportArtifactTarget, ExportOperatingSystem, ExportProfile,
    ProjectExportTarget,
};
pub use gdextension::{
    NativeArchitecture, NativeBuildProfile, NativeExtensionDescriptor, NativeOperatingSystem,
    NativePlatform,
};
pub use metadata::{
    CargoPackageModel, CargoProjectModel, CargoTargetModel, EditorWorkflowConfig, GodotRustMode,
    PackageSelectionReason, inspect_cargo_project,
};
pub use native::{
    NativeBuildPlan, NativeBuildReport, NativeProjectPlan, inspect_native_build_plan,
    plan_native_build, run_native_build,
};
pub use native_publication::{
    NATIVE_ARTIFACT_DIRECTORY, NATIVE_DESCRIPTOR_FILE, NATIVE_STATE_FILE,
    NativePublicationResolution, PublishedNativeExtension, commit_native_publication,
    publish_native_extension, rollback_native_publication,
};
pub use project::{
    DEFAULT_PACKAGE_NAME, InitOutcome, ProjectKind, ProjectProbe, initialize_project, probe_project,
};
pub use project_export::{
    ExportedProjectModule, ProjectExportEnvironment, ProjectExportReport, build_project_for_export,
};
pub use project_setup::{
    ProjectSetupOutcome, WorkspaceSelectionOutcome, configure_project,
    godot_rs_version_requirement, select_workspace_package,
};
pub use project_task::{ProjectCargoOutcome, run_project_cargo_task};
pub use protocol::{
    BuilddRequest, BuilddResponse, decode_hex_request, handle_json_request, handle_request, serve,
};
pub use publication::{
    LAST_KNOWN_GOOD_FILE, PublishedGeneration, PublishedGenerationIdentity,
    publish_validated_generation, verify_last_known_good,
};
pub use script_index::{
    ScriptIndexEntry, ScriptIndexReport, sync_configured_script_index, sync_script_index,
};
pub use toolchain::{
    RustTargetInstallReport, ToolchainReport, install_rust_target, probe_toolchain,
};

#[doc(hidden)]
pub const fn process_cancel_file_env() -> &'static str {
    process::CANCEL_FILE_ENV
}
