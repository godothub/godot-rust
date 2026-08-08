#!/usr/bin/env python3
"""Load the staged Host in an official Godot editor and reject loader errors."""

from __future__ import annotations

import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from stage_host import CANONICAL_ADDON, build_host, library_name
from stage_macos_host import stage_macos_host
from stage_project_module import build_project_module, validate_project_module
from plugin_platform import platform_directory


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "host_load"
ERROR_PATTERNS = [
    re.compile(r"ERROR:", re.IGNORECASE),
    re.compile(r"can't open dynamic library", re.IGNORECASE),
    re.compile(r"failed loading", re.IGNORECASE),
    re.compile(r"entry point .* not found", re.IGNORECASE),
    re.compile(r"godot_rust_init.*not found", re.IGNORECASE),
]
TERMINAL_ESCAPE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
MODULE_READY_MARKER = "GODOT_RUST_MODULE_READY"
MODULE_PROCESS_MARKER = "GODOT_RUST_MODULE_PROCESS"
MODULE_METHOD_MARKER = "GODOT_RUST_MODULE_METHOD"
STRING_VALUES_MARKER = "GODOT_RUST_STRING_VALUES"
STRING_NAME_VALUES_MARKER = "GODOT_RUST_STRING_NAME_VALUES"
ENGINE_API_MARKER = "GODOT_RUST_ENGINE_API"
ENGINE_MATH_API_MARKER = "GODOT_RUST_ENGINE_MATH_API"
ENGINE_STRING_API_MARKER = "GODOT_RUST_ENGINE_STRING_API"
ENGINE_STRING_NAME_API_MARKER = "GODOT_RUST_ENGINE_STRING_NAME_API"
ENGINE_RID_API_MARKER = "GODOT_RUST_ENGINE_RID_API"
ENGINE_ENUM_API_MARKER = "GODOT_RUST_ENGINE_ENUM_API"
ENGINE_NODE_PATH_API_MARKER = "GODOT_RUST_ENGINE_NODE_PATH_API"
ENGINE_EXTENDED_MATH_API_MARKER = "GODOT_RUST_ENGINE_EXTENDED_MATH_API"
PACKED_ARRAYS_MARKER = "GODOT_RUST_PACKED_ARRAYS"
DYNAMIC_VALUES_MARKER = "GODOT_RUST_DYNAMIC_VALUES"
CALLABLES_MARKER = "GODOT_RUST_CALLABLES"
SIGNALS_MARKER = "GODOT_RUST_SIGNALS"
SIGNAL_PTRCALL_MARKER = "GODOT_RUST_SIGNAL_PTRCALL"
VIRTUAL_OVERRIDES_MARKER = "GODOT_RUST_VIRTUAL_OVERRIDES"
STATIC_ENGINE_API_MARKER = "GODOT_RUST_STATIC_ENGINE_API"
COMPLETE_GENERATED_API_MARKER = "GODOT_RUST_COMPLETE_GENERATED_API"
VARARG_ENGINE_API_MARKER = "GODOT_RUST_VARARG_ENGINE_API"
NODE_FIELDS_MARKER = "GODOT_RUST_NODE_FIELDS"
GODOT_METADATA_MARKER = "GODOT_RUST_GODOT_METADATA"
MATH_VALUES_MARKER = "GODOT_RUST_MATH_VALUES"
INTEGER_VALUES_MARKER = "GODOT_RUST_INTEGER_VALUES"
OBJECT_VALUES_MARKER = "GODOT_RUST_OBJECT_VALUES"
RID_VALUES_MARKER = "GODOT_RUST_RID_VALUES"
TEMPLATE_MARKER = "GODOT_RUST_TEMPLATE_OK"
FAULT_ISOLATION_MARKER = "GODOT_RUST_FAULT_ISOLATION_OK"
EXPECTED_FAILURE_CALLBACK_MARKER = "GODOT_RUST_EXPECTED_FAILURE_CALLBACK"
EXPECTED_PANIC_CALLBACK_MARKER = "GODOT_RUST_EXPECTED_PANIC_CALLBACK"
REQUIRED_MODULE_MARKERS = (
    MODULE_READY_MARKER,
    MODULE_PROCESS_MARKER,
    MODULE_METHOD_MARKER,
    STRING_VALUES_MARKER,
    STRING_NAME_VALUES_MARKER,
    ENGINE_API_MARKER,
    ENGINE_MATH_API_MARKER,
    ENGINE_STRING_API_MARKER,
    ENGINE_STRING_NAME_API_MARKER,
    ENGINE_RID_API_MARKER,
    ENGINE_ENUM_API_MARKER,
    ENGINE_NODE_PATH_API_MARKER,
    ENGINE_EXTENDED_MATH_API_MARKER,
    PACKED_ARRAYS_MARKER,
    DYNAMIC_VALUES_MARKER,
    CALLABLES_MARKER,
    SIGNALS_MARKER,
    VIRTUAL_OVERRIDES_MARKER,
    STATIC_ENGINE_API_MARKER,
    COMPLETE_GENERATED_API_MARKER,
    VARARG_ENGINE_API_MARKER,
    NODE_FIELDS_MARKER,
    GODOT_METADATA_MARKER,
    MATH_VALUES_MARKER,
    INTEGER_VALUES_MARKER,
    OBJECT_VALUES_MARKER,
    RID_VALUES_MARKER,
    TEMPLATE_MARKER,
)


def output_errors(output: str) -> list[str]:
    """Return engine errors and missing required execution evidence."""
    errors = [
        pattern.pattern for pattern in ERROR_PATTERNS if pattern.search(output)
    ]
    for marker in REQUIRED_MODULE_MARKERS:
        if marker not in output:
            errors.append(f"missing marker {marker}")
    if os.environ.get("GODOT_RS_GODOT") == "4.7" and SIGNAL_PTRCALL_MARKER not in output:
        errors.append(f"missing marker {SIGNAL_PTRCALL_MARKER}")
    return errors


def fault_isolation_errors(status: int, output: str) -> list[str]:
    """Validate expected callback failures without hiding crashes."""
    output = TERMINAL_ESCAPE.sub("", output)
    errors = []
    if status != 0:
        errors.append(f"fault isolation status {status}")
    expected_counts = {
        FAULT_ISOLATION_MARKER: 1,
        EXPECTED_FAILURE_CALLBACK_MARKER: 3,
        EXPECTED_PANIC_CALLBACK_MARKER: 1,
        "SCRIPT ERROR: CallbackFailed: intentional callback failure": 2,
        "SCRIPT ERROR: Panic: Rust script callback panicked": 1,
        "at: failure_for_test (res://sample.rs:0)": 2,
        "at: panic_for_test (res://sample.rs:0)": 1,
        "This script instance was disabled": 2,
    }
    for marker, expected in expected_counts.items():
        actual = output.count(marker)
        if actual != expected:
            errors.append(
                f"expected {expected} occurrence(s) of {marker!r}, got {actual}"
            )
    if "res://sample.rs" not in output:
        errors.append("debugger error did not include res://sample.rs")
    for pattern in (
        re.compile(r"segmentation fault", re.IGNORECASE),
        re.compile(r"fatal error", re.IGNORECASE),
        re.compile(r"memory errors detected", re.IGNORECASE),
        re.compile(r"invalid read|invalid write", re.IGNORECASE),
    ):
        if pattern.search(output):
            errors.append(pattern.pattern)
    return errors


def discover_godot(explicit: str | None) -> str | None:
    """Resolve an explicit path, environment setting, or common executable."""
    candidates = [
        explicit,
        os.environ.get("GODOT_BIN"),
        shutil.which("godot4"),
        shutil.which("godot"),
    ]
    return next((candidate for candidate in candidates if candidate), None)


def valgrind_command(command: list[str]) -> list[str]:
    """Wrap a Godot invocation with ABI-relevant memory checks."""
    return [
        "valgrind",
        "--error-exitcode=97",
        "--leak-check=no",
        "--num-callers=40",
        "--undef-value-errors=no",
        *command,
    ]


def stage_clean_project(destination: Path, host_library: Path) -> Path:
    """Assemble an isolated project without persistent Godot editor state."""
    shutil.copytree(
        FIXTURE,
        destination,
        ignore=shutil.ignore_patterns(".godot", "addons"),
    )
    addon = destination / "addons" / "godot-rust"
    addon.mkdir(parents=True)
    for name in ("godot-rust.gdextension", "godot-rust.gdextension.uid"):
        shutil.copy2(CANONICAL_ADDON / name, addon / name)
    addon_bin = addon / "bin"
    if sys.platform == "darwin":
        stage_macos_host(host_library, addon_bin)
    else:
        binary = (
            addon_bin
            / platform_directory(sys.platform, platform.machine())
            / host_library.name
        )
        binary.parent.mkdir(parents=True)
        shutil.copy2(host_library, binary)
    return destination


def run_load_test(
    godot: str, no_build: bool, valgrind: bool
) -> tuple[int, str]:
    """Run the fixture and return process status plus combined output."""
    if not no_build:
        host_library = build_host(release=False)
        project_module = build_project_module(release=False)
    else:
        from stage_project_module import library_name as project_library_name

        host_library = (
            ROOT / "target" / "debug" / library_name(sys.platform)
        )
        project_module = (
            ROOT / "target" / "debug" / project_library_name(sys.platform)
        )
        if not host_library.is_file():
            raise FileNotFoundError(
                f"built Host library not found: {host_library}"
            )
        if not project_module.is_file():
            raise FileNotFoundError(
                f"built project module not found: {project_module}"
            )
    validate_project_module(project_module)

    with tempfile.TemporaryDirectory(prefix="godot-rust-host-") as temporary:
        project = stage_clean_project(
            Path(temporary) / "project",
            host_library,
        )
        command = [
            godot,
            "--headless",
            "--editor",
            "--path",
            str(project),
            "--frame-delay",
            "16",
            "--quit-after",
            "60",
        ]
        if valgrind:
            command = valgrind_command(command)
        environment = os.environ.copy()
        environment["GODOT_SILENCE_ROOT_WARNING"] = "1"
        environment["GODOT_RS_PROJECT_MODULE"] = str(project_module)
        completed = subprocess.run(
            command,
            cwd=ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=180 if valgrind else 60,
        )
        output = completed.stdout + completed.stderr
        if completed.returncode != 0:
            return completed.returncode, output

        smoke_command = [
            godot,
            "--headless",
            "--path",
            str(project),
            "--script",
            "res://host_smoke.gd",
        ]
        if valgrind:
            smoke_command = valgrind_command(smoke_command)
        smoke = subprocess.run(
            smoke_command,
            cwd=ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=180 if valgrind else 60,
        )
        smoke_output = smoke.stdout + smoke.stderr
        if smoke.returncode != 0:
            return smoke.returncode, output + smoke_output

        fault_command = [
            godot,
            "--headless",
            "--path",
            str(project),
            "--script",
            "res://fault_isolation.gd",
        ]
        if valgrind:
            fault_command = valgrind_command(fault_command)
        fault = subprocess.run(
            fault_command,
            cwd=ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=180 if valgrind else 60,
        )
        fault_output = fault.stdout + fault.stderr
        fault_errors = fault_isolation_errors(fault.returncode, fault_output)
        if fault_errors:
            diagnostic = (
                "\nGODOT_RUST_FAULT_ISOLATION_VALIDATION_FAILURE: "
                + ", ".join(fault_errors)
                + "\n"
            )
            return 1, output + smoke_output + fault_output + diagnostic
        return 0, output + smoke_output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument(
        "--valgrind",
        action="store_true",
        help="run Godot under Valgrind and fail on invalid memory access",
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
        status, output = run_load_test(godot, args.no_build, args.valgrind)
    except subprocess.TimeoutExpired as error:
        if error.stdout:
            sys.stderr.write(
                error.stdout.decode() if isinstance(error.stdout, bytes) else error.stdout
            )
        if error.stderr:
            sys.stderr.write(
                error.stderr.decode() if isinstance(error.stderr, bytes) else error.stderr
            )
        print(
            f"Godot Host load test timed out after {error.timeout} seconds",
            file=sys.stderr,
        )
        return 1
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Godot Host load test failed: {error}", file=sys.stderr)
        return 1

    errors = output_errors(output)
    if status != 0 or errors:
        sys.stderr.write(output)
        print(
            f"Godot Host load test failed: status={status}, patterns={errors}",
            file=sys.stderr,
        )
        return 1

    print("Godot Host load test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
