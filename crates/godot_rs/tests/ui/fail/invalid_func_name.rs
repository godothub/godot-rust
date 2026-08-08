use godot_rs::prelude::*;

#[script(base = Node)]
struct Player;

#[script]
impl Player {
    #[func(name = "not-a-godot-name")]
    fn run(&self) {}
}

fn main() {}
