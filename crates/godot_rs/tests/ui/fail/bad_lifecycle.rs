use godot_rs::prelude::*;

#[script(base = Node2D)]
struct Player;

#[script]
impl Player {
    fn _physics_process(&self, _delta: f32) {}
}

fn main() {}
