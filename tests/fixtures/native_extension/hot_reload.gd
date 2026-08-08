@tool
extends EditorPlugin

const DESCRIPTOR := "res://godot_rs_project.gdextension"
const EDITOR_READY_FILE := "res://native-hot-reload.editor-ready"
const LOAD_FILE := "res://native-hot-reload.load"
const READY_FILE := "res://native-hot-reload.ready"
const TRIGGER_FILE := "res://native-hot-reload.trigger"

var _counter: Object
var _initialized := false
var _completed := false


func _enter_tree() -> void:
	var ready := FileAccess.open(EDITOR_READY_FILE, FileAccess.WRITE)
	if ready == null:
		_fail("could not create editor-ready marker")
		return
	ready.store_string("ready\n")
	ready.close()


func _initialize_test() -> void:
	var status := GDExtensionManager.LOAD_STATUS_ALREADY_LOADED
	if not GDExtensionManager.is_extension_loaded(DESCRIPTOR):
		status = GDExtensionManager.load_extension(DESCRIPTOR)
	if (
		status != GDExtensionManager.LOAD_STATUS_OK
		and status != GDExtensionManager.LOAD_STATUS_ALREADY_LOADED
	):
		_fail("initial extension load failed with status %d" % status)
		return
	if not _runtime_is_healthy():
		_fail("initial Native runtime health probe failed")
		return
	_counter = ClassDB.instantiate("GodotRsNativeSmoke")
	if _counter == null:
		_fail("Native class could not be instantiated before reload")
		return
	_counter.value = 64
	_counter.metadata = {"preserved": true, "generation": 1}
	if _counter.generation() != "before":
		_fail("initial Native generation returned the wrong code")
		return
	var ready := FileAccess.open(READY_FILE, FileAccess.WRITE)
	if ready == null:
		_fail("could not create Native hot-reload ready marker")
		return
	ready.store_string("ready\n")
	ready.close()
	_initialized = true


func _process(_delta: float) -> void:
	if not _initialized:
		if not FileAccess.file_exists(LOAD_FILE):
			return
		DirAccess.remove_absolute(ProjectSettings.globalize_path(LOAD_FILE))
		_initialize_test()
		return
	if _completed or not FileAccess.file_exists(TRIGGER_FILE):
		return
	_completed = true
	var status := GDExtensionManager.reload_extension(DESCRIPTOR)
	if status != GDExtensionManager.LOAD_STATUS_OK:
		_fail("Native reload failed with status %d" % status)
		return
	if not _runtime_is_healthy():
		_fail("reloaded Native runtime health probe failed")
		return
	if _counter.generation() != "after":
		_fail("existing Object did not switch to the new Native code")
		return
	if _counter.value != 64:
		_fail("Native reload did not restore the Storage property")
		return
	if _counter.metadata != {"preserved": true, "generation": 1}:
		_fail("Native reload did not restore the owned Dictionary property")
		return
	if _counter.reload_marker() != 99:
		_fail("Native reload hook did not rebuild runtime-only state")
		return
	print("GODOT_RS_NATIVE_HOT_RELOAD_OK")
	_counter.free()
	_counter = null
	get_tree().quit()


func _exit_tree() -> void:
	if _counter != null and is_instance_valid(_counter):
		_counter.free()
		_counter = null
	for path in [EDITOR_READY_FILE, LOAD_FILE, READY_FILE, TRIGGER_FILE]:
		if FileAccess.file_exists(path):
			DirAccess.remove_absolute(ProjectSettings.globalize_path(path))


func _fail(message: String) -> void:
	push_error(message)
	if _counter != null and is_instance_valid(_counter):
		_counter.free()
		_counter = null
	get_tree().quit(1)


func _runtime_is_healthy() -> bool:
	if not ClassDB.class_exists("GodotRsExtensionRuntime"):
		return false
	var runtime: Object = ClassDB.instantiate("GodotRsExtensionRuntime")
	if runtime == null or not runtime.has_method("is_healthy"):
		return false
	var healthy := bool(runtime.call("is_healthy"))
	runtime = null
	return healthy
