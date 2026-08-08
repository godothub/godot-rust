use crate::{
    ApiInventory, ExpectedApiVersion, GodotApiVersion, OfficialApiSource, generate_api_snapshot,
    generate_raw_ffi, load_api, validate_api, verify_official_input,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fs;
use std::path::Path;

/// Filename of the generated Raw GDExtension FFI module.
pub const RAW_FFI_FILE: &str = "raw.rs";
/// Filename of the generated high-level API index.
pub const API_SNAPSHOT_FILE: &str = "api_snapshot.rs";
/// Filename of the deterministic generation evidence.
pub const BUNDLE_MANIFEST_FILE: &str = "bundle.json";

/// Auditable identity and content hashes for one generated binding bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BindingBundleManifest {
    pub schema_version: u32,
    pub godot: GodotApiVersion,
    pub godot_patch: u32,
    pub godot_tag: String,
    pub generator_version: String,
    pub gdextension_interface_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gdextension_interface_json_sha256: Option<String>,
    pub extension_api_sha256: String,
    pub raw_ffi_sha256: String,
    pub api_snapshot_sha256: String,
    pub api_inventory: ApiInventory,
}

/// Generated Rust source and its deterministic release evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingBundle {
    pub raw_ffi: String,
    pub api_snapshot: String,
    pub manifest: BindingBundleManifest,
}

impl BindingBundle {
    /// Serializes the manifest with deterministic formatting and a trailing newline.
    pub fn manifest_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.manifest).map(|mut value| {
            value.push('\n');
            value
        })
    }

    /// Writes the three versioned bundle files, committing the manifest last.
    pub fn write_to(&self, output: &Path) -> Result<(), Box<dyn Error>> {
        match fs::symlink_metadata(output) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "binding bundle output must not be a symbolic link: {}",
                    output.display()
                )
                .into());
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "binding bundle output is not a directory: {}",
                    output.display()
                )
                .into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(output)?;
            }
            Err(error) => return Err(error.into()),
        }

        fs::write(output.join(RAW_FFI_FILE), &self.raw_ffi)?;
        fs::write(output.join(API_SNAPSHOT_FILE), &self.api_snapshot)?;
        fs::write(output.join(BUNDLE_MANIFEST_FILE), self.manifest_json()?)?;
        Ok(())
    }

    /// Proves that an existing directory is byte-for-byte the expected bundle.
    pub fn check(&self, output: &Path) -> Result<(), Box<dyn Error>> {
        for (filename, expected) in [
            (RAW_FFI_FILE, self.raw_ffi.as_str()),
            (API_SNAPSHOT_FILE, self.api_snapshot.as_str()),
        ] {
            let path = output.join(filename);
            let actual = fs::read_to_string(&path)?;
            if actual != expected {
                return Err(format!("generated binding file is stale: {}", path.display()).into());
            }
        }
        let manifest_path = output.join(BUNDLE_MANIFEST_FILE);
        let actual = fs::read_to_string(&manifest_path)?;
        if actual != self.manifest_json()? {
            return Err(format!(
                "generated binding manifest is stale: {}",
                manifest_path.display()
            )
            .into());
        }
        Ok(())
    }
}

/// Authenticates every official input and generates one low-level binding and API-index bundle.
pub fn generate_binding_bundle(
    source: &OfficialApiSource,
    gdextension_interface: &Path,
    gdextension_interface_json: Option<&Path>,
    extension_api: &Path,
) -> Result<BindingBundle, Box<dyn Error>> {
    verify_official_input(gdextension_interface, source.gdextension_interface())?;
    match (
        source.gdextension_interface_json(),
        gdextension_interface_json,
    ) {
        (Some(expected), Some(path)) => verify_official_input(path, expected)?,
        (Some(_), None) => {
            return Err(format!(
                "Godot {} binding generation requires gdextension_interface.json",
                source.target()
            )
            .into());
        }
        (None, Some(_)) => {
            return Err(format!(
                "Godot {} source manifest does not define gdextension_interface.json",
                source.target()
            )
            .into());
        }
        (None, None) => {}
    }
    verify_official_input(extension_api, source.extension_api())?;

    let loaded = load_api(extension_api)?;
    let target = source.target();
    let issues = validate_api(
        &loaded.api,
        ExpectedApiVersion {
            major: target.major(),
            minor: target.minor(),
        },
    );
    if !issues.is_empty() {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n- ");
        return Err(format!("API validation failed:\n- {details}").into());
    }
    let api_inventory = loaded.api.inventory();
    if !api_inventory.has_required_surface() {
        return Err("official API input does not contain every required binding category".into());
    }

    let raw_ffi = generate_raw_ffi(gdextension_interface)?;
    let api_snapshot = generate_api_snapshot(&loaded.api, &loaded.sha256);
    let manifest = bundle_manifest(
        source,
        loaded.api.header.version_patch,
        api_inventory,
        &raw_ffi,
        &api_snapshot,
    );
    Ok(BindingBundle {
        raw_ffi,
        api_snapshot,
        manifest,
    })
}

/// Verifies a checked-in or packaged bundle without needing the original API files.
pub fn verify_binding_bundle(
    source: &OfficialApiSource,
    output: &Path,
) -> Result<BindingBundleManifest, Box<dyn Error>> {
    let expected_directory = format!("godot-{}", source.target());
    if output.file_name().and_then(|name| name.to_str()) != Some(expected_directory.as_str()) {
        return Err(
            format!("binding bundle directory must end with `{expected_directory}`").into(),
        );
    }
    let directory = fs::symlink_metadata(output)?;
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err(format!(
            "binding bundle must be a real directory: {}",
            output.display()
        )
        .into());
    }

    let manifest_bytes = read_regular_file(&output.join(BUNDLE_MANIFEST_FILE))?;
    let manifest: BindingBundleManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.schema_version != 2 {
        return Err(format!(
            "unsupported binding bundle schema {}; expected 2",
            manifest.schema_version
        )
        .into());
    }
    if manifest.godot != source.target() || manifest.godot_tag != source.godot_tag() {
        return Err(format!(
            "binding bundle target {} / tag {} does not match source target {} / tag {}",
            manifest.godot,
            manifest.godot_tag,
            source.target(),
            source.godot_tag()
        )
        .into());
    }
    if manifest.generator_version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "binding bundle generator version {} does not match {}",
            manifest.generator_version,
            env!("CARGO_PKG_VERSION")
        )
        .into());
    }
    if manifest.gdextension_interface_sha256 != source.gdextension_interface().sha256()
        || manifest.gdextension_interface_json_sha256.as_deref()
            != source
                .gdextension_interface_json()
                .map(|input| input.sha256())
        || manifest.extension_api_sha256 != source.extension_api().sha256()
    {
        return Err("binding bundle official input hashes do not match godot-api.toml".into());
    }
    let expected_patch = source
        .extension_api()
        .generator_version()
        .and_then(|version| version.split('.').nth(2))
        .and_then(|patch| patch.parse::<u32>().ok())
        .ok_or("godot-api.toml extension API generator version has no numeric patch")?;
    if manifest.godot_patch != expected_patch {
        return Err(format!(
            "binding bundle representative patch {} does not match godot-api.toml patch {expected_patch}",
            manifest.godot_patch
        )
        .into());
    }
    if !manifest.api_inventory.has_required_surface() {
        return Err("binding bundle API inventory is incomplete".into());
    }

    let raw_ffi = read_regular_file(&output.join(RAW_FFI_FILE))?;
    let api_snapshot = read_regular_file(&output.join(API_SNAPSHOT_FILE))?;
    if sha256(&raw_ffi) != manifest.raw_ffi_sha256 {
        return Err("binding bundle raw.rs hash does not match bundle.json".into());
    }
    if sha256(&api_snapshot) != manifest.api_snapshot_sha256 {
        return Err("binding bundle api_snapshot.rs hash does not match bundle.json".into());
    }
    Ok(manifest)
}

fn bundle_manifest(
    source: &OfficialApiSource,
    godot_patch: u32,
    api_inventory: ApiInventory,
    raw_ffi: &str,
    api_snapshot: &str,
) -> BindingBundleManifest {
    BindingBundleManifest {
        schema_version: 2,
        godot: source.target(),
        godot_patch,
        godot_tag: source.godot_tag().to_owned(),
        generator_version: env!("CARGO_PKG_VERSION").to_owned(),
        gdextension_interface_sha256: source.gdextension_interface().sha256().to_owned(),
        gdextension_interface_json_sha256: source
            .gdextension_interface_json()
            .map(|input| input.sha256().to_owned()),
        extension_api_sha256: source.extension_api().sha256().to_owned(),
        raw_ffi_sha256: sha256(raw_ffi.as_bytes()),
        api_snapshot_sha256: sha256(api_snapshot.as_bytes()),
        api_inventory,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "binding bundle member must be a regular file: {}",
            path.display()
        )
        .into());
    }
    Ok(fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use godot_rs_api_policy::load_official_api_source;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(version: GodotApiVersion) -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "godot-rust-binding-bundle-{}-{id}",
                std::process::id()
            ));
            let path = root.join(format!("godot-{version}"));
            fs::create_dir_all(&path).expect("temporary bundle directory");
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            if let Some(root) = self.0.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }

    fn source() -> OfficialApiSource {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .to_owned();
        load_official_api_source(&root.join("godot-api.toml"), GodotApiVersion::new(4, 4))
            .expect("official source")
    }

    fn complete_inventory() -> ApiInventory {
        ApiInventory {
            builtin_size_configuration_count: 4,
            builtin_size_count: 1,
            builtin_offset_configuration_count: 4,
            builtin_offset_count: 1,
            global_constant_count: 0,
            global_enum_count: 1,
            utility_function_count: 1,
            builtin_class_count: 1,
            builtin_constructor_count: 1,
            builtin_operator_count: 1,
            builtin_method_count: 1,
            builtin_member_count: 1,
            builtin_constant_count: 1,
            builtin_enum_count: 1,
            engine_class_count: 1,
            engine_method_count: 1,
            engine_property_count: 1,
            engine_signal_count: 1,
            engine_enum_count: 1,
            engine_constant_count: 1,
            singleton_count: 1,
            native_structure_count: 1,
        }
    }

    #[test]
    fn manifest_records_all_generation_identity_inputs() {
        let source = source();
        let manifest = bundle_manifest(&source, 1, complete_inventory(), "raw", "snapshot");
        assert_eq!(manifest.schema_version, 2);
        assert_eq!(manifest.godot, GodotApiVersion::new(4, 4));
        assert_eq!(manifest.godot_patch, 1);
        assert_eq!(
            manifest.gdextension_interface_sha256,
            source.gdextension_interface().sha256()
        );
        assert_eq!(
            manifest.extension_api_sha256,
            source.extension_api().sha256()
        );
        assert_eq!(manifest.raw_ffi_sha256, sha256(b"raw"));
        assert_eq!(manifest.api_snapshot_sha256, sha256(b"snapshot"));
    }

    #[test]
    fn manifest_json_is_deterministic_and_version_is_major_minor() {
        let source = source();
        let bundle = BindingBundle {
            raw_ffi: "raw".into(),
            api_snapshot: "snapshot".into(),
            manifest: bundle_manifest(&source, 1, complete_inventory(), "raw", "snapshot"),
        };
        let first = bundle.manifest_json().expect("manifest JSON");
        let second = bundle.manifest_json().expect("manifest JSON");
        assert_eq!(first, second);
        assert!(first.contains("\"godot\": \"4.4\""));
        assert!(first.ends_with('\n'));
    }

    #[test]
    fn offline_verification_rejects_modified_generated_files() {
        let source = source();
        let bundle = BindingBundle {
            raw_ffi: "raw".into(),
            api_snapshot: "snapshot".into(),
            manifest: bundle_manifest(&source, 1, complete_inventory(), "raw", "snapshot"),
        };
        let output = TempDirectory::new(source.target());
        bundle.write_to(&output.0).expect("write bundle");
        verify_binding_bundle(&source, &output.0).expect("bundle should verify");

        fs::write(output.0.join(RAW_FFI_FILE), "modified").expect("tamper fixture");
        let error =
            verify_binding_bundle(&source, &output.0).expect_err("tampered bundle must fail");
        assert!(error.to_string().contains("raw.rs hash"));
    }
}
