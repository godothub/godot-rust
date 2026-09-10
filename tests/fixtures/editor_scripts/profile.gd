extends SceneTree


func _initialize() -> void:
	EngineDebugger.profiler_enable("servers", true, [64])
	call_deferred("_run")


func _run() -> void:
	change_scene_to_file("res://main.tscn")
	for _frame in 30:
		await process_frame
	EngineDebugger.profiler_enable("servers", false)
	print("GODOT_RUST_PROFILE_COMPLETED")
	quit()
