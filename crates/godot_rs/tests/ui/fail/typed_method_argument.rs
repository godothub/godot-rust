use godot_rs::prelude::*;

#[script(base = Node)]
struct Health;

#[script]
impl Health {
    // Keep the declaration and error site on two-digit lines for consistent rustc gutters.
    #[func]
    fn damage(&mut self, amount: i64) {
        let _ = amount;
    }
}

fn main() {
    let health = NodeRef::<Health>::unresolved();
    let _ = health.damage("ten");
}
