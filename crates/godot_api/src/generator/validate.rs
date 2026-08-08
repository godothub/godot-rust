use std::collections::HashSet;
use std::fmt;

use crate::ExtensionApi;

const BUILD_CONFIGURATIONS: [&str; 4] = ["float_32", "float_64", "double_32", "double_64"];

/// Godot Major/Minor expected by a generation target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpectedApiVersion {
    pub major: u32,
    pub minor: u32,
}

/// One deterministic validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

/// Validates structural invariants required before code generation.
#[must_use]
pub fn validate_api(api: &ExtensionApi, expected: ExpectedApiVersion) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if api.header.version_major != expected.major || api.header.version_minor != expected.minor {
        issues.push(ValidationIssue {
            path: "header".into(),
            message: format!(
                "expected Godot {}.{}, found {}.{}",
                expected.major, expected.minor, api.header.version_major, api.header.version_minor
            ),
        });
    }
    if api.header.version_status != "stable" {
        issues.push(ValidationIssue {
            path: "header.version_status".into(),
            message: format!(
                "official binding input must be stable, found `{}`",
                api.header.version_status
            ),
        });
    }
    if api.header.version_build != "official" {
        issues.push(ValidationIssue {
            path: "header.version_build".into(),
            message: format!(
                "official binding input must use the official build, found `{}`",
                api.header.version_build
            ),
        });
    }

    validate_unique_names(
        api.builtin_classes.iter().map(|class| class.name.as_str()),
        "builtin_classes",
        &mut issues,
    );
    validate_unique_names(
        api.classes.iter().map(|class| class.name.as_str()),
        "classes",
        &mut issues,
    );
    validate_unique_names(
        api.builtin_class_sizes
            .iter()
            .map(|configuration| configuration.build_configuration.as_str()),
        "builtin_class_sizes",
        &mut issues,
    );
    validate_unique_names(
        api.builtin_class_member_offsets
            .iter()
            .map(|configuration| configuration.build_configuration.as_str()),
        "builtin_class_member_offsets",
        &mut issues,
    );
    validate_unique_names(
        api.global_constants
            .iter()
            .map(|constant| constant.name.as_str()),
        "global_constants",
        &mut issues,
    );
    validate_unique_names(
        api.global_enums.iter().map(|enum_| enum_.name.as_str()),
        "global_enums",
        &mut issues,
    );
    validate_unique_names(
        api.utility_functions
            .iter()
            .map(|function| function.name.as_str()),
        "utility_functions",
        &mut issues,
    );
    validate_unique_names(
        api.singletons
            .iter()
            .map(|singleton| singleton.name.as_str()),
        "singletons",
        &mut issues,
    );
    validate_unique_names(
        api.native_structures
            .iter()
            .map(|structure| structure.name.as_str()),
        "native_structures",
        &mut issues,
    );

    let class_names: HashSet<&str> = api
        .classes
        .iter()
        .map(|class| class.name.as_str())
        .collect();
    for required in ["Object", "ScriptLanguageExtension", "ScriptExtension"] {
        if !class_names.contains(required) {
            issues.push(ValidationIssue {
                path: "classes".into(),
                message: format!("required class `{required}` is missing"),
            });
        }
    }

    for class in &api.classes {
        if let Some(parent) = class.inherits.as_deref() {
            if !class_names.contains(parent) {
                issues.push(ValidationIssue {
                    path: format!("classes.{}.inherits", class.name),
                    message: format!("unknown parent class `{parent}`"),
                });
            }
        }

        validate_unique_names(
            class.methods.iter().map(|method| method.name.as_str()),
            &format!("classes.{}.methods", class.name),
            &mut issues,
        );
        validate_unique_names(
            class
                .properties
                .iter()
                .map(|property| property.name.as_str()),
            &format!("classes.{}.properties", class.name),
            &mut issues,
        );
        validate_unique_names(
            class.signals.iter().map(|signal| signal.name.as_str()),
            &format!("classes.{}.signals", class.name),
            &mut issues,
        );
        validate_unique_names(
            class.enums.iter().map(|enum_| enum_.name.as_str()),
            &format!("classes.{}.enums", class.name),
            &mut issues,
        );
        validate_unique_names(
            class
                .constants
                .iter()
                .map(|constant| constant.name.as_str()),
            &format!("classes.{}.constants", class.name),
            &mut issues,
        );
        for method in &class.methods {
            if !method.is_virtual && method.hash.is_none() {
                issues.push(ValidationIssue {
                    path: format!("classes.{}.methods.{}", class.name, method.name),
                    message: "non-virtual method has no Method Bind hash".into(),
                });
            }
            if method.is_required && !method.is_virtual {
                issues.push(ValidationIssue {
                    path: format!("classes.{}.methods.{}", class.name, method.name),
                    message: "only virtual methods may be required".into(),
                });
            }
            validate_method_hashes(
                method.hash,
                &method.hash_compatibility,
                &format!("classes.{}.methods.{}", class.name, method.name),
                &mut issues,
            );
            validate_arguments(
                &method.arguments,
                &format!("classes.{}.methods.{}.arguments", class.name, method.name),
                &mut issues,
            );
            if let Some(return_value) = &method.return_value {
                validate_type_name(
                    &return_value.r#type,
                    &format!(
                        "classes.{}.methods.{}.return_value",
                        class.name, method.name
                    ),
                    &mut issues,
                );
            }
        }
        for property in &class.properties {
            validate_type_name(
                &property.r#type,
                &format!("classes.{}.properties.{}", class.name, property.name),
                &mut issues,
            );
            if property.getter.is_empty() {
                issues.push(ValidationIssue {
                    path: format!("classes.{}.properties.{}.getter", class.name, property.name),
                    message: "getter cannot be empty".into(),
                });
            }
            if property.setter.as_deref() == Some("") {
                issues.push(ValidationIssue {
                    path: format!("classes.{}.properties.{}.setter", class.name, property.name),
                    message: "setter cannot be empty".into(),
                });
            }
        }
        for signal in &class.signals {
            validate_arguments(
                &signal.arguments,
                &format!("classes.{}.signals.{}.arguments", class.name, signal.name),
                &mut issues,
            );
        }
        for enum_ in &class.enums {
            validate_enum(
                enum_,
                &format!("classes.{}.enums.{}", class.name, enum_.name),
                &mut issues,
            );
        }
    }

    validate_build_configurations(
        api.builtin_class_sizes
            .iter()
            .map(|configuration| configuration.build_configuration.as_str()),
        "builtin_class_sizes",
        &mut issues,
    );
    validate_build_configurations(
        api.builtin_class_member_offsets
            .iter()
            .map(|configuration| configuration.build_configuration.as_str()),
        "builtin_class_member_offsets",
        &mut issues,
    );

    let mut expected_size_names: Option<HashSet<&str>> = None;
    for configuration in &api.builtin_class_sizes {
        let path = format!("builtin_class_sizes.{}", configuration.build_configuration);
        validate_unique_names(
            configuration.sizes.iter().map(|size| size.name.as_str()),
            &path,
            &mut issues,
        );
        let names = configuration
            .sizes
            .iter()
            .map(|size| size.name.as_str())
            .collect::<HashSet<_>>();
        if let Some(expected_names) = &expected_size_names {
            if &names != expected_names {
                issues.push(ValidationIssue {
                    path,
                    message: "builtin size names differ between build configurations".into(),
                });
            }
        } else {
            expected_size_names = Some(names);
        }
    }

    let mut expected_offset_shape: Option<Vec<(&str, Vec<&str>)>> = None;
    for configuration in &api.builtin_class_member_offsets {
        let path = format!(
            "builtin_class_member_offsets.{}",
            configuration.build_configuration
        );
        validate_unique_names(
            configuration
                .classes
                .iter()
                .map(|class| class.name.as_str()),
            &path,
            &mut issues,
        );
        for class in &configuration.classes {
            validate_unique_names(
                class.members.iter().map(|member| member.member.as_str()),
                &format!("{path}.{}", class.name),
                &mut issues,
            );
        }
        let mut shape = configuration
            .classes
            .iter()
            .map(|class| {
                let mut members = class
                    .members
                    .iter()
                    .map(|member| member.member.as_str())
                    .collect::<Vec<_>>();
                members.sort_unstable();
                (class.name.as_str(), members)
            })
            .collect::<Vec<_>>();
        shape.sort_by_key(|(name, _)| *name);
        if let Some(expected_shape) = &expected_offset_shape {
            if &shape != expected_shape {
                issues.push(ValidationIssue {
                    path,
                    message: "builtin member-offset shape differs between build configurations"
                        .into(),
                });
            }
        } else {
            expected_offset_shape = Some(shape);
        }
    }

    for enum_ in &api.global_enums {
        validate_enum(enum_, &format!("global_enums.{}", enum_.name), &mut issues);
    }
    for function in &api.utility_functions {
        validate_method_hashes(
            Some(function.hash),
            &[],
            &format!("utility_functions.{}", function.name),
            &mut issues,
        );
        validate_arguments(
            &function.arguments,
            &format!("utility_functions.{}.arguments", function.name),
            &mut issues,
        );
        if let Some(return_type) = &function.return_type {
            validate_type_name(
                return_type,
                &format!("utility_functions.{}.return_type", function.name),
                &mut issues,
            );
        }
    }
    for class in &api.builtin_classes {
        let path = format!("builtin_classes.{}", class.name);
        validate_unique_names(
            class.methods.iter().map(|method| method.name.as_str()),
            &format!("{path}.methods"),
            &mut issues,
        );
        validate_unique_names(
            class.members.iter().map(|member| member.name.as_str()),
            &format!("{path}.members"),
            &mut issues,
        );
        validate_unique_names(
            class
                .constants
                .iter()
                .map(|constant| constant.name.as_str()),
            &format!("{path}.constants"),
            &mut issues,
        );
        validate_unique_names(
            class.enums.iter().map(|enum_| enum_.name.as_str()),
            &format!("{path}.enums"),
            &mut issues,
        );

        let indices = class
            .constructors
            .iter()
            .map(|constructor| constructor.index)
            .collect::<Vec<_>>();
        let expected_indices =
            (0..u32::try_from(indices.len()).unwrap_or(u32::MAX)).collect::<Vec<_>>();
        if indices != expected_indices {
            issues.push(ValidationIssue {
                path: format!("{path}.constructors"),
                message: "constructor indices must be contiguous and start at zero".into(),
            });
        }
        for constructor in &class.constructors {
            validate_arguments(
                &constructor.arguments,
                &format!("{path}.constructors.{}.arguments", constructor.index),
                &mut issues,
            );
        }

        let mut operators = HashSet::new();
        for operator in &class.operators {
            if !operators.insert((operator.name.as_str(), operator.right_type.as_deref())) {
                issues.push(ValidationIssue {
                    path: format!("{path}.operators"),
                    message: format!(
                        "duplicate operator `{}` with right type {:?}",
                        operator.name, operator.right_type
                    ),
                });
            }
            validate_type_name(
                &operator.return_type,
                &format!("{path}.operators.{}.return_type", operator.name),
                &mut issues,
            );
            if let Some(right_type) = &operator.right_type {
                validate_type_name(
                    right_type,
                    &format!("{path}.operators.{}.right_type", operator.name),
                    &mut issues,
                );
            }
        }
        for method in &class.methods {
            validate_method_hashes(
                Some(method.hash),
                &method.hash_compatibility,
                &format!("{path}.methods.{}", method.name),
                &mut issues,
            );
            validate_arguments(
                &method.arguments,
                &format!("{path}.methods.{}.arguments", method.name),
                &mut issues,
            );
            if let Some(return_type) = &method.return_type {
                validate_type_name(
                    return_type,
                    &format!("{path}.methods.{}.return_type", method.name),
                    &mut issues,
                );
            }
        }
        for member in &class.members {
            validate_type_name(
                &member.r#type,
                &format!("{path}.members.{}", member.name),
                &mut issues,
            );
        }
        for enum_ in &class.enums {
            validate_unique_names(
                enum_.values.iter().map(|value| value.name.as_str()),
                &format!("{path}.enums.{}", enum_.name),
                &mut issues,
            );
        }
    }

    validate_inheritance_cycles(api, &mut issues);

    issues.sort_by(|left, right| (&left.path, &left.message).cmp(&(&right.path, &right.message)));
    issues
}

fn validate_build_configurations<'a>(
    configurations: impl Iterator<Item = &'a str>,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let actual = configurations.collect::<HashSet<_>>();
    let expected = BUILD_CONFIGURATIONS.into_iter().collect::<HashSet<_>>();
    if actual != expected {
        let mut actual = actual.into_iter().collect::<Vec<_>>();
        actual.sort_unstable();
        issues.push(ValidationIssue {
            path: path.into(),
            message: format!(
                "build configurations must be exactly `{}`, found `{}`",
                BUILD_CONFIGURATIONS.join(", "),
                actual.join(", ")
            ),
        });
    }
}

fn validate_method_hashes(
    current: Option<u64>,
    compatibility: &[u64],
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for hash in compatibility {
        if Some(*hash) == current {
            issues.push(ValidationIssue {
                path: format!("{path}.hash_compatibility"),
                message: format!("current hash `{hash}` is repeated as a compatibility hash"),
            });
        }
    }
}

fn validate_arguments(
    arguments: &[crate::ApiArgument],
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    validate_unique_names(
        arguments.iter().map(|argument| argument.name.as_str()),
        path,
        issues,
    );
    for argument in arguments {
        validate_type_name(
            &argument.r#type,
            &format!("{path}.{}", argument.name),
            issues,
        );
    }
}

fn validate_type_name(value: &str, path: &str, issues: &mut Vec<ValidationIssue>) {
    if value.is_empty()
        || value.trim() != value
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        issues.push(ValidationIssue {
            path: path.into(),
            message: format!("invalid API type spelling `{value}`"),
        });
    }
}

fn validate_enum(enum_: &crate::ApiEnum, path: &str, issues: &mut Vec<ValidationIssue>) {
    validate_unique_names(
        enum_.values.iter().map(|value| value.name.as_str()),
        path,
        issues,
    );
    if enum_.values.is_empty() {
        issues.push(ValidationIssue {
            path: path.into(),
            message: "enum has no values".into(),
        });
    }
}

fn validate_inheritance_cycles(api: &ExtensionApi, issues: &mut Vec<ValidationIssue>) {
    let parents = api
        .classes
        .iter()
        .map(|class| (class.name.as_str(), class.inherits.as_deref()))
        .collect::<std::collections::HashMap<_, _>>();
    for class in &api.classes {
        let mut seen = HashSet::new();
        let mut current = Some(class.name.as_str());
        while let Some(name) = current {
            if !seen.insert(name) {
                issues.push(ValidationIssue {
                    path: format!("classes.{}.inherits", class.name),
                    message: format!("inheritance cycle reaches `{name}`"),
                });
                break;
            }
            current = parents.get(name).copied().flatten();
        }
    }
}

fn validate_unique_names<'a>(
    names: impl Iterator<Item = &'a str>,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut known = HashSet::new();
    for name in names {
        if !known.insert(name) {
            issues.push(ValidationIssue {
                path: path.into(),
                message: format!("duplicate name `{name}`"),
            });
        }
    }
}
