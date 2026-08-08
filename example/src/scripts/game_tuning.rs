use godot_rs::prelude::*;

/// A globally named custom Resource that appears in Godot's resource UI.
#[script(base = Resource, class_name = GameTuning)]
pub struct GameTuning {
    #[export(default = 100, group = "Gameplay", range(min = 1, max = 999, step = 1))]
    max_health: i64,

    #[export(default = 240.0, range(min = 0.0, max = 2000.0, step = 1.0))]
    movement_speed: f64,
}
