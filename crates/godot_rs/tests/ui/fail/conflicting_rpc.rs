use godot_rs::prelude::*;

#[script(base = Node)]
struct Lobby;

#[script]
impl Lobby {
    #[rpc(authority, any_peer, reliable, unreliable)]
    fn sync(&mut self) {}
}

fn main() {}
