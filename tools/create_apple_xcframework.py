#!/usr/bin/env python3
"""Create and verify the iOS Script Host XCFramework."""

from __future__ import annotations

import argparse
import os
import plistlib
import re
import shutil
import subprocess
import tempfile
from pathlib import Path


FRAMEWORK_NAME = "godot_host"
BUNDLE_IDENTIFIER = "io.godothub.godot-rust.host"
EXPECTED_LIBRARIES = {
    "ios-arm64": {
        "architectures": {"arm64"},
        "platform": "ios",
        "variant": None,
    },
    "ios-arm64_x86_64-simulator": {
        "architectures": {"arm64", "x86_64"},
        "platform": "ios",
        "variant": "simulator",
    },
}


def run_output(command: list[str]) -> str:
    """Run one Apple packaging command and return bounded text output."""
    completed = subprocess.run(
        command,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        details = completed.stderr.strip() or completed.stdout.strip()
        raise ValueError(f"{' '.join(command)} failed: {details}")
    return completed.stdout.strip()


def run(command: list[str]) -> None:
    """Run one Apple packaging command with useful failure output."""
    run_output(command)


def validate_architectures(binary: Path, expected: set[str]) -> None:
    """Require the exact architecture set recorded in one Mach-O binary."""
    actual = set(run_output(["lipo", "-archs", str(binary)]).split())
    if actual != expected:
        raise ValueError(
            f"{binary} architectures do not match: "
            f"expected {sorted(expected)}, found {sorted(actual)}"
        )


def framework_plist(
    *,
    platform: str,
    version: str,
) -> dict[str, object]:
    """Return a minimal, distributable framework property list."""
    supported_platforms = {
        "macos": ["MacOSX"],
        "ios": ["iPhoneOS"],
        "ios-simulator": ["iPhoneSimulator"],
    }
    try:
        selected_platforms = supported_platforms[platform]
    except KeyError as error:
        raise ValueError(
            f"unsupported Apple framework platform: {platform}"
        ) from error
    return {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": FRAMEWORK_NAME,
        "CFBundleIdentifier": BUNDLE_IDENTIFIER,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": FRAMEWORK_NAME,
        "CFBundlePackageType": "FMWK",
        "CFBundleShortVersionString": version,
        "CFBundleSupportedPlatforms": selected_platforms,
        "CFBundleVersion": version,
    }


def create_framework(
    binary: Path,
    destination: Path,
    *,
    platform: str,
    version: str,
) -> Path:
    """Wrap one already-combined Mach-O dynamic library in a flat framework."""
    if not binary.is_file() or binary.is_symlink():
        raise ValueError(f"framework input is not a regular file: {binary}")
    framework = destination / f"{FRAMEWORK_NAME}.framework"
    framework.mkdir(parents=True)
    executable = framework / FRAMEWORK_NAME
    shutil.copyfile(binary, executable)
    executable.chmod(0o755)
    install_name = f"@rpath/{FRAMEWORK_NAME}.framework/{FRAMEWORK_NAME}"
    run(["install_name_tool", "-id", install_name, str(executable)])
    with (framework / "Info.plist").open("wb") as output:
        plistlib.dump(
            framework_plist(platform=platform, version=version),
            output,
            fmt=plistlib.FMT_XML,
            sort_keys=True,
        )
    run(
        [
            "codesign",
            "--force",
            "--sign",
            "-",
            "--timestamp=none",
            str(framework),
        ]
    )
    return framework


def regular_tree(root: Path) -> tuple[set[str], set[str]]:
    """Return the exact directory/file tree and reject links or special files."""
    directories_found: set[str] = set()
    files: set[str] = set()
    for current, directories, filenames in os.walk(root, followlinks=False):
        current_path = Path(current)
        for name in directories:
            path = current_path / name
            if path.is_symlink():
                raise ValueError(f"XCFramework contains a symlink: {path}")
            directories_found.add(path.relative_to(root).as_posix())
        for name in filenames:
            path = current_path / name
            if path.is_symlink() or not path.is_file():
                raise ValueError(f"XCFramework entry is not a regular file: {path}")
            files.add(path.relative_to(root).as_posix())
    return directories_found, files


def validate_xcframework(xcframework: Path) -> None:
    """Require the exact iOS-device and iOS-simulator slice matrix."""
    root_plist = xcframework / "Info.plist"
    if not root_plist.is_file() or root_plist.is_symlink():
        raise ValueError("XCFramework Info.plist is missing")
    with root_plist.open("rb") as source:
        information = plistlib.load(source)
    if information.get("XCFrameworkFormatVersion") != "1.0":
        raise ValueError("unsupported XCFramework format version")
    available = information.get("AvailableLibraries")
    if not isinstance(available, list):
        raise ValueError("XCFramework AvailableLibraries is missing")

    actual: dict[str, dict[str, object]] = {}
    expected_files = {"Info.plist"}
    for library in available:
        if not isinstance(library, dict):
            raise ValueError("XCFramework library metadata must be a dictionary")
        identifier = library.get("LibraryIdentifier")
        if not isinstance(identifier, str) or identifier in actual:
            raise ValueError(
                "XCFramework library identifier is invalid or duplicated"
            )
        actual[identifier] = library
    if set(actual) != set(EXPECTED_LIBRARIES):
        raise ValueError(
            "XCFramework slices do not match: "
            f"expected {sorted(EXPECTED_LIBRARIES)}, found {sorted(actual)}"
        )

    for identifier, expected in EXPECTED_LIBRARIES.items():
        library = actual[identifier]
        library_path = f"{FRAMEWORK_NAME}.framework"
        if library.get("LibraryPath") != library_path:
            raise ValueError(f"{identifier} has an unexpected LibraryPath")
        if library.get("BinaryPath") != f"{library_path}/{FRAMEWORK_NAME}":
            raise ValueError(f"{identifier} has an unexpected BinaryPath")
        supported_architectures = library.get("SupportedArchitectures")
        if not isinstance(supported_architectures, list) or not all(
            isinstance(value, str) for value in supported_architectures
        ):
            raise ValueError(f"{identifier} has invalid architecture metadata")
        if set(supported_architectures) != expected["architectures"]:
            raise ValueError(f"{identifier} has unexpected architectures")
        if library.get("SupportedPlatform") != expected["platform"]:
            raise ValueError(f"{identifier} has an unexpected platform")
        if library.get("SupportedPlatformVariant") != expected["variant"]:
            raise ValueError(f"{identifier} has an unexpected platform variant")

        framework = xcframework / identifier / library_path
        executable = framework / FRAMEWORK_NAME
        framework_info = framework / "Info.plist"
        if not executable.is_file() or executable.is_symlink():
            raise ValueError(f"{identifier} framework executable is missing")
        if not framework_info.is_file() or framework_info.is_symlink():
            raise ValueError(f"{identifier} framework Info.plist is missing")
        with framework_info.open("rb") as source:
            framework_metadata = plistlib.load(source)
        if framework_metadata.get("CFBundleExecutable") != FRAMEWORK_NAME:
            raise ValueError(f"{identifier} framework executable metadata is invalid")
        if framework_metadata.get("CFBundleIdentifier") != BUNDLE_IDENTIFIER:
            raise ValueError(f"{identifier} framework identifier is invalid")
        expected_files.update(
            {
                f"{identifier}/{library_path}/_CodeSignature/CodeResources",
                f"{identifier}/{library_path}/Info.plist",
                f"{identifier}/{library_path}/{FRAMEWORK_NAME}",
            }
        )

    expected_directories = {
        parent.as_posix()
        for file in expected_files
        for parent in Path(file).parents
        if parent != Path(".")
    }
    actual_directories, actual_files = regular_tree(xcframework)
    if (
        actual_directories != expected_directories
        or actual_files != expected_files
    ):
        raise ValueError(
            "XCFramework payload does not match: "
            f"expected directories {sorted(expected_directories)} and files "
            f"{sorted(expected_files)}, found directories "
            f"{sorted(actual_directories)} and files {sorted(actual_files)}"
        )


def normalize_xcframework_metadata(xcframework: Path) -> None:
    """Make xcodebuild's nondeterministically ordered library list stable."""
    root_plist = xcframework / "Info.plist"
    with root_plist.open("rb") as source:
        information = plistlib.load(source)
    available = information.get("AvailableLibraries")
    if not isinstance(available, list) or not all(
        isinstance(library, dict) for library in available
    ):
        raise ValueError("XCFramework AvailableLibraries is missing")
    try:
        information["AvailableLibraries"] = sorted(
            available,
            key=lambda library: library["LibraryIdentifier"],
        )
    except (KeyError, TypeError) as error:
        raise ValueError(
            "XCFramework library identifier metadata is invalid"
        ) from error
    with root_plist.open("wb") as output:
        plistlib.dump(
            information,
            output,
            fmt=plistlib.FMT_XML,
            sort_keys=True,
        )


def create_xcframework(
    *,
    ios_device: Path,
    ios_simulator: Path,
    output: Path,
    version: str,
) -> None:
    """Create a single validated XCFramework without mutating an old output."""
    if (
        re.fullmatch(
            r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)",
            version,
        )
        is None
    ):
        raise ValueError(f"invalid framework version: {version}")
    if output.exists() or output.is_symlink():
        raise ValueError(f"refusing to replace XCFramework output: {output}")
    validate_architectures(ios_device, {"arm64"})
    validate_architectures(ios_simulator, {"arm64", "x86_64"})
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="godot-rust-xcframework-") as temporary:
        staging = Path(temporary)
        frameworks = [
            create_framework(
                ios_device,
                staging / "ios-device",
                platform="ios",
                version=version,
            ),
            create_framework(
                ios_simulator,
                staging / "ios-simulator",
                platform="ios-simulator",
                version=version,
            ),
        ]
        command = ["xcodebuild", "-create-xcframework"]
        for framework in frameworks:
            command.extend(["-framework", str(framework)])
        command.extend(["-output", str(output)])
        run(command)
    normalize_xcframework_metadata(output)
    validate_xcframework(output)
    for identifier, expected in EXPECTED_LIBRARIES.items():
        framework = (
            output
            / identifier
            / f"{FRAMEWORK_NAME}.framework"
        )
        executable = framework / FRAMEWORK_NAME
        validate_architectures(executable, expected["architectures"])
        run(["codesign", "--verify", "--strict", str(framework)])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ios-device", type=Path, required=True)
    parser.add_argument("--ios-simulator", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    arguments = parser.parse_args()
    create_xcframework(
        ios_device=arguments.ios_device,
        ios_simulator=arguments.ios_simulator,
        output=arguments.output,
        version=arguments.version,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
