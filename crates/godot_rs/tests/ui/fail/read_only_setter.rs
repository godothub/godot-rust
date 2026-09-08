use godot_rs::prelude::*;

#[script(base = Node)]
struct Player {
    #[property(default = 100, read_only, set = set_health)]
    health: i64,
}

fn main() {}
