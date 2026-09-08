## 0.9.1

- Simplify singleton calls, default arguments, node bindings, and script imports.
- Call other Rust scripts' methods and access their properties directly, with
  support for custom Rust Resources.
- Declare signals more concisely, connect methods with compile-time type checks,
  and await signals to receive their arguments.
- Add asynchronous tasks tied to a script's lifetime, automatically cancelled
  when the script is destroyed or replaced during hot reload.
- Fix premature resource release and references targeting the wrong node,
  reduce allocations when emitting signals, and improve error messages.

## 0.9.0

- Use Rust with Godot 4.4–4.7 as attachable scripts or native extension classes.
- Add Inspector properties, typed methods and signals, RPC, and inheritance to
  Rust scripts. Expose Rust constants, enums, and flags to Godot.
- Start new Cargo projects automatically and manage dependencies, view compiler
  diagnostics, and apply compiler fixes from Godot. Existing Cargo projects,
  workspaces, and external editors are supported.
- Hot-reload compatible code changes after a successful build, preserving
  Inspector values. Failed updates keep the previous working version; script
  structure changes require restarting the scene.
- Export to Linux, macOS, Windows, Android, iOS, and Web through Godot's export
  window, with the required platform toolchains installed.
