# godot_rs

[Project repository](https://github.com/godothub/godot-rust)

`godot_rs` is the public Rust scripting SDK used by the
[`godot-rust`](https://github.com/godothub/godot-rust) Godot editor plugin. It
provides typed Godot classes, properties, methods, signals, lifecycle callbacks,
and tasks for Rust scripts and native extensions.

Add this crate to the Cargo project managed by the plugin:

```toml
[dependencies]
godot_rs = "0.9"

[package.metadata.godot-rust]
godot = "4.4"
mode = "script"
```

Ordinary scripts should import `godot_rs::prelude::*`; the SDK re-exports its
attribute macros, so no direct macro dependency is required.

See the [project README](https://github.com/godothub/godot-rust#readme) for setup
instructions and usage examples.

This crate is licensed under the MIT License.
