#!/usr/bin/env python3
"""Build the Script Mode project module used by the Godot Host smoke test."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = "godot_host_smoke_module"
READY_MARKER = "GODOT_RUST_MODULE_READY"
PROCESS_MARKER = "GODOT_RUST_MODULE_PROCESS"
METHOD_MARKER = "GODOT_RUST_MODULE_METHOD"


def library_name(platform: str) -> str:
    """Return Cargo's native project-module cdylib filename."""
    if platform == "darwin":
        return f"lib{PACKAGE}.dylib"
    if platform.startswith("win"):
        return f"{PACKAGE}.dll"
    return f"lib{PACKAGE}.so"


def build_project_module(release: bool) -> Path:
    """Build and return the exact immutable test module path."""
    command = ["cargo", "build", "--locked", "-p", PACKAGE]
    profile = "debug"
    if release:
        command.append("--release")
        profile = "release"
    subprocess.run(command, cwd=ROOT, check=True)

    module = ROOT / "target" / profile / library_name(sys.platform)
    if not module.is_file():
        raise FileNotFoundError(
            f"Cargo did not produce expected project module: {module}"
        )
    return module


def validate_project_module(module: Path) -> None:
    """Run the Host's ABI loader against a freshly built module."""
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "-p",
            "godot_host",
            "--bin",
            "godot_module_check",
            "--",
            str(module),
            "--exercise",
            "res://sample.rs",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    output = completed.stdout + completed.stderr
    missing = [
        marker
        for marker in (READY_MARKER, PROCESS_MARKER, METHOD_MARKER)
        if marker not in output
    ]
    if missing:
        raise RuntimeError(
            f"project module did not execute expected markers: {', '.join(missing)}"
        )
    print(output, end="")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()

    try:
        module = build_project_module(args.release)
        validate_project_module(module)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Project module build failed: {error}", file=sys.stderr)
        return 1

    print(module)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
