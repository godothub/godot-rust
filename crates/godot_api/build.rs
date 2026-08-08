use std::env;

const GODOT_API_ENV: &str = "GODOT_RS_GODOT";
const DEFAULT_GODOT_API: &str = "4.4";

fn main() {
    println!("cargo:rerun-if-env-changed={GODOT_API_ENV}");
    println!(
        "cargo:rustc-check-cfg=cfg(godot_api_version, values(\"4.4\", \"4.5\", \"4.6\", \"4.7\"))"
    );

    let selected = env::var(GODOT_API_ENV).unwrap_or_else(|_| DEFAULT_GODOT_API.to_owned());
    if !matches!(selected.as_str(), "4.4" | "4.5" | "4.6" | "4.7") {
        panic!(
            "{GODOT_API_ENV} is invalid: expected one of 4.4, 4.5, 4.6, or 4.7; \
             the godot-rust plugin derives it from package.metadata.godot-rust.godot"
        );
    }

    println!("cargo:rustc-cfg=godot_api_version={selected:?}");
    println!("cargo:rustc-env=GODOT_RS_API_GODOT={selected}");
}
