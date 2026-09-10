use godot_rs::prelude::*;

#[script(base = Node, extends = "res://src/scripts/tool_base.rs", tool)]
pub struct ToolProbe {
    #[export(default = 7)]
    value: i64,
    #[export]
    ready_calls: i64,
    #[export]
    ticks: i64,
    #[export]
    resource: Option<GodotRef<Resource>>,
    #[export]
    values: Array<i64>,
    #[signal]
    pulse: Signal<fn(value: i64)>,
    #[signal]
    retired: Signal<fn()>,
}

#[script]
impl ToolProbe {
    fn _ready(&mut self) {
        self.ready_calls += 1;
    }

    #[func]
    fn fire(&self) -> EngineResult<()> {
        self.pulse.emit(73)
    }

    #[func]
    fn old_method(&self) -> i64 {
        5
    }

    #[func]
    fn start_task(&self) {
        self.tasks().spawn(async {
            let _guard = TaskGuard;
            loop {
                next_frame().await;
            }
        });
    }

    #[func]
    fn generation(&self) -> i64 {
        1
    }

    #[func]
    fn log_messages(&self) {
        godot_print!("GODOT_RUST_LOG_INFO");
        godot_warn!("GODOT_RUST_LOG_WARNING");
        godot_error!("GODOT_RUST_LOG_ERROR");
        godot_debug!("GODOT_RUST_LOG_DEBUG");
        godot_print!("GODOT_RUST_LONG_BEGIN{}END", "中".repeat(700));
        std::thread::spawn(|| godot_print!("GODOT_RUST_LOG_WORKER"))
            .join()
            .unwrap();
    }
}

struct TaskGuard;

impl Drop for TaskGuard {
    fn drop(&mut self) {
        assert!(Engine::singleton().and_then(|engine| engine.is_editor_hint()).unwrap());
        godot_print!("GODOT_RUST_TOOL_TASK_CANCELLED");
    }
}
