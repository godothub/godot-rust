@tool
extends ConfirmationDialog

const EditorUi := preload("res://addons/godot-rust/editor_ui.gd")

signal reload_requested
signal preview_requested(change: Dictionary)
signal apply_requested(expected_sha256: String, change: Dictionary)

var _status: Label
var _tree: Tree
var _kind: OptionButton
var _target_edit: LineEdit
var _name_edit: LineEdit
var _requirement_edit: LineEdit
var _source_kind: OptionButton
var _source_edit: LineEdit
var _git_reference_kind: OptionButton
var _git_reference_edit: LineEdit
var _features_edit: LineEdit
var _default_features: CheckBox
var _optional: CheckBox
var _replace_source: CheckBox
var _preview_dialog: ConfirmationDialog
var _preview: Dictionary = {}
var _editor_scale := 1.0


func configure_ui(editor_scale: float) -> void:
	_editor_scale = maxf(editor_scale, 1.0)


func _ready() -> void:
	title = EditorUi.text("Cargo Dependencies")
	ok_button_text = EditorUi.text("Close")
	min_size = EditorUi.scaled_size(Vector2i(900, 620), _editor_scale)

	var layout := VBoxContainer.new()
	add_child(layout)
	layout.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	layout.offset_left = 12
	layout.offset_top = 12
	layout.offset_right = -12
	layout.offset_bottom = -56

	var toolbar := HBoxContainer.new()
	layout.add_child(toolbar)
	_status = Label.new()
	_status.text = EditorUi.text(
		"Load the configured Cargo package dependencies."
	)
	_status.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	EditorUi.configure_control(
		_status,
		"Cargo dependency manager",
		"Cargo dependency manager",
		true,
		false
	)
	toolbar.add_child(_status)
	var reload_button := Button.new()
	reload_button.text = EditorUi.text("Reload")
	EditorUi.configure_control(
		reload_button,
		"Reload",
		"Reload dependencies from Cargo.toml."
	)
	reload_button.shortcut = EditorUi.button_shortcut(KEY_R)
	reload_button.pressed.connect(func() -> void: reload_requested.emit())
	toolbar.add_child(reload_button)

	_tree = Tree.new()
	_tree.columns = 5
	_tree.column_titles_visible = true
	_tree.hide_root = true
	_tree.select_mode = Tree.SELECT_ROW
	_tree.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_tree.set_column_title(0, EditorUi.text("Kind"))
	_tree.set_column_title(1, EditorUi.text("Target"))
	_tree.set_column_title(2, EditorUi.text("Dependency"))
	_tree.set_column_title(3, EditorUi.text("Version / Source"))
	_tree.set_column_title(4, EditorUi.text("Features"))
	_tree.set_column_expand(0, false)
	_tree.set_column_expand(1, false)
	_tree.set_column_expand(2, false)
	_tree.set_column_expand(3, true)
	_tree.set_column_expand(4, true)
	_tree.item_selected.connect(_on_dependency_selected)
	EditorUi.configure_control(
		_tree,
		"Cargo dependency entries",
		"Cargo dependency entries"
	)
	layout.add_child(_tree)
	_tree.create_item()

	var form := GridContainer.new()
	form.columns = 2
	layout.add_child(form)
	_add_label(form, EditorUi.text("Kind"))
	_kind = OptionButton.new()
	_kind.add_item(EditorUi.text("Normal"), 0)
	_kind.add_item(EditorUi.text("Development"), 1)
	_kind.add_item(EditorUi.text("Build"), 2)
	EditorUi.configure_control(_kind, "Kind")
	form.add_child(_kind)
	_add_label(form, EditorUi.text("Target"))
	_target_edit = LineEdit.new()
	_target_edit.placeholder_text = EditorUi.text(
		"Empty, target triple, or cfg(...)"
	)
	EditorUi.configure_control(
		_target_edit,
		"Target",
		(
			"Leave empty for all targets, or enter a Cargo target triple "
			+ "or cfg expression."
		)
	)
	form.add_child(_target_edit)

	_add_label(form, EditorUi.text("Name"))
	_name_edit = LineEdit.new()
	_name_edit.placeholder_text = "serde"
	EditorUi.configure_control(_name_edit, "Name")
	form.add_child(_name_edit)
	_add_label(form, EditorUi.text("Requirement"))
	_requirement_edit = LineEdit.new()
	_requirement_edit.placeholder_text = "1.0"
	EditorUi.configure_control(_requirement_edit, "Requirement")
	form.add_child(_requirement_edit)
	_add_label(form, EditorUi.text("Source"))
	_source_kind = OptionButton.new()
	_source_kind.add_item(EditorUi.text("Registry"), 0)
	_source_kind.add_item(EditorUi.text("Git"), 1)
	_source_kind.add_item(EditorUi.text("Path"), 2)
	_source_kind.item_selected.connect(_on_source_kind_selected)
	EditorUi.configure_control(_source_kind, "Source")
	form.add_child(_source_kind)

	_add_label(form, EditorUi.text("Source value"))
	_source_edit = LineEdit.new()
	_source_edit.placeholder_text = EditorUi.text(
		"Registry alias, Git URL, or dependency path"
	)
	EditorUi.configure_control(_source_edit, "Source value")
	form.add_child(_source_edit)
	_add_label(form, EditorUi.text("Git reference"))
	var git_reference_row := HBoxContainer.new()
	_git_reference_kind = OptionButton.new()
	_git_reference_kind.add_item(EditorUi.text("None"), 0)
	_git_reference_kind.add_item(EditorUi.text("Branch"), 1)
	_git_reference_kind.add_item(EditorUi.text("Tag"), 2)
	_git_reference_kind.add_item(EditorUi.text("Revision"), 3)
	EditorUi.configure_control(_git_reference_kind, "Git reference")
	git_reference_row.add_child(_git_reference_kind)
	_git_reference_edit = LineEdit.new()
	_git_reference_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	EditorUi.configure_control(_git_reference_edit, "Git reference")
	git_reference_row.add_child(_git_reference_edit)
	form.add_child(git_reference_row)

	_add_label(form, EditorUi.text("Features"))
	_features_edit = LineEdit.new()
	_features_edit.placeholder_text = "derive, rc"
	EditorUi.configure_control(_features_edit, "Features")
	form.add_child(_features_edit)
	_add_label(form, EditorUi.text("Options"))
	var options := HFlowContainer.new()
	_default_features = CheckBox.new()
	_default_features.text = EditorUi.text("Default features")
	_default_features.button_pressed = true
	EditorUi.configure_control(_default_features, "Default features")
	options.add_child(_default_features)
	_optional = CheckBox.new()
	_optional.text = EditorUi.text("Optional")
	EditorUi.configure_control(_optional, "Optional")
	options.add_child(_optional)
	_replace_source = CheckBox.new()
	_replace_source.text = EditorUi.text("Allow source replacement")
	EditorUi.configure_control(
		_replace_source,
		"Allow source replacement",
		"Permit changing between Registry, Git, and Path after preview."
	)
	options.add_child(_replace_source)
	form.add_child(options)

	var actions := HFlowContainer.new()
	layout.add_child(actions)
	var preview_button := Button.new()
	preview_button.text = EditorUi.text("Preview Add / Update")
	EditorUi.configure_control(
		preview_button,
		"Preview Add / Update",
		"Show the exact Cargo.toml entry before writing."
	)
	preview_button.shortcut = EditorUi.button_shortcut(KEY_P)
	preview_button.pressed.connect(_on_preview_pressed)
	actions.add_child(preview_button)
	var remove_button := Button.new()
	remove_button.text = EditorUi.text("Preview Remove")
	EditorUi.configure_control(
		remove_button,
		"Preview Remove",
		"Preview removal of the selected dependency."
	)
	remove_button.shortcut = EditorUi.button_shortcut(KEY_DELETE)
	remove_button.pressed.connect(_on_remove_pressed)
	actions.add_child(remove_button)

	_preview_dialog = ConfirmationDialog.new()
	_preview_dialog.title = EditorUi.text(
		"Review Cargo Dependency Change"
	)
	_preview_dialog.ok_button_text = EditorUi.text("Apply")
	_preview_dialog.confirmed.connect(_on_apply_confirmed)
	add_child(_preview_dialog)
	_on_source_kind_selected(0)


func show_dependencies(report: Dictionary) -> void:
	var dependencies: Array = report.get("dependencies", [])
	_tree.clear()
	var root := _tree.create_item()
	for dependency in dependencies:
		if not dependency is Dictionary:
			continue
		var item := _tree.create_item(root)
		item.set_text(0, _kind_label(str(dependency.get("kind", "normal"))))
		item.set_text(1, _optional_string(dependency.get("target")))
		item.set_text(2, str(dependency.get("name", "")))
		item.set_text(3, _source_summary(dependency))
		item.set_text(
			4,
			", ".join(PackedStringArray(dependency.get("features", [])))
		)
		item.set_metadata(0, dependency)
	_status.text = EditorUi.text(
		"%s · %d direct dependency entries"
	) % [
		report.get("package_name", "Cargo package"),
		dependencies.size(),
	]
	popup_centered(EditorUi.scaled_size(Vector2i(900, 620), _editor_scale))
	_tree.grab_focus()


func show_preview(preview: Dictionary) -> void:
	_preview = preview.duplicate(true)
	var before := str(preview.get("before", "")).strip_edges()
	var after := str(preview.get("after", "")).strip_edges()
	if before.is_empty():
		before = EditorUi.text("(dependency is not present)")
	if after.is_empty():
		after = EditorUi.text("(dependency will be removed)")
	_preview_dialog.dialog_text = (
		EditorUi.text(
			"Cargo.toml dependency entry before:\n%s\n\n"
			+ "Cargo.toml dependency entry after:\n%s"
		)
	) % [before, after]
	_preview_dialog.popup_centered(
		EditorUi.scaled_size(Vector2i(760, 420), _editor_scale)
	)


func set_status(message: String) -> void:
	_status.text = message


func set_failure(message: String) -> void:
	_status.text = EditorUi.text("Dependency operation failed: %s") % message
	popup_centered(EditorUi.scaled_size(Vector2i(900, 620), _editor_scale))


func _add_label(parent: GridContainer, text: String) -> void:
	var label := Label.new()
	label.text = text
	parent.add_child(label)


func _on_dependency_selected() -> void:
	var item := _tree.get_selected()
	if item == null:
		return
	var dependency = item.get_metadata(0)
	if not dependency is Dictionary:
		return
	_kind.select(_kind_index(str(dependency.get("kind", "normal"))))
	_target_edit.text = _optional_string(dependency.get("target"))
	_name_edit.text = str(dependency.get("name", ""))
	_requirement_edit.text = _optional_string(
		dependency.get("requirement")
	)
	var source_kind := str(dependency.get("source_kind", "registry"))
	_source_kind.select(_source_index(source_kind))
	_source_edit.text = _optional_string(dependency.get("source"))
	var reference_kind := _optional_string(
		dependency.get("git_reference_kind")
	)
	_git_reference_kind.select(_git_reference_index(reference_kind))
	_git_reference_edit.text = _optional_string(
		dependency.get("git_reference")
	)
	_features_edit.text = ", ".join(
		PackedStringArray(dependency.get("features", []))
	)
	_default_features.button_pressed = bool(
		dependency.get("default_features", true)
	)
	_optional.button_pressed = bool(dependency.get("optional", false))
	_replace_source.button_pressed = false
	_on_source_kind_selected(_source_kind.selected)


func _on_source_kind_selected(index: int) -> void:
	var is_git := index == 1
	_git_reference_kind.disabled = not is_git
	_git_reference_edit.editable = is_git
	if index == 0:
		_source_edit.placeholder_text = EditorUi.text(
			"Optional Cargo registry alias"
		)
	elif index == 1:
		_source_edit.placeholder_text = "https://github.com/owner/repository"
	else:
		_source_edit.placeholder_text = "../shared_crate"


func _on_preview_pressed() -> void:
	preview_requested.emit(_change("upsert"))


func _on_remove_pressed() -> void:
	if _name_edit.text.strip_edges().is_empty():
		set_failure(EditorUi.text("Select or enter a dependency name first."))
		return
	preview_requested.emit(_change("remove"))


func _on_apply_confirmed() -> void:
	if _preview.is_empty():
		return
	apply_requested.emit(
		str(_preview.get("expected_sha256", "")),
		_preview.get("change", {})
	)


func _change(operation: String) -> Dictionary:
	var features: Array[String] = []
	for feature in _features_edit.text.split(","):
		var normalized := feature.strip_edges()
		if not normalized.is_empty() and normalized not in features:
			features.append(normalized)
	return {
		"operation": operation,
		"kind": _kind_value(_kind.selected),
		"target": _target_edit.text.strip_edges(),
		"name": _name_edit.text.strip_edges(),
		"requirement": _requirement_edit.text.strip_edges(),
		"source_kind": _source_value(_source_kind.selected),
		"source": _source_edit.text.strip_edges(),
		"git_reference_kind": _git_reference_value(
			_git_reference_kind.selected
		),
		"git_reference": _git_reference_edit.text.strip_edges(),
		"features": features,
		"default_features": _default_features.button_pressed,
		"optional": _optional.button_pressed,
		"allow_source_change": _replace_source.button_pressed,
	}


func _source_summary(dependency: Dictionary) -> String:
	var requirement = dependency.get("requirement")
	var source = dependency.get("source")
	var values: Array[String] = []
	if requirement != null and not str(requirement).is_empty():
		values.append(str(requirement))
	if source != null and not str(source).is_empty():
		values.append(
			"%s: %s"
			% [dependency.get("source_kind", "registry"), source]
		)
	if dependency.get("inherited", false):
		values.append("workspace")
	return " · ".join(values)


func _kind_label(value: String) -> String:
	match value:
		"development":
			return EditorUi.text("Development")
		"build":
			return EditorUi.text("Build")
		_:
			return EditorUi.text("Normal")


func _optional_string(value) -> String:
	return "" if value == null else str(value)


func _kind_index(value: String) -> int:
	return {"normal": 0, "development": 1, "build": 2}.get(value, 0)


func _kind_value(index: int) -> String:
	return ["normal", "development", "build"][clampi(index, 0, 2)]


func _source_index(value: String) -> int:
	return {"registry": 0, "git": 1, "path": 2}.get(value, 0)


func _source_value(index: int) -> String:
	return ["registry", "git", "path"][clampi(index, 0, 2)]


func _git_reference_index(value: String) -> int:
	return {"": 0, "branch": 1, "tag": 2, "rev": 3}.get(value, 0)


func _git_reference_value(index: int) -> String:
	return ["", "branch", "tag", "rev"][clampi(index, 0, 3)]
