use godot_rs::prelude::*;

#[script(base = Node, tool)]
pub struct ToolBase {
    #[export(default = 12)]
    inherited: i64,
}
