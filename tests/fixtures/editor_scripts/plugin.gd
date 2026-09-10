@tool
extends EditorPlugin

const SCRIPT_PATH := "res://src/scripts/editor_probe.rs"
const TOOL_PATH := "res://src/scripts/tool_probe.rs"

var _plugin
var _property_changes := 0
var _signal_value := 0
var _tool_changes := 0


func _enter_tree() -> void:
	call_deferred("_run")


func _run() -> void:
	for _frame in 5:
		await get_tree().process_frame
	_plugin = get_tree().get_first_node_in_group(&"godot_rust_editor_plugin")
	var deadline := Time.get_ticks_msec() + 30_000
	while not _plugin.is_workflow_configured() and Time.get_ticks_msec() < deadline:
		await get_tree().process_frame
	var rust := load(SCRIPT_PATH) as Script
	var owner := Node.new()
	owner.set_script(rust)
	add_child(owner)
	owner.property_list_changed.connect(func(): _property_changes += 1)
	if not _build():
		return
	if not await _wait_property(owner, "value", true):
		return
	if rust.can_instantiate() or rust.is_tool() or owner.get("value") != 3:
		_fail("A regular editor script must expose defaults without running Rust")
		return
	owner.set("value", 37)
	var discarded := Node.new()
	discarded.set_script(rust)
	discarded.free()
	print("GODOT_RUST_EDITOR_PLACEHOLDER_READY")

	var tool := load(TOOL_PATH) as Script
	var tool_owner := Node.new()
	tool_owner.set_script(tool)
	add_child(tool_owner)
	if not tool.is_tool() or tool_owner.call("generation") != 1:
		_fail("An explicit tool script did not execute in the editor")
		return
	tool_owner.set("value", 51)
	tool_owner.set("inherited", 101)
	tool_owner.property_list_changed.connect(func(): _tool_changes += 1)
	tool_owner.connect("pulse", func(value): _signal_value = value)
	tool_owner.connect("retired", func(): pass)
	var source := Node.new()
	source.add_user_signal("trigger")
	source.connect("trigger", Callable(tool_owner, "old_method"))
	var child := Node.new()
	child.name = "ReloadChild"
	tool_owner.add_child(child)
	var sibling := Node.new()
	sibling.set_script(tool)
	add_child(sibling)
	sibling.set("value", 63)
	var sibling_child := Node.new()
	sibling_child.name = "ReloadChild"
	sibling.add_child(sibling_child)
	var dormant := Node.new()
	dormant.set_script(tool)
	dormant.set("value", 79)
	var identity := tool_owner.get_instance_id()
	var resource := Resource.new()
	tool_owner.set("resource", resource)
	var values: Array[int] = [4, 8, 15]
	tool_owner.set("values", values)
	tool_owner.call("start_task")
	tool_owner.call("log_messages")
	if not await _edit_unicode(rust):
		return

	var original := FileAccess.get_file_as_string(SCRIPT_PATH)
	var added := original.replace("    ticks: i64,", "    ticks: i64,\n    #[export(default = 42)]\n    added: i64,")
	added = added.replace("impl EditorProbe {", "impl EditorProbe {\n    #[func]\n    fn new_method(&self) -> i64 { 9 }\n")
	_write(SCRIPT_PATH, added)
	var changes_before := _property_changes
	if not _build() or not await _wait_property(owner, "added", true):
		return
	if owner.get("added") != 42 or owner.get("value") != 37 or _property_changes <= changes_before:
		_fail("Adding an export did not refresh the existing placeholder and preserve its value")
		return
	var method_found := false
	for method in rust.get_script_method_list():
		if method.name == "new_method":
			method_found = true
	if not method_found or tool_owner.get("value") != 51:
		_fail("A regular script change lost metadata or another tool's state")
		return
	print("GODOT_RUST_EXPORT_ADDITION_PRESERVED")

	var tool_original := FileAccess.get_file_as_string(TOOL_PATH)
	var base_path := "res://src/scripts/tool_base.rs"
	var base_original := FileAccess.get_file_as_string(base_path)
	var tool_added := tool_original.replace("    #[export(default = 7)]", "    #[export(default = 19)]\n    added: i64,\n    #[node(\"ReloadChild\")]\n    child: NodeRef<Node>,\n    #[signal]\n    fresh: Signal<fn(value: i64)>,\n    #[export(default = 7)]")
	var callbacks := "    fn _process(&mut self, _delta: f64) { self.ticks += 1; }\n    fn _physics_process(&mut self, _delta: f64) {}\n    fn _input(&mut self, _event: InputEventRef) {}\n    fn _unhandled_input(&mut self, _event: InputEventRef) {}\n"
	tool_added = tool_added.replace("impl ToolProbe {", "impl ToolProbe {\n" + callbacks + "    #[func]\n    fn new_method(&self, #[default(9)] value: i64) -> i64 { value }\n    #[func]\n    fn child_id(&self) -> EngineResult<u64> { self.child.get_instance_id() }\n")
	tool_added = tool_added.replace("        1\n", "        3\n")
	_write(TOOL_PATH, tool_added)
	_write(base_path, base_original.replace("    inherited: i64,", "    inherited: i64,\n    #[export(default = 29)]\n    base_added: i64,"))
	_write(SCRIPT_PATH, original)
	sibling_child.name = "UnavailableDuringReload"
	_write(TOOL_PATH, tool_added.replace("        3\n", "        2\n"))
	if not _build():
		return
	await get_tree().create_timer(1.0).timeout
	if tool_owner.call("generation") != 1 or sibling.call("generation") != 1 or _has_property(tool_owner, "added") or not _has_property(owner, "added"):
		_fail("A missing node in one instance partially committed the build")
		return
	print("GODOT_RUST_TOOL_NODE_ROLLED_BACK")
	sibling_child.name = "ReloadChild"
	if not await _wait_property(tool_owner, "added", true):
		return
	if tool_owner.call("generation") != 2 or sibling.call("generation") != 2:
		_fail("Restoring a required node did not apply the pending build")
		return
	print("GODOT_RUST_TOOL_NODE_RECOVERED")
	sibling.set_process(false)
	_write(TOOL_PATH, tool_added)
	var tool_changes_before := _tool_changes
	if not _build() or not await _wait_property(tool_owner, "added", true):
		return
	deadline = Time.get_ticks_msec() + 15_000
	while tool_owner.call("generation") != 3 and Time.get_ticks_msec() < deadline:
		await get_tree().process_frame
	for _frame in 5:
		await get_tree().process_frame
	tool_owner.call("fire")
	if tool_owner.get_instance_id() != identity or tool_owner.get("value") != 51 or sibling.get("value") != 63 or tool_owner.get("inherited") != 101:
		_fail("Structural reload lost object identity or migrated fields by the wrong index")
		return
	if tool_owner.get("added") != 19 or tool_owner.get("base_added") != 29 or tool_owner.get("ready_calls") != 1 or tool_owner.get("ticks") <= 0:
		_fail("Structural reload lost defaults, inherited fields, or lifecycle behavior")
		return
	if not tool_owner.is_processing() or not tool_owner.is_physics_processing() or not tool_owner.is_processing_input() or not tool_owner.is_processing_unhandled_input():
		_fail("Added callbacks did not enable Node processing")
		return
	if _signal_value != 73 or not tool_owner.has_signal("fresh") or tool_owner.call("new_method") != 9 or tool_owner.call("child_id") != child.get_instance_id():
		_fail("Reload did not refresh signals, method defaults, or node references")
		return
	if tool_owner.get("resource") != resource or tool_owner.get("values") != values or _tool_changes <= tool_changes_before or _has_property(owner, "added"):
		_fail("Reload lost Resource/Array state or failed to refresh the Inspector")
		return
	if sibling.is_processing() or dormant.get("value") != 79 or dormant.get("ready_calls") != 0:
		_fail("Reload changed a manual processing setting or initialized a node before it was ready")
		return
	var dormant_child := Node.new()
	dormant_child.name = "ReloadChild"
	dormant.add_child(dormant_child)
	add_child(dormant)
	if dormant.get("ready_calls") != 1 or dormant.call("child_id") != dormant_child.get_instance_id():
		_fail("A reloaded node did not initialize its node reference on first ready")
		return
	print("GODOT_RUST_TOOL_STRUCTURE_RELOADED")

	tool_owner.call("start_task")
	await get_tree().process_frame

	# A changed stored type must reject the entire build, including placeholders.
	var incompatible := tool_added.replace("    #[export(default = 7)]\n    value: i64,", "    #[export(default = \"changed\")]\n    value: String,")
	_write(TOOL_PATH, incompatible.replace("        3\n", "        4\n"))
	_write(SCRIPT_PATH, added)
	if not _build():
		return
	await get_tree().create_timer(1.0).timeout
	if tool_owner.call("generation") != 3 or sibling.call("generation") != 3 or tool_owner.get("value") != 51 or _has_property(owner, "added"):
		_fail("A failed migration partially replaced the active generation")
		return
	print("GODOT_RUST_TOOL_RELOAD_ROLLED_BACK")

	# A later valid build recovers, removes obsolete connections, and stops callbacks.
	var removed := tool_original.replace("        1\n", "        5\n")
	removed = removed.replace("    #[signal]\n    retired: Signal<fn()>,\n", "")
	removed = removed.replace("    #[func]\n    fn old_method(&self) -> i64 {\n        5\n    }\n", "")
	_write(TOOL_PATH, removed)
	_write(base_path, base_original)
	if not _build() or not await _wait_property(tool_owner, "added", false):
		return
	if not await _wait_property(owner, "added", true):
		return
	tool_owner.call("fire")
	if tool_owner.call("generation") != 5 or sibling.get("value") != 63 or tool_owner.get("value") != 51 or tool_owner.get("ready_calls") != 1 or _signal_value != 73:
		_fail("A newer build did not recover without restarting lifecycle callbacks")
		return
	if tool_owner.is_processing() or tool_owner.is_physics_processing() or tool_owner.is_processing_input() or tool_owner.is_processing_unhandled_input():
		_fail("Removed callbacks left Node processing enabled")
		return
	if tool_owner.has_method("old_method") or tool_owner.has_signal("retired") or source.is_connected("trigger", Callable(tool_owner, "old_method")):
		_fail("Removed methods/signals left obsolete connections")
		return
	print("GODOT_RUST_TOOL_REMOVAL_RELOADED")

	# Explicit reset permits changing a field's stored type.
	var reset := removed.replace("    #[export(default = 7)]\n    value: i64,", "    #[export(default = \"fresh\")]\n    #[reload(skip)]\n    value: String,")
	_write(TOOL_PATH, reset.replace("        5\n", "        6\n"))
	_write(SCRIPT_PATH, original)
	if not _build() or not await _wait_property(owner, "added", false):
		return
	if tool_owner.get("value") != "fresh" or sibling.get("value") != "fresh" or tool_owner.get("ready_calls") != 1:
		_fail("Explicit reset did not initialize the changed field")
		return
	dormant.free()
	sibling.free()
	tool_owner.free()
	source.free()
	owner.free()
	print("GODOT_RUST_TOOL_RESET_RELOADED")
	get_tree().quit()


func _edit_unicode(rust: Script) -> bool:
	get_editor_interface().edit_script(rust)
	for _frame in 30:
		await get_tree().process_frame
	var editor = get_editor_interface().get_script_editor().get_current_editor()
	var base = editor.get_base_editor()
	var code: CodeEdit = base if base is CodeEdit else base.find_children("*", "CodeEdit", true, false)[0]
	var original := code.text
	for suffix in ["\nfn 测试() { let 名称 = 1; }\n", "\nfn 测试() { let 名称 = \"未完成"]:
		code.text = original + suffix
		code.text_changed.emit()
		await get_tree().create_timer(1.5).timeout
	code.text = original
	code.text_changed.emit()
	await get_tree().create_timer(1.5).timeout
	print("GODOT_RUST_UNICODE_EDIT_SURVIVED")
	return true


func _build() -> bool:
	if not _plugin._build():
		_fail("Rust Build failed")
		return false
	return true


func _has_property(owner: Object, name: String) -> bool:
	for property in owner.get_property_list():
		if property.name == name:
			return true
	return false


func _wait_property(owner: Object, name: String, present: bool) -> bool:
	var deadline := Time.get_ticks_msec() + 15_000
	while _has_property(owner, name) != present and Time.get_ticks_msec() < deadline:
		await get_tree().process_frame
	if _has_property(owner, name) != present:
		_fail("Timed out waiting for property %s, present=%s" % [name, present])
		return false
	return true


func _write(path: String, source: String) -> void:
	var file := FileAccess.open(path, FileAccess.WRITE)
	file.store_string(source)
	file.close()


func _fail(message: String) -> void:
	push_error(message)
	get_tree().quit(1)
