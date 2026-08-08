#!/usr/bin/env python3
"""Build and stage the Host library into the Godot load-test project."""

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
CANONICAL_ADDON = ROOT / "example" / "addons" / "godot-rust"
FIXTURE_ADDON = ROOT / "tests" / "fixtures" / "host_load" / "addons" / "godot-rust"
FIXTURE_BIN = (
    FIXTURE_ADDON / "bin"
)


def library_name(platform: str) -> str:
    """Return Cargo's native cdylib filename for the current platform."""
    if platform == "darwin":
        return "libgodot_rs_host.dylib"
    if platform.startswith("win"):
        return "godot_rs_host.dll"
    return "libgodot_rs_host.so"


def build_host(release: bool) -> Path:
    """Build and return the exact Host library produced by Cargo."""
    command = ["cargo", "build", "--locked", "-p", "godot_rs_host"]
    profile = "debug"
    if release:
        command.append("--release")
        profile = "release"

    subprocess.run(command, cwd=ROOT, check=True)
    filename = library_name(sys.platform)
    source = ROOT / "target" / profile / filename
    if not source.is_file():
        raise FileNotFoundError(f"Cargo did not produce expected Host: {source}")
    return source


def stage_host(release: bool) -> Path:
    """Build the Host and copy its exact output into the fixture."""
    source = build_host(release)
    filename = source.name

    fixture_bin = FIXTURE_BIN / platform_directory(sys.platform, platform.machine())
    fixture_bin.mkdir(parents=True, exist_ok=True)
    shutil.copy2(
        CANONICAL_ADDON / "godot-rust.gdextension",
        FIXTURE_ADDON / "godot-rust.gdextension",
    )
    if sys.platform == "darwin":
        return stage_macos_host(source, FIXTURE_BIN)
    destination = fixture_bin / filename
    shutil.copy2(source, destination)
    return destination


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()

    try:
        destination = stage_host(args.release)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Host staging failed: {error}", file=sys.stderr)
        return 1

    print(destination)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
