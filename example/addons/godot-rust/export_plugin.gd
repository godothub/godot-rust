@tool
extends EditorExportPlugin

var _plugin: EditorPlugin
var _ios_export_path := ""
var _ios_runtime_godot := ""
var _native_export := false
var _extension_list_backup := PackedByteArray()
var _extension_list_changed := false


func configure(plugin: EditorPlugin) -> void:
	_plugin = plugin


func _get_name() -> String:
	# Export plugins are sorted by this name. The Cargo integration must run
	# before Godot's built-in `GDExtension` exporter so Extension Mode can skip
	# the editor-only Script Host descriptor before it adds a shared object.
	return "Cargo godot-rust"


func _supports_platform(platform: EditorExportPlatform) -> bool:
	return platform.get_os_name() in [
		"Android",
		"iOS",
		"Linux",
		"macOS",
		"Web",
		"Windows",
	]


func _export_file(
	path: String,
	_type: String,
	_features: PackedStringArray
) -> void:
	if (
		_native_export
		and path == "res://addons/godot-rust/godot-rust.gdextension"
	):
		# Extension Mode exports its project descriptor and library.
		# The Script Host is editor-only for this mode and must not be shipped
		# or initialized by the exported game.
		skip()
		return
	if path.begins_with("res://addons/godot-rust/") and path.ends_with(".gd"):
		skip()
		return
	if not path.ends_with(".rs"):
		return
	if (
		path.begins_with("res://target/")
		or path.begins_with("res://.godot/rust/target/")
	):
		skip()
		return
	add_file(path, PackedByteArray([10]), false)
	skip()


func _export_begin(
	features: PackedStringArray,
	is_debug: bool,
	path: String,
	_flags: int
) -> void:
	_ios_export_path = ""
	_ios_runtime_godot = ""
	_native_export = false
	_extension_list_backup = PackedByteArray()
	_extension_list_changed = false
	var platform := get_export_platform()
	var platform_name := platform.get_os_name()
	if not is_instance_valid(_plugin):
		_fail_export(
			platform_name,
			"godot-rust editor integration is not available."
		)
		return
	if platform_name == "Web":
		var preset := get_export_preset()
		if (
			not is_instance_valid(preset)
			or not bool(preset.get("variant/extensions_support"))
		):
			_fail_export(
				platform_name,
				"Enable `Extensions Support` in the Web export preset so "
				+ "Godot uses its dynamic-linking Web template."
			)
			return

	var result: Dictionary = _plugin.prepare_rust_export(
		platform_name,
		features,
		is_debug,
		path
	)
	if not bool(result.get("ok", false)):
		_fail_export(
			platform_name,
			str(result.get("error", "Rust project export failed."))
		)
		return

	var runtime_godot := str(result.get("runtime_godot", ""))
	var mode := str(result.get("mode", "script"))
	_native_export = mode == "extension"
	if platform_name == "Web" and mode == "script":
		var host_error := _stage_web_host(runtime_godot)
		if not host_error.is_empty():
			_fail_export(
				platform_name,
				host_error
			)
			return
	if mode == "extension":
		var descriptor_path := str(
			result.get("native_descriptor_resource_path", "")
		)
		var descriptor_error := _register_native_descriptor(
			descriptor_path
		)
		if not descriptor_error.is_empty():
			_fail_export(platform_name, descriptor_error)
			return
		var list_error := _prepare_native_extension_list(descriptor_path)
		if not list_error.is_empty():
			_fail_export(platform_name, list_error)
			return

	var modules: Array = result.get("modules", [])
	if modules.is_empty():
		_fail_export(
			platform_name,
			"Build completed without validated Rust export modules."
		)
		return
	for value in modules:
		if not value is Dictionary:
			_fail_export(
				platform_name,
				"Build service returned an invalid Rust export module."
			)
			return
		var module: Dictionary = value
		var module_path := str(module.get("module_path", ""))
		var tags := PackedStringArray(module.get("godot_tags", []))
		if (
			module_path.is_empty()
			or tags.is_empty()
			or not _absolute_path_exists(module_path)
		):
			_fail_export(
				platform_name,
				"Validated Rust export module is missing or incomplete: %s"
				% module_path
			)
			return
		if mode == "extension":
			# Godot's built-in GDExtension export plugin reads the registered
			# descriptor and owns library selection, platform placement, and
			# extension-list export. Adding it again would duplicate the binary.
			continue
		if platform_name == "iOS":
			# iOS requires project code to be linked and embedded by Xcode.
			add_ios_embedded_framework(module_path)
		else:
			add_shared_object(module_path, tags, "")
	if platform_name == "iOS":
		_ios_export_path = path
		_ios_runtime_godot = runtime_godot
	print(
		"GODOT_RUST_EXPORT_MODULES_ADDED platform=%s profile=%s mode=%s count=%d"
		% [
			platform_name,
			"debug" if is_debug else "release",
			mode,
			modules.size(),
		]
	)


func _export_end() -> void:
	var ios_export_path := _ios_export_path
	var ios_runtime_godot := _ios_runtime_godot
	var extension_list_error := _restore_extension_list()
	_ios_export_path = ""
	_ios_runtime_godot = ""
	_native_export = false
	if not extension_list_error.is_empty():
		get_export_platform().add_message(
			EditorExportPlatform.EXPORT_MESSAGE_ERROR,
			"godot-rust",
			extension_list_error
		)
		push_error("godot-rust export cleanup failed: %s" % extension_list_error)
	if ios_export_path.is_empty() or ios_runtime_godot != "4.4":
		return
	var project_name := ios_export_path.get_file().get_basename()
	var project_path := (
		ios_export_path.get_base_dir()
		.path_join("%s.xcodeproj" % project_name)
		.path_join("project.pbxproj")
	)
	var error := _repair_godot_44_ios_project(project_path)
	if not error.is_empty():
		get_export_platform().add_message(
			EditorExportPlatform.EXPORT_MESSAGE_ERROR,
			"godot-rust",
			error
		)
		push_error("godot-rust export failed: %s" % error)


func _repair_godot_44_ios_project(project_path: String) -> String:
	if not FileAccess.file_exists(project_path):
		return (
			"Godot 4.4 iOS compatibility repair could not find %s."
			% project_path
		)
	var source := FileAccess.get_file_as_string(project_path)
	var retained := PackedStringArray()
	var removed := 0
	var conditional_links := 0
	for line in source.split("\n"):
		if "MetalFX.framework" in line:
			removed += 1
		else:
			retained.append(line)
			if (
				"OTHER_LDFLAGS =" in line
				and (
					"_godot_rust_init" in line
					or "_godot_rs_native_init" in line
				)
			):
				var indentation := line.left(line.length() - line.strip_edges().length())
				retained.append(
					indentation
					+ '"OTHER_LDFLAGS[sdk=iphoneos*]" = '
					+ '"$(inherited) -weak_framework MetalFX";'
				)
				conditional_links += 1
	if removed != 4 or conditional_links != 2:
		return (
			(
				"Godot 4.4 iOS compatibility repair expected four MetalFX "
				+ "project entries and two linker settings, found %d and %d; "
				+ "the Xcode project was not changed."
			)
			% [removed, conditional_links]
		)
	var file := FileAccess.open(project_path, FileAccess.WRITE)
	if file == null:
		return (
			"Godot 4.4 iOS compatibility repair could not update %s: %s"
			% [project_path, error_string(FileAccess.get_open_error())]
		)
	file.store_string("\n".join(retained))
	print("GODOT_RUST_IOS_GODOT_44_XCODE_REPAIRED")
	return ""


func _absolute_path_exists(path: String) -> bool:
	return FileAccess.file_exists(path) or DirAccess.dir_exists_absolute(path)


func _register_native_descriptor(resource_path: String) -> String:
	if (
		not resource_path.begins_with("res://")
		or not FileAccess.file_exists(resource_path)
	):
		return "Validated Native `.gdextension` descriptor is missing."
	if FileAccess.get_file_as_bytes(resource_path).is_empty():
		return "Validated Native `.gdextension` descriptor is empty."
	var filesystem := _plugin.get_editor_interface().get_resource_filesystem()
	filesystem.update_file(resource_path)
	if filesystem.get_file_type(resource_path) != "GDExtension":
		return (
			"Godot did not recognize the generated Native descriptor as a "
			+ "GDExtension: %s" % resource_path
		)
	var extension_list_error := _ensure_extension_list_entry(resource_path)
	if not extension_list_error.is_empty():
		return extension_list_error
	return ""


func _ensure_extension_list_entry(resource_path: String) -> String:
	var list_path := "res://.godot/extension_list.cfg"
	var entries := PackedStringArray()
	if FileAccess.file_exists(list_path):
		for line in FileAccess.get_file_as_string(list_path).split("\n"):
			var entry := line.strip_edges()
			if not entry.is_empty() and not entries.has(entry):
				entries.append(entry)
	if not entries.has(resource_path):
		entries.append(resource_path)
	entries.sort()
	var file := FileAccess.open(list_path, FileAccess.WRITE)
	if file == null:
		return (
			"Could not register the generated Native descriptor in %s: %s"
			% [list_path, error_string(FileAccess.get_open_error())]
		)
	file.store_string("\n".join(entries) + "\n")
	return ""


func _prepare_native_extension_list(descriptor_path: String) -> String:
	var native_path := descriptor_path
	var list_path := "res://.godot/extension_list.cfg"
	if not native_path.begins_with("res://"):
		return "Native descriptor is not a Godot project resource path."
	if not FileAccess.file_exists(list_path):
		return "Godot's GDExtension list is missing after Native registration."
	var source := FileAccess.get_file_as_bytes(list_path)
	if source.is_empty():
		return "Godot's GDExtension list is empty after Native registration."
	var source_text := source.get_string_from_utf8()
	if source_text.to_utf8_buffer() != source:
		return "Godot's GDExtension list is not valid UTF-8."
	var entries := PackedStringArray()
	var removed_hosts := 0
	for line in source_text.split("\n"):
		var entry := line.strip_edges()
		if entry.is_empty():
			continue
		if entry == "res://addons/godot-rust/godot-rust.gdextension":
			removed_hosts += 1
			continue
		if entries.has(entry):
			return "Godot's GDExtension list contains duplicate entries."
		entries.append(entry)
	if not entries.has(native_path):
		return "Godot's GDExtension list omitted the Native project descriptor."
	if removed_hosts > 1:
		return "Godot's GDExtension list contains duplicate Script Host entries."
	if removed_hosts == 0:
		return ""
	entries.sort()
	var file := FileAccess.open(list_path, FileAccess.WRITE)
	if file == null:
		return (
			"Could not prepare Godot's Native export extension list: %s"
			% error_string(FileAccess.get_open_error())
		)
	_extension_list_backup = source
	file.store_string("\n".join(entries) + "\n")
	_extension_list_changed = true
	return ""


func _restore_extension_list() -> String:
	if not _extension_list_changed:
		return ""
	var file := FileAccess.open(
		"res://.godot/extension_list.cfg",
		FileAccess.WRITE
	)
	if file == null:
		return (
			"Could not restore Godot's editor GDExtension list: %s"
			% error_string(FileAccess.get_open_error())
		)
	file.store_buffer(_extension_list_backup)
	_extension_list_backup = PackedByteArray()
	_extension_list_changed = false
	return ""


func _fail_export(platform_name: String, message: String) -> void:
	get_export_platform().add_message(
		EditorExportPlatform.EXPORT_MESSAGE_ERROR,
		"godot-rust",
		message
	)
	push_error("godot-rust export failed: %s" % message)
	var failure_path := _failure_module_path(platform_name)
	if platform_name == "iOS":
		add_ios_embedded_framework(failure_path)
	else:
		add_shared_object(failure_path, PackedStringArray(), "")


func _failure_module_path(platform_name: String) -> String:
	var module_file := "libgodot_rs_project_module.so"
	if platform_name == "Windows":
		module_file = "godot_rs_project_module.dll"
	elif platform_name in ["iOS", "macOS"]:
		module_file = "libgodot_rs_project_module.dylib"
	elif platform_name == "Web":
		module_file = "godot_rs_project_module.wasm"
	return ProjectSettings.globalize_path(
		"res://.godot/rust/export-failed/%s" % module_file
	)


func _web_host_path(godot_api: String) -> String:
	return (
		"res://addons/godot-rust/bin/web-godot-%s/godot_host.wasm"
		% godot_api
	)


func _staged_web_host_path() -> String:
	return "res://.godot/rust/web-host/godot_host.wasm"


func _stage_web_host(godot_api: String) -> String:
	var source := _web_host_path(godot_api)
	if not FileAccess.file_exists(source):
		return (
			"Packaged Godot %s Web Script Host is missing: %s"
			% [godot_api, source]
		)
	var destination := ProjectSettings.globalize_path(
		_staged_web_host_path()
	)
	var temporary := destination + ".tmp"
	var directory_error := DirAccess.make_dir_recursive_absolute(
		destination.get_base_dir()
	)
	if directory_error != OK:
		return "Could not create the managed Web Script Host directory."
	if FileAccess.file_exists(temporary):
		DirAccess.remove_absolute(temporary)
	var copy_error := DirAccess.copy_absolute(
		ProjectSettings.globalize_path(source),
		temporary
	)
	if copy_error != OK:
		return "Could not stage the Godot %s Web Script Host." % godot_api
	if FileAccess.file_exists(destination):
		var remove_error := DirAccess.remove_absolute(destination)
		if remove_error != OK:
			DirAccess.remove_absolute(temporary)
			return "Could not replace the previous staged Web Script Host."
	var rename_error := DirAccess.rename_absolute(temporary, destination)
	if rename_error != OK:
		DirAccess.remove_absolute(temporary)
		return "Could not publish the staged Web Script Host."
	return ""
