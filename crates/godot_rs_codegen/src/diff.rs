use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::ExtensionApi;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MethodSignature {
    hash: Option<u64>,
    argument_count: usize,
    is_virtual: bool,
    is_const: bool,
    is_static: bool,
    is_vararg: bool,
}

/// Deterministic structural difference between two official API dumps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApiDiff {
    pub from_version: String,
    pub to_version: String,
    pub added_classes: Vec<String>,
    pub removed_classes: Vec<String>,
    pub added_methods: Vec<String>,
    pub removed_methods: Vec<String>,
    pub changed_methods: Vec<String>,
    pub changed_builtin_sizes: Vec<String>,
}

impl ApiDiff {
    /// Returns whether both API inputs are structurally equivalent for the
    /// currently generated surface.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added_classes.is_empty()
            && self.removed_classes.is_empty()
            && self.added_methods.is_empty()
            && self.removed_methods.is_empty()
            && self.changed_methods.is_empty()
            && self.changed_builtin_sizes.is_empty()
    }

    /// Renders a review-friendly Markdown report.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut output = String::new();
        writeln!(
            output,
            "# Godot GDExtension API 差异：{} → {}",
            self.from_version, self.to_version
        )
        .unwrap();
        writeln!(output).unwrap();
        if self.is_empty() {
            writeln!(output, "当前生成表面没有结构变化。").unwrap();
            return output;
        }

        write_section(&mut output, "新增 Class", &self.added_classes);
        write_section(&mut output, "删除 Class", &self.removed_classes);
        write_section(&mut output, "新增 Method", &self.added_methods);
        write_section(&mut output, "删除 Method", &self.removed_methods);
        write_section(&mut output, "变更 Method", &self.changed_methods);
        write_section(
            &mut output,
            "变更 Builtin Size",
            &self.changed_builtin_sizes,
        );
        output
    }
}

/// Compares the generated Class, Method, and Builtin Size surfaces.
#[must_use]
pub fn diff_api(from: &ExtensionApi, to: &ExtensionApi) -> ApiDiff {
    let from_classes: BTreeSet<&str> = from
        .classes
        .iter()
        .map(|class| class.name.as_str())
        .collect();
    let to_classes: BTreeSet<&str> = to.classes.iter().map(|class| class.name.as_str()).collect();

    let from_methods = methods(from);
    let to_methods = methods(to);
    let from_sizes = builtin_sizes(from);
    let to_sizes = builtin_sizes(to);

    let mut diff = ApiDiff {
        from_version: version(from),
        to_version: version(to),
        added_classes: to_classes
            .difference(&from_classes)
            .map(ToString::to_string)
            .collect(),
        removed_classes: from_classes
            .difference(&to_classes)
            .map(ToString::to_string)
            .collect(),
        ..ApiDiff::default()
    };

    for key in to_methods.keys() {
        if !from_methods.contains_key(key) {
            diff.added_methods.push(method_name(*key));
        }
    }
    for key in from_methods.keys() {
        if !to_methods.contains_key(key) {
            diff.removed_methods.push(method_name(*key));
        }
    }
    for (key, from_signature) in &from_methods {
        if let Some(to_signature) = to_methods.get(key) {
            if from_signature != to_signature {
                diff.changed_methods.push(format!(
                    "{}: {:?} → {:?}",
                    method_name(*key),
                    from_signature,
                    to_signature
                ));
            }
        }
    }
    for (key, from_size) in &from_sizes {
        if let Some(to_size) = to_sizes.get(key) {
            if from_size != to_size {
                diff.changed_builtin_sizes
                    .push(format!("{}/{}: {} → {}", key.0, key.1, from_size, to_size));
            }
        }
    }

    diff
}

fn methods(api: &ExtensionApi) -> BTreeMap<(&str, &str), MethodSignature> {
    api.classes
        .iter()
        .flat_map(|class| {
            class.methods.iter().map(move |method| {
                (
                    (class.name.as_str(), method.name.as_str()),
                    MethodSignature {
                        hash: method.hash,
                        argument_count: method.arguments.len(),
                        is_virtual: method.is_virtual,
                        is_const: method.is_const,
                        is_static: method.is_static,
                        is_vararg: method.is_vararg,
                    },
                )
            })
        })
        .collect()
}

fn builtin_sizes(api: &ExtensionApi) -> BTreeMap<(&str, &str), u64> {
    api.builtin_class_sizes
        .iter()
        .flat_map(|configuration| {
            configuration.sizes.iter().map(move |size| {
                (
                    (
                        configuration.build_configuration.as_str(),
                        size.name.as_str(),
                    ),
                    size.size,
                )
            })
        })
        .collect()
}

fn version(api: &ExtensionApi) -> String {
    format!(
        "{}.{}.{}",
        api.header.version_major, api.header.version_minor, api.header.version_patch
    )
}

fn method_name(key: (&str, &str)) -> String {
    format!("{}::{}", key.0, key.1)
}

fn write_section(output: &mut String, heading: &str, entries: &[String]) {
    if entries.is_empty() {
        return;
    }
    writeln!(output, "## {heading}").unwrap();
    writeln!(output).unwrap();
    for entry in entries {
        writeln!(output, "- `{entry}`").unwrap();
    }
    writeln!(output).unwrap();
}
