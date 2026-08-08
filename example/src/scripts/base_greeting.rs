use godot_rs::prelude::*;

/// A reusable Rust-script base class.
#[script(base = Node, class_name = BaseGreeting)]
pub struct BaseGreeting {
    #[export(default = "Hello", group = "Greeting")]
    prefix: String,
}

#[script]
impl BaseGreeting {
    fn _ready(&mut self) {
        godot_print!("Base Rust script _ready called.");
    }

    #[func]
    fn format_greeting(&self, #[default(String::from("Godot"))] name: String) -> String {
        format!("{}, {name}!", self.prefix)
    }
}
