#!/usr/bin/env python3
"""Exercise Extension Mode instance recreation in a live Godot editor."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

from test_native_load import (
    SUPPORTED_APIS,
    build_native_fixture,
    discover_godot,
    require_dict,
    require_string,
    stage_fixture,
)


HOT_RELOAD_MARKER = "GODOT_RS_NATIVE_HOT_RELOAD_OK"
EDITOR_STARTUP_TIMEOUT_SECONDS = 300
EDITOR_READY_TIMEOUT_SECONDS = 180


def wait_for_file(
    process: subprocess.Popen[str],
    path: Path,
    timeout: float,
    output_path: Path,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            return
        status = process.poll()
        if status is not None:
            raise RuntimeError(
                "Godot exited before the Native reload fixture was ready "
                f"(status {status}):\n{read_process_output(output_path)}"
            )
        time.sleep(0.05)
    process.kill()
    process.wait()
    raise RuntimeError(
        f"Godot did not create {path.name} before timeout:\n"
        f"{read_process_output(output_path)}"
    )


def read_process_output(path: Path) -> str:
    """Read diagnostics without making the live editor depend on a pipe."""
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        return f"<could not read Godot output: {error}>"


def prime_editor_project(
    godot: str, project: Path, environment: dict[str, str]
) -> None:
    """Finish the initial import before enabling the live reload plugin."""
    completed = subprocess.run(
        [
            godot,
            "--headless",
            "--editor",
            "--path",
            str(project),
            "--quit-after",
            "2",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=environment,
        check=False,
        timeout=120,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "Godot could not prepare the Native hot-reload project:\n"
            f"{completed.stdout}"
        )


def update_generation(source: Path) -> None:
    contents = source.read_text(encoding="utf-8")
    old = 'const NATIVE_GENERATION: &str = "before";'
    new = 'const NATIVE_GENERATION: &str = "after";'
    if contents.count(old) != 1:
        raise RuntimeError("Native fixture generation marker is missing or ambiguous")
    source.write_text(contents.replace(old, new), encoding="utf-8")


def exclude_cargo_output_from_godot(project: Path) -> None:
    """Prevent editor import scans from traversing the Rust build graph."""
    target_directory = project / "target"
    if target_directory.is_dir():
        (target_directory / ".gdignore").touch()


def stage_hot_reload_plugin(project: Path) -> None:
    """Stage the reload scenario before the editor's initial project import."""
    plugin = project / "addons" / "native_hot_reload"
    plugin.mkdir(parents=True)
    source = project / "hot_reload.gd"
    source.replace(plugin / "hot_reload.gd")
    (plugin / "plugin.cfg").write_text(
        """\
[plugin]

name="Godot Rust Native Hot Reload Test"
description="Exercises Extension Mode reload in a live editor."
author="GodotHub"
version="1.0"
script="hot_reload.gd"
""",
        encoding="utf-8",
    )


def enable_hot_reload_plugin(project: Path) -> None:
    """Enable the test plugin after Godot has imported the clean project."""
    project_settings = project / "project.godot"
    contents = project_settings.read_text(encoding="utf-8")
    if "[editor_plugins]" in contents:
        raise RuntimeError("Native hot-reload project already enables editor plugins")
    project_settings.write_text(
        contents
        + '\n[editor_plugins]\n\nenabled=PackedStringArray("native_hot_reload")\n',
        encoding="utf-8",
    )


def resolve_publication(
    project: Path, publication: dict[str, object], command: str
) -> None:
    request = json.dumps(
        {
            "command": command,
            "id": 2,
            "root": str(project),
            "platform_selector": require_string(
                publication, "platform_selector"
            ),
            "sha256": require_string(publication, "sha256"),
        }
    )
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "-p",
            "godot_build",
            "--",
            "--request",
            request,
        ],
        cwd=Path(__file__).resolve().parents[1],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "Native publication resolution process failed: "
            f"{completed.stderr.strip() or completed.stdout.strip()}"
        )
    response = json.loads(completed.stdout)
    if not response.get("ok"):
        raise RuntimeError(
            "Native publication resolution failed: "
            f"{response.get('error', 'unknown error')}"
        )


def run_native_hot_reload(godot: str, api: str) -> str:
    with tempfile.TemporaryDirectory(
        prefix=f"godot-rust-native-hot-reload-{api}-"
    ) as temporary:
        temporary_root = Path(temporary)
        project = temporary_root / "project"
        project.mkdir()
        stage_fixture(project, api)
        stage_hot_reload_plugin(project)
        environment = {
            **os.environ,
            "GODOT_SILENCE_ROOT_WARNING": "1",
            "CARGO_TARGET_DIR": str(temporary_root / "cargo-target"),
        }
        (project / "target").mkdir()
        exclude_cargo_output_from_godot(project)
        prime_editor_project(godot, project, environment)
        enable_hot_reload_plugin(project)

        output_path = temporary_root / "godot-editor.log"
        with output_path.open("w", encoding="utf-8") as output_file:
            process = subprocess.Popen(
                [
                    godot,
                    "--headless",
                    "--editor",
                    "--verbose",
                    "--path",
                    str(project),
                ],
                stdout=output_file,
                stderr=subprocess.STDOUT,
                text=True,
                env=environment,
            )
            try:
                wait_for_file(
                    process,
                    project / "native-hot-reload.editor-ready",
                    EDITOR_STARTUP_TIMEOUT_SECONDS,
                    output_path,
                )
                first_report = build_native_fixture(project, api)
                (project / "native-hot-reload.load").write_text(
                    "load\n", encoding="utf-8"
                )
                wait_for_file(
                    process,
                    project / "native-hot-reload.ready",
                    EDITOR_READY_TIMEOUT_SECONDS,
                    output_path,
                )
                resolve_publication(
                    project,
                    require_dict(first_report, "publication"),
                    "native_commit",
                )
                update_generation(project / "src" / "lib.rs")
                second_report = build_native_fixture(project, api)
                (project / "native-hot-reload.trigger").write_text(
                    "reload\n", encoding="utf-8"
                )
                process.wait(timeout=120)
            except BaseException:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                raise

        output = read_process_output(output_path)
        if process.returncode != 0:
            raise RuntimeError(
                "Godot Native hot-reload fixture failed with status "
                f"{process.returncode}:\n{output}"
            )
        if HOT_RELOAD_MARKER not in output:
            raise RuntimeError(
                f"Godot did not report successful Native hot reload:\n{output}"
            )
        resolve_publication(
            project,
            require_dict(second_report, "publication"),
            "native_commit",
        )
        return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--godot")
    parser.add_argument("--api", choices=SUPPORTED_APIS, default="4.4")
    arguments = parser.parse_args()
    godot = discover_godot(arguments.godot)
    if godot is None:
        raise RuntimeError("Godot executable was not found")
    run_native_hot_reload(godot, arguments.api)
    print("Godot Rust Native hot-reload integration passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
