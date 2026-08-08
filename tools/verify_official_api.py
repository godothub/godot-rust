#!/usr/bin/env python3
"""Verify every Godot official API input in the workspace manifest."""

from __future__ import annotations

import argparse
import hashlib
import sys
import tomllib
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "godot-api.toml"
OFFICIAL_REPOSITORY = "https://github.com/godotengine/godot"
SUPPORTED_TARGETS = ("4.4", "4.5", "4.6", "4.7")
INPUT_SECTIONS = (
    "gdextension_interface",
    "gdextension_interface_json",
    "extension_api",
)


def sha256_bytes(content: bytes) -> str:
    """Return the lowercase SHA-256 digest of content."""
    return hashlib.sha256(content).hexdigest()


def download(url: str) -> bytes:
    """Download one official input without writing it into the repository."""
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "godot-rust-api-verifier/0.1"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read()


def load_toml(path: Path) -> dict[str, Any]:
    """Load one TOML document."""
    with path.open("rb") as source_file:
        return tomllib.load(source_file)


def source_targets(
    manifest: dict[str, Any],
) -> tuple[str, ...]:
    """Validate and return the complete ordered target matrix."""
    if manifest.get("schema_version") != 1:
        raise ValueError("godot-api.toml schema_version must be 1")
    if manifest.get("host_baseline") != "4.4":
        raise ValueError("godot-api.toml host_baseline must be 4.4")
    if manifest.get("source_repository") != OFFICIAL_REPOSITORY:
        raise ValueError(
            f"source_repository must be {OFFICIAL_REPOSITORY}"
        )
    targets = manifest.get("native_targets")
    if targets != list(SUPPORTED_TARGETS):
        raise ValueError(
            "godot-api.toml native_targets must be exactly 4.4 through 4.7"
        )
    sources = manifest.get("sources")
    if not isinstance(sources, dict):
        raise ValueError("godot-api.toml sources must be a table")
    if tuple(sorted(sources)) != SUPPORTED_TARGETS:
        raise ValueError("source entries must match native_targets")
    return SUPPORTED_TARGETS


def is_lowercase_sha256(value: object) -> bool:
    """Return whether a value is a canonical SHA-256 digest."""
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def validate_manifest(
    manifest: dict[str, Any],
    expected_version: str,
) -> list[str]:
    """Return deterministic provenance errors for one target entry."""
    source_targets(manifest)
    if expected_version not in SUPPORTED_TARGETS:
        raise ValueError(f"unsupported Godot API target: {expected_version}")
    source = manifest["sources"][expected_version]
    if not isinstance(source, dict):
        raise ValueError(
            f"Godot {expected_version} source entry must be a table"
        )
    errors: list[str] = []
    expected_tag = f"{expected_version}-stable"
    if source.get("godot_tag") != expected_tag:
        errors.append(f"godot_tag must be {expected_tag}")

    for section_name in INPUT_SECTIONS:
        section = source.get(section_name)
        if section is None:
            if (
                section_name == "gdextension_interface_json"
                and expected_version in {"4.4", "4.5"}
            ):
                continue
            errors.append(f"missing [{section_name}]")
            continue
        if not isinstance(section, dict):
            errors.append(f"{section_name} must be a table")
            continue
        expected_filename = {
            "gdextension_interface": "gdextension_interface.h",
            "gdextension_interface_json": "gdextension_interface.json",
            "extension_api": "extension_api.json",
        }[section_name]
        if section.get("filename") != expected_filename:
            errors.append(
                f"{section_name}.filename must be {expected_filename}"
            )
        if not is_lowercase_sha256(section.get("sha256")):
            errors.append(
                f"{section_name}.sha256 must be lowercase SHA-256"
            )
        url = section.get("url")
        acquisition = section.get("acquisition")
        if not url and not acquisition:
            errors.append(
                f"{section_name} must provide url or acquisition"
            )
        if url:
            prefix = (
                "https://raw.githubusercontent.com/godotengine/godot/"
                f"{expected_tag}/"
            )
            if not url.startswith(prefix):
                errors.append(
                    f"{section_name}.url must use official tag {expected_tag}"
                )

    extension_api = source.get("extension_api", {})
    generator_version = extension_api.get("generator_version", "")
    if not generator_version.startswith(f"{expected_version}."):
        errors.append(
            "extension_api.generator_version must match Godot target"
        )
    return errors


def verify_source(
    manifest_path: Path,
    target: str,
    supplied_inputs: dict[str, Path] | None = None,
) -> list[str]:
    """Return verification errors for one official target entry."""
    manifest = load_toml(manifest_path)
    errors = validate_manifest(manifest, target)
    supplied_inputs = supplied_inputs or {}
    source = manifest["sources"][target]

    for section_name in INPUT_SECTIONS:
        section = source.get(section_name)
        if section is None:
            continue
        expected = section.get("sha256")
        if not is_lowercase_sha256(expected):
            continue

        if section_name in supplied_inputs:
            actual = sha256_bytes(supplied_inputs[section_name].read_bytes())
        elif section.get("url"):
            actual = sha256_bytes(download(section["url"]))
        else:
            continue
        if actual != expected:
            errors.append(
                f"{section_name} SHA-256 mismatch: "
                f"expected {expected}, got {actual}"
            )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="combined Godot API target and source manifest",
    )
    parser.add_argument(
        "--godot",
        choices=SUPPORTED_TARGETS,
        help="verify one Godot target instead of the complete matrix",
    )
    parser.add_argument(
        "--extension-api",
        type=Path,
        help="freshly dumped extension_api.json to authenticate",
    )
    parser.add_argument(
        "--gdextension-interface",
        type=Path,
        help="official or freshly dumped gdextension_interface.h to authenticate",
    )
    parser.add_argument(
        "--gdextension-interface-json",
        type=Path,
        help="official gdextension_interface.json to authenticate",
    )
    args = parser.parse_args()

    supplied_inputs = {
        name: path
        for name, path in (
            ("extension_api", args.extension_api),
            ("gdextension_interface", args.gdextension_interface),
            (
                "gdextension_interface_json",
                args.gdextension_interface_json,
            ),
        )
        if path is not None
    }

    try:
        manifest = load_toml(args.manifest)
        targets = (
            (args.godot,)
            if args.godot is not None
            else source_targets(manifest)
        )
        if supplied_inputs and args.godot is None:
            raise ValueError(
                "supplied input paths require an explicit --godot target"
            )

        all_errors: list[str] = []
        for target in targets:
            errors = verify_source(
                args.manifest,
                target,
                supplied_inputs,
            )
            all_errors.extend(
                f"Godot {target}: {error}" for error in errors
            )
    except (
        OSError,
        KeyError,
        TypeError,
        ValueError,
        tomllib.TOMLDecodeError,
    ) as error:
        print(f"official API verification failed: {error}", file=sys.stderr)
        return 1

    if all_errors:
        print("official API verification failed:", file=sys.stderr)
        for error in all_errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        "official API verification passed: "
        + ", ".join(f"Godot {target}" for target in targets)
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
