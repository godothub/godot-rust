use godot_rs::prelude::*;

#[script(base = Node)]
struct Player {
    #[property(default = 100, read_only, set = set_health)]
    health: i64,
}

impl Player {
    fn set_health(&mut self, value: i64) {
        self.health = value;
    }
}

fn main() {}
