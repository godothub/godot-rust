#!/usr/bin/env python3
"""Build and stage the current platform into the canonical example addon."""

from __future__ import annotations

import argparse
import platform
import shutil
import subprocess
import sys
from pathlib import Path

from plugin_platform import platform_directory
from stage_macos_host import stage_macos_host


ROOT = Path(__file__).resolve().parents[1]
PLUGIN_ROOT = ROOT / "example" / "addons" / "godot-rust"
PLUGIN_BIN = PLUGIN_ROOT / "bin"


def host_library_name(platform: str) -> str:
    """Return the Host cdylib filename for a Python platform."""
    if platform == "darwin":
        return "libgodot_rs_host.dylib"
    if platform.startswith("win"):
        return "godot_rs_host.dll"
    return "libgodot_rs_host.so"


def executable_name(name: str, platform: str) -> str:
    """Return a Rust binary filename for a Python platform."""
    return f"{name}.exe" if platform.startswith("win") else name


def stage_example_plugin(release: bool) -> list[Path]:
    """Build and copy Host, build daemon, and validator into the addon."""
    profile = "release" if release else "debug"
    profile_argument = ["--release"] if release else []
    subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "godot_rs_host",
            "--lib",
            "--bin",
            "godot_rs_module_check",
            *profile_argument,
        ],
        cwd=ROOT,
        check=True,
    )
    subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "godot_rs_buildd",
            "--bin",
            "godot_rs_buildd",
            *profile_argument,
        ],
        cwd=ROOT,
        check=True,
    )

    host_filename = host_library_name(sys.platform)
    filenames = [
        executable_name("godot_rs_buildd", sys.platform),
        executable_name("godot_rs_module_check", sys.platform),
    ]
    plugin_bin = PLUGIN_BIN / platform_directory(sys.platform, platform.machine())
    plugin_bin.mkdir(parents=True, exist_ok=True)
    host_source = ROOT / "target" / profile / host_filename
    if not host_source.is_file():
        raise FileNotFoundError(
            f"Cargo did not produce expected plugin file: {host_source}"
    )
    if sys.platform == "darwin":
        staged = [stage_macos_host(host_source, PLUGIN_BIN)]
    else:
        host_destination = plugin_bin / host_filename
        shutil.copy2(host_source, host_destination)
        staged = [host_destination]
    for filename in filenames:
        source = ROOT / "target" / profile / filename
        if not source.is_file():
            raise FileNotFoundError(f"Cargo did not produce expected plugin file: {source}")
        destination = plugin_bin / filename
        shutil.copy2(source, destination)
        staged.append(destination)
    return staged


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()
    try:
        staged = stage_example_plugin(args.release)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Example plugin staging failed: {error}", file=sys.stderr)
        return 1
    for path in staged:
        print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
