use godot_rs::prelude::*;

#[script(base = Node2D)]
struct Player;

#[script]
impl Player {
    fn _ready(&mut self) -> EngineResult<()> {
        self.base().set_process(true)
    }

    fn _process(&mut self, _delta: f64) -> ScriptResult<()> {
        Ok(())
    }

    #[func]
    fn echo_rid(&self, rid: Rid) -> Rid {
        rid
    }

    #[func]
    fn describe(&self, value: i64, #[default(String::from("points"))] suffix: String) -> String {
        format!("{value} {suffix}")
    }

    #[func]
    fn count_values(&self, values: &[Variant]) -> i64 {
        values.len() as i64
    }
}

#[script]
impl NodeVirtual for Player {
    fn _get_configuration_warnings(&mut self) -> PackedStringArray {
        PackedStringArray::default()
    }
}

fn main() {}
