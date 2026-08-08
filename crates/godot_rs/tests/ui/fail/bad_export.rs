use godot_rs::prelude::*;

#[script(base = Node)]
struct BadExport {
    #[export(range(min = 0, max = 10))]
    label: String,

    #[export]
    values: Vec<i64>,
}

fn main() {}
