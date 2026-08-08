# godot_rs

`godot_rs` is the public Rust scripting SDK used by the `godot-rust` Godot
editor plugin. It provides typed Godot classes, properties, methods, signals,
lifecycle callbacks, tasks, and project-module integration for both `script`
and `extension` projects.

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

The Godot-facing surface uses ordinary Rust wherever metadata is unnecessary:
public associated `const` items become script constants, private fields and
methods remain internal, `#[property]` exposes a non-Inspector property, and
`#[export]` adds Inspector metadata. `#[godot_enum]` and `#[godot_flags]`
provide forward-compatible integer-backed project types.

This crate is licensed under the MIT License.
