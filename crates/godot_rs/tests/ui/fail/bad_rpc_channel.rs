use godot_rs::prelude::*;

#[script(base = Node)]
struct InvalidRpcChannel;

#[script]
impl InvalidRpcChannel {
    #[rpc(channel = -1)]
    fn synchronize(&mut self, value: i64) {
        let _ = value;
    }
}

fn main() {}
