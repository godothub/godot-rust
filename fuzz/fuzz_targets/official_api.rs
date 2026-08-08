#![no_main]

use godot_codegen::{ExpectedApiVersion, ExtensionApi, validate_api};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(api) = serde_json::from_slice::<ExtensionApi>(data) else {
        return;
    };
    let expected = ExpectedApiVersion {
        major: api.header.version_major,
        minor: api.header.version_minor,
    };
    let first = validate_api(&api, expected);
    let second = validate_api(&api, expected);
    assert_eq!(first, second);
});
