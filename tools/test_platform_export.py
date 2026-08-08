#!/usr/bin/env python3
"""Export Rust-script projects for Android, iOS, and Web through Godot."""

from __future__ import annotations

import argparse
import errno
import functools
import hashlib
import http.server
import json
import os
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata
import zipfile
from pathlib import Path, PurePosixPath

from create_apple_xcframework import (
    validate_architectures,
    validate_xcframework,
)
from stage_example_plugin import stage_example_plugin
from test_godot_export import (
    EXPORT_MARKER,
    INHERITANCE_MARKER,
    RESOURCE_MARKER,
    ROOT,
    RUNTIME_MARKER,
    run_checked,
    verify_output,
)


EXAMPLE = ROOT / "example"
ANDROID_SETTINGS_FIXTURE = (
    ROOT / "tests/fixtures/android_settings/addons/android-settings"
)
ANDROID_SETTINGS_MARKER = "GODOT_RUST_ANDROID_EDITOR_SETTINGS_CONFIGURED"
MAX_ANDROID_TEMPLATE_ENTRIES = 10_000
MAX_ANDROID_TEMPLATE_FILE_BYTES = 512 * 1024 * 1024
MAX_ANDROID_TEMPLATE_TOTAL_BYTES = 1024 * 1024 * 1024
IOS_SIMULATOR_RUNTIME_TIMEOUT_SECONDS = 120
IOS_SIMULATOR_TERMINATE_TIMEOUT_SECONDS = 15


class RetryingTemporaryDirectory(tempfile.TemporaryDirectory):
    """Remove export trees after background build helpers finish closing files."""

    def cleanup(self) -> None:
        retryable = {errno.EBUSY, errno.EEXIST, errno.ENOTEMPTY}
        for attempt in range(8):
            try:
                super().cleanup()
                return
            except OSError as error:
                if error.errno not in retryable or attempt == 7:
                    raise
                time.sleep(0.1 * (2**attempt))


def resolve_toolchain_executable(
    root: Path,
    relative_executable: Path,
    description: str,
) -> tuple[Path, Path]:
    """Resolve an executable while keeping every symbolic link inside its SDK."""
    try:
        resolved_root = root.resolve(strict=True)
        resolved_executable = (root / relative_executable).resolve(strict=True)
        resolved_executable.relative_to(resolved_root)
    except (OSError, RuntimeError, ValueError) as error:
        raise ValueError(
            f"{description} is not a usable contained toolchain: {root}"
        ) from error
    if (
        not resolved_root.is_dir()
        or not resolved_executable.is_file()
        or not os.access(resolved_executable, os.X_OK)
    ):
        raise ValueError(
            f"{description} is not a usable contained toolchain: {root}"
        )
    return resolved_root, resolved_executable


def preset_text(
    platform_name: str,
    output_name: str,
    options: str,
) -> str:
    return f"""\
[preset.0]

name="{platform_name}"
platform="{platform_name}"
runnable=true
advanced_options=true
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
"""


def quoted_path(path: Path) -> str:
    value = path.resolve().as_posix()
    if '"' in value or "\n" in value or "\r" in value:
        raise ValueError(f"export path contains unsupported characters: {path}")
    return f'"{value}"'


def canonical_android_template_entry(name: str) -> bool:
    """Return whether a ZIP member is one canonical relative POSIX path."""
    if (
        not name
        or name.startswith("/")
        or "\\" in name
        or "\0" in name
    ):
        return False
    value = name[:-1] if name.endswith("/") else name
    if not value:
        return False
    path = PurePosixPath(value)
    return (
        not path.is_absolute()
        and path.as_posix() == value
        and all(component not in ("", ".", "..") for component in path.parts)
    )


def file_md5(path: Path) -> str:
    digest = hashlib.md5(usedforsecurity=False)
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def install_android_source_template(project: Path, template: Path) -> None:
    """Install Godot's Gradle source template before starting the editor."""
    if not template.is_file() or template.is_symlink():
        raise ValueError(
            f"Android source template is not a regular file: {template}"
        )
    android_directory = project / "android"
    build_directory = android_directory / "build"
    if android_directory.exists() or android_directory.is_symlink():
        raise ValueError(
            f"Android build directory already exists: {android_directory}"
        )

    try:
        archive = zipfile.ZipFile(template)
    except zipfile.BadZipFile as error:
        raise ValueError("Android source template is not a valid ZIP") from error
    with archive:
        entries = archive.infolist()
        if not entries or len(entries) > MAX_ANDROID_TEMPLATE_ENTRIES:
            raise ValueError(
                "Android source template must contain "
                f"1..{MAX_ANDROID_TEMPLATE_ENTRIES} entries"
            )
        names: set[str] = set()
        portable_names: set[str] = set()
        total_bytes = 0
        for entry in entries:
            if not canonical_android_template_entry(entry.filename):
                raise ValueError(
                    "Android source template contains a noncanonical path: "
                    f"{entry.filename!r}"
                )
            portable_name = unicodedata.normalize(
                "NFC", entry.filename.rstrip("/")
            ).casefold()
            if entry.filename in names or portable_name in portable_names:
                raise ValueError(
                    "Android source template contains duplicate paths"
                )
            names.add(entry.filename)
            portable_names.add(portable_name)
            if entry.flag_bits & 1:
                raise ValueError(
                    "Android source template contains an encrypted entry: "
                    f"{entry.filename}"
                )
            mode = entry.external_attr >> 16
            expected_type = stat.S_IFDIR if entry.is_dir() else stat.S_IFREG
            if stat.S_IFMT(mode) != expected_type:
                raise ValueError(
                    "Android source template entry is not a regular file or "
                    f"directory: {entry.filename}"
                )
            if entry.file_size > MAX_ANDROID_TEMPLATE_FILE_BYTES:
                raise ValueError(
                    "Android source template entry exceeds "
                    f"{MAX_ANDROID_TEMPLATE_FILE_BYTES} bytes: {entry.filename}"
                )
            total_bytes += entry.file_size
            if total_bytes > MAX_ANDROID_TEMPLATE_TOTAL_BYTES:
                raise ValueError(
                    "Android source template exceeds "
                    f"{MAX_ANDROID_TEMPLATE_TOTAL_BYTES} uncompressed bytes"
                )

        with tempfile.TemporaryDirectory(
            prefix="godot-rust-android-template-",
            dir=project.parent,
        ) as staging:
            staged_build = Path(staging) / "build"
            staged_build.mkdir()
            directory_modes: list[tuple[Path, int]] = []
            for entry in entries:
                relative = PurePosixPath(entry.filename.rstrip("/"))
                destination = staged_build.joinpath(*relative.parts)
                mode = entry.external_attr >> 16
                if entry.is_dir():
                    destination.mkdir(parents=True, exist_ok=True)
                    directory_modes.append((destination, mode & 0o777))
                    continue
                destination.parent.mkdir(parents=True, exist_ok=True)
                with (
                    archive.open(entry) as source,
                    destination.open("xb") as output,
                ):
                    shutil.copyfileobj(source, output, 1024 * 1024)
                if destination.stat().st_size != entry.file_size:
                    raise ValueError(
                        "Android source template entry size changed while "
                        f"extracting: {entry.filename}"
                    )
                destination.chmod(mode & 0o777)
            for directory, mode in reversed(directory_modes):
                directory.chmod(mode)
            (staged_build / ".gdignore").write_text("\n", encoding="utf-8")
            android_directory.mkdir()
            os.replace(staged_build, build_directory)

    resolved_template = template.resolve()
    (android_directory / ".build_version").write_text(
        f"{resolved_template.as_posix()} [{file_md5(resolved_template)}]\n",
        encoding="utf-8",
    )


def stage_target_host(
    addon: Path,
    platform_name: str,
    architecture: str,
    source: Path,
    godot_api: str = "4.4",
) -> None:
    if source.is_symlink():
        raise ValueError(f"target Host cannot be a symlink: {source}")
    if platform_name == "Web":
        if not source.is_file():
            raise ValueError(f"Web Host is not a file: {source}")
        if godot_api not in ("4.4", "4.5", "4.6", "4.7"):
            raise ValueError(f"unsupported Web Godot API: {godot_api}")
        destination = (
            addon / f"bin/web-godot-{godot_api}/godot_rs_host.wasm"
        )
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        return
    if platform_name == "Android":
        if not source.is_file():
            raise ValueError(f"Android Host is not a file: {source}")
        destination = (
            addon
            / f"bin/android-{architecture}/libgodot_rs_host.so"
        )
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        return
    if platform_name == "iOS":
        if (
            not source.is_dir()
            or source.name != "godot_rs_host.xcframework"
            or not (source / "Info.plist").is_file()
        ):
            raise ValueError(f"iOS Host is not the expected XCFramework: {source}")
        validate_xcframework(source)
        destination = addon / "bin/apple/godot_rs_host.xcframework"
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_dir():
                raise ValueError(
                    f"staged iOS Host path is not a directory: {destination}"
                )
            shutil.rmtree(destination)
        shutil.copytree(source, destination)
        return
    raise ValueError(f"unsupported platform Host: {platform_name}")


def platform_configuration(
    platform_name: str,
    architecture: str,
    template: Path,
    debug_keystore: Path | None,
) -> tuple[str, str, str]:
    if not template.is_file() or template.is_symlink():
        raise ValueError(f"export template is not a regular file: {template}")
    template_value = quoted_path(template)
    if platform_name == "Web":
        return (
            "godothub_project.html",
            "--export-release",
            "\n".join(
                (
                    f"custom_template/release={template_value}",
                    "variant/extensions_support=true",
                    "variant/thread_support=false",
                    "vram_texture_compression/for_desktop=true",
                    "vram_texture_compression/for_mobile=false",
                    "html/export_icon=false",
                )
            ),
        )
    if platform_name == "Android":
        architecture_abis = {
            "arm32": "armeabi-v7a",
            "arm64": "arm64-v8a",
            "x86_32": "x86",
            "x86_64": "x86_64",
        }
        if architecture not in architecture_abis:
            raise ValueError(f"unsupported Android test architecture: {architecture}")
        if (
            debug_keystore is None
            or not debug_keystore.is_file()
            or debug_keystore.is_symlink()
        ):
            raise ValueError("Android export requires a regular debug keystore")
        selected_abi = architecture_abis[architecture]
        architecture_options = {
            abi: str(abi == selected_abi).lower()
            for abi in ("armeabi-v7a", "arm64-v8a", "x86", "x86_64")
        }
        return (
            "godothub_project.apk",
            "--export-debug",
            "\n".join(
                (
                    "gradle_build/use_gradle_build=true",
                    f"gradle_build/android_source_template={template_value}",
                    'package/unique_name="org.godothub.rusttest"',
                    'package/name="godot-rust export test"',
                    "package/signed=true",
                    f"architectures/armeabi-v7a={architecture_options['armeabi-v7a']}",
                    f"architectures/arm64-v8a={architecture_options['arm64-v8a']}",
                    f"architectures/x86={architecture_options['x86']}",
                    f"architectures/x86_64={architecture_options['x86_64']}",
                    f"keystore/debug={quoted_path(debug_keystore)}",
                    'keystore/debug_user="androiddebugkey"',
                    'keystore/debug_password="android"',
                )
            ),
        )
    if platform_name == "iOS":
        if architecture != "arm64":
            raise ValueError("the iOS export test requires an arm64 device target")
        return (
            "godothub_project.ipa",
            "--export-release",
            "\n".join(
                (
                    f"custom_template/release={template_value}",
                    "architectures/arm64=true",
                    'application/app_store_team_id="GODOTHUB"',
                    'application/bundle_identifier="org.godothub.rusttest"',
                    "application/export_project_only=true",
                    "application/delete_old_export_files_unconditionally=true",
                )
            ),
        )
    raise ValueError(f"unsupported export platform: {platform_name}")


def prepare_project(
    destination: Path,
    platform_name: str,
    architecture: str,
    template: Path,
    host: Path,
    debug_keystore: Path | None,
    godot_api: str = "4.4",
) -> tuple[str, str]:
    shutil.copytree(
        EXAMPLE,
        destination,
        ignore=shutil.ignore_patterns(".godot", "target", "build"),
    )
    dependency = (ROOT / "crates/godot_rs").resolve().as_posix()
    manifest = destination / "Cargo.toml"
    manifest_text = manifest.read_text(encoding="utf-8")
    dependency_marker = 'path = "../crates/godot_rs"'
    api_marker = 'godot = "4.4"'
    if (
        manifest_text.count(dependency_marker) != 1
        or manifest_text.count(api_marker) != 1
    ):
        raise RuntimeError("example Cargo manifest has an unexpected shape")
    manifest.write_text(
        manifest_text.replace(
            dependency_marker,
            f'path = "{dependency}"',
        ).replace(
            api_marker,
            f'godot = "{godot_api}"',
        ),
        encoding="utf-8",
    )
    if platform_name in ("Android", "iOS"):
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
    stage_target_host(
        destination / "addons/godot-rust",
        platform_name,
        architecture,
        host,
        godot_api,
    )
    return output_name, export_command


def configure_android_editor(
    godot: str,
    project: Path,
    temporary_root: Path,
    android_sdk: Path,
    java_sdk: Path,
    environment: dict[str, str],
) -> str:
    resolved_android_sdk, adb = resolve_toolchain_executable(
        android_sdk,
        Path("platform-tools/adb"),
        "Android SDK",
    )
    resolved_java_sdk, java = resolve_toolchain_executable(
        java_sdk,
        Path("bin/java"),
        "Java SDK",
    )
    java_version = run_checked(
        [str(java), "-version"],
        environment=environment,
        timeout=30,
    )
    if re.search(r'\bversion "17(?:[."]|$)', java_version) is None:
        raise ValueError(
            "Android export validation requires JDK 17; "
            f"the selected Java SDK reported:\n{java_version}"
        )
    run_checked(
        [str(adb), "start-server"],
        environment=environment,
        timeout=60,
    )

    isolated_directory = temporary_root / "isolated-godot"
    isolated_directory.mkdir()
    source_godot = Path(godot)
    if not source_godot.is_file() or source_godot.is_symlink():
        raise ValueError(f"Godot editor is not a regular executable: {godot}")
    if (
        sys.platform == "darwin"
        and source_godot.parent.name == "MacOS"
        and source_godot.parent.parent.name == "Contents"
        and source_godot.parent.parent.parent.suffix == ".app"
    ):
        source_app = source_godot.parent.parent.parent
        isolated_app = isolated_directory / source_app.name
        shutil.copytree(source_app, isolated_app, symlinks=True)
        isolated_godot = (
            isolated_app / "Contents/MacOS" / source_godot.name
        )
    else:
        isolated_godot = isolated_directory / source_godot.name
        shutil.copy2(source_godot, isolated_godot)
        isolated_godot.chmod(0o755)
    (isolated_directory / "_sc_").write_text(
        "godot-rust isolated export validation\n",
        encoding="utf-8",
    )

    fixture_destination = project / "addons/android-settings"
    shutil.copytree(ANDROID_SETTINGS_FIXTURE, fixture_destination)
    project_settings = project / "project.godot"
    text = project_settings.read_text(encoding="utf-8")
    enabled = 'enabled=PackedStringArray("godot-rust")'
    configured = (
        'enabled=PackedStringArray("godot-rust", "android-settings")'
    )
    if enabled not in text:
        raise RuntimeError("example project has no canonical editor plugin list")
    project_settings.write_text(
        text.replace(enabled, configured, 1),
        encoding="utf-8",
    )
    output = run_checked(
        [
            str(isolated_godot),
            "--headless",
            "--editor",
            "--path",
            str(project),
        ],
        environment={
            **environment,
            "GODOT_RUST_TEST_ANDROID_SDK": str(resolved_android_sdk),
            "GODOT_RUST_TEST_JAVA_SDK": str(resolved_java_sdk),
        },
        timeout=180,
    )
    if ANDROID_SETTINGS_MARKER not in output:
        raise RuntimeError(
            "isolated Godot editor did not confirm Android SDK settings\n"
            f"{output}"
        )
    shutil.rmtree(fixture_destination)
    text = project_settings.read_text(encoding="utf-8")
    if configured not in text:
        raise RuntimeError("isolated Android settings plugin list changed")
    project_settings.write_text(
        text.replace(configured, enabled, 1),
        encoding="utf-8",
    )
    return str(isolated_godot)


def verify_web_export(output_directory: Path, output_name: str) -> None:
    base = Path(output_name).stem
    required = {
        output_name,
        f"{base}.js",
        f"{base}.pck",
        f"{base}.side.wasm",
        f"{base}.wasm",
        "godot_rs_host.wasm",
        "godot_rs_project_module.wasm",
    }
    actual = {
        path.relative_to(output_directory).as_posix()
        for path in output_directory.rglob("*")
        if path.is_file()
    }
    missing = sorted(required.difference(actual))
    if missing:
        raise RuntimeError(f"Web export omitted runtime files: {missing}")
    html = (output_directory / output_name).read_text(encoding="utf-8")
    for library in ("godot_rs_host.wasm", "godot_rs_project_module.wasm"):
        if library not in html:
            raise RuntimeError(f"Web bootstrap omitted {library}")


def verify_android_export(
    package: Path,
    architecture: str,
) -> None:
    abi = {
        "arm32": "armeabi-v7a",
        "arm64": "arm64-v8a",
        "x86_32": "x86",
        "x86_64": "x86_64",
    }.get(architecture)
    if abi is None:
        raise ValueError(f"unsupported Android test architecture: {architecture}")
    required = {
        f"lib/{abi}/libgodot_rs_host.so",
        f"lib/{abi}/libgodot_rs_project_module.so",
    }
    with zipfile.ZipFile(package) as archive:
        names = set(archive.namelist())
        missing = sorted(required.difference(names))
        if missing:
            raise RuntimeError(f"Android APK omitted Rust libraries: {missing}")
        for name in required:
            if archive.getinfo(name).file_size == 0:
                raise RuntimeError(f"Android APK contains an empty library: {name}")


def verify_ios_export(
    output_directory: Path,
    require_script_host: bool = True,
) -> None:
    project = output_directory / "godothub_project.xcodeproj/project.pbxproj"
    application = output_directory / "godothub_project"
    if not project.is_file() or not application.is_dir():
        raise RuntimeError("iOS export omitted its Xcode project")
    host_frameworks = list(application.rglob("godot_rs_host.xcframework"))
    expected_host_count = 1 if require_script_host else 0
    if len(host_frameworks) != expected_host_count:
        if not require_script_host:
            raise RuntimeError(
                "Native iOS Xcode project unexpectedly contains the Script Host"
            )
        raise RuntimeError("iOS Xcode project omitted the Script Host XCFramework")
    if require_script_host:
        validate_xcframework(host_frameworks[0])
    project_xcframeworks = list(
        application.rglob("godot_rs_project_module.xcframework")
    )
    if len(project_xcframeworks) != 1:
        raise RuntimeError(
            "iOS Xcode project omitted the Rust project XCFramework"
        )
    validate_project_xcframework(project_xcframeworks[0])
    project_text = project.read_text(encoding="utf-8")
    libraries = ["godot_rs_project_module.xcframework"]
    if require_script_host:
        libraries.append("godot_rs_host.xcframework")
    for library in libraries:
        if library not in project_text:
            raise RuntimeError(f"iOS Xcode project does not link {library}")
    if "CodeSignOnCopy" not in project_text:
        raise RuntimeError("iOS Rust frameworks are not configured for embedding")


def validate_project_xcframework(xcframework: Path) -> None:
    root_plist = xcframework / "Info.plist"
    if (
        xcframework.is_symlink()
        or not xcframework.is_dir()
        or root_plist.is_symlink()
        or not root_plist.is_file()
    ):
        raise RuntimeError("Rust project XCFramework has an invalid root")
    with root_plist.open("rb") as source:
        information = plistlib.load(source)
    available = information.get("AvailableLibraries")
    if not isinstance(available, list) or len(available) != 2:
        raise RuntimeError(
            "Rust project XCFramework does not contain exactly two slices"
        )
    expected = {
        "ios-arm64": ({"arm64"}, None),
        "ios-arm64_x86_64-simulator": (
            {"arm64", "x86_64"},
            "simulator",
        ),
    }
    found: set[str] = set()
    for library in available:
        if not isinstance(library, dict):
            raise RuntimeError(
                "Rust project XCFramework has invalid library metadata"
            )
        identifier = library.get("LibraryIdentifier")
        if identifier not in expected or identifier in found:
            raise RuntimeError(
                "Rust project XCFramework has an unexpected slice identifier"
            )
        found.add(identifier)
        architectures, variant = expected[identifier]
        if (
            set(library.get("SupportedArchitectures", []))
            != architectures
            or library.get("SupportedPlatform") != "ios"
            or library.get("SupportedPlatformVariant") != variant
        ):
            raise RuntimeError(
                f"Rust project XCFramework slice metadata is invalid: {identifier}"
            )
        binary_path = library.get("BinaryPath")
        library_path = library.get("LibraryPath")
        if (
            not isinstance(binary_path, str)
            or not isinstance(library_path, str)
            or not library_path.endswith(".framework")
        ):
            raise RuntimeError(
                f"Rust project XCFramework paths are invalid: {identifier}"
            )
        binary = xcframework / identifier / binary_path
        if binary.is_dir() and binary_path.endswith(".framework"):
            binary = binary / Path(binary_path).stem
        if binary.is_symlink() or not binary.is_file():
            raise RuntimeError(
                f"Rust project XCFramework binary is missing: {identifier}"
            )
        validate_architectures(binary, architectures)
    if found != set(expected):
        raise RuntimeError("Rust project XCFramework slice matrix is incomplete")


def build_ios_xcode_targets(
    output_directory: Path,
    project_framework_name: str = "libgodot_rs_project_module",
    require_script_host: bool = True,
    run_simulator: bool = False,
    simulator_markers: tuple[str, ...] = (
        RUNTIME_MARKER,
        RESOURCE_MARKER,
        INHERITANCE_MARKER,
    ),
) -> None:
    if (
        not project_framework_name
        or "/" in project_framework_name
        or "\\" in project_framework_name
        or project_framework_name in (".", "..")
    ):
        raise ValueError(
            f"invalid iOS project framework name: {project_framework_name!r}"
        )
    project = output_directory / "godothub_project.xcodeproj"
    engine_simulator = (
        output_directory
        / "godothub_project.xcframework"
        / "ios-arm64_x86_64-simulator"
        / "libgodot.a"
    )
    if not engine_simulator.is_file():
        raise RuntimeError("official Godot XCFramework omitted its Simulator library")
    engine_architectures = set(
        run_checked(
            ["lipo", "-archs", str(engine_simulator)],
            environment=dict(os.environ),
            timeout=60,
        ).split()
    )
    simulator_architecture = (
        "arm64" if "arm64" in engine_architectures else "x86_64"
    )
    if simulator_architecture not in engine_architectures:
        raise RuntimeError(
            "official Godot XCFramework has no supported Simulator architecture"
        )
    with tempfile.TemporaryDirectory(
        prefix="godot-rust-ios-xcode-builds-"
    ) as temporary:
        derived_root = Path(temporary)
        cases = (
            (
                "device",
                "iphoneos",
                "generic/platform=iOS",
                "arm64",
                derived_root / "device",
                "Release-iphoneos",
            ),
            (
                "Simulator",
                "iphonesimulator",
                "generic/platform=iOS Simulator",
                simulator_architecture,
                derived_root / "simulator",
                "Release-iphonesimulator",
            ),
        )
        for (
            label,
            sdk,
            destination,
            architecture,
            derived,
            product_directory,
        ) in cases:
            output = run_checked(
                [
                    "xcodebuild",
                    "-project",
                    str(project),
                    "-scheme",
                    "godothub_project",
                    "-configuration",
                    "Release",
                    "-sdk",
                    sdk,
                    "-destination",
                    destination,
                    "-derivedDataPath",
                    str(derived),
                    "CODE_SIGNING_ALLOWED=NO",
                    "CODE_SIGNING_REQUIRED=NO",
                    f"ARCHS={architecture}",
                    "ONLY_ACTIVE_ARCH=YES",
                    "build",
                ],
                environment=dict(os.environ),
                timeout=900,
            )
            if "** BUILD SUCCEEDED **" not in output:
                raise RuntimeError(f"iOS {label} Xcode build has no success marker")
            application = (
                derived
                / "Build"
                / "Products"
                / product_directory
                / "godothub_project.app"
            )
            executable = application / "godothub_project"
            frameworks = application / "Frameworks"
            required_binaries = [executable]
            if require_script_host:
                required_binaries.append(
                    frameworks / "godot_rs_host.framework/godot_rs_host"
                )
            required_binaries.append(
                frameworks
                / (
                    f"{project_framework_name}.framework/"
                    f"{project_framework_name}"
                )
            )
            for index, binary in enumerate(required_binaries):
                if binary.is_symlink() or not binary.is_file():
                    raise RuntimeError(
                        f"iOS {label} build omitted Rust-linked binary: {binary}"
                    )
                if index == 0:
                    validate_architectures(binary, {architecture})
                    continue
                actual = set(
                    run_checked(
                        ["lipo", "-archs", str(binary)],
                        environment=dict(os.environ),
                        timeout=60,
                    ).split()
                )
                if (
                    architecture not in actual
                    or not actual.issubset({"arm64", "x86_64"})
                ):
                    raise RuntimeError(
                        f"iOS {label} embedded Rust framework has invalid "
                        f"architectures: {sorted(actual)}"
                    )
            if label == "Simulator" and run_simulator:
                runtime_output = run_ios_simulator(application, simulator_markers)
                (output_directory / "ios-simulator-runtime.log").write_text(
                    runtime_output,
                    encoding="utf-8",
                )


def run_ios_simulator(
    application: Path,
    required_markers: tuple[str, ...],
) -> str:
    if not application.is_dir() or application.is_symlink():
        raise RuntimeError(f"iOS Simulator application is invalid: {application}")
    environment = dict(os.environ)
    runtimes = json.loads(
        run_checked(
            ["xcrun", "simctl", "list", "runtimes", "--json"],
            environment=environment,
            timeout=60,
        )
    ).get("runtimes", [])
    runtime_ids = sorted(
        (
            runtime.get("identifier")
            for runtime in runtimes
            if runtime.get("isAvailable")
            and isinstance(runtime.get("identifier"), str)
            and runtime["identifier"].startswith(
                "com.apple.CoreSimulator.SimRuntime.iOS-"
            )
        ),
        key=ios_simulator_runtime_version,
        reverse=True,
    )
    device_types = json.loads(
        run_checked(
            ["xcrun", "simctl", "list", "devicetypes", "--json"],
            environment=environment,
            timeout=60,
        )
    ).get("devicetypes", [])
    device_type = next(
        (
            device.get("identifier")
            for device in device_types
            if isinstance(device.get("identifier"), str)
            and isinstance(device.get("name"), str)
            and device["name"].startswith("iPhone")
        ),
        None,
    )
    if not runtime_ids or device_type is None:
        raise RuntimeError("Xcode has no available iOS Simulator runtime")
    device = run_checked(
        [
            "xcrun",
            "simctl",
            "create",
            "godot-rust-runtime",
            device_type,
            runtime_ids[0],
        ],
        environment=environment,
        timeout=60,
    ).strip()
    if not device:
        raise RuntimeError("simctl did not return a created device identifier")
    process: subprocess.Popen[str] | None = None
    try:
        run_checked(
            ["xcrun", "simctl", "boot", device],
            environment=environment,
            timeout=120,
        )
        run_checked(
            ["xcrun", "simctl", "bootstatus", device, "-b"],
            environment=environment,
            timeout=300,
        )
        run_checked(
            ["xcrun", "simctl", "install", device, str(application)],
            environment=environment,
            timeout=120,
        )
        process = subprocess.Popen(
            [
                "xcrun",
                "simctl",
                "launch",
                "--console-pty",
                device,
                "org.godothub.rusttest",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=environment,
        )
        try:
            output, _ = process.communicate(
                timeout=IOS_SIMULATOR_RUNTIME_TIMEOUT_SECONDS
            )
        except subprocess.TimeoutExpired:
            try:
                subprocess.run(
                    [
                        "xcrun",
                        "simctl",
                        "terminate",
                        device,
                        "org.godothub.rusttest",
                    ],
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=IOS_SIMULATOR_TERMINATE_TIMEOUT_SECONDS,
                    env=environment,
                )
            except subprocess.TimeoutExpired:
                pass
            if process.poll() is None:
                process.kill()
            try:
                output, _ = process.communicate(timeout=15)
            except subprocess.TimeoutExpired as final_timeout:
                captured = final_timeout.output or ""
                output = (
                    captured.decode("utf-8", errors="replace")
                    if isinstance(captured, bytes)
                    else captured
                )
        missing = [marker for marker in required_markers if marker not in output]
        fatal = tuple(
            marker
            for marker in ("ERROR:", "SCRIPT ERROR:", "Fatal signal", "SIGSEGV")
            if marker in output
        )
        if missing or fatal:
            raise RuntimeError(
                "iOS Simulator Rust runtime validation failed: "
                f"missing={missing}, fatal={fatal}\n{output}"
            )
        return output
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            try:
                process.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                pass
        for command in ("shutdown", "delete"):
            try:
                subprocess.run(
                    ["xcrun", "simctl", command, device],
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=60,
                    env=environment,
                )
            except subprocess.TimeoutExpired:
                pass


def ios_simulator_runtime_version(identifier: str) -> tuple[int, ...]:
    """Return the numeric version encoded in one CoreSimulator iOS runtime ID."""
    prefix = "com.apple.CoreSimulator.SimRuntime.iOS-"
    if not identifier.startswith(prefix):
        return ()
    components = identifier.removeprefix(prefix).split("-")
    if not components or any(not component.isdigit() for component in components):
        return ()
    return tuple(int(component) for component in components)


def run_web_export(
    output_directory: Path,
    output_name: str,
    required_markers: tuple[str, ...] | None = None,
    allowed_post_success_page_errors: tuple[str, ...] = (),
) -> None:
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise RuntimeError(
            "Web runtime validation requires the pinned Playwright package"
        ) from error

    class CrossOriginIsolatedHandler(http.server.SimpleHTTPRequestHandler):
        def end_headers(self) -> None:
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
            super().end_headers()

        def log_message(self, _format: str, *_arguments: object) -> None:
            return

    handler = functools.partial(
        CrossOriginIsolatedHandler,
        directory=str(output_directory),
    )
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    messages: list[str] = []
    page_errors: list[str] = []
    required = required_markers or (
        RUNTIME_MARKER,
        RESOURCE_MARKER,
        INHERITANCE_MARKER,
    )
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                headless=True,
                args=[
                    "--enable-webgl",
                    "--ignore-gpu-blocklist",
                    "--use-angle=swiftshader",
                ],
            )
            page = browser.new_page()
            page.on("console", lambda message: messages.append(message.text))
            page.on("pageerror", lambda error: page_errors.append(str(error)))
            page.goto(
                f"http://127.0.0.1:{server.server_port}/{output_name}",
                wait_until="domcontentloaded",
                timeout=120_000,
            )
            deadline = time.monotonic() + 120
            while (
                time.monotonic() < deadline
                and not all(
                    any(marker in message for message in messages)
                    for marker in required
                )
            ):
                page.wait_for_timeout(100)
            if all(
                any(marker in message for message in messages)
                for marker in required
            ):
                page.wait_for_timeout(500)
            browser.close()
    finally:
        server.shutdown()
        server.server_close()
        server_thread.join(timeout=5)
    missing = [
        marker
        for marker in required
        if not any(marker in message for message in messages)
    ]
    ignored_page_errors: list[str] = []
    if not missing:
        ignored_page_errors = [
            error
            for error in page_errors
            if error in allowed_post_success_page_errors
        ]
        page_errors = [
            error
            for error in page_errors
            if error not in allowed_post_success_page_errors
        ]
    for error in ignored_page_errors:
        print(f"Ignored post-success Godot Web page error: {error}")
    if missing or page_errors:
        raise RuntimeError(
            "Web runtime validation failed: "
            f"missing={missing}, page_errors={page_errors}\n"
            + "\n".join(messages)
        )


def run_export_test(
    godot: str,
    platform_name: str,
    architecture: str,
    template: Path,
    host: Path,
    debug_keystore: Path | None,
    no_build: bool,
    evidence_output: Path | None = None,
    run_web: bool = False,
    build_ios: bool = False,
    android_sdk: Path | None = None,
    java_sdk: Path | None = None,
    godot_api: str = "4.4",
    run_ios: bool = False,
) -> Path:
    if not no_build:
        stage_example_plugin(release=False)
    temporary = RetryingTemporaryDirectory(
        prefix=f"godot-rust-{platform_name.lower()}-export-"
    )
    temporary_root = Path(temporary.name)
    project = temporary_root / "project"
    output_name, export_command = prepare_project(
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
            ROOT / "target" / f"godot-{platform_name.lower()}-export-test"
        ),
        "GODOT_SILENCE_ROOT_WARNING": "1",
    }
    export_godot = godot
    if platform_name == "Android":
        if android_sdk is None or java_sdk is None:
            raise ValueError(
                "Android export validation requires Android and Java SDK paths"
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
    arguments = [
        export_godot,
        "--headless",
        "--path",
        str(project),
    ]
    arguments.extend(
        [
            export_command,
            platform_name,
            str(exported),
        ]
    )
    output = run_checked(
        arguments,
        environment=environment,
        timeout=900,
    )
    verify_output(output, EXPORT_MARKER)
    try:
        if platform_name == "Web":
            verify_web_export(output_directory, output_name)
            if run_web:
                run_web_export(output_directory, output_name)
        elif platform_name == "Android":
            if not exported.is_file():
                raise RuntimeError(f"Android export omitted APK: {exported}")
            verify_android_export(exported, architecture)
        else:
            verify_ios_export(output_directory)
            if build_ios:
                build_ios_xcode_targets(
                    output_directory,
                    run_simulator=run_ios,
                )
    except Exception:
        if evidence_output is not None:
            if evidence_output.exists():
                if (
                    evidence_output.is_symlink()
                    or not evidence_output.is_dir()
                    or any(evidence_output.iterdir())
                ):
                    raise ValueError(
                        "failed export evidence destination is not an empty directory"
                    )
            else:
                evidence_output.mkdir(parents=True)
            shutil.copytree(
                output_directory,
                evidence_output,
                dirs_exist_ok=True,
            )
            (evidence_output / "godot-output.log").write_text(
                output,
                encoding="utf-8",
            )
        raise
    retained = evidence_output or Path(
        tempfile.mkdtemp(prefix="godot-rust-export-evidence-")
    )
    if retained.exists():
        if retained.is_symlink() or not retained.is_dir():
            raise ValueError(
                f"export evidence destination is not a directory: {retained}"
            )
        if any(retained.iterdir()):
            raise ValueError(
                f"export evidence destination is not empty: {retained}"
            )
    else:
        retained.mkdir(parents=True)
    shutil.copytree(output_directory, retained, dirs_exist_ok=True)
    temporary.cleanup()
    return retained


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot", required=True)
    parser.add_argument(
        "--platform",
        required=True,
        choices=("Android", "iOS", "Web"),
    )
    parser.add_argument("--architecture", required=True)
    parser.add_argument("--template", type=Path, required=True)
    parser.add_argument("--host", type=Path, required=True)
    parser.add_argument("--debug-keystore", type=Path)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--evidence-output", type=Path)
    parser.add_argument("--run-web", action="store_true")
    parser.add_argument("--build-ios", action="store_true")
    parser.add_argument("--run-ios", action="store_true")
    parser.add_argument("--android-sdk", type=Path)
    parser.add_argument("--java-sdk", type=Path)
    parser.add_argument(
        "--godot-api",
        choices=("4.4", "4.5", "4.6", "4.7"),
        default="4.4",
    )
    args = parser.parse_args()
    evidence = run_export_test(
        args.godot,
        args.platform,
        args.architecture,
        args.template,
        args.host,
        args.debug_keystore,
        args.no_build,
        args.evidence_output,
        args.run_web,
        args.build_ios,
        args.android_sdk,
        args.java_sdk,
        args.godot_api,
        args.run_ios,
    )
    print(f"Godot Rust {args.platform} export integration passed: {evidence}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
