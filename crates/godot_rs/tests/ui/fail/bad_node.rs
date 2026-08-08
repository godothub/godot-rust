use godot_rs::prelude::*;

#[script(base = Node2D)]
struct Player {
    #[node("%Camera", optional)]
    camera: NodeRef<Camera2D>,
}

fn main() {}
