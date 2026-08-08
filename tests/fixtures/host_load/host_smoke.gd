extends SceneTree


func _callable_target(prefix: String, value: int) -> String:
	return "%s:%d" % [prefix, value]


func _init() -> void:
	if not ClassDB.class_exists("GodotRustLanguage"):
		_fail("GodotRustLanguage is not registered")
		return
	if not ClassDB.class_exists("GodotRustScript"):
		_fail("GodotRustScript is not registered")
		return

	var rust_language: ScriptLanguage = null
	for index in Engine.get_script_language_count():
		var candidate := Engine.get_script_language(index)
		if candidate.get_class() == "GodotRustLanguage":
			rust_language = candidate
			break
	if rust_language == null:
		_fail("Rust ScriptLanguage is not registered with Engine")
		return

	var template_script := ClassDB.instantiate("GodotRustScript") as Script
	if template_script == null:
		_fail("GodotRustScript could not be created for the template save test")
		return
	var template_source := (
		"use godot_rs::prelude::*;\n\n"
		+ "#[script(base = CharacterBody2D)]\n"
		+ "pub struct PlayerController;\n"
	)
	template_script.source_code = template_source
	var template_path := "user://godot_rust_generated_template.rs"
	if ResourceSaver.save(template_script, template_path) != OK:
		_fail("Rust ResourceFormatSaver could not save a generated .rs script")
		return
	var restored_template := ResourceLoader.load(template_path, "Script") as Script
	if restored_template == null or restored_template.source_code != template_source:
		DirAccess.remove_absolute(ProjectSettings.globalize_path(template_path))
		_fail("Rust ResourceFormatLoader did not restore the generated .rs script")
		return
	DirAccess.remove_absolute(ProjectSettings.globalize_path(template_path))
	print("GODOT_RUST_TEMPLATE_OK")

	var rust_script := ClassDB.instantiate("GodotRustScript") as Script
	if rust_script == null:
		_fail("GodotRustScript could not be instantiated")
		return

	var expected := "use godot_rs::prelude::*;\n// 你好，Godot\n"
	rust_script.source_code = expected
	if rust_script.source_code != expected:
		_fail("Rust script source did not round-trip through UTF-8")
		return
	if not rust_script.has_source_code():
		_fail("Rust script did not report valid source")
		return
	if rust_script.reload() != OK:
		_fail("Rust script source could not be reloaded")
		return

	var loaded := ResourceLoader.load("res://sample.rs", "Script") as Script
	if loaded == null:
		_fail("ResourceLoader did not load res://sample.rs as Script")
		return
	var script_uid := ResourceLoader.get_resource_uid("res://sample.rs")
	if script_uid < 0 or ResourceUID.id_to_text(script_uid) != "uid://b1":
		_fail("Rust script did not preserve its Godot Resource UID file")
		return
	if "Godot ResourceLoader" not in loaded.source_code:
		_fail("ResourceLoader did not preserve the Rust source")
		return
	if not ResourceLoader.exists("res://sample.rs", "Script"):
		_fail("ResourceLoader did not report the Rust script as existing")
		return
	var owner := Node2D.new()
	var target := Node.new()
	target.name = "Target"
	owner.add_child(target)
	var target_3d := GridMap.new()
	target_3d.name = "Target3D"
	owner.add_child(target_3d)
	var target_visual := MeshInstance3D.new()
	target_visual.name = "TargetVisual"
	owner.add_child(target_visual)
	owner.set_script(loaded)
	if owner.get_script() != loaded:
		owner.free()
		_fail("Rust script could not be attached to a Godot object")
		return
	get_root().add_child(owner)
	target.owner = owner
	if not owner.has_method(&"_process"):
		owner.free()
		_fail("Rust _process was not exposed through ScriptInstance")
		return
	# Nodes added while SceneTree._init is running receive NOTIFICATION_READY
	# after initialization completes, which is when Godot enables _process.
	await process_frame
	if not owner.is_processing():
		owner.free()
		_fail("Godot did not activate the Rust _process lifecycle method")
		return
	# SceneTree emits process_frame before it dispatches Node._process. Suspend
	# through one complete frame so the Rust callback runs before later cleanup.
	await process_frame
	if not owner.has_method("sum_values"):
		owner.free()
		_fail("Compiled Rust #[func] was not reflected on the Godot object")
		return
	if owner.call("sum_values", 20, 0, 22) != 42:
		owner.free()
		_fail("Rust #[func] arguments or return value were not dispatched correctly")
		return
	if owner.call("sum_values", 20) != 42:
		owner.free()
		_fail("Rust #[func] default arguments were not applied")
		return
	if owner.call("count_variants", "tag", 42, true) != 5:
		owner.free()
		_fail("Rust #[func] variable arguments were not dispatched")
		return
	if owner.call("greet", "世界") != "你好，世界":
		owner.free()
		_fail("Rust String method arguments or return values did not preserve UTF-8")
		return
	var echoed_name: Variant = owner.call("echo_string_name", &"玩家/方法")
	if typeof(echoed_name) != TYPE_STRING_NAME or echoed_name != &"玩家/方法":
		owner.free()
		_fail("Rust StringName method values did not preserve their exact Godot type")
		return
	var translated_2d: Vector2 = owner.call(
		"translate_2d", Vector2(10.0, -4.0), Vector2(2.5, 9.0)
	)
	var translated_3d: Vector3 = owner.call(
		"translate_3d", Vector3(1.0, 2.0, 3.0), Vector3(4.0, 5.0, 6.0)
	)
	var blended_tint: Color = owner.call(
		"blend_tint", Color(0.8, 0.5, 0.25, 0.5)
	)
	var translated_integer_2d: Vector2i = owner.call(
		"translate_integer_2d", Vector2i(10, -4), Vector2i(2, 9)
	)
	var translated_integer_3d: Vector3i = owner.call(
		"translate_integer_3d", Vector3i(1, 2, 3), Vector3i(4, 5, 6)
	)
	var expected_region := Rect2(1.0, 2.0, 30.0, 40.0)
	var expected_atlas := Rect2i(3, 4, 50, 60)
	var expected_orientation := Quaternion.IDENTITY
	var expected_plane := Plane(Vector3.UP, 2.0)
	var expected_vector := Vector4(1.0, 2.0, 3.0, 4.0)
	var expected_integer_vector := Vector4i(5, 6, 7, 8)
	var expected_transform_2d := Transform2D.IDENTITY
	var expected_bounds := AABB(Vector3(1.0, 2.0, 3.0), Vector3(4.0, 5.0, 6.0))
	var expected_basis := Basis.IDENTITY
	var expected_transform_3d := Transform3D.IDENTITY
	var expected_projection := Projection.IDENTITY
	var expected_export_transform_2d := Transform2D(
		Vector2.RIGHT,
		Vector2.DOWN,
		Vector2(6.0, -3.0)
	)
	var expected_export_transform_3d := Transform3D(
		Basis.IDENTITY,
		Vector3(3.0, 4.0, 5.0)
	)
	var large_math_verified: bool = owner.call(
		"verify_large_math",
		expected_transform_2d,
		expected_bounds,
		expected_basis,
		expected_transform_3d,
		expected_projection
	)
	var echoed_transform: Transform3D = owner.call("echo_transform", expected_transform_3d)
	var astar_grid := AStarGrid2D.new()
	var grid_map := GridMap.new()
	var image := Image.new()
	var packed_bytes := PackedByteArray([0, 1, 127, 255])
	var packed_ints := PackedInt32Array([-2, 0, 4])
	var packed_wide_ints := PackedInt64Array([
		-9223372036854775807 - 1,
		42,
		9223372036854775807,
	])
	var packed_floats := PackedFloat32Array([0.25, -1.5, 8.0])
	var packed_doubles := PackedFloat64Array([
		0.125,
		-2.5,
		1.7976931348623157e308,
	])
	var packed_labels := PackedStringArray(["你好", "Godot", "玩家"])
	var packed_points_2d := PackedVector2Array([
		Vector2(1.0, 2.0),
		Vector2(-3.0, 4.0),
	])
	var packed_points_3d := PackedVector3Array([
		Vector3(1.0, 2.0, 3.0),
		Vector3(-4.0, 5.0, -6.0),
	])
	var packed_colors := PackedColorArray([
		Color(0.1, 0.2, 0.3, 0.4),
		Color(0.5, 0.6, 0.7, 0.8),
	])
	var packed_vectors4 := PackedVector4Array([
		Vector4(1.0, 2.0, 3.0, 4.0),
		Vector4(-5.0, 6.0, -7.0, 8.0),
	])
	var packed_engine_verified: bool = owner.call(
		"verify_packed_engine_api", image, astar_grid
	)
	var packed_scalars_verified: bool = owner.call(
		"verify_packed_scalars",
		packed_bytes,
		packed_ints,
		packed_wide_ints,
		packed_floats,
		packed_doubles
	)
	var packed_sequences_verified: bool = owner.call(
		"verify_packed_sequences",
		packed_labels,
		packed_points_2d,
		packed_points_3d
	)
	var echoed_packed_colors: PackedColorArray = owner.call(
		"echo_packed_colors", packed_colors
	)
	var echoed_packed_vectors4: PackedVector4Array = owner.call(
		"echo_packed_vectors4", packed_vectors4
	)
	if (
		not packed_engine_verified
		or not packed_scalars_verified
		or not packed_sequences_verified
		or echoed_packed_colors != packed_colors
		or echoed_packed_vectors4 != packed_vectors4
	):
		owner.free()
		_fail(
			"Rust packed-array values were not converted correctly"
			+ " (engine=%s, scalars=%s, sequences=%s, colors=%s, vectors4=%s)" % [
				packed_engine_verified,
				packed_scalars_verified,
				packed_sequences_verified,
				echoed_packed_colors,
				echoed_packed_vectors4,
			]
		)
		return
	var standard_callable := Callable(self, "_callable_target")
	var bound_callable := standard_callable.bind(42)
	var dynamic_dictionary := {
		"message": "你好，Godot",
		"position": Vector3(1.0, -2.0, 3.5),
		"values": [42, true, PackedByteArray([0, 127, 255])],
		"callback": bound_callable,
	}
	var dynamic_array := [
		"玩家",
		Vector2(4.0, -6.0),
		dynamic_dictionary,
	]
	var typed_vectors: Array[Vector2] = [
		Vector2(1.0, 2.0),
		Vector2(-3.0, 4.0),
	]
	var dynamic_variant_result: Variant = owner.call(
		"echo_variant", dynamic_dictionary
	)
	var dynamic_array_result: Array = owner.call("echo_array", dynamic_array)
	var typed_vector_result: Array[Vector2] = owner.call(
		"echo_vector_array", typed_vectors
	)
	var dynamic_dictionary_result: Dictionary = owner.call(
		"echo_dictionary", dynamic_dictionary
	)
	var image_array: Array[Image] = [image]
	var dynamic_variant_matches: bool = dynamic_variant_result == dynamic_dictionary
	var nested_callable_matches: bool = (
		dynamic_variant_result.callback.call("嵌套") == "嵌套:42"
	)
	var dynamic_array_matches: bool = dynamic_array_result == dynamic_array
	var typed_array_matches: bool = (
		typed_vector_result == typed_vectors
		and typed_vector_result.is_typed()
		and typed_vector_result.get_typed_builtin() == TYPE_VECTOR2
	)
	var dynamic_dictionary_matches: bool = (
		dynamic_dictionary_result == dynamic_dictionary
	)
	var refcounted_array_matches: bool = owner.call(
		"verify_refcounted_array", image_array
	)
	var dynamic_engine_matches: bool = owner.call("verify_dynamic_engine_api")
	if (
		not dynamic_variant_matches
		or not nested_callable_matches
		or not dynamic_array_matches
		or not typed_array_matches
		or not dynamic_dictionary_matches
		or not refcounted_array_matches
		or not dynamic_engine_matches
	):
		owner.free()
		_fail("Rust dynamic values or typed arrays were not converted correctly")
		return
	print("GODOT_RUST_DYNAMIC_VALUES")
	var echoed_standard: Callable = owner.call("echo_callable", standard_callable)
	var echoed_bound: Callable = owner.call("echo_callable", bound_callable)
	if echoed_standard.call("玩家", 7) != "玩家:7":
		owner.free()
		_fail("Rust Callable did not preserve standard behavior")
		return
	if echoed_bound.call("绑定") != "绑定:42":
		owner.free()
		_fail("Rust Callable did not preserve bound arguments")
		return
	if not owner.call("verify_callable_engine_api"):
		owner.free()
		_fail("Rust Callable did not pass through the generated Godot API")
		return
	print("GODOT_RUST_CALLABLES")
	var source_signal: Signal = owner.renamed
	var echoed_signal: Signal = owner.call("echo_signal", source_signal)
	if echoed_signal != source_signal or not owner.call("verify_signal_engine_api"):
		owner.free()
		_fail("Rust Signal did not preserve its owner, name, or Godot behavior")
		return
	print("GODOT_RUST_SIGNALS")
	owner.queue_redraw()
	await process_frame
	await process_frame
	if not owner.call("virtual_draw_was_called"):
		owner.free()
		_fail("Godot did not dispatch the generated Rust CanvasItemVirtual override")
		return
	print("GODOT_RUST_VIRTUAL_OVERRIDES")
	if not owner.call("verify_static_engine_api"):
		owner.free()
		_fail("Rust class-level API did not dispatch through a null receiver")
		return
	print("GODOT_RUST_STATIC_ENGINE_API")
	if not owner.call("verify_complete_generated_api"):
		owner.free()
		_fail("Rust complete generated API surface did not preserve Godot behavior")
		return
	print("GODOT_RUST_COMPLETE_GENERATED_API")
	if not owner.call("verify_vararg_engine_api"):
		owner.free()
		_fail("Rust variable-argument API did not preserve arguments or return values")
		return
	print("GODOT_RUST_VARARG_ENGINE_API")
	var navigation_map: RID = target_3d.get_world_3d().get_navigation_map()
	if (
		not translated_2d.is_equal_approx(Vector2(12.5, 5.0))
		or not translated_3d.is_equal_approx(Vector3(5.0, 7.0, 9.0))
		or not blended_tint.is_equal_approx(Color(0.2, 0.25, 0.1875, 0.5))
		or translated_integer_2d != Vector2i(12, 5)
		or translated_integer_3d != Vector3i(5, 7, 9)
		or not owner.call("verify_integer_engine_api", astar_grid, grid_map)
		or not owner.call(
			"verify_extended_math",
			expected_region,
			expected_atlas,
			expected_orientation,
			expected_plane,
			expected_vector,
			expected_integer_vector
		)
		or not large_math_verified
		or echoed_transform != expected_transform_3d
	):
		owner.free()
		_fail(
			"Rust math method arguments or return values were not converted correctly"
			+ " (large_verified=%s, echoed_transform=%s)" % [
				large_math_verified,
				echoed_transform,
			]
		)
		return
	if (
		not navigation_map.is_valid()
		or owner.call("echo_rid", navigation_map) != navigation_map
	):
		owner.free()
		_fail("Rust RID method arguments or return values did not preserve the handle")
		return
	if owner.call("mirror_node", target) != target:
		owner.free()
		_fail("Rust ObjectRef method values did not preserve Godot object identity")
		return
	if owner.call("mirror_node", null) != null:
		owner.free()
		_fail("nullable Rust ObjectRef method values did not preserve null")
		return
	var method_metadata_found := false
	var string_method_metadata_found := false
	var string_name_method_metadata_found := false
	var math_method_metadata_found := false
	var object_method_metadata_found := false
	var rid_method_metadata_found := false
	var extended_math_method_metadata_found := false
	var large_math_method_metadata_found := false
	var packed_method_metadata_found := false
	var callable_method_metadata_found := false
	var vararg_method_metadata_found := false
	for method_info in owner.get_method_list():
		if method_info.name == &"sum_values":
			method_metadata_found = (
				method_info.args.size() == 3
				and method_info.args[0].name == &"left"
				and method_info.args[0].type == TYPE_INT
				and method_info.args[1].name == &"offset"
				and method_info.args[1].type == TYPE_INT
				and method_info.args[2].name == &"right"
				and method_info.args[2].type == TYPE_INT
				and method_info.default_args == [0, 22]
				and method_info["return"].type == TYPE_INT
			)
		elif method_info.name == &"count_variants":
			vararg_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"label"
				and method_info.args[0].type == TYPE_STRING
				and method_info.default_args.is_empty()
				and method_info.flags == 21
				and method_info["return"].type == TYPE_INT
			)
		elif method_info.name == &"greet":
			string_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"name"
				and method_info.args[0].type == TYPE_STRING
				and method_info["return"].type == TYPE_STRING
			)
		elif method_info.name == &"echo_string_name":
			string_name_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"name"
				and method_info.args[0].type == TYPE_STRING_NAME
				and method_info["return"].type == TYPE_STRING_NAME
			)
		elif method_info.name == &"translate_2d":
			math_method_metadata_found = (
				method_info.args.size() == 2
				and method_info.args[0].name == &"point"
				and method_info.args[0].type == TYPE_VECTOR2
				and method_info.args[1].name == &"offset"
				and method_info.args[1].type == TYPE_VECTOR2
				and method_info["return"].type == TYPE_VECTOR2
			)
		elif method_info.name == &"mirror_node":
			object_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"node"
				and method_info.args[0].type == TYPE_OBJECT
				and method_info.args[0].class_name == &"Node"
				and method_info["return"].type == TYPE_OBJECT
				and method_info["return"].class_name == &"Node"
			)
		elif method_info.name == &"echo_rid":
			rid_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"resource"
				and method_info.args[0].type == TYPE_RID
				and method_info["return"].type == TYPE_RID
			)
		elif method_info.name == &"verify_extended_math":
			extended_math_method_metadata_found = (
				method_info.args.size() == 6
				and method_info.args[0].type == TYPE_RECT2
				and method_info.args[1].type == TYPE_RECT2I
				and method_info.args[2].type == TYPE_QUATERNION
				and method_info.args[3].type == TYPE_PLANE
				and method_info.args[4].type == TYPE_VECTOR4
				and method_info.args[5].type == TYPE_VECTOR4I
				and method_info["return"].type == TYPE_BOOL
			)
		elif method_info.name == &"verify_large_math":
			large_math_method_metadata_found = (
				method_info.args.size() == 5
				and method_info.args[0].type == TYPE_TRANSFORM2D
				and method_info.args[1].type == TYPE_AABB
				and method_info.args[2].type == TYPE_BASIS
				and method_info.args[3].type == TYPE_TRANSFORM3D
				and method_info.args[4].type == TYPE_PROJECTION
				and method_info["return"].type == TYPE_BOOL
			)
		elif method_info.name == &"verify_packed_scalars":
			packed_method_metadata_found = (
				method_info.args.size() == 5
				and method_info.args[0].type == TYPE_PACKED_BYTE_ARRAY
				and method_info.args[1].type == TYPE_PACKED_INT32_ARRAY
				and method_info.args[2].type == TYPE_PACKED_INT64_ARRAY
				and method_info.args[3].type == TYPE_PACKED_FLOAT32_ARRAY
				and method_info.args[4].type == TYPE_PACKED_FLOAT64_ARRAY
				and method_info["return"].type == TYPE_BOOL
			)
		elif method_info.name == &"echo_callable":
			callable_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"callback"
				and method_info.args[0].type == TYPE_CALLABLE
				and method_info["return"].type == TYPE_CALLABLE
			)
	if (
		not method_metadata_found
		or not string_method_metadata_found
		or not string_name_method_metadata_found
		or not math_method_metadata_found
		or not object_method_metadata_found
		or not rid_method_metadata_found
		or not extended_math_method_metadata_found
		or not large_math_method_metadata_found
		or not packed_method_metadata_found
		or not callable_method_metadata_found
		or not vararg_method_metadata_found
	):
		var method_diagnostic := (
			"base=%s string=%s string_name=%s math=%s object=%s rid=%s "
			+ "extended=%s large=%s packed=%s callable=%s vararg=%s"
		) % [
			method_metadata_found,
			string_method_metadata_found,
			string_name_method_metadata_found,
			math_method_metadata_found,
			object_method_metadata_found,
			rid_method_metadata_found,
			extended_math_method_metadata_found,
			large_math_method_metadata_found,
			packed_method_metadata_found,
			callable_method_metadata_found,
			vararg_method_metadata_found,
		]
		owner.free()
		_fail(
			"Rust #[func] method metadata was not exposed through ScriptInstance: "
			+ method_diagnostic
		)
		return
	var script_method_metadata_found := false
	var script_string_method_metadata_found := false
	var script_string_name_method_metadata_found := false
	var script_math_method_metadata_found := false
	var script_object_method_metadata_found := false
	var script_rid_method_metadata_found := false
	var script_packed_method_metadata_found := false
	var script_callable_method_metadata_found := false
	var script_vararg_method_metadata_found := false
	var lifecycle_method_metadata_found := false
	for method_info in loaded.get_script_method_list():
		if method_info.name == &"sum_values":
			script_method_metadata_found = (
				method_info.args.size() == 3
				and method_info.args[0].name == &"left"
				and method_info.args[0].type == TYPE_INT
				and method_info.args[1].name == &"offset"
				and method_info.args[1].type == TYPE_INT
				and method_info.args[2].name == &"right"
				and method_info.args[2].type == TYPE_INT
				and method_info.default_args == [0, 22]
				and method_info.flags == 5
				and method_info["return"].type == TYPE_INT
			)
		elif method_info.name == &"count_variants":
			script_vararg_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"label"
				and method_info.args[0].type == TYPE_STRING
				and method_info.default_args.is_empty()
				and method_info.flags == 21
				and method_info["return"].type == TYPE_INT
			)
		elif method_info.name == &"greet":
			script_string_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].name == &"name"
				and method_info.args[0].type == TYPE_STRING
				and method_info["return"].type == TYPE_STRING
			)
		elif method_info.name == &"echo_string_name":
			script_string_name_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].type == TYPE_STRING_NAME
				and method_info["return"].type == TYPE_STRING_NAME
			)
		elif method_info.name == &"translate_3d":
			script_math_method_metadata_found = (
				method_info.args.size() == 2
				and method_info.args[0].type == TYPE_VECTOR3
				and method_info.args[1].type == TYPE_VECTOR3
				and method_info["return"].type == TYPE_VECTOR3
			)
		elif method_info.name == &"mirror_node":
			script_object_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].type == TYPE_OBJECT
				and method_info.args[0].class_name == &"Node"
				and method_info["return"].type == TYPE_OBJECT
				and method_info["return"].class_name == &"Node"
			)
		elif method_info.name == &"echo_rid":
			script_rid_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].type == TYPE_RID
				and method_info["return"].type == TYPE_RID
			)
		elif method_info.name == &"echo_packed_colors":
			script_packed_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].type == TYPE_PACKED_COLOR_ARRAY
				and method_info["return"].type == TYPE_PACKED_COLOR_ARRAY
			)
		elif method_info.name == &"echo_callable":
			script_callable_method_metadata_found = (
				method_info.args.size() == 1
				and method_info.args[0].type == TYPE_CALLABLE
				and method_info["return"].type == TYPE_CALLABLE
			)
		elif method_info.name == &"_ready":
			lifecycle_method_metadata_found = (
				method_info.args.is_empty()
				and method_info["return"].type == TYPE_NIL
			)
	if (
		not script_method_metadata_found
		or not script_string_method_metadata_found
		or not script_string_name_method_metadata_found
		or not script_math_method_metadata_found
		or not script_object_method_metadata_found
		or not script_rid_method_metadata_found
		or not script_packed_method_metadata_found
		or not script_callable_method_metadata_found
		or not script_vararg_method_metadata_found
		or not lifecycle_method_metadata_found
	):
		owner.free()
		_fail("Rust method metadata was not exposed through the Script resource")
		return
	var signal_metadata_found := false
	var string_signal_metadata_found := false
	var string_name_signal_metadata_found := false
	var math_signal_metadata_found := false
	var integer_signal_metadata_found := false
	var rid_signal_metadata_found := false
	var extended_math_signal_metadata_found := false
	var transform_signal_metadata_found := false
	var packed_signal_metadata_found := false
	for signal_info in loaded.get_script_signal_list():
		if signal_info.name == &"counter_changed":
			signal_metadata_found = (
				signal_info.args.size() == 2
				and signal_info.args[0].name == &"old_value"
				and signal_info.args[0].type == TYPE_INT
				and signal_info.args[1].name == &"new_value"
				and signal_info.args[1].type == TYPE_INT
			)
		elif signal_info.name == &"message_changed":
			string_signal_metadata_found = (
				signal_info.args.size() == 1
				and signal_info.args[0].name == &"message"
				and signal_info.args[0].type == TYPE_STRING
			)
		elif signal_info.name == &"name_changed":
			string_name_signal_metadata_found = (
				signal_info.args.size() == 1
				and signal_info.args[0].name == &"name"
				and signal_info.args[0].type == TYPE_STRING_NAME
			)
		elif signal_info.name == &"motion_changed":
			math_signal_metadata_found = (
				signal_info.args.size() == 3
				and signal_info.args[0].name == &"velocity"
				and signal_info.args[0].type == TYPE_VECTOR2
				and signal_info.args[1].name == &"target_position"
				and signal_info.args[1].type == TYPE_VECTOR3
				and signal_info.args[2].name == &"tint"
				and signal_info.args[2].type == TYPE_COLOR
			)
		elif signal_info.name == &"integer_motion_changed":
			integer_signal_metadata_found = (
				signal_info.args.size() == 2
				and signal_info.args[0].name == &"grid_cell"
				and signal_info.args[0].type == TYPE_VECTOR2I
				and signal_info.args[1].name == &"voxel_cell"
				and signal_info.args[1].type == TYPE_VECTOR3I
			)
		elif signal_info.name == &"rid_changed":
			rid_signal_metadata_found = (
				signal_info.args.size() == 1
				and signal_info.args[0].name == &"resource"
				and signal_info.args[0].type == TYPE_RID
			)
		elif signal_info.name == &"extended_math_changed":
			extended_math_signal_metadata_found = (
				signal_info.args.size() == 3
				and signal_info.args[0].type == TYPE_RECT2
				and signal_info.args[1].type == TYPE_QUATERNION
				and signal_info.args[2].type == TYPE_VECTOR4
			)
		elif signal_info.name == &"transform_changed":
			transform_signal_metadata_found = (
				signal_info.args.size() == 1
				and signal_info.args[0].type == TYPE_TRANSFORM3D
			)
		elif signal_info.name == &"packed_values_changed":
			packed_signal_metadata_found = (
				signal_info.args.size() == 2
				and signal_info.args[0].name == &"labels"
				and signal_info.args[0].type == TYPE_PACKED_STRING_ARRAY
				and signal_info.args[1].name == &"points"
				and signal_info.args[1].type == TYPE_PACKED_VECTOR3_ARRAY
			)
	if (
		not loaded.has_script_signal(&"counter_changed")
		or not loaded.has_script_signal(&"message_changed")
		or not loaded.has_script_signal(&"name_changed")
		or not loaded.has_script_signal(&"motion_changed")
		or not loaded.has_script_signal(&"integer_motion_changed")
		or not loaded.has_script_signal(&"rid_changed")
		or not loaded.has_script_signal(&"extended_math_changed")
		or not loaded.has_script_signal(&"transform_changed")
		or not loaded.has_script_signal(&"packed_values_changed")
		or not owner.has_signal(&"counter_changed")
		or not owner.has_signal(&"message_changed")
		or not owner.has_signal(&"name_changed")
		or not owner.has_signal(&"motion_changed")
		or not owner.has_signal(&"integer_motion_changed")
		or not owner.has_signal(&"rid_changed")
		or not owner.has_signal(&"extended_math_changed")
		or not owner.has_signal(&"transform_changed")
		or not owner.has_signal(&"packed_values_changed")
		or not signal_metadata_found
		or not string_signal_metadata_found
		or not string_name_signal_metadata_found
		or not math_signal_metadata_found
		or not integer_signal_metadata_found
		or not rid_signal_metadata_found
		or not extended_math_signal_metadata_found
		or not transform_signal_metadata_found
		or not packed_signal_metadata_found
	):
		owner.free()
		_fail("Rust #[signal] metadata was not exposed to Godot")
		return
	var property_metadata_found := false
	var property_group_found := false
	var text_group_found := false
	var math_group_found := false
	var objects_group_found := false
	var behavior_group_found := false
	var message_property_found := false
	var scene_path_property_found := false
	var display_name_property_found := false
	var node_path_property_found := false
	var packed_property_found := false
	var typed_array_property_found := false
	var dictionary_property_found := false
	var node_object_property_found := false
	var resource_property_found := false
	var enum_property_found := false
	var flags_property_found := false
	var vector2_property_found := false
	var vector2i_property_found := false
	var vector3_property_found := false
	var vector3i_property_found := false
	var color_property_found := false
	var extended_property_types_found := 0
	var large_property_types_found := 0
	for property_info in loaded.get_script_property_list():
		if (
			property_info.name == &"State"
			and property_info.usage == PROPERTY_USAGE_GROUP
		):
			property_group_found = true
		elif (
			property_info.name == &"Text"
			and property_info.usage == PROPERTY_USAGE_GROUP
		):
			text_group_found = true
		elif (
			property_info.name == &"Math"
			and property_info.usage == PROPERTY_USAGE_GROUP
		):
			math_group_found = true
		elif (
			property_info.name == &"Objects"
			and property_info.usage == PROPERTY_USAGE_GROUP
		):
			objects_group_found = true
		elif (
			property_info.name == &"Behavior"
			and property_info.usage == PROPERTY_USAGE_GROUP
		):
			behavior_group_found = true
		elif property_info.name == &"counter":
			property_metadata_found = (
				property_info.type == TYPE_INT
				and property_info.hint == PROPERTY_HINT_RANGE
				and property_info.hint_string == "0,100,1"
				and (
					property_info.usage & PROPERTY_USAGE_SCRIPT_VARIABLE
				) != 0
			)
		elif property_info.name == &"message":
			message_property_found = (
				property_info.type == TYPE_STRING
				and property_info.hint == PROPERTY_HINT_MULTILINE_TEXT
				and property_info.hint_string == ""
			)
		elif property_info.name == &"scene_path":
			scene_path_property_found = (
				property_info.type == TYPE_STRING
				and property_info.hint == PROPERTY_HINT_FILE
				and property_info.hint_string == "*.tscn"
			)
		elif property_info.name == &"display_name":
			display_name_property_found = (
				property_info.type == TYPE_STRING_NAME
				and property_info.hint == PROPERTY_HINT_NONE
			)
		elif property_info.name == &"navigation_path":
			node_path_property_found = (
				property_info.type == TYPE_NODE_PATH
				and property_info.hint == PROPERTY_HINT_NONE
			)
		elif property_info.name == &"labels":
			packed_property_found = property_info.type == TYPE_PACKED_STRING_ARRAY
		elif property_info.name == &"checkpoints":
			typed_array_property_found = (
				property_info.type == TYPE_ARRAY
				and property_info.hint == PROPERTY_HINT_TYPE_STRING
				and property_info.hint_string == "5:"
			)
		elif property_info.name == &"settings":
			dictionary_property_found = property_info.type == TYPE_DICTIONARY
		elif property_info.name == &"selected_node":
			node_object_property_found = (
				property_info.type == TYPE_OBJECT
				and property_info.hint == PROPERTY_HINT_NODE_TYPE
				and property_info.hint_string == "Node"
				and (
					property_info.usage & PROPERTY_USAGE_NODE_PATH_FROM_SCENE_ROOT
				) != 0
			)
		elif property_info.name == &"gradient":
			resource_property_found = (
				property_info.type == TYPE_OBJECT
				and property_info.hint == PROPERTY_HINT_RESOURCE_TYPE
				and property_info.hint_string == "Gradient"
			)
		elif property_info.name == &"worker_mode":
			enum_property_found = (
				property_info.type == TYPE_INT
				and property_info.hint == PROPERTY_HINT_ENUM
				and property_info.hint_string
				== "Inherit:0,Pausable:1,When Paused:2,Always:3,Disabled:4"
			)
		elif property_info.name == &"thread_messages":
			flags_property_found = (
				property_info.type == TYPE_INT
				and property_info.hint == PROPERTY_HINT_FLAGS
				and property_info.hint_string
				== "Messages:1,Messages Physics:2,Messages All:3"
			)
		elif property_info.name == &"velocity":
			vector2_property_found = (
				property_info.type == TYPE_VECTOR2
				and property_info.hint == PROPERTY_HINT_NONE
			)
		elif property_info.name == &"target_position":
			vector3_property_found = (
				property_info.type == TYPE_VECTOR3
				and property_info.hint == PROPERTY_HINT_NONE
			)
		elif property_info.name == &"grid_cell":
			vector2i_property_found = property_info.type == TYPE_VECTOR2I
		elif property_info.name == &"voxel_cell":
			vector3i_property_found = property_info.type == TYPE_VECTOR3I
		elif property_info.name == &"viewport_region":
			extended_property_types_found += int(property_info.type == TYPE_RECT2)
		elif property_info.name == &"atlas_region":
			extended_property_types_found += int(property_info.type == TYPE_RECT2I)
		elif property_info.name == &"orientation":
			extended_property_types_found += int(property_info.type == TYPE_QUATERNION)
		elif property_info.name == &"clipping_plane":
			extended_property_types_found += int(property_info.type == TYPE_PLANE)
		elif property_info.name == &"floating_vector":
			extended_property_types_found += int(property_info.type == TYPE_VECTOR4)
		elif property_info.name == &"integer_vector":
			extended_property_types_found += int(property_info.type == TYPE_VECTOR4I)
		elif property_info.name == &"canvas_transform":
			large_property_types_found += int(property_info.type == TYPE_TRANSFORM2D)
		elif property_info.name == &"bounds":
			large_property_types_found += int(property_info.type == TYPE_AABB)
		elif property_info.name == &"local_basis":
			large_property_types_found += int(property_info.type == TYPE_BASIS)
		elif property_info.name == &"world_transform":
			large_property_types_found += int(property_info.type == TYPE_TRANSFORM3D)
		elif property_info.name == &"camera_projection":
			large_property_types_found += int(property_info.type == TYPE_PROJECTION)
		elif property_info.name == &"tint":
			color_property_found = (
				property_info.type == TYPE_COLOR
				and property_info.hint == PROPERTY_HINT_COLOR_NO_ALPHA
			)
	if (
		not property_group_found
		or not text_group_found
		or not math_group_found
		or not objects_group_found
		or not behavior_group_found
		or not property_metadata_found
		or not message_property_found
		or not scene_path_property_found
		or not display_name_property_found
		or not node_path_property_found
		or not packed_property_found
		or not typed_array_property_found
		or not dictionary_property_found
		or not node_object_property_found
		or not resource_property_found
		or not enum_property_found
		or not flags_property_found
		or not vector2_property_found
		or not vector2i_property_found
		or not vector3_property_found
		or not vector3i_property_found
		or not color_property_found
		or extended_property_types_found != 6
		or large_property_types_found != 5
	):
		owner.free()
		_fail("Rust #[export] schema was not exposed through the Script resource")
		return
	var default_velocity: Vector2 = loaded.get_property_default_value(&"velocity")
	var default_target_position: Vector3 = loaded.get_property_default_value(&"target_position")
	var default_grid_cell: Vector2i = loaded.get_property_default_value(&"grid_cell")
	var default_voxel_cell: Vector3i = loaded.get_property_default_value(&"voxel_cell")
	var default_tint: Color = loaded.get_property_default_value(&"tint")
	var default_region: Rect2 = loaded.get_property_default_value(&"viewport_region")
	var default_atlas: Rect2i = loaded.get_property_default_value(&"atlas_region")
	var default_orientation: Quaternion = loaded.get_property_default_value(&"orientation")
	var default_plane: Plane = loaded.get_property_default_value(&"clipping_plane")
	var default_vector: Vector4 = loaded.get_property_default_value(&"floating_vector")
	var default_integer_vector: Vector4i = loaded.get_property_default_value(&"integer_vector")
	var default_canvas_transform: Transform2D = (
		loaded.get_property_default_value(&"canvas_transform")
	)
	var default_bounds: AABB = loaded.get_property_default_value(&"bounds")
	var default_basis: Basis = loaded.get_property_default_value(&"local_basis")
	var default_world_transform: Transform3D = (
		loaded.get_property_default_value(&"world_transform")
	)
	var default_projection: Projection = (
		loaded.get_property_default_value(&"camera_projection")
	)
	var default_checkpoints: Array[Vector2] = (
		loaded.get_property_default_value(&"checkpoints")
	)
	if (
		loaded.get_property_default_value(&"counter") != 3
		or loaded.get_property_default_value(&"message") != "你好，Godot"
		or loaded.get_property_default_value(&"scene_path") != "res://main.tscn"
		or typeof(loaded.get_property_default_value(&"display_name")) != TYPE_STRING_NAME
		or loaded.get_property_default_value(&"display_name") != &"玩家/默认"
		or loaded.get_property_default_value(&"navigation_path") != NodePath("../Target")
		or loaded.get_property_default_value(&"labels") != PackedStringArray()
		or not default_checkpoints.is_typed()
		or default_checkpoints.get_typed_builtin() != TYPE_VECTOR2
		or not default_checkpoints.is_empty()
		or loaded.get_property_default_value(&"settings") != {}
		or loaded.get_property_default_value(&"selected_node") != null
		or loaded.get_property_default_value(&"gradient") != null
		or loaded.get_property_default_value(&"worker_mode") != 3
		or loaded.get_property_default_value(&"thread_messages") != 0
		or not default_velocity.is_equal_approx(Vector2(12.0, -4.0))
		or not default_target_position.is_equal_approx(Vector3(1.0, 2.0, 3.0))
		or default_grid_cell != Vector2i(4, -6)
		or default_voxel_cell != Vector3i(7, 8, -9)
		or not default_tint.is_equal_approx(Color(0.25, 0.5, 0.75, 1.0))
		or default_region != expected_region
		or default_atlas != expected_atlas
		or default_orientation != expected_orientation
		or default_plane != expected_plane
		or default_vector != expected_vector
		or default_integer_vector != expected_integer_vector
		or default_canvas_transform != expected_export_transform_2d
		or default_bounds != expected_bounds
		or default_basis != expected_basis
		or default_world_transform != expected_export_transform_3d
		or default_projection != expected_projection
	):
		owner.free()
		_fail(
			"Rust #[export] default value was not exposed through the Script resource"
			+ " (path=%s, labels=%s, checkpoints=%s typed=%s/%s, settings=%s)" % [
				loaded.get_property_default_value(&"navigation_path"),
				loaded.get_property_default_value(&"labels"),
				default_checkpoints,
				default_checkpoints.is_typed(),
				default_checkpoints.get_typed_builtin(),
				loaded.get_property_default_value(&"settings"),
			]
		)
		return
	var instance_property_found := false
	var instance_string_property_found := false
	var instance_string_name_property_found := false
	var instance_node_path_property_found := false
	var instance_packed_property_found := false
	var instance_typed_array_property_found := false
	var instance_dictionary_property_found := false
	var instance_node_object_property_found := false
	var instance_resource_property_found := false
	var instance_enum_property_found := false
	var instance_flags_property_found := false
	var instance_vector2_property_found := false
	var instance_vector2i_property_found := false
	var instance_vector3_property_found := false
	var instance_vector3i_property_found := false
	var instance_color_property_found := false
	var instance_extended_types_found := 0
	var instance_large_types_found := 0
	for property_info in owner.get_property_list():
		if property_info.name == &"counter":
			instance_property_found = (
				property_info.type == TYPE_INT
				and property_info.hint == PROPERTY_HINT_RANGE
				and property_info.hint_string == "0,100,1"
			)
		elif property_info.name == &"message":
			instance_string_property_found = (
				property_info.type == TYPE_STRING
				and property_info.hint == PROPERTY_HINT_MULTILINE_TEXT
			)
		elif property_info.name == &"display_name":
			instance_string_name_property_found = (
				property_info.type == TYPE_STRING_NAME
				and property_info.hint == PROPERTY_HINT_NONE
			)
		elif property_info.name == &"navigation_path":
			instance_node_path_property_found = property_info.type == TYPE_NODE_PATH
		elif property_info.name == &"labels":
			instance_packed_property_found = (
				property_info.type == TYPE_PACKED_STRING_ARRAY
			)
		elif property_info.name == &"checkpoints":
			instance_typed_array_property_found = (
				property_info.type == TYPE_ARRAY
				and property_info.hint == PROPERTY_HINT_TYPE_STRING
				and property_info.hint_string == "5:"
			)
		elif property_info.name == &"settings":
			instance_dictionary_property_found = property_info.type == TYPE_DICTIONARY
		elif property_info.name == &"selected_node":
			instance_node_object_property_found = (
				property_info.type == TYPE_OBJECT
				and property_info.hint == PROPERTY_HINT_NODE_TYPE
				and property_info.hint_string == "Node"
			)
		elif property_info.name == &"gradient":
			instance_resource_property_found = (
				property_info.type == TYPE_OBJECT
				and property_info.hint == PROPERTY_HINT_RESOURCE_TYPE
				and property_info.hint_string == "Gradient"
			)
		elif property_info.name == &"worker_mode":
			instance_enum_property_found = (
				property_info.type == TYPE_INT
				and property_info.hint == PROPERTY_HINT_ENUM
				and property_info.hint_string
				== "Inherit:0,Pausable:1,When Paused:2,Always:3,Disabled:4"
			)
		elif property_info.name == &"thread_messages":
			instance_flags_property_found = (
				property_info.type == TYPE_INT
				and property_info.hint == PROPERTY_HINT_FLAGS
				and property_info.hint_string
				== "Messages:1,Messages Physics:2,Messages All:3"
			)
		elif property_info.name == &"velocity":
			instance_vector2_property_found = property_info.type == TYPE_VECTOR2
		elif property_info.name == &"target_position":
			instance_vector3_property_found = property_info.type == TYPE_VECTOR3
		elif property_info.name == &"grid_cell":
			instance_vector2i_property_found = property_info.type == TYPE_VECTOR2I
		elif property_info.name == &"voxel_cell":
			instance_vector3i_property_found = property_info.type == TYPE_VECTOR3I
		elif property_info.name == &"viewport_region":
			instance_extended_types_found += int(property_info.type == TYPE_RECT2)
		elif property_info.name == &"atlas_region":
			instance_extended_types_found += int(property_info.type == TYPE_RECT2I)
		elif property_info.name == &"orientation":
			instance_extended_types_found += int(property_info.type == TYPE_QUATERNION)
		elif property_info.name == &"clipping_plane":
			instance_extended_types_found += int(property_info.type == TYPE_PLANE)
		elif property_info.name == &"floating_vector":
			instance_extended_types_found += int(property_info.type == TYPE_VECTOR4)
		elif property_info.name == &"integer_vector":
			instance_extended_types_found += int(property_info.type == TYPE_VECTOR4I)
		elif property_info.name == &"canvas_transform":
			instance_large_types_found += int(property_info.type == TYPE_TRANSFORM2D)
		elif property_info.name == &"bounds":
			instance_large_types_found += int(property_info.type == TYPE_AABB)
		elif property_info.name == &"local_basis":
			instance_large_types_found += int(property_info.type == TYPE_BASIS)
		elif property_info.name == &"world_transform":
			instance_large_types_found += int(property_info.type == TYPE_TRANSFORM3D)
		elif property_info.name == &"camera_projection":
			instance_large_types_found += int(property_info.type == TYPE_PROJECTION)
		elif property_info.name == &"tint":
			instance_color_property_found = (
				property_info.type == TYPE_COLOR
				and property_info.hint == PROPERTY_HINT_COLOR_NO_ALPHA
			)
	var initial_velocity: Vector2 = owner.get(&"velocity")
	var initial_target_position: Vector3 = owner.get(&"target_position")
	var initial_grid_cell: Vector2i = owner.get(&"grid_cell")
	var initial_voxel_cell: Vector3i = owner.get(&"voxel_cell")
	var initial_tint: Color = owner.get(&"tint")
	var initial_region: Rect2 = owner.get(&"viewport_region")
	var initial_atlas: Rect2i = owner.get(&"atlas_region")
	var initial_orientation: Quaternion = owner.get(&"orientation")
	var initial_plane: Plane = owner.get(&"clipping_plane")
	var initial_vector: Vector4 = owner.get(&"floating_vector")
	var initial_integer_vector: Vector4i = owner.get(&"integer_vector")
	var initial_canvas_transform: Transform2D = owner.get(&"canvas_transform")
	var initial_bounds: AABB = owner.get(&"bounds")
	var initial_basis: Basis = owner.get(&"local_basis")
	var initial_world_transform: Transform3D = owner.get(&"world_transform")
	var initial_projection: Projection = owner.get(&"camera_projection")
	var initial_checkpoints: Array[Vector2] = owner.get(&"checkpoints")
	if (
		not instance_property_found
		or not instance_string_property_found
		or not instance_string_name_property_found
		or not instance_node_path_property_found
		or not instance_packed_property_found
		or not instance_typed_array_property_found
		or not instance_dictionary_property_found
		or not instance_node_object_property_found
		or not instance_resource_property_found
		or not instance_enum_property_found
		or not instance_flags_property_found
		or not instance_vector2_property_found
		or not instance_vector2i_property_found
		or not instance_vector3_property_found
		or not instance_vector3i_property_found
		or not instance_color_property_found
		or instance_extended_types_found != 6
		or instance_large_types_found != 5
		or owner.get(&"counter") != 3
		or owner.get(&"message") != "你好，Godot"
		or typeof(owner.get(&"display_name")) != TYPE_STRING_NAME
		or owner.get(&"display_name") != &"玩家/默认"
		or owner.get(&"navigation_path") != NodePath("../Target")
		or owner.get(&"labels") != PackedStringArray()
		or not initial_checkpoints.is_typed()
		or initial_checkpoints.get_typed_builtin() != TYPE_VECTOR2
		or not initial_checkpoints.is_empty()
		or owner.get(&"settings") != {}
		or owner.get(&"selected_node") != null
		or owner.get(&"gradient") != null
		or owner.get(&"worker_mode") != 3
		or owner.get(&"thread_messages") != 0
		or not initial_velocity.is_equal_approx(Vector2(12.0, -4.0))
		or not initial_target_position.is_equal_approx(Vector3(1.0, 2.0, 3.0))
		or initial_grid_cell != Vector2i(4, -6)
		or initial_voxel_cell != Vector3i(7, 8, -9)
		or not initial_tint.is_equal_approx(Color(0.25, 0.5, 0.75, 1.0))
		or initial_region != expected_region
		or initial_atlas != expected_atlas
		or initial_orientation != expected_orientation
		or initial_plane != expected_plane
		or initial_vector != expected_vector
		or initial_integer_vector != expected_integer_vector
		or initial_canvas_transform != expected_export_transform_2d
		or initial_bounds != expected_bounds
		or initial_basis != expected_basis
		or initial_world_transform != expected_export_transform_3d
		or initial_projection != expected_projection
	):
		var export_diagnostic := (
			"enum_meta=%s flags_meta=%s worker=%s messages=%s "
			+ "node_meta=%s resource_meta=%s"
		) % [
			instance_enum_property_found,
			instance_flags_property_found,
			owner.get(&"worker_mode"),
			owner.get(&"thread_messages"),
			instance_node_object_property_found,
			instance_resource_property_found,
		]
		owner.free()
		_fail("Rust #[export] was not readable through the ScriptInstance: " + export_diagnostic)
		return
	owner.set(&"counter", 77)
	if owner.get(&"counter") != 77 or owner.call("current_counter") != 77:
		owner.free()
		_fail("Inspector property writes did not update the native Rust field")
		return
	var emitted_arguments: Array[int] = []
	var on_counter_changed := func(old_value: int, new_value: int) -> void:
		emitted_arguments.append(old_value)
		emitted_arguments.append(new_value)
	if owner.connect(&"counter_changed", on_counter_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust signal")
		return
	owner.call("update_counter", 88)
	if emitted_arguments != [77, 88] or owner.call("current_counter") != 88:
		owner.free()
		_fail("Signal::emit did not synchronously deliver the typed Rust arguments")
		return
	var emitted_messages: Array[String] = []
	var on_message_changed := func(message: String) -> void:
		emitted_messages.append(message)
	if owner.connect(&"message_changed", on_message_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust String signal")
		return
	owner.call("update_message", "场景已保存")
	if (
		emitted_messages != ["场景已保存"]
		or owner.call("current_message") != "场景已保存"
		or owner.get(&"message") != "场景已保存"
	):
		owner.free()
		_fail("String property, method, or signal values did not stay consistent")
		return
	var emitted_names: Array[StringName] = []
	var on_name_changed := func(name: StringName) -> void:
		emitted_names.append(name)
	if owner.connect(&"name_changed", on_name_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust StringName signal")
		return
	owner.call("update_display_name", &"玩家/信号")
	if (
		emitted_names != [&"玩家/信号"]
		or typeof(owner.call("current_display_name")) != TYPE_STRING_NAME
		or owner.call("current_display_name") != &"玩家/信号"
		or typeof(owner.get(&"display_name")) != TYPE_STRING_NAME
		or owner.get(&"display_name") != &"玩家/信号"
	):
		owner.free()
		_fail("StringName property, method, or signal values did not stay consistent")
		return
	var emitted_velocities: Array[Vector2] = []
	var emitted_positions: Array[Vector3] = []
	var emitted_tints: Array[Color] = []
	var on_motion_changed := func(
		velocity: Vector2, target_position: Vector3, tint: Color
	) -> void:
		emitted_velocities.append(velocity)
		emitted_positions.append(target_position)
		emitted_tints.append(tint)
	if owner.connect(&"motion_changed", on_motion_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust math signal")
		return
	owner.call(
		"update_motion",
		Vector2(3.0, 4.0),
		Vector3(5.0, 6.0, 7.0),
		Color(0.8, 0.6, 0.4, 0.2)
	)
	if (
		emitted_velocities.size() != 1
		or emitted_positions.size() != 1
		or emitted_tints.size() != 1
		or not emitted_velocities[0].is_equal_approx(Vector2(3.0, 4.0))
		or not emitted_positions[0].is_equal_approx(Vector3(5.0, 6.0, 7.0))
		or not emitted_tints[0].is_equal_approx(Color(0.8, 0.6, 0.4, 0.2))
	):
		owner.free()
		_fail("Math Signal::emit did not preserve its typed Rust arguments")
		return
	var emitted_grid_cells: Array[Vector2i] = []
	var emitted_voxel_cells: Array[Vector3i] = []
	var on_integer_motion_changed := func(grid_cell: Vector2i, voxel_cell: Vector3i) -> void:
		emitted_grid_cells.append(grid_cell)
		emitted_voxel_cells.append(voxel_cell)
	if owner.connect(&"integer_motion_changed", on_integer_motion_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust integer vector signal")
		return
	owner.call("update_integer_motion", Vector2i(-3, 12), Vector3i(4, -5, 6))
	if (
		emitted_grid_cells != [Vector2i(-3, 12)]
		or emitted_voxel_cells != [Vector3i(4, -5, 6)]
	):
		owner.free()
		_fail("Integer vector Signal::emit did not preserve its typed Rust arguments")
		return
	var emitted_regions: Array[Rect2] = []
	var emitted_orientations: Array[Quaternion] = []
	var emitted_vectors: Array[Vector4] = []
	var on_extended_math_changed := func(
		region: Rect2, orientation: Quaternion, vector: Vector4
	) -> void:
		emitted_regions.append(region)
		emitted_orientations.append(orientation)
		emitted_vectors.append(vector)
	if owner.connect(&"extended_math_changed", on_extended_math_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust extended math signal")
		return
	owner.call(
		"emit_extended_math",
		expected_region,
		expected_orientation,
		expected_vector
	)
	if (
		emitted_regions != [expected_region]
		or emitted_orientations != [expected_orientation]
		or emitted_vectors != [expected_vector]
	):
		owner.free()
		_fail("Extended math Signal::emit did not preserve its typed Rust arguments")
		return
	var emitted_transforms: Array[Transform3D] = []
	var on_transform_changed := func(transform: Transform3D) -> void:
		emitted_transforms.append(transform)
	if owner.connect(&"transform_changed", on_transform_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust transform signal")
		return
	owner.call("emit_transform", expected_transform_3d)
	if emitted_transforms != [expected_transform_3d]:
		owner.free()
		_fail("Transform Signal::emit did not preserve its typed Rust argument")
		return
	var emitted_packed_labels: Array[PackedStringArray] = []
	var emitted_packed_points: Array[PackedVector3Array] = []
	var on_packed_values_changed := func(
		labels: PackedStringArray, points: PackedVector3Array
	) -> void:
		emitted_packed_labels.append(labels)
		emitted_packed_points.append(points)
	if owner.connect(&"packed_values_changed", on_packed_values_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust packed-array signal")
		return
	owner.call("emit_packed_values", packed_labels, packed_points_3d)
	if (
		emitted_packed_labels != [packed_labels]
		or emitted_packed_points != [packed_points_3d]
	):
		owner.free()
		_fail("Packed-array Signal::emit did not preserve its typed Rust arguments")
		return
	var emitted_rids: Array[RID] = []
	var on_rid_changed := func(resource: RID) -> void:
		emitted_rids.append(resource)
	if owner.connect(&"rid_changed", on_rid_changed) != OK:
		owner.free()
		_fail("Godot could not connect to the generated Rust RID signal")
		return
	owner.call("emit_rid", navigation_map)
	if emitted_rids != [navigation_map]:
		owner.free()
		_fail("RID Signal::emit did not preserve the opaque Godot handle")
		return
	owner.set(&"message", "来自 Inspector")
	owner.set(&"scene_path", "res://levels/测试.tscn")
	owner.set(&"display_name", &"玩家/Inspector")
	owner.set(&"navigation_path", NodePath("Target3D"))
	owner.set(&"labels", PackedStringArray(["玩家", "检查点"]))
	var updated_checkpoints: Array[Vector2] = [
		Vector2(1.0, 2.0),
		Vector2(-3.0, 4.0),
	]
	owner.set(&"checkpoints", updated_checkpoints)
	owner.set(&"settings", {"difficulty": "hard", "lives": 3})
	owner.set(&"selected_node", target)
	var updated_gradient: Gradient = Gradient.new()
	updated_gradient.set_color(0, Color(0.1, 0.2, 0.3, 1.0))
	updated_gradient.set_color(1, Color(0.8, 0.7, 0.6, 1.0))
	var gradient_instance_id := updated_gradient.get_instance_id()
	owner.set(&"gradient", updated_gradient)
	updated_gradient = null
	owner.set(&"worker_mode", 2)
	owner.set(&"thread_messages", 3)
	owner.set(&"velocity", Vector2(-8.0, 16.0))
	owner.set(&"target_position", Vector3(9.0, 10.0, 11.0))
	owner.set(&"grid_cell", Vector2i(-8, 16))
	owner.set(&"voxel_cell", Vector3i(9, 10, -11))
	owner.set(&"tint", Color(0.1, 0.2, 0.3, 0.4))
	var updated_region := Rect2(-1.0, -2.0, 13.0, 14.0)
	var updated_atlas := Rect2i(-3, -4, 15, 16)
	var updated_orientation := Quaternion(0.0, 0.0, 1.0, 0.0)
	var updated_plane := Plane(Vector3.RIGHT, 7.0)
	var updated_vector := Vector4(-1.0, -2.0, -3.0, -4.0)
	var updated_integer_vector := Vector4i(-5, -6, -7, -8)
	var updated_canvas_transform := Transform2D(
		Vector2(2.0, 0.0),
		Vector2(0.0, 3.0),
		Vector2(-9.0, 10.0)
	)
	var updated_bounds := AABB(Vector3(-1.0, -2.0, -3.0), Vector3(7.0, 8.0, 9.0))
	var updated_basis := Basis(
		Vector3(2.0, 0.0, 0.0),
		Vector3(0.0, 3.0, 0.0),
		Vector3(0.0, 0.0, 4.0)
	)
	var updated_world_transform := Transform3D(
		updated_basis,
		Vector3(-4.0, 5.0, -6.0)
	)
	var updated_projection := Projection(
		Vector4(2.0, 0.0, 0.0, 0.0),
		Vector4(0.0, 2.0, 0.0, 0.0),
		Vector4(0.0, 0.0, 2.0, 0.0),
		Vector4(0.0, 0.0, 0.0, 1.0)
	)
	owner.set(&"viewport_region", updated_region)
	owner.set(&"atlas_region", updated_atlas)
	owner.set(&"orientation", updated_orientation)
	owner.set(&"clipping_plane", updated_plane)
	owner.set(&"floating_vector", updated_vector)
	owner.set(&"integer_vector", updated_integer_vector)
	owner.set(&"canvas_transform", updated_canvas_transform)
	owner.set(&"bounds", updated_bounds)
	owner.set(&"local_basis", updated_basis)
	owner.set(&"world_transform", updated_world_transform)
	owner.set(&"camera_projection", updated_projection)
	var saved_velocity: Vector2 = owner.get(&"velocity")
	var saved_target_position: Vector3 = owner.get(&"target_position")
	var saved_grid_cell: Vector2i = owner.get(&"grid_cell")
	var saved_voxel_cell: Vector3i = owner.get(&"voxel_cell")
	var saved_tint: Color = owner.get(&"tint")
	if (
		owner.call("current_message") != "来自 Inspector"
		or owner.get(&"scene_path") != "res://levels/测试.tscn"
		or typeof(owner.get(&"display_name")) != TYPE_STRING_NAME
		or owner.get(&"display_name") != &"玩家/Inspector"
		or owner.get(&"navigation_path") != NodePath("Target3D")
		or owner.get(&"labels") != PackedStringArray(["玩家", "检查点"])
		or owner.get(&"checkpoints") != updated_checkpoints
		or not (owner.get(&"checkpoints") as Array).is_typed()
		or owner.get(&"settings") != {"difficulty": "hard", "lives": 3}
		or owner.get(&"selected_node") != target
		or owner.get(&"gradient") == null
		or (owner.get(&"gradient") as Gradient).get_instance_id() != gradient_instance_id
		or (owner.get(&"gradient") as Gradient).get_color(0) != Color(0.1, 0.2, 0.3, 1.0)
		or owner.get(&"worker_mode") != 2
		or owner.get(&"thread_messages") != 3
		or not saved_velocity.is_equal_approx(Vector2(-8.0, 16.0))
		or not saved_target_position.is_equal_approx(Vector3(9.0, 10.0, 11.0))
		or saved_grid_cell != Vector2i(-8, 16)
		or saved_voxel_cell != Vector3i(9, 10, -11)
		or not saved_tint.is_equal_approx(Color(0.1, 0.2, 0.3, 0.4))
		or not owner.call(
			"verify_extended_math",
			updated_region,
			updated_atlas,
			updated_orientation,
			updated_plane,
			updated_vector,
			updated_integer_vector
		)
		or owner.get(&"canvas_transform") != updated_canvas_transform
		or owner.get(&"bounds") != updated_bounds
		or owner.get(&"local_basis") != updated_basis
		or owner.get(&"world_transform") != updated_world_transform
		or owner.get(&"camera_projection") != updated_projection
	):
		owner.free()
		_fail("Godot could not write Inspector values into Rust fields")
		return
	var rpc_config: Dictionary = loaded.get_rpc_config()
	var counter_rpc: Dictionary = rpc_config.get(&"set_counter", {})
	if (
		counter_rpc.get(&"rpc_mode", -1) != MultiplayerAPI.RPC_MODE_AUTHORITY
		or counter_rpc.get(&"call_local", true)
		or counter_rpc.get(&"transfer_mode", -1) != MultiplayerPeer.TRANSFER_MODE_RELIABLE
		or counter_rpc.get(&"channel", -1) != 0
	):
		owner.free()
		_fail("Rust #[rpc] metadata did not match the generated Godot configuration")
		return
	print("GODOT_RUST_MATH_VALUES")
	print("GODOT_RUST_INTEGER_VALUES")
	print("GODOT_RUST_OBJECT_VALUES")
	print("GODOT_RUST_RID_VALUES")
	print("GODOT_RUST_STRING_NAME_VALUES")
	print("GODOT_RUST_PACKED_ARRAYS")
	print("GODOT_RUST_GODOT_METADATA")
	owner.call("set_counter", 91)
	if owner.call("current_counter") != 91:
		owner.free()
		_fail("Rust #[rpc] mutation was not retained by the script instance")
		return
	if not loaded.instance_has(owner):
		owner.free()
		_fail("Attached Rust script did not create a runtime ScriptInstance")
		return

	var packed := PackedScene.new()
	if packed.pack(owner) != OK:
		owner.free()
		_fail("A node with a Rust script could not be packed as a scene")
		return
	var scene_path := "user://godot_rust_host_smoke.tscn"
	if ResourceSaver.save(packed, scene_path) != OK:
		owner.free()
		_fail("A scene with a Rust script could not be saved")
		return
	owner.free()

	var restored_scene := ResourceLoader.load(scene_path) as PackedScene
	if restored_scene == null:
		_fail("The saved Rust-script scene could not be loaded")
		return
	var restored := restored_scene.instantiate()
	if restored == null or restored.get_script() == null:
		if restored != null:
			restored.free()
		_fail("The Rust script was not restored with its saved scene")
		return
	if restored.get(&"counter") != 91:
		restored.free()
		_fail("The saved Rust #[export] value was not restored with the scene")
		return
	if (
		restored.get(&"message") != "来自 Inspector"
		or restored.get(&"scene_path") != "res://levels/测试.tscn"
		or typeof(restored.get(&"display_name")) != TYPE_STRING_NAME
		or restored.get(&"display_name") != &"玩家/Inspector"
		or restored.get(&"navigation_path") != NodePath("Target3D")
		or restored.get(&"labels") != PackedStringArray(["玩家", "检查点"])
		or restored.get(&"checkpoints") != updated_checkpoints
		or not (restored.get(&"checkpoints") as Array).is_typed()
		or restored.get(&"settings") != {"difficulty": "hard", "lives": 3}
		or restored.get(&"selected_node") != restored.get_node("Target")
		or restored.get(&"gradient") == null
		or (restored.get(&"gradient") as Gradient).get_color(0) != Color(0.1, 0.2, 0.3, 1.0)
		or (restored.get(&"gradient") as Gradient).get_color(1) != Color(0.8, 0.7, 0.6, 1.0)
		or restored.get(&"worker_mode") != 2
		or restored.get(&"thread_messages") != 3
	):
		restored.free()
		_fail("Saved Rust text, container, node, and Resource properties were not restored")
		return
	var restored_velocity: Vector2 = restored.get(&"velocity")
	var restored_target_position: Vector3 = restored.get(&"target_position")
	var restored_grid_cell: Vector2i = restored.get(&"grid_cell")
	var restored_voxel_cell: Vector3i = restored.get(&"voxel_cell")
	var restored_tint: Color = restored.get(&"tint")
	var restored_canvas_transform: Transform2D = restored.get(&"canvas_transform")
	var restored_bounds: AABB = restored.get(&"bounds")
	var restored_basis: Basis = restored.get(&"local_basis")
	var restored_world_transform: Transform3D = restored.get(&"world_transform")
	var restored_projection: Projection = restored.get(&"camera_projection")
	if (
		not restored_velocity.is_equal_approx(Vector2(-8.0, 16.0))
		or not restored_target_position.is_equal_approx(Vector3(9.0, 10.0, 11.0))
		or restored_grid_cell != Vector2i(-8, 16)
		or restored_voxel_cell != Vector3i(9, 10, -11)
		or not restored_tint.is_equal_approx(Color(0.1, 0.2, 0.3, 0.4))
		or restored_canvas_transform != updated_canvas_transform
		or restored_bounds != updated_bounds
		or restored_basis != updated_basis
		or restored_world_transform != updated_world_transform
		or restored_projection != updated_projection
	):
		restored.free()
		_fail("Saved Rust math properties were not restored with the scene")
		return
	restored.free()
	DirAccess.remove_absolute(ProjectSettings.globalize_path(scene_path))

	print("godot-rust Host smoke test passed")
	quit(0)


func _fail(message: String) -> void:
	printerr("GODOT_RUST_SMOKE_FAILURE: ", message)
	quit(1)
