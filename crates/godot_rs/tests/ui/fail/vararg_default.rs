use godot_rs::prelude::*;

#[script(base = Node)]
struct Defaults;

#[script]
impl Defaults {
    #[func]
    fn count(&self, #[default(&[])] values: &[Variant]) -> i64 {
        values.len() as i64
    }
}

fn main() {}
