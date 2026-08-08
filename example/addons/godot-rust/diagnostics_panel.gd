@tool
extends VBoxContainer

const EditorUi := preload("res://addons/godot-rust/editor_ui.gd")

signal check_requested
signal build_requested
signal cancel_requested
signal diagnostic_activated(file_name: String, line: int, column: int)
signal suggestion_requested(suggestion: Dictionary)
signal undo_requested(plan: Dictionary)
signal support_bundle_requested
signal toolchain_repair_requested

const MAX_DIAGNOSTICS := 2_000
const MAX_CELL_CHARACTERS := 4_096

var _status_label: Label
var _check_button: Button
var _build_button: Button
var _cancel_button: Button
var _fix_button: Button
var _undo_button: Button
var _clear_button: Button
var _support_button: Button
var _repair_button: Button
var _tree: Tree
var _preview_dialog: ConfirmationDialog
var _selected_suggestion: Dictionary = {}
var _undo_plan: Dictionary = {}
var _actions_enabled := true
var _support_diagnostics: Array[Dictionary] = []
var _editor_scale := 1.0


func configure_ui(editor_scale: float) -> void:
	_editor_scale = maxf(editor_scale, 1.0)


func _ready() -> void:
	name = EditorUi.text("Rust")
	add_to_group(&"godot_rust_diagnostics_panel")
	custom_minimum_size = Vector2(
		0,
		EditorUi.scaled_size(Vector2i(0, 220), _editor_scale).y
	)

	_status_label = Label.new()
	_status_label.text = EditorUi.text("Rust tools are ready.")
	_status_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	EditorUi.configure_control(
		_status_label,
		"Rust status",
		"Rust status",
		true,
		false
	)
	add_child(_status_label)

	var toolbar := HFlowContainer.new()
	add_child(toolbar)

	_check_button = Button.new()
	_check_button.text = EditorUi.text("Check")
	EditorUi.configure_control(
		_check_button,
		"Check",
		"Run Cargo Check for the configured Rust package."
	)
	_check_button.shortcut = EditorUi.button_shortcut(KEY_C)
	_check_button.pressed.connect(_on_check_pressed)
	toolbar.add_child(_check_button)

	_build_button = Button.new()
	_build_button.text = EditorUi.text("Build")
	EditorUi.configure_control(
		_build_button,
		"Build",
		"Build and validate the configured Rust package."
	)
	_build_button.shortcut = EditorUi.button_shortcut(KEY_B)
	_build_button.pressed.connect(_on_build_pressed)
	toolbar.add_child(_build_button)

	_cancel_button = Button.new()
	_cancel_button.text = EditorUi.text("Cancel")
	EditorUi.configure_control(
		_cancel_button,
		"Cancel",
		"Cancel the active Rust operation."
	)
	_cancel_button.shortcut = EditorUi.button_shortcut(KEY_X)
	_cancel_button.disabled = true
	_cancel_button.pressed.connect(_on_cancel_pressed)
	toolbar.add_child(_cancel_button)

	_fix_button = Button.new()
	_fix_button.text = EditorUi.text("Preview Fix")
	EditorUi.configure_control(
		_fix_button,
		"Preview Fix",
		"Preview and apply a machine-applicable suggestion from rustc."
	)
	_fix_button.shortcut = EditorUi.button_shortcut(KEY_F)
	_fix_button.disabled = true
	_fix_button.pressed.connect(_on_fix_pressed)
	toolbar.add_child(_fix_button)

	_undo_button = Button.new()
	_undo_button.text = EditorUi.text("Undo Fix")
	EditorUi.configure_control(
		_undo_button,
		"Undo Fix",
		"Restore files changed by the last Rust quick fix."
	)
	_undo_button.shortcut = EditorUi.button_shortcut(KEY_Z)
	_undo_button.disabled = true
	_undo_button.pressed.connect(_on_undo_pressed)
	toolbar.add_child(_undo_button)

	_clear_button = Button.new()
	_clear_button.text = EditorUi.text("Clear")
	EditorUi.configure_control(
		_clear_button,
		"Clear",
		"Clear Rust diagnostics from this panel."
	)
	_clear_button.shortcut = EditorUi.button_shortcut(KEY_DELETE)
	_clear_button.pressed.connect(clear_diagnostics)
	toolbar.add_child(_clear_button)

	_support_button = Button.new()
	_support_button.text = EditorUi.text("Support Bundle")
	EditorUi.configure_control(
		_support_button,
		"Support Bundle",
		(
			"Create a redacted ZIP with tool versions and bounded diagnostics. "
			+ "Project source, environment variables, and absolute paths are excluded."
		)
	)
	_support_button.shortcut = EditorUi.button_shortcut(KEY_S)
	_support_button.pressed.connect(_on_support_bundle_pressed)
	toolbar.add_child(_support_button)

	_repair_button = Button.new()
	_repair_button.text = EditorUi.text("Repair Toolchain")
	EditorUi.configure_control(
		_repair_button,
		"Repair Toolchain",
		(
			"Install a supported Rust standard-library target through rustup. "
			+ "Platform SDKs and linkers are never modified automatically."
		)
	)
	_repair_button.shortcut = EditorUi.button_shortcut(KEY_T)
	_repair_button.pressed.connect(_on_toolchain_repair_pressed)
	toolbar.add_child(_repair_button)

	_tree = Tree.new()
	_tree.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_tree.columns = 4
	_tree.column_titles_visible = true
	_tree.hide_root = true
	_tree.select_mode = Tree.SELECT_ROW
	_tree.set_column_title(0, EditorUi.text("Level"))
	_tree.set_column_title(1, EditorUi.text("Code"))
	_tree.set_column_title(2, EditorUi.text("Message"))
	_tree.set_column_title(3, EditorUi.text("Location"))
	_tree.set_column_expand(0, false)
	_tree.set_column_expand(1, false)
	_tree.set_column_expand(2, true)
	_tree.set_column_expand(3, false)
	_tree.set_column_custom_minimum_width(
		0,
		EditorUi.scaled_size(Vector2i(90, 0), _editor_scale).x
	)
	_tree.set_column_custom_minimum_width(
		1,
		EditorUi.scaled_size(Vector2i(90, 0), _editor_scale).x
	)
	_tree.set_column_custom_minimum_width(
		3,
		EditorUi.scaled_size(Vector2i(240, 0), _editor_scale).x
	)
	EditorUi.configure_control(
		_tree,
		"Rust diagnostics table",
		"Rust diagnostics"
	)
	_tree.item_activated.connect(_on_item_activated)
	_tree.item_selected.connect(_on_item_selected)
	add_child(_tree)
	_tree.create_item()

	_preview_dialog = ConfirmationDialog.new()
	_preview_dialog.title = EditorUi.text("Preview Rust Quick Fix")
	_preview_dialog.ok_button_text = EditorUi.text("Apply Fix")
	_preview_dialog.confirmed.connect(_on_fix_confirmed)
	add_child(_preview_dialog)


func set_running(action: String) -> void:
	_status_label.text = EditorUi.text("%s is running…") % EditorUi.text(action)
	_set_actions_enabled(false)
	_cancel_button.disabled = false


func set_status(message: String) -> void:
	_status_label.text = _bounded_text(message)
	_set_actions_enabled(true)


func set_failure(action: String, message: String) -> void:
	_status_label.text = EditorUi.text("%s failed: %s") % [
		EditorUi.text(action),
		_bounded_text(message),
	]
	_set_actions_enabled(true)


func set_undo_plan(plan: Dictionary) -> void:
	_undo_plan = plan.duplicate(true)
	_undo_button.disabled = not _actions_enabled or _undo_plan.is_empty()


func show_cargo_report(task_name: String, report: Dictionary) -> bool:
	clear_diagnostics(false)
	var diagnostics: Array = report.get("diagnostics", [])
	var errors := 0
	var warnings := 0
	var shown := mini(diagnostics.size(), MAX_DIAGNOSTICS)
	var root := _tree.get_root()
	for index in shown:
		var diagnostic = diagnostics[index]
		if not diagnostic is Dictionary:
			continue
		var level: String = str(diagnostic.get("level", "unknown"))
		if level == "error" or level == "failure_note":
			errors += 1
		elif level == "warning":
			warnings += 1
		_add_diagnostic(root, diagnostic)
		_support_diagnostics.append(_support_entry(diagnostic))

	var success := bool(report.get("success", false))
	var summary := (
		EditorUi.text("%s succeeded")
		if success
		else EditorUi.text("%s failed")
	) % EditorUi.text(task_name)
	summary += " · " + EditorUi.text(
		"%d error(s) · %d warning(s)"
	) % [errors, warnings]
	if diagnostics.size() > shown:
		summary += " · " + EditorUi.text(
			"showing first %d of %d diagnostics"
		) % [
			shown,
			diagnostics.size(),
		]
	_status_label.text = summary
	_set_actions_enabled(true)
	return errors > 0 or not success


func clear_diagnostics(update_status := true) -> void:
	if not is_instance_valid(_tree):
		return
	_tree.clear()
	_tree.create_item()
	_selected_suggestion = {}
	_support_diagnostics.clear()
	_fix_button.disabled = true
	if update_status:
		_status_label.text = EditorUi.text("Rust diagnostics cleared.")
		_set_actions_enabled(true)


func _add_diagnostic(root: TreeItem, diagnostic: Dictionary) -> void:
	var item := _tree.create_item(root)
	var level: String = str(diagnostic.get("level", "unknown"))
	var message: String = _bounded_text(str(diagnostic.get("message", "")))
	var code_value = diagnostic.get("code")
	var code := "" if code_value == null else _bounded_text(str(code_value))
	var span := _primary_span(diagnostic.get("spans", []))
	var location := ""
	if not span.is_empty():
		location = "%s:%d:%d" % [
			span.get("file_name", ""),
			span.get("line_start", 0),
			span.get("column_start", 0),
		]
	var metadata := {
		"file_name": str(span.get("file_name", "")),
		"line": int(span.get("line_start", 1)),
		"column": int(span.get("column_start", 1)),
	}
	var suggestion := _machine_applicable_suggestion(
		diagnostic.get("suggestions", [])
	)
	if not suggestion.is_empty():
		metadata["suggestion"] = suggestion
		message += " · " + EditorUi.text("quick fix available")
	item.set_metadata(0, metadata)
	item.set_text(0, _level_label(level))
	item.set_text(1, code)
	item.set_text(2, message)
	item.set_text(3, _bounded_text(location))
	item.set_tooltip_text(2, message)
	var color := _level_color(level)
	item.set_custom_color(0, color)
	item.set_custom_color(1, color)


func _machine_applicable_suggestion(value) -> Dictionary:
	if not value is Array:
		return {}
	for suggestion in value:
		if (
			suggestion is Dictionary
			and str(suggestion.get("applicability", "")) == "MachineApplicable"
			and not suggestion.get("replacements", []).is_empty()
		):
			return suggestion
	return {}


func _primary_span(spans_value) -> Dictionary:
	if not spans_value is Array:
		return {}
	var first: Dictionary = {}
	for span in spans_value:
		if not span is Dictionary:
			continue
		if first.is_empty():
			first = span
		if span.get("is_primary", false):
			return span
	return first


func _level_label(level: String) -> String:
	match level:
		"error":
			return EditorUi.text("Error")
		"warning":
			return EditorUi.text("Warning")
		"failure_note":
			return EditorUi.text("Failure")
		"note":
			return EditorUi.text("Note")
		"help":
			return EditorUi.text("Help")
		_:
			return EditorUi.text("Info")


func _level_color(level: String) -> Color:
	match level:
		"error", "failure_note":
			return get_theme_color(&"error_color", &"Editor")
		"warning":
			return get_theme_color(&"warning_color", &"Editor")
		_:
			return get_theme_color(&"font_color", &"Tree")


func _bounded_text(value: String) -> String:
	if value.length() <= MAX_CELL_CHARACTERS:
		return value
	return value.left(MAX_CELL_CHARACTERS) + "…"


func _set_actions_enabled(enabled: bool) -> void:
	_actions_enabled = enabled
	_check_button.disabled = not enabled
	_build_button.disabled = not enabled
	_cancel_button.disabled = true
	_fix_button.disabled = not enabled or _selected_suggestion.is_empty()
	_undo_button.disabled = not enabled or _undo_plan.is_empty()
	_support_button.disabled = not enabled
	_repair_button.disabled = not enabled


func support_snapshot() -> Dictionary:
	return {
		"status": _bounded_text(_status_label.text),
		"diagnostics": _support_diagnostics.duplicate(true),
	}


func _support_entry(diagnostic: Dictionary) -> Dictionary:
	var span := _primary_span(diagnostic.get("spans", []))
	var code_value = diagnostic.get("code")
	return {
		"level": _bounded_text(str(diagnostic.get("level", "unknown"))),
		"code": (
			""
			if code_value == null
			else _bounded_text(str(code_value))
		),
		"message": _bounded_text(str(diagnostic.get("message", ""))),
		"file_name": _bounded_text(str(span.get("file_name", ""))),
		"line": int(span.get("line_start", 0)),
		"column": int(span.get("column_start", 0)),
	}


func _on_check_pressed() -> void:
	check_requested.emit()


func _on_build_pressed() -> void:
	build_requested.emit()


func _on_cancel_pressed() -> void:
	_cancel_button.disabled = true
	cancel_requested.emit()


func _on_support_bundle_pressed() -> void:
	support_bundle_requested.emit()


func _on_toolchain_repair_pressed() -> void:
	toolchain_repair_requested.emit()


func _on_item_selected() -> void:
	_selected_suggestion = {}
	var item := _tree.get_selected()
	if item != null:
		var metadata = item.get_metadata(0)
		if metadata is Dictionary:
			var suggestion = metadata.get("suggestion", {})
			if suggestion is Dictionary:
				_selected_suggestion = suggestion
	_fix_button.disabled = not _actions_enabled or _selected_suggestion.is_empty()


func _on_fix_pressed() -> void:
	if _selected_suggestion.is_empty():
		return
	var lines := PackedStringArray([
		_bounded_text(
			str(
				_selected_suggestion.get(
					"message",
					EditorUi.text("Rust quick fix")
				)
			)
		),
		"",
	])
	for replacement in _selected_suggestion.get("replacements", []):
		if not replacement is Dictionary:
			continue
		lines.append(
			"%s · bytes %d–%d"
			% [
				replacement.get("file_name", ""),
				replacement.get("byte_start", 0),
				replacement.get("byte_end", 0),
			]
		)
		var replacement_text := str(replacement.get("replacement", ""))
		lines.append(
			EditorUi.text("Replace with: %s")
			% _bounded_text(replacement_text.replace("\n", "\\n"))
		)
	_preview_dialog.dialog_text = "\n".join(lines)
	_preview_dialog.popup_centered(
		EditorUi.scaled_size(Vector2i(720, 420), _editor_scale)
	)


func _on_fix_confirmed() -> void:
	if _selected_suggestion.is_empty():
		return
	suggestion_requested.emit(_selected_suggestion.duplicate(true))
	_fix_button.disabled = true


func _on_undo_pressed() -> void:
	if _undo_plan.is_empty():
		return
	undo_requested.emit(_undo_plan.duplicate(true))
	_undo_button.disabled = true


func _on_item_activated() -> void:
	var item := _tree.get_selected()
	if item == null:
		return
	var location = item.get_metadata(0)
	if not location is Dictionary or location.is_empty():
		return
	diagnostic_activated.emit(
		str(location.get("file_name", "")),
		int(location.get("line", 1)),
		int(location.get("column", 1))
	)
