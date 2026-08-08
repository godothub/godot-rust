//! Rust callback source generated for Godot's signal connection window.

use std::collections::HashSet;
use std::fmt::Write;

const BODY_COMMENT: &str = "// TODO: Handle the connected Godot signal.";

pub(crate) fn render(
    script_type: &str,
    function_name: &str,
    arguments: &[String],
    mut is_godot_class: impl FnMut(&str) -> bool,
) -> Result<String, String> {
    if !is_rust_identifier(script_type) {
        return Err(format!(
            "`{script_type}` is not a supported Rust script type"
        ));
    }
    if !is_rust_identifier(function_name) || is_reserved_path_keyword(function_name) {
        return Err(format!(
            "`{function_name}` cannot be represented as a Rust method name"
        ));
    }

    let mut names = HashSet::with_capacity(arguments.len());
    let mut rendered_arguments = Vec::with_capacity(arguments.len());
    for (index, argument) in arguments.iter().enumerate() {
        let (name, godot_type) = argument
            .split_once(':')
            .ok_or_else(|| format!("signal argument `{argument}` has no Godot type"))?;
        let name = unique_argument_name(name, index, &mut names);
        let rust_type = rust_type(godot_type.trim(), &mut is_godot_class)?;
        rendered_arguments.push(format!("{name}: {rust_type}"));
    }

    let method_name = raw_identifier(function_name);
    let signature = if rendered_arguments.is_empty() {
        format!("fn {method_name}(&mut self)")
    } else {
        format!(
            "fn {method_name}(&mut self, {})",
            rendered_arguments.join(", ")
        )
    };
    let mut source = String::new();
    writeln!(source, "#[script]").expect("String writes cannot fail");
    writeln!(source, "impl {script_type} {{").expect("String writes cannot fail");
    writeln!(source, "    #[func]").expect("String writes cannot fail");
    writeln!(source, "    {signature} {{").expect("String writes cannot fail");
    writeln!(source, "        {BODY_COMMENT}").expect("String writes cannot fail");
    writeln!(source, "    }}").expect("String writes cannot fail");
    write!(source, "}}").expect("String writes cannot fail");
    Ok(source)
}

pub(crate) fn render_failure(script_type: &str, function_name: &str, error: &str) -> String {
    format!("// godot-rust could not create `{function_name}` for `{script_type}`:\n// {error}")
}

fn rust_type(
    godot_type: &str,
    is_godot_class: &mut impl FnMut(&str) -> bool,
) -> Result<String, String> {
    let value = match godot_type {
        "bool" => Some("bool"),
        "int" => Some("i64"),
        "float" => Some("f64"),
        "String" => Some("String"),
        "StringName" => Some("StringName"),
        "NodePath" => Some("NodePath"),
        "RID" => Some("Rid"),
        "Callable" => Some("Callable"),
        "Signal" => Some("Signal<()>"),
        "Variant" => Some("Variant"),
        "Array" => Some("Array<Variant>"),
        "Dictionary" => Some("Dictionary"),
        "Vector2" => Some("Vector2"),
        "Vector2i" => Some("Vector2i"),
        "Vector3" => Some("Vector3"),
        "Vector3i" => Some("Vector3i"),
        "Vector4" => Some("Vector4"),
        "Vector4i" => Some("Vector4i"),
        "Rect2" => Some("Rect2"),
        "Rect2i" => Some("Rect2i"),
        "Quaternion" => Some("Quaternion"),
        "Plane" => Some("Plane"),
        "Transform2D" => Some("Transform2D"),
        "AABB" => Some("Aabb"),
        "Basis" => Some("Basis"),
        "Transform3D" => Some("Transform3D"),
        "Projection" => Some("Projection"),
        "Color" => Some("Color"),
        "PackedByteArray" => Some("PackedByteArray"),
        "PackedInt32Array" => Some("PackedInt32Array"),
        "PackedInt64Array" => Some("PackedInt64Array"),
        "PackedFloat32Array" => Some("PackedFloat32Array"),
        "PackedFloat64Array" => Some("PackedFloat64Array"),
        "PackedStringArray" => Some("PackedStringArray"),
        "PackedVector2Array" => Some("PackedVector2Array"),
        "PackedVector3Array" => Some("PackedVector3Array"),
        "PackedColorArray" => Some("PackedColorArray"),
        "PackedVector4Array" => Some("PackedVector4Array"),
        _ => None,
    };
    if let Some(value) = value {
        return Ok(value.to_owned());
    }
    if is_rust_identifier(godot_type) && is_godot_class(godot_type) {
        return Ok(format!("Option<ObjectRef<{godot_type}>>"));
    }
    Err(format!(
        "Godot signal type `{godot_type}` is not supported by generated Rust callbacks"
    ))
}

fn unique_argument_name(name: &str, index: usize, used: &mut HashSet<String>) -> String {
    let mut normalized = String::from("_");
    for (position, character) in name.chars().enumerate() {
        if character == '_' || character.is_ascii_alphanumeric() {
            if position == 0 && character.is_ascii_digit() {
                normalized.push('_');
            }
            normalized.push(character);
        } else {
            normalized.push('_');
        }
    }
    if normalized == "_" {
        normalized.push_str(&format!("arg_{}", index + 1));
    }
    if is_reserved_path_keyword(normalized.trim_start_matches('_')) {
        normalized.push_str("_arg");
    }
    let base = normalized.clone();
    let mut suffix = 2;
    while !used.insert(normalized.clone()) {
        normalized = format!("{base}_{suffix}");
        suffix += 1;
    }
    normalized
}

fn raw_identifier(name: &str) -> String {
    if is_rust_keyword(name) {
        format!("r#{name}")
    } else {
        name.to_owned()
    }
}

fn is_rust_identifier(name: &str) -> bool {
    let mut characters = name.bytes();
    matches!(characters.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && characters.all(|value| value.is_ascii_alphanumeric() || value == b'_')
}

fn is_reserved_path_keyword(name: &str) -> bool {
    matches!(name, "crate" | "self" | "Self" | "super")
}

fn is_rust_keyword(name: &str) -> bool {
    crate::rust_source::HIGHLIGHT_WORDS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_supported_variant_family_and_nullable_objects() {
        let source = render(
            "Player",
            "_on_body_entered",
            &[
                "body:Node2D".into(),
                "shape_index:int".into(),
                "message:String".into(),
                "name:StringName".into(),
                "path:NodePath".into(),
                "rid:RID".into(),
                "callable:Callable".into(),
                "signal:Signal".into(),
                "value:Variant".into(),
                "values:Array".into(),
                "mapping:Dictionary".into(),
                "cell:Vector2i".into(),
                "voxel:Vector3i".into(),
                "transform:Transform3D".into(),
                "box:AABB".into(),
                "bytes:PackedByteArray".into(),
            ],
            |class| class == "Node2D",
        )
        .expect("supported callback");
        assert!(source.contains("impl Player"));
        assert!(source.contains("_body: Option<ObjectRef<Node2D>>"));
        assert!(source.contains("_shape_index: i64"));
        assert!(source.contains("_message: String"));
        assert!(source.contains("_name: StringName"));
        assert!(source.contains("_path: NodePath"));
        assert!(source.contains("_rid: Rid"));
        assert!(source.contains("_callable: Callable"));
        assert!(source.contains("_signal: Signal<()>"));
        assert!(source.contains("_value: Variant"));
        assert!(source.contains("_values: Array<Variant>"));
        assert!(source.contains("_mapping: Dictionary"));
        assert!(source.contains("_cell: Vector2i"));
        assert!(source.contains("_voxel: Vector3i"));
        assert!(source.contains("_transform: Transform3D"));
        assert!(source.contains("_box: Aabb"));
        assert!(source.contains("_bytes: PackedByteArray"));
    }

    #[test]
    fn sanitizes_argument_names_but_never_changes_the_connected_method() {
        let source = render(
            "Player",
            "_on_changed",
            &["type:int".into(), "value-name:float".into()],
            |_| false,
        )
        .expect("supported callback");
        assert!(source.contains("fn _on_changed"));
        assert!(source.contains("_type: i64"));
        assert!(source.contains("_value_name: f64"));
        assert!(render("Player", "invalid-name", &[], |_| false).is_err());
    }

    #[test]
    fn unknown_godot_values_produce_an_actionable_comment() {
        let error = render(
            "Player",
            "_on_transform",
            &["future_value:FutureBuiltin".into()],
            |_| false,
        )
        .expect_err("unknown builtin must fail");
        let comment = render_failure("Player", "_on_transform", &error);
        assert!(comment.contains("FutureBuiltin"));
        assert!(comment.contains("could not create"));
    }
}
