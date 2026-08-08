#!/usr/bin/env python3
"""Validate Cargo dependency provenance for the Godot interface boundary."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

# Cargo package identifiers that would introduce another Godot interface layer.
DENIED_PACKAGE_NAMES = {
    "godot",
    "godot-cell",
    "godot-core",
    "godot-ffi",
    "godot-macros",
}


def find_violations(metadata: dict[str, object]) -> list[str]:
    """Return deterministic dependency-policy violations."""
    violations: list[str] = []

    for package in metadata["packages"]:
        name = package["name"]
        if name in DENIED_PACKAGE_NAMES:
            violations.append(f"denied Cargo package: {name}")

    return sorted(set(violations))


def load_cargo_metadata() -> tuple[int, dict[str, object] | None]:
    """Load locked Cargo metadata without mutating the dependency graph."""
    completed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        return completed.returncode, None

    return 0, json.loads(completed.stdout)


def main() -> int:
    status, metadata = load_cargo_metadata()
    if status != 0 or metadata is None:
        return status

    violations = find_violations(metadata)
    if violations:
        print("godot-rust dependency provenance check failed:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation}", file=sys.stderr)
        return 1

    print("godot-rust dependency provenance check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
