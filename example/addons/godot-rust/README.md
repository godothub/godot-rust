# godot-rust

Write Rust scripts in Godot, edit their properties in the Inspector, and export
your game through Godot's familiar workflow.

## Get started

You need Godot 4.4–4.7, Rust 1.85 or newer with Cargo, and your platform's native
compiler toolchain.

1. Copy `addons/godot-rust` into your Godot project.
2. Enable **godot-rust** in **Project > Project Settings > Plugins**.
3. Let the plugin configure Cargo, or follow the setup prompt for an existing
   Cargo project.
4. Create a **Rust** script from Godot's Attach Script dialog and save it under
   `res://src/scripts/`.

Press **F5** to run the project or **F6** to run the current scene. The plugin
builds Rust when needed. Use the bottom **Rust** panel to build manually, inspect
compiler diagnostics, apply suggested fixes, or cancel a running operation.

## Work with Rust in Godot

- Expose fields to the Inspector, methods to Godot, and typed signals to the
  standard signal connection dialog.
- Get syntax highlighting in Godot's script editor, or use your preferred
  external editor and rust-analyzer.
- Check code automatically when you save. Build to update Inspector properties
  and `tool` scripts without restarting the editor, keeping compatible property
  values. Regular scripts run only in the game. If an update cannot be applied,
  check Godot's Output panel for the reason.
- Use Cargo dependencies and workspaces, custom Resources, script inheritance,
  and cooperative async tasks. Extension Mode is available for registering
  native Godot classes directly.

## Export your game

Use **Project > Export**. Install the matching Godot export templates and native
platform toolchains first. The plugin builds and packages the Rust runtime for
Windows, Linux, macOS, Android, iOS, or Web. Mobile and Web targets need their
platform SDKs; Web also needs a matching Godot dynamic extension template.

See the [full guide](https://github.com/godothub/godot-rust/blob/main/example/README.md)
for examples, platform setup, and hot reload details. Report problems through
[GitHub Issues](https://github.com/godothub/godot-rust/issues).

The release includes the project license, dependency notices, and
[Godot's license](GODOT_LICENSE.md).
