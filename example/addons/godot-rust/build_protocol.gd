@tool
extends RefCounted

const OUTPUT_LIMIT := 64 * 1024 * 1024
const ERROR_TEXT_LIMIT := 64 * 1024


static func encode_request(request: Dictionary) -> String:
	return JSON.stringify(request).to_utf8_buffer().hex_encode()


static func start_request(
	executable: String,
	request: Dictionary
) -> Dictionary:
	var nonce := Crypto.new().generate_random_bytes(16)
	if nonce.is_empty():
		return {
			"pid": -1,
			"response_path": "",
			"error": "could not generate a secure response-file name",
		}
	var response_path := OS.get_temp_dir().path_join(
		"godot-rust-response-%d-%d-%s.json"
		% [
			OS.get_process_id(),
			Time.get_ticks_usec(),
			nonce.hex_encode(),
		]
	)
	var cancel_path := response_path.trim_suffix(".json") + ".cancel"
	var pid := OS.create_process(
		executable,
		[
			"--request-hex",
			encode_request(request),
			"--response-file",
			response_path,
			"--cancel-file",
			cancel_path,
		],
		false
	)
	if pid < 0:
		return {
			"pid": -1,
			"response_path": "",
			"cancel_path": "",
			"error": "could not start the packaged build service",
		}
	return {
		"pid": pid,
		"response_path": response_path,
		"cancel_path": cancel_path,
		"error": "",
	}


static func collect_response(response_path: String) -> Dictionary:
	if (
		response_path.is_empty()
		or not FileAccess.file_exists(response_path)
	):
		return {
			"response": {},
			"error": "the packaged build service returned no response file",
		}
	var response_file := FileAccess.open(response_path, FileAccess.READ)
	if response_file == null:
		discard_response_file(response_path)
		return {
			"response": {},
			"error": "could not open the packaged build response file",
		}
	var response_size := response_file.get_length()
	if response_size > OUTPUT_LIMIT:
		response_file.close()
		discard_response_file(response_path)
		return {
			"response": {},
			"error": output_limit_error(),
		}
	var response_bytes := response_file.get_buffer(response_size)
	var read_error := response_file.get_error()
	response_file.close()
	discard_response_file(response_path)
	if response_bytes.size() != response_size or read_error != OK:
		return {
			"response": {},
			"error": "could not read the complete packaged build response",
		}
	return parse_response(response_bytes.get_string_from_utf8(), "")


static func discard_response_file(response_path: String) -> void:
	if (
		not response_path.is_empty()
		and FileAccess.file_exists(response_path)
	):
		DirAccess.remove_absolute(response_path)


static func parse_response(stdout: String, stderr: String) -> Dictionary:
	var json := JSON.new()
	var parse_error := json.parse(stdout.strip_edges())
	if parse_error != OK or not json.data is Dictionary:
		return {
			"response": {},
			"error": _bounded_error(
				(
					"returned invalid JSON at line %d: %s\n%s\n%s"
					% [
						json.get_error_line(),
						json.get_error_message(),
						stdout,
						stderr,
					]
				)
			),
		}
	return {
		"response": json.data,
		"error": _bounded_error(stderr),
	}


static func output_within_limit(stdout_bytes: int, stderr_bytes: int) -> bool:
	return stdout_bytes + stderr_bytes <= OUTPUT_LIMIT


static func output_limit_error() -> String:
	return "exceeded the %d-byte output safety limit" % OUTPUT_LIMIT


static func _bounded_error(value: String) -> String:
	if value.length() <= ERROR_TEXT_LIMIT:
		return value
	return value.left(ERROR_TEXT_LIMIT) + "\n…error output truncated"
