use godot_rs::prelude::*;

#[script(base = Node)]
struct BadSignal {
    #[signal(args(only_one))]
    changed: Signal<(i32, i32)>,
}

fn main() {}
