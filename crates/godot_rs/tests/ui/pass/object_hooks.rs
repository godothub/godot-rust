use godot_rs::prelude::*;
#[script(base = Node)] struct Hooks {}
#[script] impl Hooks {
    fn _notification(&mut self, _what: i32) {}
    fn _to_string(&self) -> String { "Hooks".into() }
    fn _get(&self, _name: StringName) -> Variant { Variant::nil() }
    fn _set(&mut self, _name: StringName, _value: Variant) -> bool { false }
    fn _get_property_list(&self) -> Array<Dictionary> { Array::new() }
    fn _validate_property(&self, _property: Dictionary) {}
    fn _property_can_revert(&self, _name: StringName) -> bool { false }
    fn _property_get_revert(&self, _name: StringName) -> Variant { Variant::nil() }
    fn _reloaded(&mut self) -> EngineResult<()> { Ok(()) }
}
fn main() {}
