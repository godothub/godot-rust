# godot_macro

`godot_macro` implements the procedural macros used by the `godot-rust` Rust
scripting SDK. It is a separate crate because Rust requires procedural macros
to be compiled as a dedicated `proc-macro` library.

Application projects should depend on `godot_rs` and use its re-exported
macros rather than adding `godot_macro` directly.

This crate is licensed under the MIT License.
