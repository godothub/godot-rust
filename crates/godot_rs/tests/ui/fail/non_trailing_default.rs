use godot_rs::prelude::*;

#[script(base = Node)]
struct Defaults;

#[script]
impl Defaults {
    #[func]
    fn add(&self, #[default(20)] left: i64, right: i64) -> i64 {
        left + right
    }
}

fn main() {}
