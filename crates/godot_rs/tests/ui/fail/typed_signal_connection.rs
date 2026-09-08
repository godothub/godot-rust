use godot_rs::prelude::*;

#[script(base = Node)]
struct Health {
    #[signal]
    damaged: Signal<fn(amount: i64)>,
}

#[script]
impl Health {
    #[func]
    fn display(&mut self, text: String) {
        let _ = text;
    }

    fn _ready(&mut self) -> EngineResult<()> {
        self.damaged.connect_method(&self.callbacks()?.display())
    }
}

fn main() {}
