@tool
extends Node

signal request_started(action: String)
signal request_completed(action: String, response: Dictionary, error: String)
signal request_cancelled(action: String, reason: String)

const BuildProtocol := preload(
	"res://addons/godot-rust/build_protocol.gd"
)
const WAIT_POLL_DELAY_MSEC := 5
const WAIT_TIMEOUT_MSEC := 30 * 60 * 1_000
const CANCEL_GRACE_MSEC := 5_000
const SHUTDOWN_GRACE_MSEC := 3_000

var _action := ""
var _pid := -1
var _response_path := ""
var _cancel_path := ""
var _cancel_reason := ""
var _cancel_requested_at := -1
var _last_completion: Dictionary = {}


func _ready() -> void:
	set_process(false)


func is_running() -> bool:
	return _pid >= 0


func active_action() -> String:
	return _action


func start(action: String, executable: String, request: Dictionary) -> bool:
	if is_running():
		return false
	_last_completion = {}
	var started: Dictionary = BuildProtocol.start_request(executable, request)
	_pid = int(started.get("pid", -1))
	_response_path = str(started.get("response_path", ""))
	_cancel_path = str(started.get("cancel_path", ""))
	if _pid < 0:
		_response_path = ""
		_cancel_path = ""
		return false
	_action = action
	set_process(true)
	request_started.emit(action)
	return true


func shutdown() -> void:
	if not is_running():
		return
	cancel("plugin shutdown")
	var deadline := Time.get_ticks_msec() + SHUTDOWN_GRACE_MSEC
	while is_running() and Time.get_ticks_msec() < deadline:
		_poll()
		if is_running():
			OS.delay_msec(WAIT_POLL_DELAY_MSEC)
	if is_running():
		_force_stop(false, "plugin shutdown")
	_last_completion = {}


func cancel(reason := "request cancelled by user") -> bool:
	if not is_running():
		return false
	if _cancel_requested_at >= 0:
		return true
	if _cancel_path.is_empty():
		var action := _action
		_force_stop(true, reason)
		request_cancelled.emit(action, reason)
		return true
	var marker := FileAccess.open(_cancel_path, FileAccess.WRITE)
	if marker == null:
		var action := _action
		_force_stop(true, reason)
		request_cancelled.emit(action, reason)
		return true
	marker.store_string("cancel\n")
	marker.flush()
	marker = null
	_cancel_reason = reason
	_cancel_requested_at = Time.get_ticks_msec()
	return true


func take_last_completion() -> Dictionary:
	var completion := _last_completion
	_last_completion = {}
	return completion


func _force_stop(keep_completion: bool, reason: String) -> void:
	if OS.is_process_running(_pid):
		OS.kill(_pid)
	BuildProtocol.discard_response_file(_response_path)
	BuildProtocol.discard_response_file(_cancel_path)
	_reset_process()
	_action = ""
	if keep_completion:
		_last_completion = {
			"response": {},
			"error": reason,
			"cancelled": true,
		}
	else:
		_last_completion = {}


func wait_for_completion(timeout_msec := WAIT_TIMEOUT_MSEC) -> bool:
	if not is_running():
		return true
	var started_at := Time.get_ticks_msec()
	while is_running():
		_poll()
		if not is_running():
			return not bool(_last_completion.get("cancelled", false))
		if Time.get_ticks_msec() - started_at >= timeout_msec:
			var action := _action
			_force_stop(
				true,
				"process exceeded the foreground wait safety limit"
			)
			request_completed.emit(
				action,
				{},
				"process exceeded the foreground wait safety limit"
			)
			return false
		DisplayServer.process_events()
		OS.delay_msec(WAIT_POLL_DELAY_MSEC)
	return not bool(_last_completion.get("cancelled", false))


func _process(_delta: float) -> void:
	_poll()


func _poll() -> void:
	if not is_running():
		set_process(false)
		return
	if OS.is_process_running(_pid):
		if (
			_cancel_requested_at >= 0
			and Time.get_ticks_msec() - _cancel_requested_at >= CANCEL_GRACE_MSEC
		):
			var action := _action
			var reason := _cancel_reason
			_force_stop(true, reason)
			request_cancelled.emit(action, reason)
		return

	var action := _action
	if _cancel_requested_at >= 0:
		var reason := _cancel_reason
		BuildProtocol.discard_response_file(_response_path)
		BuildProtocol.discard_response_file(_cancel_path)
		_reset_process()
		_action = ""
		_last_completion = {
			"response": {},
			"error": reason,
			"cancelled": true,
		}
		request_cancelled.emit(action, reason)
		return
	var parsed := BuildProtocol.collect_response(_response_path)
	BuildProtocol.discard_response_file(_cancel_path)
	_reset_process()
	_action = ""
	_last_completion = {
		"response": parsed.get("response", {}),
		"error": parsed.get("error", ""),
		"cancelled": false,
	}
	request_completed.emit(
		action,
		_last_completion.get("response", {}),
		_last_completion.get("error", "")
	)


func _reset_process() -> void:
	_pid = -1
	_response_path = ""
	_cancel_path = ""
	_cancel_reason = ""
	_cancel_requested_at = -1
	set_process(false)
