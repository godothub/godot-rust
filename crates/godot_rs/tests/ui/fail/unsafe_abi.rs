use godot_rs::abi::AbiValueV1;
use godot_rs::script::{FromAbiValue, encode_method_result};
use godot_rs::engine::EngineReturn;
fn main() {
    // Forged ABI payloads and raw output slots require an explicit unsafe boundary.
    let _ = encode_method_result(1_i64, 0x1000usize as *mut AbiValueV1);
    let _ = String::from_abi(AbiValueV1::NIL);
    let _ = String::__from_engine_return(AbiValueV1::NIL);
}
