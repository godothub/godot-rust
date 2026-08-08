//! Small, error-tolerant Rust source helpers used by Godot's script editor.
//!
//! This is deliberately not a Rust parser. Editor callbacks must keep working
//! while a user is in the middle of typing incomplete code, so the scanner only
//! recognizes the lexical constructs needed for highlighting, navigation, and
//! conservative brace-based indentation.

use syn::parse::Parser;

/// Rust 2024 keywords plus primitive types that should receive keyword-style
/// highlighting in Godot's standard script highlighter.
pub(crate) const HIGHLIGHT_WORDS: &[&str] = &[
    // Strict keywords.
    "Self",
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    // Reserved keywords, including `gen` from edition 2024.
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "gen",
    "macro",
    "override",
    "priv",
    "try",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    // Weak keywords that are meaningful in specific syntactic positions.
    "macro_rules",
    "raw",
    "safe",
    "union",
    // Primitive types. ScriptLanguageExtension cannot provide a separate
    // language-specific type list, so these use the standard keyword color.
    "bool",
    "char",
    "f32",
    "f64",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "str",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
];

pub(crate) const CONTROL_FLOW_WORDS: &[&str] = &[
    "async", "await", "break", "continue", "else", "for", "if", "loop", "match", "return", "while",
    "yield",
];

pub(crate) const COMMENT_DELIMITERS: &[&str] = &["//", "/* */"];
pub(crate) const DOC_COMMENT_DELIMITERS: &[&str] = &["///", "//!", "/** */", "/*! */"];

// Godot's standard highlighter only supports a fixed opening and closing
// delimiter. A double quote still highlights ordinary, byte, C, and the common
// zero-hash raw strings without treating Rust lifetimes as character strings.
pub(crate) const STRING_DELIMITERS: &[&str] = &["\" \""];

pub(crate) fn is_control_flow_word(word: &str) -> bool {
    CONTROL_FLOW_WORDS.binary_search(&word).is_ok()
}

/// Finds the zero-based line containing a Rust function name.
///
/// Comments, strings, raw strings, and character literals are ignored. The
/// search intentionally accepts functions inside `impl` blocks because that is
/// where Godot Rust script callbacks live.
pub(crate) fn find_function_line(source: &str, requested_name: &str) -> Option<usize> {
    let requested_name = requested_name.strip_prefix("r#").unwrap_or(requested_name);
    let mut scanner = Scanner::new(source);
    while let Some(token) = scanner.next_token() {
        if token.text != "fn" {
            continue;
        }
        let name_token = scanner.next_token()?;
        let name = name_token
            .text
            .strip_prefix("r#")
            .unwrap_or(name_token.text);
        if name == requested_name {
            return Some(name_token.line);
        }
    }
    None
}

/// Returns every distinct function and its zero-based declaration line.
pub(crate) fn function_declarations(source: &str) -> Vec<(String, usize)> {
    let mut scanner = Scanner::new(source);
    let mut functions = Vec::<(String, usize)>::new();
    while let Some(token) = scanner.next_token() {
        if token.text != "fn" {
            continue;
        }
        let Some(name) = scanner.next_token() else {
            break;
        };
        let line = name.line;
        let name = name.text.strip_prefix("r#").unwrap_or(name.text);
        if is_identifier(name)
            && !functions
                .iter()
                .any(|(existing, _)| existing.as_str() == name)
        {
            functions.push((name.to_owned(), line));
        }
    }
    functions
}

/// Collects source-local identifiers for conservative editor completion.
pub(crate) fn identifiers(source: &str) -> Vec<String> {
    let mut scanner = Scanner::new(source);
    let mut values = Vec::new();
    while let Some(token) = scanner.next_token() {
        let value = token.text.strip_prefix("r#").unwrap_or(token.text);
        if is_identifier(value)
            && !HIGHLIGHT_WORDS.contains(&value)
            && !values.iter().any(|existing| existing == value)
        {
            values.push(value.to_owned());
        }
    }
    values.sort();
    values
}

/// Finds declarations that Godot can navigate to without a full compiler AST.
pub(crate) fn find_declaration_line(source: &str, requested_name: &str) -> Option<usize> {
    let requested_name = requested_name.strip_prefix("r#").unwrap_or(requested_name);
    let mut scanner = Scanner::new(source);
    while let Some(token) = scanner.next_token() {
        if !matches!(
            token.text,
            "fn" | "struct" | "enum" | "trait" | "type" | "const" | "static" | "mod"
        ) {
            continue;
        }
        let name = scanner.next_token()?;
        if name.text.strip_prefix("r#").unwrap_or(name.text) == requested_name {
            return Some(name.line);
        }
    }
    None
}

/// Finds the first source token matching a reflected field or method name.
pub(crate) fn find_identifier_line(source: &str, requested_name: &str) -> Option<usize> {
    let requested_name = requested_name.strip_prefix("r#").unwrap_or(requested_name);
    let mut scanner = Scanner::new(source);
    while let Some(token) = scanner.next_token() {
        if token.text.strip_prefix("r#").unwrap_or(token.text) == requested_name {
            return Some(token.line);
        }
    }
    None
}

/// Extracts a contiguous Rust doc-comment block immediately above a line.
pub(crate) fn documentation_before_line(source: &str, line: usize) -> String {
    let lines = source.lines().collect::<Vec<_>>();
    let mut cursor = line.min(lines.len());
    let mut docs = Vec::new();
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim();
        if let Some(value) = trimmed
            .strip_prefix("///")
            .or_else(|| trimmed.strip_prefix("//!"))
        {
            docs.push(value.strip_prefix(' ').unwrap_or(value).to_owned());
            continue;
        }
        if trimmed.is_empty() && docs.is_empty() {
            continue;
        }
        break;
    }
    docs.reverse();
    docs.join("\n")
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SourceConstantValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SourceConstant {
    pub(crate) name: String,
    pub(crate) value: SourceConstantValue,
    pub(crate) line: usize,
}

/// Reads literal associated constants from the Rust script implementation.
///
/// Constants with computed values remain ordinary Rust constants; only values
/// with an exact Godot scalar representation are exposed through `Script`.
pub(crate) fn script_constants(source: &str, script_type: &str) -> Vec<SourceConstant> {
    let Ok(file) = syn::parse_file(source) else {
        return Vec::new();
    };
    let mut constants = Vec::new();
    for item in file.items {
        let syn::Item::Impl(item) = item else {
            continue;
        };
        let syn::Type::Path(self_type) = item.self_ty.as_ref() else {
            continue;
        };
        if self_type
            .path
            .segments
            .last()
            .is_none_or(|segment| segment.ident != script_type)
        {
            continue;
        }
        for item in item.items {
            let syn::ImplItem::Const(constant) = item else {
                continue;
            };
            let Some(value) = literal_constant_value(&constant.expr) else {
                continue;
            };
            constants.push(SourceConstant {
                name: constant.ident.to_string(),
                value,
                line: constant.ident.span().start().line.saturating_sub(1),
            });
        }
    }
    constants
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScriptSourceAttributes {
    pub(crate) abstract_: bool,
    pub(crate) icon_path: Option<String>,
    pub(crate) base_script_path: Option<String>,
    pub(crate) global_name: Option<String>,
    pub(crate) base_class: Option<String>,
}

pub(crate) fn script_source_attributes(source: &str, script_type: &str) -> ScriptSourceAttributes {
    let Ok(file) = syn::parse_file(source) else {
        return ScriptSourceAttributes::default();
    };
    let Some(item) = file.items.into_iter().find_map(|item| {
        let syn::Item::Struct(item) = item else {
            return None;
        };
        (item.ident == script_type).then_some(item)
    }) else {
        return ScriptSourceAttributes::default();
    };
    let mut result = ScriptSourceAttributes::default();
    for attribute in item
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("script"))
    {
        let syn::Meta::List(list) = &attribute.meta else {
            continue;
        };
        let tokens = list
            .tokens
            .clone()
            .into_iter()
            .map(|token| match token {
                proc_macro2::TokenTree::Ident(ident) if ident == "abstract" => {
                    proc_macro2::TokenTree::Ident(proc_macro2::Ident::new_raw(
                        "abstract",
                        ident.span(),
                    ))
                }
                token => token,
            })
            .collect();
        let Ok(entries) =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
                .parse2(tokens)
        else {
            continue;
        };
        for entry in entries {
            match entry {
                syn::Meta::Path(path)
                    if path
                        .segments
                        .first()
                        .is_some_and(|segment| segment.ident == "r#abstract") =>
                {
                    result.abstract_ = true;
                }
                syn::Meta::NameValue(value) if value.path.is_ident("icon") => {
                    let syn::Expr::Lit(value) = value.value else {
                        continue;
                    };
                    let syn::Lit::Str(value) = value.lit else {
                        continue;
                    };
                    let value = value.value();
                    if value.starts_with("res://")
                        && !value.contains('\\')
                        && value.split('/').all(|part| !matches!(part, "." | ".."))
                    {
                        result.icon_path = Some(value);
                    }
                }
                syn::Meta::NameValue(value) if value.path.is_ident("extends") => {
                    let syn::Expr::Lit(value) = value.value else {
                        continue;
                    };
                    let syn::Lit::Str(value) = value.lit else {
                        continue;
                    };
                    let value = value.value();
                    if is_canonical_resource_path(&value) && value.ends_with(".rs") {
                        result.base_script_path = Some(value);
                    }
                }
                syn::Meta::NameValue(value) if value.path.is_ident("class_name") => {
                    let name = match value.value {
                        syn::Expr::Path(path)
                            if path.path.leading_colon.is_none()
                                && path.path.segments.len() == 1 =>
                        {
                            path.path
                                .segments
                                .first()
                                .map(|segment| segment.ident.to_string())
                        }
                        syn::Expr::Lit(value) => match value.lit {
                            syn::Lit::Str(value) => Some(value.value()),
                            _ => None,
                        },
                        _ => None,
                    };
                    if name.as_deref().is_some_and(is_godot_identifier) {
                        result.global_name = name;
                    }
                }
                syn::Meta::NameValue(value) if value.path.is_ident("base") => {
                    let syn::Expr::Path(path) = value.value else {
                        continue;
                    };
                    let name = path
                        .path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string());
                    if name.as_deref().is_some_and(is_godot_identifier) {
                        result.base_class = name;
                    }
                }
                _ => {}
            }
        }
    }
    result
}

pub(crate) fn script_dependencies(source: &str, script_type: &str) -> Vec<String> {
    let attributes = script_source_attributes(source, script_type);
    let mut dependencies = Vec::new();
    if let Some(path) = attributes.base_script_path {
        dependencies.push(path);
    }
    if let Some(path) = attributes.icon_path {
        if !dependencies.contains(&path) {
            dependencies.push(path);
        }
    }
    dependencies
}

pub(crate) fn rename_script_dependencies(
    source: &str,
    script_type: &str,
    renames: &std::collections::HashMap<String, String>,
) -> Result<Option<String>, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let Some(item) = file.items.into_iter().find_map(|item| {
        let syn::Item::Struct(item) = item else {
            return None;
        };
        (item.ident == script_type).then_some(item)
    }) else {
        return Ok(None);
    };
    let mut edits = Vec::<(usize, usize, String)>::new();
    for attribute in item
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("script"))
    {
        for entry in script_attribute_entries(attribute) {
            let syn::Meta::NameValue(value) = entry else {
                continue;
            };
            let dependency_kind = if value.path.is_ident("extends") {
                Some(true)
            } else if value.path.is_ident("icon") {
                Some(false)
            } else {
                None
            };
            let Some(requires_rust_script) = dependency_kind else {
                continue;
            };
            let syn::Expr::Lit(value) = value.value else {
                continue;
            };
            let syn::Lit::Str(value) = value.lit else {
                continue;
            };
            let Some(replacement) = renames.get(&value.value()) else {
                continue;
            };
            if !is_canonical_resource_path(replacement)
                || (requires_rust_script && !replacement.ends_with(".rs"))
            {
                return Err(format!(
                    "replacement dependency path is invalid: {replacement}"
                ));
            }
            let start = source_offset(source, value.span().start())
                .ok_or_else(|| "could not locate dependency string in Rust source".to_owned())?;
            let end = source_offset(source, value.span().end())
                .ok_or_else(|| "could not locate dependency string in Rust source".to_owned())?;
            edits.push((start, end, format!("{replacement:?}")));
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    edits.sort_unstable_by_key(|(start, _, _)| *start);
    if edits.windows(2).any(|entries| entries[0].1 > entries[1].0) {
        return Err("dependency edits overlap in Rust source".to_owned());
    }
    let mut output = source.to_owned();
    for (start, end, replacement) in edits.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    Ok(Some(output))
}

fn script_attribute_entries(attribute: &syn::Attribute) -> Vec<syn::Meta> {
    let syn::Meta::List(list) = &attribute.meta else {
        return Vec::new();
    };
    let tokens = list
        .tokens
        .clone()
        .into_iter()
        .map(|token| match token {
            proc_macro2::TokenTree::Ident(ident) if ident == "abstract" => {
                proc_macro2::TokenTree::Ident(proc_macro2::Ident::new_raw("abstract", ident.span()))
            }
            token => token,
        })
        .collect();
    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
        .parse2(tokens)
        .map(|entries| entries.into_iter().collect())
        .unwrap_or_default()
}

fn source_offset(source: &str, position: proc_macro2::LineColumn) -> Option<usize> {
    let line = position.line.checked_sub(1)?;
    let mut offset = 0_usize;
    for (index, value) in source.split_inclusive('\n').enumerate() {
        if index == line {
            let candidate = offset.checked_add(position.column)?;
            return (candidate <= offset + value.len() && source.is_char_boundary(candidate))
                .then_some(candidate);
        }
        offset = offset.checked_add(value.len())?;
    }
    (line == source.lines().count() && position.column == 0 && offset == source.len())
        .then_some(offset)
}

fn is_canonical_resource_path(value: &str) -> bool {
    let Some(relative) = value.strip_prefix("res://") else {
        return false;
    };
    !relative.is_empty()
        && !relative.contains('\\')
        && relative
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}

fn is_godot_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn literal_constant_value(expression: &syn::Expr) -> Option<SourceConstantValue> {
    match expression {
        syn::Expr::Lit(value) => match &value.lit {
            syn::Lit::Bool(value) => Some(SourceConstantValue::Bool(value.value)),
            syn::Lit::Int(value) => value.base10_parse().ok().map(SourceConstantValue::I64),
            syn::Lit::Float(value) => value.base10_parse().ok().map(SourceConstantValue::F64),
            syn::Lit::Str(value) => Some(SourceConstantValue::String(value.value())),
            _ => None,
        },
        syn::Expr::Unary(value) if matches!(value.op, syn::UnOp::Neg(_)) => {
            match literal_constant_value(&value.expr)? {
                SourceConstantValue::I64(value) => {
                    value.checked_neg().map(SourceConstantValue::I64)
                }
                SourceConstantValue::F64(value) => Some(SourceConstantValue::F64(-value)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_identifier(value: &str) -> bool {
    let Some(first) = value.as_bytes().first().copied() else {
        return false;
    };
    is_identifier_start(first)
        && value
            .bytes()
            .skip(1)
            .all(|byte| is_identifier_start(byte) || byte.is_ascii_digit())
}

/// Finds the Rust type declared by the attachable `#[script(...)]` struct.
///
/// The scanner stays tolerant of incomplete method bodies so signal callback
/// generation continues to work while the editor contains temporary errors.
pub(crate) fn find_script_type(source: &str) -> Option<&str> {
    let mut scanner = Scanner::new(source);
    while let Some(token) = scanner.next_token() {
        if token.text != "#" || scanner.next_token()?.text != "[" {
            continue;
        }
        if scanner.next_token()?.text != "script" {
            continue;
        }
        while scanner.next_token()?.text != "]" {}
        for _ in 0..16 {
            let token = scanner.next_token()?;
            if token.text == "struct" {
                let name = scanner.next_token()?.text;
                let name = name.strip_prefix("r#").unwrap_or(name);
                return name
                    .as_bytes()
                    .first()
                    .copied()
                    .filter(|byte| is_identifier_start(*byte))
                    .map(|_| name);
            }
            if matches!(token.text, "#" | "impl" | "enum" | "trait") {
                break;
            }
        }
    }
    None
}

/// Applies conservative brace-based indentation to an inclusive line range.
///
/// Existing text outside the selected range is byte-for-byte preserved. The
/// scanner ignores braces in comments and literals and never rejects incomplete
/// source, which is important while the editor is actively receiving input.
pub(crate) fn auto_indent(source: &str, from_line: i64, to_line: i64) -> String {
    if source.is_empty() {
        return String::new();
    }
    let line_count = source.split('\n').count();
    let from = usize::try_from(from_line.max(0))
        .unwrap_or(usize::MAX)
        .min(line_count);
    let to = usize::try_from(to_line.max(from_line).max(0))
        .unwrap_or(usize::MAX)
        .min(line_count.saturating_sub(1));
    if from >= line_count || from > to {
        return source.to_owned();
    }

    let mut state = LexicalState::default();
    let mut depth = 0_usize;
    let mut output = String::with_capacity(source.len());

    for (line_index, line) in source.split('\n').enumerate() {
        if line_index != 0 {
            output.push('\n');
        }

        let analysis = analyze_line(line, &mut state);
        if (from..=to).contains(&line_index) && !line.trim().is_empty() {
            let content = line.trim_start_matches([' ', '\t']);
            let line_depth = depth.saturating_sub(analysis.leading_closing_braces);
            for _ in 0..line_depth {
                output.push('\t');
            }
            output.push_str(content);
        } else {
            output.push_str(line);
        }

        depth = depth
            .saturating_sub(analysis.closing_braces)
            .saturating_add(analysis.opening_braces);
    }
    output
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LineAnalysis {
    opening_braces: usize,
    closing_braces: usize,
    leading_closing_braces: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LexicalState {
    #[default]
    Normal,
    BlockComment(usize),
    String {
        escaped: bool,
    },
    Character {
        escaped: bool,
    },
    RawString {
        hashes: usize,
    },
}

fn analyze_line(line: &str, state: &mut LexicalState) -> LineAnalysis {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut result = LineAnalysis::default();
    let mut saw_code = false;

    while index < bytes.len() {
        match *state {
            LexicalState::Normal => {
                if bytes[index..].starts_with(b"//") {
                    break;
                }
                if bytes[index..].starts_with(b"/*") {
                    *state = LexicalState::BlockComment(1);
                    index += 2;
                    continue;
                }
                if let Some((consumed, hashes)) = raw_string_start(&bytes[index..]) {
                    *state = LexicalState::RawString { hashes };
                    index += consumed;
                    saw_code = true;
                    continue;
                }
                match bytes[index] {
                    b'"' => {
                        *state = LexicalState::String { escaped: false };
                        index += 1;
                        saw_code = true;
                    }
                    b'\'' if is_character_literal(&bytes[index..]) => {
                        *state = LexicalState::Character { escaped: false };
                        index += 1;
                        saw_code = true;
                    }
                    b'{' => {
                        result.opening_braces += 1;
                        saw_code = true;
                        index += 1;
                    }
                    b'}' => {
                        result.closing_braces += 1;
                        if !saw_code {
                            result.leading_closing_braces += 1;
                        }
                        saw_code = true;
                        index += 1;
                    }
                    byte => {
                        if !byte.is_ascii_whitespace() {
                            saw_code = true;
                        }
                        index += 1;
                    }
                }
            }
            LexicalState::BlockComment(mut depth) => {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    *state = LexicalState::BlockComment(depth);
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    *state = if depth == 0 {
                        LexicalState::Normal
                    } else {
                        LexicalState::BlockComment(depth)
                    };
                    index += 2;
                } else {
                    index += 1;
                }
            }
            LexicalState::String { mut escaped } => {
                let byte = bytes[index];
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    *state = LexicalState::Normal;
                    index += 1;
                    continue;
                }
                *state = LexicalState::String { escaped };
                index += 1;
            }
            LexicalState::Character { mut escaped } => {
                let byte = bytes[index];
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\'' {
                    *state = LexicalState::Normal;
                    index += 1;
                    continue;
                }
                *state = LexicalState::Character { escaped };
                index += 1;
            }
            LexicalState::RawString { hashes } => {
                if bytes[index] == b'"'
                    && bytes
                        .get(index + 1..index + 1 + hashes)
                        .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
                {
                    *state = LexicalState::Normal;
                    index += hashes + 1;
                } else {
                    index += 1;
                }
            }
        }
    }

    if matches!(
        state,
        LexicalState::String { .. } | LexicalState::Character { .. }
    ) {
        // Ordinary Rust string and character literals cannot span an unescaped
        // newline. Resetting makes recovery from incomplete editor input local.
        *state = LexicalState::Normal;
    }
    result
}

fn raw_string_start(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut index = match bytes {
        [b'r', rest @ ..] if !rest.is_empty() => 1,
        [b'b' | b'c', b'r', ..] => 2,
        _ => return None,
    };
    let hash_start = index;
    while bytes.get(index) == Some(&b'#') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'"')).then_some((index + 1, index - hash_start))
}

fn is_character_literal(bytes: &[u8]) -> bool {
    let mut escaped = false;
    for byte in bytes.iter().copied().skip(1) {
        if byte == b'\n' {
            return false;
        }
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'\'' {
            return true;
        } else if byte.is_ascii_whitespace() {
            return false;
        }
    }
    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Token<'a> {
    text: &'a str,
    line: usize,
}

struct Scanner<'a> {
    source: &'a str,
    offset: usize,
    line: usize,
    block_comment_depth: usize,
}

impl<'a> Scanner<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            offset: 0,
            line: 0,
            block_comment_depth: 0,
        }
    }

    fn next_token(&mut self) -> Option<Token<'a>> {
        let bytes = self.source.as_bytes();
        while self.offset < bytes.len() {
            if self.block_comment_depth != 0 {
                if bytes[self.offset..].starts_with(b"/*") {
                    self.block_comment_depth += 1;
                    self.offset += 2;
                } else if bytes[self.offset..].starts_with(b"*/") {
                    self.block_comment_depth -= 1;
                    self.offset += 2;
                } else {
                    self.bump();
                }
                continue;
            }
            if bytes[self.offset..].starts_with(b"//") {
                while self.offset < bytes.len() && bytes[self.offset] != b'\n' {
                    self.offset += 1;
                }
                continue;
            }
            if bytes[self.offset..].starts_with(b"/*") {
                self.block_comment_depth = 1;
                self.offset += 2;
                continue;
            }
            if let Some((consumed, hashes)) = raw_string_start(&bytes[self.offset..]) {
                self.offset += consumed;
                self.skip_raw_string(hashes);
                continue;
            }
            if bytes[self.offset] == b'"' {
                self.offset += 1;
                self.skip_quoted(b'"');
                continue;
            }
            if bytes[self.offset] == b'\'' && is_character_literal(&bytes[self.offset..]) {
                self.offset += 1;
                self.skip_quoted(b'\'');
                continue;
            }
            if bytes[self.offset] == b'\n' {
                self.bump();
                continue;
            }
            if bytes[self.offset].is_ascii_whitespace() {
                self.offset += 1;
                continue;
            }

            let start = self.offset;
            let token_line = self.line;
            if bytes[self.offset..].starts_with(b"r#")
                && bytes
                    .get(self.offset + 2)
                    .is_some_and(|byte| is_identifier_start(*byte))
            {
                self.offset += 2;
            }
            if bytes
                .get(self.offset)
                .is_some_and(|byte| is_identifier_start(*byte))
            {
                self.offset += 1;
                while bytes
                    .get(self.offset)
                    .is_some_and(|byte| is_identifier_continue(*byte))
                {
                    self.offset += 1;
                }
            } else {
                self.offset += 1;
            }
            return Some(Token {
                text: &self.source[start..self.offset],
                line: token_line,
            });
        }
        None
    }

    fn skip_quoted(&mut self, delimiter: u8) {
        let bytes = self.source.as_bytes();
        let mut escaped = false;
        while self.offset < bytes.len() {
            let byte = bytes[self.offset];
            self.bump();
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter || byte == b'\n' {
                break;
            }
        }
    }

    fn skip_raw_string(&mut self, hashes: usize) {
        let bytes = self.source.as_bytes();
        while self.offset < bytes.len() {
            if bytes[self.offset] == b'"'
                && bytes
                    .get(self.offset + 1..self.offset + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                self.offset += hashes + 1;
                return;
            }
            self.bump();
        }
    }

    fn bump(&mut self) {
        if self.source.as_bytes()[self.offset] == b'\n' {
            self.line += 1;
        }
        self.offset += 1;
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_words_are_unique_and_control_flow_words_are_sorted() {
        let mut words = HIGHLIGHT_WORDS.to_vec();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), HIGHLIGHT_WORDS.len());
        assert!(CONTROL_FLOW_WORDS.is_sorted());
        assert!(HIGHLIGHT_WORDS.contains(&"gen"));
        assert!(HIGHLIGHT_WORDS.contains(&"i64"));
    }

    #[test]
    fn finds_script_methods_and_raw_identifiers_by_zero_based_line() {
        let source = r#"
#[script]
impl Player {
    fn _ready(&mut self) {}

    #[func]
    fn r#type(&self) -> String {
        "fn fake() {}".into()
    }
}
"#;
        assert_eq!(find_function_line(source, "_ready"), Some(3));
        assert_eq!(find_function_line(source, "type"), Some(6));
        assert_eq!(find_function_line(source, "r#type"), Some(6));
        assert_eq!(
            function_declarations(source),
            vec![("_ready".to_owned(), 3), ("type".to_owned(), 6)]
        );
        assert_eq!(find_declaration_line(source, "Player"), None);
    }

    #[test]
    fn completion_identifiers_and_navigation_ignore_literals() {
        let source = r#"
struct Player;
const SPEED: f32 = 2.0;
impl Player {
    fn accelerate(&mut self) {
        let velocity = "ignored_literal";
    }
}
"#;
        let values = identifiers(source);
        assert!(values.contains(&"Player".to_owned()));
        assert!(values.contains(&"velocity".to_owned()));
        assert!(!values.contains(&"ignored_literal".to_owned()));
        assert_eq!(find_declaration_line(source, "Player"), Some(1));
        assert_eq!(find_declaration_line(source, "SPEED"), Some(2));
        assert_eq!(find_declaration_line(source, "accelerate"), Some(4));
        assert_eq!(
            documentation_before_line(
                "/// Player speed.\n/// Measured per second.\nconst SPEED: f32 = 2.0;",
                2,
            ),
            "Player speed.\nMeasured per second."
        );
    }

    #[test]
    fn script_constants_include_only_exact_literal_scalars() {
        let source = r#"
impl Player {
    const ENABLED: bool = true;
    const LIVES: i64 = 3;
    const OFFSET: i64 = -2;
    const SPEED: f64 = 4.5;
    const TITLE: &str = "Rust";
    const COMPUTED: i64 = 1 + 2;
}
impl Enemy {
    const LIVES: i64 = 99;
}
"#;
        let constants = script_constants(source, "Player");
        assert_eq!(constants.len(), 5);
        assert_eq!(constants[0].name, "ENABLED");
        assert_eq!(constants[1].value, SourceConstantValue::I64(3));
        assert_eq!(constants[2].value, SourceConstantValue::I64(-2));
        assert_eq!(constants[3].value, SourceConstantValue::F64(4.5));
        assert_eq!(
            constants[4].value,
            SourceConstantValue::String("Rust".to_owned())
        );
    }

    #[test]
    fn script_attributes_expose_abstract_state_and_canonical_icon() {
        let source = r#"
#[script(
    base = Node2D,
    class_name = PlayerController,
    icon = "res://icons/player.svg",
    abstract
)]
pub struct Player;

#[script(base = Node)]
pub struct Enemy;
"#;
        assert_eq!(
            script_source_attributes(source, "Player"),
            ScriptSourceAttributes {
                abstract_: true,
                icon_path: Some("res://icons/player.svg".to_owned()),
                base_script_path: None,
                global_name: Some("PlayerController".to_owned()),
                base_class: Some("Node2D".to_owned()),
            }
        );
        assert_eq!(
            script_source_attributes(source, "Enemy"),
            ScriptSourceAttributes {
                base_class: Some("Node".to_owned()),
                ..ScriptSourceAttributes::default()
            }
        );
        assert_eq!(
            script_source_attributes(
                "#[script(base = Node, icon = \"../icon.svg\")] struct Player;",
                "Player",
            ),
            ScriptSourceAttributes {
                base_class: Some("Node".to_owned()),
                ..ScriptSourceAttributes::default()
            }
        );
    }

    #[test]
    fn script_dependencies_are_reported_and_renamed_without_reformatting() {
        let source = r#"
#[script(
    extends = "res://scripts/base.rs",
    icon = "res://icons/player.svg",
    abstract,
)]
pub struct Player;
"#;
        assert_eq!(
            script_dependencies(source, "Player"),
            [
                "res://scripts/base.rs".to_owned(),
                "res://icons/player.svg".to_owned()
            ]
        );
        let renames = std::collections::HashMap::from([
            (
                "res://scripts/base.rs".to_owned(),
                "res://actors/base.rs".to_owned(),
            ),
            (
                "res://icons/player.svg".to_owned(),
                "res://art/player.svg".to_owned(),
            ),
        ]);
        let renamed = rename_script_dependencies(source, "Player", &renames)
            .expect("valid dependency renames")
            .expect("source changed");
        assert_eq!(
            renamed,
            source
                .replace("res://scripts/base.rs", "res://actors/base.rs")
                .replace("res://icons/player.svg", "res://art/player.svg")
        );
        assert_eq!(
            rename_script_dependencies(&renamed, "Player", &renames)
                .expect("unchanged source is valid"),
            None
        );
    }

    #[test]
    fn finds_the_attachable_script_type_without_parsing_method_bodies() {
        let source = r#"
#[script(base = Node2D, tool)]
pub struct Player {
    text: String,
}

impl Player {
    fn unfinished(&mut self) {
"#;
        assert_eq!(find_script_type(source), Some("Player"));
        assert_eq!(
            find_script_type(r##"const TEXT: &str = "#[script] struct Fake";"##),
            None
        );
    }

    #[test]
    fn ignores_functions_inside_every_rust_comment_and_string_form() {
        let source = r####"
// fn line_comment() {}
/* fn block_comment() {
    /* fn nested_comment() {} */
} */
const NORMAL: &str = "fn normal_string() {}";
const RAW: &str = r###"fn raw_string() { "quoted" }"###;
const BYTE: &[u8] = br#"fn raw_byte_string() {}"#;
const CHARACTER: char = '}';
fn real() {}
"####;
        for ignored in [
            "line_comment",
            "block_comment",
            "nested_comment",
            "normal_string",
            "raw_string",
            "raw_byte_string",
        ] {
            assert_eq!(find_function_line(source, ignored), None, "{ignored}");
        }
        assert_eq!(find_function_line(source, "real"), Some(9));
    }

    #[test]
    fn indentation_uses_code_braces_without_touching_literals_or_comments() {
        let source =
            "impl Player {\nfn ready() {\nlet text = \"}\";\n/* { */\nif true {\nrun();\n}\n}\n}";
        assert_eq!(
            auto_indent(source, 1, 8),
            "impl Player {\n\tfn ready() {\n\t\tlet text = \"}\";\n\t\t/* { */\n\t\tif true {\n\t\t\trun();\n\t\t}\n\t}\n}"
        );
    }

    #[test]
    fn indentation_preserves_unselected_and_blank_lines() {
        let source = "impl Player {\n  fn ready() {\n\nrun();\n }\n}";
        assert_eq!(
            auto_indent(source, 3, 3),
            "impl Player {\n  fn ready() {\n\n\t\trun();\n }\n}"
        );
        assert_eq!(auto_indent(source, 99, 100), source);
    }

    #[test]
    fn incomplete_editor_input_never_loses_source() {
        let source = "impl Player {\nfn ready() {\nlet text = \"unfinished\n}";
        let output = auto_indent(source, 0, 99);
        assert!(output.contains("unfinished"));
        assert_eq!(output.lines().count(), source.lines().count());
    }
}
