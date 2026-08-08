use godot_rs::prelude::*;

#[script(base = Node)]
struct BadColorHint {
    #[export(no_alpha)]
    opacity: f32,
}

fn main() {}
