#!/usr/bin/env python3
"""Export and run the Native Extension fixture on non-desktop platforms."""

from __future__ import annotations

import argparse
import os
import shutil
import zipfile
from pathlib import Path

from stage_example_plugin import stage_example_plugin
from test_godot_export import EXPORT_MARKER, ROOT, run_checked, verify_output
from test_native_load import (
    CLASS_MARKER,
    READY_MARKER,
    stage_fixture,
)
from test_platform_export import (
    RetryingTemporaryDirectory,
    build_ios_xcode_targets,
    configure_android_editor,
    install_android_source_template,
    platform_configuration,
    preset_text,
    run_web_export,
    stage_target_host,
    verify_ios_export,
)


EXAMPLE_ADDON = ROOT / "example" / "addons" / "godot-rust"
NATIVE_MODULE_BASENAME = "godot_rs_native_smoke"


def prepare_native_project(
    destination: Path,
    platform_name: str,
    architecture: str,
    template: Path,
    host: Path,
    debug_keystore: Path | None,
    godot_api: str,
) -> tuple[str, str]:
    stage_fixture(destination, godot_api)
    addon = destination / "addons" / "godot-rust"
    shutil.copytree(EXAMPLE_ADDON, addon)
    stage_target_host(
        addon,
        platform_name,
        architecture,
        host,
        godot_api=godot_api,
    )
    project_settings = destination / "project.godot"
    project_text = project_settings.read_text(encoding="utf-8")
    if "[editor_plugins]" in project_text:
        raise RuntimeError("Native fixture unexpectedly configures editor plugins")
    project_settings.write_text(
        project_text
        + '\n[editor_plugins]\n\nenabled=PackedStringArray("godot-rust")\n',
        encoding="utf-8",
    )
    output_name, export_command, options = platform_configuration(
        platform_name,
        architecture,
        template,
        debug_keystore,
    )
    (destination / "export_presets.cfg").write_text(
        preset_text(platform_name, output_name, options),
        encoding="utf-8",
    )
    return output_name, export_command


def verify_native_android(package: Path, architecture: str) -> None:
    abi = {
        "arm32": "armeabi-v7a",
        "arm64": "arm64-v8a",
        "x86_32": "x86",
        "x86_64": "x86_64",
    }.get(architecture)
    if abi is None:
        raise ValueError(f"unsupported Android test architecture: {architecture}")
    expected = f"lib/{abi}/lib{NATIVE_MODULE_BASENAME}.so"
    with zipfile.ZipFile(package) as archive:
        names = set(archive.namelist())
        if expected not in names:
            raise RuntimeError(
                f"Native Android APK omitted its extension library: {expected}"
            )
        if archive.getinfo(expected).file_size == 0:
            raise RuntimeError("Native Android APK contains an empty extension")


def verify_native_web(output_directory: Path) -> str:
    candidates = [
        path
        for path in output_directory.rglob("*")
        if path.is_file()
        and path.suffix == ".wasm"
        and NATIVE_MODULE_BASENAME in path.name
    ]
    if len(candidates) != 1:
        raise RuntimeError(
            "Native Web export did not contain exactly one extension module: "
            f"{[path.name for path in candidates]}"
        )
    return candidates[0].name


def run_native_platform_export(
    godot: str,
    platform_name: str,
    architecture: str,
    template: Path,
    host: Path,
    debug_keystore: Path | None,
    no_build: bool,
    evidence_output: Path,
    run_web: bool,
    android_sdk: Path | None,
    java_sdk: Path | None,
    godot_api: str,
    build_ios: bool,
    run_ios: bool,
) -> Path:
    if not no_build:
        stage_example_plugin(release=False)
    with RetryingTemporaryDirectory(
        prefix=f"godot-rust-native-{platform_name.lower()}-export-"
    ) as temporary:
        temporary_root = Path(temporary)
        project = temporary_root / "project"
        project.mkdir()
        output_name, export_command = prepare_native_project(
            project,
            platform_name,
            architecture,
            template,
            host,
            debug_keystore,
            godot_api,
        )
        output_directory = project / "build"
        output_directory.mkdir()
        exported = output_directory / output_name
        environment = {
            **os.environ,
            "CARGO_TARGET_DIR": str(
                ROOT
                / "target"
                / f"godot-native-{platform_name.lower()}-export-test"
            ),
            "GODOT_SILENCE_ROOT_WARNING": "1",
        }
        export_godot = godot
        if platform_name == "Android":
            if android_sdk is None or java_sdk is None:
                raise ValueError(
                    "Native Android validation requires Android and Java SDK paths"
                )
            install_android_source_template(project, template)
            export_godot = configure_android_editor(
                godot,
                project,
                temporary_root,
                android_sdk,
                java_sdk,
                environment,
            )
            environment["GRADLE_USER_HOME"] = str(
                temporary_root / "gradle-home"
            )
            gradle_options = environment.get("GRADLE_OPTS", "").strip()
            environment["GRADLE_OPTS"] = " ".join(
                option
                for option in (gradle_options, "-Dorg.gradle.daemon=false")
                if option
            )
        output = run_checked(
            [
                export_godot,
                "--headless",
                "--path",
                str(project),
                export_command,
                platform_name,
                str(exported),
            ],
            environment=environment,
            timeout=900,
        )
        verify_output(output, EXPORT_MARKER)
        if "mode=extension" not in output:
            raise RuntimeError("Native export did not select Extension Mode")
        if platform_name == "Android":
            if not exported.is_file():
                raise RuntimeError("Native Android export omitted its APK")
            verify_native_android(exported, architecture)
        elif platform_name == "Web":
            verify_native_web(output_directory)
            if run_web:
                run_web_export(
                    output_directory,
                    output_name,
                    required_markers=(
                        f"{READY_MARKER} api={godot_api}",
                        CLASS_MARKER,
                    ),
                    allowed_post_success_page_errors=(
                        "Failed to construct 'AudioWorkletNode': parameter 1 "
                        "is not of type 'BaseAudioContext'.",
                    ),
                )
        elif platform_name == "iOS":
            verify_ios_export(
                output_directory,
                require_script_host=False,
            )
            if build_ios:
                build_ios_xcode_targets(
                    output_directory,
                    require_script_host=False,
                    run_simulator=run_ios,
                    simulator_markers=(
                        f"{READY_MARKER} api={godot_api}",
                        CLASS_MARKER,
                    ),
                )
        else:
            raise ValueError(f"unsupported Native platform: {platform_name}")
        if evidence_output.exists():
            if (
                evidence_output.is_symlink()
                or not evidence_output.is_dir()
                or any(evidence_output.iterdir())
            ):
                raise ValueError(
                    "Native export evidence destination is not an empty directory"
                )
        else:
            evidence_output.mkdir(parents=True)
        shutil.copytree(output_directory, evidence_output, dirs_exist_ok=True)
        (evidence_output / "godot-output.log").write_text(
            output,
            encoding="utf-8",
        )
    return evidence_output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot", required=True)
    parser.add_argument(
        "--platform",
        required=True,
        choices=("Android", "Web", "iOS"),
    )
    parser.add_argument("--architecture", required=True)
    parser.add_argument("--template", type=Path, required=True)
    parser.add_argument("--host", type=Path, required=True)
    parser.add_argument("--debug-keystore", type=Path)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--evidence-output", type=Path, required=True)
    parser.add_argument("--run-web", action="store_true")
    parser.add_argument("--android-sdk", type=Path)
    parser.add_argument("--java-sdk", type=Path)
    parser.add_argument(
        "--godot-api",
        choices=("4.4", "4.5", "4.6", "4.7"),
        default="4.4",
    )
    parser.add_argument("--build-ios", action="store_true")
    parser.add_argument("--run-ios", action="store_true")
    args = parser.parse_args()
    evidence = run_native_platform_export(
        args.godot,
        args.platform,
        args.architecture,
        args.template,
        args.host,
        args.debug_keystore,
        args.no_build,
        args.evidence_output,
        args.run_web,
        args.android_sdk,
        args.java_sdk,
        args.godot_api,
        args.build_ios,
        args.run_ios,
    )
    print(f"Godot Rust Native {args.platform} export passed: {evidence}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
