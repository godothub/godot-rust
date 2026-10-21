use godot_rs::prelude::*;
fn main() {
    let array = Array::<i64>::new();
    std::thread::spawn(move || drop(array));
}
