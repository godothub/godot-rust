#!/usr/bin/env python3
"""Build SDK archives offline before their interdependent versions are published.

Requires Python 3.12+, the selected Rust toolchain, and cached Cargo dependencies.
Only a temporary workspace lockfile is changed. Cargo's CLI patch configuration
resolves unpublished SDK dependencies to extracted archives, never source crates.
The resulting archives are audit artifacts: publication must regenerate their
lockfiles against the registry after publishing dependencies in order.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SDK_CRATES = ("godot_api", "godot_macro", "godot_rs")
GENERATED_FILES = {".cargo_vcs_info.json", "Cargo.lock", "Cargo.toml"}


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def tree_hashes(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): sha256(path.read_bytes())
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def registry_packages(lock: bytes) -> set[tuple[str, str, str, str]]:
    return {
        (item["name"], item["version"], item["source"], item["checksum"])
        for item in tomllib.loads(lock.decode())["package"]
        if item.get("source", "").startswith("registry+")
    }


def run(
    command: list[str],
    *,
    cwd: Path,
    log_dir: Path,
    label: str,
    env: dict[str, str] | None = None,
) -> str:
    log_path = log_dir / f"sdk-{label}.log"
    print(f"{label}: {shlex.join(command)}", flush=True)
    completed = subprocess.run(
        command, cwd=cwd, env=env, text=True, capture_output=True, check=False
    )
    environment = f"GODOT_RS_GODOT={env['GODOT_RS_GODOT']} " if env is not None else ""
    log_path.write_text(
        f"$ {environment}{shlex.join(command)}\n"
        + completed.stdout
        + completed.stderr
        + f"\nexit_code={completed.returncode}\n"
    )
    if completed.returncode:
        raise RuntimeError(f"{label} failed; see {log_path}")
    return completed.stdout


def verify_archive(
    archive_path: Path,
    *,
    crate: str,
    version: str,
    expected_files: set[str],
    snapshot: Path,
    extracted: Path,
    locked_registry: set[tuple[str, str, str, str]],
) -> dict[str, object]:
    prefix = f"{crate}-{version}"
    with tarfile.open(archive_path) as archive:
        members = archive.getmembers()
        names = {member.name.removeprefix(prefix + "/") for member in members}
        if len(names) != len(members) or any(
            not member.isfile() or not member.name.startswith(prefix + "/")
            for member in members
        ):
            raise RuntimeError(f"{crate} archive has unexpected members")
        expected = expected_files - {".cargo_vcs_info.json"}
        if names != expected:
            raise RuntimeError(
                f"{crate} archive differs from cargo package --list: "
                f"missing={sorted(expected - names)}, extra={sorted(names - expected)}"
            )
        # All archives are produced by this process; data filtering additionally
        # rejects path traversal when extracting with supported Python versions.
        archive.extractall(extracted, filter="data")
    package_root = extracted / prefix
    manifest = tomllib.loads((package_root / "Cargo.toml").read_text())
    if (manifest["package"]["name"], manifest["package"]["version"]) != (
        crate, version
    ):
        raise RuntimeError(f"{crate} archive identity does not match the workspace")
    for section in ("dependencies", "build-dependencies", "dev-dependencies"):
        for dependency in manifest.get(section, {}).values():
            if "path" in dependency or "git" in dependency:
                raise RuntimeError(f"{crate} archive has a non-registry dependency")
    unexpected = registry_packages((package_root / "Cargo.lock").read_bytes()) - locked_registry
    if unexpected:
        raise RuntimeError(f"{crate} changed locked registry dependencies: {sorted(unexpected)}")
    source_hashes = {}
    for name in sorted(names - GENERATED_FILES):
        source_name = "Cargo.toml" if name == "Cargo.toml.orig" else name
        source = snapshot / "crates" / crate / source_name
        packaged = package_root / name
        if packaged.read_bytes() != source.read_bytes():
            raise RuntimeError(f"{crate}/{name} differs from the source snapshot")
        source_hashes[f"crates/{crate}/{source_name}"] = sha256(source.read_bytes())
    return {
        "name": crate,
        "version": version,
        "archive": str(archive_path),
        "archive_sha256": sha256(archive_path.read_bytes()),
        "archive_bytes": archive_path.stat().st_size,
        "file_count": len(names),
        "source_sha256": source_hashes,
    }


def verify(*, toolchain: str, target_dir: Path, log_dir: Path) -> dict[str, object]:
    root_manifest = (ROOT / "Cargo.toml").read_text()
    workspace = tomllib.loads(root_manifest)["workspace"]
    version = workspace["package"]["version"]
    original_lock = (ROOT / "Cargo.lock").read_bytes()
    locked_registry = registry_packages(original_lock)
    cargo = ["cargo", f"+{toolchain}"]
    target_dir.mkdir(parents=True, exist_ok=True)
    log_dir.mkdir(parents=True, exist_ok=True)
    rustc = run(
        ["rustc", f"+{toolchain}", "--version", "--verbose"],
        cwd=ROOT, log_dir=log_dir, label="rustc",
    )
    report: dict[str, object] = {
        "version": version,
        "toolchain": toolchain,
        "rustc": rustc.strip(),
        "offline": True,
        "publication_attempted": False,
        "workspace_lock_sha256": sha256(original_lock),
        "scope": "Packaged SDK source builds with extracted-archive dependency patches",
        "publication_caveat": (
            "Audit archives use local SDK dependency patches in generated lockfiles. "
            "Regenerate publication archives against the registry after publishing dependencies."
        ),
        "archives": [],
        "builds": [],
    }
    # Keep this outside ROOT: unmodified extracted manifests must not inherit the
    # repository workspace, which does not list their temporary directories.
    with tempfile.TemporaryDirectory(prefix="godot-sdk-packages-") as temporary:
        temporary_root = Path(temporary)
        snapshot = temporary_root / "snapshot"
        extracted = temporary_root / "archives"
        snapshot.mkdir()
        extracted.mkdir()
        sdk_workspace = (
            "[workspace]\n"
            f"members = {json.dumps([f'crates/{crate}' for crate in SDK_CRATES])}\n"
            f"resolver = {json.dumps(workspace.get('resolver', '2'))}\n\n"
        )
        manifest, replacements = re.subn(
            r"(?ms)^\[workspace\].*?(?=^\[|\Z)", sdk_workspace, root_manifest, count=1
        )
        if replacements != 1:
            raise RuntimeError("could not isolate the SDK workspace manifest")
        (snapshot / "Cargo.toml").write_text(manifest)
        (snapshot / "Cargo.lock").write_bytes(original_lock)
        listings = {}
        for crate in SDK_CRATES:
            output = run(
                cargo + ["package", "--list", "--locked", "--offline", "--allow-dirty",
                         "-p", crate, "--target-dir", str(target_dir)],
                cwd=ROOT, log_dir=log_dir, label=f"list-{crate}",
            )
            listings[crate] = set(output.splitlines())
            shutil.copytree(ROOT / "crates" / crate, snapshot / "crates" / crate)
        patches = []
        for crate in SDK_CRATES:
            # Cargo may prune/add unused patch entries in the snapshot lockfile.
            # Every packaged registry version/checksum is checked against the
            # untouched repository lockfile before the locked archive builds.
            run(
                cargo + ["package", "--offline", "--no-verify", "-p", crate,
                         "--target-dir", str(target_dir)] + patches,
                cwd=snapshot, log_dir=log_dir, label=f"package-{crate}",
            )
            archive = target_dir / "package" / f"{crate}-{version}.crate"
            record = verify_archive(
                archive, crate=crate, version=version, expected_files=listings[crate],
                snapshot=snapshot, extracted=extracted, locked_registry=locked_registry,
            )
            report["archives"].append(record)
            if crate in ("godot_api", "godot_macro"):
                patches.extend([
                    "--config",
                    f"patch.crates-io.{crate}.path={json.dumps(str(extracted / f'{crate}-{version}'))}",
                ])
        original_extracted = tree_hashes(extracted)
        api_targets = tomllib.loads((ROOT / "godot-api.toml").read_text())["native_targets"]
        matrix = [("godot_api", "4.4", ["--all-features"]), ("godot_macro", "4.4", [])]
        matrix.extend(("godot_rs", api, []) for api in api_targets)
        for crate, api, features in matrix:
            env = dict(os.environ, GODOT_RS_GODOT=api)
            command = cargo + [
                "build", "--locked", "--offline", "--manifest-path",
                str(extracted / f"{crate}-{version}" / "Cargo.toml"),
                "--target-dir", str(target_dir / "verified-build"),
            ] + features
            if crate == "godot_rs":
                command.extend(patches)
            run(command, cwd=temporary_root, log_dir=log_dir,
                label=f"build-{crate}-godot-{api}", env=env)
            report["builds"].append({"crate": crate, "godot_api": api, "features": features})
        if tree_hashes(extracted) != original_extracted:
            raise RuntimeError("building modified the extracted archive source")
        for record in report["archives"]:
            for relative, expected in record["source_sha256"].items():
                if sha256((ROOT / relative).read_bytes()) != expected:
                    raise RuntimeError(f"source changed during the audit; rerun: {relative}")
    if (ROOT / "Cargo.lock").read_bytes() != original_lock:
        raise RuntimeError("workspace Cargo.lock changed during the audit")
    if (ROOT / "Cargo.toml").read_text() != root_manifest:
        raise RuntimeError("workspace Cargo.toml changed during the audit; rerun")
    report["status"] = "passed"
    report["workspace_lock_unchanged"] = True
    report["extracted_source_unchanged"] = True
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", default="1.85.0")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/release-package-audit")
    parser.add_argument("--log-dir", type=Path, default=ROOT / "artifacts/sdk-package-audit")
    args = parser.parse_args()
    log_dir = args.log_dir.resolve()
    log_dir.mkdir(parents=True, exist_ok=True)
    report_path = log_dir / "sdk-package-audit.json"
    try:
        report = verify(toolchain=args.toolchain, target_dir=args.target_dir.resolve(), log_dir=log_dir)
    except (OSError, ValueError, RuntimeError, tarfile.TarError) as error:
        report_path.write_text(json.dumps({"status": "failed", "error": str(error)}, indent=2) + "\n")
        print(f"SDK package audit failed: {error}")
        return 1
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Verified all three SDK archives; report: {report_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
