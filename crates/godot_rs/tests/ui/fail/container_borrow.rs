use godot_rs::prelude::*;
fn main() {
    let array = Array::<i64>::new();
    let _borrowed: &i64 = &array[0];
}
