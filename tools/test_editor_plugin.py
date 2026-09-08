#!/usr/bin/env python3
"""Load the canonical editor plugin in official Godot and reject script errors."""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from stage_example_plugin import stage_example_plugin


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "example"
PROBE_MARKER = "godot-rust: project probe completed."
SCRIPT_CREATED_MARKER = "GODOT_RUST_DIALOG_SCRIPT_CREATED"
SCRIPT_SIGNAL_MARKER = "GODOT_RUST_DIALOG_SIGNAL_EMITTED"
AUTO_CHECK_MARKER = "GODOT_RUST_AUTOMATIC_CHECK_COMPLETED"
SCRIPT_INDEXED_MARKER = "GODOT_RUST_SCRIPT_INDEXED"
DIAGNOSTIC_SOURCE_MARKER = "GODOT_RUST_DIAGNOSTIC_SOURCE_OPENED"
SUPER_CALL_MARKER = "Inherited Rust script returned:"
SUPER_LIFECYCLE_MARKER = "Base Rust script _ready called."
REQUIRED_MARKERS = (
    PROBE_MARKER,
    SCRIPT_SIGNAL_MARKER,
    SCRIPT_CREATED_MARKER,
    AUTO_CHECK_MARKER,
    SCRIPT_INDEXED_MARKER,
    DIAGNOSTIC_SOURCE_MARKER,
    "GODOT_RUST_EDITOR_LANGUAGE_CONTRACT",
    "GODOT_RUST_EDITOR_INDENTATION_READY",
    "GODOT_RUST_EDITOR_HIGHLIGHTING_READY",
    "GODOT_RUST_SIGNAL_CALLBACK_CREATED",
    "GODOT_RUST_BUILD_BEFORE_PLAY_SUCCEEDED",
    "GODOT_RUST_GLOBAL_CLASSES_REGISTERED",
    SUPER_CALL_MARKER,
    SUPER_LIFECYCLE_MARKER,
    "GODOT_RUST_HOT_RELOAD_STATE_PRESERVED",
    "GODOT_RUST_BUILD_RECEIPT_REUSED",
    "GODOT_RUST_RECEIPT_REUSE_MSEC",
    "GODOT_RUST_UNCHANGED_BUILD_REUSED",
    "godot-rust: Automatic Rust check succeeded.",
)
FLOW_PLUGIN = ROOT / "tests" / "fixtures" / "editor_flow"
DIAGNOSTICS_PLUGIN = ROOT / "tests" / "fixtures" / "editor_diagnostics"
SETUP_PLUGIN = ROOT / "tests" / "fixtures" / "editor_setup"
INITIALIZE_PLUGIN = ROOT / "tests" / "fixtures" / "editor_initialize"
DIAGNOSTICS_MARKERS = (
    PROBE_MARKER,
    "GODOT_RUST_BASELINE_RECEIPT_RECORDED",
    "GODOT_RUST_ACCESSIBLE_LOCALIZED_EDITOR_UI",
    "GODOT_RUST_FAILED_CHECK_OBSERVED",
    "GODOT_RUST_BUILD_BEFORE_PLAY_REJECTED",
    "GODOT_RUST_DIAGNOSTICS_PANEL_POPULATED",
    "GODOT_RUST_FAILED_DIAGNOSTIC_OPENED",
    "GODOT_RUST_REDACTED_SUPPORT_BUNDLE",
    "GODOT_RUST_RAPID_SAVES_COALESCED",
    "GODOT_RUST_CHECK_RECOVERED",
    "GODOT_RUST_BUILD_BEFORE_PLAY_RECOVERED",
)
SETUP_MARKERS = (
    PROBE_MARKER,
    "GODOT_RUST_SETUP_USER_FILES_PRESERVED",
    "GODOT_RUST_SETUP_CONFIGURED",
    "GODOT_RUST_SETUP_BUILD_SUCCEEDED",
)
INITIALIZE_MARKERS = (
    PROBE_MARKER,
    "GODOT_RUST_INITIALIZATION_AUTOMATIC",
    "GODOT_RUST_INITIALIZATION_CONFIGURED",
)
EXPECTED_CARGO_FAILURES = (
    (
        "ERROR: godot-rust: Automatic Rust check failed; "
        "see Rust diagnostics above."
    ),
    (
        "ERROR: godot-rust: Rust build before play failed; "
        "see Rust diagnostics above."
    ),
)
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
ERROR_PATTERNS = (
    re.compile(r"\bERROR:", re.IGNORECASE),
    re.compile(r"SCRIPT ERROR:", re.IGNORECASE),
    re.compile(r"Parse Error:", re.IGNORECASE),
    re.compile(r"Failed to load script", re.IGNORECASE),
    re.compile(r"Invalid plugin", re.IGNORECASE),
    re.compile(r"ObjectDB instances? (?:(?:was|were) )?leaked at exit", re.IGNORECASE),
    re.compile(r"resources still in use at exit", re.IGNORECASE),
)
EDITOR_FLOW_TIMEOUT_SECONDS = 300
COMPONENT_MARKER = "GODOT_RUST_EDITOR_COMPONENTS_PASSED"
COMPONENT_TEST_SCRIPT = '''\
@tool
extends EditorPlugin

const SourceMonitor = preload("res://addons/godot-rust/source_monitor.gd")
const DiagnosticsPanel = preload("res://addons/godot-rust/diagnostics_panel.gd")
const RustPlugin = preload("res://addons/godot-rust/plugin.gd")
const RustSyntaxHighlighter = preload("res://addons/godot-rust/syntax_highlighter.gd")

class PendingRequest:
    extends RefCounted

    var running := true

    func is_running() -> bool:
        return running

var cancellation_requested := false


func _enter_tree() -> void:
    call_deferred("_run")


func _run() -> void:
    for _frame in 5:
        await get_tree().process_frame
    if not _verify_syntax_highlighter():
        return
    var monitor = SourceMonitor.new()
    add_child(monitor)
    for path in [
        "res://src/owned/mod.rs",
        "res://src/empty/mod.rs",
        "res://src/header_prefix/mod.rs",
    ]:
        if not monitor._is_tracked_path(path):
            _fail("User-owned Rust module is excluded from monitoring: " + path)
            return
    for path in ["res://src/generated/mod.rs", "res://src/generated_crlf/mod.rs"]:
        if monitor._is_tracked_path(path):
            _fail("Generated script index would retrigger automatic checks")
            return
    monitor.queue_free()

    var panel = DiagnosticsPanel.new()
    add_control_to_bottom_panel(panel, "Rust component test")
    # Godot 4.4 schedules bottom-panel focus/scroll updates for a later frame.
    for _frame in 2:
        await get_tree().process_frame
    panel.show_cargo_report("Rust check", {
        "success": true,
        "diagnostics": [{"level": "warning", "message": "test diagnostic"}],
    })
    if panel._tree.get_root().get_first_child() == null:
        _fail("Diagnostic fixture did not populate the real panel")
        return
    panel.set_running("Rust build")
    var running_status: String = panel._status_label.text
    panel._clear_button.pressed.emit()
    if panel._tree.get_root().get_first_child() != null:
        _fail("Clear did not remove the diagnostic rows")
        return
    if (
        panel._cancel_button.disabled
        or not panel._check_button.disabled
        or not panel._build_button.disabled
        or panel._status_label.text != running_status
    ):
        _fail("Clearing diagnostics changed the active operation controls")
        return
    panel.cancel_requested.connect(_on_cancel_requested)
    panel._cancel_button.pressed.emit()
    if not cancellation_requested:
        _fail("The active build could not be cancelled after clearing diagnostics")
        return
    panel.set_status("Finished")
    panel._clear_button.pressed.emit()
    if panel._check_button.disabled or not panel._cancel_button.disabled:
        _fail("Clearing idle diagnostics changed the available actions")
        return
    var plugin = RustPlugin.new()
    plugin._diagnostics_panel = panel
    var request = PendingRequest.new()
    plugin._build_process = request
    panel.set_running("Rust build")
    plugin._show_toolchain_repair()
    if panel._cancel_button.disabled or not panel._build_button.disabled:
        plugin.free()
        _fail("A rejected tool action changed the active build controls")
        return
    plugin._set_workflow_status("Opened a diagnostic source")
    if panel._cancel_button.disabled or not panel._check_button.disabled:
        plugin.free()
        _fail("An informational status changed the active build controls")
        return
    request.running = false
    panel.set_running("Native reload commit")
    plugin._request_finished("Native reload commit", {"ok": true, "result": {}}, "")
    if (
        panel._check_button.disabled
        or panel._build_button.disabled
        or not panel._cancel_button.disabled
    ):
        plugin.free()
        _fail("A successful service response left the Rust panel busy")
        return
    plugin.free()
    for _frame in 2:
        await get_tree().process_frame
    remove_control_from_bottom_panel(panel)
    panel.queue_free()
    await get_tree().process_frame
    while get_editor_interface().get_resource_filesystem().is_scanning():
        await get_tree().process_frame
    print("GODOT_RUST_EDITOR_COMPONENTS_PASSED")
    get_tree().quit()


func _on_cancel_requested() -> void:
    cancellation_requested = true


func _verify_syntax_highlighter() -> bool:
    var editor := CodeEdit.new()
    var highlighter = RustSyntaxHighlighter.new()
    var buffer_id: int = highlighter._buffer.get_instance_id()
    editor.syntax_highlighter = highlighter
    editor.text = 'fn callback() { "text"; /* note\\ncontinued */ let value = 42; }'
    var colors: Dictionary = highlighter.get_line_syntax_highlighting(0)
    var settings := get_editor_interface().get_editor_settings()
    var prefix := "text_editor/theme/highlighting/"
    var failure := ""
    if _color_at(colors, 0) != settings.get_setting(prefix + "keyword_color"):
        failure = "Rust keywords did not use the editor keyword color"
    elif _color_at(colors, 16) != settings.get_setting(prefix + "string_color"):
        failure = "Rust string contents did not use the editor string color"
    elif _color_at(highlighter.get_line_syntax_highlighting(1), 0) != settings.get_setting(prefix + "comment_color"):
        failure = "Rust multiline comment state was not preserved"
    editor.text = 'let value = 42;\\nfn callback() {}'
    if _color_at(highlighter.get_line_syntax_highlighting(1), 0) != settings.get_setting(prefix + "keyword_color"):
        failure = "Editing Rust source retained stale multiline highlighting"
    var alternate_color := Color(0.12, 0.34, 0.56)
    var original_color: Color = settings.get_setting(prefix + "keyword_color")
    settings.set_setting(prefix + "keyword_color", alternate_color)
    highlighter.update_cache()
    if _color_at(highlighter.get_line_syntax_highlighting(1), 0) != alternate_color:
        failure = "Rust highlighting did not refresh after the editor theme changed"
    settings.set_setting(prefix + "keyword_color", original_color)
    editor.free()
    highlighter = null
    if is_instance_id_valid(buffer_id):
        failure = "Rust syntax highlighter leaked its off-tree text buffer"
    if not failure.is_empty():
        _fail(failure)
        return false
    return true


func _color_at(colors: Dictionary, column: int) -> Variant:
    var color: Variant = null
    for start in colors:
        if int(start) > column:
            break
        color = colors[start].get("color")
    return color


func _fail(message: String) -> void:
    push_error(message)
    get_tree().quit(1)
'''


def discover_godot(explicit: str | None) -> str | None:
    candidates = (
        explicit,
        os.environ.get("GODOT_BIN"),
        shutil.which("godot4"),
        shutil.which("godot"),
    )
    return next((candidate for candidate in candidates if candidate), None)


def output_errors(
    output: str,
    required_markers: tuple[str, ...] = REQUIRED_MARKERS,
    allow_expected_cargo_failure: bool = False,
) -> list[str]:
    patterns = ERROR_PATTERNS
    if allow_expected_cargo_failure:
        patterns = ERROR_PATTERNS[1:]
    plain_output = ANSI_ESCAPE.sub("", output)
    errors = [
        pattern.pattern for pattern in patterns if pattern.search(plain_output)
    ]
    if allow_expected_cargo_failure:
        for line in output.splitlines():
            plain_line = ANSI_ESCAPE.sub("", line)
            if (
                "ERROR:" in plain_line
                and not any(
                    expected in plain_line
                    for expected in EXPECTED_CARGO_FAILURES
                )
            ):
                errors.append(
                    f"unexpected editor error: {plain_line.strip()}"
                )
    for marker in required_markers:
        if marker not in output:
            errors.append(f"missing marker {marker}")
    return errors


def prepare_test_project(destination: Path, scenario: str = "flow") -> None:
    shutil.copytree(
        EXAMPLE,
        destination,
        ignore=shutil.ignore_patterns(".godot", "target"),
    )
    dependency = (ROOT / "crates" / "godot_rs").resolve().as_posix()
    manifest = destination / "Cargo.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            'path = "../crates/godot_rs"',
            f'path = "{dependency}"',
        ),
        encoding="utf-8",
    )
    if scenario == "flow":
        fixture = FLOW_PLUGIN
        plugin_name = "godot-rust-editor-flow"
    elif scenario == "diagnostics":
        fixture = DIAGNOSTICS_PLUGIN
        plugin_name = "godot-rust-editor-diagnostics"
    elif scenario == "setup":
        fixture = SETUP_PLUGIN
        plugin_name = "godot-rust-editor-setup"
        manifest.write_text(
            f"""\
# preserve this Cargo comment
[package]
name = "existing_game"
version = "0.1.0"
edition = "2024"

[dependencies]
godot_rs = {{ path = "{dependency}" }}

[workspace]
""",
            encoding="utf-8",
        )
        (destination / "src" / "lib.rs").write_text(
            "pub fn existing_user_code() -> u32 { 7 }\n",
            encoding="utf-8",
        )
        shutil.rmtree(destination / "src" / "scripts")
        shutil.rmtree(destination / ".cargo")
        (destination / "project.godot").write_text(
            """\
[application]
config/name="godot-rust editor setup test"

[editor_plugins]
enabled=PackedStringArray("godot-rust")
""",
            encoding="utf-8",
        )
        (destination / "main.tscn").unlink()
    elif scenario == "initialize":
        fixture = INITIALIZE_PLUGIN
        plugin_name = "godot-rust-editor-initialize"
        (destination / "Cargo.toml").unlink()
        (destination / "Cargo.lock").unlink()
        shutil.rmtree(destination / "src")
        shutil.rmtree(destination / ".cargo")
        (destination / "project.godot").write_text(
            """\
[application]
config/name="godot-rust editor initialization test"

[editor_plugins]
enabled=PackedStringArray("godot-rust")
""",
            encoding="utf-8",
        )
        (destination / "main.tscn").unlink()
    else:
        raise ValueError(f"unknown editor test scenario: {scenario}")
    addon = destination / "addons" / plugin_name
    shutil.copytree(fixture, addon)
    project = destination / "project.godot"
    project.write_text(
        project.read_text(encoding="utf-8").replace(
            'enabled=PackedStringArray("godot-rust")',
            f'enabled=PackedStringArray("godot-rust", "{plugin_name}")',
        ),
        encoding="utf-8",
    )


def run_editor_test(godot: str, no_build: bool, scenario: str = "flow") -> str:
    component_output = ""
    if scenario in ("flow", "components"):
        component_output = run_component_test(godot)
    if scenario == "components":
        return component_output
    if not no_build:
        stage_example_plugin(release=False)
    runtime_returncode = 0
    runtime_output = ""
    with tempfile.TemporaryDirectory(
        prefix=f"godot-rust-editor-{scenario}-"
    ) as temporary:
        project = Path(temporary) / "project"
        prepare_test_project(project, scenario)
        environment = {
            **os.environ,
            "CARGO_TARGET_DIR": str(
                ROOT / "target" / f"editor-{scenario}"
            ),
            "GODOT_SILENCE_ROOT_WARNING": "1",
        }
        completed = subprocess.run(
            [
                godot,
                "--headless",
                "--editor",
                "--path",
                str(project),
            ],
            cwd=ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=EDITOR_FLOW_TIMEOUT_SECONDS,
        )
        if scenario == "flow" and completed.returncode == 0:
            runtime = subprocess.run(
                [
                    godot,
                    "--headless",
                    "--path",
                    str(project),
                    "--quit-after",
                    "10",
                ],
                cwd=ROOT,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
                timeout=120,
            )
            runtime_returncode = runtime.returncode
            runtime_output = runtime.stdout + runtime.stderr
    output = completed.stdout + completed.stderr + runtime_output
    diagnostics = scenario == "diagnostics"
    setup = scenario == "setup"
    initialize = scenario == "initialize"
    if diagnostics:
        markers = DIAGNOSTICS_MARKERS
    elif setup:
        markers = SETUP_MARKERS
    elif initialize:
        markers = INITIALIZE_MARKERS
    else:
        markers = REQUIRED_MARKERS
    errors = output_errors(output, markers, diagnostics)
    if completed.returncode != 0 or runtime_returncode != 0 or errors:
        raise RuntimeError(
            "Godot editor plugin load failed: "
            f"editor_status={completed.returncode}, "
            f"runtime_status={runtime_returncode}, evidence={errors}\n{output}"
        )
    return component_output + output


def run_component_test(godot: str) -> str:
    """Exercise the shipped GDScript controls without loading Rust or Cargo."""
    with tempfile.TemporaryDirectory(
        prefix="godot-rust-editor-components-"
    ) as temporary:
        project = Path(temporary)
        addon = project / "addons" / "godot-rust"
        addon.mkdir(parents=True)
        for source in (EXAMPLE / "addons" / "godot-rust").glob("*.gd"):
            shutil.copy2(source, addon / source.name)
        (project / "project.godot").write_text(
            '[application]\nconfig/name="Rust editor component tests"\n'
            '\n[editor_plugins]\nenabled=PackedStringArray("editor-components")\n',
            encoding="utf-8",
        )
        fixture = project / "addons" / "editor-components"
        fixture.mkdir()
        (fixture / "plugin.cfg").write_text(
            '[plugin]\nname="Editor Components"\n'
            'description="Rust editor behavior tests"\n'
            'author="godot-rust"\nversion="1.0"\nscript="plugin.gd"\n',
            encoding="utf-8",
        )
        (fixture / "plugin.gd").write_text(
            COMPONENT_TEST_SCRIPT, encoding="utf-8"
        )
        for directory, source in {
            "owned": "pub fn user_module() {}\n",
            "empty": "",
            "header_prefix": "// @generated by godot-rust; user-maintained module\n",
            "generated": (
                "// @generated by godot-rust; "
                "script files themselves are user-owned.\n"
            ),
            "generated_crlf": (
                "// @generated by godot-rust; "
                "script files themselves are user-owned.\r\n"
            ),
        }.items():
            module = project / "src" / directory / "mod.rs"
            module.parent.mkdir(parents=True)
            module.write_bytes(source.encode("utf-8"))
        completed = subprocess.run(
            [godot, "--headless", "--editor", "--path", str(project)],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )
    output = completed.stdout + completed.stderr
    errors = output_errors(output, (COMPONENT_MARKER,))
    if completed.returncode != 0 or errors:
        raise RuntimeError(
            "Godot editor component test failed: "
            f"status={completed.returncode}, evidence={errors}\n{output}"
        )
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument(
        "--scenario",
        choices=("flow", "diagnostics", "setup", "initialize", "components"),
        default="flow",
    )
    parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="report a skip instead of failing when Godot is unavailable",
    )
    args = parser.parse_args()
    godot = discover_godot(args.godot)
    if godot is None:
        message = "Godot executable not found; set GODOT_BIN or pass --godot"
        if args.allow_missing:
            print(f"SKIP: {message}")
            return 0
        print(message, file=sys.stderr)
        return 1
    try:
        output = run_editor_test(godot, args.no_build, args.scenario)
    except subprocess.TimeoutExpired as error:
        print(f"Godot editor plugin test timed out: {error}", file=sys.stderr)
        for captured in (error.stdout, error.stderr):
            if captured:
                text = captured.decode("utf-8", errors="replace") if isinstance(captured, bytes) else captured
                print(text, file=sys.stderr)
        return 1
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Godot editor plugin test failed: {error}", file=sys.stderr)
        return 1
    print(output, end="")
    print(f"Godot editor plugin {args.scenario} test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
