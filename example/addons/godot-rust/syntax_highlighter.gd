@tool
extends EditorSyntaxHighlighter

# Keep these aligned with the ScriptLanguage word lists in rust_source.rs.
const HIGHLIGHT_WORDS := [
	"Self", "as", "async", "await", "break", "const", "continue", "crate", "dyn",
	"else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let",
	"loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
	"static", "struct", "super", "trait", "true", "type", "unsafe", "use",
	"where", "while", "abstract", "become", "box", "do", "final", "gen", "macro",
	"override", "priv", "try", "typeof", "unsized", "virtual", "yield",
	"macro_rules", "raw", "safe", "union", "bool", "char", "f32", "f64", "i8",
	"i16", "i32", "i64", "i128", "isize", "str", "u8", "u16", "u32", "u64",
	"u128", "usize",
]
const CONTROL_FLOW_WORDS := [
	"async", "await", "break", "continue", "else", "for", "if", "loop", "match",
	"return", "while", "yield",
]

var _highlighter := CodeHighlighter.new()
# CodeHighlighter's TextEdit setter is not exposed to scripts. An off-tree
# buffer lets the editor adapter reuse Godot's lexer and multiline cache.
var _buffer := TextEdit.new()
var _source_version := -1


func _init() -> void:
	_buffer.syntax_highlighter = _highlighter


func _notification(what: int) -> void:
	if what == NOTIFICATION_PREDELETE and is_instance_valid(_buffer):
		_buffer.free()


func _get_name() -> String:
	return "Rust"


func _get_supported_languages() -> PackedStringArray:
	return PackedStringArray(["Rust", "rs"])


func _create() -> EditorSyntaxHighlighter:
	return get_script().new()


func _clear_highlighting_cache() -> void:
	_source_version = -1
	_highlighter.clear_highlighting_cache()


func _update_cache() -> void:
	var settings := EditorInterface.get_editor_settings()
	var prefix := "text_editor/theme/highlighting/"
	_buffer.add_theme_color_override(
		"font_color", settings.get_setting(prefix + "text_color")
	)
	_highlighter.symbol_color = settings.get_setting(prefix + "symbol_color")
	_highlighter.function_color = settings.get_setting(prefix + "function_color")
	_highlighter.number_color = settings.get_setting(prefix + "number_color")
	_highlighter.member_variable_color = settings.get_setting(
		prefix + "member_variable_color"
	)
	_highlighter.clear_keyword_colors()
	for word in HIGHLIGHT_WORDS:
		var color_name := "control_flow_keyword_color" if (
			word in CONTROL_FLOW_WORDS
		) else "keyword_color"
		_highlighter.add_keyword_color(
			word, settings.get_setting(prefix + color_name)
		)
	for type_name in ClassDB.get_class_list():
		_highlighter.add_keyword_color(
			type_name, settings.get_setting(prefix + "engine_type_color")
		)
	_highlighter.clear_color_regions()
	var comment_color: Color = settings.get_setting(prefix + "comment_color")
	var doc_color: Color = settings.get_setting(prefix + "doc_comment_color")
	_highlighter.add_color_region("//", "", comment_color, true)
	_highlighter.add_color_region("/*", "*/", comment_color)
	_highlighter.add_color_region("///", "", doc_color, true)
	_highlighter.add_color_region("//!", "", doc_color, true)
	_highlighter.add_color_region("/**", "*/", doc_color)
	_highlighter.add_color_region("/*!", "*/", doc_color)
	_highlighter.add_color_region(
		'"', '"', settings.get_setting(prefix + "string_color")
	)
	_highlighter.update_cache()


func _get_line_syntax_highlighting(line: int) -> Dictionary:
	var source := get_text_edit()
	if not is_instance_valid(source):
		return {}
	if source.get_version() != _source_version:
		_buffer.text = source.text
		_buffer.clear_undo_history()
		_highlighter.clear_highlighting_cache()
		_source_version = source.get_version()
	return _highlighter.get_line_syntax_highlighting(line)
