## 0.9.0

- Use a generated, version-matched Rust SDK for Godot 4.4–4.7 in both `script` and `extension` modes, including classes, Builtins, utilities, singletons, constants, signals, and virtual methods.
- Create Rust scripts with typed Inspector and hidden properties, custom accessors, methods, signals, RPC, inheritance, parent calls, stable Resource UIDs, and compile-time validation.
- Expose normal public Rust constants and forward-compatible custom enums and flags to Godot without duplicating declarations.
- Hot-reload successful builds without restarting the editor, preserve compatible state, and keep the previous working generation when an update fails.
- Manage Cargo projects, workspaces, dependencies, diagnostics, compiler fixes, Rust targets, Safe Mode, and external editors directly from Godot.
- Build and export Script Mode and Extension Mode games for Linux, macOS, Windows, Android, iOS, and Web.
- Install the SDK through Cargo with bindings generated only for the selected Godot 4.4–4.7 API.
