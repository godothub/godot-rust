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
)
EDITOR_FLOW_TIMEOUT_SECONDS = 300


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
    if diagnostics:
        markers = DIAGNOSTICS_MARKERS
    elif setup:
        markers = SETUP_MARKERS
    else:
        markers = REQUIRED_MARKERS
    errors = output_errors(output, markers, diagnostics)
    if completed.returncode != 0 or runtime_returncode != 0 or errors:
        raise RuntimeError(
            "Godot editor plugin load failed: "
            f"editor_status={completed.returncode}, "
            f"runtime_status={runtime_returncode}, evidence={errors}\n{output}"
        )
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument(
        "--scenario",
        choices=("flow", "diagnostics", "setup"),
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
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Godot editor plugin test failed: {error}", file=sys.stderr)
        return 1
    print(output, end="")
    print(f"Godot editor plugin {args.scenario} test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
