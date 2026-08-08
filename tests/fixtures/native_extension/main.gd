extends Node


func _ready() -> void:
	if not ClassDB.class_exists("GodotRsNativeSmoke"):
		_fail("GodotRsNativeSmoke was not registered in ClassDB")
		return

	var counter: Object = ClassDB.instantiate("GodotRsNativeSmoke")
	if counter == null:
		_fail("GodotRsNativeSmoke could not be instantiated")
		return
	if counter.current() != 41:
		_fail("Native constructor state was not preserved")
		return
	if counter.add(1) != 42:
		_fail("typed Native ptrcall returned the wrong value")
		return
	if counter.call("add", 8) != 50:
		_fail("dynamic Native Variant call returned the wrong value")
		return
	if counter.current() != 50 or not counter.is_positive():
		_fail("Native instance state did not survive multiple calls")
		return
	if counter.instance_id() <= 0:
		_fail("typed Base<Node> did not expose a valid Godot instance ID")
		return
	if counter.describe_offset("native", Vector2(3, 5)) != "native:3:5":
		_fail("multi-argument Native ptrcall lost String or Vector2 data")
		return
	if (
		counter.call("describe_offset", "dynamic", Vector2(8, 13))
		!= "dynamic:8:13"
	):
		_fail("multi-argument Native Variant call lost String or Vector2 data")
		return
	counter.value = 64
	if counter.value != 64:
		_fail("typed Native property accessors lost state")
		return
	counter.metadata = {"kind": "native", "count": 2}
	if counter.metadata != {"kind": "native", "count": 2}:
		_fail("owned Native Dictionary property lost state")
		return
	counter.dynamic_note = "runtime\nmetadata"
	if counter.dynamic_note != "runtime\nmetadata":
		_fail("dynamic Native property callbacks lost state")
		return
	if not counter.property_can_revert("dynamic_note"):
		_fail("dynamic Native property did not expose a revert value")
		return
	counter.dynamic_note = counter.property_get_revert("dynamic_note")
	if counter.dynamic_note != "default note":
		_fail("dynamic Native property revert returned the wrong value")
		return
	var found_dynamic := false
	var found_range := false
	var found_validated := false
	for property in counter.get_property_list():
		if property.name == "dynamic_note":
			found_dynamic = property.hint == PROPERTY_HINT_MULTILINE_TEXT
		elif property.name == "value":
			found_range = (
				property.hint == PROPERTY_HINT_RANGE
				and property.hint_string == "0,100,1"
			)
		elif property.name == "metadata":
			found_validated = bool(
				property.usage & PROPERTY_USAGE_STORE_IF_NULL
			)
	if not found_dynamic or not found_range or not found_validated:
		_fail("Native Inspector metadata callbacks were not preserved")
		return
	if str(counter) != "GodotRsNativeSmoke(value=64)":
		_fail("Native custom string callback returned the wrong value")
		return
	if not counter.has_signal("offset_described"):
		_fail("typed Native signal metadata was not registered")
		return
	if not counter.is_same_node(counter) or counter.owner() != counter:
		_fail("typed Native Object references lost identity or class metadata")
		return
	if not counter.enable_processing():
		_fail("generated Node API did not execute through the Native runtime")
		return
	add_child(counter)
	await get_tree().process_frame
	await get_tree().process_frame
	if counter.process_frames() <= 0:
		_fail("generated Native virtual _process override was not called")
		return
	remove_child(counter)
	if counter.generated_api_round_trip() != "NativeGenerated:12.5:-4":
		_fail("generated text or math API did not round-trip through Native ptrcall")
		return
	if not counter.generated_global_api_round_trip():
		_fail("generated builtin, utility, or singleton API failed in Native mode")
		return
	if not counter.generated_refcounted_round_trip():
		_fail("generated RefCounted construction or ownership failed in Native mode")
		return
	if not counter.generated_dynamic_api_round_trip():
		_fail("generated Variant, container, keyed, or vararg API failed in Native mode")
		return
	if not counter.generated_callable_signal_round_trip():
		_fail("generated Callable or Signal ownership failed in Native mode")
		return
	if not counter.native_owned_values(
		[1, "two"],
		{"answer": 42},
		PackedInt64Array([3, 5, 8]),
		Callable(counter, "current"),
		Signal(counter, "offset_described"),
		"any Variant",
		NodePath("Child/Camera"),
		&"NativeName",
	):
		_fail("owned Native method arguments did not preserve Godot values")
		return
	if counter.native_enum_round_trip(OK) != OK:
		_fail("generated Native enum value did not round-trip")
		return
	var rpc_config: Dictionary
	if counter.has_method("get_node_rpc_config"):
		rpc_config = counter.get_node_rpc_config()
	else:
		rpc_config = counter.get_rpc_config()
	if (
		not rpc_config.has("network_add")
		or rpc_config.network_add.rpc_mode != MultiplayerAPI.RPC_MODE_ANY_PEER
		or not rpc_config.network_add.call_local
		or rpc_config.network_add.transfer_mode != MultiplayerPeer.TRANSFER_MODE_RELIABLE
		or rpc_config.network_add.channel != 3
	):
		_fail("Native RPC metadata was not installed on the Node instance")
		return

	counter.free()
	print("GODOT_RS_NATIVE_CLASS_OK")
	get_tree().quit()


func _fail(message: String) -> void:
	push_error(message)
	get_tree().quit(1)
