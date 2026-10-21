use godot_rs::engine::native_virtual;
use godot_rs::native::{Base, ClassRegistrar, NativeClass, NativeResult, NativeVirtualRegistrar};
use godot_rs::prelude::*;

struct Mover {
    base: Base<Node2D>,
    speed: f64,
}

impl Mover {
    fn process(&mut self, delta: f64) {
        let _ = self.base.translate(Vector2::new((self.speed * delta) as f32, 0.0));
    }

    fn speed(&self) -> f64 { self.speed }
    fn set_speed(&mut self, value: f64) { self.speed = value; }
}

impl NativeClass for Mover {
    type Base = Node2D;
    const CLASS_NAME: &'static str = "Mover";

    fn init(base: Base<Node2D>) -> Self {
        Self { base, speed: 120.0 }
    }

    fn register_virtuals(
        registrar: &mut NativeVirtualRegistrar<'_, Self>,
    ) -> NativeResult {
        native_virtual::node::_process(registrar, Self::process)?;
        Ok(())
    }

    fn register_methods(
        registrar: &mut ClassRegistrar<'_, Self>,
    ) -> NativeResult {
        registrar.property("speed", Self::speed, Self::set_speed)?;
        Ok(())
    }
}

fn main() {}
