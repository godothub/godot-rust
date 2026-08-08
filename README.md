# godot-rust

English | [简体中文](README-ZH.md)

Godot 4.4–4.7 · Rust 1.85+ · MIT

`godot-rust` is a Godot-first Rust language integration. It makes Rust scripts
natural to create, attach, edit, diagnose, hot-reload, and export from Godot,
while preserving Rust's performance, type safety, and Cargo ecosystem.

The normal workflow starts in Godot rather than in a Rust application. A Godot
project is also a standard Cargo project, `.rs` files are real Script resources,
and the editor plugin coordinates the toolchain and runtime boundary.

## Highlights

- **Godot-first workflow** — create and attach `.rs` files through Godot's
  standard script dialog; use the editor for Cargo setup, checking, builds,
  diagnostics, dependencies, play, and export.
- **Complete generated API** — classes, Builtins, utilities, singletons,
  constants, signals, and virtual methods are generated from the official
  Godot 4.4–4.7 APIs. Coverage gates require every official entry to be
  generated or explicitly classified.
- **Reliable hot reload** — compatible Script state is migrated transactionally;
  rejected builds and incompatible candidates leave the last working generation
  active. `extension` classes also support reload with property restoration.
- **Actionable diagnostics** — Cargo messages appear in the Rust bottom panel
  with source navigation and previewable compiler fixes. Panics are contained
  at the ABI boundary and isolated to the affected script instance.
- **Standard Rust tooling** — crates.io dependencies, Cargo workspaces,
  rust-analyzer, Clippy, tests, CodeLLDB, and GDB remain available.
- **One cross-platform plugin** — one plugin ZIP contains the supported editor
  hosts and export runtimes for desktop, Android, iOS, and Web.

## Quick start

### 1. Install the plugin

Download the plugin ZIP from
[Releases](https://github.com/godothub/godot-rust/releases) and extract it into
the Godot project root. The resulting path must be:

```text
res://addons/godot-rust/plugin.cfg
```

Open the project in Godot, then enable **godot-rust** at:

```text
Project > Project Settings > Plugins
```

The ZIP already contains the platform-specific plugin binaries; users do not
select a separate download for each editor operating system.

### 2. Configure the Cargo project

If the project has no `Cargo.toml`, the plugin automatically runs the following
command without displaying a dialog or requiring any user action:

```sh
cargo init --lib --vcs none --name godothub_project .
```

It then adds the compatible `godot_rs` dependency, library targets, `script` mode
metadata, generated script entry, Cargo API selection, and editor integration.
It does not choose a Rust edition for `cargo init`, overwrite an existing
manifest, or replace an existing `src/lib.rs`.

Existing Cargo packages and workspaces are configured in place. Conflicting or
ambiguous settings stop setup before any file is changed.

### 3. Create a Rust script

1. Select a node in the scene tree and use Godot's **Attach Script** button.
2. Select **Rust** as the language.
3. Save the script below `res://src/scripts/`.
4. Click **Create**. The plugin generates the Rust type and stable Resource UID.

The generated script uses ordinary Rust plus metadata only where Godot needs it:

```rust
use godot_rs::prelude::*;

#[script(base = Node2D)]
pub struct Player {
    #[export(default = 240.0, range(min = 0.0, max = 1000.0))]
    speed: f64,

    #[signal(args(speed))]
    speed_changed: Signal<(f64,)>,
}

#[script]
impl Player {
    pub const MAX_SPEED: f64 = 1000.0;

    fn _ready(&mut self) -> EngineResult<()> {
        self.base().set_process(true)?;
        godot_print!("Player is ready");
        Ok(())
    }

    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let offset = Vector2::new(self.speed * delta, 0.0);
        self.base().translate(offset)?;
        Ok(())
    }

    #[func(name = "double_speed")]
    fn doubled_speed(&self) -> f64 {
        self.speed * 2.0
    }
}
```

### 4. Build and run

Use **Project > Tools > Rust: Build** or the **Build** button in the Rust bottom
panel. A successful build is validated before it becomes the active project
module. F5/F6 checks the content-verified build receipt and performs an
incremental build only when Rust inputs or toolchain evidence changed.

Saving a Rust file schedules a debounced Cargo Check by default. Errors and
warnings appear in the Rust panel; double-click a diagnostic to open its `.rs`
location.

## Script model

| Rust syntax | Godot behavior |
| --- | --- |
| `#[script(base = Node2D)]` | Declares an attachable Rust Script and its Godot base class. |
| `#[script] impl Player` | Collects lifecycle callbacks and reflected members from this impl block. |
| `_ready`, `_process`, `_input`, … | Registers a correctly typed lifecycle callback automatically. |
| `#[export(...)]` | Creates a saved property visible in the Inspector. |
| `#[property(...)]` | Creates a Godot property hidden from the Inspector. |
| `#[node("%Name")]` | Resolves and type-checks a scene node before `_ready`. |
| `#[signal(args(...))]` | Declares a typed Godot signal. |
| `#[func]` | Exposes a typed method to Godot. |
| `#[rpc(...)]` | Exposes the method with validated Godot RPC metadata. |
| `pub const` | Exposes the compiled value as a script constant. |
| `#[godot_enum]` / `#[godot_flags]` | Defines forward-compatible project enum/flag values. |

`#[script]` on an impl is the compile-time collection marker required for a
separate stable-Rust impl block. Pure Rust helpers can live in ordinary impl
blocks without it. Multiple marked impl blocks for the same type are merged
into one deterministic method table.

`godot_rs::prelude::*` is a one-line script-oriented convenience import. Normal
explicit Rust imports remain supported.

## Advanced usage

### Properties, accessors, enums, and flags

`#[property]` and `#[export]` use the same typed storage and accessor path. The
difference is Inspector visibility. `read_only` is enforced in both Godot
metadata and the project ABI.

```rust
#[godot_enum]
enum MovementMode {
    Idle = 0,
    #[default]
    Walk = 1,
    Sprint = 2,
}

#[godot_flags]
enum Ability {
    Jump = 1 << 0,
    Dash = 1 << 1,
}

#[script(base = CharacterBody2D, class_name = Player)]
pub struct Player {
    stamina_cache: f64,

    #[property(default = 100, get = health, set = set_health)]
    health: i64,

    #[property(default = 1, read_only)]
    level: i64,

    #[export(default = MovementMode::Walk)]
    movement: MovementMode,

    #[export(default = Ability::Jump | Ability::Dash)]
    abilities: Ability,

    #[node("%Sprite")]
    sprite: NodeRef<AnimatedSprite2D>,
}

#[script]
impl Player {
    pub const MAX_HEALTH: i64 = 100;

    fn health(&self) -> i64 {
        self.health
    }

    fn set_health(&mut self, value: i64) {
        self.health = value.clamp(0, Self::MAX_HEALTH);
    }

    #[rpc(name = "sync_health", authority, reliable)]
    fn apply_network_health(&mut self, value: i64) {
        self.set_health(value);
    }
}
```

Custom enums and flags retain unknown integer values. Loading data written by a
newer game version therefore never creates an invalid Rust enum.

### Inheritance, Resources, and global classes

- Add `class_name = Name` only when the script should be globally registered.
- Use `base = Resource` for custom Rust Resource scripts.
- Use `extends = "res://path/to/base.rs"` for project-local Rust script
  inheritance, then call the next implementation with typed
  `self.call_super(...)`.
- Cycles, missing bases, base-class mismatches, duplicate members, and invalid
  reflected signatures are rejected before the module becomes active.

### Async tasks and engine APIs

- `spawn`, `next_frame`, `sleep`, `timeout`, and typed Signal futures run
  cooperatively on the Godot main thread. `spawn_blocking` is available for
  owned blocking work.
- `self.base()` exposes the current Godot object. Generated class methods,
  constructors, Builtins, utilities, singletons, constants, signals, and
  virtual interfaces use the API selected for the project.
- `godot_debug!`, `godot_print!`, `godot_warn!`, and `godot_error!` route to the
  Godot debugger in both project modes.
- Script errors return through `EngineResult`/`ScriptResult`; Rust panics never
  unwind into Godot.

### Hot-reload state policy

- Inspector `#[export]` values are preserved by default.
- `#[reload(persist)]` preserves a supported non-exported value explicitly.
- `#[reload(skip)]` and ordinary internal fields use their new default values.
- Node fields are resolved again against the current scene.
- A field, method, inheritance, or ABI incompatibility keeps the previous
  generation active and asks for a scene/editor restart when required.

## Project modes

| Mode | Intended use | Runtime model |
| --- | --- | --- |
| `script` | Recommended for normal Godot gameplay scripts. | `.rs` Script resources run through the plugin Host, with inheritance and transactional state migration. |
| `extension` | Advanced direct ClassDB integration and low-level engine extensions. | Rust `NativeClass` types register directly through Godot's native extension interface and retain registered property state on supported reloads. |

Select the mode in `Cargo.toml`:

```toml
[package.metadata.godot-rust]
enabled = true
mode = "script"
godot = "4.4"
scripts = "src/scripts"
```

The `godot` value selects the official major/minor API used by the entire Rust
build. It may be `4.4`, `4.5`, `4.6`, or `4.7`, and must not be newer than the
running editor/runtime. Patch versions are intentionally not accepted.

## Workflow configuration

```toml
[package.metadata.godot-rust]
auto-check = true
build-before-play = true
auto-build-on-idle = false
```

| Setting | Default behavior |
| --- | --- |
| `auto-check` | Coalesces rapid saves and runs Cargo Check in the background. |
| `build-before-play` | Requires a verified current build before F5/F6 starts the game. |
| `auto-build-on-idle` | When `auto-check = false`, builds a changed project after the editor becomes idle. |

Cargo workspaces can select the owning package with
`[workspace.metadata.godot-rust].package`. The plugin also provides dependency
editing, exact Rust target installation through rustup, Safe Mode, support
bundles, and external-editor integration without overwriting user-owned files.

## Export support

Export from Godot's normal **Project > Export** window. The plugin selects the
Cargo Debug/Release profile, builds the architectures enabled in the Godot
preset, validates every output, and gives only verified artifacts to Godot's
exporter.

| Platform | Supported Rust targets |
| --- | --- |
| Linux | x86_32, x86_64, ARM32, ARM64 |
| Windows | x86_32, x86_64, ARM64 |
| macOS | x86_64, ARM64, Universal |
| Android | ARMv7, ARM64, x86, x86_64 |
| iOS | ARM64 device plus ARM64/x86_64 Simulator XCFramework slices |
| Web | `wasm32-unknown-emscripten` with the matching Godot dynamic template |

Desktop Rust exports run on the corresponding host operating system. Apple
exports require macOS and Xcode. Android and Web additionally require their
platform toolchains. Missing tools, targets, templates, signatures, or ABI
validation fail the export instead of producing a partial package.

## Developing the repository

This repository includes a complete Godot project in `example/`. Its
`addons/godot-rust` directory is also the canonical source used to assemble the
release plugin.

```sh
git clone https://github.com/godothub/godot-rust.git
cd godot-rust
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 tools/stage_example_plugin.py
```

Use Python 3.11 or newer for repository tools; API verification and release
tools use its standard `tomllib` module. The staging command builds the current
editor host, build service, and module validator into the example project. Open
`example/project.godot` with Godot 4.4 or newer after staging.

### Repository layout

| Path | Responsibility |
| --- | --- |
| `example/` | Runnable Godot + Cargo example and canonical addon source. |
| `crates/godot_rs/` | Public application SDK; this is the dependency user projects import. |
| `crates/godot_macro/` | Procedural macros re-exported by `godot_rs`. |
| `crates/godot_api/` | Versioned official API model, generated bindings, and stable project ABI. |
| `crates/godot_host/` | Godot ScriptLanguage Host and runtime integration. |
| `crates/godot_build/` | Cargo setup, diagnostics, build, reload publication, and export orchestration. |
| `crates/godot_codegen/` | Authenticated API bundle generation and coverage policy. |
| `tools/` | Release, integration, export, and API verification tools. |

Application projects should depend on `godot_rs` only. Its macro and API
dependencies are implementation details and are re-exported where needed.

## Documentation

- [Example project and complete workflow guide](example/README.md)
- [`godot_rs` SDK overview](crates/godot_rs/README.md)
- [English changelog](CHANGELOG.md)
- [中文更新日志](CHANGELOG-ZH.md)

## License

The project is licensed under the [MIT License](LICENSE). The release addon
also includes the license information for the official Godot API inputs used
to generate the Rust interface.
