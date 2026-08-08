use godot_rs::prelude::*;

#[script(base = Node)]
struct UnsupportedMethod;

#[script]
impl UnsupportedMethod {
    #[func]
    fn set_name(&mut self, name: Vec<i64>) {
        let _ = name;
    }
}

fn main() {}
