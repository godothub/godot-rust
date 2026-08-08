#!/usr/bin/env python3
"""Canonical desktop platform directories used by the packaged Godot addon."""

from __future__ import annotations


def platform_directory(system: str, machine: str) -> str:
    """Map Python platform values to one packaged desktop binary directory."""
    normalized_system = system.lower()
    normalized_machine = machine.lower()
    if normalized_system == "darwin":
        return "macos-universal"
    if normalized_system.startswith("win"):
        if normalized_machine not in ("amd64", "x86_64"):
            raise ValueError(f"unsupported Windows architecture: {machine}")
        return "windows-x86_64"
    if normalized_system.startswith("linux"):
        if normalized_machine in ("aarch64", "arm64"):
            return "linux-arm64"
        if normalized_machine in ("amd64", "x86_64"):
            return "linux-x86_64"
        raise ValueError(f"unsupported Linux architecture: {machine}")
    raise ValueError(f"unsupported plugin platform: {system}")
