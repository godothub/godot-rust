use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=GODOT_RS_GODOT");
    println!("cargo:rustc-check-cfg=cfg(godot_rs_test_api_4_7)");
    if env::var("GODOT_RS_GODOT").as_deref() == Ok("4.7") {
        println!("cargo:rustc-cfg=godot_rs_test_api_4_7");
    }
}
