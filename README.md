# godot-rust

English | [简体中文](README-ZH.md)

**Write your Godot game in Rust.**

Create and attach `.rs` files just like Godot scripts, write code in a style
designed for Godot users, and export your game through Godot. Keep your familiar
editor workflow with Rust's performance, type safety, and Cargo ecosystem.

## Why godot-rust?

- **At home in Godot**: create and attach Rust scripts in the editor, with
  readable code and syntactic sugar designed for Godot users.
- **Typed engine APIs**: call Godot classes, singletons, and built-in types from
  Rust, with compile-time type checking.
- **Hot reload**: build your changes and preserve Inspector property values
  across compatible updates. Failed builds leave the last working version active.
- **Diagnostics in the editor**: saving runs a check automatically. See errors
  and warnings in the Rust panel, then double-click to jump to the source.
- **Your Rust tools**: use crates.io dependencies, Cargo workspaces,
  rust-analyzer, and Clippy, or write code in your preferred external editor.

## Quick start

Install Godot 4.4+, Rust 1.85+ with Cargo, and your platform's native build
tools. Make sure Godot can find `cargo`.

### 1. Install the plugin

Download the plugin ZIP from
[Releases](https://github.com/godothub/godot-rust/releases) and extract it into
your Godot project root. Check that this file exists:

```text
res://addons/godot-rust/plugin.cfg
```

Open the project and enable **godot-rust** under **Project > Project Settings >
Plugins**. New projects get a configured Cargo project automatically; for
existing Cargo projects, follow the editor's setup prompts.

### 2. Create a Rust script

Select a scene node, click **Attach Script**, choose **Rust**, and save under
`res://src/scripts/`. The plugin manages script module declarations for you.

For example, attach this script to a `Node2D` to control its horizontal movement
speed from the Inspector:

```rust
use godot_rs::prelude::*;

#[script(base = Node2D)]
pub struct Player {
    #[export(default = 240.0)]
    speed: f64,
}

#[script]
impl Player {
    fn _ready(&mut self) -> EngineResult<()> {
        self.base().set_process(true)?;
        Ok(())
    }

    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let offset = Vector2::new((self.speed * delta) as f32, 0.0);
        self.base().translate(offset)?;
        Ok(())
    }
}
```

`#[script]` declares the script and its callbacks, `#[export]` makes a field
editable in the Inspector, and `self.base()` accesses the attached node.

### 3. Run and iterate

Press **F5** to run the project or **F6** to run the current scene. The plugin
builds Rust code when needed. To build manually, use **Build** in the bottom
**Rust** panel or **Project > Tools > Rust: Build**.

Saving checks your code; run **Build** again to apply changes. Compatible changes
support hot reload. Changes to field types, method signatures, or inheritance
require restarting the scene.

## Export your game

Use Godot's **Project > Export** window. The plugin builds and packages Rust code
for your export preset. One plugin ZIP includes runtime components for supported
platforms; each platform still needs its toolchain and Godot export templates.

| Platform | Requirements |
| --- | --- |
| Windows, Linux, macOS | Export on the matching operating system. macOS requires Xcode. |
| Android | Android SDK, NDK, and the matching Rust targets. |
| iOS | macOS and Xcode; supports devices and simulators. |
| Web | Emscripten and a matching Godot dynamic extension template. |
