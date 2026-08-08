extends SceneTree


func _init() -> void:
	var loaded := ResourceLoader.load("res://sample.rs", "Script") as Script
	if loaded == null:
		_fail("ResourceLoader did not load the Rust script")
		return

	var failing := Node2D.new()
	var panicking := Node2D.new()
	var healthy := Node2D.new()
	failing.set_script(loaded)
	panicking.set_script(loaded)
	healthy.set_script(loaded)

	for invocation in 4:
		failing.call("failure_for_test")
	panicking.call("panic_for_test")
	panicking.call("panic_for_test")

	if healthy.call("sum_values", 20) != 42:
		failing.free()
		panicking.free()
		healthy.free()
		_fail("a healthy script instance stopped after failures in other instances")
		return

	failing.free()
	panicking.free()
	healthy.free()
	print("GODOT_RUST_FAULT_ISOLATION_OK")
	quit(0)


func _fail(message: String) -> void:
	printerr("GODOT_RUST_FAULT_ISOLATION_FAILURE: ", message)
	quit(1)
