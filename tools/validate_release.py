#!/usr/bin/env python3
"""Validate godot-rust's decimal version and release changelog."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
NUMERIC_IDENTIFIER = r"(?:0|[1-9]\d*)"
VERSION_PATTERN = re.compile(
    rf"^(?P<major>{NUMERIC_IDENTIFIER})"
    rf"\.(?P<minor>{NUMERIC_IDENTIFIER})"
    rf"\.(?P<patch>{NUMERIC_IDENTIFIER})$"
)


def parse_version(version: str) -> tuple[int, int, int]:
    """Parse one canonical three-part SemVer release version."""
    match = VERSION_PATTERN.fullmatch(version)
    if match is None:
        raise ValueError(
            "version must use canonical numeric MAJOR.MINOR.PATCH identifiers"
        )
    return tuple(int(match.group(name)) for name in ("major", "minor", "patch"))


def release_entries(changelog: str, version: str) -> list[str]:
    """Extract bullet entries from a version section."""
    section = release_section(changelog, version)
    return [
        line[2:].strip()
        for line in section.splitlines()
        if line.startswith("- ") and line[2:].strip()
    ]


def release_section(changelog: str, version: str) -> str:
    """Extract the exact Markdown body under one version heading."""
    heading = re.compile(
        rf"^## \[?{re.escape(version)}\]?(?: - \d{{4}}-\d{{2}}-\d{{2}})?\s*$",
        re.MULTILINE,
    )
    match = heading.search(changelog)
    if match is None:
        return ""

    next_heading = re.search(r"^## ", changelog[match.end() :], re.MULTILINE)
    end = (
        match.end() + next_heading.start()
        if next_heading is not None
        else len(changelog)
    )
    return changelog[match.end() : end].strip()


def release_body(changelog: str, version: str) -> str:
    """Return the exact release body written from one changelog section."""
    section = release_section(changelog, version)
    return f"{section}\n" if section else ""


def workspace_version(cargo_toml: Path) -> str:
    """Read the single Workspace version."""
    with cargo_toml.open("rb") as manifest:
        data = tomllib.load(manifest)
    return data["workspace"]["package"]["version"]


def changelog_errors(
    changelog: str,
    version: str,
    filename: str,
) -> tuple[list[str], list[str]]:
    """Validate one release section and return its entries plus violations."""
    section = release_section(changelog, version)
    entries = release_entries(changelog, version)
    errors: list[str] = []
    invalid_lines = [
        line
        for line in section.splitlines()
        if line.strip() and not line.startswith("- ")
    ]
    if invalid_lines:
        errors.append(
            f"{filename} release {version} must contain only unordered-list entries"
        )
    if not 6 <= len(entries) <= 15:
        errors.append(
            f"{filename} release {version} must contain 6-15 user-facing entries; "
            f"found {len(entries)}"
        )
    return entries, errors


def validate(
    version: str,
    cargo_toml: Path,
    changelog: Path,
    translated_changelog: Path,
) -> list[str]:
    """Return all release-policy violations."""
    errors: list[str] = []
    try:
        parse_version(version)
    except ValueError as error:
        errors.append(str(error))
        return errors

    actual = workspace_version(cargo_toml)
    if actual != version:
        errors.append(
            f"workspace version is {actual}, requested release is {version}"
        )

    entries, changelog_violations = changelog_errors(
        changelog.read_text(encoding="utf-8"),
        version,
        changelog.name,
    )
    translated_entries, translated_violations = changelog_errors(
        translated_changelog.read_text(encoding="utf-8"),
        version,
        translated_changelog.name,
    )
    errors.extend(changelog_violations)
    errors.extend(translated_violations)
    if len(entries) != len(translated_entries):
        errors.append(
            f"release {version} must contain matching English and Chinese "
            f"entry counts; found {len(entries)} and {len(translated_entries)}"
        )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--notes-output",
        type=Path,
        help="write the matching CHANGELOG.md section as the release body",
    )
    parser.add_argument(
        "--published-release-json",
        type=Path,
        help="verify the published GitHub Release body against CHANGELOG.md",
    )
    args = parser.parse_args()

    try:
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        errors = validate(
            args.version,
            ROOT / "Cargo.toml",
            ROOT / "CHANGELOG.md",
            ROOT / "CHANGELOG-ZH.md",
        )
        if args.published_release_json is not None:
            published = json.loads(
                args.published_release_json.read_text(encoding="utf-8")
            )
            published_body = published.get("body")
            if not isinstance(published_body, str):
                errors.append("published release JSON must contain a string body")
            elif published_body != release_body(changelog, args.version):
                errors.append(
                    "published release body does not exactly match the "
                    f"CHANGELOG.md section for {args.version}"
                )
    except (OSError, KeyError, tomllib.TOMLDecodeError) as error:
        print(f"release validation failed: {error}", file=sys.stderr)
        return 1
    except json.JSONDecodeError as error:
        print(f"release validation failed: {error}", file=sys.stderr)
        return 1

    if errors:
        print("release validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    if args.notes_output is not None:
        args.notes_output.parent.mkdir(parents=True, exist_ok=True)
        args.notes_output.write_text(
            release_body(changelog, args.version),
            encoding="utf-8",
        )

    print(f"release validation passed: {args.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
