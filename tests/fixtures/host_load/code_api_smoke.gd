extends SceneTree


func _init() -> void:
	_run.call_deferred()


func _run() -> void:
	var health_script := load("res://code_api/health.rs") as Script
	var watcher_script := load("res://code_api/watcher.rs") as Script
	var tuning_script := load("res://code_api/tuning.rs") as Script
	var coordinator_script := load("res://code_api/coordinator.rs") as Script
	if health_script == null or watcher_script == null or tuning_script == null or coordinator_script == null:
		_fail("fixture Rust scripts could not be loaded")
		return

	var coordinator := Node.new()
	coordinator.name = "CodeApi"
	var health := Node.new()
	health.name = "Health"
	health.set_script(health_script)
	var watcher := Node.new()
	watcher.name = "Watcher"
	watcher.set_script(watcher_script)
	coordinator.add_child(health)
	coordinator.add_child(watcher)
	coordinator.set_script(coordinator_script)
	root.add_child(coordinator)
	await process_frame

	if not coordinator.verify_calls():
		_fail("typed nodes, calls, properties, or wrong-script rejection failed")
		return
	print("GODOT_RUST_CODE_API_NODES")

	var finished_args: Array = []
	coordinator.connect(&"finished", func(amount: int, remaining: int): finished_args.assign([amount, remaining]))
	coordinator.start_watch()
	await process_frame
	await process_frame
	if health.damage(5) != 83:
		_fail("damage returned an unexpected value")
		return
	await process_frame
	await process_frame
	if not coordinator.verify_watch() or finished_args != [5, 83]:
		_fail("typed signal connection or awaited arguments were incorrect")
		return
	coordinator.disconnect_watch()
	health.damage(1)
	if not coordinator.verify_disconnected():
		_fail("dropping the signal connection did not disconnect")
		return
	print("GODOT_RUST_CODE_API_SIGNALS")

	InputMap.add_action("code_api_jump")
	if not coordinator.verify_input_defaults():
		_fail("generated Input defaults or overrides were incorrect")
		return
	InputMap.erase_action("code_api_jump")
	print("GODOT_RUST_CODE_API_DEFAULTS")

	for mode in range(4):
		var tuning := Resource.new()
		tuning.set_script(tuning_script)
		var tuning_id := tuning.get_instance_id()
		var weak: WeakRef = weakref(tuning)
		health.config = tuning
		tuning = null
		if coordinator.retain_returned_resource(mode) != tuning_id or health.config != null:
			_fail("typed resource return mode %d did not transfer the original reference" % mode)
			return
		if not _is_alive(weak) or not coordinator.verify_retained_resource():
			_fail("returned typed resource mode %d was not retained" % mode)
			return
		coordinator.release_resource()
		await process_frame
		if _is_alive(weak):
			_fail("typed resource mode %d leaked after its final owner was dropped" % mode)
			return
	print("GODOT_RUST_CODE_API_RESOURCE")

	for dynamic in [false, true]:
		var signal_resource := Resource.new()
		signal_resource.set_script(tuning_script)
		var signal_id := signal_resource.get_instance_id()
		var signal_weak: WeakRef = weakref(signal_resource)
		health.config = signal_resource
		signal_resource = null
		coordinator.start_config_watch(dynamic)
		await process_frame
		await process_frame
		if not health.send_config(dynamic) or health.config != null or not _is_alive(signal_weak):
			_fail("typed resource signal (dynamic=%s) did not retain pending arguments" % dynamic)
			return
		await process_frame
		await process_frame
		if not coordinator.verify_received_config(signal_id):
			_fail("typed signal (dynamic=%s) callback or await lost its resource" % dynamic)
			return
		coordinator.release_resource()
		coordinator.disconnect_watch()
		await process_frame
		if _is_alive(signal_weak):
			_fail("typed signal (dynamic=%s) leaked its released resource" % dynamic)
			return
	print("GODOT_RUST_CODE_API_RESOURCE_SIGNAL")

	var callable_resource := Resource.new()
	callable_resource.set_script(tuning_script)
	var callable_id := callable_resource.get_instance_id()
	var callable_weak: WeakRef = weakref(callable_resource)
	health.config = callable_resource
	callable_resource = null
	if coordinator.retain_returned_callable() != callable_id or health.config != null:
		_fail("typed Callable return did not transfer its original resource target")
		return
	await process_frame
	if not _is_alive(callable_weak) or not watcher.verify_callable():
		_fail("stored typed Callable argument lost its resource target")
		return
	watcher.release_callable()
	await process_frame
	if _is_alive(callable_weak):
		_fail("Callable leaked its released resource target")
		return
	print("GODOT_RUST_CODE_API_CALLABLE")

	if not coordinator.verify_error_context():
		_fail("typed method error lost its script, method, or original message")
		return
	print("GODOT_RUST_CODE_API_ERROR_CONTEXT")

	# Preserve the ObjectID while replacing the Rust script with another Node script.
	health.set_script(watcher_script)
	if not coordinator.rejects_replaced_script():
		_fail("a cached typed handle accepted a different script on the same object")
		return
	print("GODOT_RUST_CODE_API_REPLACED_SCRIPT")

	coordinator.free()
	await process_frame
	print("GODOT_RUST_CODE_API_OK")
	quit(0)


func _is_alive(value: WeakRef) -> bool:
	# Release the temporary strong reference before the calling coroutine resumes.
	return value.get_ref() != null


func _fail(message: String) -> void:
	push_error(message)
	quit(1)
