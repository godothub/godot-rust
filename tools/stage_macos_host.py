#!/usr/bin/env python3
"""Stage a local macOS Host dylib at the packaged addon path."""

from __future__ import annotations

import shutil
from pathlib import Path

from plugin_platform import platform_directory


LOCAL_HOST = Path(
    platform_directory("darwin", "universal")
) / "libgodot_host.dylib"


def stage_macos_host(binary: Path, addon_bin: Path) -> Path:
    """Atomically replace only the ignored local macOS Host dylib."""
    if not binary.is_file() or binary.is_symlink():
        raise ValueError(f"macOS Host input is not a regular file: {binary}")
    destination = addon_bin / LOCAL_HOST
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_symlink() or (
        destination.exists() and not destination.is_file()
    ):
        raise ValueError(f"local macOS Host path is not a file: {destination}")

    temporary = destination.with_name(f".{destination.name}.new")
    if temporary.exists() or temporary.is_symlink():
        raise ValueError(f"stale local macOS Host staging file exists: {temporary}")
    shutil.copy2(binary, temporary)
    temporary.chmod(0o755)
    temporary.replace(destination)
    return destination
