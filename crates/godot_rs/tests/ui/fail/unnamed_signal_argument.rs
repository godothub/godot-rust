use godot_rs::prelude::*;

#[script(base = Node)]
struct Player {
    #[signal]
    damaged: Signal<fn(i64)>,
}

fn main() {}
