#![no_main]

use godot_rs_buildd::{decode_hex_request, handle_json_request};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let first = handle_json_request(text);
        let second = handle_json_request(text);
        assert_eq!(first.id, second.id);
        assert_eq!(first.ok, second.ok);
        assert_eq!(first.error, second.error);

        if let Ok(decoded) = decode_hex_request(text) {
            let _ = handle_json_request(&decoded);
        }
    }
});
