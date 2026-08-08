#![no_main]

use godot_api::abi::{
    AbiValueType, callable_value_ownership_token, dynamic_value_ownership_token,
    validate_callable_value, validate_dynamic_value, validate_signal_value,
    visit_dynamic_callable_tokens,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let callable_valid = validate_callable_value(data);
    assert_eq!(
        callable_value_ownership_token(data).is_some(),
        callable_valid && data.get(10..12) == Some(&1_u16.to_le_bytes()),
    );

    let _ = validate_signal_value(data);
    for expected in [
        AbiValueType::VARIANT,
        AbiValueType::ARRAY,
        AbiValueType::DICTIONARY,
    ] {
        let valid = validate_dynamic_value(expected, data);
        if valid {
            assert!(validate_dynamic_value(AbiValueType::VARIANT, data));
        }
    }

    let root_valid = validate_dynamic_value(AbiValueType::VARIANT, data);
    assert_eq!(
        dynamic_value_ownership_token(data).is_some(),
        root_valid && data.get(10..12) == Some(&1_u16.to_le_bytes()),
    );
    assert_eq!(visit_dynamic_callable_tokens(data, |_| true), root_valid,);
});
