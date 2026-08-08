#!/usr/bin/env python3
"""Package and idempotently publish one workspace crate."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
USER_AGENT = "godot-rust-release-workflow (https://github.com/godothub/godot-rust)"
MAX_CRATE_BYTES = 10 * 1024 * 1024


def is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdefABCDEF" for character in value)
    )


def sparse_index_path(crate: str) -> str:
    """Return the crates.io sparse-index path for one canonical crate name."""
    name = crate.lower()
    if len(name) == 1:
        return f"1/{name}"
    if len(name) == 2:
        return f"2/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def request_bytes(url: str, *, missing_is_none: bool = False) -> bytes | None:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return response.read()
    except urllib.error.HTTPError as error:
        if missing_is_none and error.code == 404:
            return None
        raise RuntimeError(f"registry request failed with HTTP {error.code}: {url}") from error
    except OSError as error:
        raise RuntimeError(f"registry request failed: {url}: {error}") from error


def api_checksum(crate: str, version: str) -> str | None:
    name = urllib.parse.quote(crate, safe="")
    target = urllib.parse.quote(version, safe="")
    body = request_bytes(
        f"https://crates.io/api/v1/crates/{name}/{target}",
        missing_is_none=True,
    )
    if body is None:
        return None
    try:
        value = json.loads(body)
        published = value["version"]
        if published["num"] != version:
            raise ValueError("version number does not match the request")
        checksum = published["checksum"]
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise RuntimeError("crates.io returned an invalid version record") from error
    if not is_sha256(checksum):
        raise RuntimeError("crates.io returned an invalid crate checksum")
    return checksum.lower()


def parse_sparse_checksum(body: bytes, version: str) -> str | None:
    try:
        lines = body.decode("utf-8").splitlines()
        records = [json.loads(line) for line in lines]
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError("crates.io sparse index returned invalid JSON") from error
    for record in records:
        if not isinstance(record, dict):
            raise RuntimeError("crates.io sparse index returned a non-object record")
        if record.get("vers") != version:
            continue
        checksum = record.get("cksum")
        if not is_sha256(checksum):
            raise RuntimeError("crates.io sparse index returned an invalid checksum")
        return checksum.lower()
    return None


def sparse_checksum(crate: str, version: str) -> str | None:
    body = request_bytes(
        f"https://index.crates.io/{sparse_index_path(crate)}",
        missing_is_none=True,
    )
    return None if body is None else parse_sparse_checksum(body, version)


def workspace_package_version(crate: str) -> str:
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"cargo metadata failed: {completed.stderr.strip()}")
    try:
        packages = json.loads(completed.stdout)["packages"]
        matches = [package for package in packages if package["name"] == crate]
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise RuntimeError("cargo metadata returned an invalid package list") from error
    if len(matches) != 1:
        raise RuntimeError(f"workspace must contain exactly one `{crate}` package")
    return matches[0]["version"]


def package_archive(crate: str, version: str) -> tuple[Path, str]:
    completed = subprocess.run(
        ["cargo", "package", "--locked", "-p", crate],
        cwd=ROOT,
        check=False,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"cargo package failed for `{crate}`")
    archive = ROOT / "target" / "package" / f"{crate}-{version}.crate"
    if not archive.is_file():
        raise RuntimeError(f"cargo did not create expected archive: {archive}")
    size = archive.stat().st_size
    if size > MAX_CRATE_BYTES:
        raise RuntimeError(
            f"package archive for `{crate}` is {size} bytes, exceeding the "
            f"{MAX_CRATE_BYTES}-byte release limit"
        )
    return archive, hashlib.sha256(archive.read_bytes()).hexdigest()


def registry_checksums(crate: str, version: str) -> tuple[str | None, str | None]:
    return api_checksum(crate, version), sparse_checksum(crate, version)


def require_matching_checksum(
    crate: str,
    version: str,
    expected: str,
    checksums: tuple[str | None, str | None],
) -> None:
    for source, actual in zip(("crates.io API", "sparse index"), checksums):
        if actual is not None and actual != expected:
            raise RuntimeError(
                f"{source} already contains `{crate} {version}` with checksum "
                f"{actual}, but the local package checksum is {expected}"
            )


def wait_for_publication(
    crate: str,
    version: str,
    checksum: str,
    attempts: int = 30,
) -> None:
    last = (None, None)
    for attempt in range(attempts):
        last = registry_checksums(crate, version)
        require_matching_checksum(crate, version, checksum, last)
        if last == (checksum, checksum):
            return
        if attempt + 1 < attempts:
            time.sleep(10)
    raise RuntimeError(
        f"`{crate} {version}` did not propagate to both crates.io registry "
        f"views; observed checksums: API={last[0]}, sparse={last[1]}"
    )


def publish(crate: str, version: str) -> None:
    actual_version = workspace_package_version(crate)
    if actual_version != version:
        raise RuntimeError(
            f"`{crate}` has version {actual_version}, expected release {version}"
        )
    archive, checksum = package_archive(crate, version)
    print(f"verified {archive.relative_to(ROOT)} sha256={checksum}")

    existing = registry_checksums(crate, version)
    require_matching_checksum(crate, version, checksum, existing)
    if existing == (checksum, checksum):
        print(f"`{crate} {version}` is already published with identical content")
        return
    if any(value is not None for value in existing):
        print(f"`{crate} {version}` is partially propagated; waiting")
        wait_for_publication(crate, version, checksum)
        return

    completed = subprocess.run(
        ["cargo", "publish", "--locked", "-p", crate],
        cwd=ROOT,
        check=False,
    )
    if completed.returncode != 0:
        # A timed-out upload can still have reached crates.io. Re-read both
        # registry views before deciding whether this release is broken.
        observed = registry_checksums(crate, version)
        require_matching_checksum(crate, version, checksum, observed)
        if not any(value is not None for value in observed):
            raise RuntimeError(f"cargo publish failed for `{crate}`")
    wait_for_publication(crate, version, checksum)
    print(f"published `{crate} {version}` with verified registry checksum")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("crate")
    args = parser.parse_args()
    publish(args.crate, args.version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
