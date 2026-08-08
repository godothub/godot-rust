const DEFAULT_BASE_CLASS: &str = "Node";

pub(crate) fn render(template: &str, class_name: &str, base_class: &str) -> String {
    let class_name = rust_type_name(class_name);
    let base_class = rust_base_class(base_class);
    let template = if template.trim().is_empty() {
        "use godot_rs::prelude::*;\n\n\
         #[script(base = _BASE_)]\n\
         pub struct _CLASS_;\n\n\
         #[script]\n\
         impl _CLASS_ {\n\
         \tfn _ready(&mut self) {\n\
         \t}\n\
         }\n"
    } else {
        template
    };
    template
        .replace("_BASE_", base_class)
        .replace("_CLASS_SNAKE_CASE_", &rust_module_name(&class_name))
        .replace("_CLASS_", &class_name)
        .replace("_TS_", "\t")
}

fn rust_type_name(value: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if result.is_empty() && character.is_ascii_digit() {
                result.push_str("Script");
            }
            if capitalize {
                result.push(character.to_ascii_uppercase());
                capitalize = false;
            } else {
                result.push(character);
            }
        } else {
            capitalize = true;
        }
    }
    if result.is_empty() {
        result = format!("GodotScript{:08X}", stable_hash(value));
    }
    if matches!(result.as_str(), "Self" | "Super" | "Crate") {
        result.insert_str(0, "Godot");
    }
    result
}

fn rust_module_name(class_name: &str) -> String {
    let mut result = String::with_capacity(class_name.len());
    for (index, character) in class_name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index != 0 {
                result.push('_');
            }
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character.to_ascii_lowercase());
        }
    }
    result
}

fn rust_base_class(value: &str) -> &str {
    let mut characters = value.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    let valid_rest =
        characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if valid_start && valid_rest && !matches!(value, "self" | "Self" | "super" | "crate") {
        value
    } else {
        DEFAULT_BASE_CLASS
    }
}

fn stable_hash(value: &str) -> u32 {
    value.as_bytes().iter().fold(2_166_136_261, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_template_produces_a_minimal_godot_style_script() {
        let source = render("", "player_controller", "CharacterBody2D");
        assert!(source.contains("#[script(base = CharacterBody2D)]"));
        assert!(source.contains("pub struct PlayerController;"));
        assert!(source.contains("fn _ready(&mut self)"));
        assert!(!source.contains("_BASE_"));
        assert!(!source.contains("_CLASS_"));
    }

    #[test]
    fn custom_templates_use_the_official_godot_placeholders() {
        assert_eq!(
            render(
                "_CLASS_|_CLASS_SNAKE_CASE_|_BASE_|_TS_done",
                "health_bar",
                "Control",
            ),
            "HealthBar|health_bar|Control|\tdone"
        );
    }

    #[test]
    fn filenames_that_are_not_rust_identifiers_get_stable_safe_names() {
        let chinese = rust_type_name("玩家");
        assert!(chinese.starts_with("GodotScript"));
        assert!(
            chinese
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        );
        assert_eq!(rust_type_name("2d-player"), "Script2dPlayer");
        assert_eq!(rust_type_name("self"), "GodotSelf");
    }

    #[test]
    fn invalid_or_script_path_bases_fail_closed_to_node() {
        assert_eq!(rust_base_class("Node2D"), "Node2D");
        assert_eq!(rust_base_class("\"res://base.rs\""), "Node");
        assert_eq!(rust_base_class(""), "Node");
    }
}
