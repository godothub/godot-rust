use godot_rs::prelude::*;

#[script(base = Node)]
struct Health;

#[script]
impl Health {
    #[func]
    fn damage(&mut self, amount: i64) {
        let _ = amount;
    }
}

fn main() {
    let health = NodeRef::<Health>::unresolved();
    let _ = health.damage("ten");
}
