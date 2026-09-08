# godot_api

[Project repository](https://github.com/godothub/godot-rust)

`godot_api` contains the versioned Godot API data and the stable binary
contract shared by the
[`godot-rust`](https://github.com/godothub/godot-rust) SDK, Script Host, and
build service.

It is published because `godot_rs` uses it during normal compilation and
build-time API generation. Application projects should depend on `godot_rs`
instead of using this low-level crate directly.

The default feature exposes the selected generated bindings. Internal tools
can disable default features and enable only `serde` or `generator` to avoid
compiling the complete binding surface.

This crate is licensed under the MIT License.
