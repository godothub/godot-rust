use godot_rs::prelude::*;

#[script(base = Node)]
pub struct EditorProbe {
    #[export(default = 3)]
    value: i64,
    ticks: i64,
}

#[script]
impl EditorProbe {
    fn _ready(&mut self) {
        godot_print!("GODOT_RUST_RUNTIME_READY");
        godot_debug!("GODOT_RUST_LOG_DEBUG");
    }

    fn _process(&mut self, _delta: f64) {
        self.ticks += 1;
        if self.ticks == 1 {
            godot_print!("GODOT_RUST_RUNTIME_PROCESS");
        }
    }
}
