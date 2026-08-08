use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt::{self, Write};

use serde::Serialize;

use crate::{
    ApiArgument, ApiClass, ApiEnumValue, ApiMethod, ApiReturnValue, BuiltinOperator, ExtensionApi,
};

/// Failure while turning an authenticated Godot API into high-level bindings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineApiGenerationError {
    message: String,
}

impl EngineApiGenerationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for EngineApiGenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for EngineApiGenerationError {}

/// One Godot value type that prevents a safe method binding from being generated.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UnsupportedEngineType {
    /// Type and optional ptrcall metadata exactly as declared by Godot.
    pub godot_type: String,
    /// Number of distinct methods blocked by this type.
    pub blocked_methods: usize,
}

/// How one exact entry from the official API is handled by the SDK.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiCoverageDisposition {
    /// A callable/type/constant wrapper is emitted into the public SDK.
    Generated,
    /// The entry is represented by another generated entry, such as a
    /// property whose official getter and setter methods are both emitted.
    CoveredByGeneratedEntry,
    /// Engine-native layout metadata is consumed and validated by the Host,
    /// but is intentionally not exposed as a project API.
    HostLayoutMetadata,
    /// A raw native pointer cannot safely cross the Script Mode boundary.
    UnsafeNativePointer,
    /// The official entry has no callable signature hash.
    MissingRequiredHash,
    /// A safe value type is not yet modeled by the SDK.
    UnsupportedSafeType,
}

/// Per-entry proof used by the full official-API coverage gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ApiCoverageEntry {
    pub category: String,
    pub identity: String,
    pub disposition: ApiCoverageDisposition,
    pub reason: String,
}

/// Exhaustive classification of Godot class methods seen by the generator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EngineApiGenerationReport {
    /// Every identity-bearing public or ABI-metadata entry in the official
    /// extension API.
    pub total_official_entries: usize,
    /// Official entries with a directly generated public SDK representation.
    pub generated_official_entries: usize,
    /// Official entries covered by an explicit, reviewed non-direct category.
    pub classified_official_entries: usize,
    /// Stable, duplicate-free proof for every official entry.
    pub coverage: Vec<ApiCoverageEntry>,
    /// Every method declared by Object-derived classes.
    pub total_methods: usize,
    /// Instance and class-level ptrcall methods emitted into the typed SDK.
    pub generated_methods: usize,
    /// Virtual callbacks, which require an override API instead of a MethodBind call.
    pub virtual_methods: usize,
    /// Virtual callbacks emitted as compile-time checked override traits.
    pub generated_virtual_methods: usize,
    /// Virtual callbacks blocked by value types the safe SDK has not modeled.
    pub unsupported_virtual_methods: usize,
    /// Unsupported virtual callback value types sorted by blocked method count.
    pub unsupported_virtual_types: Vec<UnsupportedEngineType>,
    /// Virtual callbacks deliberately omitted because they expose raw engine
    /// pointers that cannot cross the project-module boundary.
    pub unsafe_pointer_virtual_methods: usize,
    /// Raw virtual pointer types sorted by blocked method count.
    pub unsafe_pointer_virtual_types: Vec<UnsupportedEngineType>,
    /// Class-level methods emitted as receiver-free associated functions.
    pub static_methods: usize,
    /// Variable-argument methods emitted with bounded trailing Variant slices.
    pub vararg_methods: usize,
    /// Non-virtual methods lacking the Method Hash required by ptrcall.
    pub methods_without_hash: usize,
    /// Otherwise callable methods blocked by one or more unsupported value types.
    pub methods_with_unsupported_types: usize,
    /// Unsupported value types sorted by blocked method count and Godot spelling.
    pub unsupported_types: Vec<UnsupportedEngineType>,
    /// Non-virtual methods deliberately omitted because they expose raw engine
    /// pointers that cannot cross the project-module boundary.
    pub unsafe_pointer_methods: usize,
    /// Raw method pointer types sorted by blocked method count.
    pub unsafe_pointer_types: Vec<UnsupportedEngineType>,
}

#[derive(Default)]
struct GenerationReportBuilder {
    total_methods: usize,
    generated_methods: usize,
    virtual_methods: usize,
    static_methods: usize,
    vararg_methods: usize,
    methods_without_hash: usize,
    methods_with_unsupported_types: usize,
    unsupported_types: BTreeMap<String, usize>,
    unsafe_pointer_methods: usize,
    unsafe_pointer_types: BTreeMap<String, usize>,
}

impl GenerationReportBuilder {
    fn record_skip(&mut self, reason: MethodSkipReason) {
        match reason {
            MethodSkipReason::Virtual => self.virtual_methods += 1,
            MethodSkipReason::MissingHash => self.methods_without_hash += 1,
            MethodSkipReason::UnsupportedTypes(types) => {
                self.methods_with_unsupported_types += 1;
                for godot_type in types {
                    *self.unsupported_types.entry(godot_type).or_default() += 1;
                }
            }
            MethodSkipReason::UnsafePointerTypes(types) => {
                self.unsafe_pointer_methods += 1;
                for godot_type in types {
                    *self.unsafe_pointer_types.entry(godot_type).or_default() += 1;
                }
            }
        }
    }

    fn finish(self) -> EngineApiGenerationReport {
        EngineApiGenerationReport {
            total_official_entries: 0,
            generated_official_entries: 0,
            classified_official_entries: 0,
            coverage: Vec::new(),
            total_methods: self.total_methods,
            generated_methods: self.generated_methods,
            virtual_methods: self.virtual_methods,
            generated_virtual_methods: 0,
            unsupported_virtual_methods: 0,
            unsupported_virtual_types: Vec::new(),
            unsafe_pointer_virtual_methods: 0,
            unsafe_pointer_virtual_types: Vec::new(),
            static_methods: self.static_methods,
            vararg_methods: self.vararg_methods,
            methods_without_hash: self.methods_without_hash,
            methods_with_unsupported_types: self.methods_with_unsupported_types,
            unsupported_types: sorted_type_counts(self.unsupported_types),
            unsafe_pointer_methods: self.unsafe_pointer_methods,
            unsafe_pointer_types: sorted_type_counts(self.unsafe_pointer_types),
        }
    }
}

fn sorted_type_counts(types: BTreeMap<String, usize>) -> Vec<UnsupportedEngineType> {
    let mut types = types
        .into_iter()
        .map(|(godot_type, blocked_methods)| UnsupportedEngineType {
            godot_type,
            blocked_methods,
        })
        .collect::<Vec<_>>();
    types.sort_by(|left, right| {
        right
            .blocked_methods
            .cmp(&left.blocked_methods)
            .then_with(|| left.godot_type.cmp(&right.godot_type))
    });
    types
}

#[derive(Clone)]
struct ValueBinding {
    rust_type: String,
    value_type: &'static str,
    ptrcall_type: &'static str,
    class_name: Option<String>,
    typed_array_element: Option<String>,
}

struct EnumBindings<'api> {
    definitions: BTreeMap<String, Vec<EnumDefinition<'api>>>,
    values: HashMap<String, ValueBinding>,
}

struct EnumDefinition<'api> {
    godot_name: String,
    rust_name: String,
    is_bitfield: bool,
    values: &'api [ApiEnumValue],
}

struct EnumSource<'source, 'api> {
    module: &'source str,
    owner: Option<&'source str>,
    name: &'source str,
    is_bitfield: bool,
    values: &'api [ApiEnumValue],
}

struct MethodBinding<'api> {
    class: &'api ApiClass,
    method: &'api ApiMethod,
    id: u64,
    arguments: Vec<ArgumentBinding>,
    return_value: ValueBinding,
}

struct VirtualMethodBinding<'api> {
    class: &'api ApiClass,
    method: &'api ApiMethod,
    arguments: Vec<ArgumentBinding>,
    return_value: ValueBinding,
}

struct VirtualBindings<'api> {
    methods: Vec<VirtualMethodBinding<'api>>,
    generated_methods: usize,
    unsupported_methods: usize,
    unsupported_types: Vec<UnsupportedEngineType>,
    unsafe_pointer_methods: usize,
    unsafe_pointer_types: Vec<UnsupportedEngineType>,
}

#[derive(Clone)]
struct ArgumentBinding {
    rust_name: String,
    value: ValueBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeneratedApiKind {
    Utility,
    BuiltinConstructor,
    BuiltinMethod,
    BuiltinOperator,
    BuiltinMemberGetter,
    BuiltinMemberSetter,
    BuiltinIndexedGetter,
    BuiltinIndexedSetter,
    BuiltinKeyedGetter,
    BuiltinKeyedSetter,
    BuiltinConstant,
    Singleton,
    ObjectConstructor,
}

struct GeneratedApiBinding {
    kind: GeneratedApiKind,
    id: u64,
    owner_name: Option<String>,
    member_name: Option<String>,
    numeric: u64,
    is_static: bool,
    is_const: bool,
    is_vararg: bool,
    mutates_base: bool,
    base_value: ValueBinding,
    arguments: Vec<ArgumentBinding>,
    return_value: ValueBinding,
    identity: String,
}

struct PublicApiBindings {
    entries: Vec<GeneratedApiBinding>,
    coverage: Vec<ApiCoverageEntry>,
}

enum MethodBindingOutcome<'api> {
    Bound(MethodBinding<'api>),
    Skipped(MethodSkipReason),
}

enum MethodSkipReason {
    Virtual,
    MissingHash,
    UnsupportedTypes(BTreeSet<String>),
    UnsafePointerTypes(BTreeSet<String>),
}

fn void_binding() -> ValueBinding {
    ValueBinding {
        rust_type: "()".into(),
        value_type: "NIL",
        ptrcall_type: "VOID",
        class_name: None,
        typed_array_element: None,
    }
}

fn bind_named_return(
    type_name: Option<&str>,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Option<ValueBinding> {
    let Some(type_name) = type_name else {
        return Some(void_binding());
    };
    let mut binding = bind_value(type_name, None, classes, enums)?;
    if binding.class_name.is_some() {
        let class = classes.get(type_name)?;
        if class.is_refcounted {
            binding.rust_type = format!("super::GodotRef<super::{type_name}>");
            binding.ptrcall_type = "REFCOUNTED_OBJECT";
        }
        binding.rust_type = format!("Option<{}>", binding.rust_type);
    }
    Some(binding)
}

fn bind_api_arguments(
    arguments: &[ApiArgument],
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Result<Vec<ArgumentBinding>, EngineApiGenerationError> {
    let mut result = Vec::with_capacity(arguments.len());
    let mut names = HashSet::from([
        "self".to_owned(),
        "base".to_owned(),
        "varargs".to_owned(),
        "arguments".to_owned(),
    ]);
    for (index, argument) in arguments.iter().enumerate() {
        let value = bind_argument(argument, classes, enums).ok_or_else(|| {
            EngineApiGenerationError::new(format!(
                "safe Godot API argument type `{}` is not modeled",
                godot_type_name(&argument.r#type, argument.meta.as_deref())
            ))
        })?;
        let base = if argument.name.is_empty() {
            format!("argument_{index}")
        } else {
            rust_argument_identifier(&argument.name)
        };
        let mut rust_name = base.clone();
        let mut suffix = 2;
        while !names.insert(rust_name.clone()) {
            rust_name = format!("{base}_{suffix}");
            suffix += 1;
        }
        result.push(ArgumentBinding { rust_name, value });
    }
    Ok(result)
}

fn collect_public_api_bindings(
    api: &ExtensionApi,
    enums: &HashMap<String, ValueBinding>,
) -> Result<PublicApiBindings, EngineApiGenerationError> {
    let classes = api
        .classes
        .iter()
        .map(|class| (class.name.as_str(), class))
        .collect::<HashMap<_, _>>();
    let mut entries = Vec::new();
    let mut coverage = Vec::new();
    let mut ids = HashMap::<u64, String>::new();

    let mut push = |entry: GeneratedApiBinding,
                    _category: &str,
                    _reason: &str|
     -> Result<(), EngineApiGenerationError> {
        if let Some(previous) = ids.insert(entry.id, entry.identity.clone()) {
            return Err(EngineApiGenerationError::new(format!(
                "generated API ID collision: `{previous}` and `{}`",
                entry.identity
            )));
        }
        entries.push(entry);
        Ok(())
    };

    let mut utilities = api.utility_functions.iter().collect::<Vec<_>>();
    utilities.sort_by(|left, right| left.name.cmp(&right.name));
    for utility in utilities {
        validate_identifier(&utility.name, "Godot utility function")?;
        let arguments = bind_api_arguments(&utility.arguments, &classes, enums)?;
        let return_value = bind_named_return(utility.return_type.as_deref(), &classes, enums)
            .ok_or_else(|| {
                EngineApiGenerationError::new(format!(
                    "safe Godot utility return type `{}` is not modeled",
                    utility.return_type.as_deref().unwrap_or("void")
                ))
            })?;
        let identity = format!("utility.{}", utility.name);
        let mut entry = GeneratedApiBinding {
            kind: GeneratedApiKind::Utility,
            id: 0,
            owner_name: None,
            member_name: Some(utility.name.clone()),
            numeric: utility.hash,
            is_static: true,
            is_const: true,
            is_vararg: utility.is_vararg,
            mutates_base: false,
            base_value: void_binding(),
            arguments,
            return_value,
            identity,
        };
        entry.id = generated_api_id(&entry);
        push(entry, "utility_function", "typed utility wrapper")?;
    }

    let mut builtins = api.builtin_classes.iter().collect::<Vec<_>>();
    builtins.sort_by(|left, right| left.name.cmp(&right.name));
    for builtin in builtins {
        let base_value = bind_value(&builtin.name, None, &classes, enums).ok_or_else(|| {
            EngineApiGenerationError::new(format!(
                "safe Godot builtin `{}` is not modeled",
                builtin.name
            ))
        })?;
        for constructor in &builtin.constructors {
            let arguments = bind_api_arguments(&constructor.arguments, &classes, enums)?;
            let identity = format!("builtin.{}.constructor.{}", builtin.name, constructor.index);
            let mut entry = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinConstructor,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: None,
                numeric: u64::from(constructor.index),
                is_static: true,
                is_const: true,
                is_vararg: false,
                mutates_base: false,
                base_value: void_binding(),
                arguments,
                return_value: base_value.clone(),
                identity,
            };
            entry.id = generated_api_id(&entry);
            push(entry, "builtin_constructor", "typed builtin constructor")?;
        }
        for (operator_index, operator) in builtin.operators.iter().enumerate() {
            let mut arguments = Vec::new();
            if let Some(right_type) = operator.right_type.as_deref() {
                let argument = ApiArgument {
                    name: "right".to_owned(),
                    r#type: right_type.to_owned(),
                    meta: None,
                    default_value: None,
                };
                arguments = bind_api_arguments(&[argument], &classes, enums)?;
            }
            let return_value = bind_named_return(Some(&operator.return_type), &classes, enums)
                .ok_or_else(|| {
                    EngineApiGenerationError::new(format!(
                        "safe Godot builtin operator return type `{}` is not modeled",
                        operator.return_type
                    ))
                })?;
            let ordinal = builtin_operator_ordinal(operator)?;
            let identity = format!(
                "builtin.{}.operator.{}.{}",
                builtin.name, operator_index, operator.name
            );
            let mut entry = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinOperator,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: Some(operator.name.clone()),
                numeric: u64::from(ordinal),
                is_static: false,
                is_const: true,
                is_vararg: false,
                mutates_base: false,
                base_value: base_value.clone(),
                arguments,
                return_value,
                identity,
            };
            entry.id = generated_api_id(&entry);
            push(entry, "builtin_operator", "typed builtin operator wrapper")?;
        }
        let mut methods = builtin.methods.iter().collect::<Vec<_>>();
        methods.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.hash.cmp(&right.hash))
        });
        for method in methods {
            validate_identifier(&method.name, "Godot builtin method")?;
            let arguments = bind_api_arguments(&method.arguments, &classes, enums)?;
            let return_value = bind_named_return(method.return_type.as_deref(), &classes, enums)
                .ok_or_else(|| {
                    EngineApiGenerationError::new(format!(
                        "safe Godot builtin return type `{}` is not modeled",
                        method.return_type.as_deref().unwrap_or("void")
                    ))
                })?;
            let identity = format!(
                "builtin.{}.method.{}.{}",
                builtin.name, method.name, method.hash
            );
            let mut entry = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinMethod,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: Some(method.name.clone()),
                numeric: method.hash,
                is_static: method.is_static,
                is_const: method.is_const,
                is_vararg: method.is_vararg,
                mutates_base: !method.is_static && !method.is_const,
                base_value: if method.is_static {
                    void_binding()
                } else {
                    base_value.clone()
                },
                arguments,
                return_value,
                identity,
            };
            entry.id = generated_api_id(&entry);
            push(entry, "builtin_method", "typed builtin method wrapper")?;
        }
        for member in &builtin.members {
            let return_value = bind_named_return(Some(&member.r#type), &classes, enums)
                .ok_or_else(|| {
                    EngineApiGenerationError::new(format!(
                        "safe Godot builtin member type `{}` is not modeled",
                        member.r#type
                    ))
                })?;
            let identity = format!("builtin.{}.member.{}", builtin.name, member.name);
            let mut getter = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinMemberGetter,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: Some(member.name.clone()),
                numeric: 0,
                is_static: false,
                is_const: true,
                is_vararg: false,
                mutates_base: false,
                base_value: base_value.clone(),
                arguments: Vec::new(),
                return_value: return_value.clone(),
                identity: format!("{identity}.get"),
            };
            getter.id = generated_api_id(&getter);
            push(
                getter,
                "builtin_member_accessor",
                "typed builtin member getter",
            )?;
            let setter_argument = ArgumentBinding {
                rust_name: "value".to_owned(),
                value: argument_binding_from_value(return_value),
            };
            let mut setter = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinMemberSetter,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: Some(member.name.clone()),
                numeric: 0,
                is_static: false,
                is_const: false,
                is_vararg: false,
                mutates_base: true,
                base_value: base_value.clone(),
                arguments: vec![setter_argument],
                return_value: void_binding(),
                identity: format!("{identity}.set"),
            };
            setter.id = generated_api_id(&setter);
            push(
                setter,
                "builtin_member_accessor",
                "typed builtin member setter",
            )?;
            coverage.push(ApiCoverageEntry {
                category: "builtin_member".to_owned(),
                identity,
                disposition: ApiCoverageDisposition::CoveredByGeneratedEntry,
                reason: "generated getter and setter contracts".to_owned(),
            });
        }
        if let Some(index_type) = builtin.indexing_return_type.as_deref() {
            let return_value =
                bind_named_return(Some(index_type), &classes, enums).ok_or_else(|| {
                    EngineApiGenerationError::new(format!(
                        "safe Godot builtin index type `{index_type}` is not modeled"
                    ))
                })?;
            let identity = format!("builtin.{}.indexing", builtin.name);
            let index = ArgumentBinding {
                rust_name: "index".to_owned(),
                value: bind_value("int", None, &classes, enums).expect("int binding"),
            };
            let mut getter = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinIndexedGetter,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: None,
                numeric: 0,
                is_static: false,
                is_const: true,
                is_vararg: false,
                mutates_base: false,
                base_value: base_value.clone(),
                arguments: vec![index.clone()],
                return_value: return_value.clone(),
                identity: format!("{identity}.get"),
            };
            getter.id = generated_api_id(&getter);
            push(
                getter,
                "builtin_index_accessor",
                "typed builtin indexed getter",
            )?;
            let mut setter = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinIndexedSetter,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: None,
                numeric: 0,
                is_static: false,
                is_const: false,
                is_vararg: false,
                mutates_base: true,
                base_value: base_value.clone(),
                arguments: vec![
                    index,
                    ArgumentBinding {
                        rust_name: "value".to_owned(),
                        value: argument_binding_from_value(return_value),
                    },
                ],
                return_value: void_binding(),
                identity: format!("{identity}.set"),
            };
            setter.id = generated_api_id(&setter);
            push(
                setter,
                "builtin_index_accessor",
                "typed builtin indexed setter",
            )?;
            coverage.push(ApiCoverageEntry {
                category: "builtin_indexing".to_owned(),
                identity,
                disposition: ApiCoverageDisposition::CoveredByGeneratedEntry,
                reason: "generated indexed getter and setter contracts".to_owned(),
            });
        }
        if builtin.is_keyed {
            let variant = bind_value("Variant", None, &classes, enums).expect("Variant binding");
            let identity = format!("builtin.{}.keyed", builtin.name);
            let key = ArgumentBinding {
                rust_name: "key".to_owned(),
                value: argument_binding_from_value(variant.clone()),
            };
            let mut getter = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinKeyedGetter,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: None,
                numeric: 0,
                is_static: false,
                is_const: true,
                is_vararg: false,
                mutates_base: false,
                base_value: base_value.clone(),
                arguments: vec![key.clone()],
                return_value: variant.clone(),
                identity: format!("{identity}.get"),
            };
            getter.id = generated_api_id(&getter);
            push(getter, "builtin_key_accessor", "typed builtin keyed getter")?;
            let mut setter = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinKeyedSetter,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: None,
                numeric: 0,
                is_static: false,
                is_const: false,
                is_vararg: false,
                mutates_base: true,
                base_value: base_value.clone(),
                arguments: vec![
                    key,
                    ArgumentBinding {
                        rust_name: "value".to_owned(),
                        value: argument_binding_from_value(variant),
                    },
                ],
                return_value: void_binding(),
                identity: format!("{identity}.set"),
            };
            setter.id = generated_api_id(&setter);
            push(setter, "builtin_key_accessor", "typed builtin keyed setter")?;
            coverage.push(ApiCoverageEntry {
                category: "builtin_keyed".to_owned(),
                identity,
                disposition: ApiCoverageDisposition::CoveredByGeneratedEntry,
                reason: "generated keyed getter and setter contracts".to_owned(),
            });
        }
        for constant in &builtin.constants {
            let return_value = bind_named_return(Some(&constant.r#type), &classes, enums)
                .ok_or_else(|| {
                    EngineApiGenerationError::new(format!(
                        "safe Godot builtin constant type `{}` is not modeled",
                        constant.r#type
                    ))
                })?;
            let identity = format!("builtin.{}.constant.{}", builtin.name, constant.name);
            let mut entry = GeneratedApiBinding {
                kind: GeneratedApiKind::BuiltinConstant,
                id: 0,
                owner_name: Some(builtin.name.clone()),
                member_name: Some(constant.name.clone()),
                numeric: 0,
                is_static: true,
                is_const: true,
                is_vararg: false,
                mutates_base: false,
                base_value: void_binding(),
                arguments: Vec::new(),
                return_value,
                identity,
            };
            entry.id = generated_api_id(&entry);
            push(entry, "builtin_constant", "typed builtin constant accessor")?;
        }
    }

    for singleton in &api.singletons {
        let class = classes.get(singleton.r#type.as_str()).ok_or_else(|| {
            EngineApiGenerationError::new(format!(
                "Godot singleton `{}` refers to missing class `{}`",
                singleton.name, singleton.r#type
            ))
        })?;
        let return_value = ValueBinding {
            rust_type: format!("super::ObjectRef<super::{}>", class.name),
            value_type: "OBJECT_ID",
            ptrcall_type: "OBJECT",
            class_name: Some(class.name.clone()),
            typed_array_element: None,
        };
        let identity = format!("singleton.{}", singleton.name);
        let mut entry = GeneratedApiBinding {
            kind: GeneratedApiKind::Singleton,
            id: 0,
            owner_name: Some(singleton.r#type.clone()),
            member_name: Some(singleton.name.clone()),
            numeric: 0,
            is_static: true,
            is_const: true,
            is_vararg: false,
            mutates_base: false,
            base_value: void_binding(),
            arguments: Vec::new(),
            return_value,
            identity,
        };
        entry.id = generated_api_id(&entry);
        push(entry, "singleton", "typed singleton accessor")?;
    }

    for class in &api.classes {
        if !class.is_instantiable {
            continue;
        }
        let return_value = ValueBinding {
            rust_type: if class.is_refcounted {
                format!("super::GodotRef<super::{}>", class.name)
            } else {
                format!("super::ObjectRef<super::{}>", class.name)
            },
            value_type: "OBJECT_ID",
            ptrcall_type: if class.is_refcounted {
                "REFCOUNTED_OBJECT"
            } else {
                "OBJECT"
            },
            class_name: Some(class.name.clone()),
            typed_array_element: None,
        };
        let identity = format!("class.{}.constructor", class.name);
        let mut entry = GeneratedApiBinding {
            kind: GeneratedApiKind::ObjectConstructor,
            id: 0,
            owner_name: Some(class.name.clone()),
            member_name: None,
            numeric: 0,
            is_static: true,
            is_const: false,
            is_vararg: false,
            mutates_base: false,
            base_value: void_binding(),
            arguments: Vec::new(),
            return_value,
            identity,
        };
        entry.id = generated_api_id(&entry);
        push(
            entry,
            "object_constructor",
            "typed ClassDB object constructor",
        )?;
    }
    coverage.extend(entries.iter().map(|entry| ApiCoverageEntry {
        category: "generated_contract".to_owned(),
        identity: entry.identity.clone(),
        disposition: ApiCoverageDisposition::Generated,
        reason: "generated Host ABI contract".to_owned(),
    }));
    Ok(PublicApiBindings { entries, coverage })
}

fn argument_binding_from_value(mut value: ValueBinding) -> ValueBinding {
    if matches!(value.value_type, "STRING" | "STRING_NAME" | "NODE_PATH") {
        value.rust_type = "&str".to_owned();
    } else if matches!(
        value.value_type,
        "TRANSFORM2D"
            | "AABB"
            | "BASIS"
            | "TRANSFORM3D"
            | "PROJECTION"
            | "PACKED_BYTE_ARRAY"
            | "PACKED_INT32_ARRAY"
            | "PACKED_INT64_ARRAY"
            | "PACKED_FLOAT32_ARRAY"
            | "PACKED_FLOAT64_ARRAY"
            | "PACKED_STRING_ARRAY"
            | "PACKED_VECTOR2_ARRAY"
            | "PACKED_VECTOR3_ARRAY"
            | "PACKED_COLOR_ARRAY"
            | "PACKED_VECTOR4_ARRAY"
            | "VARIANT"
            | "ARRAY"
            | "DICTIONARY"
            | "CALLABLE"
            | "SIGNAL"
    ) {
        value.rust_type = format!("&{}", value.rust_type);
    }
    value
}

fn builtin_operator_ordinal(operator: &BuiltinOperator) -> Result<u32, EngineApiGenerationError> {
    let unary = operator.right_type.is_none();
    match (operator.name.as_str(), unary) {
        ("==", _) => Ok(0),
        ("!=", _) => Ok(1),
        ("<", _) => Ok(2),
        ("<=", _) => Ok(3),
        (">", _) => Ok(4),
        (">=", _) => Ok(5),
        ("+", false) => Ok(6),
        ("-", false) => Ok(7),
        ("*", _) => Ok(8),
        ("/", _) => Ok(9),
        ("-" | "unary-", true) => Ok(10),
        ("+" | "unary+", true) => Ok(11),
        ("%", _) => Ok(12),
        ("**", _) => Ok(13),
        ("<<", _) => Ok(14),
        (">>", _) => Ok(15),
        ("&", _) => Ok(16),
        ("|", _) => Ok(17),
        ("^", _) => Ok(18),
        ("~", _) => Ok(19),
        ("and", _) => Ok(20),
        ("or", _) => Ok(21),
        ("xor", _) => Ok(22),
        ("not", _) => Ok(23),
        ("in", _) => Ok(24),
        _ => Err(EngineApiGenerationError::new(format!(
            "unknown Godot builtin operator `{}`",
            operator.name
        ))),
    }
}

fn generated_api_id(entry: &GeneratedApiBinding) -> u64 {
    let mut digest = Sha256::new();
    digest.update(b"godot-rust-generated-api-v1\0");
    digest.update((entry.kind as u8).to_le_bytes());
    hash_text(&mut digest, entry.owner_name.as_deref().unwrap_or(""));
    hash_text(&mut digest, entry.member_name.as_deref().unwrap_or(""));
    digest.update(entry.numeric.to_le_bytes());
    digest.update([
        u8::from(entry.is_static),
        u8::from(entry.is_const),
        u8::from(entry.is_vararg),
        u8::from(entry.mutates_base),
    ]);
    hash_value(&mut digest, &entry.base_value);
    for argument in &entry.arguments {
        hash_value(&mut digest, &argument.value);
    }
    hash_value(&mut digest, &entry.return_value);
    let id = u64::from_le_bytes(
        digest.finalize()[..8]
            .try_into()
            .expect("SHA-256 has at least eight bytes"),
    );
    if id == 0 { 1 } else { id }
}

fn populate_full_coverage(
    api: &ExtensionApi,
    methods: &[MethodBinding<'_>],
    virtual_methods: &[VirtualMethodBinding<'_>],
    public_api: &PublicApiBindings,
    report: &mut EngineApiGenerationReport,
) -> Result<(), EngineApiGenerationError> {
    let direct = public_api
        .coverage
        .iter()
        .map(|entry| entry.identity.as_str())
        .collect::<HashSet<_>>();
    let generated_methods = methods
        .iter()
        .map(|binding| (binding.class.name.as_str(), binding.method.name.as_str()))
        .collect::<HashSet<_>>();
    let generated_virtual_methods = virtual_methods
        .iter()
        .map(|binding| (binding.class.name.as_str(), binding.method.name.as_str()))
        .collect::<HashSet<_>>();
    let classes = api
        .classes
        .iter()
        .map(|class| (class.name.as_str(), class))
        .collect::<HashMap<_, _>>();
    let enums = collect_enum_bindings(api)?;
    let mut coverage = Vec::new();
    let mut identities = HashSet::new();
    let mut add = |category: &str,
                   identity: String,
                   disposition: ApiCoverageDisposition,
                   reason: &str|
     -> Result<(), EngineApiGenerationError> {
        let key = format!("{category}\0{identity}");
        if !identities.insert(key) {
            return Err(EngineApiGenerationError::new(format!(
                "duplicate official API coverage identity `{category}:{identity}`"
            )));
        }
        coverage.push(ApiCoverageEntry {
            category: category.to_owned(),
            identity,
            disposition,
            reason: reason.to_owned(),
        });
        Ok(())
    };

    for configuration in &api.builtin_class_sizes {
        add(
            "builtin_size_configuration",
            configuration.build_configuration.clone(),
            ApiCoverageDisposition::HostLayoutMetadata,
            "validated native builtin size table",
        )?;
        for size in &configuration.sizes {
            add(
                "builtin_size",
                format!("{}.{}", configuration.build_configuration, size.name),
                ApiCoverageDisposition::HostLayoutMetadata,
                "validated native builtin size",
            )?;
        }
    }
    for configuration in &api.builtin_class_member_offsets {
        add(
            "builtin_offset_configuration",
            configuration.build_configuration.clone(),
            ApiCoverageDisposition::HostLayoutMetadata,
            "validated native builtin member-offset table",
        )?;
        for class in &configuration.classes {
            for member in &class.members {
                add(
                    "builtin_member_offset",
                    format!(
                        "{}.{}.{}",
                        configuration.build_configuration, class.name, member.member
                    ),
                    ApiCoverageDisposition::HostLayoutMetadata,
                    "validated native builtin member offset",
                )?;
            }
        }
    }
    for constant in &api.global_constants {
        add(
            "global_constant",
            constant.name.clone(),
            ApiCoverageDisposition::Generated,
            "generated global integer constant",
        )?;
    }
    for enum_ in &api.global_enums {
        add(
            "global_enum",
            enum_.name.clone(),
            ApiCoverageDisposition::Generated,
            "generated unknown-value-safe enum or bitfield",
        )?;
        for value in &enum_.values {
            add(
                "global_enum_value",
                format!("{}.{}", enum_.name, value.name),
                ApiCoverageDisposition::Generated,
                "generated enum value",
            )?;
        }
    }
    for utility in &api.utility_functions {
        let identity = format!("utility.{}", utility.name);
        if !direct.contains(identity.as_str()) {
            return Err(EngineApiGenerationError::new(format!(
                "official utility `{}` has no generated contract",
                utility.name
            )));
        }
        add(
            "utility_function",
            identity,
            ApiCoverageDisposition::Generated,
            "generated typed utility wrapper",
        )?;
    }
    for builtin in &api.builtin_classes {
        add(
            "builtin_class",
            builtin.name.clone(),
            ApiCoverageDisposition::Generated,
            "generated builtin module and receiver binding",
        )?;
        if builtin.has_destructor {
            add(
                "builtin_destructor",
                builtin.name.clone(),
                ApiCoverageDisposition::HostLayoutMetadata,
                "Host owns and destroys native temporary storage",
            )?;
        }
        for constructor in &builtin.constructors {
            let identity = format!("builtin.{}.constructor.{}", builtin.name, constructor.index);
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_constructor",
                identity,
                ApiCoverageDisposition::Generated,
                "generated typed builtin constructor",
            )?;
        }
        for (index, operator) in builtin.operators.iter().enumerate() {
            let identity = format!(
                "builtin.{}.operator.{}.{}",
                builtin.name, index, operator.name
            );
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_operator",
                identity,
                ApiCoverageDisposition::Generated,
                "generated typed builtin operator",
            )?;
        }
        for method in &builtin.methods {
            let identity = format!(
                "builtin.{}.method.{}.{}",
                builtin.name, method.name, method.hash
            );
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_method",
                identity,
                ApiCoverageDisposition::Generated,
                "generated typed builtin method",
            )?;
        }
        for member in &builtin.members {
            let identity = format!("builtin.{}.member.{}", builtin.name, member.name);
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_member",
                identity,
                ApiCoverageDisposition::CoveredByGeneratedEntry,
                "generated typed getter and setter",
            )?;
        }
        if builtin.indexing_return_type.is_some() {
            let identity = format!("builtin.{}.indexing", builtin.name);
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_indexing",
                identity,
                ApiCoverageDisposition::CoveredByGeneratedEntry,
                "generated typed indexed getter and setter",
            )?;
        }
        if builtin.is_keyed {
            let identity = format!("builtin.{}.keyed", builtin.name);
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_keyed",
                identity,
                ApiCoverageDisposition::CoveredByGeneratedEntry,
                "generated typed keyed getter and setter",
            )?;
        }
        for constant in &builtin.constants {
            let identity = format!("builtin.{}.constant.{}", builtin.name, constant.name);
            ensure_generated_identity(&direct, &identity)?;
            add(
                "builtin_constant",
                identity,
                ApiCoverageDisposition::Generated,
                "generated runtime constant accessor",
            )?;
        }
        for enum_ in &builtin.enums {
            add(
                "builtin_enum",
                format!("{}.{}", builtin.name, enum_.name),
                ApiCoverageDisposition::Generated,
                "generated unknown-value-safe builtin enum",
            )?;
            for value in &enum_.values {
                add(
                    "builtin_enum_value",
                    format!("{}.{}.{}", builtin.name, enum_.name, value.name),
                    ApiCoverageDisposition::Generated,
                    "generated builtin enum value",
                )?;
            }
        }
    }
    for class in &api.classes {
        add(
            "engine_class",
            class.name.clone(),
            ApiCoverageDisposition::Generated,
            "generated class marker and inheritance",
        )?;
        if class.is_instantiable {
            let identity = format!("class.{}.constructor", class.name);
            ensure_generated_identity(&direct, &identity)?;
            add(
                "object_constructor",
                identity,
                ApiCoverageDisposition::Generated,
                "generated ClassDB constructor",
            )?;
        }
        for method in &class.methods {
            let identity = format!("{}.{}", class.name, method.name);
            let types = method_types(method);
            let (disposition, reason) = if types.iter().any(|type_| type_.contains('*')) {
                (
                    ApiCoverageDisposition::UnsafeNativePointer,
                    "raw native pointer cannot cross the Script Mode ABI",
                )
            } else if method.is_virtual && is_script_lifecycle(class, &method.name) {
                (
                    ApiCoverageDisposition::Generated,
                    "generated specialized script lifecycle contract",
                )
            } else if method.is_virtual
                && generated_virtual_methods.contains(&(class.name.as_str(), method.name.as_str()))
            {
                (
                    ApiCoverageDisposition::Generated,
                    "generated virtual override contract",
                )
            } else if !method.is_virtual
                && generated_methods.contains(&(class.name.as_str(), method.name.as_str()))
            {
                (
                    ApiCoverageDisposition::Generated,
                    "generated typed MethodBind wrapper",
                )
            } else if method.hash.is_none() {
                (
                    ApiCoverageDisposition::MissingRequiredHash,
                    "official non-virtual method has no MethodBind hash",
                )
            } else {
                (
                    ApiCoverageDisposition::UnsupportedSafeType,
                    "one or more safe value types are not modeled",
                )
            };
            add("engine_method", identity, disposition, reason)?;
        }
        for property in &class.properties {
            if let Some(getter) = class
                .methods
                .iter()
                .find(|method| method.name == property.getter)
            {
                if !getter.is_virtual
                    && !generated_methods.contains(&(class.name.as_str(), getter.name.as_str()))
                    && !method_types(getter).iter().any(|type_| type_.contains('*'))
                {
                    return Err(EngineApiGenerationError::new(format!(
                        "property `{}.{}` getter is not generated",
                        class.name, property.name
                    )));
                }
            }
            if let Some(setter) = property.setter.as_deref() {
                if let Some(setter) = class.methods.iter().find(|method| method.name == setter) {
                    if !setter.is_virtual
                        && !generated_methods.contains(&(class.name.as_str(), setter.name.as_str()))
                        && !method_types(setter).iter().any(|type_| type_.contains('*'))
                    {
                        return Err(EngineApiGenerationError::new(format!(
                            "property `{}.{}` setter is not generated",
                            class.name, property.name
                        )));
                    }
                }
            }
            add(
                "engine_property",
                format!("{}.{}", class.name, property.name),
                ApiCoverageDisposition::CoveredByGeneratedEntry,
                "generated accessor methods or generated Object property access cover this entry",
            )?;
        }
        for signal in &class.signals {
            for argument in &signal.arguments {
                if bind_argument(argument, &classes, &enums.values).is_none() {
                    return Err(EngineApiGenerationError::new(format!(
                        "signal `{}.{}` uses unsupported safe type `{}`",
                        class.name, signal.name, argument.r#type
                    )));
                }
            }
            add(
                "engine_signal",
                format!("{}.{}", class.name, signal.name),
                ApiCoverageDisposition::Generated,
                "generated typed Signal handle",
            )?;
        }
        for enum_ in &class.enums {
            add(
                "engine_enum",
                format!("{}.{}", class.name, enum_.name),
                ApiCoverageDisposition::Generated,
                "generated unknown-value-safe class enum",
            )?;
            for value in &enum_.values {
                add(
                    "engine_enum_value",
                    format!("{}.{}.{}", class.name, enum_.name, value.name),
                    ApiCoverageDisposition::Generated,
                    "generated class enum value",
                )?;
            }
        }
        for constant in &class.constants {
            add(
                "engine_constant",
                format!("{}.{}", class.name, constant.name),
                ApiCoverageDisposition::Generated,
                "generated associated integer constant",
            )?;
        }
    }
    for singleton in &api.singletons {
        let identity = format!("singleton.{}", singleton.name);
        ensure_generated_identity(&direct, &identity)?;
        add(
            "singleton",
            identity,
            ApiCoverageDisposition::Generated,
            "generated typed singleton accessor",
        )?;
    }
    for structure in &api.native_structures {
        add(
            "native_structure",
            structure.name.clone(),
            ApiCoverageDisposition::UnsafeNativePointer,
            "engine-native structure is restricted to Host internals",
        )?;
    }

    coverage.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    report.generated_official_entries = coverage
        .iter()
        .filter(|entry| entry.disposition == ApiCoverageDisposition::Generated)
        .count();
    report.classified_official_entries = coverage.len() - report.generated_official_entries;
    report.total_official_entries = coverage.len();
    report.coverage = coverage;
    Ok(())
}

fn ensure_generated_identity(
    identities: &HashSet<&str>,
    identity: &str,
) -> Result<(), EngineApiGenerationError> {
    if identities.contains(identity) {
        Ok(())
    } else {
        Err(EngineApiGenerationError::new(format!(
            "official API entry `{identity}` has no generated contract"
        )))
    }
}

fn method_types(method: &ApiMethod) -> BTreeSet<String> {
    let mut types = method
        .arguments
        .iter()
        .map(|argument| godot_type_name(&argument.r#type, argument.meta.as_deref()))
        .collect::<BTreeSet<_>>();
    if let Some(return_value) = &method.return_value {
        types.insert(godot_type_name(
            &return_value.r#type,
            return_value.meta.as_deref(),
        ));
    }
    types
}

/// Emits type-safe Object MethodBind wrappers for the official Godot API.
pub fn generate_engine_api(
    api: &ExtensionApi,
    source_sha256: &str,
) -> Result<String, EngineApiGenerationError> {
    let enums = collect_enum_bindings(api)?;
    let (sorted_classes, methods, mut report) = collect_method_bindings(api, &enums.values)?;
    let virtuals = collect_virtual_bindings(api, &enums.values)?;
    let public_api = collect_public_api_bindings(api, &enums.values)?;
    report.generated_virtual_methods = virtuals.generated_methods;
    report.unsupported_virtual_methods = virtuals.unsupported_methods;
    report.unsupported_virtual_types = virtuals.unsupported_types.clone();
    report.unsafe_pointer_virtual_methods = virtuals.unsafe_pointer_methods;
    report.unsafe_pointer_virtual_types = virtuals.unsafe_pointer_types.clone();
    populate_full_coverage(api, &methods, &virtuals.methods, &public_api, &mut report)?;

    let mut output = String::new();
    writeln!(output, "// @generated by godot_codegen; DO NOT EDIT.").unwrap();
    writeln!(
        output,
        "// Generated from authenticated Godot {}.{}.{} extension_api.json.",
        api.header.version_major, api.header.version_minor, api.header.version_patch
    )
    .unwrap();
    writeln!(output, "// Source SHA-256: {source_sha256}").unwrap();
    writeln!(output).unwrap();
    writeln!(
        output,
        "#[doc(hidden)]\npub const GENERATED_GODOT_API: &str = \"{}.{}\";",
        api.header.version_major, api.header.version_minor
    )
    .unwrap();
    writeln!(
        output,
        "#[doc(hidden)]\npub const GENERATED_ENGINE_METHOD_COUNT: usize = {};",
        report.generated_methods
    )
    .unwrap();
    writeln!(
        output,
        "#[doc(hidden)]\npub const SKIPPED_ENGINE_METHOD_COUNT: usize = {};",
        report.total_methods - report.generated_methods
    )
    .unwrap();
    for (name, count) in [
        ("VIRTUAL_ENGINE_METHOD_COUNT", report.virtual_methods),
        (
            "GENERATED_VIRTUAL_OVERRIDE_COUNT",
            report.generated_virtual_methods,
        ),
        (
            "UNSUPPORTED_VIRTUAL_OVERRIDE_COUNT",
            report.unsupported_virtual_methods,
        ),
        (
            "UNSAFE_POINTER_VIRTUAL_OVERRIDE_COUNT",
            report.unsafe_pointer_virtual_methods,
        ),
        (
            "GENERATED_STATIC_ENGINE_METHOD_COUNT",
            report.static_methods,
        ),
        (
            "GENERATED_VARARG_ENGINE_METHOD_COUNT",
            report.vararg_methods,
        ),
        ("HASHLESS_ENGINE_METHOD_COUNT", report.methods_without_hash),
        (
            "TYPE_BLOCKED_ENGINE_METHOD_COUNT",
            report.methods_with_unsupported_types,
        ),
        (
            "UNSAFE_POINTER_ENGINE_METHOD_COUNT",
            report.unsafe_pointer_methods,
        ),
    ] {
        writeln!(output, "#[doc(hidden)]\npub const {name}: usize = {count};").unwrap();
    }
    writeln!(output).unwrap();

    emit_enums(&mut output, &enums.definitions);
    emit_global_constants(&mut output, api);

    let classes = sorted_classes
        .iter()
        .map(|class| (class.name.as_str(), *class))
        .collect::<HashMap<_, _>>();
    emit_inheritance(&mut output, &sorted_classes, &classes)?;
    emit_virtual_traits(&mut output, &sorted_classes, &virtuals.methods);
    emit_native_virtual_registrars(&mut output, &virtuals.methods);

    for binding in &public_api.entries {
        emit_api_contract(&mut output, binding);
    }
    let mut by_class = HashMap::<&str, Vec<&MethodBinding<'_>>>::new();
    for method in &methods {
        by_class
            .entry(method.class.name.as_str())
            .or_default()
            .push(method);
        emit_contract(&mut output, method);
    }
    for class in sorted_classes {
        let Some(methods) = by_class.get(class.name.as_str()) else {
            continue;
        };
        emit_class_api(&mut output, class, methods);
    }
    emit_class_constants(&mut output, api);
    emit_utility_api(&mut output, api, &public_api.entries);
    emit_builtin_api(&mut output, api, &public_api.entries);
    emit_singletons_and_constructors(&mut output, api, &public_api.entries);
    emit_engine_signals(&mut output, api, &enums.values)?;
    Ok(output)
}

/// Classifies every class method using the exact same rules as code generation.
pub fn analyze_engine_api(
    api: &ExtensionApi,
) -> Result<EngineApiGenerationReport, EngineApiGenerationError> {
    let enums = collect_enum_bindings(api)?;
    let (_, methods, mut report) = collect_method_bindings(api, &enums.values)?;
    let virtuals = collect_virtual_bindings(api, &enums.values)?;
    let public_api = collect_public_api_bindings(api, &enums.values)?;
    report.generated_virtual_methods = virtuals.generated_methods;
    report.unsupported_virtual_methods = virtuals.unsupported_methods;
    report.unsafe_pointer_virtual_methods = virtuals.unsafe_pointer_methods;
    populate_full_coverage(api, &methods, &virtuals.methods, &public_api, &mut report)?;
    report.unsupported_virtual_types = virtuals.unsupported_types;
    report.unsafe_pointer_virtual_types = virtuals.unsafe_pointer_types;
    Ok(report)
}

/// Rejects incomplete or internally inconsistent Object-class method coverage.
///
/// Raw engine pointers are intentionally excluded from the project-module ABI.
/// Every other official class method must be generated, or this check fails.
pub fn verify_engine_api_coverage(
    report: &EngineApiGenerationReport,
) -> Result<(), EngineApiGenerationError> {
    if report.total_official_entries != report.coverage.len()
        || report.generated_official_entries + report.classified_official_entries
            != report.total_official_entries
    {
        return Err(EngineApiGenerationError::new(format!(
            "full API coverage is inconsistent: {} generated + {} classified != {} official entries",
            report.generated_official_entries,
            report.classified_official_entries,
            report.total_official_entries
        )));
    }
    let mut identities = HashSet::new();
    for entry in &report.coverage {
        if entry.category.is_empty()
            || entry.identity.is_empty()
            || entry.reason.is_empty()
            || !identities.insert((entry.category.as_str(), entry.identity.as_str()))
        {
            return Err(EngineApiGenerationError::new(
                "full API coverage contains an empty or duplicate classification",
            ));
        }
        if entry.disposition == ApiCoverageDisposition::UnsafeNativePointer
            && !entry.category.contains("native")
            && entry.category != "engine_method"
        {
            return Err(EngineApiGenerationError::new(format!(
                "safe API entry `{}` was incorrectly classified as a native pointer",
                entry.identity
            )));
        }
    }
    let accounted_methods = report.generated_methods
        + report.virtual_methods
        + report.methods_without_hash
        + report.methods_with_unsupported_types
        + report.unsafe_pointer_methods;
    if accounted_methods != report.total_methods {
        return Err(EngineApiGenerationError::new(format!(
            "engine API classification is inconsistent: {} of {} methods accounted for",
            accounted_methods, report.total_methods
        )));
    }

    let accounted_virtual_methods = report.generated_virtual_methods
        + report.unsupported_virtual_methods
        + report.unsafe_pointer_virtual_methods;
    if accounted_virtual_methods != report.virtual_methods {
        return Err(EngineApiGenerationError::new(format!(
            "virtual engine API classification is inconsistent: {} of {} methods accounted for",
            accounted_virtual_methods, report.virtual_methods
        )));
    }

    let generated_engine_method_entries = report
        .coverage
        .iter()
        .filter(|entry| {
            entry.category == "engine_method"
                && entry.disposition == ApiCoverageDisposition::Generated
        })
        .count();
    let unsafe_engine_method_entries = report
        .coverage
        .iter()
        .filter(|entry| {
            entry.category == "engine_method"
                && entry.disposition == ApiCoverageDisposition::UnsafeNativePointer
        })
        .count();
    let hashless_engine_method_entries = report
        .coverage
        .iter()
        .filter(|entry| {
            entry.category == "engine_method"
                && entry.disposition == ApiCoverageDisposition::MissingRequiredHash
        })
        .count();
    let unsupported_engine_method_entries = report
        .coverage
        .iter()
        .filter(|entry| {
            entry.category == "engine_method"
                && entry.disposition == ApiCoverageDisposition::UnsupportedSafeType
        })
        .count();
    if generated_engine_method_entries
        != report.generated_methods + report.generated_virtual_methods
        || unsafe_engine_method_entries
            != report.unsafe_pointer_methods + report.unsafe_pointer_virtual_methods
        || hashless_engine_method_entries != report.methods_without_hash
        || unsupported_engine_method_entries
            != report.methods_with_unsupported_types + report.unsupported_virtual_methods
    {
        return Err(EngineApiGenerationError::new(format!(
            "engine method coverage classifications disagree with generation: \
             generated {generated_engine_method_entries}, unsafe pointer \
             {unsafe_engine_method_entries}, missing hash \
             {hashless_engine_method_entries}, unsupported \
             {unsupported_engine_method_entries}"
        )));
    }

    if report.methods_without_hash != 0 {
        return Err(EngineApiGenerationError::new(format!(
            "{} non-virtual engine methods lack the MethodBind hash required for ptrcall",
            report.methods_without_hash
        )));
    }
    if report.methods_with_unsupported_types != 0 {
        let types = report
            .unsupported_types
            .iter()
            .map(|type_| type_.godot_type.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(EngineApiGenerationError::new(format!(
            "{} non-virtual engine methods use unsupported safe types: {types}",
            report.methods_with_unsupported_types
        )));
    }
    if report.unsupported_virtual_methods != 0 {
        let types = report
            .unsupported_virtual_types
            .iter()
            .map(|type_| type_.godot_type.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(EngineApiGenerationError::new(format!(
            "{} virtual engine methods use unsupported safe types: {types}",
            report.unsupported_virtual_methods
        )));
    }

    for type_ in report
        .unsafe_pointer_types
        .iter()
        .chain(&report.unsafe_pointer_virtual_types)
    {
        if !type_.godot_type.contains('*') {
            return Err(EngineApiGenerationError::new(format!(
                "non-pointer type `{}` was classified as an unsafe raw pointer",
                type_.godot_type
            )));
        }
    }
    Ok(())
}

fn collect_enum_bindings(api: &ExtensionApi) -> Result<EnumBindings<'_>, EngineApiGenerationError> {
    let mut definitions = BTreeMap::<String, Vec<EnumDefinition<'_>>>::new();
    let mut values = HashMap::new();
    let mut rust_types = BTreeSet::new();

    for enum_ in &api.global_enums {
        let (module, owner, name) = if let Some((owner, name)) = enum_.name.split_once('.') {
            validate_identifier(owner, "Godot enum namespace")?;
            (rust_module_identifier(owner), Some(owner), name)
        } else {
            ("global".to_owned(), None, enum_.name.as_str())
        };
        register_enum(
            &mut definitions,
            &mut values,
            &mut rust_types,
            EnumSource {
                module: &module,
                owner,
                name,
                is_bitfield: enum_.is_bitfield,
                values: &enum_.values,
            },
        )?;
    }
    for builtin in &api.builtin_classes {
        validate_identifier(&builtin.name, "Godot builtin")?;
        let module = rust_module_identifier(&builtin.name);
        for enum_ in &builtin.enums {
            register_enum(
                &mut definitions,
                &mut values,
                &mut rust_types,
                EnumSource {
                    module: &module,
                    owner: Some(&builtin.name),
                    name: &enum_.name,
                    is_bitfield: false,
                    values: &enum_.values,
                },
            )?;
        }
    }
    for class in &api.classes {
        validate_identifier(&class.name, "Godot class")?;
        let module = rust_module_identifier(&class.name);
        for enum_ in &class.enums {
            register_enum(
                &mut definitions,
                &mut values,
                &mut rust_types,
                EnumSource {
                    module: &module,
                    owner: Some(&class.name),
                    name: &enum_.name,
                    is_bitfield: enum_.is_bitfield,
                    values: &enum_.values,
                },
            )?;
        }
    }
    for definitions in definitions.values_mut() {
        definitions.sort_by(|left, right| left.rust_name.cmp(&right.rust_name));
    }
    Ok(EnumBindings {
        definitions,
        values,
    })
}

fn register_enum<'api>(
    definitions: &mut BTreeMap<String, Vec<EnumDefinition<'api>>>,
    values: &mut HashMap<String, ValueBinding>,
    rust_types: &mut BTreeSet<(String, String)>,
    source: EnumSource<'_, 'api>,
) -> Result<(), EngineApiGenerationError> {
    validate_identifier(source.name, "Godot enum")?;
    for value in source.values {
        validate_identifier(&value.name, "Godot enum value")?;
        if source.is_bitfield && value.value < 0 {
            return Err(EngineApiGenerationError::new(format!(
                "Godot bitfield `{}.{}` contains negative value `{}`",
                source.owner.unwrap_or("global"),
                source.name,
                value.name
            )));
        }
    }
    let scope = source
        .owner
        .map_or_else(String::new, |owner| format!("{owner}."));
    let prefix = if source.is_bitfield {
        "bitfield::"
    } else {
        "enum::"
    };
    let godot_name = format!("{prefix}{scope}{}", source.name);
    let rust_type = (source.module.to_owned(), source.name.to_owned());
    if !rust_types.insert(rust_type.clone()) {
        return Err(EngineApiGenerationError::new(format!(
            "generated Godot enum type collision: {}::{}",
            rust_type.0, rust_type.1
        )));
    }
    let value = ValueBinding {
        rust_type: format!("{}::{}", rust_type.0, rust_type.1),
        value_type: if source.is_bitfield { "U64" } else { "I64" },
        ptrcall_type: if source.is_bitfield { "U64" } else { "I64" },
        class_name: None,
        typed_array_element: None,
    };
    if values.insert(godot_name.clone(), value).is_some() {
        return Err(EngineApiGenerationError::new(format!(
            "duplicate Godot enum binding `{godot_name}`"
        )));
    }
    definitions
        .entry(source.module.to_owned())
        .or_default()
        .push(EnumDefinition {
            godot_name,
            rust_name: source.name.to_owned(),
            is_bitfield: source.is_bitfield,
            values: source.values,
        });
    Ok(())
}

fn emit_enums(output: &mut String, definitions: &BTreeMap<String, Vec<EnumDefinition<'_>>>) {
    for (module, definitions) in definitions {
        writeln!(
            output,
            "/// Generated enums and bitfields in Godot `{}`.\npub mod {module} {{",
            if module == "global" {
                "global scope"
            } else {
                module
            }
        )
        .unwrap();
        for definition in definitions {
            let macro_name = if definition.is_bitfield {
                "define_godot_bitfield"
            } else {
                "define_godot_enum"
            };
            writeln!(output, "    {macro_name}! {{").unwrap();
            writeln!(
                output,
                "        #[doc = {:?}]",
                format!(
                    "Unknown-value-safe binding for Godot `{}`.",
                    definition.godot_name
                )
            )
            .unwrap();
            writeln!(output, "        pub struct {} {{", definition.rust_name).unwrap();
            for value in definition.values {
                writeln!(
                    output,
                    "            #[doc = {:?}]",
                    format!("Godot `{}`.", value.name)
                )
                .unwrap();
                if definition.is_bitfield {
                    writeln!(
                        output,
                        "            {} = {}_u64,",
                        value.name, value.value as u64
                    )
                    .unwrap();
                } else {
                    writeln!(output, "            {} = {}_i64,", value.name, value.value).unwrap();
                }
            }
            writeln!(output, "        }}\n    }}").unwrap();
        }
        writeln!(output, "}}\n").unwrap();
    }
}

fn collect_method_bindings<'api>(
    api: &'api ExtensionApi,
    enums: &HashMap<String, ValueBinding>,
) -> Result<
    (
        Vec<&'api ApiClass>,
        Vec<MethodBinding<'api>>,
        EngineApiGenerationReport,
    ),
    EngineApiGenerationError,
> {
    let classes = api
        .classes
        .iter()
        .map(|class| (class.name.as_str(), class))
        .collect::<HashMap<_, _>>();
    for class in api.classes.iter() {
        validate_identifier(&class.name, "Godot class")?;
    }

    let mut sorted_classes = api.classes.iter().collect::<Vec<_>>();
    sorted_classes.sort_by(|left, right| left.name.cmp(&right.name));
    let mut methods = Vec::new();
    let mut method_ids = HashMap::new();
    let mut rust_method_names = HashMap::<&str, HashSet<String>>::new();
    let mut report = GenerationReportBuilder::default();
    for class in &sorted_classes {
        let mut class_methods = class.methods.iter().collect::<Vec<_>>();
        class_methods.sort_by(|left, right| left.name.cmp(&right.name));
        for method in class_methods {
            report.total_methods += 1;
            match bind_method(class, method, &classes, enums)? {
                MethodBindingOutcome::Bound(binding) => {
                    if let Some((previous_class, previous_method)) =
                        method_ids.insert(binding.id, (&class.name, &method.name))
                    {
                        return Err(EngineApiGenerationError::new(format!(
                            "generated method ID collision: {previous_class}.{previous_method} and {}.{}",
                            class.name, method.name
                        )));
                    }
                    let rust_name = rust_module_identifier(&method.name);
                    if !rust_method_names
                        .entry(class.name.as_str())
                        .or_default()
                        .insert(rust_name.clone())
                    {
                        return Err(EngineApiGenerationError::new(format!(
                            "Godot class `{}` has duplicate generated Rust method `{rust_name}`",
                            class.name
                        )));
                    }
                    if method.is_static {
                        report.static_methods += 1;
                    }
                    if method.is_vararg {
                        report.vararg_methods += 1;
                    }
                    methods.push(binding);
                    report.generated_methods += 1;
                }
                MethodBindingOutcome::Skipped(reason) => report.record_skip(reason),
            }
        }
    }
    Ok((sorted_classes, methods, report.finish()))
}

fn collect_virtual_bindings<'api>(
    api: &'api ExtensionApi,
    enums: &HashMap<String, ValueBinding>,
) -> Result<VirtualBindings<'api>, EngineApiGenerationError> {
    let classes = api
        .classes
        .iter()
        .map(|class| (class.name.as_str(), class))
        .collect::<HashMap<_, _>>();
    let mut methods = Vec::new();
    let mut generated_methods = 0;
    let mut unsupported_methods = 0;
    let mut unsupported_types = BTreeMap::<String, usize>::new();
    let mut unsafe_pointer_methods = 0;
    let mut unsafe_pointer_types = BTreeMap::<String, usize>::new();

    for class in &api.classes {
        for method in &class.methods {
            if !method.is_virtual {
                continue;
            }
            match bind_virtual_method(class, method, &classes, enums)? {
                Ok(binding) => {
                    methods.push(binding);
                    generated_methods += 1;
                }
                Err(types) => {
                    if contains_only_unsafe_pointer_types(&types) {
                        unsafe_pointer_methods += 1;
                        for type_ in types {
                            *unsafe_pointer_types.entry(type_).or_default() += 1;
                        }
                    } else {
                        unsupported_methods += 1;
                        for type_ in types {
                            *unsupported_types.entry(type_).or_default() += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(VirtualBindings {
        methods,
        generated_methods,
        unsupported_methods,
        unsupported_types: sorted_type_counts(unsupported_types),
        unsafe_pointer_methods,
        unsafe_pointer_types: sorted_type_counts(unsafe_pointer_types),
    })
}

fn bind_virtual_method<'api>(
    class: &'api ApiClass,
    method: &'api ApiMethod,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Result<Result<VirtualMethodBinding<'api>, BTreeSet<String>>, EngineApiGenerationError> {
    validate_identifier(&method.name, "Godot virtual method")?;
    let mut used_names = HashSet::from([String::from("self")]);
    let mut arguments = Vec::with_capacity(method.arguments.len());
    let mut unsupported = BTreeSet::new();
    for (index, argument) in method.arguments.iter().enumerate() {
        let Some(mut value) =
            bind_value(&argument.r#type, argument.meta.as_deref(), classes, enums)
        else {
            unsupported.insert(godot_type_name(&argument.r#type, argument.meta.as_deref()));
            continue;
        };
        if value.class_name.is_some() && argument.default_value.as_deref() == Some("null") {
            value.rust_type = format!("Option<{}>", value.rust_type);
        }
        let base_name = if argument.name.is_empty() {
            format!("argument_{index}")
        } else {
            rust_argument_identifier(&argument.name)
        };
        let mut rust_name = base_name.clone();
        let mut suffix = 2;
        while !used_names.insert(rust_name.clone()) {
            rust_name = format!("{base_name}_{suffix}");
            suffix += 1;
        }
        arguments.push(ArgumentBinding { rust_name, value });
    }
    let return_value = if let Some(return_value) = method.return_value.as_ref() {
        let Some(mut value) = bind_value(
            &return_value.r#type,
            return_value.meta.as_deref(),
            classes,
            enums,
        ) else {
            unsupported.insert(godot_type_name(
                &return_value.r#type,
                return_value.meta.as_deref(),
            ));
            return Ok(Err(unsupported));
        };
        if value.class_name.is_some() {
            value.rust_type = format!("Option<{}>", value.rust_type);
        }
        value
    } else {
        ValueBinding {
            rust_type: "()".into(),
            value_type: "NIL",
            ptrcall_type: "VOID",
            class_name: None,
            typed_array_element: None,
        }
    };
    if !unsupported.is_empty() {
        return Ok(Err(unsupported));
    }
    Ok(Ok(VirtualMethodBinding {
        class,
        method,
        arguments,
        return_value,
    }))
}

fn is_script_lifecycle(class: &ApiClass, name: &str) -> bool {
    class.name == "Node"
        && matches!(
            name,
            "_enter_tree"
                | "_ready"
                | "_process"
                | "_physics_process"
                | "_input"
                | "_unhandled_input"
                | "_exit_tree"
        )
}

fn emit_virtual_traits(
    output: &mut String,
    classes: &[&ApiClass],
    methods: &[VirtualMethodBinding<'_>],
) {
    let mut by_class = HashMap::<&str, Vec<&VirtualMethodBinding<'_>>>::new();
    for method in methods {
        if is_script_lifecycle(method.class, &method.method.name) {
            continue;
        }
        by_class
            .entry(method.class.name.as_str())
            .or_default()
            .push(method);
    }
    for class in classes {
        let Some(methods) = by_class.get_mut(class.name.as_str()) else {
            continue;
        };
        methods.sort_by(|left, right| left.method.name.cmp(&right.method.name));
        writeln!(
            output,
            "/// Compile-time checked Godot virtual callbacks declared by `{}`.",
            class.name
        )
        .unwrap();
        writeln!(
            output,
            "pub trait {}Virtual: crate::script::ScriptInherits<super::{}> {{",
            class.name, class.name
        )
        .unwrap();
        for method in methods {
            writeln!(
                output,
                "    /// Overrides Godot `{}.{}`.",
                class.name, method.method.name
            )
            .unwrap();
            write!(output, "    fn {}(&mut self", method.method.name).unwrap();
            for argument in &method.arguments {
                write!(
                    output,
                    ", {}: {}",
                    argument.rust_name, argument.value.rust_type
                )
                .unwrap();
            }
            write!(output, ")").unwrap();
            if method.return_value.rust_type != "()" {
                write!(output, " -> {}", method.return_value.rust_type).unwrap();
            }
            writeln!(output, " {{").unwrap();
            if !method.arguments.is_empty() {
                write!(output, "        let _ = (").unwrap();
                for argument in &method.arguments {
                    write!(output, "{},", argument.rust_name).unwrap();
                }
                writeln!(output, ");").unwrap();
            }
            if method.return_value.rust_type != "()" {
                writeln!(
                    output,
                    "        panic!({:?})",
                    format!(
                        "Godot virtual method `{}.{}` has no Rust implementation",
                        class.name, method.method.name
                    )
                )
                .unwrap();
            }
            writeln!(output, "    }}").unwrap();
        }
        writeln!(output, "}}\n").unwrap();
    }
}

fn emit_native_virtual_registrars(output: &mut String, methods: &[VirtualMethodBinding<'_>]) {
    let mut by_class = HashMap::<&str, Vec<&VirtualMethodBinding<'_>>>::new();
    for method in methods {
        by_class
            .entry(method.class.name.as_str())
            .or_default()
            .push(method);
    }
    writeln!(
        output,
        "/// Type-safe Extension Mode registration for every safe Godot virtual method."
    )
    .unwrap();
    writeln!(output, "#[allow(clippy::type_complexity)]").unwrap();
    writeln!(output, "pub mod native_virtual {{").unwrap();
    writeln!(output, "    use crate::engine::*;").unwrap();
    let mut class_names = by_class.keys().copied().collect::<Vec<_>>();
    class_names.sort_unstable();
    for class_name in class_names {
        let methods = by_class
            .get_mut(class_name)
            .expect("collected class is present");
        methods.sort_by(|left, right| left.method.name.cmp(&right.method.name));
        let module_name = rust_module_identifier(class_name);
        writeln!(
            output,
            "    /// Virtual methods declared by Godot `{class_name}`."
        )
        .unwrap();
        writeln!(output, "    pub mod {module_name} {{").unwrap();
        writeln!(output, "        #[allow(unused_imports)]").unwrap();
        writeln!(output, "        use super::*;").unwrap();
        for method in methods {
            let Some(hash) = method.method.hash else {
                // Synthetic test inputs and incomplete third-party API dumps
                // may omit a Method Hash. The official snapshots validate
                // every safe virtual before this emitter is reached.
                continue;
            };
            let method_name = rust_module_identifier(&method.method.name);
            let argument_count = method.arguments.len();
            let id = method_id(
                method.class,
                method.method,
                &method.arguments,
                &method.return_value,
            );
            writeln!(
                output,
                "        /// Registers Godot `{}.{}` for one Native class.",
                class_name, method.method.name
            )
            .unwrap();
            write!(
                output,
                "        pub fn {method_name}<T>(registrar: &mut crate::native::NativeVirtualRegistrar<'_, T>, method: fn(&mut T"
            )
            .unwrap();
            for argument in &method.arguments {
                write!(
                    output,
                    ", {}",
                    native_virtual_rust_type(&argument.value.rust_type)
                )
                .unwrap();
            }
            write!(output, ")").unwrap();
            if method.return_value.rust_type != "()" {
                write!(
                    output,
                    " -> {}",
                    native_virtual_rust_type(&method.return_value.rust_type)
                )
                .unwrap();
            }
            writeln!(output, ") -> crate::native::NativeResult").unwrap();
            writeln!(
                output,
                "        where T: crate::native::NativeClass, T::Base: crate::engine::Inherits<crate::engine::{class_name}>"
            )
            .unwrap();
            writeln!(output, "        {{").unwrap();
            writeln!(
                output,
                "            registrar.__virtual_method::<{id}_u64, _, _>({class_name:?}, {:?}, {hash}_u32, {argument_count}_u32, method).map(|_| ())",
                method.method.name,
            )
            .unwrap();
            writeln!(output, "        }}").unwrap();
        }
        writeln!(output, "    }}").unwrap();
    }
    writeln!(output, "}}\n").unwrap();
}

fn native_virtual_rust_type(rust_type: &str) -> String {
    let Some(separator) = rust_type.find("::") else {
        return rust_type.to_owned();
    };
    let module = &rust_type[..separator];
    if !module.is_empty()
        && module
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !matches!(module, "crate" | "self" | "super")
    {
        format!("crate::engine::{rust_type}")
    } else {
        rust_type.to_owned()
    }
}

fn emit_inheritance(
    output: &mut String,
    classes: &[&ApiClass],
    class_map: &HashMap<&str, &ApiClass>,
) -> Result<(), EngineApiGenerationError> {
    for class in classes {
        let mut parent = class.inherits.as_deref();
        let mut visited = HashSet::new();
        while let Some(parent_name) = parent {
            if !visited.insert(parent_name) {
                return Err(EngineApiGenerationError::new(format!(
                    "Godot class `{}` has a cyclic inheritance chain",
                    class.name
                )));
            }
            let parent_class = class_map.get(parent_name).ok_or_else(|| {
                EngineApiGenerationError::new(format!(
                    "Godot class `{}` inherits missing class `{parent_name}`",
                    class.name
                ))
            })?;
            writeln!(
                output,
                "impl super::Inherits<super::{parent_name}> for super::{} {{}}",
                class.name
            )
            .unwrap();
            parent = parent_class.inherits.as_deref();
        }
    }
    writeln!(output).unwrap();
    Ok(())
}

fn emit_contract(output: &mut String, binding: &MethodBinding<'_>) {
    let name = format!("__GODOT_RS_METHOD_{:016X}", binding.id);
    let arguments_name = format!("{name}_ARGUMENTS");
    writeln!(
        output,
        "const {arguments_name}: &[crate::abi::AbiGodotValueSpecV1] = &["
    )
    .unwrap();
    for argument in &binding.arguments {
        emit_value_contract(output, &argument.value, "    ");
    }
    writeln!(output, "];").unwrap();
    writeln!(
        output,
        "const {name}: crate::abi::AbiGodotMethodSpecV1 = crate::abi::AbiGodotMethodSpecV1 {{"
    )
    .unwrap();
    writeln!(
        output,
        "    struct_size: crate::abi::AbiGodotMethodSpecV1::MINIMUM_SIZE,"
    )
    .unwrap();
    let mut method_flags = Vec::new();
    if binding.method.is_static {
        method_flags.push("crate::abi::ABI_GODOT_METHOD_STATIC");
    }
    if binding.method.is_vararg {
        method_flags.push("crate::abi::ABI_GODOT_METHOD_VARARG");
    }
    writeln!(
        output,
        "    reserved_flags: {},",
        if method_flags.is_empty() {
            "0".to_owned()
        } else {
            method_flags.join(" | ")
        }
    )
    .unwrap();
    writeln!(output, "    id: {}_u64,", binding.id).unwrap();
    writeln!(
        output,
        "    class_name: crate::abi::AbiByteSlice::from_static({:?}),",
        binding.class.name
    )
    .unwrap();
    writeln!(
        output,
        "    method_name: crate::abi::AbiByteSlice::from_static({:?}),",
        binding.method.name
    )
    .unwrap();
    writeln!(
        output,
        "    method_hash: {}_u64,",
        binding.method.hash.expect("bound methods have a hash")
    )
    .unwrap();
    writeln!(
        output,
        "    arguments: crate::abi::AbiGodotValueSpecSlice::from_static({arguments_name}),"
    )
    .unwrap();
    write!(output, "    return_value: ").unwrap();
    emit_value_contract_expression(output, &binding.return_value, "    ");
    writeln!(output, "    reserved: [0; 4],").unwrap();
    writeln!(output, "}};").unwrap();
    writeln!(output).unwrap();
}

fn emit_api_contract(output: &mut String, binding: &GeneratedApiBinding) {
    let name = format!("__GODOT_RS_API_{:016X}", binding.id);
    let arguments_name = format!("{name}_ARGUMENTS");
    writeln!(
        output,
        "const {arguments_name}: &[crate::abi::AbiGodotValueSpecV1] = &["
    )
    .unwrap();
    for argument in &binding.arguments {
        emit_value_contract(output, &argument.value, "    ");
    }
    writeln!(output, "];").unwrap();
    writeln!(
        output,
        "const {name}: crate::abi::AbiGodotApiSpecV1 = crate::abi::AbiGodotApiSpecV1 {{"
    )
    .unwrap();
    writeln!(
        output,
        "    struct_size: crate::abi::AbiGodotApiSpecV1::MINIMUM_SIZE,"
    )
    .unwrap();
    let mut flags = Vec::new();
    if binding.is_static {
        flags.push("crate::abi::ABI_GODOT_API_STATIC");
    }
    if binding.is_const {
        flags.push("crate::abi::ABI_GODOT_API_CONST");
    }
    if binding.is_vararg {
        flags.push("crate::abi::ABI_GODOT_API_VARARG");
    }
    if binding.mutates_base {
        flags.push("crate::abi::ABI_GODOT_API_MUTATES_BASE");
    }
    writeln!(
        output,
        "    reserved_flags: {},",
        if flags.is_empty() {
            "0".to_owned()
        } else {
            flags.join(" | ")
        }
    )
    .unwrap();
    writeln!(output, "    id: {}_u64,", binding.id).unwrap();
    writeln!(
        output,
        "    kind: crate::abi::AbiGodotApiKind::{},",
        match binding.kind {
            GeneratedApiKind::Utility => "UTILITY_FUNCTION",
            GeneratedApiKind::BuiltinConstructor => "BUILTIN_CONSTRUCTOR",
            GeneratedApiKind::BuiltinMethod => "BUILTIN_METHOD",
            GeneratedApiKind::BuiltinOperator => "BUILTIN_OPERATOR",
            GeneratedApiKind::BuiltinMemberGetter => "BUILTIN_MEMBER_GETTER",
            GeneratedApiKind::BuiltinMemberSetter => "BUILTIN_MEMBER_SETTER",
            GeneratedApiKind::BuiltinIndexedGetter => "BUILTIN_INDEXED_GETTER",
            GeneratedApiKind::BuiltinIndexedSetter => "BUILTIN_INDEXED_SETTER",
            GeneratedApiKind::BuiltinKeyedGetter => "BUILTIN_KEYED_GETTER",
            GeneratedApiKind::BuiltinKeyedSetter => "BUILTIN_KEYED_SETTER",
            GeneratedApiKind::BuiltinConstant => "BUILTIN_CONSTANT",
            GeneratedApiKind::Singleton => "SINGLETON",
            GeneratedApiKind::ObjectConstructor => "OBJECT_CONSTRUCTOR",
        }
    )
    .unwrap();
    for (field, value) in [
        ("owner_name", binding.owner_name.as_deref()),
        ("member_name", binding.member_name.as_deref()),
    ] {
        writeln!(
            output,
            "    {field}: {},",
            value.map_or_else(
                || "crate::abi::AbiByteSlice::EMPTY".to_owned(),
                |value| format!("crate::abi::AbiByteSlice::from_static({value:?})")
            )
        )
        .unwrap();
    }
    writeln!(output, "    numeric: {}_u64,", binding.numeric).unwrap();
    write!(output, "    base_value: ").unwrap();
    emit_value_contract_expression(output, &binding.base_value, "    ");
    writeln!(
        output,
        "    arguments: crate::abi::AbiGodotValueSpecSlice::from_static({arguments_name}),"
    )
    .unwrap();
    write!(output, "    return_value: ").unwrap();
    emit_value_contract_expression(output, &binding.return_value, "    ");
    writeln!(output, "    reserved: [0; 4],").unwrap();
    writeln!(output, "}};\n").unwrap();
}

fn emit_global_constants(output: &mut String, api: &ExtensionApi) {
    if api.global_constants.is_empty() {
        return;
    }
    writeln!(
        output,
        "/// Integer constants declared in Godot's global scope."
    )
    .unwrap();
    writeln!(output, "pub mod global_constants {{").unwrap();
    for constant in &api.global_constants {
        writeln!(
            output,
            "    /// Godot global constant `{}`.\n    pub const {}: i64 = {}_i64;",
            constant.name, constant.name, constant.value
        )
        .unwrap();
    }
    writeln!(output, "}}\n").unwrap();
}

fn emit_class_constants(output: &mut String, api: &ExtensionApi) {
    for class in &api.classes {
        if class.constants.is_empty() {
            continue;
        }
        writeln!(output, "impl super::{} {{", class.name).unwrap();
        for constant in &class.constants {
            writeln!(
                output,
                "    /// Godot `{}.{}`.\n    pub const {}: i64 = {}_i64;",
                class.name, constant.name, constant.name, constant.value
            )
            .unwrap();
        }
        writeln!(output, "}}\n").unwrap();
    }
}

fn emit_utility_api(output: &mut String, api: &ExtensionApi, bindings: &[GeneratedApiBinding]) {
    let by_identity = bindings
        .iter()
        .map(|binding| (binding.identity.as_str(), binding))
        .collect::<HashMap<_, _>>();
    writeln!(
        output,
        "/// Typed wrappers for Godot global utility functions."
    )
    .unwrap();
    writeln!(output, "pub mod utility {{").unwrap();
    let mut utilities = api.utility_functions.iter().collect::<Vec<_>>();
    utilities.sort_by(|left, right| left.name.cmp(&right.name));
    for utility in utilities {
        let identity = format!("utility.{}", utility.name);
        let binding = by_identity[identity.as_str()];
        emit_free_api_function(
            output,
            binding,
            &rust_module_identifier(&utility.name),
            "    ",
            None,
        );
    }
    writeln!(output, "}}\n").unwrap();
}

fn emit_builtin_api(output: &mut String, api: &ExtensionApi, bindings: &[GeneratedApiBinding]) {
    let by_identity = bindings
        .iter()
        .map(|binding| (binding.identity.as_str(), binding))
        .collect::<HashMap<_, _>>();
    writeln!(
        output,
        "/// Complete generated API for Godot builtin value types."
    )
    .unwrap();
    writeln!(output, "pub mod builtin {{").unwrap();
    let mut builtins = api.builtin_classes.iter().collect::<Vec<_>>();
    builtins.sort_by(|left, right| left.name.cmp(&right.name));
    for builtin in builtins {
        let module = rust_module_identifier(&builtin.name);
        writeln!(
            output,
            "    /// Generated Godot `{}` builtin surface.",
            builtin.name
        )
        .unwrap();
        writeln!(output, "    pub mod {module} {{").unwrap();
        for constructor in &builtin.constructors {
            let identity = format!("builtin.{}.constructor.{}", builtin.name, constructor.index);
            emit_free_api_function(
                output,
                by_identity[identity.as_str()],
                &format!("construct_{}", constructor.index),
                "        ",
                None,
            );
        }
        for (index, operator) in builtin.operators.iter().enumerate() {
            let identity = format!(
                "builtin.{}.operator.{}.{}",
                builtin.name, index, operator.name
            );
            let suffix = operator.right_type.as_deref().map_or_else(
                || "unary".to_owned(),
                |value| rust_prefixed_identifier("", value),
            );
            let function = format!(
                "operator_{}_{}_{}",
                operator_word(&operator.name),
                suffix,
                index
            );
            emit_free_api_function(
                output,
                by_identity[identity.as_str()],
                &function,
                "        ",
                Some("base"),
            );
        }
        let mut methods = builtin.methods.iter().collect::<Vec<_>>();
        methods.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.hash.cmp(&right.hash))
        });
        for method in methods {
            let identity = format!(
                "builtin.{}.method.{}.{}",
                builtin.name, method.name, method.hash
            );
            let binding = by_identity[identity.as_str()];
            emit_free_api_function(
                output,
                binding,
                &rust_module_identifier(&method.name),
                "        ",
                (!method.is_static).then_some("base"),
            );
        }
        for member in &builtin.members {
            let identity = format!("builtin.{}.member.{}", builtin.name, member.name);
            emit_free_api_function(
                output,
                by_identity[format!("{identity}.get").as_str()],
                &rust_prefixed_identifier("member_get_", &member.name),
                "        ",
                Some("base"),
            );
            emit_free_api_function(
                output,
                by_identity[format!("{identity}.set").as_str()],
                &rust_prefixed_identifier("member_set_", &member.name),
                "        ",
                Some("base"),
            );
        }
        if builtin.indexing_return_type.is_some() {
            let identity = format!("builtin.{}.indexing", builtin.name);
            emit_free_api_function(
                output,
                by_identity[format!("{identity}.get").as_str()],
                "indexed_get",
                "        ",
                Some("base"),
            );
            emit_free_api_function(
                output,
                by_identity[format!("{identity}.set").as_str()],
                "indexed_set",
                "        ",
                Some("base"),
            );
        }
        if builtin.is_keyed {
            let identity = format!("builtin.{}.keyed", builtin.name);
            emit_free_api_function(
                output,
                by_identity[format!("{identity}.get").as_str()],
                "keyed_get",
                "        ",
                Some("base"),
            );
            emit_free_api_function(
                output,
                by_identity[format!("{identity}.set").as_str()],
                "keyed_set",
                "        ",
                Some("base"),
            );
        }
        for constant in &builtin.constants {
            let identity = format!("builtin.{}.constant.{}", builtin.name, constant.name);
            emit_free_api_function(
                output,
                by_identity[identity.as_str()],
                &rust_prefixed_identifier("constant_", &constant.name),
                "        ",
                None,
            );
        }
        writeln!(output, "    }}").unwrap();
    }
    writeln!(output, "}}\n").unwrap();
    emit_builtin_traits(output, api, &by_identity);
}

fn emit_builtin_traits(
    output: &mut String,
    api: &ExtensionApi,
    by_identity: &HashMap<&str, &GeneratedApiBinding>,
) {
    let mut builtins = api.builtin_classes.iter().collect::<Vec<_>>();
    builtins.sort_by(|left, right| left.name.cmp(&right.name));
    for builtin in builtins {
        let instance_methods = builtin
            .methods
            .iter()
            .filter(|method| !method.is_static)
            .collect::<Vec<_>>();
        if instance_methods.is_empty()
            && builtin.members.is_empty()
            && builtin.indexing_return_type.is_none()
            && !builtin.is_keyed
        {
            continue;
        }
        let module = rust_module_identifier(&builtin.name);
        let trait_name = format!("{}BuiltinApi", rust_type_name(&builtin.name));
        let base = by_identity
            .values()
            .find(|binding| {
                binding.owner_name.as_deref() == Some(builtin.name.as_str())
                    && binding.base_value.ptrcall_type != "VOID"
            })
            .expect("builtin receiver binding");
        let rust_type = nested_rust_type(&base.base_value.rust_type);
        writeln!(
            output,
            "/// Godot-style instance methods for the `{}` builtin.\npub trait {trait_name} {{",
            builtin.name
        )
        .unwrap();
        for method in &instance_methods {
            let identity = format!(
                "builtin.{}.method.{}.{}",
                builtin.name, method.name, method.hash
            );
            emit_builtin_trait_method(
                output,
                by_identity[identity.as_str()],
                &rust_prefixed_identifier("godot_", &method.name),
                &module,
                "    ",
                true,
            );
        }
        for member in &builtin.members {
            let identity = format!("builtin.{}.member.{}", builtin.name, member.name);
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.get").as_str()],
                &rust_prefixed_identifier("godot_member_", &member.name),
                &module,
                "    ",
                true,
            );
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.set").as_str()],
                &rust_prefixed_identifier("set_godot_member_", &member.name),
                &module,
                "    ",
                true,
            );
        }
        if builtin.indexing_return_type.is_some() {
            let identity = format!("builtin.{}.indexing", builtin.name);
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.get").as_str()],
                "godot_index",
                &module,
                "    ",
                true,
            );
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.set").as_str()],
                "set_godot_index",
                &module,
                "    ",
                true,
            );
        }
        if builtin.is_keyed {
            let identity = format!("builtin.{}.keyed", builtin.name);
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.get").as_str()],
                "godot_key",
                &module,
                "    ",
                true,
            );
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.set").as_str()],
                "set_godot_key",
                &module,
                "    ",
                true,
            );
        }
        writeln!(output, "}}\nimpl {trait_name} for {rust_type} {{").unwrap();
        for method in &instance_methods {
            let identity = format!(
                "builtin.{}.method.{}.{}",
                builtin.name, method.name, method.hash
            );
            emit_builtin_trait_method(
                output,
                by_identity[identity.as_str()],
                &rust_prefixed_identifier("godot_", &method.name),
                &module,
                "    ",
                false,
            );
        }
        for member in &builtin.members {
            let identity = format!("builtin.{}.member.{}", builtin.name, member.name);
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.get").as_str()],
                &rust_prefixed_identifier("godot_member_", &member.name),
                &module,
                "    ",
                false,
            );
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.set").as_str()],
                &rust_prefixed_identifier("set_godot_member_", &member.name),
                &module,
                "    ",
                false,
            );
        }
        if builtin.indexing_return_type.is_some() {
            let identity = format!("builtin.{}.indexing", builtin.name);
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.get").as_str()],
                "godot_index",
                &module,
                "    ",
                false,
            );
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.set").as_str()],
                "set_godot_index",
                &module,
                "    ",
                false,
            );
        }
        if builtin.is_keyed {
            let identity = format!("builtin.{}.keyed", builtin.name);
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.get").as_str()],
                "godot_key",
                &module,
                "    ",
                false,
            );
            emit_builtin_trait_method(
                output,
                by_identity[format!("{identity}.set").as_str()],
                "set_godot_key",
                &module,
                "    ",
                false,
            );
        }
        writeln!(output, "}}\n").unwrap();
    }
}

fn emit_builtin_trait_method(
    output: &mut String,
    binding: &GeneratedApiBinding,
    method_name: &str,
    module: &str,
    indent: &str,
    declaration: bool,
) {
    writeln!(output, "{indent}/// Calls Godot `{}`.", binding.identity).unwrap();
    write!(
        output,
        "{indent}fn {method_name}({}",
        if binding.mutates_base {
            "&mut self"
        } else {
            "&self"
        }
    )
    .unwrap();
    for argument in &binding.arguments {
        write!(
            output,
            ", {}: {}",
            argument.rust_name,
            nested_rust_type(&argument.value.rust_type)
        )
        .unwrap();
    }
    if binding.is_vararg {
        write!(output, ", varargs: &[crate::variant::Variant]").unwrap();
    }
    write!(
        output,
        ") -> crate::error::EngineResult<{}>",
        nested_rust_type(&binding.return_value.rust_type)
    )
    .unwrap();
    if declaration {
        writeln!(output, ";").unwrap();
        return;
    }
    writeln!(output, " {{").unwrap();
    let module_function = match binding.kind {
        GeneratedApiKind::BuiltinMemberGetter => rust_prefixed_identifier(
            "member_get_",
            binding.member_name.as_deref().expect("member"),
        ),
        GeneratedApiKind::BuiltinMemberSetter => rust_prefixed_identifier(
            "member_set_",
            binding.member_name.as_deref().expect("member"),
        ),
        GeneratedApiKind::BuiltinIndexedGetter => "indexed_get".to_owned(),
        GeneratedApiKind::BuiltinIndexedSetter => "indexed_set".to_owned(),
        GeneratedApiKind::BuiltinKeyedGetter => "keyed_get".to_owned(),
        GeneratedApiKind::BuiltinKeyedSetter => "keyed_set".to_owned(),
        GeneratedApiKind::BuiltinMethod => {
            rust_module_identifier(binding.member_name.as_deref().expect("method"))
        }
        _ => unreachable!("only instance builtin entries become trait methods"),
    };
    write!(
        output,
        "{indent}    builtin::{module}::{module_function}(self"
    )
    .unwrap();
    for argument in &binding.arguments {
        write!(output, ", {}", argument.rust_name).unwrap();
    }
    if binding.is_vararg {
        write!(output, ", varargs").unwrap();
    }
    writeln!(output, ")\n{indent}}}").unwrap();
}

fn rust_type_name(value: &str) -> String {
    let module = rust_module_identifier(value);
    let mut output = String::new();
    for part in module.trim_start_matches("r#").split('_') {
        let mut characters = part.chars();
        if let Some(first) = characters.next() {
            output.push(first.to_ascii_uppercase());
            output.extend(characters);
        }
    }
    if output.is_empty() {
        "Godot".to_owned()
    } else if output == "Nil" {
        "Variant".to_owned()
    } else {
        output
    }
}

fn emit_singletons_and_constructors(
    output: &mut String,
    api: &ExtensionApi,
    bindings: &[GeneratedApiBinding],
) {
    let by_identity = bindings
        .iter()
        .map(|binding| (binding.identity.as_str(), binding))
        .collect::<HashMap<_, _>>();
    for singleton in &api.singletons {
        let identity = format!("singleton.{}", singleton.name);
        let binding = by_identity[identity.as_str()];
        writeln!(output, "impl super::{} {{", singleton.r#type).unwrap();
        writeln!(
            output,
            "    /// Returns Godot's `{}` singleton.\n    pub fn singleton() -> crate::error::EngineResult<{}> {{",
            singleton.name,
            nested_rust_type(&binding.return_value.rust_type)
        )
        .unwrap();
        writeln!(
            output,
            "        crate::engine::invoke_godot_api(&__GODOT_RS_API_{:016X}, &[])",
            binding.id
        )
        .unwrap();
        writeln!(output, "    }}\n}}\n").unwrap();
    }
    for class in &api.classes {
        if !class.is_instantiable {
            continue;
        }
        let identity = format!("class.{}.constructor", class.name);
        let binding = by_identity[identity.as_str()];
        writeln!(output, "impl super::{} {{", class.name).unwrap();
        writeln!(
            output,
            "    /// Constructs one Godot `{}` and performs post-initialization.\n    pub fn new_godot() -> crate::error::EngineResult<{}> {{",
            class.name,
            nested_rust_type(&binding.return_value.rust_type)
        )
        .unwrap();
        writeln!(
            output,
            "        crate::engine::invoke_godot_api(&__GODOT_RS_API_{:016X}, &[])",
            binding.id
        )
        .unwrap();
        writeln!(output, "    }}\n}}\n").unwrap();
    }
}

fn emit_engine_signals(
    output: &mut String,
    api: &ExtensionApi,
    enums: &HashMap<String, ValueBinding>,
) -> Result<(), EngineApiGenerationError> {
    let classes = api
        .classes
        .iter()
        .map(|class| (class.name.as_str(), class))
        .collect::<HashMap<_, _>>();
    for class in &api.classes {
        if class.signals.is_empty() {
            continue;
        }
        let trait_name = format!("{}Signals", class.name);
        writeln!(
            output,
            "/// Typed signal handles declared by Godot `{}`.\n#[allow(clippy::type_complexity)]\npub trait {trait_name} {{",
            class.name
        )
        .unwrap();
        for signal in &class.signals {
            let tuple = signal_tuple_type(signal, &classes, enums)?;
            let method = rust_prefixed_identifier("signal_", &signal.name);
            writeln!(
                output,
                "    /// Returns Godot signal `{}.{}`.\n    fn {method}(&self) -> crate::error::EngineResult<crate::signal::Signal<{tuple}>>;",
                class.name,
                signal.name
            )
            .unwrap();
        }
        writeln!(
            output,
            "}}\n#[allow(clippy::type_complexity)]\nimpl<R> {trait_name} for R\nwhere\n    R: super::EngineObject,\n    R::Class: super::Inherits<super::{}>,\n{{",
            class.name
        )
        .unwrap();
        for signal in &class.signals {
            let tuple = signal_tuple_type(signal, &classes, enums)?;
            let method = rust_prefixed_identifier("signal_", &signal.name);
            writeln!(
                output,
                "    fn {method}(&self) -> crate::error::EngineResult<crate::signal::Signal<{tuple}>> {{"
            )
            .unwrap();
            writeln!(
                output,
                "        let object = super::EngineObject::__engine_object(self)?;"
            )
            .unwrap();
            writeln!(
                output,
                "        Ok(crate::signal::Signal::__from_object(object, {:?}))",
                signal.name
            )
            .unwrap();
            writeln!(output, "    }}").unwrap();
        }
        writeln!(output, "}}\n").unwrap();
    }
    Ok(())
}

fn signal_tuple_type(
    signal: &crate::ApiSignal,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Result<String, EngineApiGenerationError> {
    if signal.arguments.is_empty() {
        return Ok("()".to_owned());
    }
    let mut types = Vec::with_capacity(signal.arguments.len());
    for argument in &signal.arguments {
        let mut value = bind_value(&argument.r#type, argument.meta.as_deref(), classes, enums)
            .ok_or_else(|| {
                EngineApiGenerationError::new(format!(
                    "Godot signal `{}` uses unsupported type `{}`",
                    signal.name, argument.r#type
                ))
            })?;
        if value.class_name.is_some() && argument.default_value.as_deref() == Some("null") {
            value.rust_type = format!("Option<{}>", value.rust_type);
        }
        types.push(nested_rust_type(&value.rust_type));
    }
    Ok(format!("({},)", types.join(", ")))
}

fn emit_free_api_function(
    output: &mut String,
    binding: &GeneratedApiBinding,
    function_name: &str,
    indent: &str,
    base_name: Option<&str>,
) {
    writeln!(output, "{indent}/// Calls Godot `{}`.", binding.identity).unwrap();
    write!(output, "{indent}pub fn {function_name}(").unwrap();
    let mut needs_comma = false;
    if let Some(base_name) = base_name {
        let qualifier = if binding.mutates_base { "&mut " } else { "&" };
        write!(
            output,
            "{base_name}: {qualifier}{}",
            nested_rust_type(&binding.base_value.rust_type)
        )
        .unwrap();
        needs_comma = true;
    }
    for argument in &binding.arguments {
        if needs_comma {
            write!(output, ", ").unwrap();
        }
        write!(
            output,
            "{}: {}",
            argument.rust_name,
            nested_rust_type(&argument.value.rust_type)
        )
        .unwrap();
        needs_comma = true;
    }
    if binding.is_vararg {
        if needs_comma {
            write!(output, ", ").unwrap();
        }
        write!(output, "varargs: &[crate::variant::Variant]").unwrap();
    }
    writeln!(
        output,
        ") -> crate::error::EngineResult<{}> {{",
        nested_rust_type(&binding.return_value.rust_type)
    )
    .unwrap();
    emit_generated_argument_array(output, binding, &format!("{indent}    "));
    let invocation = if binding.mutates_base {
        "invoke_builtin_api_mut"
    } else if base_name.is_some() {
        "invoke_builtin_api"
    } else {
        "invoke_godot_api"
    };
    let contract = if indent.len() >= 8 {
        format!("super::super::__GODOT_RS_API_{:016X}", binding.id)
    } else {
        format!("super::__GODOT_RS_API_{:016X}", binding.id)
    };
    if base_name.is_some() {
        writeln!(
            output,
            "{indent}    crate::engine::{invocation}(base, &{contract}, &arguments)"
        )
        .unwrap();
    } else {
        writeln!(
            output,
            "{indent}    crate::engine::{invocation}(&{contract}, &arguments)"
        )
        .unwrap();
    }
    writeln!(output, "{indent}}}").unwrap();
}

fn emit_generated_argument_array(output: &mut String, binding: &GeneratedApiBinding, indent: &str) {
    if binding.is_vararg {
        if binding.arguments.is_empty() {
            writeln!(
                output,
                "{indent}let mut arguments = Vec::with_capacity(varargs.len());"
            )
            .unwrap();
        } else {
            writeln!(
                output,
                "{indent}let mut arguments = Vec::with_capacity({} + varargs.len());",
                binding.arguments.len()
            )
            .unwrap();
        }
        if !binding.arguments.is_empty() {
            writeln!(output, "{indent}arguments.extend([").unwrap();
        }
    } else {
        writeln!(output, "{indent}let arguments = [").unwrap();
    }
    for argument in &binding.arguments {
        let expression = match argument.value.value_type {
            "STRING_NAME" => format!(
                "crate::engine::string_name_argument({})",
                argument.rust_name
            ),
            "NODE_PATH" => format!("crate::engine::node_path_argument({})", argument.rust_name),
            _ => format!(
                "crate::engine::EngineArgument::__into_engine_argument({})",
                argument.rust_name
            ),
        };
        writeln!(output, "{indent}    {expression},").unwrap();
    }
    if binding.is_vararg {
        if !binding.arguments.is_empty() {
            writeln!(output, "{indent}]);").unwrap();
        }
        writeln!(
            output,
            "{indent}arguments.extend(varargs.iter().map(crate::engine::EngineArgument::__into_engine_argument));"
        )
        .unwrap();
    } else {
        writeln!(output, "{indent}];").unwrap();
    }
}

fn nested_rust_type(type_: &str) -> String {
    if let Some(inner) = type_.strip_prefix('&') {
        return format!("&{}", nested_rust_type(inner));
    }
    for wrapper in ["Option", "super::GodotRef", "super::ObjectRef"] {
        if let Some(inner) = type_
            .strip_prefix(wrapper)
            .and_then(|value| value.strip_prefix('<'))
            .and_then(|value| value.strip_suffix('>'))
        {
            let wrapper = wrapper.replace("super::", "crate::engine::");
            return format!("{wrapper}<{}>", nested_rust_type(inner));
        }
    }
    if type_.starts_with("super::") {
        return type_.replacen("super::", "crate::engine::", 1);
    }
    if type_.starts_with("crate::")
        || matches!(
            type_,
            "()" | "bool"
                | "char"
                | "String"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "f32"
                | "f64"
        )
    {
        type_.to_owned()
    } else if type_.contains("::") {
        format!("crate::engine::{type_}")
    } else {
        type_.to_owned()
    }
}

fn operator_word(operator: &str) -> &'static str {
    match operator {
        "==" => "equal",
        "!=" => "not_equal",
        "<" => "less",
        "<=" => "less_equal",
        ">" => "greater",
        ">=" => "greater_equal",
        "+" => "add",
        "-" => "subtract",
        "unary-" => "negate",
        "unary+" => "positive",
        "*" => "multiply",
        "/" => "divide",
        "%" => "remainder",
        "**" => "power",
        "<<" => "shift_left",
        ">>" => "shift_right",
        "&" => "bit_and",
        "|" => "bit_or",
        "^" => "bit_xor",
        "~" => "bit_not",
        "and" => "and",
        "or" => "or",
        "xor" => "xor",
        "not" => "not",
        "in" => "in",
        _ => "operator",
    }
}

fn emit_value_contract(output: &mut String, value: &ValueBinding, indent: &str) {
    write!(output, "{indent}").unwrap();
    emit_value_contract_expression(output, value, indent);
}

fn emit_value_contract_expression(output: &mut String, value: &ValueBinding, indent: &str) {
    writeln!(
        output,
        "crate::abi::AbiGodotValueSpecV1 {{\n{indent}    value_type: crate::abi::AbiValueType::{},\n{indent}    ptrcall_type: crate::abi::AbiPtrcallType::{},",
        value.value_type, value.ptrcall_type
    )
    .unwrap();
    if let Some(class_name) = value
        .class_name
        .as_ref()
        .or(value.typed_array_element.as_ref())
    {
        writeln!(
            output,
            "{indent}    class_name: crate::abi::AbiByteSlice::from_static({class_name:?}),"
        )
        .unwrap();
    } else {
        writeln!(
            output,
            "{indent}    class_name: crate::abi::AbiByteSlice::EMPTY,"
        )
        .unwrap();
    }
    let reserved_flags = if value.typed_array_element.is_some() {
        "crate::abi::ABI_GODOT_VALUE_TYPED_ARRAY"
    } else {
        "0"
    };
    writeln!(
        output,
        "{indent}    reserved_flags: {reserved_flags},\n{indent}    reserved: [0; 2],\n{indent}}},"
    )
    .unwrap();
}

fn emit_class_api(output: &mut String, class: &ApiClass, methods: &[&MethodBinding<'_>]) {
    let instance_methods = methods
        .iter()
        .copied()
        .filter(|binding| !binding.method.is_static)
        .collect::<Vec<_>>();
    let static_methods = methods
        .iter()
        .copied()
        .filter(|binding| binding.method.is_static)
        .collect::<Vec<_>>();
    if !instance_methods.is_empty() {
        emit_trait(output, class, &instance_methods);
    }
    if !static_methods.is_empty() {
        emit_static_impl(output, class, &static_methods);
    }
}

fn emit_trait(output: &mut String, class: &ApiClass, methods: &[&MethodBinding<'_>]) {
    let trait_name = format!("{}Api", class.name);
    writeln!(
        output,
        "/// Type-safe methods declared by Godot `{}`.",
        class.name
    )
    .unwrap();
    writeln!(output, "pub trait {trait_name} {{").unwrap();
    for binding in methods {
        emit_trait_signature(output, binding, "    ", true);
    }
    writeln!(output, "}}").unwrap();
    writeln!(
        output,
        "impl<R> {trait_name} for R\nwhere\n    R: super::EngineObject,\n    R::Class: super::Inherits<super::{}>,\n{{",
        class.name
    )
    .unwrap();
    for binding in methods {
        emit_trait_signature(output, binding, "    ", false);
        writeln!(
            output,
            "        let receiver = super::EngineObject::__engine_object(self)?;"
        )
        .unwrap();
        emit_argument_array(output, binding, "        ");
        writeln!(
            output,
            "        super::invoke_engine_method(receiver, &__GODOT_RS_METHOD_{:016X}, &arguments)",
            binding.id
        )
        .unwrap();
        writeln!(output, "    }}").unwrap();
    }
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
}

fn emit_static_impl(output: &mut String, class: &ApiClass, methods: &[&MethodBinding<'_>]) {
    writeln!(
        output,
        "/// Type-safe class-level methods declared by Godot `{}`.",
        class.name
    )
    .unwrap();
    writeln!(output, "impl super::{} {{", class.name).unwrap();
    for binding in methods {
        let method_name = rust_module_identifier(&binding.method.name);
        if method_name == "new" {
            writeln!(output, "    #[allow(clippy::new_ret_no_self)]").unwrap();
        }
        writeln!(
            output,
            "    /// Calls the class-level Godot `{}.{}`.",
            binding.class.name, binding.method.name
        )
        .unwrap();
        write!(output, "    pub fn {method_name}(").unwrap();
        for (index, argument) in binding.arguments.iter().enumerate() {
            if index != 0 {
                write!(output, ", ").unwrap();
            }
            write!(
                output,
                "{}: {}",
                argument.rust_name, argument.value.rust_type
            )
            .unwrap();
        }
        if binding.method.is_vararg {
            if !binding.arguments.is_empty() {
                write!(output, ", ").unwrap();
            }
            write!(output, "varargs: &[crate::variant::Variant]").unwrap();
        }
        writeln!(
            output,
            ") -> crate::error::EngineResult<{}> {{",
            binding.return_value.rust_type
        )
        .unwrap();
        emit_argument_array(output, binding, "        ");
        writeln!(
            output,
            "        super::invoke_engine_method(super::ObjectRef::<super::{}>::unresolved(), &__GODOT_RS_METHOD_{:016X}, &arguments)",
            class.name, binding.id
        )
        .unwrap();
        writeln!(output, "    }}").unwrap();
    }
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
}

fn emit_argument_array(output: &mut String, binding: &MethodBinding<'_>, indent: &str) {
    if binding.method.is_vararg {
        let capacity = if binding.arguments.is_empty() {
            "varargs.len()".to_owned()
        } else {
            format!("{} + varargs.len()", binding.arguments.len())
        };
        writeln!(
            output,
            "{indent}let mut arguments = Vec::<crate::abi::AbiValueV1>::with_capacity({capacity});"
        )
        .unwrap();
        if !binding.arguments.is_empty() {
            writeln!(output, "{indent}arguments.extend([").unwrap();
        }
    } else {
        writeln!(output, "{indent}let arguments = [").unwrap();
    }
    for argument in &binding.arguments {
        let expression = match argument.value.value_type {
            "STRING_NAME" => format!("super::string_name_argument({})", argument.rust_name),
            "NODE_PATH" => format!("super::node_path_argument({})", argument.rust_name),
            _ => format!(
                "super::EngineArgument::__into_engine_argument({})",
                argument.rust_name
            ),
        };
        writeln!(output, "{indent}    {expression},").unwrap();
    }
    if binding.method.is_vararg {
        if !binding.arguments.is_empty() {
            writeln!(output, "{indent}]);").unwrap();
        }
        writeln!(
            output,
            "{indent}arguments.extend(varargs.iter().map(super::EngineArgument::__into_engine_argument));"
        )
        .unwrap();
    } else {
        writeln!(output, "{indent}];").unwrap();
    }
}

fn emit_trait_signature(
    output: &mut String,
    binding: &MethodBinding<'_>,
    indent: &str,
    declaration: bool,
) {
    let method_name = rust_module_identifier(&binding.method.name);
    if method_name == "new" {
        writeln!(output, "{indent}#[allow(clippy::new_ret_no_self)]").unwrap();
    }
    writeln!(
        output,
        "{indent}/// Calls Godot `{}.{}`.",
        binding.class.name, binding.method.name
    )
    .unwrap();
    write!(output, "{indent}fn {method_name}(&self").unwrap();
    for argument in &binding.arguments {
        write!(
            output,
            ", {}: {}",
            argument.rust_name, argument.value.rust_type
        )
        .unwrap();
    }
    if binding.method.is_vararg {
        write!(output, ", varargs: &[crate::variant::Variant]").unwrap();
    }
    write!(
        output,
        ") -> crate::error::EngineResult<{}>",
        binding.return_value.rust_type
    )
    .unwrap();
    if declaration {
        writeln!(output, ";").unwrap();
    } else {
        writeln!(output, " {{").unwrap();
    }
}

fn bind_method<'api>(
    class: &'api ApiClass,
    method: &'api ApiMethod,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Result<MethodBindingOutcome<'api>, EngineApiGenerationError> {
    if method.is_virtual {
        return Ok(MethodBindingOutcome::Skipped(MethodSkipReason::Virtual));
    }
    if method.hash.is_none() {
        return Ok(MethodBindingOutcome::Skipped(MethodSkipReason::MissingHash));
    }
    validate_identifier(&method.name, "Godot method")?;
    // Generated method bodies reserve this local for the Godot object that
    // receives ptrcall. Keep an official argument named `receiver` distinct
    // so it cannot be shadowed before the argument array is encoded.
    let mut used_names = HashSet::from([String::from("receiver"), String::from("varargs")]);
    let mut arguments = Vec::with_capacity(method.arguments.len());
    let mut unsupported_types = BTreeSet::new();
    for (index, argument) in method.arguments.iter().enumerate() {
        let Some(value) = bind_argument(argument, classes, enums) else {
            unsupported_types.insert(godot_type_name(&argument.r#type, argument.meta.as_deref()));
            continue;
        };
        let base_name = if argument.name.is_empty() {
            format!("argument_{index}")
        } else {
            rust_argument_identifier(&argument.name)
        };
        let mut rust_name = base_name.clone();
        let mut suffix = 2;
        while !used_names.insert(rust_name.clone()) {
            rust_name = format!("{base_name}_{suffix}");
            suffix += 1;
        }
        arguments.push(ArgumentBinding { rust_name, value });
    }
    let return_value = bind_return(method.return_value.as_ref(), classes, enums);
    if return_value.is_none() {
        if let Some(value) = &method.return_value {
            unsupported_types.insert(godot_type_name(&value.r#type, value.meta.as_deref()));
        }
    }
    if !unsupported_types.is_empty() {
        return Ok(MethodBindingOutcome::Skipped(
            if contains_only_unsafe_pointer_types(&unsupported_types) {
                MethodSkipReason::UnsafePointerTypes(unsupported_types)
            } else {
                MethodSkipReason::UnsupportedTypes(unsupported_types)
            },
        ));
    };
    let return_value = return_value.expect("supported return was checked above");
    let id = method_id(class, method, &arguments, &return_value);
    Ok(MethodBindingOutcome::Bound(MethodBinding {
        class,
        method,
        id,
        arguments,
        return_value,
    }))
}

fn godot_type_name(type_name: &str, meta: Option<&str>) -> String {
    match meta {
        Some(meta) => format!("{type_name} [{meta}]"),
        None => type_name.to_owned(),
    }
}

fn contains_only_unsafe_pointer_types(types: &BTreeSet<String>) -> bool {
    !types.is_empty() && types.iter().all(|type_| type_.contains('*'))
}

fn bind_argument(
    argument: &ApiArgument,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Option<ValueBinding> {
    let mut binding = bind_value(&argument.r#type, argument.meta.as_deref(), classes, enums)?;
    if matches!(binding.value_type, "STRING" | "STRING_NAME" | "NODE_PATH") {
        binding.rust_type = "&str".into();
    }
    if matches!(
        binding.value_type,
        "TRANSFORM2D"
            | "AABB"
            | "BASIS"
            | "TRANSFORM3D"
            | "PROJECTION"
            | "PACKED_BYTE_ARRAY"
            | "PACKED_INT32_ARRAY"
            | "PACKED_INT64_ARRAY"
            | "PACKED_FLOAT32_ARRAY"
            | "PACKED_FLOAT64_ARRAY"
            | "PACKED_STRING_ARRAY"
            | "PACKED_VECTOR2_ARRAY"
            | "PACKED_VECTOR3_ARRAY"
            | "PACKED_COLOR_ARRAY"
            | "PACKED_VECTOR4_ARRAY"
            | "VARIANT"
            | "ARRAY"
            | "DICTIONARY"
            | "CALLABLE"
            | "SIGNAL"
    ) {
        binding.rust_type = format!("&{}", binding.rust_type);
    }
    if binding.class_name.is_some() && argument.default_value.as_deref() == Some("null") {
        binding.rust_type = format!("Option<{}>", binding.rust_type);
    }
    Some(binding)
}

fn bind_return(
    value: Option<&ApiReturnValue>,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Option<ValueBinding> {
    let Some(value) = value else {
        return Some(ValueBinding {
            rust_type: "()".into(),
            value_type: "NIL",
            ptrcall_type: "VOID",
            class_name: None,
            typed_array_element: None,
        });
    };
    let mut binding = bind_value(&value.r#type, value.meta.as_deref(), classes, enums)?;
    if binding.class_name.is_some() {
        if classes.get(value.r#type.as_str())?.is_refcounted {
            binding.rust_type = format!("super::GodotRef<super::{}>", value.r#type);
            binding.ptrcall_type = "REFCOUNTED_OBJECT";
        }
        binding.rust_type = format!("Option<{}>", binding.rust_type);
    }
    Some(binding)
}

fn bind_value(
    type_name: &str,
    meta: Option<&str>,
    classes: &HashMap<&str, &ApiClass>,
    enums: &HashMap<String, ValueBinding>,
) -> Option<ValueBinding> {
    if let Some(element) = type_name.strip_prefix("typedarray::") {
        let element_binding = match element {
            "int" => ValueBinding {
                rust_type: "i64".into(),
                value_type: "I64",
                ptrcall_type: "I64",
                class_name: None,
                typed_array_element: None,
            },
            "float" => ValueBinding {
                rust_type: "f64".into(),
                value_type: "F64",
                ptrcall_type: "F64",
                class_name: None,
                typed_array_element: None,
            },
            _ => bind_value(element, None, classes, enums)?,
        };
        let rust_element = if let Some(class_name) = element_binding.class_name.as_deref() {
            let class = classes.get(class_name)?;
            if class.is_refcounted {
                format!("super::GodotRef<super::{class_name}>")
            } else {
                format!("super::ObjectRef<super::{class_name}>")
            }
        } else {
            element_binding.rust_type
        };
        return Some(ValueBinding {
            rust_type: format!("crate::variant::Array<{rust_element}>"),
            value_type: "ARRAY",
            ptrcall_type: "ARRAY",
            class_name: None,
            typed_array_element: Some(element.to_owned()),
        });
    }
    if meta.is_none() {
        if let Some(binding) = enums.get(type_name) {
            return Some(binding.clone());
        }
    }
    let scalar = match (type_name, meta) {
        ("bool", None) => Some(("bool", "BOOL", "BOOL")),
        // Godot's unqualified Variant scalar types use the full Variant
        // widths. Metadata narrows them only for typed ptrcall APIs.
        ("int", None) => Some(("i64", "I64", "I64")),
        ("int", Some("int8")) => Some(("i8", "I64", "I8")),
        ("int", Some("int16")) => Some(("i16", "I64", "I16")),
        ("int", Some("int32")) => Some(("i32", "I64", "I32")),
        ("int", Some("int64")) => Some(("i64", "I64", "I64")),
        ("int", Some("uint8")) => Some(("u8", "U64", "U8")),
        ("int", Some("uint16")) => Some(("u16", "U64", "U16")),
        ("int", Some("uint32")) => Some(("u32", "U64", "U32")),
        ("int", Some("uint64")) => Some(("u64", "U64", "U64")),
        ("int", Some("char32")) => Some(("char", "U64", "U32")),
        ("float", None) => Some(("f64", "F64", "F64")),
        ("float", Some("float")) => Some(("f32", "F64", "F32")),
        ("float", Some("double")) => Some(("f64", "F64", "F64")),
        _ => None,
    };
    if let Some((rust_type, value_type, ptrcall_type)) = scalar {
        return Some(ValueBinding {
            rust_type: rust_type.into(),
            value_type,
            ptrcall_type,
            class_name: None,
            typed_array_element: None,
        });
    }
    let builtin = match (type_name, meta) {
        ("String", None) => Some(("String", "STRING", "STRING")),
        ("StringName", None) => Some((
            "crate::string_name::StringName",
            "STRING_NAME",
            "STRING_NAME",
        )),
        ("NodePath", None) => Some(("crate::node_path::NodePath", "NODE_PATH", "NODE_PATH")),
        ("Vector2", None) => Some(("crate::math::Vector2", "VECTOR2", "VECTOR2")),
        ("Vector2i", None) => Some(("crate::math::Vector2i", "VECTOR2I", "VECTOR2I")),
        ("Vector3", None) => Some(("crate::math::Vector3", "VECTOR3", "VECTOR3")),
        ("Vector3i", None) => Some(("crate::math::Vector3i", "VECTOR3I", "VECTOR3I")),
        ("Vector4", None) => Some(("crate::math::Vector4", "VECTOR4", "VECTOR4")),
        ("Vector4i", None) => Some(("crate::math::Vector4i", "VECTOR4I", "VECTOR4I")),
        ("Rect2", None) => Some(("crate::math::Rect2", "RECT2", "RECT2")),
        ("Rect2i", None) => Some(("crate::math::Rect2i", "RECT2I", "RECT2I")),
        ("Quaternion", None) => Some(("crate::math::Quaternion", "QUATERNION", "QUATERNION")),
        ("Plane", None) => Some(("crate::math::Plane", "PLANE", "PLANE")),
        ("Transform2D", None) => Some(("crate::math::Transform2D", "TRANSFORM2D", "TRANSFORM2D")),
        ("AABB", None) => Some(("crate::math::Aabb", "AABB", "AABB")),
        ("Basis", None) => Some(("crate::math::Basis", "BASIS", "BASIS")),
        ("Transform3D", None) => Some(("crate::math::Transform3D", "TRANSFORM3D", "TRANSFORM3D")),
        ("Projection", None) => Some(("crate::math::Projection", "PROJECTION", "PROJECTION")),
        ("PackedByteArray", None) => Some((
            "crate::packed_array::PackedByteArray",
            "PACKED_BYTE_ARRAY",
            "PACKED_BYTE_ARRAY",
        )),
        ("PackedInt32Array", None) => Some((
            "crate::packed_array::PackedInt32Array",
            "PACKED_INT32_ARRAY",
            "PACKED_INT32_ARRAY",
        )),
        ("PackedInt64Array", None) => Some((
            "crate::packed_array::PackedInt64Array",
            "PACKED_INT64_ARRAY",
            "PACKED_INT64_ARRAY",
        )),
        ("PackedFloat32Array", None) => Some((
            "crate::packed_array::PackedFloat32Array",
            "PACKED_FLOAT32_ARRAY",
            "PACKED_FLOAT32_ARRAY",
        )),
        ("PackedFloat64Array", None) => Some((
            "crate::packed_array::PackedFloat64Array",
            "PACKED_FLOAT64_ARRAY",
            "PACKED_FLOAT64_ARRAY",
        )),
        ("PackedStringArray", None) => Some((
            "crate::packed_array::PackedStringArray",
            "PACKED_STRING_ARRAY",
            "PACKED_STRING_ARRAY",
        )),
        ("PackedVector2Array", None) => Some((
            "crate::packed_array::PackedVector2Array",
            "PACKED_VECTOR2_ARRAY",
            "PACKED_VECTOR2_ARRAY",
        )),
        ("PackedVector3Array", None) => Some((
            "crate::packed_array::PackedVector3Array",
            "PACKED_VECTOR3_ARRAY",
            "PACKED_VECTOR3_ARRAY",
        )),
        ("PackedColorArray", None) => Some((
            "crate::packed_array::PackedColorArray",
            "PACKED_COLOR_ARRAY",
            "PACKED_COLOR_ARRAY",
        )),
        ("PackedVector4Array", None) => Some((
            "crate::packed_array::PackedVector4Array",
            "PACKED_VECTOR4_ARRAY",
            "PACKED_VECTOR4_ARRAY",
        )),
        ("Nil" | "Variant", None) => Some(("crate::variant::Variant", "VARIANT", "VARIANT")),
        ("Array", None) => Some(("crate::variant::Array", "ARRAY", "ARRAY")),
        ("Dictionary", None) => Some(("crate::variant::Dictionary", "DICTIONARY", "DICTIONARY")),
        ("Callable", None) => Some(("crate::callable::Callable", "CALLABLE", "CALLABLE")),
        ("Signal", None) => Some(("crate::signal::Signal", "SIGNAL", "SIGNAL")),
        ("Color", None) => Some(("crate::math::Color", "COLOR", "COLOR")),
        ("RID", None) => Some(("crate::rid::Rid", "RID", "RID")),
        _ => None,
    };
    if let Some((rust_type, value_type, ptrcall_type)) = builtin {
        return Some(ValueBinding {
            rust_type: rust_type.into(),
            value_type,
            ptrcall_type,
            class_name: None,
            typed_array_element: None,
        });
    }
    (matches!(meta, None | Some("required")) && classes.contains_key(type_name)).then(|| {
        ValueBinding {
            rust_type: format!("super::ObjectRef<super::{type_name}>"),
            value_type: "OBJECT_ID",
            ptrcall_type: "OBJECT",
            class_name: Some(type_name.to_owned()),
            typed_array_element: None,
        }
    })
}

fn method_id(
    class: &ApiClass,
    method: &ApiMethod,
    arguments: &[ArgumentBinding],
    return_value: &ValueBinding,
) -> u64 {
    let mut digest = Sha256::new();
    digest.update(b"godot-rust-engine-method-v1\0");
    if method.is_static {
        digest.update(b"static\0");
    }
    if method.is_vararg {
        digest.update(b"vararg\0");
    }
    hash_text(&mut digest, &class.name);
    hash_text(&mut digest, &method.name);
    digest.update(
        method
            .hash
            .expect("bound methods have hashes")
            .to_le_bytes(),
    );
    for argument in arguments {
        hash_value(&mut digest, &argument.value);
    }
    hash_value(&mut digest, return_value);
    let bytes: [u8; 8] = digest.finalize()[..8]
        .try_into()
        .expect("SHA-256 has at least eight bytes");
    let id = u64::from_le_bytes(bytes);
    if id == 0 { 1 } else { id }
}

fn hash_value(digest: &mut Sha256, value: &ValueBinding) {
    hash_text(digest, value.value_type);
    hash_text(digest, value.ptrcall_type);
    hash_text(digest, value.class_name.as_deref().unwrap_or(""));
    hash_text(digest, value.typed_array_element.as_deref().unwrap_or(""));
}

fn hash_text(digest: &mut Sha256, text: &str) {
    digest.update(text.as_bytes());
    digest.update([0]);
}

fn validate_identifier(value: &str, kind: &str) -> Result<(), EngineApiGenerationError> {
    let mut characters = value.chars();
    let valid_start = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    if !valid_start
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(EngineApiGenerationError::new(format!(
            "{kind} `{value}` is not a valid Rust identifier"
        )));
    }
    Ok(())
}

fn rust_module_identifier(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    let mut result = String::with_capacity(value.len() + 4);
    for (index, character) in characters.iter().copied().enumerate() {
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        if character.is_ascii_uppercase()
            && index != 0
            && (previous.is_some_and(|value| value.is_ascii_lowercase() || value.is_ascii_digit())
                || (previous.is_some_and(|value| value.is_ascii_uppercase())
                    && next.is_some_and(|value| value.is_ascii_lowercase())))
        {
            result.push('_');
        }
        result.push(character.to_ascii_lowercase());
    }
    rust_identifier(&result)
}

fn rust_prefixed_identifier(prefix: &str, value: &str) -> String {
    let identifier = rust_module_identifier(value);
    format!(
        "{prefix}{}",
        identifier.strip_prefix("r#").unwrap_or(&identifier)
    )
}

fn rust_identifier(value: &str) -> String {
    if value == "self" || value == "Self" || value == "super" || value == "crate" {
        return format!("{value}_");
    }
    if matches!(
        value,
        "as" | "async"
            | "await"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "union"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    ) {
        format!("r#{value}")
    } else {
        value.to_owned()
    }
}

fn rust_argument_identifier(value: &str) -> String {
    rust_identifier(&value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_methods_generate_base_checked_override_traits() {
        let class = ApiClass {
            name: "CanvasItem".into(),
            inherits: Some("Node".into()),
            api_type: "core".into(),
            is_refcounted: false,
            is_instantiable: true,
            methods: Vec::new(),
            properties: Vec::new(),
            signals: Vec::new(),
            enums: Vec::new(),
            constants: Vec::new(),
        };
        let method = ApiMethod {
            name: "_accept_point".into(),
            is_virtual: true,
            is_const: false,
            is_static: false,
            is_vararg: false,
            is_required: false,
            hash: Some(123),
            hash_compatibility: Vec::new(),
            return_value: Some(ApiReturnValue {
                r#type: "bool".into(),
                meta: None,
            }),
            arguments: vec![ApiArgument {
                name: "point".into(),
                r#type: "Vector2".into(),
                meta: None,
                default_value: None,
            }],
        };
        let classes = HashMap::from([("CanvasItem", &class)]);
        let binding = bind_virtual_method(&class, &method, &classes, &HashMap::new())
            .expect("valid method")
            .expect("supported types");
        let bindings = [binding];

        let mut output = String::new();
        emit_virtual_traits(&mut output, &[&class], &bindings);
        emit_native_virtual_registrars(&mut output, &bindings);

        assert!(output.contains(
            "pub trait CanvasItemVirtual: crate::script::ScriptInherits<super::CanvasItem>"
        ));
        assert!(
            output.contains("fn _accept_point(&mut self, point: crate::math::Vector2) -> bool")
        );
        assert!(output.contains("Godot virtual method `CanvasItem._accept_point`"));
        assert!(output.contains("pub mod native_virtual"));
        assert!(output.contains("pub mod canvas_item"));
        assert!(output.contains("registrar.__virtual_method::<"));
        assert!(output.contains("\"CanvasItem\", \"_accept_point\", 123_u32, 1_u32"));
    }

    #[test]
    fn virtual_methods_report_unsafe_internal_pointer_types() {
        let class = ApiClass {
            name: "ExtensionServer".into(),
            inherits: Some("Object".into()),
            api_type: "core".into(),
            is_refcounted: false,
            is_instantiable: true,
            methods: Vec::new(),
            properties: Vec::new(),
            signals: Vec::new(),
            enums: Vec::new(),
            constants: Vec::new(),
        };
        let method = ApiMethod {
            name: "_consume_internal_state".into(),
            is_virtual: true,
            is_const: false,
            is_static: false,
            is_vararg: false,
            is_required: false,
            hash: None,
            hash_compatibility: Vec::new(),
            return_value: None,
            arguments: vec![ApiArgument {
                name: "state".into(),
                r#type: "const void*".into(),
                meta: None,
                default_value: None,
            }],
        };
        let classes = HashMap::from([("ExtensionServer", &class)]);
        let unsupported = match bind_virtual_method(&class, &method, &classes, &HashMap::new())
            .expect("valid method")
        {
            Ok(_) => panic!("raw pointers must not enter the safe script ABI"),
            Err(unsupported) => unsupported,
        };

        assert_eq!(unsupported, BTreeSet::from(["const void*".to_owned()]));
    }

    #[test]
    fn exact_scalar_metadata_controls_rust_and_ptrcall_types() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let value = bind_value("int", Some("uint32"), &classes, &enums).expect("uint32");
        assert_eq!(value.rust_type, "u32");
        assert_eq!(value.value_type, "U64");
        assert_eq!(value.ptrcall_type, "U32");
        let character = bind_value("int", Some("char32"), &classes, &enums).expect("char32");
        assert_eq!(character.rust_type, "char");
        assert_eq!(character.value_type, "U64");
        assert_eq!(character.ptrcall_type, "U32");
        let variant_integer = bind_value("int", None, &classes, &enums).expect("Variant int");
        assert_eq!(variant_integer.rust_type, "i64");
        assert_eq!(variant_integer.value_type, "I64");
        assert_eq!(variant_integer.ptrcall_type, "I64");
        let variant_float = bind_value("float", None, &classes, &enums).expect("Variant float");
        assert_eq!(variant_float.rust_type, "f64");
        assert_eq!(variant_float.value_type, "F64");
        assert_eq!(variant_float.ptrcall_type, "F64");
    }

    #[test]
    fn common_math_builtins_use_exact_ptrcall_storage() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        for (godot_type, rust_type, abi_type) in [
            ("Vector2", "crate::math::Vector2", "VECTOR2"),
            ("Vector2i", "crate::math::Vector2i", "VECTOR2I"),
            ("Vector3", "crate::math::Vector3", "VECTOR3"),
            ("Vector3i", "crate::math::Vector3i", "VECTOR3I"),
            ("Vector4", "crate::math::Vector4", "VECTOR4"),
            ("Vector4i", "crate::math::Vector4i", "VECTOR4I"),
            ("Rect2", "crate::math::Rect2", "RECT2"),
            ("Rect2i", "crate::math::Rect2i", "RECT2I"),
            ("Quaternion", "crate::math::Quaternion", "QUATERNION"),
            ("Plane", "crate::math::Plane", "PLANE"),
            ("Transform2D", "crate::math::Transform2D", "TRANSFORM2D"),
            ("AABB", "crate::math::Aabb", "AABB"),
            ("Basis", "crate::math::Basis", "BASIS"),
            ("Transform3D", "crate::math::Transform3D", "TRANSFORM3D"),
            ("Projection", "crate::math::Projection", "PROJECTION"),
            ("Color", "crate::math::Color", "COLOR"),
        ] {
            let value = bind_value(godot_type, None, &classes, &enums).expect("supported builtin");
            assert_eq!(value.rust_type, rust_type);
            assert_eq!(value.value_type, abi_type);
            assert_eq!(value.ptrcall_type, abi_type);
            assert!(value.class_name.is_none());
        }
    }

    #[test]
    fn string_arguments_borrow_and_string_returns_are_owned() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let argument = bind_argument(
            &ApiArgument {
                name: "text".into(),
                r#type: "String".into(),
                meta: None,
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("String argument");
        assert_eq!(argument.rust_type, "&str");
        assert_eq!(argument.value_type, "STRING");
        assert_eq!(argument.ptrcall_type, "STRING");

        let returned = bind_return(
            Some(&ApiReturnValue {
                r#type: "String".into(),
                meta: None,
            }),
            &classes,
            &enums,
        )
        .expect("String return");
        assert_eq!(returned.rust_type, "String");
        assert_eq!(returned.value_type, "STRING");
        assert_eq!(returned.ptrcall_type, "STRING");
    }

    #[test]
    fn string_name_arguments_are_ergonomic_and_returns_keep_their_type() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let argument = bind_argument(
            &ApiArgument {
                name: "name".into(),
                r#type: "StringName".into(),
                meta: None,
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("StringName argument");
        assert_eq!(argument.rust_type, "&str");
        assert_eq!(argument.value_type, "STRING_NAME");
        assert_eq!(argument.ptrcall_type, "STRING_NAME");

        let returned = bind_return(
            Some(&ApiReturnValue {
                r#type: "StringName".into(),
                meta: None,
            }),
            &classes,
            &enums,
        )
        .expect("StringName return");
        assert_eq!(returned.rust_type, "crate::string_name::StringName");
        assert_eq!(returned.value_type, "STRING_NAME");
        assert_eq!(returned.ptrcall_type, "STRING_NAME");
    }

    #[test]
    fn node_path_arguments_are_ergonomic_and_returns_keep_their_type() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let argument = bind_argument(
            &ApiArgument {
                name: "path".into(),
                r#type: "NodePath".into(),
                meta: None,
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("NodePath argument");
        assert_eq!(argument.rust_type, "&str");
        assert_eq!(argument.value_type, "NODE_PATH");
        assert_eq!(argument.ptrcall_type, "NODE_PATH");

        let returned = bind_return(
            Some(&ApiReturnValue {
                r#type: "NodePath".into(),
                meta: None,
            }),
            &classes,
            &enums,
        )
        .expect("NodePath return");
        assert_eq!(returned.rust_type, "crate::node_path::NodePath");
        assert_eq!(returned.value_type, "NODE_PATH");
        assert_eq!(returned.ptrcall_type, "NODE_PATH");
    }

    #[test]
    fn packed_arrays_generate_borrowed_arguments_and_owned_returns() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        for (godot_type, rust_type, abi_type) in [
            ("PackedByteArray", "PackedByteArray", "PACKED_BYTE_ARRAY"),
            ("PackedInt32Array", "PackedInt32Array", "PACKED_INT32_ARRAY"),
            ("PackedInt64Array", "PackedInt64Array", "PACKED_INT64_ARRAY"),
            (
                "PackedFloat32Array",
                "PackedFloat32Array",
                "PACKED_FLOAT32_ARRAY",
            ),
            (
                "PackedFloat64Array",
                "PackedFloat64Array",
                "PACKED_FLOAT64_ARRAY",
            ),
            (
                "PackedStringArray",
                "PackedStringArray",
                "PACKED_STRING_ARRAY",
            ),
            (
                "PackedVector2Array",
                "PackedVector2Array",
                "PACKED_VECTOR2_ARRAY",
            ),
            (
                "PackedVector3Array",
                "PackedVector3Array",
                "PACKED_VECTOR3_ARRAY",
            ),
            ("PackedColorArray", "PackedColorArray", "PACKED_COLOR_ARRAY"),
            (
                "PackedVector4Array",
                "PackedVector4Array",
                "PACKED_VECTOR4_ARRAY",
            ),
        ] {
            let argument = bind_argument(
                &ApiArgument {
                    name: "values".into(),
                    r#type: godot_type.into(),
                    meta: None,
                    default_value: None,
                },
                &classes,
                &enums,
            )
            .expect("packed-array argument");
            assert_eq!(
                argument.rust_type,
                format!("&crate::packed_array::{rust_type}")
            );
            assert_eq!(argument.value_type, abi_type);
            assert_eq!(argument.ptrcall_type, abi_type);

            let returned = bind_return(
                Some(&ApiReturnValue {
                    r#type: godot_type.into(),
                    meta: None,
                }),
                &classes,
                &enums,
            )
            .expect("packed-array return");
            assert_eq!(
                returned.rust_type,
                format!("crate::packed_array::{rust_type}")
            );
            assert_eq!(returned.value_type, abi_type);
            assert_eq!(returned.ptrcall_type, abi_type);
        }
    }

    #[test]
    fn callable_arguments_are_borrowed_and_returns_are_owned() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let argument = bind_argument(
            &ApiArgument {
                name: "callback".into(),
                r#type: "Callable".into(),
                meta: None,
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("Callable argument");
        assert_eq!(argument.rust_type, "&crate::callable::Callable");
        assert_eq!(argument.value_type, "CALLABLE");
        assert_eq!(argument.ptrcall_type, "CALLABLE");

        let returned = bind_return(
            Some(&ApiReturnValue {
                r#type: "Callable".into(),
                meta: None,
            }),
            &classes,
            &enums,
        )
        .expect("Callable return");
        assert_eq!(returned.rust_type, "crate::callable::Callable");
        assert_eq!(returned.value_type, "CALLABLE");
        assert_eq!(returned.ptrcall_type, "CALLABLE");
    }

    #[test]
    fn signal_arguments_are_borrowed_and_returns_are_owned() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let argument = bind_argument(
            &ApiArgument {
                name: "signal".into(),
                r#type: "Signal".into(),
                meta: None,
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("Signal argument");
        assert_eq!(argument.rust_type, "&crate::signal::Signal");
        assert_eq!(argument.value_type, "SIGNAL");
        assert_eq!(argument.ptrcall_type, "SIGNAL");

        let returned = bind_return(
            Some(&ApiReturnValue {
                r#type: "Signal".into(),
                meta: None,
            }),
            &classes,
            &enums,
        )
        .expect("Signal return");
        assert_eq!(returned.rust_type, "crate::signal::Signal");
        assert_eq!(returned.value_type, "SIGNAL");
        assert_eq!(returned.ptrcall_type, "SIGNAL");
    }

    #[test]
    fn typed_arrays_preserve_builtin_and_refcounted_element_types() {
        let resource = ApiClass {
            name: "Resource".into(),
            inherits: Some("RefCounted".into()),
            api_type: "core".into(),
            is_refcounted: true,
            is_instantiable: true,
            methods: Vec::new(),
            properties: Vec::new(),
            signals: Vec::new(),
            enums: Vec::new(),
            constants: Vec::new(),
        };
        let classes = HashMap::from([("Resource", &resource)]);
        let enums = HashMap::new();

        let builtin = bind_argument(
            &ApiArgument {
                name: "names".into(),
                r#type: "typedarray::StringName".into(),
                meta: None,
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("typed builtin Array");
        assert_eq!(
            builtin.rust_type,
            "&crate::variant::Array<crate::string_name::StringName>"
        );
        assert_eq!(builtin.value_type, "ARRAY");
        assert_eq!(builtin.ptrcall_type, "ARRAY");
        assert_eq!(builtin.typed_array_element.as_deref(), Some("StringName"));

        let returned = bind_return(
            Some(&ApiReturnValue {
                r#type: "typedarray::Resource".into(),
                meta: None,
            }),
            &classes,
            &enums,
        )
        .expect("typed RefCounted Array");
        assert_eq!(
            returned.rust_type,
            "crate::variant::Array<super::GodotRef<super::Resource>>"
        );
        assert_eq!(returned.typed_array_element.as_deref(), Some("Resource"));

        let binding = MethodBinding {
            class: &resource,
            method: &ApiMethod {
                name: "take_names".into(),
                is_const: false,
                is_static: false,
                is_vararg: false,
                is_virtual: false,
                is_required: false,
                hash: Some(1),
                hash_compatibility: Vec::new(),
                arguments: Vec::new(),
                return_value: None,
            },
            id: 1,
            arguments: vec![ArgumentBinding {
                rust_name: "names".into(),
                value: builtin,
            }],
            return_value: ValueBinding {
                rust_type: "()".into(),
                value_type: "NIL",
                ptrcall_type: "VOID",
                class_name: None,
                typed_array_element: None,
            },
        };
        let mut output = String::new();
        emit_contract(&mut output, &binding);
        assert!(output.contains("ABI_GODOT_VALUE_TYPED_ARRAY"));
        assert!(output.contains("from_static(\"StringName\")"));
    }

    #[test]
    fn required_object_arguments_are_non_optional_object_references() {
        let class = ApiClass {
            name: "Node".into(),
            inherits: Some("Object".into()),
            api_type: "core".into(),
            is_refcounted: false,
            is_instantiable: true,
            methods: Vec::new(),
            properties: Vec::new(),
            signals: Vec::new(),
            enums: Vec::new(),
            constants: Vec::new(),
        };
        let classes = HashMap::from([("Node", &class)]);
        let enums = HashMap::new();
        let argument = bind_argument(
            &ApiArgument {
                name: "node".into(),
                r#type: "Node".into(),
                meta: Some("required".into()),
                default_value: None,
            },
            &classes,
            &enums,
        )
        .expect("required object argument");

        assert_eq!(argument.rust_type, "super::ObjectRef<super::Node>");
        assert_eq!(argument.value_type, "OBJECT_ID");
        assert_eq!(argument.ptrcall_type, "OBJECT");
        assert_eq!(argument.class_name.as_deref(), Some("Node"));
    }

    #[test]
    fn rid_values_use_the_exact_opaque_ptrcall_storage() {
        let classes = HashMap::new();
        let enums = HashMap::new();
        let value = bind_value("RID", None, &classes, &enums).expect("RID binding");
        assert_eq!(value.rust_type, "crate::rid::Rid");
        assert_eq!(value.value_type, "RID");
        assert_eq!(value.ptrcall_type, "RID");
        assert!(value.class_name.is_none());
    }

    #[test]
    fn official_enums_and_bitfields_generate_distinct_integer_contracts() {
        let enum_values = [ApiEnumValue {
            name: "PROCESS_MODE_ALWAYS".into(),
            value: 3,
        }];
        let bitfield_values = [ApiEnumValue {
            name: "FLAG_PROCESS_THREAD_MESSAGES".into(),
            value: 1,
        }];
        let mut definitions = BTreeMap::new();
        let mut values = HashMap::new();
        let mut rust_types = BTreeSet::new();
        register_enum(
            &mut definitions,
            &mut values,
            &mut rust_types,
            EnumSource {
                module: "node",
                owner: Some("Node"),
                name: "ProcessMode",
                is_bitfield: false,
                values: &enum_values,
            },
        )
        .expect("enum");
        register_enum(
            &mut definitions,
            &mut values,
            &mut rust_types,
            EnumSource {
                module: "node",
                owner: Some("Node"),
                name: "ProcessThreadMessages",
                is_bitfield: true,
                values: &bitfield_values,
            },
        )
        .expect("bitfield");

        let enum_binding = values.get("enum::Node.ProcessMode").expect("enum binding");
        assert_eq!(enum_binding.rust_type, "node::ProcessMode");
        assert_eq!(enum_binding.value_type, "I64");
        assert_eq!(enum_binding.ptrcall_type, "I64");
        let bitfield_binding = values
            .get("bitfield::Node.ProcessThreadMessages")
            .expect("bitfield binding");
        assert_eq!(bitfield_binding.rust_type, "node::ProcessThreadMessages");
        assert_eq!(bitfield_binding.value_type, "U64");
        assert_eq!(bitfield_binding.ptrcall_type, "U64");

        let mut output = String::new();
        emit_enums(&mut output, &definitions);
        assert!(output.contains("define_godot_enum!"));
        assert!(output.contains("define_godot_bitfield!"));
        assert!(output.contains("pub struct ProcessMode"));
        assert!(output.contains("pub struct ProcessThreadMessages"));
    }

    #[test]
    fn rust_keywords_are_escaped_without_changing_godot_names() {
        assert_eq!(rust_identifier("type"), "r#type");
        assert_eq!(rust_identifier("self"), "self_");
        assert_eq!(rust_identifier("set_process"), "set_process");
        assert_eq!(rust_module_identifier("codeLens"), "code_lens");
        assert_eq!(
            rust_module_identifier("willSaveWaitUntil"),
            "will_save_wait_until"
        );
    }

    #[test]
    fn uppercase_official_argument_names_become_rust_snake_case() {
        assert_eq!(rust_argument_identifier("body_A"), "body_a");
        assert_eq!(rust_argument_identifier("RID"), "rid");
        assert_eq!(rust_argument_identifier("type"), "r#type");
        assert_eq!(rust_module_identifier("AStarGrid2D"), "a_star_grid2_d");
        assert_eq!(rust_module_identifier("AESContext"), "aes_context");
    }
}
