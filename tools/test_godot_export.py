#!/usr/bin/env python3
"""Build, export, and run a Rust-script project through official Godot."""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from stage_example_plugin import stage_example_plugin


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "example"
EXPORT_MARKER = "GODOT_RUST_EXPORT_MODULES_ADDED"
RUNTIME_MARKER = "Hello from Rust object"
RESOURCE_MARKER = "Loaded custom Rust Resource object"
INHERITANCE_MARKER = "Inherited Rust script returned: 你好, Godot!"
ERROR_MARKERS = (
    "ERROR:",
    "SCRIPT ERROR:",
    "Parse Error:",
    "Failed to load script",
    "Invalid plugin",
    "could not be loaded",
    "ObjectDB instances leaked at exit",
    "resources still in use at exit",
)
EXPORT_TIMEOUT_SECONDS = 900


def export_configuration(
    godot: str,
    template: Path | None = None,
) -> tuple[str, str, str, str, tuple[str, ...]]:
    system = sys.platform
    machine = platform.machine().lower()
    if system == "darwin":
        architecture = "arm64" if machine in ("arm64", "aarch64") else "x86_64"
        if template is not None:
            template_path = template.resolve().as_posix()
            if (
                not template.is_file()
                or template.is_symlink()
                or '"' in template_path
            ):
                raise RuntimeError(
                    f"macOS export template is not a regular safe path: {template}"
                )
            return (
                "macOS",
                "godothub_project.app",
                "--export-release",
                (
                    f'custom_template/release="{template_path}"\n'
                    'binary_format/architecture="universal"\n'
                    "export/distribution_type=0\n"
                    'application/bundle_identifier="org.godothub.rusttest"\n'
                    "codesign/codesign=1"
                ),
                (
                    "libgodot_host.dylib",
                    "libgodot_rs_project_module.dylib",
                ),
            )
        return (
            "macOS",
            "godothub_project.pck",
            "--export-pack",
            f'binary_format/architecture="{architecture}"',
            (),
        )
    if system.startswith("win"):
        return (
            "Windows Desktop",
            "godothub_project.exe",
            "--export-debug",
            (
                f'custom_template/debug="{Path(godot).resolve().as_posix()}"\n'
                'binary_format/architecture="x86_64"\n'
                "application/modify_resources=false"
            ),
            (
                "godothub_project.exe",
                "godot_host.dll",
                "godot_rs_project_module.dll",
            ),
        )
    if system.startswith("linux"):
        return (
            "Linux",
            "godothub_project.x86_64",
            "--export-debug",
            (
                f'custom_template/debug="{Path(godot).resolve().as_posix()}"\n'
                'binary_format/architecture="x86_64"\n'
                "binary_format/embed_pck=false"
            ),
            (
                "godothub_project.x86_64",
                "godothub_project.pck",
                "libgodot_host.so",
                "libgodot_rs_project_module.so",
            ),
        )
    raise RuntimeError(f"unsupported export-test platform: {system}")


def prepare_project(
    destination: Path,
    godot: str,
    template: Path | None = None,
) -> tuple[str, str, str, tuple[str, ...]]:
    shutil.copytree(
        EXAMPLE,
        destination,
        ignore=shutil.ignore_patterns(".godot", "target", "build"),
    )
    dependency = (ROOT / "crates" / "godot_rs").resolve().as_posix()
    manifest = destination / "Cargo.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            'path = "../crates/godot_rs"',
            f'path = "{dependency}"',
        ),
        encoding="utf-8",
    )
    preset, output_name, export_command, platform_options, runtime_files = (
        export_configuration(godot, template)
    )
    (destination / "export_presets.cfg").write_text(
        f"""\
[preset.0]

name="{preset}"
platform="{preset}"
runnable=true
advanced_options=false
dedicated_server=false
custom_features=""
export_filter="all_resources"
include_filter=""
exclude_filter=""
export_path="build/{output_name}"
patches=PackedStringArray()
encryption_include_filters=""
encryption_exclude_filters=""
encrypt_pck=false
encrypt_directory=false
script_export_mode=2

[preset.0.options]

{platform_options}
""",
        encoding="utf-8",
    )
    if preset == "macOS" and template is not None:
        project_settings = destination / "project.godot"
        text = project_settings.read_text(encoding="utf-8")
        marker = '[rendering]\n'
        if marker not in text:
            raise RuntimeError("example project has no rendering settings section")
        text = text.replace(
            marker,
            marker + "\ntextures/vram_compression/import_etc2_astc=true\n",
            1,
        )
        project_settings.write_text(text, encoding="utf-8")
    return preset, output_name, export_command, runtime_files


def run_checked(
    arguments: list[str],
    *,
    environment: dict[str, str],
    timeout: int,
) -> str:
    completed = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    output = completed.stdout + completed.stderr
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed with status {completed.returncode}: "
            f"{' '.join(arguments)}\n{output}"
        )
    return output


def verify_output(output: str, required_marker: str) -> None:
    errors = [marker for marker in ERROR_MARKERS if marker in output]
    if required_marker not in output:
        errors.append(f"missing marker {required_marker}")
    if errors:
        raise RuntimeError(f"Godot output verification failed: {errors}\n{output}")


def run_export_test(
    godot: str,
    no_build: bool = False,
    template: Path | None = None,
) -> tuple[str, str]:
    if not no_build:
        stage_example_plugin(release=False)
    with tempfile.TemporaryDirectory(
        prefix="godot-rust-project-export-"
    ) as temporary:
        temporary_root = Path(temporary)
        project = temporary_root / "project"
        preset, output_name, export_command, runtime_files = prepare_project(
            project, godot, template
        )
        output_directory = project / "build"
        output_directory.mkdir()
        exported = output_directory / output_name
        environment = {
            **os.environ,
            "CARGO_TARGET_DIR": str(ROOT / "target" / "godot-export-test"),
            "GODOT_SILENCE_ROOT_WARNING": "1",
        }
        export_output = run_checked(
            [
                godot,
                "--headless",
                "--editor",
                "--path",
                str(project),
                export_command,
                preset,
                str(exported),
            ],
            environment=environment,
            timeout=EXPORT_TIMEOUT_SECONDS,
        )
        verify_output(export_output, EXPORT_MARKER)
        produced_names = {
            path.name for path in output_directory.rglob("*") if path.is_file()
        }
        missing = [name for name in runtime_files if name not in produced_names]
        if not exported.exists():
            missing.append(str(exported))
        if missing:
            raise RuntimeError(f"Godot export omitted runtime files: {missing}")

        if export_command == "--export-pack":
            return export_output, ""

        runtime_executable = exported
        runtime_arguments = [
            str(runtime_executable),
            "--headless",
            "--path",
            str(output_directory),
            "--quit-after",
            "10",
        ]
        if exported.suffix == ".app":
            executable_directory = exported / "Contents/MacOS"
            candidates = [
                path
                for path in executable_directory.iterdir()
                if path.is_file() and path.suffix != ".dylib"
            ]
            if len(candidates) != 1:
                raise RuntimeError(
                    "macOS export did not contain exactly one application "
                    f"executable: {candidates}"
                )
            runtime_executable = candidates[0]
            runtime_arguments = [
                str(runtime_executable),
                "--headless",
                "--quit-after",
                "10",
            ]
        runtime_output = run_checked(
            runtime_arguments,
            environment=environment,
            timeout=120,
        )
        verify_output(runtime_output, RUNTIME_MARKER)
        verify_output(runtime_output, RESOURCE_MARKER)
        verify_output(runtime_output, INHERITANCE_MARKER)
        return export_output, runtime_output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot", required=True)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--template", type=Path)
    args = parser.parse_args()
    run_export_test(args.godot, args.no_build, args.template)
    print("Godot Rust project export integration passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
