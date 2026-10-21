#!/usr/bin/env python3
"""Load the canonical editor plugin in official Godot and reject script errors."""

from __future__ import annotations

import argparse
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
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
    "GODOT_RUST_HOT_RELOAD_CONTAINER_IDENTITY",
    "GODOT_RUST_BUILD_RECEIPT_REUSED",
    "GODOT_RUST_RECEIPT_REUSE_MSEC",
    "GODOT_RUST_UNCHANGED_BUILD_REUSED",
    "godot-rust: Automatic Rust check succeeded.",
)
FLOW_PLUGIN = ROOT / "tests" / "fixtures" / "editor_flow"
DIAGNOSTICS_PLUGIN = ROOT / "tests" / "fixtures" / "editor_diagnostics"
SETUP_PLUGIN = ROOT / "tests" / "fixtures" / "editor_setup"
INITIALIZE_PLUGIN = ROOT / "tests" / "fixtures" / "editor_initialize"
SCRIPTS_PLUGIN = ROOT / "tests" / "fixtures" / "editor_scripts"
SCRIPTS_MARKERS = (
    "GODOT_RUST_EDITOR_PLACEHOLDER_READY",
    "GODOT_RUST_UNICODE_EDIT_SURVIVED",
    "GODOT_RUST_EXPORT_ADDITION_PRESERVED",
    "GODOT_RUST_TOOL_NODE_ROLLED_BACK",
    "GODOT_RUST_TOOL_NODE_RECOVERED",
    "GODOT_RUST_TOOL_STRUCTURE_RELOADED",
    "GODOT_RUST_TOOL_RELOAD_ROLLED_BACK",
    "GODOT_RUST_TOOL_REMOVAL_RELOADED",
    "GODOT_RUST_TOOL_RESET_RELOADED",
    "GODOT_RUST_RUNTIME_READY",
    "GODOT_RUST_RUNTIME_PROCESS",
)
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
# The flow performs a cold check followed by builds of two module generations.
EDITOR_FLOW_TIMEOUT_SECONDS = 600
COMPONENT_MARKER = "GODOT_RUST_EDITOR_COMPONENTS_PASSED"
COMPONENT_TEST_SCRIPT = '''\
@tool
extends EditorPlugin

const SourceMonitor = preload("res://addons/godot-rust/source_monitor.gd")
const DiagnosticsPanel = preload("res://addons/godot-rust/diagnostics_panel.gd")
const RustPlugin = preload("res://addons/godot-rust/plugin.gd")
const RustSyntaxHighlighter = preload("res://addons/godot-rust/syntax_highlighter.gd")

class CountingMonitor:
    extends SourceMonitor
    var scans := 0
    func _refresh_signature(_schedule_check: bool) -> void:
        scans += 1

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
    var counting_monitor = CountingMonitor.new()
    add_child(counting_monitor)
    for _event in 100:
        counting_monitor._on_filesystem_changed()
    if counting_monitor.scans != 0:
        _fail("Source monitor fingerprints inputs before debounce")
        return
    await get_tree().create_timer(0.6).timeout
    if counting_monitor.scans != 1:
        _fail("Source monitor did not coalesce filesystem events")
        return
    counting_monitor.queue_free()
    print("GODOT_RUST_MONITOR_DEBOUNCE_OK")
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
    if not _verify_root_setup_paths(plugin):
        plugin.free()
        return
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


func _verify_root_setup_paths(plugin) -> bool:
    var cases := [
        ["/canonical/game", "/canonical/game/Cargo.toml"],
        ["//?/c:/game", "C:/game/Cargo.toml"],
        ["C:/game", "//?/c:/game/Cargo.toml"],
        ["//?/UNC/server/share/game", "//server/share/game/Cargo.toml"],
    ]
    for paths in cases:
        for backslashes in [false, true]:
            var root: String = paths[0]
            var manifest: String = paths[1]
            if backslashes:
                root = root.replace("/", String.chr(92))
                manifest = manifest.replace("/", String.chr(92))
            var package := {
                "name": "existing_game",
                "manifest_path": manifest,
                "godot_rust_mode": "script",
                "configuration_issues": [],
            }
            var result := {"project_root": root, "packages": [package]}
            if plugin._root_script_setup_candidate(result) != package:
                _fail("Cargo setup did not match its canonical root: %s" % root)
                return false
    for rejected in [
        {"manifest_path": "/canonical/other/Cargo.toml"},
        {"manifest_path": "/canonical/game/cargo.toml"},
        {"godot_rust_mode": "extension"},
        {"configuration_issues": ["conflicting configuration"]},
    ]:
        var package := {
            "manifest_path": "/canonical/game/Cargo.toml",
            "godot_rust_mode": "script",
            "configuration_issues": [],
        }
        package.merge(rejected, true)
        var result := {"project_root": "/canonical/game", "packages": [package]}
        if not plugin._root_script_setup_candidate(result).is_empty():
            _fail("Cargo setup accepted an unrelated or incompatible package")
            return false
    return true


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
    expect_error: bool = False,
) -> list[str]:
    patterns = ERROR_PATTERNS[1:] if expect_error else ERROR_PATTERNS
    plain_output = ANSI_ESCAPE.sub("", output)
    errors = [
        pattern.pattern for pattern in patterns if pattern.search(plain_output)
    ]
    if expect_error and not ERROR_PATTERNS[0].search(plain_output):
        errors.append("the invalid operation did not report an error")
    for marker in required_markers:
        if marker not in plain_output:
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
        base = destination / "src" / "scripts" / "base_greeting.rs"
        source = base.read_text(encoding="utf-8").replace(
            "#[script(base = Node,", "#[script(base = Node, tool,"
        ).replace("prefix: String,", """prefix: String,
    #[export(get = get_shared_values, set = set_shared_values)]
    shared_values: Array<i64>,
    #[export]
    shared_metadata: Dictionary,
    #[reload(persist)]
    shared_setter_calls: i64,
    reloaded_calls: i64,""")
        source += """
#[script]
impl BaseGreeting {
    fn get_shared_values(&self) -> Array<i64> { self.shared_values.clone() }
    fn set_shared_values(&mut self, value: Array<i64>) {
        value.push(77).expect("shared setter");
        self.shared_values = value;
        self.shared_setter_calls += 1;
    }
    #[func]
    fn shared_setter_calls(&self) -> i64 { self.shared_setter_calls }
    #[func]
    fn reloaded_calls(&self) -> i64 { self.reloaded_calls }
    fn _reloaded(&mut self) { self.reloaded_calls += 1; }
}
"""
        base.write_text(
            source,
            encoding="utf-8",
        )
    elif scenario == "scripts":
        fixture = SCRIPTS_PLUGIN
        plugin_name = "godot-rust-editor-scripts"
        scripts = destination / "src" / "scripts"
        shutil.rmtree(scripts)
        scripts.mkdir()
        for index, source in enumerate(sorted(fixture.glob("*.rs")), start=1):
            shutil.copy2(source, scripts / source.name)
            (scripts / (source.name + ".uid")).write_text(
                f"uid://b{index}\n", encoding="utf-8"
            )
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                "auto-check = true", "auto-check = false"
            ),
            encoding="utf-8",
        )
        (destination / "main.tscn").write_text(
            '[gd_scene load_steps=2 format=3]\n'
            '[ext_resource type="Script" path="res://src/scripts/editor_probe.rs" id="1"]\n'
            '[node name="Probe" type="Node"]\nscript=ExtResource("1")\n',
            encoding="utf-8",
        )
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
    shutil.copytree(fixture, addon, ignore=shutil.ignore_patterns("*.rs"))
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
    verbose_output = ""
    verbose_returncode = 0
    profiling_output = ""
    profiling_status = 0
    profiling_packets = b""
    with tempfile.TemporaryDirectory(
        prefix=f"godot-rust-editor-{scenario}-"
    ) as temporary:
        # Exercise Cargo dep-info escaping and Windows path separators during
        # real builds, publication, hot reload, and Build Receipt reuse.
        project = Path(temporary) / "project with spaces" / "#项目"
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
            encoding="utf-8",
            timeout=EDITOR_FLOW_TIMEOUT_SECONDS,
        )
        if scenario in ("flow", "scripts") and completed.returncode == 0:
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
                encoding="utf-8",
                timeout=120,
            )
            runtime_returncode = runtime.returncode
            runtime_output = runtime.stdout + runtime.stderr
            if scenario == "scripts" and runtime.returncode == 0:
                verbose = subprocess.run(
                    [godot, "--headless", "--verbose", "--path", str(project), "--quit-after", "10"],
                    cwd=ROOT,
                    env=environment,
                    check=False,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    timeout=120,
                )
                verbose_returncode = verbose.returncode
                verbose_output = verbose.stdout + verbose.stderr
                profiling_status, profiling_output, profiling_packets = run_profiled_game(
                    godot, project, environment
                )
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
    elif scenario == "scripts":
        markers = SCRIPTS_MARKERS
    else:
        markers = REQUIRED_MARKERS
    checked_output = output
    if scenario == "scripts":
        checked_output = re.sub(
            r"(?m)^ERROR: GODOT_RUST_LOG_ERROR\r?$", "", ANSI_ESCAPE.sub("", output)
        )
    errors = output_errors(checked_output, markers, diagnostics or scenario == "scripts")
    if scenario == "scripts":
        task_marker = "GODOT_RUST_TOOL_TASK_CANCELLED"
        cancellations = [match.start() for match in re.finditer(task_marker, completed.stdout)]
        if len(cancellations) != 2 or cancellations[-1] < completed.stdout.find("GODOT_RUST_TOOL_RELOAD_ROLLED_BACK"):
            errors.append("old tasks were not cancelled at successful generation replacement")
        for marker in (
            "GODOT_RUST_LOG_INFO",
            "GODOT_RUST_LOG_WORKER",
            "GODOT_RUST_LONG_BEGIN" + "中" * 700 + "END",
        ):
            if marker not in completed.stdout or marker in completed.stderr:
                errors.append(f"log missing from Godot stdout: {marker[:50]}")
        for marker in ("WARNING: GODOT_RUST_LOG_WARNING", "ERROR: GODOT_RUST_LOG_ERROR"):
            if marker not in ANSI_ESCAPE.sub("", completed.stderr):
                errors.append(f"missing Godot severity: {marker}")
        if "GODOT_RUST_LOG_DEBUG" in output:
            errors.append("debug logging ignored Godot's verbosity setting")
        if "GODOT_RUST_RUNTIME_" in completed.stdout + completed.stderr:
            errors.append("a regular Rust script executed in the editor")
        errors.extend(output_errors(verbose_output, ("GODOT_RUST_LOG_DEBUG",)))
        errors.extend(output_errors(profiling_output, ("GODOT_RUST_PROFILE_COMPLETED",)))
        if b"res://src/scripts/editor_probe.rs::_process" not in profiling_packets:
            errors.append("the Godot profiler received no Rust frame data")
    if any((completed.returncode, runtime_returncode, verbose_returncode, profiling_status)) or errors:
        raise RuntimeError(
            "Godot editor plugin load failed: "
            f"editor_status={completed.returncode}, "
            f"runtime_status={runtime_returncode}, profiler_status={profiling_status}, "
            f"evidence={errors}\n{output}\n{verbose_output}\n{profiling_output}"
        )
    return component_output + output + profiling_output


def run_profiled_game(
    godot: str, project: Path, environment: dict[str, str]
) -> tuple[int, str, bytes]:
    received = bytearray()

    def collect(connection: socket.socket) -> None:
        with connection:
            try:
                while packet := connection.recv(65536):
                    received.extend(packet)
            except ConnectionResetError:
                pass

    with socket.create_server(("127.0.0.1", 0)) as listener:
        listener.settimeout(10)
        command = [
            godot, "--headless", "--path", str(project),
            "--remote-debug", f"tcp://127.0.0.1:{listener.getsockname()[1]}",
            "--script", "res://addons/godot-rust-editor-scripts/profile.gd",
        ]
        with subprocess.Popen(
            command, cwd=ROOT, env=environment, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8",
        ) as child:
            reader = None
            try:
                connection, _ = listener.accept()
                reader = threading.Thread(target=collect, args=(connection,), daemon=True)
                reader.start()
                stdout, stderr = child.communicate(timeout=120)
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait()
                if reader is not None:
                    reader.join(timeout=10)
            return child.returncode, stdout + stderr, bytes(received)


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
            encoding="utf-8",
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
        choices=("flow", "diagnostics", "setup", "initialize", "components", "scripts"),
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
    # Windows redirects test logs using the system code page by default.
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    raise SystemExit(main())
