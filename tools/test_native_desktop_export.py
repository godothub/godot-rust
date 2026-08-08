#!/usr/bin/env python3
"""Export and run the independent Native Extension fixture on desktop."""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import tempfile
from pathlib import Path

from stage_example_plugin import stage_example_plugin
from test_godot_export import (
    EXPORT_MARKER,
    ROOT,
    export_configuration,
    run_checked,
    verify_output,
)
from test_native_load import (
    CLASS_MARKER,
    READY_MARKER,
    STOPPED_MARKER,
    stage_fixture,
)


EXAMPLE_ADDON = ROOT / "example" / "addons" / "godot-rust"
NATIVE_MODULE_BASENAME = "godot_native_smoke"


def prepare_native_project(
    destination: Path,
    godot: str,
    template: Path | None,
    godot_api: str,
) -> tuple[str, str, str]:
    stage_fixture(destination, godot_api)
    shutil.copytree(
        EXAMPLE_ADDON,
        destination / "addons" / "godot-rust",
    )
    project_settings = destination / "project.godot"
    project_settings.write_text(
        project_settings.read_text(encoding="utf-8")
        + '\n[editor_plugins]\n\nenabled=PackedStringArray("godot-rust")\n',
        encoding="utf-8",
    )
    preset, output_name, export_command, options, _runtime_files = (
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

{options}
""",
        encoding="utf-8",
    )
    return preset, output_name, export_command


def find_runtime_executable(exported: Path) -> Path:
    if exported.suffix != ".app":
        return exported
    executable_directory = exported / "Contents" / "MacOS"
    candidates = [
        path
        for path in executable_directory.iterdir()
        if path.is_file() and path.suffix != ".dylib"
    ]
    if len(candidates) != 1:
        raise RuntimeError(
            "Native macOS export did not contain exactly one application "
            f"executable: {candidates}"
        )
    return candidates[0]


def verify_native_library(output_directory: Path) -> Path:
    if sys.platform == "darwin":
        # Universal slices are merged into an immutable generated file whose
        # content-addressed publication name is intentionally independent of
        # the Cargo target name.
        candidates = [
            path
            for path in output_directory.rglob("*.dylib")
            if path.is_file()
        ]
    else:
        candidates = [
            path
            for path in output_directory.rglob("*")
            if path.is_file() and NATIVE_MODULE_BASENAME in path.name
        ]
    if len(candidates) != 1:
        raise RuntimeError(
            "Native desktop export did not contain exactly one extension "
            f"library: {[path.name for path in candidates]}"
        )
    if candidates[0].is_symlink() or candidates[0].stat().st_size == 0:
        raise RuntimeError("Native desktop export contains an invalid extension")
    if sys.platform == "darwin":
        architectures = set(
            run_checked(
                ["lipo", "-archs", str(candidates[0])],
                environment=dict(os.environ),
                timeout=60,
            ).split()
        )
        if architectures != {"arm64", "x86_64"}:
            raise RuntimeError(
                "Native macOS export is not Universal: "
                f"{sorted(architectures)}"
            )
    return candidates[0]


def run_native_desktop_export(
    godot: str,
    template: Path | None,
    no_build: bool,
    godot_api: str,
) -> tuple[str, str]:
    if not no_build:
        stage_example_plugin(release=False)
    with tempfile.TemporaryDirectory(
        prefix="godot-rust-native-desktop-export-"
    ) as temporary:
        project = Path(temporary) / "project"
        project.mkdir()
        preset, output_name, export_command = prepare_native_project(
            project,
            godot,
            template,
            godot_api,
        )
        output_directory = project / "build"
        output_directory.mkdir()
        exported = output_directory / output_name
        environment = {
            **os.environ,
            "CARGO_TARGET_DIR": str(
                ROOT / "target" / "godot-native-desktop-export-test"
            ),
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
            timeout=900,
        )
        verify_output(export_output, EXPORT_MARKER)
        if "mode=extension" not in export_output:
            raise RuntimeError(
                "desktop export did not select Extension Mode"
            )
        if export_command == "--export-pack":
            if not exported.is_file() or exported.stat().st_size == 0:
                raise RuntimeError("Native desktop pack export is missing")
            return export_output, ""
        if not exported.exists():
            raise RuntimeError("Native desktop export output is missing")
        verify_native_library(output_directory)

        runtime_executable = find_runtime_executable(exported)
        runtime_arguments = [
            str(runtime_executable),
            "--headless",
            "--path",
            str(output_directory),
            "--quit-after",
            "120",
        ]
        if exported.suffix == ".app":
            runtime_arguments = [
                str(runtime_executable),
                "--headless",
                "--quit-after",
                "120",
            ]
        runtime_output = run_checked(
            runtime_arguments,
            environment=environment,
            timeout=180,
        )
        for marker in (
            f"{READY_MARKER} api={godot_api}",
            CLASS_MARKER,
            f"{STOPPED_MARKER} api={godot_api}",
        ):
            verify_output(runtime_output, marker)
        return export_output, runtime_output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot", required=True)
    parser.add_argument("--template", type=Path)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument(
        "--godot-api",
        choices=("4.4", "4.5", "4.6", "4.7"),
        default="4.4",
    )
    args = parser.parse_args()
    run_native_desktop_export(
        args.godot,
        args.template,
        args.no_build,
        args.godot_api,
    )
    print("Godot Rust Native desktop export integration passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
