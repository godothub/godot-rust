@tool
extends EditorPlugin

const SCRIPT_PATH := "res://src/scripts/editor_created.rs"
const SCRIPT_UID_PATH := SCRIPT_PATH + ".uid"
const INDEX_PATH := "res://src/scripts/mod.rs"
const HOT_RELOAD_SCRIPT_PATH := "res://src/scripts/base_greeting.rs"

var _dialog
var _godot_rust_plugin
var _required_check_revision := -1
var _finished_check_revision := -1
var _finished_check_success := false


func _enter_tree() -> void:
	call_deferred("_run_editor_flow")


func _exit_tree() -> void:
	if is_instance_valid(_dialog):
		_dialog.queue_free()


func _run_editor_flow() -> void:
	for _frame in 5:
		await get_tree().process_frame
	_godot_rust_plugin = get_tree().get_first_node_in_group(
		&"godot_rust_editor_plugin"
	)
	if _godot_rust_plugin == null:
		_fail("The godot-rust editor plugin is absent from the editor tree")
		return
	if not _godot_rust_plugin.has_signal(&"automatic_check_finished"):
		_fail("The godot-rust editor plugin does not report automatic checks")
		return
	var check_connection_error: int = _godot_rust_plugin.connect(
		&"automatic_check_finished",
		_on_automatic_check_finished
	)
	if check_connection_error != OK:
		_fail("Could not observe automatic Rust checks")
		return
	var workflow_deadline := Time.get_ticks_msec() + 30_000
	while Time.get_ticks_msec() < workflow_deadline:
		if _godot_rust_plugin.call(&"is_workflow_configured"):
			break
		await get_tree().process_frame
	if not _godot_rust_plugin.call(&"is_workflow_configured"):
		_fail("godot-rust did not load the Cargo editor configuration")
		return
	if not _godot_rust_plugin.call(&"is_auto_check_enabled"):
		_fail("The test project did not enable automatic Rust checks")
		return
	if not ClassDB.class_exists("ScriptCreateDialog"):
		_fail("ScriptCreateDialog is unavailable")
		return
	if not ClassDB.class_exists("GodotRustScript"):
		_fail("GodotRustScript is unavailable")
		return

	_dialog = ClassDB.instantiate("ScriptCreateDialog")
	if _dialog == null:
		_fail("ScriptCreateDialog could not be created")
		return
	get_editor_interface().get_base_control().add_child(_dialog)
	var connection_error: int = _dialog.connect(&"script_created", _on_script_created)
	if connection_error != OK:
		_fail("Could not observe ScriptCreateDialog.script_created")
		return
	_dialog.config("Node2D", SCRIPT_PATH, false, false)
	_dialog.popup_centered()
	await get_tree().process_frame

	if not _select_rust_language():
		_fail("Rust is absent from Godot's Attach Script language selector")
		return
	await get_tree().process_frame
	if _dialog.get_ok_button().disabled:
		_fail("Godot rejected the Rust script path")
		return
	_required_check_revision = (
		int(_godot_rust_plugin.call(&"current_source_revision")) + 1
	)
	_dialog.get_ok_button().pressed.emit()

	var script_deadline := Time.get_ticks_msec() + 10_000
	while Time.get_ticks_msec() < script_deadline:
		if FileAccess.file_exists(SCRIPT_PATH):
			break
		await get_tree().process_frame
	if not FileAccess.file_exists(SCRIPT_PATH):
		_print_dialog_state("after create")
		_fail("Godot did not save the Rust script")
		return
	var filesystem := get_editor_interface().get_resource_filesystem()
	filesystem.scan()
	var uid_deadline := Time.get_ticks_msec() + 10_000
	while Time.get_ticks_msec() < uid_deadline:
		if FileAccess.file_exists(SCRIPT_UID_PATH):
			break
		await get_tree().process_frame
	if not FileAccess.file_exists(SCRIPT_UID_PATH):
		_fail("Godot did not save the Rust script Resource UID")
		return

	var restored := ResourceLoader.load(SCRIPT_PATH, "Script") as Script
	if restored == null:
		_fail("Godot could not reload the newly saved Rust script")
		return
	var source := restored.source_code
	if (
		"#[script(base = Node2D)]" not in source
		or "pub struct EditorCreated;" not in source
		or "fn _ready(&mut self)" not in source
	):
		_fail("Godot did not use the base-aware Rust script template")
		return
	var owner := Node2D.new()
	owner.set_script(restored)
	if owner.get_script() != restored:
		owner.free()
		_fail("Godot could not attach the new Rust script to Node2D")
		return
	owner.free()
	print("GODOT_RUST_DIALOG_SCRIPT_CREATED")

	var uid := ResourceLoader.get_resource_uid(SCRIPT_PATH)
	if uid < 0:
		_fail("Godot did not register the new Rust script Resource UID")
		return
	var uid_text := ResourceUID.id_to_text(uid)
	filesystem.scan()
	var check_deadline := Time.get_ticks_msec() + 90_000
	while Time.get_ticks_msec() < check_deadline:
		if (
			_finished_check_revision >= _required_check_revision
			and _finished_check_success
			and _index_contains_script(uid_text)
		):
			break
		await get_tree().process_frame
	if _finished_check_revision < _required_check_revision:
		_fail("Saving the Rust script did not trigger an automatic Cargo Check")
		return
	if not _finished_check_success:
		_fail("Automatic Cargo Check rejected the Rust script created by Godot")
		return
	if not _index_contains_script(uid_text):
		_fail("Automatic Cargo Check did not index the new Rust script")
		return
	var index_file := FileAccess.open(INDEX_PATH, FileAccess.READ)
	if index_file == null:
		_fail("The generated Cargo script index could not be read")
		return
	var index_source := index_file.get_as_text()
	if (
		"editor_created.rs" not in index_source
		or "::EditorCreated =>" not in index_source
		or uid_text not in index_source
	):
		_fail("The generated Cargo script index has incomplete registration data")
		return
	print("GODOT_RUST_AUTOMATIC_CHECK_COMPLETED")
	print("GODOT_RUST_SCRIPT_INDEXED")
	_godot_rust_plugin.call(
		&"_open_diagnostic",
		"src/scripts/editor_created.rs",
		1,
		1
	)
	for _frame in 2:
		await get_tree().process_frame
	var current_script = (
		get_editor_interface().get_script_editor().get_current_script()
	)
	if current_script == null or current_script.resource_path != SCRIPT_PATH:
		_fail("The Rust diagnostic did not open its project source")
		return
	print("GODOT_RUST_DIAGNOSTIC_SOURCE_OPENED")
	if not _verify_rust_editor_contract():
		return
	if not await _verify_signal_callback_generation():
		return
	if not bool(_godot_rust_plugin.call(&"_build")):
		_fail("The foreground Rust build rejected the valid project")
		return
	if not FileAccess.file_exists("res://.godot/rust/last-known-good.json"):
		_fail("The foreground Rust build did not publish Last Known Good")
		return
	if (
		int(_godot_rust_plugin.call(&"last_successful_build_revision"))
		< _required_check_revision
	):
		_fail("The foreground Rust build did not record its source revision")
		return
	print("GODOT_RUST_BUILD_BEFORE_PLAY_SUCCEEDED")
	if not await _verify_hot_reload():
		return
	var reuse_started_at := Time.get_ticks_msec()
	if not bool(_godot_rust_plugin.call(&"_build")):
		_fail("The unchanged foreground Rust build was not reusable")
		return
	var reuse_elapsed := Time.get_ticks_msec() - reuse_started_at
	print("GODOT_RUST_RECEIPT_REUSE_MSEC %d" % reuse_elapsed)
	print("GODOT_RUST_UNCHANGED_BUILD_REUSED")
	get_tree().quit()


func _verify_hot_reload() -> bool:
	var rust_script := ResourceLoader.load(
		HOT_RELOAD_SCRIPT_PATH,
		"Script"
	) as Script
	if rust_script == null:
		_fail("Could not load the Rust script used by the hot-reload test")
		return false
	var activation_deadline := Time.get_ticks_msec() + 10_000
	while (
		Time.get_ticks_msec() < activation_deadline
		and not rust_script.can_instantiate()
	):
		await get_tree().process_frame
	if not rust_script.can_instantiate():
		_fail("The first Rust module generation did not become active")
		return false
	var class_deadline := Time.get_ticks_msec() + 10_000
	var missing_global_classes: Array[String] = []
	while Time.get_ticks_msec() < class_deadline:
		missing_global_classes.assign(
			["BaseGreeting", "GameTuning", "RustHello"]
		)
		for entry in ProjectSettings.get_global_class_list():
			missing_global_classes.erase(str(entry.get("class", "")))
		if missing_global_classes.is_empty():
			break
		await get_tree().process_frame
	if not missing_global_classes.is_empty():
		_fail(
			"Rust global classes were not registered after the build: %s"
			% ", ".join(missing_global_classes)
		)
		return false
	print("GODOT_RUST_GLOBAL_CLASSES_REGISTERED")
	var owner := Node.new()
	owner.set_script(rust_script)
	if (
		not owner.has_method(&"format_greeting")
		or owner.call(&"format_greeting", "Godot") != "Hello, Godot!"
	):
		owner.free()
		_fail("The first Rust module generation did not become active")
		return false
	owner.set(&"prefix", "Saved")

	var source_file := FileAccess.open(HOT_RELOAD_SCRIPT_PATH, FileAccess.READ)
	if source_file == null:
		owner.free()
		_fail("Could not read the Rust source used by the hot-reload test")
		return false
	var source := source_file.get_as_text()
	source_file.close()
	var changed_source := source.replace(
		'format!("{}, {name}!", self.prefix)',
		'format!("reloaded {}, {name}!", self.prefix)'
	)
	if changed_source == source:
		owner.free()
		_fail("The hot-reload test could not identify its Rust method body")
		return false
	var output_file := FileAccess.open(HOT_RELOAD_SCRIPT_PATH, FileAccess.WRITE)
	if output_file == null:
		owner.free()
		_fail("Could not update the Rust source used by the hot-reload test")
		return false
	output_file.store_string(changed_source)
	output_file.close()
	_godot_rust_plugin.call(
		&"_on_auto_check_requested",
		int(_godot_rust_plugin.call(&"current_source_revision")) + 1
	)
	if not bool(_godot_rust_plugin.call(&"_build")):
		owner.free()
		_fail("The second Rust generation failed to build")
		return false

	var reload_deadline := Time.get_ticks_msec() + 15_000
	while (
		Time.get_ticks_msec() < reload_deadline
		and (
			owner.call(&"format_greeting", "Godot")
			!= "reloaded Saved, Godot!"
		)
	):
		await get_tree().process_frame
	if (
		owner.call(&"format_greeting", "Godot")
		!= "reloaded Saved, Godot!"
	):
		owner.free()
		_fail("The Script Host did not activate the compatible Rust generation")
		return false
	if owner.get(&"prefix") != "Saved":
		owner.free()
		_fail("Rust hot reload did not preserve Inspector state")
		return false
	owner.free()
	print("GODOT_RUST_HOT_RELOAD_STATE_PRESERVED")
	return true


func _select_rust_language() -> bool:
	for selector in _dialog.find_children("*", "OptionButton", true, false):
		for index in selector.item_count:
			if selector.get_item_text(index) == "Rust":
				selector.select(index)
				selector.item_selected.emit(index)
				return true
	return false


func _verify_rust_editor_contract() -> bool:
	var script_editor := get_editor_interface().get_script_editor()
	var current_editor = script_editor.get_current_editor()
	if current_editor == null:
		_fail("The Rust script has no active editor")
		return false
	var base_editor: Control = current_editor.get_base_editor()
	var code_edit: CodeEdit
	if base_editor is CodeEdit:
		code_edit = base_editor as CodeEdit
	else:
		var code_edits := base_editor.find_children(
			"*",
			"CodeEdit",
			true,
			false
		)
		if not code_edits.is_empty():
			code_edit = code_edits[0] as CodeEdit
	if code_edit == null:
		_fail("The Rust script editor does not contain a CodeEdit")
		return false
	for delimiter in ["//", "/*", "///", "//!", "/**", "/*!"]:
		if not code_edit.has_comment_delimiter(delimiter):
			_fail(
				"Godot's Rust editor is missing comment delimiter `%s`"
				% delimiter
			)
			return false
	if not code_edit.has_string_delimiter('"'):
		_fail("Godot's Rust editor is missing its string delimiter")
		return false

	var original_source := code_edit.text
	var highlight_source := 'fn callback() { "text"; // note\n}'
	code_edit.text = highlight_source
	var highlighter := code_edit.syntax_highlighter
	if highlighter == null:
		code_edit.text = original_source
		_fail("The Rust script editor has no syntax highlighter")
		return false
	highlighter.update_cache()
	var highlighting: Dictionary = (
		highlighter.get_line_syntax_highlighting(0)
	)
	var keyword_color: Variant = _syntax_color_at(highlighting, 0)
	var identifier_color: Variant = _syntax_color_at(highlighting, 3)
	var string_color: Variant = _syntax_color_at(
		highlighting,
		highlight_source.find('"')
	)
	var comment_color: Variant = _syntax_color_at(
		highlighting,
		highlight_source.find("//")
	)
	if (
		keyword_color == null
		or identifier_color == null
		or string_color == null
		or comment_color == null
		or keyword_color == identifier_color
		or string_color == identifier_color
		or comment_color == identifier_color
	):
		code_edit.text = original_source
		_fail("Godot did not color Rust keywords, strings, and comments")
		return false
	print("GODOT_RUST_EDITOR_LANGUAGE_CONTRACT")
	print("GODOT_RUST_EDITOR_HIGHLIGHTING_READY")

	var unindented := "impl EditorCreated {\nfn callback() {\nrun();\n}\n}"
	var expected := (
		"impl EditorCreated {\n"
		+ "\tfn callback() {\n"
		+ "\t\trun();\n"
		+ "\t}\n"
		+ "}"
	)
	code_edit.text = unindented
	code_edit.deselect()
	var auto_indent_menu_found := false
	var auto_indent_invoked := false
	var editor_root := get_editor_interface().get_base_control()
	for popup in editor_root.find_children("*", "PopupMenu", true, false):
		for item_index in popup.item_count:
			var shortcut: Shortcut = popup.get_item_shortcut(item_index)
			if shortcut == null:
				continue
			for event in shortcut.events:
				if (
					event is InputEventKey
					and event.keycode == KEY_I
					and not event.shift_pressed
					and (event.ctrl_pressed or event.meta_pressed)
				):
					auto_indent_menu_found = true
					code_edit.text = unindented
					code_edit.deselect()
					popup.emit_signal(
						&"id_pressed",
						popup.get_item_id(item_index)
					)
					if code_edit.text != unindented:
						auto_indent_invoked = true
					break
			if auto_indent_invoked:
				break
		if auto_indent_invoked:
			break
	var indented := code_edit.text
	code_edit.text = original_source
	highlighter.update_cache()
	if not auto_indent_menu_found:
		_fail("Godot's Rust editor has no Auto Indent action")
		return false
	if not auto_indent_invoked:
		_fail("Godot's Auto Indent action did not update Rust source")
		return false
	if indented != expected:
		_fail(
			"Godot's Auto Indent action returned unexpected Rust source: %s"
			% indented.replace("\n", "\\n").replace("\t", "\\t")
		)
		return false
	print("GODOT_RUST_EDITOR_INDENTATION_READY")
	return true


func _syntax_color_at(highlighting: Dictionary, column: int) -> Variant:
	var selected_column := -1
	var selected_color: Variant
	for transition in highlighting:
		var transition_column := int(transition)
		if (
			transition_column <= column
			and transition_column >= selected_column
		):
			selected_column = transition_column
			selected_color = highlighting[transition].get(&"color")
	return selected_color


func _find_connection_node(node: Node, target_class: String) -> Node:
	if node.get_class() == target_class:
		return node
	for child in node.get_children():
		var found := _find_connection_node(child, target_class)
		if found != null:
			return found
	return null


func _find_signal_item(item: TreeItem, signal_name: StringName) -> TreeItem:
	if item == null:
		return null
	var metadata: Variant = item.get_metadata(0)
	if metadata is Dictionary and metadata.get(&"name") == signal_name:
		return item
	var child := item.get_first_child()
	while child != null:
		var found := _find_signal_item(child, signal_name)
		if found != null:
			return found
		child = child.get_next()
	return null


func _verify_signal_callback_generation() -> bool:
	var edited_root := get_tree().edited_scene_root
	if edited_root == null:
		_fail("The editor has no scene root for the signal callback test")
		return false
	var connection_dock := _find_connection_node(
		get_editor_interface().get_base_control(),
		"ConnectionsDock"
	)
	if connection_dock == null:
		_fail("Godot's ConnectionsDock is unavailable")
		return false
	get_editor_interface().get_selection().clear()
	get_editor_interface().get_selection().add_node(edited_root)
	connection_dock.call(&"update_tree")
	for _frame in 3:
		await get_tree().process_frame

	var signal_tree: Tree
	for child in connection_dock.get_children():
		if child is Tree:
			signal_tree = child as Tree
			break
	if signal_tree == null:
		_fail("Godot's ConnectionsDock has no signal tree")
		return false
	var signal_item := _find_signal_item(
		signal_tree.get_root(),
		&"child_entered_tree"
	)
	if signal_item == null:
		_fail("Godot did not expose Node.child_entered_tree")
		return false
	signal_tree.set_selected(signal_item, 0)
	signal_tree.item_activated.emit()
	for _frame in 3:
		await get_tree().process_frame

	var connect_dialog := _find_connection_node(
		connection_dock,
		"ConnectDialog"
	)
	if connect_dialog == null or not connect_dialog.visible:
		_fail("Activating a signal did not open Godot's Connect Dialog")
		return false
	if connect_dialog.get_ok_button().disabled:
		_fail("Godot rejected the Rust script as a signal target")
		return false
	connect_dialog.get_ok_button().pressed.emit()
	for _frame in 8:
		await get_tree().process_frame

	var current_script = (
		get_editor_interface().get_script_editor().get_current_script()
	)
	if (
		current_script == null
		or current_script.resource_path != "res://src/scripts/hello.rs"
	):
		_fail("Connecting a signal did not open the target Rust script")
		return false
	var current_editor = (
		get_editor_interface().get_script_editor().get_current_editor()
	)
	if current_editor == null:
		_fail("The generated Rust callback has no script editor")
		return false
	var base_editor: Control = current_editor.get_base_editor()
	var code_edit: CodeEdit
	if base_editor is CodeEdit:
		code_edit = base_editor as CodeEdit
	else:
		var code_edits := base_editor.find_children(
			"*",
			"CodeEdit",
			true,
			false
		)
		if not code_edits.is_empty():
			code_edit = code_edits[0] as CodeEdit
	if code_edit == null:
		_fail("The generated Rust callback has no CodeEdit")
		return false
	var source: String = code_edit.text
	if (
		"#[script]\nimpl Hello {" not in source
		or "fn _on_child_entered_tree(" not in source
		or "_node: Option<ObjectRef<Node>>" not in source
	):
		_fail("Godot's Connect Dialog did not generate a typed Rust callback")
		return false
	var source_file := FileAccess.open(
		"res://src/scripts/hello.rs",
		FileAccess.WRITE
	)
	if source_file == null:
		_fail("The generated Rust callback could not be saved")
		return false
	source_file.store_string(source)
	print("GODOT_RUST_SIGNAL_CALLBACK_CREATED")
	return true


func _print_dialog_state(stage: String) -> void:
	print(
		"GODOT_RUST_DIALOG_STATE %s visible=%s ok_disabled=%s"
		% [stage, _dialog.visible, _dialog.get_ok_button().disabled]
	)
	for selector in _dialog.find_children("*", "OptionButton", true, false):
		for index in selector.item_count:
			if selector.get_item_text(index) == "Rust":
				print(
					"GODOT_RUST_DIALOG_LANGUAGE selected=%s text=%s"
					% [selector.selected == index, selector.text]
				)
	for line_edit in _dialog.find_children("*", "LineEdit", true, false):
		if not line_edit.text.is_empty():
			print("GODOT_RUST_DIALOG_TEXT ", line_edit.text)
	print(
		"GODOT_RUST_DIALOG_FILES rs=%s gd=%s"
		% [
			FileAccess.file_exists(SCRIPT_PATH),
			FileAccess.file_exists(SCRIPT_PATH.trim_suffix(".rs") + ".gd"),
		]
	)


func _on_script_created(script) -> void:
	print("GODOT_RUST_DIALOG_SIGNAL_EMITTED")
	if script == null:
		_fail("ScriptCreateDialog emitted a null Rust script")


func _on_automatic_check_finished(revision: int, success: bool) -> void:
	if revision < _required_check_revision:
		return
	_finished_check_revision = revision
	_finished_check_success = success


func _index_contains_script(uid_text: String) -> bool:
	if not FileAccess.file_exists(INDEX_PATH):
		return false
	var index_file := FileAccess.open(INDEX_PATH, FileAccess.READ)
	if index_file == null:
		return false
	var source := index_file.get_as_text()
	return (
		"editor_created.rs" in source
		and "::EditorCreated =>" in source
		and uid_text in source
	)


func _fail(message: String) -> void:
	push_error("godot-rust editor flow test: %s" % message)
	get_tree().quit(1)
