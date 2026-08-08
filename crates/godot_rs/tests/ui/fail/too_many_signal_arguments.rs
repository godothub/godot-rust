use godot_rs::prelude::*;

#[script(base = Node)]
struct TooManySignalArguments {
    #[signal(args(a, b, c, d, e, f, g, h, i))]
    changed: Signal<(i32, i32, i32, i32, i32, i32, i32, i32, i32)>,
}

fn main() {}
