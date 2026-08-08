@tool
extends EditorPlugin

const MARKER := "GODOT_RUST_ANDROID_EDITOR_SETTINGS_CONFIGURED"


func _enter_tree() -> void:
	var android_sdk := OS.get_environment("GODOT_RUST_TEST_ANDROID_SDK")
	var java_sdk := OS.get_environment("GODOT_RUST_TEST_JAVA_SDK")
	if (
		android_sdk.is_empty()
		or java_sdk.is_empty()
		or not DirAccess.dir_exists_absolute(android_sdk)
		or not DirAccess.dir_exists_absolute(java_sdk)
	):
		push_error("Android export validation received invalid SDK paths.")
		get_tree().quit(2)
		return
	var settings := get_editor_interface().get_editor_settings()
	settings.set_setting("export/android/android_sdk_path", android_sdk)
	settings.set_setting("export/android/java_sdk_path", java_sdk)
	settings.set_setting("export/android/shutdown_adb_on_exit", false)
	print(MARKER)
	call_deferred("_finish")


func _finish() -> void:
	get_tree().quit()
