use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed={}", godot_api::GODOT_API_ENV);
    let selected = env::var(godot_api::GODOT_API_ENV)
        .unwrap_or_else(|_| godot_api::DEFAULT_GODOT_API.to_string());
    let (api_path, _) = godot_api::bundled_api(&selected)
        .unwrap_or_else(|error| panic!("failed to select Godot {selected} API: {error}"));
    println!("cargo:rerun-if-changed={}", api_path.display());
    let (source, report) = godot_api::generate_bundled_engine_api(&selected)
        .unwrap_or_else(|error| panic!("failed to generate Godot {selected} bindings: {error}"));
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("engine_api.rs");
    fs::write(&output, source).expect("write generated Godot bindings");
    assert!(report.total_official_entries > 0);
}
