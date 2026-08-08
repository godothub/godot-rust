#!/usr/bin/env python3
"""Install an exported APK and prove its Rust scripts run on Android."""

from __future__ import annotations

import argparse
import subprocess
import time
from pathlib import Path

from test_godot_export import (
    INHERITANCE_MARKER,
    RESOURCE_MARKER,
    RUNTIME_MARKER,
)


REQUIRED_MARKERS = (
    RUNTIME_MARKER,
    RESOURCE_MARKER,
    INHERITANCE_MARKER,
)
FATAL_MARKERS = (
    "FATAL EXCEPTION",
    "Fatal signal",
    "SIGABRT",
    "SIGSEGV",
    "Rust script callback panicked",
    "Failed to load script",
)


def checked(arguments: list[str], timeout: int = 120) -> str:
    completed = subprocess.run(
        arguments,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    output = completed.stdout + completed.stderr
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed with status {completed.returncode}: "
            f"{' '.join(arguments)}\n{output}"
        )
    return output


def runtime_log_errors(
    output: str,
    required_markers: tuple[str, ...] = REQUIRED_MARKERS,
) -> list[str]:
    errors = [
        f"missing marker {marker}"
        for marker in required_markers
        if marker not in output
    ]
    errors.extend(
        f"fatal Android log marker {marker}"
        for marker in FATAL_MARKERS
        if marker in output
    )
    return errors


def wait_for_process(adb: str, package: str, timeout_seconds: int = 30) -> str:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        completed = subprocess.run(
            [adb, "shell", "pidof", package],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
        candidates = completed.stdout.strip().split()
        if completed.returncode == 0 and candidates:
            if not all(candidate.isascii() and candidate.isdigit() for candidate in candidates):
                raise RuntimeError(
                    f"adb returned invalid process IDs for {package}: {candidates}"
                )
            return candidates[0]
        time.sleep(1)
    raise RuntimeError(f"Android package did not start within {timeout_seconds}s: {package}")


def run_android_runtime(
    adb: str,
    apk: Path,
    package: str,
    timeout_seconds: int,
    required_markers: tuple[str, ...] = REQUIRED_MARKERS,
    allow_process_exit: bool = False,
) -> str:
    if not apk.is_file() or apk.is_symlink() or apk.stat().st_size == 0:
        raise ValueError(f"Android runtime test requires a regular APK: {apk}")
    if not package or any(character.isspace() for character in package):
        raise ValueError(f"invalid Android package name: {package!r}")

    checked([adb, "get-state"], timeout=30)
    checked([adb, "install", "-r", str(apk.resolve())], timeout=180)
    checked([adb, "logcat", "--clear"], timeout=30)
    checked(
        [
            adb,
            "shell",
            "monkey",
            "-p",
            package,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
        timeout=60,
    )
    process_id = None if allow_process_exit else wait_for_process(adb, package)

    deadline = time.monotonic() + timeout_seconds
    output = ""
    try:
        while time.monotonic() < deadline:
            logcat_arguments = [adb, "logcat", "-d"]
            if process_id is not None:
                logcat_arguments.extend(("--pid", process_id))
            logcat_arguments.extend(("-v", "brief"))
            output = checked(
                logcat_arguments,
                timeout=30,
            )
            if not runtime_log_errors(output, required_markers):
                return output
            if any(marker in output for marker in FATAL_MARKERS):
                break
            time.sleep(1)
    finally:
        checked([adb, "shell", "am", "force-stop", package], timeout=30)
    raise RuntimeError(
        "Android Rust runtime validation failed: "
        f"{runtime_log_errors(output, required_markers)}\n"
        f"{output}"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--log-output", type=Path)
    parser.add_argument(
        "--required-marker",
        action="append",
        default=[],
        help="runtime marker to require; repeat for multiple markers",
    )
    parser.add_argument(
        "--allow-process-exit",
        action="store_true",
        help="read the cleared device log when the test application exits quickly",
    )
    args = parser.parse_args()
    if not 1 <= args.timeout <= 600:
        parser.error("--timeout must be between 1 and 600 seconds")
    output = run_android_runtime(
        args.adb,
        args.apk,
        args.package,
        args.timeout,
        tuple(args.required_marker) or REQUIRED_MARKERS,
        args.allow_process_exit,
    )
    if args.log_output is not None:
        args.log_output.parent.mkdir(parents=True, exist_ok=True)
        args.log_output.write_text(output, encoding="utf-8")
    print("Godot Rust Android runtime integration passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
