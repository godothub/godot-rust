#!/usr/bin/env python3
"""Verify an assembled godot-rust release ZIP without extracting it."""

from __future__ import annotations

import argparse
import sys
import zipfile
from pathlib import Path

from assemble_release import verify_release_zip


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("package", type=Path)
    args = parser.parse_args()
    try:
        verify_release_zip(args.package, args.version)
    except (OSError, RuntimeError, ValueError, zipfile.BadZipFile) as error:
        print(f"release package verification failed: {error}", file=sys.stderr)
        return 1
    print(args.package)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
