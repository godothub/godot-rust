@tool
extends EditorPlugin

signal workflow_configured(auto_check_enabled: bool)
signal automatic_check_started(revision: int)
signal automatic_check_finished(revision: int, success: bool)

const ADDON_ROOT := "res://addons/godot-rust"
const NATIVE_EXTENSION_PATH := "res://godot_rs_project.gdextension"
const SAFE_MODE_PATH := "res://.godot/rust/safe-mode"
const INIT_COMMAND := "cargo init --lib --vcs none --name godothub_project ."
const MAX_SCRIPT_CLASS_REFRESH_FILES := 10_000
const SCRIPT_CLASS_REFRESH_TIMEOUT_MSEC := 10_000
const BuildProcess := preload("res://addons/godot-rust/build_process.gd")
const DiagnosticsPanel := preload(
	"res://addons/godot-rust/diagnostics_panel.gd"
)
const DependencyDialog := preload(
	"res://addons/godot-rust/dependency_dialog.gd"
)
const RustExportPlugin := preload(
	"res://addons/godot-rust/export_plugin.gd"
)
const SourceMonitor := preload("res://addons/godot-rust/source_monitor.gd")
const EditorUi := preload("res://addons/godot-rust/editor_ui.gd")

var _active_action := ""
var _initialize_dialog: ConfirmationDialog
var _configure_dialog: ConfirmationDialog
var _workspace_dialog: ConfirmationDialog
var _workspace_selector: OptionButton
var _toolchain_repair_dialog: ConfirmationDialog
var _toolchain_target_selector: OptionButton
var _workspace_candidates: Array[Dictionary] = []
var _build_process
var _diagnostics_panel
var _dependency_dialog: ConfirmationDialog
var _bottom_panel_button: Button
var _export_plugin
var _source_monitor
var _pending_auto_check_revision := -1
var _running_check_revision := -1
var _pending_idle_build_revision := -1
var _running_idle_build_revision := -1
var _last_successful_check_revision := -1
var _last_successful_build_revision := -1
var _auto_check_enabled := false
var _build_before_play := true
var _auto_build_on_idle := false
var _cargo_project_available := false
var _workflow_ready := false
var _safe_mode_enabled := false
var _safe_mode_started := false
var _script_class_refresh_ticket := 0
var _known_rust_script_paths: Array[String] = []
var _pending_native_reload: Dictionary = {}
var _editor_scale := 1.0
var _tool_menu_items: Array[String] = []


func _enter_tree() -> void:
	add_to_group(&"godot_rust_editor_plugin")
	_editor_scale = maxf(get_editor_interface().get_editor_scale(), 1.0)
	_safe_mode_enabled = FileAccess.file_exists(SAFE_MODE_PATH)
	_safe_mode_started = _safe_mode_enabled
	_export_plugin = RustExportPlugin.new()
	_export_plugin.configure(self)
	add_export_plugin(_export_plugin)
	_build_process = BuildProcess.new()
	_build_process.request_completed.connect(_request_finished)
	_build_process.request_cancelled.connect(_request_cancelled)
	add_child(_build_process)
	_source_monitor = SourceMonitor.new()
	_source_monitor.check_requested.connect(_on_auto_check_requested)
	_source_monitor.revision_changed.connect(_on_source_revision_changed)
	_source_monitor.monitor_failed.connect(_on_source_monitor_failed)
	add_child(_source_monitor)
	_source_monitor.start(get_editor_interface().get_resource_filesystem())
	resource_saved.connect(_source_monitor.notify_resource_saved)
	_diagnostics_panel = DiagnosticsPanel.new()
	_diagnostics_panel.configure_ui(_editor_scale)
	_diagnostics_panel.check_requested.connect(_check_project)
	_diagnostics_panel.build_requested.connect(_build_project)
	_diagnostics_panel.cancel_requested.connect(_cancel_active_request)
	_diagnostics_panel.diagnostic_activated.connect(_open_diagnostic)
	_diagnostics_panel.suggestion_requested.connect(
		_apply_diagnostic_suggestion
	)
	_diagnostics_panel.undo_requested.connect(_undo_diagnostic_suggestion)
	_diagnostics_panel.support_bundle_requested.connect(
		_create_support_bundle
	)
	_diagnostics_panel.toolchain_repair_requested.connect(
		_show_toolchain_repair
	)
	_bottom_panel_button = add_control_to_bottom_panel(
		_diagnostics_panel,
		EditorUi.text("Rust")
	)
	_add_rust_tool_menu("Rust: Project Status", _probe_project)
	_add_rust_tool_menu("Rust: Check", _check_project)
	_add_rust_tool_menu("Rust: Build", _build_project)
	_add_rust_tool_menu("Rust: Dependencies", _show_dependencies)
	_add_rust_tool_menu("Rust: Toggle Safe Mode", _toggle_safe_mode)
	_add_rust_tool_menu("Rust: Create Support Bundle", _create_support_bundle)
	_add_rust_tool_menu("Rust: Repair Toolchain...", _show_toolchain_repair)
	_dependency_dialog = DependencyDialog.new()
	_dependency_dialog.configure_ui(_editor_scale)
	_dependency_dialog.reload_requested.connect(_load_dependencies)
	_dependency_dialog.preview_requested.connect(_preview_dependency_change)
	_dependency_dialog.apply_requested.connect(_apply_dependency_change)
	get_editor_interface().get_base_control().add_child(_dependency_dialog)
	_initialize_dialog = ConfirmationDialog.new()
	_initialize_dialog.title = EditorUi.text(
		"Initialize Rust for this Godot project?"
	)
	_initialize_dialog.dialog_text = (
		EditorUi.text(
			"godot-rust did not find a Cargo.toml.\n\n"
			+ "It can initialize a standard Cargo library in the project root "
			+ "with:\n%s\n\nIt will then add the godot_rs dependency, Script "
			+ "Mode metadata, dynamic-library targets, and generated script "
			+ "entry files. Existing Cargo files and src/lib.rs are never "
			+ "overwritten."
		)
	) % INIT_COMMAND
	_initialize_dialog.ok_button_text = EditorUi.text("Initialize")
	_initialize_dialog.confirmed.connect(_initialize_project)
	get_editor_interface().get_base_control().add_child(_initialize_dialog)
	_configure_dialog = ConfirmationDialog.new()
	_configure_dialog.title = EditorUi.text(
		"Configure this Cargo package for godot-rust?"
	)
	_configure_dialog.dialog_text = EditorUi.text(
		"godot-rust found a Cargo package that is not ready for Rust scripts.\n\n"
		+ "It can add missing Script Mode settings, the compatible godot_rs "
		+ "dependency, cdylib/rlib targets, `mod scripts;`, and the Cargo cache "
		+ "directory. Existing values, dependencies, source code, comments, "
		+ "and formatting are preserved. "
		+ "Any conflict stops setup before files change."
	)
	_configure_dialog.ok_button_text = EditorUi.text("Configure")
	_configure_dialog.confirmed.connect(_configure_project)
	get_editor_interface().get_base_control().add_child(_configure_dialog)
	_workspace_dialog = ConfirmationDialog.new()
	_workspace_dialog.title = EditorUi.text("Select the Rust Cargo package")
	_workspace_dialog.dialog_text = EditorUi.text(
		"This Cargo workspace has multiple packages that can provide the "
		+ "Godot Rust scripts. Select the package owned by this Godot project."
		+ "\n\ngodot-rust will persist the choice in "
		+ "[workspace.metadata.godot-rust].package."
	)
	_workspace_dialog.ok_button_text = EditorUi.text("Use Package")
	_workspace_selector = OptionButton.new()
	_workspace_selector.custom_minimum_size = EditorUi.scaled_size(
		Vector2i(560, 36),
		_editor_scale
	)
	EditorUi.configure_control(
		_workspace_selector,
		"Select the Rust Cargo package",
		"Cargo package used for Rust Check, Build, Run, and Export."
	)
	_workspace_dialog.add_child(_workspace_selector)
	_workspace_dialog.confirmed.connect(_select_workspace_package)
	get_editor_interface().get_base_control().add_child(_workspace_dialog)
	_toolchain_repair_dialog = ConfirmationDialog.new()
	_toolchain_repair_dialog.title = EditorUi.text(
		"Install a Rust export target?"
	)
	_toolchain_repair_dialog.dialog_text = EditorUi.text(
		"Select a supported Rust standard-library target to install with "
		+ "`rustup target add`.\n\nThis action never installs or changes Xcode, "
		+ "Android SDK/NDK, Emscripten, system compilers, or linkers."
	)
	_toolchain_repair_dialog.ok_button_text = EditorUi.text("Install Target")
	_toolchain_target_selector = OptionButton.new()
	_toolchain_target_selector.custom_minimum_size = EditorUi.scaled_size(
		Vector2i(620, 36),
		_editor_scale
	)
	EditorUi.configure_control(
		_toolchain_target_selector,
		"Install a Rust export target?",
		"Exact rustup target used by the selected Godot export platform."
	)
	_toolchain_repair_dialog.add_child(_toolchain_target_selector)
	_toolchain_repair_dialog.confirmed.connect(_install_selected_rust_target)
	get_editor_interface().get_base_control().add_child(
		_toolchain_repair_dialog
	)
	call_deferred("_start_after_editor_ready")


func _add_rust_tool_menu(label: String, callback: Callable) -> void:
	var localized := EditorUi.text(label)
	add_tool_menu_item(localized, callback)
	_tool_menu_items.append(localized)


func _start_after_editor_ready() -> void:
	for _frame in 5:
		await get_tree().process_frame
	if is_inside_tree():
		if _safe_mode_enabled:
			_workflow_ready = true
			_cargo_project_available = false
			_source_monitor.set_auto_check_enabled(false)
			_set_workflow_status(
				"Rust Safe Mode is active: language and resources only; "
				+ "Cargo and project-module loading are disabled."
			)
			workflow_configured.emit(false)
		else:
			_probe_project()


func _exit_tree() -> void:
	for item in _tool_menu_items:
		remove_tool_menu_item(item)
	_tool_menu_items.clear()
	if is_instance_valid(_export_plugin):
		remove_export_plugin(_export_plugin)
		_export_plugin = null
	if is_instance_valid(_build_process):
		_build_process.shutdown()
		_build_process.queue_free()
	if is_instance_valid(_source_monitor):
		_source_monitor.queue_free()
	if is_instance_valid(_diagnostics_panel):
		remove_control_from_bottom_panel(_diagnostics_panel)
		_diagnostics_panel.queue_free()
	if is_instance_valid(_dependency_dialog):
		_dependency_dialog.queue_free()
	if is_instance_valid(_initialize_dialog):
		_initialize_dialog.queue_free()
	if is_instance_valid(_configure_dialog):
		_configure_dialog.queue_free()
	if is_instance_valid(_workspace_dialog):
		_workspace_dialog.queue_free()
	if is_instance_valid(_toolchain_repair_dialog):
		_toolchain_repair_dialog.queue_free()


func _probe_project() -> void:
	_run_request(
		"project probe",
		{
			"command": "probe",
			"id": 1,
			"root": _project_root(),
		}
	)


func _check_project() -> void:
	if _run_request(
		"Rust check",
		{
			"command": "project_cargo",
			"id": 2,
			"root": _project_root(),
			"kind": "check",
		}
	):
		_pending_auto_check_revision = -1


func _build_project() -> void:
	_run_request(
		"Rust build",
		{
			"command": "project_cargo",
			"id": 3,
			"root": _project_root(),
			"kind": "build",
			"validator": _tool_path("godot_rs_module_check"),
		}
	)


func _build() -> bool:
	if _safe_mode_enabled:
		_show_workflow_failure(
			"Rust build before play",
			"Rust Safe Mode disables Cargo and project-module loading. "
			+ "Disable Safe Mode and restart the editor before running."
		)
		return false
	if not _workflow_ready:
		_show_workflow_failure(
			"Rust build before play",
			"Cargo project configuration is still loading."
		)
		return false
	if not _cargo_project_available:
		_show_workflow_failure(
			"Rust build before play",
			"No configured Cargo package is available."
		)
		return false
	if not _build_before_play:
		return true
	if _build_process.is_running():
		if _active_action not in [
			"Automatic Rust check",
			"Automatic Rust build",
		]:
			_show_workflow_failure(
				"Rust build before play",
				"Wait for %s to finish, then run the project again."
				% _active_action
			)
			return false
		print(
			"godot-rust: waiting for the active automatic Rust operation "
			+ "before building."
		)
		_set_workflow_status(
			"Waiting for the active automatic Rust operation before building."
		)
		if not _build_process.wait_for_completion():
			return false

	print("godot-rust: checking whether the last Rust build is current.")
	if not _run_request(
		"Rust build receipt check",
		{
			"command": "build_receipt",
			"id": 8,
			"root": _project_root(),
			"validator": _tool_path("godot_rs_module_check"),
		}
	):
		return false
	var receipt_completed := _wait_for_active_request()
	var receipt_response: Dictionary = receipt_completed.get("response", {})
	if _build_receipt_succeeded(receipt_response):
		var receipt_result: Dictionary = receipt_response.get("result", {})
		var receipt: Dictionary = receipt_result.get("receipt", {})
		_active_action = ""
		_last_successful_build_revision = current_source_revision()
		if is_instance_valid(_diagnostics_panel):
			_diagnostics_panel.clear_diagnostics(false)
			_diagnostics_panel.set_status(
				"Rust build is current; Cargo was not run."
			)
		print(
			"godot-rust: Build Receipt %s verified for %s; Cargo build skipped."
			% [
				receipt.get("receipt_id", ""),
				receipt.get("build_id", ""),
			]
		)
		print(
			"GODOT_RUST_BUILD_RECEIPT_REUSED receipt_id=%s build_id=%s"
			% [
				receipt.get("receipt_id", ""),
				receipt.get("build_id", ""),
			]
		)
		_schedule_rust_script_class_refresh()
		return true
	_print_stale_build_receipt(
		receipt_response,
		str(receipt_completed.get("error", ""))
	)
	var action := "Rust build before play"
	if not _run_request(
		action,
		{
			"command": "project_cargo",
			"id": 7,
			"root": _project_root(),
			"kind": "build",
			"validator": _tool_path("godot_rs_module_check"),
		}
	):
		return false
	var completed := _wait_for_active_request()
	var response: Dictionary = completed.get("response", {})
	var success := _build_response_succeeded(response)
	if success:
		var result: Dictionary = response.get("result", {})
		if str(result.get("mode", "")) == "extension":
			var report: Dictionary = result.get("report", {})
			var publication: Dictionary = report.get("publication", {})
			success = _activate_native_extension(publication)
	if success:
		_last_successful_build_revision = current_source_revision()
	return success


func prepare_rust_export(
	platform: String,
	features: PackedStringArray,
	is_debug: bool,
	export_path: String
) -> Dictionary:
	var action := "Rust %s export" % ("debug" if is_debug else "release")
	if _safe_mode_enabled:
		return _rust_export_failure(
			action,
			"Rust Safe Mode disables Cargo and project-module export. "
			+ "Disable Safe Mode and restart the editor before exporting."
		)
	if _build_process.is_running():
		if _active_action not in [
			"Automatic Rust check",
			"Automatic Rust build",
		]:
			return _rust_export_failure(
				action,
				"Wait for %s to finish, then export again." % _active_action
			)
		_set_workflow_status(
			"Waiting for the active automatic Rust operation before export."
		)
		if not _build_process.wait_for_completion():
			return _rust_export_failure(
				action,
				"The active automatic Rust operation did not complete."
			)

	print(
		"godot-rust: building Rust project for %s at %s."
		% [platform, export_path]
	)
	if not _run_request(
		action,
		{
			"command": "project_export",
			"id": 10,
			"root": _project_root(),
			"platform": platform,
			"features": features,
			"is_debug": is_debug,
			"runtime_godot": _runtime_godot_api(),
			"android_sdk": _android_sdk_path(platform),
			"validator": _tool_path("godot_rs_module_check"),
		}
	):
		return _rust_export_failure(
			action,
			"Could not start the Rust export build."
		)
	var completed := _wait_for_active_request()
	var response: Dictionary = completed.get("response", {})
	if response.is_empty():
		var message := str(completed.get("error", "")).strip_edges()
		if message.is_empty():
			message = "The packaged build service returned no response."
		return _rust_export_failure(action, message)
	if not bool(response.get("ok", false)):
		var message := str(response.get("error", "Rust export failed."))
		var process_error := str(completed.get("error", "")).strip_edges()
		if not process_error.is_empty():
			message += "\n" + process_error
		return _rust_export_failure(action, message)

	var result: Dictionary = response.get("result", {})
	var cargo_reports: Array = result.get("cargo", [])
	for value in cargo_reports:
		if not value is Dictionary:
			continue
		var cargo_report: Dictionary = value
		var rust_target := _cargo_report_target(cargo_report)
		var task_name := action
		if not rust_target.is_empty():
			task_name += " (%s)" % rust_target
		_print_cargo_report(cargo_report, task_name)
		if is_instance_valid(_diagnostics_panel):
			_diagnostics_panel.show_cargo_report(task_name, cargo_report)
		if not bool(cargo_report.get("success", false)):
			var detail := str(cargo_report.get("stderr", "")).strip_edges()
			if detail.is_empty():
				detail = "Cargo failed; see Rust diagnostics above."
			return _rust_export_failure(action, detail)

	var modules: Array = result.get("modules", [])
	if modules.is_empty():
		return _rust_export_failure(
			action,
			"Build completed without validated Rust export modules."
		)
	var validated_modules: Array[Dictionary] = []
	for value in modules:
		if not value is Dictionary:
			return _rust_export_failure(
				action,
				"Build service returned an invalid Rust export module record."
			)
		var module: Dictionary = value
		var module_path := str(module.get("module_path", ""))
		var build_id := str(module.get("build_id", ""))
		var tags: PackedStringArray = PackedStringArray(
			module.get("godot_tags", [])
		)
		if (
			module_path.is_empty()
			or build_id.is_empty()
			or tags.is_empty()
		):
			return _rust_export_failure(
				action,
				"Build service returned an incomplete Rust export module."
			)
		validated_modules.append(module)
		print(
			"godot-rust: validated Rust export module %s at %s (%s)."
			% [build_id, module_path, ", ".join(tags)]
		)
	if is_instance_valid(_diagnostics_panel):
		_diagnostics_panel.clear_diagnostics(false)
		_diagnostics_panel.set_status(
			"%d Rust %s export module(s) are ready for %s."
			% [
				validated_modules.size(),
				"debug" if is_debug else "release",
				platform,
			]
		)
	return {
		"ok": true,
		"mode": str(result.get("mode", "script")),
		"godot_api": str(result.get("godot_api", "4.4")),
		"runtime_godot": str(result.get("runtime_godot", "")),
		"modules": validated_modules,
		"native_descriptor_resource_path": str(
			result.get("native_descriptor_resource_path", "")
		),
	}


func _runtime_godot_api() -> String:
	var version := Engine.get_version_info()
	return "%d.%d" % [
		int(version.get("major", 0)),
		int(version.get("minor", 0)),
	]


func _android_sdk_path(platform: String):
	if platform != "Android":
		return null
	var settings := get_editor_interface().get_editor_settings()
	var path := str(
		settings.get_setting("export/android/android_sdk_path")
	).strip_edges()
	return path if not path.is_empty() else null


func _cargo_report_target(report: Dictionary) -> String:
	var command: Array = report.get("command", [])
	for index in command.size():
		if str(command[index]) == "--target" and index + 1 < command.size():
			return str(command[index + 1])
	return ""


func _rust_export_failure(action: String, message: String) -> Dictionary:
	_active_action = ""
	push_error("godot-rust %s failed: %s" % [action, message])
	_show_workflow_failure(action, message)
	if is_instance_valid(_diagnostics_panel):
		make_bottom_panel_item_visible(_diagnostics_panel)
	return {
		"ok": false,
		"error": message,
	}


func _build_receipt_succeeded(response: Dictionary) -> bool:
	if response.is_empty() or not response.get("ok", false):
		return false
	var result: Dictionary = response.get("result", {})
	if not bool(result.get("fresh", false)):
		return false
	var receipt: Dictionary = result.get("receipt", {})
	return (
		not str(receipt.get("receipt_id", "")).is_empty()
		and not str(receipt.get("build_id", "")).is_empty()
	)


func _print_stale_build_receipt(
	response: Dictionary,
	process_errors: String
) -> void:
	if response.get("ok", false):
		var result: Dictionary = response.get("result", {})
		print(
			"godot-rust: Rust build is not current (%s): %s"
			% [
				result.get("reason", "unknown"),
				result.get("detail", "no verification detail"),
			]
		)
		return
	var detail := str(response.get("error", "")).strip_edges()
	if detail.is_empty():
		detail = process_errors.strip_edges()
	if detail.is_empty():
		detail = "the Build Receipt query returned no usable response"
	push_warning(
		"godot-rust could not verify the previous Rust build; Cargo will run: %s"
		% detail
	)


func _build_response_succeeded(response: Dictionary) -> bool:
	if response.is_empty() or not response.get("ok", false):
		return false
	var result: Dictionary = response.get("result", {})
	var report: Dictionary = result.get("report", {})
	var cargo: Dictionary = report.get("cargo", {})
	return (
		bool(cargo.get("success", false))
		and not report.get("publication", {}).is_empty()
	)


func _initialize_project() -> void:
	_run_request(
		"Cargo initialization",
		{
			"command": "initialize",
			"id": 4,
			"root": _project_root(),
		}
	)


func _configure_project() -> void:
	_run_request(
		"Cargo configuration",
		{
			"command": "configure",
			"id": 9,
			"root": _project_root(),
		}
	)


func _load_cargo_metadata() -> void:
	_run_request(
		"Cargo metadata",
		{
			"command": "metadata",
			"id": 5,
			"root": _project_root(),
		}
	)


func _show_dependencies() -> void:
	if not _workflow_ready or not _cargo_project_available:
		_show_workflow_failure(
			"Cargo dependencies",
			"No configured Cargo package is available."
		)
		return
	_load_dependencies()


func _load_dependencies() -> void:
	if not _run_request(
		"Cargo dependencies",
		{
			"command": "dependency_list",
			"id": 15,
			"root": _project_root(),
		}
	):
		_dependency_dialog.set_status(
			"Finish the active Rust operation before loading dependencies."
		)


func _preview_dependency_change(change: Dictionary) -> void:
	if not _run_request(
		"Dependency preview",
		{
			"command": "dependency_preview",
			"id": 16,
			"root": _project_root(),
			"change": change,
		}
	):
		_dependency_dialog.set_status(
			"Finish the active Rust operation before previewing a dependency."
		)


func _apply_dependency_change(
	expected_sha256: String,
	change: Dictionary
) -> void:
	if not _run_request(
		"Dependency apply",
		{
			"command": "dependency_apply",
			"id": 17,
			"root": _project_root(),
			"expected_sha256": expected_sha256,
			"change": change,
		}
	):
		_dependency_dialog.set_status(
			"Finish the active Rust operation before changing dependencies."
		)


func _on_auto_check_requested(revision: int) -> void:
	_pending_idle_build_revision = -1
	if not _auto_check_enabled and _auto_build_on_idle:
		_pending_idle_build_revision = revision
		_start_pending_idle_build()
		return
	_pending_auto_check_revision = maxi(_pending_auto_check_revision, revision)
	print(
		"godot-rust: Rust source revision %d queued for automatic check."
		% revision
	)
	_start_pending_auto_check()


func _on_source_revision_changed(_revision: int) -> void:
	_pending_idle_build_revision = -1


func _start_pending_auto_check() -> void:
	if _pending_auto_check_revision < 0 or _build_process.is_running():
		return
	_running_check_revision = _pending_auto_check_revision
	_pending_auto_check_revision = -1
	if _run_request(
		"Automatic Rust check",
		{
			"command": "project_cargo",
			"id": 6,
			"root": _project_root(),
			"kind": "check",
		}
	):
		automatic_check_started.emit(_running_check_revision)
		return
	_pending_auto_check_revision = maxi(
		_pending_auto_check_revision,
		_running_check_revision
	)
	_running_check_revision = -1


func _start_pending_idle_build() -> void:
	if (
		not _auto_build_on_idle
		or _pending_idle_build_revision < 0
		or _pending_auto_check_revision >= 0
		or _build_process.is_running()
		or _safe_mode_enabled
		or not _cargo_project_available
	):
		return
	var revision := _pending_idle_build_revision
	_pending_idle_build_revision = -1
	if revision != current_source_revision():
		return
	_running_idle_build_revision = revision
	if _run_request(
		"Automatic Rust build",
		{
			"command": "project_cargo",
			"id": 12,
			"root": _project_root(),
			"kind": "build",
			"validator": _tool_path("godot_rs_module_check"),
		}
	):
		return
	_running_idle_build_revision = -1


func _on_source_monitor_failed(message: String) -> void:
	push_warning("godot-rust source monitor: %s" % message)
	_show_workflow_failure("Rust source monitoring", message)


func _toggle_safe_mode() -> void:
	var marker := ProjectSettings.globalize_path(SAFE_MODE_PATH)
	if FileAccess.file_exists(marker):
		var remove_error := DirAccess.remove_absolute(marker)
		if remove_error != OK:
			_show_workflow_failure(
				"Rust Safe Mode",
				"Could not remove the Safe Mode marker (error %d)." % remove_error
			)
			return
		if _safe_mode_started:
			_safe_mode_enabled = true
			_set_workflow_status(
				"Rust Safe Mode will be disabled after the editor restarts."
			)
			return
		_safe_mode_enabled = false
		_set_workflow_status("Rust Safe Mode is disabled.")
		call_deferred("_probe_project")
		return

	var directory_error := DirAccess.make_dir_recursive_absolute(
		marker.get_base_dir()
	)
	if directory_error != OK:
		_show_workflow_failure(
			"Rust Safe Mode",
			"Could not create the Safe Mode directory (error %d)."
			% directory_error
		)
		return
	var temporary := marker + ".tmp"
	if FileAccess.file_exists(temporary):
		DirAccess.remove_absolute(temporary)
	var file := FileAccess.open(temporary, FileAccess.WRITE)
	if file == null:
		_show_workflow_failure(
			"Rust Safe Mode",
			"Could not create the Safe Mode marker (error %d)."
			% FileAccess.get_open_error()
		)
		return
	file.store_string("godot-rust safe mode\n")
	file.flush()
	file = null
	var rename_error := DirAccess.rename_absolute(temporary, marker)
	if rename_error != OK:
		DirAccess.remove_absolute(temporary)
		_show_workflow_failure(
			"Rust Safe Mode",
			"Could not publish the Safe Mode marker (error %d)." % rename_error
		)
		return

	_safe_mode_enabled = true
	_active_action = ""
	_pending_auto_check_revision = -1
	_pending_idle_build_revision = -1
	_build_process.shutdown()
	_source_monitor.set_auto_check_enabled(false)
	_set_workflow_status(
		"Rust Safe Mode is enabled. Restart the editor to unload the current "
		+ "project module; Cargo is disabled immediately."
	)
	workflow_configured.emit(false)


func _wait_for_active_request() -> Dictionary:
	_build_process.wait_for_completion()
	var completion: Dictionary = _build_process.take_last_completion()
	if completion.is_empty():
		return {
			"response": {},
			"error": "The packaged build service returned no completion state.",
			"cancelled": false,
		}
	return completion


func _cancel_active_request() -> void:
	if not _build_process.cancel():
		_set_workflow_status("No Rust operation is running.")


func _request_cancelled(action: String, reason: String) -> void:
	_active_action = ""
	if action == "Automatic Rust check":
		_pending_auto_check_revision = -1
		_finish_automatic_check(false)
	elif action == "Automatic Rust build":
		_running_idle_build_revision = -1
	_set_workflow_status("%s was cancelled: %s" % [action, reason])
	call_deferred("_continue_automatic_work")


func _continue_automatic_work() -> void:
	_start_pending_auto_check()
	_start_pending_idle_build()


func _run_request(action: String, request: Dictionary) -> bool:
	if _safe_mode_enabled:
		_show_workflow_failure(
			action,
			"Rust Safe Mode disables build-service requests. "
			+ "Use Rust: Toggle Safe Mode and restart the editor."
		)
		return false
	if _build_process.is_running():
		push_warning("godot-rust is already running %s." % _active_action)
		return false
	var build_daemon := _tool_path("godot_rs_buildd")
	if not FileAccess.file_exists(build_daemon):
		var message := (
			"Tool is missing: %s. Install a complete release package or run "
			+ "the repository staging command."
		) % build_daemon
		push_error("godot-rust %s" % message)
		_show_workflow_failure(action, message)
		return false
	_active_action = action
	if not _build_process.start(action, build_daemon, request):
		_active_action = ""
		var message := "Could not start the packaged build service."
		push_error("godot-rust could not start %s." % action)
		_show_workflow_failure(action, message)
		return false
	if is_instance_valid(_diagnostics_panel):
		_diagnostics_panel.set_running(action)
	print("godot-rust: %s started." % action)
	return true


func _show_workflow_failure(action: String, message: String) -> void:
	if is_instance_valid(_diagnostics_panel):
		_diagnostics_panel.set_failure(action, message)
		make_bottom_panel_item_visible(_diagnostics_panel)


func _set_workflow_status(message: String) -> void:
	if is_instance_valid(_diagnostics_panel):
		_diagnostics_panel.set_status(message)


func _open_diagnostic(file_name: String, line: int, column: int) -> void:
	if file_name.is_empty():
		return
	var absolute_path := file_name.replace("\\", "/")
	if not absolute_path.is_absolute_path():
		absolute_path = _project_root().path_join(absolute_path)
	absolute_path = absolute_path.simplify_path()
	var resource_path := ProjectSettings.localize_path(absolute_path)
	if not resource_path.begins_with("res://"):
		var message := "The diagnostic is outside this Godot project: %s" % file_name
		push_warning("godot-rust: %s" % message)
		_set_workflow_status(message)
		return
	var script = ResourceLoader.load(resource_path, "Script")
	if not script is Script:
		var message := "Could not open the diagnostic source: %s" % resource_path
		push_warning("godot-rust: %s" % message)
		_set_workflow_status(message)
		return
	get_editor_interface().edit_script(
		script,
		maxi(line - 1, 0),
		maxi(column - 1, 0),
		true
	)
	_set_workflow_status(
		"Opened %s:%d:%d" % [resource_path, line, column]
	)


func _apply_diagnostic_suggestion(suggestion: Dictionary) -> void:
	var replacements = suggestion.get("replacements", [])
	if not replacements is Array or replacements.is_empty():
		_show_workflow_failure(
			"Rust quick fix",
			"rustc did not provide a complete machine-applicable edit."
		)
		return
	if not _run_request(
		"Rust quick fix",
		{
			"command": "apply_suggestion",
			"id": 13,
			"root": _project_root(),
			"plan": {"edits": replacements},
		}
	):
		_diagnostics_panel.set_status(
			"Rust quick fix is still available; finish the active operation first."
		)


func _undo_diagnostic_suggestion(plan: Dictionary) -> void:
	if plan.is_empty():
		return
	if not _run_request(
		"Undo Rust quick fix",
		{
			"command": "apply_suggestion",
			"id": 14,
			"root": _project_root(),
			"plan": plan,
		}
	):
		_diagnostics_panel.set_undo_plan(plan)
		_diagnostics_panel.set_status(
			"Undo is still available; finish the active operation first."
		)


func _create_support_bundle() -> void:
	if _build_process.is_running():
		_show_workflow_failure(
			"Rust support bundle",
			"Finish the active Rust operation before collecting tool versions."
		)
		return
	if (
		_safe_mode_enabled
		or not FileAccess.file_exists(_tool_path("godot_rs_buildd"))
	):
		_write_support_bundle({})
		return
	_run_request(
		"Rust support bundle",
		{
			"command": "toolchain",
			"id": 18,
			"root": _project_root(),
		}
	)


func _show_toolchain_repair() -> void:
	if _build_process.is_running():
		_show_workflow_failure(
			"Rust toolchain repair",
			"Finish the active Rust operation before installing a target."
		)
		return
	_toolchain_target_selector.clear()
	for target in _repair_targets_for_editor():
		var label := str(target.get("label", ""))
		var triple := str(target.get("target", ""))
		var index := _toolchain_target_selector.item_count
		_toolchain_target_selector.add_item("%s — %s" % [label, triple])
		_toolchain_target_selector.set_item_metadata(index, triple)
	if _toolchain_target_selector.item_count == 0:
		_show_workflow_failure(
			"Rust toolchain repair",
			"This editor platform has no supported cross-export targets."
		)
		return
	_toolchain_target_selector.select(0)
	_toolchain_repair_dialog.popup_centered(
		EditorUi.scaled_size(Vector2i(680, 320), _editor_scale)
	)
	_toolchain_target_selector.grab_focus()


func _repair_targets_for_editor() -> Array[Dictionary]:
	var targets: Array[Dictionary] = [
		{"label": "Android ARM64", "target": "aarch64-linux-android"},
		{"label": "Android ARMv7", "target": "armv7-linux-androideabi"},
		{"label": "Android x86_64", "target": "x86_64-linux-android"},
		{"label": "Android x86", "target": "i686-linux-android"},
		{"label": "Web", "target": "wasm32-unknown-emscripten"},
	]
	match OS.get_name():
		"macOS":
			targets.append_array([
				{"label": "iOS ARM64", "target": "aarch64-apple-ios"},
				{
					"label": "iOS Simulator ARM64",
					"target": "aarch64-apple-ios-sim",
				},
				{
					"label": "iOS Simulator x86_64",
					"target": "x86_64-apple-ios",
				},
				{"label": "macOS ARM64", "target": "aarch64-apple-darwin"},
				{"label": "macOS x86_64", "target": "x86_64-apple-darwin"},
			])
		"Linux":
			targets.append_array([
				{"label": "Linux ARM64", "target": "aarch64-unknown-linux-gnu"},
				{"label": "Linux ARMv7", "target": "armv7-unknown-linux-gnueabihf"},
				{"label": "Linux x86", "target": "i686-unknown-linux-gnu"},
				{"label": "Linux x86_64", "target": "x86_64-unknown-linux-gnu"},
			])
		"Windows":
			targets.append_array([
				{"label": "Windows ARM64", "target": "aarch64-pc-windows-msvc"},
				{"label": "Windows x86", "target": "i686-pc-windows-msvc"},
				{"label": "Windows x86_64", "target": "x86_64-pc-windows-msvc"},
			])
	return targets


func _install_selected_rust_target() -> void:
	var selected := _toolchain_target_selector.selected
	if selected < 0:
		_show_workflow_failure(
			"Rust toolchain repair",
			"Select a Rust target before continuing."
		)
		return
	var target := str(
		_toolchain_target_selector.get_item_metadata(selected)
	)
	if target.is_empty():
		_show_workflow_failure(
			"Rust toolchain repair",
			"The selected Rust target is invalid."
		)
		return
	_run_request(
		"Rust toolchain repair",
		{
			"command": "install_rust_target",
			"id": 19,
			"root": _project_root(),
			"target": target,
		}
	)


func _write_support_bundle(toolchain: Dictionary) -> void:
	var resource_directory := "res://.godot/rust/support"
	var absolute_directory := ProjectSettings.globalize_path(
		resource_directory
	)
	var directory_error := DirAccess.make_dir_recursive_absolute(
		absolute_directory
	)
	if directory_error != OK:
		_show_workflow_failure(
			"Rust support bundle",
			"Could not create the managed support directory: %s"
			% error_string(directory_error)
		)
		return
	var timestamp := Time.get_datetime_string_from_system(false, true)
	timestamp = timestamp.replace(":", "-").replace(" ", "_")
	var resource_path := (
		"%s/godot-rust-support-%s.zip"
		% [resource_directory, timestamp]
	)
	var absolute_path := ProjectSettings.globalize_path(resource_path)
	var engine_version := Engine.get_version_info()
	var snapshot: Dictionary = (
		_diagnostics_panel.support_snapshot()
		if is_instance_valid(_diagnostics_panel)
		else {}
	)
	var payload := {
		"format": 1,
		"plugin": {
			"name": "godot-rust",
			"version": _plugin_version(),
		},
		"godot": {
			"major": int(engine_version.get("major", 0)),
			"minor": int(engine_version.get("minor", 0)),
			"patch": int(engine_version.get("patch", 0)),
			"status": _redact_text(str(engine_version.get("status", ""))),
		},
		"platform": {
			"os": OS.get_name(),
			"distribution": _redact_text(OS.get_distribution_name()),
		},
		"workflow": {
			"cargo_project_available": _cargo_project_available,
			"safe_mode": _safe_mode_enabled,
			"automatic_check": _auto_check_enabled,
			"build_before_play": _build_before_play,
			"automatic_idle_build": _auto_build_on_idle,
		},
		"toolchain": {
			"cargo": _redact_text(
				str(toolchain.get("cargo_version_verbose", "unavailable"))
			),
			"rustc": _redact_text(
				str(toolchain.get("rustc_version_verbose", "unavailable"))
			),
		},
		"diagnostics": _redact_support_snapshot(snapshot),
	}
	var packer := ZIPPacker.new()
	var open_error := packer.open(absolute_path)
	if open_error != OK:
		_show_workflow_failure(
			"Rust support bundle",
			"Could not create the support ZIP: %s"
			% error_string(open_error)
		)
		return
	var contents := JSON.stringify(payload, "\t", false) + "\n"
	var file_error := packer.start_file("diagnostics.json")
	if file_error == OK:
		file_error = packer.write_file(contents.to_utf8_buffer())
	if file_error == OK:
		file_error = packer.close_file()
	var close_error := packer.close()
	if file_error != OK or close_error != OK:
		DirAccess.remove_absolute(absolute_path)
		_show_workflow_failure(
			"Rust support bundle",
			"Could not finish the support ZIP: %s"
			% error_string(
				file_error if file_error != OK else close_error
			)
		)
		return
	_set_workflow_status(
		"Created redacted Rust support bundle: %s" % resource_path
	)
	if DisplayServer.get_name() != "headless":
		OS.shell_show_in_file_manager(absolute_path)


func _plugin_version() -> String:
	var configuration := ConfigFile.new()
	if configuration.load(ADDON_ROOT.path_join("plugin.cfg")) != OK:
		return "unknown"
	return str(configuration.get_value("plugin", "version", "unknown"))


func _redact_support_snapshot(snapshot: Dictionary) -> Dictionary:
	var redacted: Array[Dictionary] = []
	for value in snapshot.get("diagnostics", []):
		if not value is Dictionary:
			continue
		var diagnostic: Dictionary = value
		redacted.append({
			"level": _redact_text(str(diagnostic.get("level", ""))),
			"code": _redact_text(str(diagnostic.get("code", ""))),
			"message": _redact_text(str(diagnostic.get("message", ""))),
			"file_name": _support_file_label(
				str(diagnostic.get("file_name", ""))
			),
			"line": int(diagnostic.get("line", 0)),
			"column": int(diagnostic.get("column", 0)),
		})
	return {
		"status": _redact_text(str(snapshot.get("status", ""))),
		"entries": redacted,
	}


func _support_file_label(value: String) -> String:
	var normalized := value.replace("\\", "/")
	if normalized.is_empty():
		return ""
	return normalized.get_file().left(512)


func _redact_text(value: String) -> String:
	var redacted := value
	var project_root := _project_root()
	for sensitive in [
		project_root,
		project_root.replace("\\", "/"),
		OS.get_environment("HOME"),
		OS.get_environment("USERPROFILE"),
	]:
		var path := str(sensitive)
		if not path.is_empty():
			redacted = redacted.replace(path, "<REDACTED>")
	return redacted.left(4096)


func _request_finished(
	action: String,
	response: Dictionary,
	process_errors: String
) -> void:
	_active_action = ""
	if response.is_empty():
		var message := process_errors.strip_edges()
		if message.is_empty():
			message = "The packaged build service returned no response."
		push_error("godot-rust %s %s" % [action, message])
		_show_workflow_failure(action, message)
		if action.begins_with("Dependency") or action == "Cargo dependencies":
			_dependency_dialog.set_failure(message)
		if action == "Automatic Rust check":
			_finish_automatic_check(false)
		elif action == "Automatic Rust build":
			_running_idle_build_revision = -1
		elif action == "Native reload rollback":
			_pending_native_reload = {}
		call_deferred("_continue_automatic_work")
		return
	if not response.get("ok", false):
		var message := str(response.get("error", "unknown error"))
		if not process_errors.strip_edges().is_empty():
			message += "\n" + process_errors.strip_edges()
		push_error(
			"godot-rust %s failed: %s"
			% [action, message]
		)
		_show_workflow_failure(action, message)
		if action.begins_with("Dependency") or action == "Cargo dependencies":
			_dependency_dialog.set_failure(message)
		if action == "Automatic Rust check":
			_finish_automatic_check(false)
		elif action == "Automatic Rust build":
			_running_idle_build_revision = -1
		elif action == "Native reload rollback":
			_pending_native_reload = {}
		call_deferred("_continue_automatic_work")
		return
	var result: Dictionary = response.get("result", {})
	print("godot-rust: %s completed." % action)
	if action == "project probe":
		_handle_probe(result)
	elif action == "Cargo metadata":
		_handle_cargo_metadata(result)
	elif (
		action == "Rust check"
		or action == "Rust build"
		or action == "Rust build before play"
		or action == "Automatic Rust check"
		or action == "Automatic Rust build"
	):
		_print_project_task(result, action)
		if action == "Automatic Rust check":
			var report: Dictionary = result.get("report", {})
			var cargo: Dictionary = report.get("cargo", {})
			_finish_automatic_check(bool(cargo.get("success", false)))
		elif action == "Automatic Rust build":
			var report: Dictionary = result.get("report", {})
			var cargo: Dictionary = report.get("cargo", {})
			var publication: Dictionary = report.get("publication", {})
			if (
				bool(cargo.get("success", false))
				and not publication.is_empty()
				and _running_idle_build_revision == current_source_revision()
			):
				_last_successful_build_revision = _running_idle_build_revision
			_running_idle_build_revision = -1
	elif action == "Cargo initialization":
		print(
			"godot-rust: initialized and configured Cargo package "
			+ "godothub_project."
		)
		_set_workflow_status(
			"Cargo package godothub_project is configured for Rust scripts."
		)
		call_deferred("_probe_project")
	elif action == "Cargo configuration":
		var changed_files: Array = result.get("changed_files", [])
		print(
			"godot-rust: configured Cargo package %s (%d file(s) changed)."
			% [result.get("package_name", ""), changed_files.size()]
		)
		_set_workflow_status("Cargo package configured for Rust scripts.")
		call_deferred("_probe_project")
	elif action == "Workspace package selection":
		print(
			"godot-rust: selected Cargo workspace package %s."
			% result.get("package", "")
		)
		_set_workflow_status(
			"Cargo workspace package %s selected."
			% result.get("package", "")
		)
		call_deferred("_load_cargo_metadata")
	elif action == "Rust quick fix":
		var changed_files: Array = result.get("changed_files", [])
		var undo: Dictionary = result.get("undo", {})
		_diagnostics_panel.set_undo_plan(undo)
		_set_workflow_status(
			"Applied the rustc quick fix to %d file(s)."
			% changed_files.size()
		)
		get_editor_interface().get_resource_filesystem().scan()
	elif action == "Undo Rust quick fix":
		var changed_files: Array = result.get("changed_files", [])
		_diagnostics_panel.set_undo_plan({})
		_set_workflow_status(
			"Restored %d file(s) changed by the Rust quick fix."
			% changed_files.size()
		)
		get_editor_interface().get_resource_filesystem().scan()
	elif action == "Cargo dependencies":
		_dependency_dialog.show_dependencies(result)
	elif action == "Dependency preview":
		_dependency_dialog.show_preview(result)
	elif action == "Dependency apply":
		_dependency_dialog.set_status("Cargo.toml dependency change applied.")
		get_editor_interface().get_resource_filesystem().scan()
		call_deferred("_load_dependencies")
	elif action == "Rust support bundle":
		_write_support_bundle(result)
	elif action == "Native reload commit":
		pass
	elif action == "Native reload rollback":
		call_deferred("_restore_native_extension")
	elif action == "Rust toolchain repair":
		_set_workflow_status(
			"Rust target %s is installed and ready for export."
			% result.get("target", "")
		)
	call_deferred("_continue_automatic_work")


func _handle_probe(result: Dictionary) -> void:
	var kind: Dictionary = result.get("kind", {})
	match kind.get("kind", ""):
		"uninitialized":
			_workflow_ready = true
			_cargo_project_available = false
			_auto_check_enabled = false
			_source_monitor.set_auto_check_enabled(false)
			_set_workflow_status("This Godot project needs a Cargo package.")
			workflow_configured.emit(false)
			_initialize_dialog.popup_centered(
				EditorUi.scaled_size(Vector2i(620, 300), _editor_scale)
			)
		"package":
			print("godot-rust: using Cargo package %s." % kind.get("name", ""))
			_load_cargo_metadata()
		"workspace":
			print(
				"godot-rust: using Cargo workspace with %d member(s)."
				% kind.get("member_count", 0)
			)
			_load_cargo_metadata()
		"conflict":
			_workflow_ready = true
			_cargo_project_available = false
			_auto_check_enabled = false
			_source_monitor.set_auto_check_enabled(false)
			var message := "; ".join(kind.get("messages", []))
			push_error("godot-rust cannot initialize this project safely: %s" % message)
			_show_workflow_failure("Cargo project inspection", message)
			workflow_configured.emit(false)


func _handle_cargo_metadata(result: Dictionary) -> void:
	var package: Dictionary = {}
	var selected = result.get("selected_package")
	if selected is Dictionary:
		package = selected
	var native = result.get("native_package")
	if package.is_empty() and native is Dictionary:
		package = native
	if package.is_empty():
		var workspace_candidates := _workspace_script_candidates(result)
		if workspace_candidates.size() > 1:
			_offer_workspace_selection(workspace_candidates)
			return
		var candidate := _root_script_setup_candidate(result)
		if not candidate.is_empty():
			_offer_script_setup(candidate)
			return
		_auto_check_enabled = false
		_cargo_project_available = false
		_workflow_ready = true
		_source_monitor.set_auto_check_enabled(false)
		push_warning("godot-rust could not select a Cargo package for editor tasks.")
		_set_workflow_status("No Cargo package is available for Rust editor tasks.")
		workflow_configured.emit(false)
		return
	if not bool(package.get("godot_rs_dependency", false)):
		if str(package.get("godot_rust_mode", "script")) != "extension":
			_offer_script_setup(package)
			return
		_auto_check_enabled = false
		_cargo_project_available = false
		_workflow_ready = true
		_source_monitor.set_auto_check_enabled(false)
		var message := (
			"Extension Mode package %s does not expose godot_rs under the "
			+ "`godot_rs` dependency name."
		) % package.get("name", "")
		push_error("godot-rust: %s" % message)
		_show_workflow_failure("Cargo project configuration", message)
		workflow_configured.emit(false)
		return
	var editor: Dictionary = package.get("editor", {})
	_auto_check_enabled = bool(editor.get("auto_check", true))
	_build_before_play = bool(editor.get("build_before_play", true))
	_auto_build_on_idle = bool(editor.get("auto_build_on_idle", false))
	if not _auto_build_on_idle:
		_pending_idle_build_revision = -1
	_cargo_project_available = true
	_workflow_ready = true
	_source_monitor.set_auto_check_enabled(
		_auto_check_enabled or _auto_build_on_idle
	)
	print(
		"godot-rust: automatic Rust check %s."
		% ("enabled" if _auto_check_enabled else "disabled")
	)
	_set_workflow_status(
		(
			"Rust tools are ready. Automatic check is %s; "
			+ "build before play is %s; idle build is %s."
		) % [
			"enabled" if _auto_check_enabled else "disabled",
			"enabled" if _build_before_play else "disabled",
			"enabled" if _auto_build_on_idle else "disabled",
		]
	)
	workflow_configured.emit(_auto_check_enabled)


func _root_script_setup_candidate(result: Dictionary) -> Dictionary:
	var expected_manifest := _project_root().path_join("Cargo.toml")
	expected_manifest = expected_manifest.replace("\\", "/").simplify_path()
	for value in result.get("packages", []):
		if not value is Dictionary:
			continue
		var package: Dictionary = value
		var manifest := str(package.get("manifest_path", ""))
		manifest = manifest.replace("\\", "/").simplify_path()
		if manifest != expected_manifest:
			continue
		if str(package.get("godot_rust_mode", "script")) == "extension":
			continue
		if not package.get("configuration_issues", []).is_empty():
			continue
		return package
	return {}


func _workspace_script_candidates(result: Dictionary) -> Array[Dictionary]:
	var candidates: Array[Dictionary] = []
	for value in result.get("packages", []):
		if not value is Dictionary:
			continue
		var package: Dictionary = value
		if not bool(package.get("godot_rust_enabled", true)):
			continue
		if str(package.get("godot_rust_mode", "script")) == "extension":
			continue
		if not package.get("configuration_issues", []).is_empty():
			continue
		var targets: Array = package.get("cdylib_targets", [])
		if targets.size() != 1:
			continue
		candidates.append(package)
	candidates.sort_custom(
		func(left: Dictionary, right: Dictionary) -> bool:
			var left_name := str(left.get("name", ""))
			var right_name := str(right.get("name", ""))
			if left_name == right_name:
				return (
					str(left.get("manifest_path", ""))
					< str(right.get("manifest_path", ""))
				)
			return left_name < right_name
	)
	return candidates


func _offer_workspace_selection(candidates: Array[Dictionary]) -> void:
	_auto_check_enabled = false
	_cargo_project_available = false
	_workflow_ready = true
	_source_monitor.set_auto_check_enabled(false)
	_workspace_candidates = candidates.duplicate(true)
	_workspace_selector.clear()
	for package in _workspace_candidates:
		var package_name := str(package.get("name", ""))
		var manifest := str(package.get("manifest_path", ""))
		var localized := ProjectSettings.localize_path(manifest)
		_workspace_selector.add_item("%s — %s" % [package_name, localized])
	_workspace_selector.select(0)
	_set_workflow_status(
		"Select which Cargo workspace package owns this Godot project."
	)
	workflow_configured.emit(false)
	_workspace_dialog.popup_centered(
		EditorUi.scaled_size(Vector2i(640, 360), _editor_scale)
	)
	_workspace_selector.grab_focus()


func _select_workspace_package() -> void:
	var selected := _workspace_selector.get_selected_id()
	if selected < 0 or selected >= _workspace_candidates.size():
		_show_workflow_failure(
			"Workspace package selection",
			"Select one Cargo package before continuing."
		)
		return
	var package_name := str(_workspace_candidates[selected].get("name", ""))
	if package_name.is_empty():
		_show_workflow_failure(
			"Workspace package selection",
			"The selected Cargo package has no valid name."
		)
		return
	_run_request(
		"Workspace package selection",
		{
			"command": "select_workspace",
			"id": 11,
			"root": _project_root(),
			"package": package_name,
		}
	)


func _offer_script_setup(package: Dictionary) -> void:
	_auto_check_enabled = false
	_cargo_project_available = false
	_workflow_ready = true
	_source_monitor.set_auto_check_enabled(false)
	var package_name := str(package.get("name", "Cargo package"))
	_set_workflow_status("%s needs godot-rust Script Mode setup." % package_name)
	workflow_configured.emit(false)
	_configure_dialog.dialog_text = (
		"Cargo package `%s` is not ready for Rust scripts.\n\n" % package_name
		+ "godot-rust can add only the missing Script Mode settings, the "
		+ "compatible godot_rs dependency, cdylib/rlib targets, `mod scripts;`, "
		+ "and the Cargo cache directory.\n\nExisting values, dependencies, "
		+ "source code, comments, and formatting are preserved. Any conflict "
		+ "stops setup before files change."
	)
	_configure_dialog.popup_centered(
		EditorUi.scaled_size(Vector2i(660, 340), _editor_scale)
	)


func is_auto_check_enabled() -> bool:
	return _auto_check_enabled


func is_workflow_configured() -> bool:
	return _workflow_ready


func current_source_revision() -> int:
	return _source_monitor.revision() if is_instance_valid(_source_monitor) else 0


func last_successful_check_revision() -> int:
	return _last_successful_check_revision


func last_successful_build_revision() -> int:
	return _last_successful_build_revision


func _finish_automatic_check(success: bool) -> void:
	var revision := _running_check_revision
	if success:
		_last_successful_check_revision = revision
	_running_check_revision = -1
	automatic_check_finished.emit(revision, success)
	if (
		success
		and _auto_build_on_idle
		and revision >= 0
		and revision == current_source_revision()
	):
		_pending_idle_build_revision = revision
		call_deferred("_start_pending_idle_build")


func _print_cargo_report(result: Dictionary, task_name: String) -> void:
	for diagnostic in result.get("diagnostics", []):
		var rendered: String = str(diagnostic.get("rendered", ""))
		if not rendered.is_empty():
			print(rendered)
	if result.get("success", false):
		print("godot-rust: %s succeeded." % task_name)
	else:
		push_error(
			"godot-rust: %s failed; see Rust diagnostics above." % task_name
		)


func _print_project_task(result: Dictionary, action: String) -> void:
	var mode: String = str(result.get("mode", ""))
	var report: Dictionary = result.get("report", {})
	var cargo: Dictionary = report.get("cargo", {})
	_print_cargo_report(cargo, action)
	if is_instance_valid(_diagnostics_panel):
		var needs_attention: bool = _diagnostics_panel.show_cargo_report(
			action,
			cargo
		)
		if needs_attention or action != "Automatic Rust check":
			make_bottom_panel_item_visible(_diagnostics_panel)
	if not cargo.get("success", false):
		return
	if mode == "script":
		var script_index: Dictionary = report.get("script_index", {})
		if not script_index.is_empty():
			_known_rust_script_paths.clear()
			for entry in script_index.get("scripts", []):
				if not entry is Dictionary:
					continue
				var resource_path := str(entry.get("resource_path", ""))
				if (
					resource_path.begins_with("res://")
					and resource_path.get_extension() == "rs"
				):
					_known_rust_script_paths.append(resource_path)
			print(
				"godot-rust: Rust script index %s with %d script(s)."
				% [
					(
						"updated"
						if script_index.get("changed", false)
						else "verified"
					),
					script_index.get("scripts", []).size(),
				]
			)
	var publication: Dictionary = report.get("publication", {})
	if publication.is_empty():
		return
	if mode == "script":
		print(
			"godot-rust: validated Last Known Good %s."
			% publication.get("build_id", "")
		)
		var receipt: Dictionary = report.get("receipt", {})
		if not receipt.is_empty():
			print(
				"godot-rust: recorded Build Receipt %s with %d input(s)."
				% [
					receipt.get("receipt_id", ""),
					receipt.get("input_count", 0),
				]
			)
		var receipt_issue := str(report.get("receipt_issue", ""))
		if not receipt_issue.is_empty():
			push_warning(
				"godot-rust built and published the project, but could not "
				+ "record a reusable Build Receipt: "
				+ receipt_issue
			)
		print(
			"godot-rust: the Script Host will activate this build at the next "
			+ "editor safe point; an incompatible schema keeps the previous build active."
		)
		_schedule_rust_script_class_refresh()
	elif mode == "extension":
		var native: Dictionary = report.get("native", {})
		print(
			"godot-rust: generated Native GDExtension for Godot %s (%s)."
			% [
				native.get("godot_api", ""),
				publication.get("platform_selector", ""),
			]
		)
		print(
			"godot-rust: descriptor: %s"
			% publication.get("descriptor_path", "")
		)
		if action != "Rust build before play":
			call_deferred("_activate_native_extension", publication)


func _activate_native_extension(publication: Dictionary) -> bool:
	if not FileAccess.file_exists(NATIVE_EXTENSION_PATH):
		_show_workflow_failure(
			"Native hot reload",
			"Generated extension descriptor is missing: %s"
			% NATIVE_EXTENSION_PATH
		)
		return false
	var was_loaded := GDExtensionManager.is_extension_loaded(
		NATIVE_EXTENSION_PATH
	)
	var status: int
	if was_loaded:
		status = GDExtensionManager.reload_extension(NATIVE_EXTENSION_PATH)
	else:
		status = GDExtensionManager.load_extension(NATIVE_EXTENSION_PATH)
	if (
		status == GDExtensionManager.LOAD_STATUS_OK
		and not _native_extension_is_healthy()
	):
		status = GDExtensionManager.LOAD_STATUS_FAILED
	if status == GDExtensionManager.LOAD_STATUS_OK:
		print(
			"godot-rust: Native Rust extension %s without restarting."
			% ("reloaded" if was_loaded else "loaded")
		)
		get_editor_interface().get_resource_filesystem().scan()
		_set_workflow_status("Native Rust extension is active.")
		return _resolve_native_publication(
			"Native reload commit",
			"native_commit",
			publication
		)
	if (
		not was_loaded
		and status == GDExtensionManager.LOAD_STATUS_ALREADY_LOADED
	):
		if not _native_extension_is_healthy():
			status = GDExtensionManager.LOAD_STATUS_FAILED
		else:
			return _resolve_native_publication(
				"Native reload commit",
				"native_commit",
				publication
			)
	if status == GDExtensionManager.LOAD_STATUS_ALREADY_LOADED:
		status = GDExtensionManager.LOAD_STATUS_FAILED
	if status == GDExtensionManager.LOAD_STATUS_NEEDS_RESTART:
		push_warning(
			"godot-rust: this Native API or class-layout change requires "
			+ "an editor restart; the new generation will load on restart."
		)
		_set_workflow_status(
			"Native build is ready and requires an editor restart."
		)
		_resolve_native_publication(
			"Native reload commit",
			"native_commit",
			publication
		)
		return false

	var detail := _native_load_status_text(status)
	_pending_native_reload = publication.duplicate(true)
	_pending_native_reload["reload_error"] = detail
	push_warning(
		"godot-rust: Native reload failed (%s); rejecting the candidate "
		+ "generation." % detail
	)
	if not _resolve_native_publication(
		"Native reload rollback",
		"native_rollback",
		publication
	):
		_pending_native_reload = {}
	return false


func _restore_native_extension() -> void:
	if _pending_native_reload.is_empty():
		return
	var publication := _pending_native_reload
	_pending_native_reload = {}
	if not bool(publication.get("rollback_available", false)):
		_show_workflow_failure(
			"Native hot reload",
			(
				"Could not activate the first Native Rust generation (%s). "
				+ "The rejected descriptor was removed safely."
			) % str(publication.get("reload_error", "unknown error"))
		)
		return
	var status := GDExtensionManager.reload_extension(NATIVE_EXTENSION_PATH)
	if (
		status == GDExtensionManager.LOAD_STATUS_OK
		and _native_extension_is_healthy()
	):
		get_editor_interface().get_resource_filesystem().scan()
		_set_workflow_status(
			"Native reload failed; the previous generation remains active."
		)
		print(
			"godot-rust: restored the previous Native Rust generation."
		)
		return
	_show_workflow_failure(
		"Native reload rollback",
		(
			"Previous Native generation could not be restored (%s). "
			+ "Restart the editor to recover it from the restored descriptor."
		) % _native_load_status_text(status)
	)


func _native_extension_is_healthy() -> bool:
	if not ClassDB.class_exists("GodotRsExtensionRuntime"):
		push_error(
			"godot-rust: Native runtime health class was not registered."
		)
		return false
	var runtime: Object = ClassDB.instantiate("GodotRsExtensionRuntime")
	if runtime == null or not runtime.has_method("is_healthy"):
		push_error(
			"godot-rust: Native runtime health probe could not be created."
		)
		return false
	var healthy := bool(runtime.call("is_healthy"))
	runtime = null
	return healthy


func _resolve_native_publication(
	action: String,
	command: String,
	publication: Dictionary
) -> bool:
	return _run_request(
		action,
		{
			"command": command,
			"id": 18 if command == "native_commit" else 19,
			"root": _project_root(),
			"platform_selector": str(
				publication.get("platform_selector", "")
			),
			"sha256": str(publication.get("sha256", "")),
		}
	)


func _native_load_status_text(status: int) -> String:
	match status:
		GDExtensionManager.LOAD_STATUS_FAILED:
			return "library load or initialization failed"
		GDExtensionManager.LOAD_STATUS_ALREADY_LOADED:
			return "extension was already loaded"
		GDExtensionManager.LOAD_STATUS_NOT_LOADED:
			return "extension was not registered by Godot"
		GDExtensionManager.LOAD_STATUS_NEEDS_RESTART:
			return "editor restart required"
		_:
			return "unexpected Godot status %d" % status


func _schedule_rust_script_class_refresh() -> void:
	_script_class_refresh_ticket += 1
	call_deferred(
		"_refresh_rust_script_classes",
		_script_class_refresh_ticket
	)


func _refresh_rust_script_classes(ticket: int) -> void:
	var filesystem := get_editor_interface().get_resource_filesystem()
	var paths: Array[String] = _known_rust_script_paths.duplicate()
	if paths.is_empty():
		var root := filesystem.get_filesystem()
		if root == null:
			return
		_collect_rust_script_paths(root, paths)
	var deadline := Time.get_ticks_msec() + SCRIPT_CLASS_REFRESH_TIMEOUT_MSEC
	while ticket == _script_class_refresh_ticket and is_inside_tree():
		var generation_ready := paths.is_empty()
		for path in paths:
			var script := ResourceLoader.load(path, "Script") as Script
			if script != null and script.can_instantiate():
				generation_ready = true
				break
		if generation_ready:
			for path in paths:
				filesystem.update_file(path)
			print(
				"godot-rust: refreshed Godot metadata for %d Rust script(s)."
				% paths.size()
			)
			return
		if Time.get_ticks_msec() >= deadline:
			push_warning(
				"godot-rust: the new Rust module did not become active before "
				+ "the global class refresh timeout."
			)
			return
		await get_tree().process_frame


func _collect_rust_script_paths(
	directory: EditorFileSystemDirectory,
	output: Array[String]
) -> void:
	for index in directory.get_file_count():
		if output.size() >= MAX_SCRIPT_CLASS_REFRESH_FILES:
			return
		var path := directory.get_file_path(index)
		if (
			path.get_extension() == "rs"
			and path.get_file() != "mod.rs"
			and FileAccess.file_exists(path + ".uid")
		):
			output.append(path)
	for index in directory.get_subdir_count():
		_collect_rust_script_paths(directory.get_subdir(index), output)
		if output.size() >= MAX_SCRIPT_CLASS_REFRESH_FILES:
			return


func _project_root() -> String:
	return ProjectSettings.globalize_path("res://")


func _tool_path(name: String) -> String:
	var executable := name + (".exe" if OS.has_feature("windows") else "")
	return ProjectSettings.globalize_path(
		"%s/bin/%s/%s" % [ADDON_ROOT, _platform_directory(), executable]
	)


func _platform_directory() -> String:
	if OS.has_feature("macos"):
		return "macos-universal"
	if OS.has_feature("windows"):
		return "windows-x86_64"
	if OS.has_feature("linuxbsd"):
		var architecture := Engine.get_architecture_name()
		if architecture == "arm64" or architecture == "aarch64":
			return "linux-arm64"
		return "linux-x86_64"
	return "unsupported"
