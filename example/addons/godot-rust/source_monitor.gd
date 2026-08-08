@tool
extends Node

signal check_requested(revision: int)
signal revision_changed(revision: int)
signal monitor_failed(message: String)

const DEBOUNCE_SECONDS := 0.4
const MAX_TRACKED_FILES := 10_000
const MAX_TRACKED_FILE_BYTES := 16 * 1024 * 1024
const MAX_TRACKED_TOTAL_BYTES := 256 * 1024 * 1024

var _filesystem: EditorFileSystem
var _timer: Timer
var _enabled := false
var _initialized := false
var _signature := ""
var _revision := 0
var _last_requested_revision := 0
var _last_error := ""


func _ready() -> void:
	_timer = Timer.new()
	_timer.one_shot = true
	_timer.wait_time = DEBOUNCE_SECONDS
	_timer.timeout.connect(_on_debounce_timeout)
	add_child(_timer)


func start(filesystem: EditorFileSystem) -> void:
	_filesystem = filesystem
	if not _filesystem.filesystem_changed.is_connected(_on_filesystem_changed):
		_filesystem.filesystem_changed.connect(_on_filesystem_changed)
	if not _filesystem.resources_reload.is_connected(_on_resources_changed):
		_filesystem.resources_reload.connect(_on_resources_changed)
	if not _filesystem.resources_reimported.is_connected(_on_resources_changed):
		_filesystem.resources_reimported.connect(_on_resources_changed)
	call_deferred("_refresh_signature", false)


func set_auto_check_enabled(enabled: bool) -> void:
	_enabled = enabled
	if not _enabled and is_instance_valid(_timer):
		_timer.stop()
	if not _initialized:
		_refresh_signature(false)
	if (
		_enabled
		and _revision > _last_requested_revision
		and is_instance_valid(_timer)
	):
		_timer.start()


func revision() -> int:
	return _revision


func notify_resource_saved(resource: Resource) -> void:
	if resource == null or not _is_tracked_path(resource.resource_path):
		return
	_refresh_signature(true)


func _on_filesystem_changed() -> void:
	_refresh_signature(true)


func _on_resources_changed(_paths: PackedStringArray) -> void:
	_refresh_signature(true)


func _refresh_signature(schedule_check: bool) -> void:
	if _filesystem == null:
		return
	var root := _filesystem.get_filesystem()
	if root == null:
		return
	var paths: Array[String] = []
	if not _collect_paths(root, paths):
		return
	for fixed_path in ["res://Cargo.toml", "res://Cargo.lock"]:
		if FileAccess.file_exists(fixed_path) and fixed_path not in paths:
			paths.append(fixed_path)
	paths.sort()
	var entries: PackedStringArray = []
	var total_bytes := 0
	for path in paths:
		var file := FileAccess.open(path, FileAccess.READ)
		if file == null:
			_fail("Could not open changed Rust input: %s" % path)
			return
		var file_bytes := file.get_length()
		file.close()
		if file_bytes > MAX_TRACKED_FILE_BYTES:
			_fail(
				"Rust input exceeds the %d-byte monitoring limit: %s"
				% [MAX_TRACKED_FILE_BYTES, path]
			)
			return
		total_bytes += file_bytes
		if total_bytes > MAX_TRACKED_TOTAL_BYTES:
			_fail(
				"Rust input monitoring exceeded %d total bytes"
				% MAX_TRACKED_TOTAL_BYTES
			)
			return
		var digest := FileAccess.get_md5(path)
		if digest.is_empty():
			_fail("Could not fingerprint changed Rust input: %s" % path)
			return
		entries.append("%s:%s" % [path, digest])
	_last_error = ""
	var signature := "\n".join(entries).sha256_text()
	if not _initialized:
		_signature = signature
		_initialized = true
		return
	if signature == _signature:
		return
	_signature = signature
	_revision += 1
	revision_changed.emit(_revision)
	if schedule_check and _enabled:
		_timer.start()


func _collect_paths(
	directory: EditorFileSystemDirectory,
	output: Array[String]
) -> bool:
	for index in directory.get_file_count():
		var path := directory.get_file_path(index)
		if not _is_tracked_path(path):
			continue
		output.append(path)
		if output.size() > MAX_TRACKED_FILES:
			_fail("Rust input monitoring exceeded %d files" % MAX_TRACKED_FILES)
			return false
	for index in directory.get_subdir_count():
		if not _collect_paths(directory.get_subdir(index), output):
			return false
	return true


func _is_tracked_path(path: String) -> bool:
	var file_name := path.get_file()
	if file_name == "Cargo.toml" or file_name == "Cargo.lock":
		return true
	if file_name == "build.rs":
		return true
	return path.get_extension() == "rs" and file_name != "mod.rs"


func _on_debounce_timeout() -> void:
	if _enabled:
		_last_requested_revision = _revision
		check_requested.emit(_revision)


func _fail(message: String) -> void:
	if message == _last_error:
		return
	_last_error = message
	monitor_failed.emit(message)
