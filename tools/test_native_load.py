#!/usr/bin/env python3
"""Build and load an independent godot_rs Native extension fixture."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "native_extension"
SUPPORTED_APIS = ("4.4", "4.5", "4.6", "4.7")
READY_MARKER = "GODOT_RS_NATIVE_READY"
STOPPED_MARKER = "GODOT_RS_NATIVE_STOPPED"
CLASS_MARKER = "GODOT_RS_NATIVE_CLASS_OK"
ERROR_PATTERNS = (
    re.compile(r"ERROR:", re.IGNORECASE),
    re.compile(r"can't open dynamic library", re.IGNORECASE),
    re.compile(r"failed loading", re.IGNORECASE),
    re.compile(r"entry point .* not found", re.IGNORECASE),
    re.compile(r"godot_rs_native_init.*not found", re.IGNORECASE),
)


def render_manifest(api: str, godot_rs_path: Path) -> str:
    """Render a standalone Cargo project using one user-facing API target."""
    if api not in SUPPORTED_APIS:
        raise ValueError(f"unsupported Godot API target: {api}")
    dependency_path = godot_rs_path.resolve().as_posix().replace('"', '\\"')
    return f"""\
[workspace]

[package]
name = "godot_rs_native_smoke"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
publish = false

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
godot_rs = {{ path = "{dependency_path}" }}

[package.metadata.godot-rust]
enabled = true
mode = "extension"
godot = "{api}"
"""


def stage_fixture(directory: Path, api: str) -> None:
    """Create a standalone Godot/Cargo Native fixture without changing the repo."""
    shutil.copy2(FIXTURE / "project.godot", directory / "project.godot")
    shutil.copy2(ROOT / "example" / "icon.svg", directory / "icon.svg")
    shutil.copy2(FIXTURE / "main.gd", directory / "main.gd")
    shutil.copy2(FIXTURE / "hot_reload.gd", directory / "hot_reload.gd")
    shutil.copy2(FIXTURE / "main.tscn", directory / "main.tscn")
    source_directory = directory / "src"
    source_directory.mkdir()
    shutil.copy2(FIXTURE / "src" / "lib.rs", source_directory / "lib.rs")
    (directory / "Cargo.toml").write_text(
        render_manifest(api, ROOT / "crates" / "godot_rs"),
        encoding="utf-8",
    )


def build_native_fixture(directory: Path, api: str) -> dict[str, object]:
    """Run the same structured Native Build request used by the editor plugin."""
    request = json.dumps(
        {
            "command": "project_cargo",
            "id": 1,
            "root": str(directory),
            "kind": "build",
        }
    )
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "-p",
            "godot_rs_buildd",
            "--",
            "--request",
            request,
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=300,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "Native build daemon process failed: "
            f"{completed.stderr.strip() or completed.stdout.strip()}"
        )
    try:
        response = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(
            f"Native build daemon returned invalid JSON: {error}"
        ) from error
    if not response.get("ok"):
        raise RuntimeError(
            f"Native build failed: {response.get('error', 'unknown error')}"
        )
    result = response.get("result")
    if not isinstance(result, dict):
        raise RuntimeError("Native build response has no result object")
    if result.get("mode") != "extension":
        raise RuntimeError("project task did not select Extension Mode")
    report = require_dict(result, "report")
    validate_build_result(report, directory, api)
    return report


def validate_build_result(
    result: dict[str, object], directory: Path, api: str
) -> None:
    """Validate the version selection, immutable product, and descriptor."""
    native = require_dict(result, "native")
    cargo = require_dict(result, "cargo")
    environment = require_dict(cargo, "environment")
    if native.get("godot_api") != api:
        raise RuntimeError("Native build selected the wrong Godot API")
    if native.get("compatibility_minimum") != api:
        raise RuntimeError("Native descriptor minimum does not match its API")
    if native.get("entry_symbol") != "godot_rs_native_init":
        raise RuntimeError("Native build selected an unexpected entry symbol")
    if environment.get("GODOT_RS_GODOT") != api:
        raise RuntimeError("Native Cargo task did not receive the selected API")
    if not cargo.get("success"):
        diagnostics = cargo.get("diagnostics", [])
        messages = [
            str(diagnostic.get("rendered") or diagnostic.get("message"))
            for diagnostic in diagnostics
            if isinstance(diagnostic, dict)
        ]
        detail = "\n".join(messages) or str(cargo.get("stderr", "")).strip()
        raise RuntimeError(
            "Native Cargo task was not successful"
            + (f":\n{detail}" if detail else "")
        )
    publication = require_dict(result, "publication")

    descriptor_path = Path(require_string(publication, "descriptor_path"))
    state_path = Path(require_string(publication, "state_path"))
    library_path = Path(require_string(publication, "library_path"))
    for product in (descriptor_path, state_path, library_path):
        if not path_is_within(product, directory):
            raise RuntimeError(
                f"Native product escaped the project root: {product}"
            )
        if not product.is_file():
            raise RuntimeError(f"Native product is missing: {product}")

    descriptor = descriptor_path.read_text(encoding="utf-8")
    if f'compatibility_minimum = "{api}"' not in descriptor:
        raise RuntimeError("generated descriptor has the wrong minimum")
    if 'entry_symbol = "godot_rs_native_init"' not in descriptor:
        raise RuntimeError("generated descriptor has the wrong entry symbol")
    if "compatibility_maximum" in descriptor:
        raise RuntimeError("generated descriptor must not set a maximum")
    if "reloadable = true" not in descriptor:
        raise RuntimeError("Native descriptor must enable editor hot reload")
    expected_hash = require_string(publication, "sha256")
    actual_hash = hashlib.sha256(library_path.read_bytes()).hexdigest()
    if actual_hash != expected_hash:
        raise RuntimeError("published Native library hash does not match")
    if expected_hash not in library_path.parts:
        raise RuntimeError("published Native library is not hash-addressed")


def strip_windows_extended_prefix(path: str) -> str:
    if path.startswith("\\\\?\\UNC\\"):
        return "\\\\" + path[8:]
    if path.startswith("\\\\?\\"):
        return path[4:]
    return path


def comparison_path(path: Path) -> str:
    resolved = strip_windows_extended_prefix(str(path.resolve()))
    return os.path.normcase(os.path.normpath(resolved))


def path_is_within(path: Path, directory: Path) -> bool:
    candidate = comparison_path(path)
    root = comparison_path(directory)
    try:
        return os.path.commonpath((candidate, root)) == root
    except ValueError:
        return False


def require_dict(mapping: dict[str, object], key: str) -> dict[str, object]:
    value = mapping.get(key)
    if not isinstance(value, dict):
        raise RuntimeError(f"Native build response `{key}` is not an object")
    return value


def require_string(mapping: dict[str, object], key: str) -> str:
    value = mapping.get(key)
    if not isinstance(value, str):
        raise RuntimeError(f"Native build response `{key}` is not a string")
    return value


def discover_godot(explicit: str | None) -> str | None:
    """Resolve an explicit path, environment setting, or common executable."""
    candidates = (
        explicit,
        os.environ.get("GODOT_BIN"),
        shutil.which("godot4"),
        shutil.which("godot"),
    )
    return next((candidate for candidate in candidates if candidate), None)


def run_godot(godot: str, directory: Path, api: str, valgrind: bool) -> str:
    """Run the project and require Native class plus lifecycle evidence."""
    environment = {**os.environ, "GODOT_SILENCE_ROOT_WARNING": "1"}
    discovery = subprocess.run(
        [
            godot,
            "--headless",
            "--editor",
            "--path",
            str(directory),
            "--quit-after",
            "120",
        ],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    run_command = [
        godot,
        "--headless",
        "--path",
        str(directory),
        "--quit-after",
        "120",
    ]
    if valgrind:
        run_command = [
            "valgrind",
            "--error-exitcode=97",
            "--leak-check=no",
            "--num-callers=40",
            "--undef-value-errors=no",
            *run_command,
        ]
    completed = subprocess.run(
        run_command,
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        timeout=180 if valgrind else 60,
    )
    output = (
        discovery.stdout
        + discovery.stderr
        + completed.stdout
        + completed.stderr
    )
    errors = output_errors(output, api)
    if discovery.returncode != 0 or completed.returncode != 0 or errors:
        raise RuntimeError(
            "Godot Native load failed: "
            f"discovery_status={discovery.returncode}, "
            f"run_status={completed.returncode}, evidence={errors}\n{output}"
        )
    return output


def output_errors(output: str, api: str) -> list[str]:
    """Return loader errors and missing lifecycle evidence."""
    errors = [
        pattern.pattern for pattern in ERROR_PATTERNS if pattern.search(output)
    ]
    for marker in (
        f"{READY_MARKER} api={api}",
        CLASS_MARKER,
        f"{STOPPED_MARKER} api={api}",
    ):
        if marker not in output:
            errors.append(f"missing marker {marker}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api", choices=SUPPORTED_APIS, default="4.4")
    parser.add_argument("--godot")
    parser.add_argument(
        "--valgrind",
        action="store_true",
        help="run the Native project under Valgrind and fail on memory errors",
    )
    parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="build and verify products, but skip runtime load if Godot is unavailable",
    )
    args = parser.parse_args()

    try:
        with tempfile.TemporaryDirectory(
            prefix=f"godot-rust-native-{args.api}-"
        ) as temporary:
            directory = Path(temporary)
            stage_fixture(directory, args.api)
            build_native_fixture(directory, args.api)
            godot = discover_godot(args.godot)
            if godot is None:
                if args.allow_missing:
                    print(
                        f"SKIP: Native {args.api} products passed; "
                        "Godot executable is unavailable"
                    )
                    return 0
                raise RuntimeError(
                    "Godot executable not found; set GODOT_BIN or pass --godot"
                )
            output = run_godot(godot, directory, args.api, args.valgrind)
            print(output, end="")
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Native Extension load test failed: {error}", file=sys.stderr)
        return 1

    print(f"Godot {args.api} Native Extension load test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
