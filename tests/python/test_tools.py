from __future__ import annotations

import errno
import importlib.util
import hashlib
import json
import platform
import plistlib
import random
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"
sys.path.insert(0, str(TOOLS))


def load_tool(name: str):
    spec = importlib.util.spec_from_file_location(name, TOOLS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


check_dependency_provenance = load_tool("check_dependency_provenance")
create_apple_xcframework = load_tool("create_apple_xcframework")
assemble_release = load_tool("assemble_release")
verify_release_package = load_tool("verify_release_package")
verify_official_api = load_tool("verify_official_api")
stage_host = load_tool("stage_host")
stage_macos_host = load_tool("stage_macos_host")
stage_example_plugin = load_tool("stage_example_plugin")
stage_project_module = load_tool("stage_project_module")
plugin_platform = load_tool("plugin_platform")
validate_release = load_tool("validate_release")
test_godot_load = load_tool("test_godot_load")
test_editor_plugin = load_tool("test_editor_plugin")
test_godot_export = load_tool("test_godot_export")
test_platform_export = load_tool("test_platform_export")
test_android_runtime = load_tool("test_android_runtime")
test_native_load = load_tool("test_native_load")
test_native_hot_reload = load_tool("test_native_hot_reload")
test_native_desktop_export = load_tool("test_native_desktop_export")
test_native_platform_export = load_tool("test_native_platform_export")
publish_crate = load_tool("publish_crate")
WORKSPACE_VERSION = validate_release.workspace_version(ROOT / "Cargo.toml")


def write_release_binary_matrix(root: Path) -> None:
    """Create structurally valid placeholder payloads for release-tool tests."""
    for platform_name, names in assemble_release.EXPECTED_BINARIES.items():
        platform_directory = root / platform_name
        platform_directory.mkdir(parents=True)
        for name in names:
            output = platform_directory / name
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(f"{platform_name}/{name}".encode())

    xcframework = root / "apple" / "godot_host.xcframework"
    libraries = []
    for identifier, expected in (
        create_apple_xcframework.EXPECTED_LIBRARIES.items()
    ):
        library = {
            "BinaryPath": "godot_host.framework/godot_host",
            "LibraryIdentifier": identifier,
            "LibraryPath": "godot_host.framework",
            "SupportedArchitectures": sorted(expected["architectures"]),
            "SupportedPlatform": expected["platform"],
        }
        if expected["variant"] is not None:
            library["SupportedPlatformVariant"] = expected["variant"]
        libraries.append(library)
        platform_name = (
            "ios-simulator"
            if expected["variant"] == "simulator"
            else expected["platform"]
        )
        framework_info = (
            xcframework
            / identifier
            / "godot_host.framework"
            / "Info.plist"
        )
        with framework_info.open("wb") as output:
            plistlib.dump(
                create_apple_xcframework.framework_plist(
                    platform=platform_name,
                    version=WORKSPACE_VERSION,
                ),
                output,
            )
    with (xcframework / "Info.plist").open("wb") as output:
        plistlib.dump(
            {
                "AvailableLibraries": libraries,
                "CFBundlePackageType": "XFWK",
                "XCFrameworkFormatVersion": "1.0",
            },
            output,
        )


class AppleXCFrameworkTests(unittest.TestCase):
    def test_release_xcframework_requires_all_ios_slices(self):
        with tempfile.TemporaryDirectory() as directory:
            binaries = Path(directory) / "binaries"
            write_release_binary_matrix(binaries)
            xcframework = binaries / "apple" / "godot_host.xcframework"
            create_apple_xcframework.normalize_xcframework_metadata(xcframework)
            with (xcframework / "Info.plist").open("rb") as source:
                metadata = plistlib.load(source)
            identifiers = [
                library["LibraryIdentifier"]
                for library in metadata["AvailableLibraries"]
            ]
            self.assertEqual(identifiers, sorted(identifiers))
            create_apple_xcframework.validate_xcframework(xcframework)
            unexpected = xcframework / "unexpected.txt"
            unexpected.write_text("not part of the signed payload", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "payload does not match"):
                create_apple_xcframework.validate_xcframework(xcframework)


class DependencyProvenanceTests(unittest.TestCase):
    def test_clean_metadata_is_allowed(self):
        metadata = {
            "packages": [
                {
                    "name": "godot_rs",
                    "repository": "https://example.invalid/godot-rust",
                }
            ]
        }
        self.assertEqual(check_dependency_provenance.find_violations(metadata), [])

    def test_denied_godot_interface_package_is_reported(self):
        metadata = {
            "packages": [
                {
                    "name": "godot-core",
                    "repository": "https://example.invalid/godot-interface",
                }
            ]
        }
        violations = check_dependency_provenance.find_violations(metadata)
        self.assertEqual(violations, ["denied Cargo package: godot-core"])


class CratePublicationTests(unittest.TestCase):
    def test_public_crates_have_package_readmes_and_the_project_license(self):
        project_license = (ROOT / "LICENSE").read_bytes()
        for crate in ("godot_rs", "godot_api", "godot_macro"):
            root = ROOT / "crates" / crate
            self.assertTrue((root / "README.md").read_text(encoding="utf-8").strip())
            self.assertEqual((root / "LICENSE").read_bytes(), project_license)

    def test_sparse_index_paths_match_the_cargo_registry_layout(self):
        self.assertEqual(publish_crate.sparse_index_path("a"), "1/a")
        self.assertEqual(publish_crate.sparse_index_path("ab"), "2/ab")
        self.assertEqual(publish_crate.sparse_index_path("abc"), "3/a/abc")
        self.assertEqual(
            publish_crate.sparse_index_path("godot_rs"),
            "go/do/godot_rs",
        )

    def test_sparse_index_selects_the_exact_version_checksum(self):
        checksum = "a" * 64
        body = (
            json.dumps({"name": "godot_rs", "vers": "0.8.0", "cksum": "b" * 64})
            + "\n"
            + json.dumps(
                {"name": "godot_rs", "vers": "0.9.0", "cksum": checksum}
            )
            + "\n"
        ).encode()
        self.assertEqual(
            publish_crate.parse_sparse_checksum(body, "0.9.0"),
            checksum,
        )

    def test_sparse_index_rejects_non_hex_checksums(self):
        body = (
            json.dumps(
                {"name": "godot_rs", "vers": "0.9.0", "cksum": "z" * 64}
            )
            + "\n"
        ).encode()
        with self.assertRaisesRegex(RuntimeError, "invalid checksum"):
            publish_crate.parse_sparse_checksum(body, "0.9.0")

    def test_registry_checksum_mismatch_fails_closed(self):
        with self.assertRaisesRegex(RuntimeError, "local package checksum"):
            publish_crate.require_matching_checksum(
                "godot_rs",
                "0.9.0",
                "a" * 64,
                ("b" * 64, "a" * 64),
            )


class OfficialApiTests(unittest.TestCase):
    def test_authenticated_api_inputs_keep_lf_on_every_platform(self):
        attributes = (ROOT / ".gitattributes").read_text(encoding="utf-8")
        self.assertIn(
            "crates/godot_api/metadata/**/extension_api.json text eol=lf",
            attributes.splitlines(),
        )

    def test_sha256_is_lowercase_and_stable(self):
        self.assertEqual(
            verify_official_api.sha256_bytes(b"godot-rust"),
            "118f6140eb0a8bc2b02e6fa16c40d4dd246f0d51b5b6aefca90579bfb2d8001a",
        )

    def test_complete_source_matrix_is_discovered_in_order(self):
        manifest = verify_official_api.load_toml(
            ROOT / "godot-api.toml"
        )
        self.assertEqual(
            verify_official_api.source_targets(manifest),
            ("4.4", "4.5", "4.6", "4.7"),
        )

    def test_all_source_manifests_satisfy_provenance_policy(self):
        manifest = verify_official_api.load_toml(
            ROOT / "godot-api.toml"
        )
        for target in verify_official_api.source_targets(manifest):
            self.assertEqual(
                verify_official_api.validate_manifest(
                    manifest,
                    target,
                ),
                [],
            )


class RepositoryMarkerTests(unittest.TestCase):
    def test_project_configuration_is_tracked_while_local_products_are_ignored(self):
        def is_ignored(path: str) -> bool:
            completed = subprocess.run(
                ["git", "check-ignore", "--no-index", "--quiet", path],
                cwd=ROOT,
                check=False,
            )
            self.assertIn(completed.returncode, (0, 1))
            return completed.returncode == 0

        self.assertFalse(is_ignored("example/.cargo/config.toml"))
        self.assertFalse(
            is_ignored("example/addons/godot-rust/bin/.godothub")
        )
        for path in (
            "example/.cargo/credentials.toml",
            "crates/godot_rs/target/debug/module",
            "example/.godot/editor/filesystem_cache8",
            "tests/fixtures/native_extension/rust/native.gdextension",
            "tests/fixtures/host_load/addons/godot-rust/bin/apple/"
            "godot_host.xcframework/Info.plist",
            "tools/__pycache__/tool.pyc",
            ".idea/workspace.xml",
            ".env.local",
            "coverage/lcov.info",
            "dist/godot-rust.tar.gz",
        ):
            with self.subTest(path=path):
                self.assertTrue(is_ignored(path))

    def test_empty_directories_use_godothub_markers(self):
        self.assertEqual(list(ROOT.rglob(".gitkeep")), [])
        self.assertEqual(
            sorted(
                path.relative_to(ROOT).as_posix()
                for path in ROOT.rglob(".godothub")
                if "target" not in path.parts
            ),
            [
                "example/addons/godot-rust/bin/.godothub",
                "tests/fixtures/host_load/addons/godot-rust/bin/.godothub",
            ],
        )


class StageHostTests(unittest.TestCase):
    def test_workflow_files_have_concrete_responsibilities(self):
        workflows = ROOT / ".github" / "workflows"
        workflow_paths = sorted(workflows.glob("*.yml"))
        node24_official_actions = {
            "actions/attest@508db95dd578ae2727ebd6217d5ba78e4fbda05d",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
            "actions/setup-java@03ad4de0992f5dab5e18fcb136590ce7c4a0ac95",
            "actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97",
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
        }
        self.assertEqual(
            {path.name for path in workflow_paths},
            {
                "cross-platform-tests.yml",
                "godot-api-compatibility.yml",
                "godot-editor-integration.yml",
                "platform-export-validation.yml",
                "publish-release.yml",
                "quality-gates.yml",
                "runtime-stability.yml",
                "security-audit.yml",
            },
        )
        for path in workflow_paths:
            for line in path.read_text(encoding="utf-8").splitlines():
                stripped = line.strip()
                if not stripped.startswith("uses:"):
                    continue
                reference = stripped.removeprefix("uses:").split("#", 1)[0].strip()
                if reference.startswith("./"):
                    continue
                with self.subTest(workflow=path.name, reference=reference):
                    self.assertRegex(reference, r"@[\da-f]{40}$")
                    if reference.startswith("actions/"):
                        self.assertIn(reference, node24_official_actions)

    def test_api_workflow_verifies_release_checksums_and_bundled_metadata(self):
        workflow = (
            ROOT / ".github" / "workflows" / "godot-api-compatibility.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("$2 == archive", workflow)
        self.assertIn("sha512sum --check -", workflow)
        self.assertNotIn('sed "s# ${GODOT_ARCHIVE}$#', workflow)
        self.assertIn("generate-engine-api", workflow)
        self.assertIn("cmp /tmp/engine-api-a.rs /tmp/engine-api-b.rs", workflow)
        self.assertIn(
            "crates/godot_api/metadata/godot-",
            workflow,
        )

    def test_quality_workflow_gates_generated_api_performance(self):
        workflow = (
            ROOT / ".github" / "workflows" / "quality-gates.yml"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "github.com/rhysd/actionlint/cmd/actionlint@v1.7.12",
            workflow,
        )
        self.assertIn(
            "cargo bench --locked",
            workflow,
        )
        self.assertIn(
            "--bench engine_api_hot_paths",
            workflow,
        )
        self.assertIn("for api in 4.4 4.5 4.6 4.7", workflow)

    def test_platform_library_names(self):
        self.assertEqual(
            stage_host.library_name("darwin"), "libgodot_host.dylib"
        )
        self.assertEqual(
            stage_host.library_name("linux"), "libgodot_host.so"
        )
        self.assertEqual(stage_host.library_name("win32"), "godot_host.dll")
        self.assertEqual(
            stage_example_plugin.host_library_name("darwin"),
            "libgodot_host.dylib",
        )
        self.assertEqual(
            stage_example_plugin.executable_name("godot_build", "win32"),
            "godot_build.exe",
        )
        self.assertEqual(
            stage_example_plugin.executable_name("godot_build", "linux"),
            "godot_build",
        )
        self.assertEqual(
            stage_project_module.library_name("darwin"),
            "libgodot_host_smoke_module.dylib",
        )
        self.assertEqual(
            stage_project_module.library_name("linux"),
            "libgodot_host_smoke_module.so",
        )
        self.assertEqual(
            stage_project_module.library_name("win32"),
            "godot_host_smoke_module.dll",
        )
        self.assertEqual(
            plugin_platform.platform_directory("darwin", "arm64"),
            "macos-universal",
        )
        self.assertEqual(
            plugin_platform.platform_directory("linux", "x86_64"),
            "linux-x86_64",
        )
        self.assertEqual(
            plugin_platform.platform_directory("linux", "aarch64"),
            "linux-arm64",
        )
        self.assertEqual(
            plugin_platform.platform_directory("win32", "AMD64"),
            "windows-x86_64",
        )
        with self.assertRaises(ValueError):
            plugin_platform.platform_directory("win32", "ARM64")

    def test_host_fixture_uses_the_canonical_addon_descriptor(self):
        canonical = (
            ROOT
            / "example"
            / "addons"
            / "godot-rust"
            / "godot-rust.gdextension"
        )
        fixture = (
            ROOT
            / "tests"
            / "fixtures"
            / "host_load"
            / "addons"
            / "godot-rust"
            / "godot-rust.gdextension"
        )
        self.assertEqual(
            fixture.read_text(encoding="utf-8"),
            canonical.read_text(encoding="utf-8"),
        )

    def test_canonical_addon_contains_the_editor_plugin_contract(self):
        addon = ROOT / "example" / "addons" / "godot-rust"
        self.assertEqual(
            {
                "GODOT_LICENSE.md",
                "README.md",
                "bin",
                "build_process.gd",
                "build_process.gd.uid",
                "build_protocol.gd",
                "build_protocol.gd.uid",
                "diagnostics_panel.gd",
                "diagnostics_panel.gd.uid",
                "dependency_dialog.gd",
                "dependency_dialog.gd.uid",
                "editor_ui.gd",
                "editor_ui.gd.uid",
                "export_plugin.gd",
                "export_plugin.gd.uid",
                "godot-rust.gdextension",
                "godot-rust.gdextension.uid",
                "plugin.cfg",
                "plugin.gd",
                "plugin.gd.uid",
                "source_monitor.gd",
                "source_monitor.gd.uid",
            },
            {path.name for path in addon.iterdir()},
        )
        self.assertTrue((addon / "bin" / ".godothub").is_file())
        self.assertIn('name="godot-rust"', (addon / "plugin.cfg").read_text())
        descriptor = (addon / "godot-rust.gdextension").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            "bin/macos-universal/libgodot_host.dylib",
            descriptor,
        )
        self.assertIn(
            'ios.release = "res://addons/godot-rust/bin/apple/'
            'godot_host.xcframework"',
            descriptor,
        )

    def test_editor_actions_auto_select_script_or_native_mode(self):
        plugin = (
            ROOT / "example" / "addons" / "godot-rust" / "plugin.gd"
        ).read_text(encoding="utf-8")
        editor_ui = (
            ROOT / "example" / "addons" / "godot-rust" / "editor_ui.gd"
        ).read_text(encoding="utf-8")
        self.assertEqual(plugin.count('"command": "project_cargo"'), 5)
        self.assertEqual(plugin.count('"command": "project_export"'), 1)
        self.assertIn('"runtime_godot": _runtime_godot_api()', plugin)
        self.assertIn('"android_sdk": _android_sdk_path(platform)', plugin)
        self.assertIn('mode == "script"', plugin)
        self.assertIn('mode == "extension"', plugin)
        self.assertIn("add_control_to_bottom_panel(", plugin)
        self.assertIn("show_cargo_report(", plugin)
        self.assertIn('"command": "install_rust_target"', plugin)
        self.assertIn("ZIPPacker.new()", plugin)
        self.assertIn("support_snapshot()", plugin)
        self.assertIn("OS.shell_show_in_file_manager(", plugin)
        self.assertIn("get_editor_scale()", plugin)
        self.assertIn("accessibility_name", editor_ui)
        self.assertIn('"Support Bundle": "支持包"', editor_ui)
        self.assertIn("get_editor_interface().edit_script(", plugin)
        self.assertIn("func _build() -> bool:", plugin)
        self.assertIn('"Automatic Rust build"', plugin)
        self.assertIn("_start_pending_idle_build()", plugin)
        self.assertIn("editor.get(\"auto_build_on_idle\", false)", plugin)
        self.assertIn("_auto_check_enabled or _auto_build_on_idle", plugin)
        self.assertIn("_wait_for_active_request()", plugin)
        self.assertIn("add_export_plugin(", plugin)
        self.assertIn("remove_export_plugin(", plugin)
        self.assertIn("func prepare_rust_export(", plugin)
        self.assertIn('"command": "configure"', plugin)
        self.assertIn("func _offer_script_setup(", plugin)
        self.assertIn("Any conflict stops setup before files change.", plugin)
        self.assertIn('return "macos-universal"', plugin)
        self.assertIn('return "linux-x86_64"', plugin)
        self.assertIn('return "linux-arm64"', plugin)
        self.assertIn('return "windows-x86_64"', plugin)
        process = (
            ROOT / "example" / "addons" / "godot-rust" / "build_process.gd"
        ).read_text(encoding="utf-8")
        protocol = (
            ROOT / "example" / "addons" / "godot-rust" / "build_protocol.gd"
        ).read_text(encoding="utf-8")
        self.assertIn("BuildProtocol.start_request(", process)
        self.assertIn("wait_for_completion(", process)
        self.assertIn("func cancel(", process)
        self.assertIn("request_cancelled.emit(", process)
        self.assertIn("SHUTDOWN_GRACE_MSEC", process)
        self.assertNotIn("Thread.new()", plugin)
        self.assertIn('"command": "build_receipt"', plugin)
        self.assertIn("ERROR_TEXT_LIMIT", protocol)
        self.assertIn("parse_response(", protocol)
        self.assertIn("OS.create_process(", protocol)
        self.assertIn('"--response-file"', protocol)
        self.assertIn('"--cancel-file"', protocol)
        self.assertIn("OUTPUT_LIMIT", protocol)
        export_plugin = (
            ROOT
            / "example"
            / "addons"
            / "godot-rust"
            / "export_plugin.gd"
        ).read_text(encoding="utf-8")
        self.assertIn("extends EditorExportPlugin", export_plugin)
        self.assertIn("func _export_begin(", export_plugin)
        self.assertIn("func _export_file(", export_plugin)
        self.assertIn("add_shared_object(", export_plugin)
        self.assertIn("_stage_web_host(runtime_godot)", export_plugin)
        self.assertIn("_register_native_descriptor(", export_plugin)
        self.assertIn("_ensure_extension_list_entry(", export_plugin)
        self.assertIn("native_descriptor_resource_path", export_plugin)
        self.assertNotIn(
            "ProjectSettings.localize_path(descriptor_path)", export_plugin
        )
        self.assertIn("add_ios_embedded_framework(module_path)", export_plugin)
        self.assertIn("func _export_end()", export_plugin)
        self.assertIn("_repair_godot_44_ios_project(", export_plugin)
        self.assertIn('"OTHER_LDFLAGS[sdk=iphoneos*]"', export_plugin)
        self.assertIn('mode == "extension"', export_plugin)
        self.assertIn("add_file(path, PackedByteArray([10]), false)", export_plugin)
        self.assertIn('path.begins_with("res://target/")', export_plugin)
        self.assertIn(
            'path.begins_with("res://addons/godot-rust/")',
            export_plugin,
        )
        self.assertIn("EditorExportPlatform.EXPORT_MESSAGE_ERROR", export_plugin)
        self.assertIn("export-failed", export_plugin)
        monitor = (
            ROOT / "example" / "addons" / "godot-rust" / "source_monitor.gd"
        ).read_text(encoding="utf-8")
        self.assertIn("DEBOUNCE_SECONDS", monitor)
        self.assertIn("MAX_TRACKED_FILE_BYTES", monitor)
        self.assertIn("MAX_TRACKED_TOTAL_BYTES", monitor)
        self.assertIn('file_name != "mod.rs"', monitor)
        panel = (
            ROOT / "example" / "addons" / "godot-rust" / "diagnostics_panel.gd"
        ).read_text(encoding="utf-8")
        self.assertIn("MAX_DIAGNOSTICS", panel)
        self.assertIn("diagnostic_activated.emit(", panel)
        self.assertIn("suggestion_requested.emit(", panel)
        self.assertIn("undo_requested.emit(", panel)
        self.assertIn('"command": "apply_suggestion"', plugin)
        self.assertIn('"command": "dependency_list"', plugin)
        self.assertIn('"command": "dependency_preview"', plugin)
        self.assertIn('"command": "dependency_apply"', plugin)
        self.assertIn("Rust script index %s with %d script(s)", plugin)
        self.assertIn("GDExtensionManager.reload_extension(", plugin)
        self.assertIn('"native_commit"', plugin)
        self.assertIn('"native_rollback"', plugin)
        self.assertIn("rollback_available", plugin)

    def test_release_assembles_the_canonical_addon_not_a_fixture(self):
        workflow = (
            ROOT / ".github" / "workflows" / "publish-release.yml"
        ).read_text(
            encoding="utf-8"
        )
        self.assertIn("tools/assemble_release.py", workflow)
        self.assertIn("platform: linux-x86_64", workflow)
        self.assertIn("platform: linux-arm64", workflow)
        self.assertIn("name: Build Apple Hosts", workflow)
        self.assertIn("plugin-binaries-apple", workflow)
        self.assertIn("aarch64-apple-ios-sim", workflow)
        self.assertIn("x86_64-apple-ios", workflow)
        self.assertIn("tools/create_apple_xcframework.py", workflow)
        self.assertIn("platform: windows-x86_64", workflow)
        self.assertIn("platform: android-arm64", workflow)
        self.assertIn("name: Build web-godot-${{ matrix.godot }}", workflow)
        self.assertIn('gh release create "${{ inputs.version }}"', workflow)
        self.assertIn('--title "${{ inputs.version }}"', workflow)
        self.assertIn("--notes-file dist/release-notes.md", workflow)
        self.assertIn("assemble-release:\n    name: Assemble release package", workflow)
        self.assertIn("publish-crates:", workflow)
        self.assertIn(
            "for crate in godot_api godot_macro godot_rs; do",
            workflow,
        )
        self.assertIn("CARGO_REGISTRY_TOKEN", workflow)
        self.assertIn("- publish-crates", workflow)
        self.assertLess(
            workflow.index("- name: Verify downloaded release package"),
            workflow.index("- name: Attest release provenance"),
        )
        self.assertIn('RUST_RELEASE_TOOLCHAIN: "1.85.0"', workflow)
        self.assertIn(
            "cargo install cargo-audit --version 0.22.1 --locked",
            workflow,
        )
        self.assertIn("cargo audit", workflow)
        self.assertIn("--source-commit \"${{ github.sha }}\"", workflow)
        self.assertIn("sbom-path:", workflow)
        self.assertIn("--json body", workflow)
        self.assertIn(
            "--published-release-json dist/published-release.json",
            workflow,
        )
        self.assertNotIn('gh release create "v${{ inputs.version }}"', workflow)
        self.assertNotIn("--generate-notes", workflow)
        self.assertNotIn("tar.gz", workflow)
        self.assertNotIn("SHA256SUMS", workflow)
        self.assertTrue(
            (
                ROOT
                / "example"
                / "addons"
                / "godot-rust"
                / "GODOT_LICENSE.md"
            ).is_file()
        )
        self.assertNotIn(
            "tests/fixtures/host_load/addons/godot-rust/godot-rust.gdextension",
            workflow,
        )

    def test_release_requires_real_mobile_and_web_exports(self):
        platform_workflow = (
            ROOT / ".github" / "workflows" / "platform-export-validation.yml"
        ).read_text(encoding="utf-8")
        for value in (
            "runs-on: ubuntu-22.04",
            "tools/test_android_runtime.py",
            "system-images;android-35;google_apis;x86_64",
            "${ANDROID_HOME}/cmdline-tools/latest/bin/sdkmanager",
            '--adb "${ANDROID_HOME}/platform-tools/adb"',
            "--run-web",
            "nightly-2025-02-20",
            "build-std=std,panic_abort",
            "-C symbol-mangling-version=v0",
            "unsupported JavaScript exception bridge",
            "tools/test_godot_export.py",
            "tools/test_native_desktop_export.py",
            "tools/test_platform_export.py",
            "tools/test_native_platform_export.py",
            "godot_host.xcframework",
            'release: "4.5.2"',
            'release: "4.6.3"',
            'release: "4.7.1"',
            '--godot-api "${{ matrix.api }}"',
            "--build-ios",
            "--run-ios",
            '"arm32 armv7-linux-androideabi"',
            '"arm64 aarch64-linux-android"',
            '"x86_32 i686-linux-android"',
            '"x86_64-linux-android x86_64-linux-android"',
            "artifacts/native-android",
            'artifacts/native-web-${{ matrix.api }}',
            "artifacts/native-ios",
        ):
            with self.subTest(value=value):
                self.assertIn(value, platform_workflow)
        self.assertNotIn("--version 0.9.0", platform_workflow)
        release_workflow = (
            ROOT / ".github" / "workflows" / "publish-release.yml"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUSTFLAGS",
            release_workflow,
        )
        self.assertIn("-C symbol-mangling-version=v0", release_workflow)
        self.assertNotIn("export RUSTFLAGS=", release_workflow)
        stability_workflow = (
            ROOT / ".github" / "workflows" / "runtime-stability.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("--component rust-src", stability_workflow)
        self.assertIn("test -Z build-std --locked", stability_workflow)
        self.assertIn("fetch --locked", stability_workflow)
        platform_tool = (
            ROOT / "tools" / "test_platform_export.py"
        ).read_text(encoding="utf-8")
        self.assertIn("CodeSignOnCopy", platform_tool)
        self.assertIn(
            "textures/vram_compression/import_etc2_astc=true",
            platform_tool,
        )
        self.assertIn("configure_android_editor(", platform_tool)
        self.assertIn("_sc_", platform_tool)
        self.assertIn("build_ios_xcode_targets(", platform_tool)
        self.assertIn("godot_rs_project_module.xcframework", platform_tool)
        self.assertTrue(
            (
                ROOT
                / "tests/fixtures/android_settings/addons/android-settings/plugin.gd"
            ).is_file()
        )
        desktop_tool = (
            ROOT / "tools" / "test_godot_export.py"
        ).read_text(encoding="utf-8")
        self.assertIn('binary_format/architecture="universal"', desktop_tool)
        self.assertIn(
            "textures/vram_compression/import_etc2_astc=true",
            desktop_tool,
        )
        native_desktop_tool = (
            ROOT / "tools/test_native_desktop_export.py"
        ).read_text(encoding="utf-8")
        self.assertIn("mode=extension", native_desktop_tool)
        self.assertIn("verify_native_library(", native_desktop_tool)
        self.assertIn("STOPPED_MARKER", native_desktop_tool)
        self.assertIn(
            'architectures != {"arm64", "x86_64"}',
            native_desktop_tool,
        )
        native_platform_tool = (
            ROOT / "tools/test_native_platform_export.py"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "allowed_post_success_page_errors=",
            native_platform_tool,
        )
        self.assertIn(
            "is not of type 'BaseAudioContext'.",
            native_platform_tool,
        )
        self.assertIn(
            "if not missing:",
            platform_tool,
        )

        release_workflow = (
            ROOT / ".github" / "workflows" / "publish-release.yml"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "platform-export-gates:\n"
            "    name: Mobile and Web export gates\n"
            "    needs: validate\n"
            "    uses: ./.github/workflows/platform-export-validation.yml",
            release_workflow,
        )
        self.assertIn("      - platform-export-gates", release_workflow)
        self.assertIn(
            "stability-gates:\n"
            "    name: Runtime stability gates\n"
            "    needs: validate\n"
            "    uses: ./.github/workflows/runtime-stability.yml",
            release_workflow,
        )
        self.assertIn("      - stability-gates", release_workflow)
        for value in (
            "WEB_RUST_TOOLCHAIN: nightly-2025-02-20",
            "build-std=std,panic_abort",
            "unsupported JavaScript exception bridge",
        ):
            with self.subTest(value=value):
                self.assertIn(value, release_workflow)

    @unittest.skipIf(
        sys.platform == "win32",
        "symbolic-link creation is not guaranteed on Windows runners",
    )
    def test_android_toolchains_accept_only_contained_executable_links(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            sdk = temporary / "jdk"
            executable = sdk / "runtime/bin/java"
            executable.parent.mkdir(parents=True)
            executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            executable.chmod(0o755)
            link = sdk / "bin/java"
            link.parent.mkdir()
            link.symlink_to(Path("../runtime/bin/java"))

            resolved_root, resolved_executable = (
                test_platform_export.resolve_toolchain_executable(
                    sdk,
                    Path("bin/java"),
                    "Java SDK",
                )
            )
            self.assertEqual(resolved_root, sdk.resolve())
            self.assertEqual(resolved_executable, executable.resolve())

            escaped = temporary / "outside-java"
            escaped.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            escaped.chmod(0o755)
            link.unlink()
            link.symlink_to(escaped)
            with self.assertRaisesRegex(ValueError, "contained toolchain"):
                test_platform_export.resolve_toolchain_executable(
                    sdk,
                    Path("bin/java"),
                    "Java SDK",
                )

    def test_official_editor_workflow_covers_three_desktop_platforms(self):
        workflow = (
            ROOT / ".github" / "workflows" / "godot-editor-integration.yml"
        ).read_text(encoding="utf-8")
        for value in (
            "os: ubuntu-22.04",
            "os: macos-latest",
            "os: windows-latest",
            "Godot_v4.4.1-stable_linux.x86_64.zip",
            "Godot_v4.4.1-stable_macos.universal.zip",
            "Godot_v4.4.1-stable_win64.exe.zip",
            "actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97",
            "tools/test_godot_export.py",
        ):
            with self.subTest(value=value):
                self.assertIn(value, workflow)
        self.assertIn("workflow_call:", workflow)
        self.assertIn("GODOT_SHA512", workflow)

        release_workflow = (
            ROOT / ".github" / "workflows" / "publish-release.yml"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "functional-gates:\n"
            "    name: Functional editor gates\n"
            "    needs: validate\n"
            "    uses: ./.github/workflows/godot-editor-integration.yml",
            release_workflow,
        )
        self.assertIn(
            "assemble-release:\n"
            "    name: Assemble release package\n"
            "    needs:\n"
            "      - functional-gates",
            release_workflow,
        )

    def test_godot_load_rejects_engine_errors(self):
        output = "\x1b[1;31mERROR:\x1b[0m extension registration failed"
        self.assertTrue(
            any(pattern.search(output) for pattern in test_godot_load.ERROR_PATTERNS)
        )

    def test_godot_load_requires_project_callback_marker(self):
        self.assertEqual(
            test_godot_load.output_errors(
                "\n".join(test_godot_load.REQUIRED_MODULE_MARKERS)
            ),
            [],
        )
        self.assertIn(
            f"missing marker {test_godot_load.MODULE_READY_MARKER}",
            test_godot_load.output_errors(
                "Godot exited without running Rust"
            ),
        )

    def test_godot_load_validates_expected_instance_failures(self):
        output = "\n".join(
            (
                test_godot_load.FAULT_ISOLATION_MARKER,
                *(
                    [test_godot_load.EXPECTED_FAILURE_CALLBACK_MARKER]
                    * 3
                ),
                test_godot_load.EXPECTED_PANIC_CALLBACK_MARKER,
                *(
                    [
                        "SCRIPT ERROR: CallbackFailed: intentional callback failure"
                    ]
                    * 2
                ),
                "SCRIPT ERROR: Panic: Rust script callback panicked",
                "at: failure_for_test (res://sample.rs:0)",
                "at: failure_for_test (res://sample.rs:0)",
                "at: panic_for_test (res://sample.rs:0)",
                "This script instance was disabled",
                "This script instance was disabled",
                "res://sample.rs",
            )
        )
        self.assertEqual(
            test_godot_load.fault_isolation_errors(0, output),
            [],
        )
        colored_output = output.replace(
            "SCRIPT ERROR:",
            "\x1b[1;35mSCRIPT ERROR:\x1b[0;95m",
        ).replace(
            "at:",
            "\x1b[0;90mat:\x1b[0m",
        )
        self.assertEqual(
            test_godot_load.fault_isolation_errors(0, colored_output),
            [],
        )
        self.assertIn(
            "expected 3 occurrence(s)",
            test_godot_load.fault_isolation_errors(
                0,
                output.replace(
                    test_godot_load.EXPECTED_FAILURE_CALLBACK_MARKER,
                    "",
                    1,
                ),
            )[0],
        )

    def test_godot_load_stages_a_clean_isolated_project(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            host_library = root / stage_host.library_name(sys.platform)
            host_library.write_bytes(b"host")
            if sys.platform == "darwin":
                def fake_stage(_source: Path, addon_bin: Path) -> Path:
                    executable = addon_bin / stage_macos_host.LOCAL_HOST
                    executable.parent.mkdir(parents=True)
                    executable.write_bytes(b"host")
                    return executable

                with mock.patch.object(
                    test_godot_load,
                    "stage_macos_host",
                    side_effect=fake_stage,
                ):
                    project = test_godot_load.stage_clean_project(
                        root / "project",
                        host_library,
                    )
            else:
                project = test_godot_load.stage_clean_project(
                    root / "project",
                    host_library,
                )

            self.assertFalse((project / ".godot").exists())
            self.assertTrue((project / "project.godot").is_file())
            self.assertTrue((project / "sample.rs").is_file())
            self.assertTrue((project / "fault_isolation.gd").is_file())
            addon = project / "addons" / "godot-rust"
            self.assertEqual(
                (addon / "godot-rust.gdextension").read_text(
                    encoding="utf-8"
                ),
                (
                    ROOT
                    / "example"
                    / "addons"
                    / "godot-rust"
                    / "godot-rust.gdextension"
                ).read_text(encoding="utf-8"),
            )
            if sys.platform == "darwin":
                staged_host = addon / "bin" / stage_macos_host.LOCAL_HOST
            else:
                staged_host = (
                    addon
                    / "bin"
                    / plugin_platform.platform_directory(
                        sys.platform,
                        platform.machine(),
                    )
                    / host_library.name
                )
            self.assertEqual(staged_host.read_bytes(), b"host")

    def test_editor_plugin_load_rejects_parse_errors(self):
        self.assertEqual(
            test_editor_plugin.output_errors(
                "\n".join(test_editor_plugin.REQUIRED_MARKERS)
            ),
            [],
        )
        self.assertTrue(
            test_editor_plugin.output_errors(
                "SCRIPT ERROR: Parse Error: broken plugin"
            )
        )

    def test_export_output_requires_build_and_runtime_evidence(self):
        source = (ROOT / "tools" / "test_godot_export.py").read_text(
            encoding="utf-8"
        )
        self.assertIn('encoding="utf-8"', source)
        export_plugin = (
            ROOT / "example" / "addons" / "godot-rust" / "export_plugin.gd"
        ).read_text(encoding="utf-8")
        self.assertIn(test_godot_export.EXPORT_MARKER, export_plugin)
        self.assertIn("application/modify_resources=false", source)
        for leak_marker in (
            "ObjectDB instances leaked at exit",
            "resources still in use at exit",
        ):
            with self.assertRaises(RuntimeError):
                test_godot_export.verify_output(
                    f"{test_godot_export.EXPORT_MARKER}\n{leak_marker}",
                    test_godot_export.EXPORT_MARKER,
                )
        test_godot_export.verify_output(
            test_godot_export.EXPORT_MARKER,
            test_godot_export.EXPORT_MARKER,
        )
        with self.assertRaises(RuntimeError):
            test_godot_export.verify_output(
                "SCRIPT ERROR: exported script failed",
                test_godot_export.EXPORT_MARKER,
            )
        with self.assertRaises(RuntimeError):
            test_godot_export.verify_output(
                f"{test_godot_export.EXPORT_MARKER}\nERROR: export failed",
                test_godot_export.EXPORT_MARKER,
            )

    def test_editor_binary_exports_use_matching_debug_feature_tags(self):
        for system, expected_preset in (
            ("linux", "Linux"),
            ("win32", "Windows Desktop"),
        ):
            with self.subTest(system=system), mock.patch.object(
                sys, "platform", system
            ):
                preset, _, command, options, _ = (
                    test_godot_export.export_configuration("godot")
                )
            self.assertEqual(preset, expected_preset)
            self.assertEqual(command, "--export-debug")
            self.assertIn("custom_template/debug=", options)
            self.assertNotIn("custom_template/release=", options)

    def test_mobile_and_web_export_presets_stage_exact_hosts(self):
        self.assertIn(
            'config/icon="res://icon.svg"',
            (ROOT / "example" / "project.godot").read_text(encoding="utf-8"),
        )
        self.assertTrue((ROOT / "example" / "icon.svg").is_file())
        with tempfile.TemporaryDirectory() as configuration_directory:
            configuration_root = Path(configuration_directory)
            source_template = configuration_root / "android_source.zip"
            source_template.write_bytes(b"source template")
            keystore = configuration_root / "debug.keystore"
            keystore.write_bytes(b"keystore")
            _, export_command, options = (
                test_platform_export.platform_configuration(
                    "Android",
                    "x86_64",
                    source_template,
                    keystore,
                )
            )
            self.assertEqual(export_command, "--export-debug")
            self.assertIn("gradle_build/use_gradle_build=true", options)
            self.assertIn(
                "gradle_build/android_source_template=",
                options,
            )
            self.assertNotIn("custom_template/debug=", options)
            android_abis = {
                "arm32": "armeabi-v7a",
                "arm64": "arm64-v8a",
                "x86_32": "x86",
                "x86_64": "x86_64",
            }
            for architecture, abi in android_abis.items():
                with self.subTest(architecture=architecture):
                    _, _, architecture_options = (
                        test_platform_export.platform_configuration(
                            "Android",
                            architecture,
                            source_template,
                            keystore,
                        )
                    )
                    for candidate in android_abis.values():
                        self.assertIn(
                            f"architectures/{candidate}="
                            f"{str(candidate == abi).lower()}",
                            architecture_options,
                        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            addon = root / "addons/godot-rust"
            addon.mkdir(parents=True)
            web_host = root / "host.wasm"
            web_host.write_bytes(b"web host")
            test_platform_export.stage_target_host(
                addon,
                "Web",
                "wasm32",
                web_host,
            )
            self.assertEqual(
                (
                    addon
                    / "bin/web-godot-4.4/godot_host.wasm"
                ).read_bytes(),
                b"web host",
            )
            test_platform_export.stage_target_host(
                addon,
                "Web",
                "wasm32",
                web_host,
                "4.7",
            )
            self.assertEqual(
                (
                    addon
                    / "bin/web-godot-4.7/godot_host.wasm"
                ).read_bytes(),
                b"web host",
            )

            android_host = root / "host.so"
            android_host.write_bytes(b"android host")
            test_platform_export.stage_target_host(
                addon,
                "Android",
                "x86_64",
                android_host,
            )
            self.assertEqual(
                (
                    addon
                    / "bin/android-x86_64/libgodot_host.so"
                ).read_bytes(),
                b"android host",
            )

            binaries = root / "binaries"
            write_release_binary_matrix(binaries)
            xcframework = (
                binaries / "apple" / "godot_host.xcframework"
            )
            test_platform_export.stage_target_host(
                addon,
                "iOS",
                "arm64",
                xcframework,
            )
            self.assertTrue(
                (
                    addon
                    / "bin/apple/godot_host.xcframework/Info.plist"
                ).is_file()
            )

    def test_ios_simulator_runtime_versions_sort_numerically(self):
        version = test_platform_export.ios_simulator_runtime_version
        self.assertGreater(
            version("com.apple.CoreSimulator.SimRuntime.iOS-18-1"),
            version("com.apple.CoreSimulator.SimRuntime.iOS-9-3"),
        )
        self.assertGreater(
            version("com.apple.CoreSimulator.SimRuntime.iOS-18-2"),
            version("com.apple.CoreSimulator.SimRuntime.iOS-18-1"),
        )
        self.assertEqual(version("not-an-ios-runtime"), ())

    def test_export_cleanup_retries_background_directory_writes(self):
        directory = object.__new__(
            test_platform_export.RetryingTemporaryDirectory
        )
        with (
            mock.patch.object(
                tempfile.TemporaryDirectory,
                "cleanup",
                side_effect=[
                    OSError(errno.ENOTEMPTY, "directory is still active"),
                    None,
                ],
            ) as cleanup,
            mock.patch.object(test_platform_export.time, "sleep") as sleep,
        ):
            directory.cleanup()
        self.assertEqual(cleanup.call_count, 2)
        sleep.assert_called_once_with(0.1)

    def test_ios_runtime_timeout_preserves_application_diagnostics(self):
        source = (ROOT / "tools/test_platform_export.py").read_text(
            encoding="utf-8"
        )
        self.assertIn("IOS_SIMULATOR_RUNTIME_TIMEOUT_SECONDS = 120", source)
        self.assertIn("IOS_SIMULATOR_TERMINATE_TIMEOUT_SECONDS = 15", source)
        self.assertIn("except subprocess.TimeoutExpired:", source)
        self.assertIn("if process.poll() is None:", source)
        self.assertIn("process.kill()", source)

    def test_android_source_template_is_installed_safely(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            project.mkdir()
            template = root / "android_source.zip"
            with zipfile.ZipFile(template, "w") as archive:
                directory_info = zipfile.ZipInfo("gradle/")
                directory_info.external_attr = (
                    stat.S_IFDIR | 0o775
                ) << 16
                archive.writestr(directory_info, b"")
                file_info = zipfile.ZipInfo("gradlew")
                file_info.external_attr = (
                    stat.S_IFREG | 0o755
                ) << 16
                archive.writestr(file_info, b"#!/bin/sh\n")
            test_platform_export.install_android_source_template(
                project,
                template,
            )
            build = project / "android/build"
            self.assertEqual((build / "gradlew").read_bytes(), b"#!/bin/sh\n")
            self.assertTrue((build / "gradlew").stat().st_mode & stat.S_IXUSR)
            self.assertEqual((build / ".gdignore").read_text(), "\n")
            digest = hashlib.md5(
                template.read_bytes(),
                usedforsecurity=False,
            ).hexdigest()
            self.assertEqual(
                (project / "android/.build_version").read_text(),
                f"{template.resolve().as_posix()} [{digest}]\n",
            )

    def test_android_source_template_rejects_unsafe_archives(self):
        cases = (
            ("../outside", stat.S_IFREG | 0o644, "noncanonical"),
            ("link", stat.S_IFLNK | 0o777, "not a regular"),
        )
        for name, mode, expected in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                project.mkdir()
                template = root / "android_source.zip"
                with zipfile.ZipFile(template, "w") as archive:
                    entry = zipfile.ZipInfo(name)
                    entry.external_attr = mode << 16
                    archive.writestr(entry, b"payload")
                with self.assertRaisesRegex(ValueError, expected):
                    test_platform_export.install_android_source_template(
                        project,
                        template,
                    )
                self.assertFalse((root / "outside").exists())
                self.assertFalse((project / "android").exists())

    def test_android_source_template_rejects_duplicates_and_size_limits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            template = root / "android_source.zip"
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)
                with zipfile.ZipFile(template, "w") as archive:
                    for _ in range(2):
                        entry = zipfile.ZipInfo("build.gradle")
                        entry.external_attr = (
                            stat.S_IFREG | 0o644
                        ) << 16
                        archive.writestr(entry, b"payload")
            project = root / "duplicate"
            project.mkdir()
            with self.assertRaisesRegex(ValueError, "duplicate paths"):
                test_platform_export.install_android_source_template(
                    project,
                    template,
                )

            limited = root / "limited.zip"
            with zipfile.ZipFile(limited, "w") as archive:
                entry = zipfile.ZipInfo("build.gradle")
                entry.external_attr = (stat.S_IFREG | 0o644) << 16
                archive.writestr(entry, b"too large")
            project = root / "limited"
            project.mkdir()
            with (
                mock.patch.object(
                    test_platform_export,
                    "MAX_ANDROID_TEMPLATE_FILE_BYTES",
                    4,
                ),
                self.assertRaisesRegex(ValueError, "entry exceeds"),
            ):
                test_platform_export.install_android_source_template(
                    project,
                    limited,
                )

    def test_mobile_and_web_export_evidence_is_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package = root / "project.apk"
            with zipfile.ZipFile(package, "w") as archive:
                archive.writestr(
                    "lib/x86_64/libgodot_host.so",
                    b"host",
                )
                archive.writestr(
                    "lib/x86_64/libgodot_rs_project_module.so",
                    b"module",
                )
            test_platform_export.verify_android_export(package, "x86_64")
            with zipfile.ZipFile(package, "w") as archive:
                archive.writestr(
                    "lib/x86_64/libgodot_host.so",
                    b"host",
                )
            with self.assertRaisesRegex(RuntimeError, "omitted Rust libraries"):
                test_platform_export.verify_android_export(
                    package,
                    "x86_64",
                )

            web = root / "web"
            web.mkdir()
            for name in (
                "godothub_project.js",
                "godothub_project.pck",
                "godothub_project.side.wasm",
                "godothub_project.wasm",
                "godot_host.wasm",
                "godot_rs_project_module.wasm",
            ):
                (web / name).write_bytes(b"payload")
            (web / "godothub_project.html").write_text(
                "godot_host.wasm godot_rs_project_module.wasm",
                encoding="utf-8",
            )
            test_platform_export.verify_web_export(
                web,
                "godothub_project.html",
            )
            (web / "godot_rs_project_module.wasm").unlink()
            with self.assertRaisesRegex(RuntimeError, "omitted runtime files"):
                test_platform_export.verify_web_export(
                    web,
                    "godothub_project.html",
                )

    def test_android_runtime_requires_all_rust_markers_and_no_fatal_signal(self):
        source = (ROOT / "tools/test_android_runtime.py").read_text(
            encoding="utf-8"
        )
        self.assertIn('[adb, "install", "-r"', source)
        self.assertNotIn('"--replace"', source)
        self.assertIn('"--pid",', source)
        complete = "\n".join(test_android_runtime.REQUIRED_MARKERS)
        self.assertEqual(test_android_runtime.runtime_log_errors(complete), [])
        errors = test_android_runtime.runtime_log_errors(
            f"{complete}\nFatal signal 11"
        )
        self.assertIn("fatal Android log marker Fatal signal", errors)
        errors = test_android_runtime.runtime_log_errors(
            test_android_runtime.RUNTIME_MARKER
        )
        self.assertIn(
            f"missing marker {test_android_runtime.RESOURCE_MARKER}",
            errors,
        )
        self.assertIn(
            f"missing marker {test_android_runtime.INHERITANCE_MARKER}",
            errors,
        )
        native_markers = (
            "GODOT_RS_NATIVE_READY api=4.4",
            "GODOT_RS_NATIVE_CLASS_OK",
            "GODOT_RS_NATIVE_STOPPED api=4.4",
        )
        self.assertEqual(
            test_android_runtime.runtime_log_errors(
                "\n".join(native_markers),
                native_markers,
            ),
            [],
        )

    def test_native_non_desktop_export_verifiers_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package = root / "native.apk"
            expected = (
                "lib/x86_64/"
                f"lib{test_native_platform_export.NATIVE_MODULE_BASENAME}.so"
            )
            with zipfile.ZipFile(package, "w") as archive:
                archive.writestr(expected, b"native")
            test_native_platform_export.verify_native_android(
                package,
                "x86_64",
            )
            with zipfile.ZipFile(package, "w") as archive:
                archive.writestr(expected, b"")
            with self.assertRaisesRegex(RuntimeError, "empty extension"):
                test_native_platform_export.verify_native_android(
                    package,
                    "x86_64",
                )

            web = root / "web"
            web.mkdir()
            module = (
                web
                / (
                    f"{test_native_platform_export.NATIVE_MODULE_BASENAME}"
                    ".wasm"
                )
            )
            module.write_bytes(b"native")
            self.assertEqual(
                test_native_platform_export.verify_native_web(web),
                module.name,
            )
            (web / "duplicate-godot_native_smoke.wasm").write_bytes(
                b"native"
            )
            with self.assertRaisesRegex(RuntimeError, "exactly one"):
                test_native_platform_export.verify_native_web(web)

    def test_native_platform_project_uses_selected_api(self):
        source = (
            ROOT / "tools/test_native_platform_export.py"
        ).read_text(encoding="utf-8")
        self.assertIn("stage_fixture(destination, godot_api)", source)
        self.assertIn("godot_api=godot_api", source)
        self.assertIn("build_ios_xcode_targets(", source)

    def test_editor_flow_uses_an_isolated_project_copy(self):
        self.assertTrue(test_editor_plugin.FLOW_PLUGIN.is_dir())
        flow = (test_editor_plugin.FLOW_PLUGIN / "plugin.gd").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            "GODOT_RUST_DIALOG_SCRIPT_CREATED",
            flow,
        )
        self.assertIn(
            "GODOT_RUST_UNCHANGED_BUILD_REUSED",
            flow,
        )
        self.assertIn("GODOT_RUST_SIGNAL_CALLBACK_CREATED", flow)
        self.assertIn("GODOT_RUST_GLOBAL_CLASSES_REGISTERED", flow)
        self.assertIn("GODOT_RUST_HOT_RELOAD_STATE_PRESERVED", flow)
        self.assertIn('"ConnectionsDock"', flow)
        self.assertIn("_node: Option<ObjectRef<Node>>", flow)
        self.assertTrue(test_editor_plugin.DIAGNOSTICS_PLUGIN.is_dir())
        diagnostics = (
            test_editor_plugin.DIAGNOSTICS_PLUGIN / "plugin.gd"
        ).read_text(encoding="utf-8")
        self.assertIn("GODOT_RUST_DIAGNOSTICS_PANEL_POPULATED", diagnostics)
        self.assertIn("GODOT_RUST_RAPID_SAVES_COALESCED", diagnostics)
        self.assertIn("GODOT_RUST_BASELINE_RECEIPT_RECORDED", diagnostics)
        self.assertTrue(test_editor_plugin.SETUP_PLUGIN.is_dir())
        setup = (test_editor_plugin.SETUP_PLUGIN / "plugin.gd").read_text(
            encoding="utf-8"
        )
        self.assertIn("GODOT_RUST_SETUP_USER_FILES_PRESERVED", setup)
        self.assertIn("GODOT_RUST_SETUP_BUILD_SUCCEEDED", setup)

    def test_expected_cargo_failure_does_not_hide_editor_errors(self):
        expected = "\n".join(
            (
                *test_editor_plugin.EXPECTED_CARGO_FAILURES,
                *test_editor_plugin.DIAGNOSTICS_MARKERS,
            )
        )
        self.assertEqual(
            test_editor_plugin.output_errors(
                expected,
                test_editor_plugin.DIAGNOSTICS_MARKERS,
                True,
            ),
            [],
        )
        errors = test_editor_plugin.output_errors(
            expected + "\nSCRIPT ERROR: Parse Error: broken fixture",
            test_editor_plugin.DIAGNOSTICS_MARKERS,
            True,
        )
        self.assertTrue(errors)


class ReleaseTests(unittest.TestCase):
    def test_release_zip_path_policy_has_stable_properties(self):
        accepted = (
            "addons/godot-rust/plugin.cfg",
            "addons/godot-rust/bin/linux-x86_64/libgodot_host.so",
            "中文/资源.rs",
        )
        rejected = (
            "",
            "/absolute",
            "../outside",
            "a/../outside",
            "a/./file",
            "a//file",
            "a\\file",
            "directory/",
            "nul\0file",
        )
        for value in accepted:
            with self.subTest(value=value):
                self.assertTrue(assemble_release.canonical_zip_entry(value))
        for value in rejected:
            with self.subTest(value=value):
                self.assertFalse(assemble_release.canonical_zip_entry(value))

        generator = random.Random(0x47445253)
        alphabet = "abcXYZ09._-/\\中文"
        for _ in range(10_000):
            value = "".join(
                generator.choice(alphabet)
                for _ in range(generator.randrange(0, 48))
            )
            if not assemble_release.canonical_zip_entry(value):
                continue
            path = assemble_release.PurePosixPath(value)
            self.assertFalse(path.is_absolute())
            self.assertEqual(path.as_posix(), value)
            self.assertNotIn("\\", value)
            self.assertFalse(value.endswith("/"))
            self.assertTrue(
                all(part not in ("", ".", "..") for part in path.parts)
            )

    def test_release_zip_verifier_rejects_duplicate_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory) / "duplicate.zip"
            info = zipfile.ZipInfo("addons/godot-rust/plugin.cfg")
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)
                with zipfile.ZipFile(package, "w") as archive:
                    archive.writestr(info, 'version="0.9.0"\n')
                    archive.writestr(info, 'version="0.9.0"\n')
            with self.assertRaisesRegex(ValueError, "duplicate paths"):
                assemble_release.verify_release_zip(package, "0.9.0")

    def test_release_zip_rejects_unexpected_platform_payloads(self):
        with tempfile.TemporaryDirectory() as directory:
            binaries = Path(directory) / "binaries"
            write_release_binary_matrix(binaries)
            package = Path(directory) / "release.zip"
            assemble_release.assemble_release(
                assemble_release.ADDON_SOURCE,
                binaries,
                package,
                WORKSPACE_VERSION,
                "0" * 40,
            )
            modified = Path(directory) / "modified.zip"
            with zipfile.ZipFile(package) as source, zipfile.ZipFile(
                modified,
                "w",
            ) as destination:
                for entry in source.infolist():
                    destination.writestr(entry, source.read(entry.filename))
                extra = zipfile.ZipInfo(
                    "addons/godot-rust/bin/unknown/host.bin",
                    assemble_release.ZIP_TIMESTAMP,
                )
                extra.compress_type = zipfile.ZIP_DEFLATED
                extra.create_system = 3
                extra.external_attr = (stat.S_IFREG | 0o644) << 16
                destination.writestr(extra, b"unexpected")
            with self.assertRaisesRegex(ValueError, "unexpected="):
                assemble_release.verify_release_zip(
                    modified,
                    WORKSPACE_VERSION,
                )

    def test_release_zip_rejects_portable_path_collisions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "binaries"
            write_release_binary_matrix(binaries)
            package = root / "release.zip"
            assemble_release.assemble_release(
                assemble_release.ADDON_SOURCE,
                binaries,
                package,
                WORKSPACE_VERSION,
                "0" * 40,
            )
            modified = root / "modified.zip"
            with zipfile.ZipFile(package) as source, zipfile.ZipFile(
                modified,
                "w",
                compression=zipfile.ZIP_DEFLATED,
            ) as destination:
                for entry in source.infolist():
                    destination.writestr(entry, source.read(entry.filename))
                for name in ("Case.txt", "case.txt"):
                    entry = zipfile.ZipInfo(
                        f"addons/godot-rust/{name}",
                        assemble_release.ZIP_TIMESTAMP,
                    )
                    entry.compress_type = zipfile.ZIP_DEFLATED
                    entry.create_system = 3
                    entry.external_attr = (stat.S_IFREG | 0o644) << 16
                    destination.writestr(entry, b"collision")
            with self.assertRaisesRegex(ValueError, "collide"):
                assemble_release.verify_release_zip(
                    modified,
                    WORKSPACE_VERSION,
                )

    def test_release_addon_has_one_exact_version_declaration(self):
        with tempfile.TemporaryDirectory() as directory:
            addon = Path(directory)
            (addon / "plugin.cfg").write_text(
                f'version="{WORKSPACE_VERSION}"\n'
                f'version="{WORKSPACE_VERSION}"\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "plugin.cfg version"):
                assemble_release.validate_addon_version(
                    addon,
                    WORKSPACE_VERSION,
                )

    def test_release_version_accepts_canonical_semver_numbers(self):
        self.assertEqual(validate_release.parse_version("0.1.9"), (0, 1, 9))
        self.assertEqual(
            validate_release.parse_version("0.1.10"),
            (0, 1, 10),
        )
        self.assertEqual(
            validate_release.parse_version("0.10.0"),
            (0, 10, 0),
        )
        for invalid in ("00.1.0", "0.01.0", "0.1.00", "v0.1.0"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                validate_release.parse_version(invalid)

    def test_release_entries_are_scoped_to_requested_version(self):
        changelog = """\
## 0.2.0 - 2026-07-27

- One
- Two

## 0.1.9

- Old
"""
        self.assertEqual(
            validate_release.release_entries(changelog, "0.2.0"),
            ["One", "Two"],
        )
        self.assertEqual(
            validate_release.release_section(changelog, "0.2.0"),
            "- One\n- Two",
        )
        self.assertEqual(
            validate_release.release_section(changelog, "9.9.9"),
            "",
        )

    def test_release_changelogs_require_matching_unordered_lists(self):
        entries, errors = validate_release.changelog_errors(
            "## 0.9.0\n\n- One\n- Two\n- Three\n- Four\n- Five\n- Six\n",
            "0.9.0",
            "CHANGELOG.md",
        )
        self.assertEqual(len(entries), 6)
        self.assertEqual(errors, [])

        _, errors = validate_release.changelog_errors(
            "## 0.9.0\n\n### Added\n\n- One\n",
            "0.9.0",
            "CHANGELOG.md",
        )
        self.assertIn("only unordered-list entries", errors[0])

    def test_release_notes_are_written_from_the_changelog_section(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "notes.md"
            result = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS / "validate_release.py"),
                    "--version",
                    WORKSPACE_VERSION,
                    "--notes-output",
                    str(output),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                output.read_text(encoding="utf-8"),
                validate_release.release_section(
                    (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"),
                    WORKSPACE_VERSION,
                )
                + "\n",
            )

    def test_published_release_body_is_compared_as_raw_json_text(self):
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        expected = validate_release.release_body(
            changelog,
            WORKSPACE_VERSION,
        )
        with tempfile.TemporaryDirectory() as directory:
            release_json = Path(directory) / "release.json"
            release_json.write_text(
                json.dumps({"body": expected}),
                encoding="utf-8",
            )
            command = [
                sys.executable,
                str(TOOLS / "validate_release.py"),
                "--version",
                WORKSPACE_VERSION,
                "--published-release-json",
                str(release_json),
            ]
            result = subprocess.run(
                command,
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            release_json.write_text(
                json.dumps({"body": f"{expected}\n"}),
                encoding="utf-8",
            )
            result = subprocess.run(
                command,
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(
                "published release body does not exactly match",
                result.stderr,
            )

    def test_release_zip_contains_one_addon_and_every_platform(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            addon = root / "addon"
            shutil.copytree(
                ROOT / "example" / "addons" / "godot-rust",
                addon,
                ignore=shutil.ignore_patterns("bin"),
            )
            binaries = root / "binaries"
            write_release_binary_matrix(binaries)
            first = root / "first.zip"
            second = root / "second.zip"
            source_commit = "a" * 40
            assemble_release.assemble_release(
                addon, binaries, first, WORKSPACE_VERSION, source_commit
            )
            assemble_release.assemble_release(
                addon, binaries, second, WORKSPACE_VERSION, source_commit
            )
            self.assertEqual(first.read_bytes(), second.read_bytes())
            assemble_release.verify_release_zip(first, WORKSPACE_VERSION)

            with zipfile.ZipFile(first) as archive:
                names = set(archive.namelist())
                self.assertIn("addons/godot-rust/plugin.cfg", names)
                self.assertIn("addons/godot-rust/LICENSE.md", names)
                self.assertIn(
                    "addons/godot-rust/supply-chain/cyclonedx.json", names
                )
                self.assertIn(
                    "addons/godot-rust/supply-chain/THIRD_PARTY_LICENSES.md",
                    names,
                )
                self.assertIn(
                    "addons/godot-rust/supply-chain/MANIFEST.sha256", names
                )
                build_info = json.loads(
                    archive.read(
                        "addons/godot-rust/supply-chain/BUILD-INFO.json"
                    )
                )
                self.assertEqual(build_info["source"]["commit"], source_commit)
                self.assertEqual(build_info["rust_toolchain"], "1.85.0")
                sbom = json.loads(
                    archive.read(
                        "addons/godot-rust/supply-chain/cyclonedx.json"
                    )
                )
                self.assertEqual(sbom["bomFormat"], "CycloneDX")
                self.assertEqual(sbom["specVersion"], "1.5")
                self.assertRegex(
                    sbom["serialNumber"],
                    (
                        r"^urn:uuid:[0-9a-f]{8}-[0-9a-f]{4}-"
                        r"5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
                    ),
                )
                application_reference = sbom["metadata"]["component"]["bom-ref"]
                application_dependencies = next(
                    dependency
                    for dependency in sbom["dependencies"]
                    if dependency["ref"] == application_reference
                )
                self.assertEqual(len(application_dependencies["dependsOn"]), 2)
                manifest_lines = archive.read(
                    "addons/godot-rust/supply-chain/MANIFEST.sha256"
                ).decode().splitlines()
                self.assertTrue(manifest_lines)
                for line in manifest_lines:
                    checksum, relative = line.split("  ", 1)
                    self.assertEqual(len(checksum), 64)
                    self.assertEqual(
                        assemble_release.hashlib.sha256(
                            archive.read(f"addons/godot-rust/{relative}")
                        ).hexdigest(),
                        checksum,
                    )
                license_text = archive.read(
                    "addons/godot-rust/supply-chain/THIRD_PARTY_LICENSES.md"
                ).decode()
                self.assertIn("Declared license:", license_text)
                self.assertIn("Permission is hereby granted", license_text)
                self.assertNotIn("addons/godot-rust/bin/.godothub", names)
                for platform_name, filenames in (
                    assemble_release.EXPECTED_BINARIES.items()
                ):
                    for filename in filenames:
                        self.assertIn(
                            "addons/godot-rust/bin/"
                            f"{platform_name}/{filename}",
                            names,
                        )
                framework_root = (
                    "addons/godot-rust/bin/apple/"
                    "godot_host.xcframework/ios-arm64/"
                    "godot_host.framework/"
                )
                self.assertEqual(
                    archive.getinfo(
                        framework_root + "godot_host"
                    ).external_attr
                    >> 16
                    & 0o777,
                    0o755,
                )
                self.assertEqual(
                    archive.getinfo(
                        framework_root + "Info.plist"
                    ).external_attr
                    >> 16
                    & 0o777,
                    0o644,
                )
                self.assertEqual(
                    archive.getinfo(
                        "addons/godot-rust/bin/macos-universal/"
                        "libgodot_host.dylib"
                    ).external_attr
                    >> 16
                    & 0o777,
                    0o755,
                )
                self.assertTrue(
                    all(name.startswith("addons/godot-rust/") for name in names)
                )

            tampered = root / "tampered.zip"
            with zipfile.ZipFile(first) as source, zipfile.ZipFile(
                tampered, "w"
            ) as destination:
                for entry in source.infolist():
                    contents = source.read(entry)
                    if entry.filename.endswith("/plugin.gd"):
                        contents += b"\n# modified after assembly\n"
                    destination.writestr(entry, contents)
            with self.assertRaisesRegex(ValueError, "digest does not match"):
                assemble_release.verify_release_zip(
                    tampered, WORKSPACE_VERSION
                )

    def test_release_zip_rejects_an_incomplete_platform_matrix(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "binaries"
            binaries.mkdir()
            with self.assertRaisesRegex(ValueError, "platforms do not match"):
                assemble_release.validate_binaries(binaries)


class NativeLoadToolTests(unittest.TestCase):
    def test_windows_extended_paths_are_compared_canonically(self):
        self.assertEqual(
            test_native_load.strip_windows_extended_prefix(
                "\\\\?\\C:\\project\\target\\library.dll"
            ),
            "C:\\project\\target\\library.dll",
        )
        self.assertEqual(
            test_native_load.strip_windows_extended_prefix(
                "\\\\?\\UNC\\server\\share\\library.dll"
            ),
            "\\\\server\\share\\library.dll",
        )

    def test_hot_reload_builds_inside_a_running_editor_lifecycle(self):
        source = (ROOT / "tools/test_native_hot_reload.py").read_text(
            encoding="utf-8"
        )
        self.assertNotIn('"--script"', source)
        self.assertIn("process.wait(timeout=120)", source)
        self.assertIn('temporary_root / "godot-editor.log"', source)
        self.assertIn("stderr=subprocess.STDOUT", source)
        self.assertIn(
            "prime_editor_project(godot, project, environment)", source
        )
        self.assertIn('target_directory / ".gdignore"', source)
        self.assertIn("EDITOR_STARTUP_TIMEOUT_SECONDS = 300", source)
        self.assertIn("EDITOR_READY_TIMEOUT_SECONDS = 180", source)
        self.assertLess(
            source.index("process = subprocess.Popen"),
            source.index("first_report = build_native_fixture(project, api)"),
        )
        self.assertIn('project / "native-hot-reload.load"', source)
        self.assertIn('enabled=PackedStringArray("native_hot_reload")', source)

    def test_release_export_allows_cold_cross_target_builds(self):
        source = (ROOT / "tools/test_godot_export.py").read_text(
            encoding="utf-8"
        )
        self.assertIn("EXPORT_TIMEOUT_SECONDS = 900", source)
        self.assertIn("timeout=EXPORT_TIMEOUT_SECONDS", source)

    def test_manifest_uses_user_metadata_instead_of_cargo_features(self):
        manifest = test_native_load.render_manifest(
            "4.6", ROOT / "crates" / "godot_rs"
        )
        self.assertIn('mode = "extension"', manifest)
        self.assertIn('godot = "4.6"', manifest)
        self.assertNotIn("features", manifest)
        self.assertNotIn("GODOT_RS_GODOT", manifest)

    def test_runtime_evidence_requires_the_selected_api(self):
        fixture_source = (
            ROOT / "tests/fixtures/native_extension/src/lib.rs"
        ).read_text(encoding="utf-8")
        self.assertIn("utility::print(", fixture_source)
        self.assertNotIn(
            'eprintln!("GODOT_RS_NATIVE_READY',
            fixture_source,
        )
        self.assertNotIn(
            'eprintln!("GODOT_RS_NATIVE_STOPPED',
            fixture_source,
        )
        output = "\n".join(
            (
                "GODOT_RS_NATIVE_READY api=4.7",
                "GODOT_RS_NATIVE_CLASS_OK",
                "GODOT_RS_NATIVE_STOPPED api=4.7",
            )
        )
        self.assertEqual(test_native_load.output_errors(output, "4.7"), [])
        self.assertIn(
            "missing marker GODOT_RS_NATIVE_READY api=4.6",
            test_native_load.output_errors(output, "4.6"),
        )


if __name__ == "__main__":
    unittest.main()
