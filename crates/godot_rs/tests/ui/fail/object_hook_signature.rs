use godot_rs::prelude::*;
#[script(base = Node)] struct Hooks {}
#[script] impl Hooks {
    fn _notification(&mut self, _what: String) {}
}
fn main() {}
