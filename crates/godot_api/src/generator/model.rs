use serde::{Deserialize, Serialize};

/// Root of Godot's official `extension_api.json`.
///
/// Every object rejects unknown fields. A new or renamed official schema field
/// must therefore be reviewed instead of being silently discarded.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionApi {
    /// Engine build that emitted the API.
    pub header: ApiHeader,
    /// ABI size tables for real/precision configurations.
    pub builtin_class_sizes: Vec<BuiltinClassSizeConfiguration>,
    /// ABI member offsets for builtin value types.
    pub builtin_class_member_offsets: Vec<BuiltinClassOffsetConfiguration>,
    /// Global integer constants.
    pub global_constants: Vec<GlobalConstant>,
    /// Global enums and bitfields.
    pub global_enums: Vec<ApiEnum>,
    /// Global utility functions.
    pub utility_functions: Vec<UtilityFunction>,
    /// Builtin types such as `String`, `Vector2`, and `Variant`.
    pub builtin_classes: Vec<BuiltinClass>,
    /// Engine Object-derived classes.
    pub classes: Vec<ApiClass>,
    /// Registered singleton descriptors.
    pub singletons: Vec<ApiSingleton>,
    /// Native structure descriptors used by pointer arguments.
    pub native_structures: Vec<NativeStructure>,
}

impl ExtensionApi {
    /// Returns a complete count inventory of every modeled API category.
    #[must_use]
    pub fn inventory(&self) -> ApiInventory {
        ApiInventory {
            builtin_size_configuration_count: self.builtin_class_sizes.len(),
            builtin_size_count: self
                .builtin_class_sizes
                .iter()
                .map(|configuration| configuration.sizes.len())
                .sum(),
            builtin_offset_configuration_count: self.builtin_class_member_offsets.len(),
            builtin_offset_count: self
                .builtin_class_member_offsets
                .iter()
                .flat_map(|configuration| &configuration.classes)
                .map(|class| class.members.len())
                .sum(),
            global_constant_count: self.global_constants.len(),
            global_enum_count: self.global_enums.len(),
            utility_function_count: self.utility_functions.len(),
            builtin_class_count: self.builtin_classes.len(),
            builtin_constructor_count: self
                .builtin_classes
                .iter()
                .map(|class| class.constructors.len())
                .sum(),
            builtin_operator_count: self
                .builtin_classes
                .iter()
                .map(|class| class.operators.len())
                .sum(),
            builtin_method_count: self
                .builtin_classes
                .iter()
                .map(|class| class.methods.len())
                .sum(),
            builtin_member_count: self
                .builtin_classes
                .iter()
                .map(|class| class.members.len())
                .sum(),
            builtin_constant_count: self
                .builtin_classes
                .iter()
                .map(|class| class.constants.len())
                .sum(),
            builtin_enum_count: self
                .builtin_classes
                .iter()
                .map(|class| class.enums.len())
                .sum(),
            engine_class_count: self.classes.len(),
            engine_method_count: self.classes.iter().map(|class| class.methods.len()).sum(),
            engine_property_count: self
                .classes
                .iter()
                .map(|class| class.properties.len())
                .sum(),
            engine_signal_count: self.classes.iter().map(|class| class.signals.len()).sum(),
            engine_enum_count: self.classes.iter().map(|class| class.enums.len()).sum(),
            engine_constant_count: self.classes.iter().map(|class| class.constants.len()).sum(),
            singleton_count: self.singletons.len(),
            native_structure_count: self.native_structures.len(),
        }
    }
}

/// Count inventory proving which official API categories were parsed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiInventory {
    pub builtin_size_configuration_count: usize,
    pub builtin_size_count: usize,
    pub builtin_offset_configuration_count: usize,
    pub builtin_offset_count: usize,
    pub global_constant_count: usize,
    pub global_enum_count: usize,
    pub utility_function_count: usize,
    pub builtin_class_count: usize,
    pub builtin_constructor_count: usize,
    pub builtin_operator_count: usize,
    pub builtin_method_count: usize,
    pub builtin_member_count: usize,
    pub builtin_constant_count: usize,
    pub builtin_enum_count: usize,
    pub engine_class_count: usize,
    pub engine_method_count: usize,
    pub engine_property_count: usize,
    pub engine_signal_count: usize,
    pub engine_enum_count: usize,
    pub engine_constant_count: usize,
    pub singleton_count: usize,
    pub native_structure_count: usize,
}

impl ApiInventory {
    /// Returns whether all categories that exist in every supported official
    /// API dump contain data. Global constants are excluded because they are
    /// intentionally empty before Godot 4.7.
    #[must_use]
    pub fn has_required_surface(self) -> bool {
        self.builtin_size_configuration_count == 4
            && self.builtin_size_count > 0
            && self.builtin_offset_configuration_count == 4
            && self.builtin_offset_count > 0
            && self.global_enum_count > 0
            && self.utility_function_count > 0
            && self.builtin_class_count > 0
            && self.builtin_constructor_count > 0
            && self.builtin_operator_count > 0
            && self.builtin_method_count > 0
            && self.builtin_member_count > 0
            && self.builtin_constant_count > 0
            && self.builtin_enum_count > 0
            && self.engine_class_count > 0
            && self.engine_method_count > 0
            && self.engine_property_count > 0
            && self.engine_signal_count > 0
            && self.engine_enum_count > 0
            && self.engine_constant_count > 0
            && self.singleton_count > 0
            && self.native_structure_count > 0
    }
}

/// Engine version metadata embedded in the official dump.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiHeader {
    pub version_major: u32,
    pub version_minor: u32,
    pub version_patch: u32,
    pub version_status: String,
    pub version_build: String,
    pub version_full_name: String,
    /// Real-number precision, emitted by Godot 4.5 and newer.
    #[serde(default)]
    pub precision: Option<String>,
}

/// One ABI build configuration such as `double_64`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinClassSizeConfiguration {
    pub build_configuration: String,
    pub sizes: Vec<BuiltinClassSize>,
}

/// Size of one builtin under a build configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinClassSize {
    pub name: String,
    pub size: u64,
}

/// Member-offset table for one ABI build configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinClassOffsetConfiguration {
    pub build_configuration: String,
    pub classes: Vec<BuiltinClassOffsets>,
}

/// Member offsets for one builtin value type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinClassOffsets {
    pub name: String,
    pub members: Vec<BuiltinMemberOffset>,
}

/// Offset and scalar representation of one builtin member.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinMemberOffset {
    pub member: String,
    pub offset: u64,
    pub meta: String,
}

/// One global integer constant.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConstant {
    pub name: String,
    pub value: i64,
    pub is_bitfield: bool,
}

/// One Godot enum or bitfield.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiEnum {
    pub name: String,
    pub is_bitfield: bool,
    pub values: Vec<ApiEnumValue>,
}

/// One named enum value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiEnumValue {
    pub name: String,
    pub value: i64,
}

/// One global utility function.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UtilityFunction {
    pub name: String,
    pub category: String,
    pub is_vararg: bool,
    pub hash: u64,
    #[serde(default)]
    pub return_type: Option<String>,
    #[serde(default)]
    pub arguments: Vec<ApiArgument>,
}

/// Builtin type such as `String`, `Vector2`, or `Variant`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinClass {
    pub name: String,
    pub has_destructor: bool,
    pub is_keyed: bool,
    pub constructors: Vec<BuiltinConstructor>,
    pub operators: Vec<BuiltinOperator>,
    #[serde(default)]
    pub indexing_return_type: Option<String>,
    #[serde(default)]
    pub methods: Vec<BuiltinMethod>,
    #[serde(default)]
    pub members: Vec<BuiltinMember>,
    #[serde(default)]
    pub constants: Vec<BuiltinConstant>,
    #[serde(default)]
    pub enums: Vec<BuiltinEnum>,
}

/// Indexed constructor exposed by one builtin type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinConstructor {
    pub index: u32,
    #[serde(default)]
    pub arguments: Vec<ApiArgument>,
}

/// Operator exposed by one builtin type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinOperator {
    pub name: String,
    pub return_type: String,
    #[serde(default)]
    pub right_type: Option<String>,
}

/// Method exposed by one builtin type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinMethod {
    pub name: String,
    pub is_const: bool,
    pub is_static: bool,
    pub is_vararg: bool,
    pub hash: u64,
    #[serde(default)]
    pub hash_compatibility: Vec<u64>,
    #[serde(default)]
    pub return_type: Option<String>,
    #[serde(default)]
    pub arguments: Vec<ApiArgument>,
}

/// Named field of a builtin value type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinMember {
    pub name: String,
    pub r#type: String,
}

/// Named constant of a builtin value type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinConstant {
    pub name: String,
    pub r#type: String,
    pub value: String,
}

/// Enum nested in a builtin value type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinEnum {
    pub name: String,
    pub values: Vec<ApiEnumValue>,
}

/// Object-derived Godot class.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiClass {
    pub name: String,
    #[serde(default)]
    pub inherits: Option<String>,
    pub api_type: String,
    pub is_refcounted: bool,
    pub is_instantiable: bool,
    #[serde(default)]
    pub methods: Vec<ApiMethod>,
    #[serde(default)]
    pub properties: Vec<ApiProperty>,
    #[serde(default)]
    pub signals: Vec<ApiSignal>,
    #[serde(default)]
    pub enums: Vec<ApiEnum>,
    #[serde(default)]
    pub constants: Vec<ApiClassConstant>,
}

/// Method exposed by an Object-derived class.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiMethod {
    pub name: String,
    pub is_virtual: bool,
    pub is_const: bool,
    pub is_static: bool,
    pub is_vararg: bool,
    #[serde(default)]
    pub is_required: bool,
    #[serde(default)]
    pub hash: Option<u64>,
    #[serde(default)]
    pub hash_compatibility: Vec<u64>,
    #[serde(default)]
    pub return_value: Option<ApiReturnValue>,
    #[serde(default)]
    pub arguments: Vec<ApiArgument>,
}

/// One function or method argument.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiArgument {
    pub name: String,
    pub r#type: String,
    #[serde(default)]
    pub meta: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
}

/// One class method return value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiReturnValue {
    pub r#type: String,
    #[serde(default)]
    pub meta: Option<String>,
}

/// Property exposed by an Object-derived class.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiProperty {
    pub name: String,
    pub r#type: String,
    pub getter: String,
    #[serde(default)]
    pub setter: Option<String>,
    #[serde(default)]
    pub index: Option<i64>,
}

/// Signal exposed by an Object-derived class.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSignal {
    pub name: String,
    #[serde(default)]
    pub arguments: Vec<ApiArgument>,
}

/// Named integer constant exposed by an Object-derived class.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiClassConstant {
    pub name: String,
    pub value: i64,
}

/// Registered engine singleton.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSingleton {
    pub name: String,
    pub r#type: String,
}

/// Native C structure referenced by pointer-shaped API types.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeStructure {
    pub name: String,
    pub format: String,
}
