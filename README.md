# Godot Rust

English | [简体中文](README-ZH.md)

**Write your Godot game in Rust.**

Create and attach `.rs` files just like GDScript files, write game logic in Rust,
and export your game through Godot. Keep your familiar editor workflow with
Rust's type checking and Cargo ecosystem.

## Features

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

For example, attach this script to a `Node2D` to move it with the arrow keys:

```rust
use godot_rs::prelude::*;

#[script(base = Node2D)]
pub struct Player;

#[script]
impl Player {
    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let direction = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
        self.base().translate(direction * (240.0 * delta) as f32)?;
        Ok(())
    }
}
```

`#[script]` declares the script and its callbacks. `self.base()` accesses the
attached node. Declaring `_process` automatically enables the per-frame callback.

### 3. Run and iterate

Press **F5** to run the project or **F6** to run the current scene. The plugin
builds Rust code when needed. To build manually, use **Build** in the bottom
**Rust** panel or **Project > Tools > Rust: Build**.

Saving checks your code; use **Build** to apply changes. Inspector properties
refresh automatically, and `tool` scripts update without reopening the scene.
Existing property values are kept when their types stay compatible. Regular
scripts run only in the game; restart the game to try your changes.

If an update cannot be applied, Godot's Output panel explains why. Changing a
script's Godot base class or `tool` setting requires closing and reopening the
affected scene, or removing and reattaching its script. See the
[hot reload guide](example/README.md#热重载) for field reset options and troubleshooting.

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

## Advanced usage

The following examples add state to `Player`: replace `pub struct Player;` with
`pub struct Player { ... }` and put the fields inside the braces. Add methods to
its `#[script] impl`. Statements using `?` belong in callbacks returning
`EngineResult`.

### Input and default arguments

Call singletons directly and use Godot's defaults for common options:

```rust
let direction = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
let pressed = Input::is_action_pressed("ui_accept")?;
```

To override optional arguments, use `*_ex(...)` and finish with `.done()`:

```rust
let input = Input::singleton()?;
let pressed = input.is_action_pressed_ex("ui_accept")
    .exact_match(true)
    .done()?;
```

### Node bindings and script interaction

Declare frequently used nodes as fields of `Player`. `%Health` refers to a scene unique name;
regular paths such as `"Children/Health"` work too:

```rust
#[node("%Health")]
health: NodeRef<Health>,

#[node("%Camera")]
camera: Option<NodeRef<Camera2D>>,
```

`Health` can be another Rust script's type. Mark a method with `#[func]` to call it
from other scripts:

```rust
// health.rs
use godot_rs::prelude::*;

#[script(base = Node)]
pub struct Health {
    #[export(default = 100)]
    points: i64,
}

#[script]
impl Health {
    #[func]
    fn damage(&mut self, amount: i64) -> i64 {
        self.points = (self.points - amount).max(0);
        self.points
    }
}
```

Add `use super::health::Health;` to `player.rs`, attach `health.rs` to the Health
node, and give it a scene unique name. Module paths follow script files: use
`super::health::Health` for a sibling file or
`crate::scripts::actors::enemy::Enemy` for a nested file. Then call it from a
`Player` callback:

```rust
let remaining = self.health.damage(10)?;
let current = self.health.properties().points()?;
self.health.properties().set_points(100)?;
```

Method arguments and return types are checked at compile time. Property access
respects declared getters and setters. Missing required nodes and incorrect attached
scripts produce errors; absent optional nodes become `None`. For occasional lookups,
use `self.base().node::<Health>("%Health")?`. If a script method shares an engine
method's name, use `self.health.script().method(...)` to select the script method.

### Inspector properties and private state

Use `#[export]` for Inspector properties and `#[property]` for properties accessible
to Godot but hidden from the Inspector. Other fields remain private Rust state and
can be initialized with expressions. For example, add these fields to `Player`:

```rust
#[export(default = 240.0, group = "Movement", range(min = 0, max = 1000))]
speed: f64,

#[property(default = 1, read_only)]
level: i64,

#[init(default = Vec::with_capacity(32))]
recent_hits: Vec<i64>,
```

To use `speed` for movement, replace `240.0` in `_process` with `self.speed`.
Initialization expressions run when script state is created and are useful for
private caches and containers. Direct field assignments follow normal Rust semantics;
call your own setter explicitly when you need validation or notifications.

### Custom resources

Put configuration in a Resource to share it across characters, weapons, or levels:

```rust
use godot_rs::prelude::*;

#[script(base = Resource)]
pub struct WeaponConfig {
    #[export(default = 25)]
    damage: i64,
}
```

Save the script as `weapon_config.rs`, attach it to a Resource, and save that as a
`.tres`. Add `use super::weapon_config::WeaponConfig;` to the consuming script,
then load the resource:

```rust
let weapon = load::<WeaponConfig>("res://weapons/sword.tres")?;
let damage = weapon.properties().damage()?;
```

Alternatively, declare `#[export] weapon: Option<GodotRef<WeaponConfig>>` to assign
it in the Inspector. `GodotRef<T>` keeps a resource alive. `NodeRef<T>` refers to a
Godot-managed node and does not prevent that node from being freed.

### Signals and callbacks

Add this signal field to the `Health` struct above, declaring names and types together:

```rust
#[signal]
damaged: Signal<fn(amount: i64, remaining: i64)>,
```

Replace `Health::damage` with this method to emit a signal for each hit:

```rust
#[func]
fn damage(&mut self, amount: i64) -> EngineResult<i64> {
    self.points = (self.points - amount).max(0);
    self.damaged.emit(amount, self.points)?;
    Ok(self.points)
}
```

Add a `_ready` callback to `Player` to connect before other game logic calls `damage`:

```rust
fn _ready(&mut self) -> EngineResult<()> {
    self.health.signals().damaged()
        .connect_deferred(&self.callbacks()?.on_damaged())?;
    Ok(())
}
```

Add the target callback to `Player`'s `#[script] impl`:

```rust
#[func]
fn on_damaged(&mut self, amount: i64, remaining: i64) {
    godot_print!("Took {amount} damage; {remaining} remaining");
}
```

Mismatched parameters produce a compile error. Use `connect_method(...)` for
synchronous delivery or `connect_deferred(...)` when a signal may return to the
currently executing script. Godot's signal panel remains available. For automatic
disconnection, use `connect_scoped(...)` and retain the returned connection object;
dropping it disconnects the signal.

### Await signals and run async tasks

Using the `Health::damaged` signal above, create a wait in a `Player` callback,
then run it as a task belonging to that script:

```rust
let damaged = self.health.signals().damaged().wait();
self.tasks().spawn(async move {
    match damaged.await {
        Ok((amount, remaining)) => godot_print!("Damage: {amount}; remaining: {remaining}"),
        Err(error) => godot_warn!("Wait interrupted: {error}"),
    }
});
```

The wait starts listening when the task first runs. Tasks in `self.tasks()` are
cancelled when the script is destroyed or replaced by hot reload. To stop work when
a node leaves the tree, retain its `TaskHandle` and cancel it in `_exit_tree`.
Use `next_frame()`, `sleep(...)`, and `timeout(...)` without blocking the game loop.
Send expensive pure computation to `spawn_blocking(...)`, keeping node and resource
operations on the main thread.
