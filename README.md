# godot-rust

[简体中文](README-ZH.md) | English

`godot-rust` is a Godot-first Rust language integration. It aims to make Rust
scripts as natural to create, attach, debug, and export as Godot's built-in
scripting languages, while preserving Rust's performance, type safety, and
Cargo ecosystem.

## Features

- **Godot-first workflow**: Create and attach `.rs` scripts, manage the project
  and its dependencies, build, inspect diagnostics, and export directly from
  Godot. Rust does not have to become the main entry point of the game.
- **Familiar scripting experience**: Use lifecycle callbacks, Inspector
  properties, signals, RPC, node references, script inheritance, parent calls,
  custom Resources, and global classes.
- **Complete typed Godot API**: Generate classes, Builtins, utilities,
  singletons, constants, signals, and virtual methods from the official Godot
  4.4–4.7 APIs. Every official entry must be generated or explicitly
  classified by the coverage checks.
- **A standard Cargo project**: A Godot project is also a regular Cargo project
  and works with crates.io, workspaces, rust-analyzer, Clippy, tests, and the
  existing Rust toolchain.
- **Reliable hot reload**: Load successful builds automatically and migrate
  compatible state. A failed build or validation keeps the last working
  generation active.
- **Delivery-oriented safety boundaries**: Contain script panics, validate the
  module ABI, build artifacts, and official API sources, and block incomplete
  releases with API coverage, cross-version, and platform export checks.
- **One cross-platform plugin**: A single plugin ZIP supports Linux, macOS,
  Windows, Android, iOS, and Web, with both `script` and `extension` project
  modes.

## Rust script example

```rust
use godot_rs::prelude::*;

#[script(base = Node)]
pub struct Player {
    #[export(default = 240.0, range(min = 0.0, max = 1000.0))]
    speed: f64,

    #[signal(args(speed))]
    speed_changed: Signal<(f64,)>,
}

#[script]
impl Player {
    pub const MAX_SPEED: f64 = 1000.0;

    fn _ready(&mut self) {
        self.speed_changed.emit((self.speed,));
    }

    #[func(name = "get_double_speed")]
    fn double_speed(&self) -> f64 {
        self.speed * 2.0
    }
}
```

Ordinary fields remain private Rust implementation details. `#[property]`
exposes a field to Godot without displaying it in the Inspector, while
`#[export]` also displays it in the Inspector. Public associated constants
automatically become Godot script constants, and private constants remain Rust
implementation details. `class_name` is only needed when the script should be
registered as a global script class.
