#!/usr/bin/env python3
"""Assemble one reproducible, installable, cross-platform plugin ZIP."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unicodedata
import uuid
import zipfile
from pathlib import Path, PurePosixPath

from create_apple_xcframework import validate_xcframework
from validate_release import parse_version


ROOT = Path(__file__).resolve().parents[1]
PROJECT_LICENSE = ROOT / "LICENSE"
ADDON_SOURCE = ROOT / "example" / "addons" / "godot-rust"
EXPECTED_BINARIES = {
    "android-arm32": ("libgodot_host.so",),
    "android-arm64": ("libgodot_host.so",),
    "android-x86_32": ("libgodot_host.so",),
    "android-x86_64": ("libgodot_host.so",),
    "apple": (
        "godot_host.xcframework/Info.plist",
        "godot_host.xcframework/ios-arm64/"
        "godot_host.framework/_CodeSignature/CodeResources",
        "godot_host.xcframework/ios-arm64/"
        "godot_host.framework/Info.plist",
        "godot_host.xcframework/ios-arm64/"
        "godot_host.framework/godot_host",
        "godot_host.xcframework/ios-arm64_x86_64-simulator/"
        "godot_host.framework/_CodeSignature/CodeResources",
        "godot_host.xcframework/ios-arm64_x86_64-simulator/"
        "godot_host.framework/Info.plist",
        "godot_host.xcframework/ios-arm64_x86_64-simulator/"
        "godot_host.framework/godot_host",
    ),
    "linux-arm32": ("libgodot_host.so",),
    "linux-x86_64": (
        "libgodot_host.so",
        "godot_build",
        "godot_module_check",
    ),
    "linux-arm64": (
        "libgodot_host.so",
        "godot_build",
        "godot_module_check",
    ),
    "linux-x86_32": ("libgodot_host.so",),
    "macos-universal": (
        "libgodot_host.dylib",
        "godot_build",
        "godot_module_check",
    ),
    "windows-x86_64": (
        "godot_host.dll",
        "godot_build.exe",
        "godot_module_check.exe",
    ),
    "windows-arm64": ("godot_host.dll",),
    "windows-x86_32": ("godot_host.dll",),
    "web-godot-4.4": ("godot_host.wasm",),
    "web-godot-4.5": ("godot_host.wasm",),
    "web-godot-4.6": ("godot_host.wasm",),
    "web-godot-4.7": ("godot_host.wasm",),
}
ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)
MAX_ZIP_ENTRIES = 10_000
MAX_ZIP_FILE_BYTES = 256 * 1024 * 1024
MAX_ZIP_TOTAL_BYTES = 2 * 1024 * 1024 * 1024


def regular_files(root: Path) -> list[Path]:
    """Return regular files below root while rejecting links and special files."""
    files: list[Path] = []
    for current, directories, filenames in os.walk(root, followlinks=False):
        current_path = Path(current)
        for name in directories:
            path = current_path / name
            if path.is_symlink():
                raise ValueError(f"release input contains a symlink: {path}")
        for name in filenames:
            path = current_path / name
            if path.is_symlink() or not path.is_file():
                raise ValueError(f"release input is not a regular file: {path}")
            files.append(path)
    return sorted(files)


def validate_addon_version(addon: Path, version: str) -> None:
    """Require plugin.cfg to advertise the exact release version."""
    config = (addon / "plugin.cfg").read_text(encoding="utf-8")
    matches = re.findall(r'^version="([^"]+)"$', config, re.MULTILINE)
    if matches != [version]:
        actual = matches if matches else ["<missing>"]
        raise ValueError(
            f"plugin.cfg version is {actual}, requested release is {version}"
        )


def validate_binaries(binaries: Path) -> None:
    """Require the exact supported platform payload matrix."""
    entries = list(binaries.iterdir()) if binaries.is_dir() else []
    if any(path.is_symlink() or not path.is_dir() for path in entries):
        raise ValueError("release binary root must contain only platform directories")
    actual_platforms = {path.name for path in entries}
    expected_platforms = set(EXPECTED_BINARIES)
    if actual_platforms != expected_platforms:
        raise ValueError(
            "release binary platforms do not match: "
            f"expected {sorted(expected_platforms)}, "
            f"found {sorted(actual_platforms)}"
        )
    for platform, expected_names in EXPECTED_BINARIES.items():
        directory = binaries / platform
        actual_names = {
            path.relative_to(directory).as_posix()
            for path in regular_files(directory)
        }
        if actual_names != set(expected_names):
            raise ValueError(
                f"{platform} binaries do not match: "
                f"expected {sorted(expected_names)}, "
                f"found {sorted(actual_names)}"
            )
    validate_xcframework(binaries / "apple" / "godot_host.xcframework")


def copy_release_tree(
    addon_source: Path,
    binaries: Path,
    destination: Path,
    version: str,
) -> Path:
    """Copy source once and inject every supported platform."""
    validate_addon_version(addon_source, version)
    validate_binaries(binaries)
    addon = destination / "addons" / "godot-rust"

    def ignore_bin(current: str, _names: list[str]) -> set[str]:
        return {"bin"} if Path(current) == addon_source else set()

    shutil.copytree(addon_source, addon, ignore=ignore_bin)
    for platform, names in EXPECTED_BINARIES.items():
        output = addon / "bin" / platform
        output.mkdir(parents=True)
        for name in names:
            destination_file = output / name
            destination_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(binaries / platform / name, destination_file)
    shutil.copyfile(PROJECT_LICENSE, addon / "LICENSE.md")
    return addon


def cargo_metadata() -> dict:
    """Read the locked Cargo graph used to build the release."""
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise ValueError(f"cargo metadata failed: {completed.stderr.strip()}")
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(f"cargo metadata returned invalid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ValueError("cargo metadata must return one JSON object")
    return value


def component_reference(package: dict) -> str:
    source = package.get("source") or "workspace"
    identity = f"{package['name']}@{package['version']}|{source}"
    digest = hashlib.sha256(identity.encode()).hexdigest()[:16]
    return f"pkg:cargo/{package['name']}@{package['version']}?identity={digest}"


def release_package_ids(metadata: dict) -> set[str]:
    """Select the non-development dependency graph of the shipped binaries."""
    packages = metadata.get("packages", [])
    roots = {
        package["id"]
        for package in packages
        if package.get("name") in {"godot_host", "godot_build"}
    }
    if len(roots) != 2:
        raise ValueError("release metadata must contain the Host and build service")
    nodes = {
        node["id"]: node for node in (metadata.get("resolve") or {}).get("nodes", [])
    }
    selected = set(roots)
    pending = list(roots)
    while pending:
        node = nodes.get(pending.pop())
        if node is None:
            raise ValueError("release dependency graph is incomplete")
        for dependency in node.get("deps", []):
            kinds = dependency.get("dep_kinds", [])
            if kinds and all(kind.get("kind") == "dev" for kind in kinds):
                continue
            package_id = dependency["pkg"]
            if package_id not in selected:
                selected.add(package_id)
                pending.append(package_id)
    return selected


def license_files(package: dict) -> list[Path]:
    manifest = package.get("manifest_path")
    if not isinstance(manifest, str):
        return []
    directory = Path(manifest).parent
    if not directory.is_dir():
        return []
    names = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")
    return sorted(
        (
            path
            for path in directory.iterdir()
            if path.is_file()
            and any(path.name.upper().startswith(name) for name in names)
            and path.stat().st_size <= 1024 * 1024
        ),
        key=lambda path: path.name,
    )


def third_party_license_document(packages: list[dict]) -> str:
    lines = [
        "# Third-party Rust dependencies",
        "",
        "Generated from the locked, non-development dependency graph of the "
        "shipped Host and build-service binaries.",
        "",
    ]
    for package in packages:
        if package.get("source") is None:
            continue
        lines.extend(
            [
                f"## {package['name']} {package['version']}",
                "",
                f"Declared license: {package.get('license') or 'not declared'}",
                "",
            ]
        )
        files = license_files(package)
        if not files:
            lines.extend(["No license or notice file was included in the crate archive.", ""])
            continue
        for path in files:
            text = path.read_text(encoding="utf-8", errors="replace").rstrip()
            lines.extend([f"### {path.name}", "", "````text", text, "````", ""])
    return "\n".join(lines)


def write_supply_chain_files(addon: Path, version: str, source_commit: str) -> None:
    """Embed deterministic dependency, license, and payload integrity records."""
    if not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise ValueError("source commit must be one lowercase 40-character Git SHA")
    metadata = cargo_metadata()
    selected = release_package_ids(metadata)
    packages = sorted(
        (
            package
            for package in metadata.get("packages", [])
            if package.get("id") in selected
        ),
        key=lambda package: (
            package.get("name", ""),
            package.get("version", ""),
            package.get("source") or "",
        ),
    )
    references = {
        package["id"]: component_reference(package) for package in packages
    }
    components = []
    for package in packages:
        component = {
            "type": "library",
            "bom-ref": references[package["id"]],
            "name": package["name"],
            "version": package["version"],
        }
        if package.get("license"):
            component["licenses"] = [{"expression": package["license"]}]
        if package.get("repository"):
            component["externalReferences"] = [
                {"type": "vcs", "url": package["repository"]}
            ]
        components.append(component)

    dependencies = []
    resolve = metadata.get("resolve") or {}
    application_reference = f"pkg:github/godothub/godot-rust@{version}"
    binary_roots = sorted(
        references[package["id"]]
        for package in packages
        if package["name"] in {"godot_host", "godot_build"}
    )
    dependencies.append({"ref": application_reference, "dependsOn": binary_roots})
    for node in sorted(resolve.get("nodes", []), key=lambda value: value["id"]):
        if node["id"] not in selected:
            continue
        reference = references.get(node["id"])
        if reference is None:
            continue
        dependencies.append(
            {
                "ref": reference,
                "dependsOn": sorted(
                    references[dependency]
                    for dependency in node.get("dependencies", [])
                    if dependency in references
                ),
            }
        )
    supply_chain = addon / "supply-chain"
    supply_chain.mkdir()
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": (
            "urn:uuid:"
            + str(
                uuid.uuid5(
                    uuid.NAMESPACE_URL,
                    "https://github.com/godothub/godot-rust/commit/"
                    + source_commit,
                )
            )
        ),
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": application_reference,
                "name": "godot-rust",
                "version": version,
            }
        },
        "components": components,
        "dependencies": dependencies,
    }
    (supply_chain / "cyclonedx.json").write_text(
        json.dumps(bom, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    (supply_chain / "THIRD_PARTY_LICENSES.md").write_text(
        third_party_license_document(packages) + "\n",
        encoding="utf-8",
    )
    build_information = {
        "schema": 1,
        "version": version,
        "source": {
            "repository": "https://github.com/godothub/godot-rust",
            "commit": source_commit,
        },
        "rust_toolchain": "1.85.0",
        "godot_host_api": "4.4",
        "platforms": sorted(EXPECTED_BINARIES),
    }
    (supply_chain / "BUILD-INFO.json").write_text(
        json.dumps(build_information, ensure_ascii=False, indent=2, sort_keys=True)
        + "\n",
        encoding="utf-8",
    )

    manifest = supply_chain / "MANIFEST.sha256"
    checksums = []
    for path in regular_files(addon):
        if path == manifest:
            continue
        relative = path.relative_to(addon).as_posix()
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {relative}")
    manifest.write_text("\n".join(checksums) + "\n", encoding="utf-8")


def write_zip(tree: Path, output: Path) -> None:
    """Write a deterministic ZIP with executable packaged binaries."""
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(
        output,
        "w",
        compression=zipfile.ZIP_DEFLATED,
        compresslevel=9,
    ) as archive:
        for path in regular_files(tree):
            relative = path.relative_to(tree).as_posix()
            info = zipfile.ZipInfo(relative, ZIP_TIMESTAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            executable_names = {
                "godot_build",
                "godot_build.exe",
                "godot_host",
                "godot_host.dll",
                "godot_module_check",
                "godot_module_check.exe",
                "libgodot_host.dylib",
                "libgodot_host.so",
            }
            mode = 0o755 if path.name in executable_names else 0o644
            info.external_attr = (stat.S_IFREG | mode) << 16
            archive.writestr(info, path.read_bytes(), compresslevel=9)


def canonical_zip_entry(name: str) -> bool:
    """Return whether a ZIP member is one canonical relative POSIX file path."""
    if (
        not name
        or name.startswith("/")
        or name.endswith("/")
        or "\\" in name
        or "\0" in name
    ):
        return False
    path = PurePosixPath(name)
    return (
        not path.is_absolute()
        and path.as_posix() == name
        and all(component not in ("", ".", "..") for component in path.parts)
    )


def verify_release_zip(path: Path, version: str) -> None:
    """Fail closed on malformed, unsafe, incomplete, or modified release ZIPs."""
    parse_version(version)
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if not entries or len(entries) > MAX_ZIP_ENTRIES:
            raise ValueError(
                f"release ZIP must contain 1..{MAX_ZIP_ENTRIES} file entries"
            )
        names = [entry.filename for entry in entries]
        if len(names) != len(set(names)):
            raise ValueError("release ZIP contains duplicate paths")
        prefix = "addons/godot-rust/"
        total_bytes = 0
        portable_names: set[str] = set()
        for entry in entries:
            if not canonical_zip_entry(entry.filename):
                raise ValueError(
                    f"release ZIP contains a noncanonical path: {entry.filename!r}"
                )
            portable_name = unicodedata.normalize(
                "NFC", entry.filename
            ).casefold()
            if portable_name in portable_names:
                raise ValueError(
                    "release ZIP contains paths that collide on supported "
                    f"filesystems: {entry.filename}"
                )
            portable_names.add(portable_name)
            if not entry.filename.startswith(prefix):
                raise ValueError(
                    f"release ZIP entry is outside {prefix}: {entry.filename}"
                )
            if entry.flag_bits & 1:
                raise ValueError(
                    f"release ZIP entry is encrypted: {entry.filename}"
                )
            if entry.compress_type != zipfile.ZIP_DEFLATED:
                raise ValueError(
                    "release ZIP entry does not use the canonical compression "
                    f"method: {entry.filename}"
                )
            mode = entry.external_attr >> 16
            if stat.S_IFMT(mode) != stat.S_IFREG:
                raise ValueError(
                    f"release ZIP entry is not a regular file: {entry.filename}"
                )
            if entry.file_size > MAX_ZIP_FILE_BYTES:
                raise ValueError(
                    f"release ZIP entry exceeds {MAX_ZIP_FILE_BYTES} bytes: "
                    f"{entry.filename}"
                )
            total_bytes += entry.file_size
            if total_bytes > MAX_ZIP_TOTAL_BYTES:
                raise ValueError(
                    f"release ZIP exceeds {MAX_ZIP_TOTAL_BYTES} uncompressed bytes"
                )
            with archive.open(entry) as source:
                while source.read(1024 * 1024):
                    pass

        config_name = f"{prefix}plugin.cfg"
        try:
            config = archive.read(config_name).decode("utf-8")
        except KeyError as error:
            raise ValueError("release ZIP is missing plugin.cfg") from error
        except UnicodeDecodeError as error:
            raise ValueError("release ZIP plugin.cfg is not UTF-8") from error
        matches = re.findall(r'^version="([^"]+)"$', config, re.MULTILINE)
        if matches != [version]:
            actual = matches if matches else ["<missing>"]
            raise ValueError(
                f"release ZIP plugin.cfg version is {actual}, expected {version}"
            )

        expected_binary_entries = {
            f"{prefix}bin/{platform}/{name}"
            for platform, filenames in EXPECTED_BINARIES.items()
            for name in filenames
        }
        actual_binary_entries = {
            name for name in names if name.startswith(f"{prefix}bin/")
        }
        if actual_binary_entries != expected_binary_entries:
            missing_binaries = sorted(
                expected_binary_entries.difference(actual_binary_entries)
            )
            unexpected_binaries = sorted(
                actual_binary_entries.difference(expected_binary_entries)
            )
            raise ValueError(
                "release ZIP platform binaries do not match: "
                f"missing={missing_binaries}, unexpected={unexpected_binaries}"
            )

        manifest_name = f"{prefix}supply-chain/MANIFEST.sha256"
        try:
            manifest_text = archive.read(manifest_name).decode("utf-8")
        except KeyError as error:
            raise ValueError("release ZIP is missing MANIFEST.sha256") from error
        except UnicodeDecodeError as error:
            raise ValueError("release ZIP MANIFEST.sha256 is not UTF-8") from error
        expected_manifest_paths = {
            name.removeprefix(prefix)
            for name in names
            if name != manifest_name
        }
        manifest_paths: set[str] = set()
        for line in manifest_text.splitlines():
            match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
            if match is None:
                raise ValueError("release ZIP manifest contains an invalid line")
            digest, relative = match.groups()
            if (
                not canonical_zip_entry(relative)
                or relative in manifest_paths
                or relative == "supply-chain/MANIFEST.sha256"
            ):
                raise ValueError(
                    f"release ZIP manifest path is invalid: {relative!r}"
                )
            manifest_paths.add(relative)
            member = f"{prefix}{relative}"
            try:
                contents = archive.read(member)
            except KeyError as error:
                raise ValueError(
                    f"release ZIP manifest references a missing file: {relative}"
                ) from error
            if hashlib.sha256(contents).hexdigest() != digest:
                raise ValueError(
                    f"release ZIP manifest digest does not match: {relative}"
                )
        if manifest_paths != expected_manifest_paths:
            missing = sorted(expected_manifest_paths.difference(manifest_paths))
            extra = sorted(manifest_paths.difference(expected_manifest_paths))
            raise ValueError(
                "release ZIP manifest coverage does not match payload: "
                f"missing={missing}, extra={extra}"
            )


def assemble_release(
    addon_source: Path,
    binaries: Path,
    output: Path,
    version: str,
    source_commit: str,
    sbom_output: Path | None = None,
) -> None:
    """Validate and assemble the complete release ZIP."""
    parse_version(version)
    with tempfile.TemporaryDirectory(prefix="godot-rust-release-") as temporary:
        tree = Path(temporary)
        addon = copy_release_tree(addon_source, binaries, tree, version)
        write_supply_chain_files(addon, version, source_commit)
        if sbom_output is not None:
            sbom_output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(
                addon / "supply-chain" / "cyclonedx.json",
                sbom_output,
            )
        write_zip(tree, output)
        verify_release_zip(output, version)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--binaries", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--addon-source", type=Path, default=ADDON_SOURCE)
    parser.add_argument("--sbom-output", type=Path)
    args = parser.parse_args()
    try:
        assemble_release(
            args.addon_source,
            args.binaries,
            args.output,
            args.version,
            args.source_commit,
            args.sbom_output,
        )
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"release assembly failed: {error}", file=sys.stderr)
        return 1
    print(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
