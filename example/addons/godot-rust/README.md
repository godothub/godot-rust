# godot-rust addon

This directory is the canonical release-package root for the `godot-rust`
Godot plugin. Release automation copies this directory once and injects every
supported desktop platform into `bin/<platform>/`. The resulting release is
one installable ZIP whose root is `addons/godot-rust`. `GODOT_LICENSE.md`
records the license for the official Godot API inputs used to generate the
Rust interface files.

Each platform directory contains:

- `godot_host` dynamic library;
- `godot_build` executable;
- `godot_module_check` executable.

The editor plugin keeps the project as a standard Cargo package. It monitors
Rust inputs, coalesces rapid saves into one background Cargo Check, displays
structured diagnostics in the Rust bottom panel, and runs a fail-closed
foreground build before Godot starts the project. A content-verified Build
Receipt skips Cargo when the Rust inputs, Cargo configuration, toolchain,
packaged tools, and published module still match the last successful build.
Missing, corrupt, or stale evidence falls back to the full build gate.
Check/Build actions use the same packaged build service and the user's local
Cargo toolchain. Godot's standard Node signal dialog can create typed Rust
callbacks, and separate generated `#[script] impl` blocks are merged into one
script method table at compile time. Godot's standard Export window builds the
matching Cargo Debug or Release target, validates the project ABI, and hands
the runtime module to Godot's platform exporter. Exported Rust resource paths
are retained with source-private placeholder contents.

Successful editor builds are loaded at a main-thread safe point. Compatible
live state is migrated transactionally; a rejected candidate leaves the
previous generation active. The SDK also provides cooperative main-thread
futures, named Rust classes, custom Resources, and Rust-script inheritance.
Inherited Rust scripts can explicitly call the next implementation with the
typed `self.call_super(...)` API, including lifecycle callbacks such as
`self.call_super::<(), _>("_ready", ())`; trailing `#[default(...)]`
arguments are applied by the retained base-script generation.
Project setup creates rust-analyzer, Cargo task, CodeLLDB, and GDB files only
when the matching user file does not already exist.

The editor controls follow the Godot editor scale and theme, wrap their action
bars at narrow widths, expose keyboard focus and shortcuts, and provide
English and Simplified Chinese labels from an addon-private catalog. Godot
4.5 and newer also receive explicit accessibility names, descriptions, and
polite live status announcements through the engine's accessibility API.
Godot 4.4 retains the same keyboard workflow and descriptive tooltips because
that version does not expose those screen-reader properties to extensions.

Rust callback errors are reported through Godot's debugger with the `.rs`
resource path and callback name. Panics are contained at the project-module
ABI boundary. A panic or three consecutive callback failures disables only the
affected script instance; other instances of the same script keep running.

Extension Mode registers complete ClassDB metadata and generated virtual
overrides directly from Rust. Export validation covers four Android ABIs,
real Android emulator execution, iOS device and Simulator XCFramework slices,
real iOS Simulator execution, desktop applications, and Web browser runtime.

The GDScript components have separate responsibilities:

- `source_monitor.gd`: bounded input fingerprinting and save debounce;
- `build_process.gd`: cancellable background and foreground requests;
- `build_protocol.gd`: bounded request, response, and cancellation transport;
- `diagnostics_panel.gd`: Cargo diagnostics and source activation;
- `export_plugin.gd`: fail-closed Rust builds and runtime-module packaging;
- `plugin.gd`: Godot workflow coordination.

Do not package files from `tests/fixtures/`; those projects only exercise the
canonical addon. Release assembly also embeds the project and Godot licenses,
a CycloneDX SBOM, a Cargo third-party license inventory, and a SHA-256 manifest
inside this single addon ZIP.
