@tool
extends EditorPlugin

const EditorUi := preload("res://addons/godot-rust/editor_ui.gd")
const SCRIPT_PATH := "res://src/scripts/hello.rs"
const INVALID_SOURCE := """use godot_rs::prelude::*;

#[script(base = Node)]
pub struct Hello;

#[script]
impl Hello {
    fn _ready(&mut self) {
        let broken: i64 = "not an integer";
        godot_print!("{broken}");
    }
}
"""

var _godot_rust_plugin
var _script: Script
var _original_source := ""
var _started_revisions: Array[int] = []
var _finished_revision := -1
var _finished_success := false


func _enter_tree() -> void:
	call_deferred("_run_diagnostics_flow")


func _run_diagnostics_flow() -> void:
	for _frame in 5:
		await get_tree().process_frame
	_godot_rust_plugin = get_tree().get_first_node_in_group(
		&"godot_rust_editor_plugin"
	)
	if _godot_rust_plugin == null:
		_fail("The godot-rust editor plugin is absent from the editor tree")
		return
	if not _verify_editor_ui():
		return
	_godot_rust_plugin.automatic_check_started.connect(
		_on_automatic_check_started
	)
	_godot_rust_plugin.automatic_check_finished.connect(
		_on_automatic_check_finished
	)
	var workflow_deadline := Time.get_ticks_msec() + 30_000
	while Time.get_ticks_msec() < workflow_deadline:
		if _godot_rust_plugin.call(&"is_workflow_configured"):
			break
		await get_tree().process_frame
	if not _godot_rust_plugin.call(&"is_workflow_configured"):
		_fail("godot-rust did not load the Cargo editor configuration")
		return

	_script = ResourceLoader.load(SCRIPT_PATH, "Script") as Script
	if _script == null:
		_fail("The Rust source used by the diagnostics test could not be loaded")
		return
	_original_source = _script.source_code
	if not bool(_godot_rust_plugin.call(&"_build")):
		_fail("The valid baseline Rust project did not build")
		return
	if not FileAccess.file_exists("res://.godot/rust/build-receipt.json"):
		_fail("The valid baseline Rust build did not record a Build Receipt")
		return
	print("GODOT_RUST_BASELINE_RECEIPT_RECORDED")
	var failed_baseline := _source_revision()
	if not _save_source(INVALID_SOURCE):
		return
	var failed_revision := _source_revision()
	if failed_revision <= failed_baseline:
		_fail("Saving invalid Rust did not advance the source revision")
		return
	get_editor_interface().get_resource_filesystem().scan()
	if not await _wait_for_check(failed_revision):
		return
	if _finished_success:
		_fail("Automatic Cargo Check unexpectedly accepted invalid Rust")
		return
	print("GODOT_RUST_FAILED_CHECK_OBSERVED")
	if bool(_godot_rust_plugin.call(&"_build")):
		_fail("The foreground Rust build allowed invalid Rust to run")
		return
	print("GODOT_RUST_BUILD_BEFORE_PLAY_REJECTED")

	var panel = get_tree().get_first_node_in_group(
		&"godot_rust_diagnostics_panel"
	)
	if panel == null:
		_fail("The Rust diagnostics panel is absent from the editor tree")
		return
	var trees := panel.find_children("*", "Tree", true, false)
	if trees.size() != 1:
		_fail("The Rust diagnostics panel does not contain one diagnostics tree")
		return
	var tree: Tree = trees[0]
	var item := tree.get_root().get_first_child()
	if item == null:
		_fail("Failed Cargo Check did not populate the diagnostics panel")
		return
	var location = item.get_metadata(0)
	if (
		not location is Dictionary
		or not str(location.get("file_name", "")).ends_with("hello.rs")
		or int(location.get("line", 0)) <= 0
	):
		_fail("The first Cargo diagnostic has no Rust source location")
		return
	print("GODOT_RUST_DIAGNOSTICS_PANEL_POPULATED")

	item.select(0)
	tree.item_activated.emit()
	for _frame in 2:
		await get_tree().process_frame
	var current_script = (
		get_editor_interface().get_script_editor().get_current_script()
	)
	if current_script == null or current_script.resource_path != SCRIPT_PATH:
		_fail("Activating the Cargo diagnostic did not open its Rust source")
		return
	print("GODOT_RUST_FAILED_DIAGNOSTIC_OPENED")
	if not await _verify_support_bundle():
		return

	var recovery_baseline := _source_revision()
	var starts_before := _started_revisions.size()
	for index in 3:
		var source := _original_source + "\n// editor recovery save %d\n" % index
		if not _save_source(source):
			return
	var recovery_revision := _source_revision()
	if recovery_revision < recovery_baseline + 3:
		_fail("Rapid Rust saves were not all observed by the source monitor")
		return
	get_editor_interface().get_resource_filesystem().scan()
	if not await _wait_for_check(recovery_revision):
		return
	if not _finished_success:
		_fail("Automatic Cargo Check did not recover after valid Rust was saved")
		return
	var recovery_starts := 0
	for index in range(starts_before, _started_revisions.size()):
		if _started_revisions[index] > recovery_baseline:
			recovery_starts += 1
	if recovery_starts != 1:
		_fail(
			"Three rapid Rust saves started %d checks instead of one"
			% recovery_starts
		)
		return
	if tree.get_root().get_first_child() != null:
		_fail("Successful recovery did not clear stale Rust diagnostics")
		return
	print("GODOT_RUST_RAPID_SAVES_COALESCED")
	print("GODOT_RUST_CHECK_RECOVERED")
	if not bool(_godot_rust_plugin.call(&"_build")):
		_fail("The foreground Rust build did not recover with valid Rust")
		return
	if not FileAccess.file_exists("res://.godot/rust/last-known-good.json"):
		_fail("Recovered foreground build did not publish Last Known Good")
		return
	print("GODOT_RUST_BUILD_BEFORE_PLAY_RECOVERED")
	get_tree().quit()


func _verify_editor_ui() -> bool:
	EditorUi.set_locale_override("zh_CN")
	var translated := EditorUi.text("Support Bundle")
	EditorUi.set_locale_override("")
	if translated != "支持包":
		_fail("The bundled Simplified Chinese editor catalog is not active")
		return false
	var panel = get_tree().get_first_node_in_group(
		&"godot_rust_diagnostics_panel"
	)
	if panel == null:
		_fail("The keyboard-accessible Rust diagnostics panel is absent")
		return false
	var shortcut_count := 0
	for child in panel.find_children("*", "Button", true, false):
		var button := child as Button
		if button.shortcut == null or not button.shortcut.has_valid_event():
			continue
		if (
			button.focus_mode != Control.FOCUS_ALL
			or button.tooltip_text.is_empty()
		):
			_fail("A Rust diagnostics action has no keyboard focus or description")
			return false
		shortcut_count += 1
	if shortcut_count < 8:
		_fail("Rust diagnostics actions do not expose all keyboard shortcuts")
		return false
	var trees := panel.find_children("*", "Tree", true, false)
	if trees.size() != 1 or trees[0].focus_mode != Control.FOCUS_ALL:
		_fail("The Rust diagnostics table cannot receive keyboard focus")
		return false
	print("GODOT_RUST_ACCESSIBLE_LOCALIZED_EDITOR_UI")
	return true


func _verify_support_bundle() -> bool:
	_godot_rust_plugin.call(&"_create_support_bundle")
	var support_directory := "res://.godot/rust/support"
	var deadline := Time.get_ticks_msec() + 30_000
	var archive_path := ""
	while Time.get_ticks_msec() < deadline:
		var directory := DirAccess.open(support_directory)
		if directory != null:
			directory.list_dir_begin()
			var entry := directory.get_next()
			while not entry.is_empty():
				if entry.ends_with(".zip"):
					archive_path = support_directory.path_join(entry)
					break
				entry = directory.get_next()
			directory.list_dir_end()
		if not archive_path.is_empty():
			break
		await get_tree().process_frame
	if archive_path.is_empty():
		_fail("The one-click Rust support bundle was not created")
		return false
	var reader := ZIPReader.new()
	if reader.open(archive_path) != OK:
		_fail("The Rust support bundle is not a readable ZIP")
		return false
	if not reader.file_exists("diagnostics.json"):
		reader.close()
		_fail("The Rust support bundle has no diagnostics.json")
		return false
	var contents := reader.read_file("diagnostics.json").get_string_from_utf8()
	reader.close()
	var project_root := ProjectSettings.globalize_path("res://")
	if (
		project_root in contents
		or 'let broken: i64 = "not an integer";' in contents
		or '"project_root"' in contents
	):
		_fail("The Rust support bundle leaked a path or project source")
		return false
	var payload = JSON.parse_string(contents)
	if (
		not payload is Dictionary
		or int(payload.get("format", 0)) != 1
		or not payload.get("diagnostics", {}) is Dictionary
	):
		_fail("The Rust support bundle payload is invalid")
		return false
	print("GODOT_RUST_REDACTED_SUPPORT_BUNDLE")
	return true


func _save_source(source: String) -> bool:
	_script.source_code = source
	var error := ResourceSaver.save(_script, SCRIPT_PATH)
	if error != OK:
		_fail("Could not save the Rust diagnostics test source: %d" % error)
		return false
	_godot_rust_plugin.emit_signal(&"resource_saved", _script)
	return true


func _wait_for_check(required_revision: int) -> bool:
	var deadline := Time.get_ticks_msec() + 90_000
	while Time.get_ticks_msec() < deadline:
		if _finished_revision >= required_revision:
			return true
		await get_tree().process_frame
	_fail("Automatic Cargo Check did not finish for revision %d" % required_revision)
	return false


func _source_revision() -> int:
	return int(_godot_rust_plugin.call(&"current_source_revision"))


func _on_automatic_check_started(revision: int) -> void:
	_started_revisions.append(revision)


func _on_automatic_check_finished(revision: int, success: bool) -> void:
	_finished_revision = revision
	_finished_success = success


func _fail(message: String) -> void:
	push_error("godot-rust diagnostics test: %s" % message)
	get_tree().quit(1)
