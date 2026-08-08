use std::env;
use std::fs;
use std::path::PathBuf;

const GODOT_API_ENV: &str = "GODOT_RS_GODOT";
const DEFAULT_GODOT_API: &str = "4.4";

fn main() {
    println!("cargo:rerun-if-env-changed={GODOT_API_ENV}");
    let selected = env::var(GODOT_API_ENV).unwrap_or_else(|_| DEFAULT_GODOT_API.to_owned());
    let (api_path, _) = godot_rs_api::bundled_api(&selected)
        .unwrap_or_else(|error| panic!("failed to select Godot {selected} API: {error}"));
    println!("cargo:rerun-if-changed={}", api_path.display());
    let (source, report) = godot_rs_api::generate_bundled_engine_api(&selected)
        .unwrap_or_else(|error| panic!("failed to generate Godot {selected} bindings: {error}"));
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("engine_api.rs");
    fs::write(&output, source).expect("write generated Godot bindings");
    assert!(report.total_official_entries > 0);
}
