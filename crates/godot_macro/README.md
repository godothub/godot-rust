# godot_macro

[Project repository](https://github.com/godothub/godot-rust)

`godot_macro` implements the procedural macros used by the
[`godot-rust`](https://github.com/godothub/godot-rust) Rust scripting SDK. It
is a separate crate because Rust requires procedural macros to be compiled as
a dedicated `proc-macro` library.

Application projects should depend on `godot_rs` and use its re-exported
macros rather than adding `godot_macro` directly.

The crate implements `#[script]`, `#[godot_enum]`, and `#[godot_flags]`.
Attributes such as `#[export]`, `#[property]`, `#[node]`, `#[signal]`,
`#[func]`, `#[rpc]`, `#[default]`, and `#[reload]` are helper syntax consumed
by `#[script]`; they are not separate procedural macros.

This crate is licensed under the MIT License.
