use godot_rs_codegen::{
    ApiCoverageDisposition, ApiCoverageEntry, ExpectedApiVersion, ExtensionApi, analyze_engine_api,
    diff_api, generate_api_snapshot, generate_engine_api, load_api, validate_api,
    verify_engine_api_coverage,
};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_extension_api.json")
}

#[test]
fn official_model_fixture_validates() {
    let loaded = load_api(&fixture()).expect("fixture should parse");
    let issues = validate_api(&loaded.api, ExpectedApiVersion { major: 4, minor: 4 });
    assert!(issues.is_empty(), "{issues:#?}");
}

#[test]
fn wrong_minor_is_rejected() {
    let loaded = load_api(&fixture()).expect("fixture should parse");
    let issues = validate_api(&loaded.api, ExpectedApiVersion { major: 4, minor: 5 });
    assert!(
        issues
            .iter()
            .any(|issue| issue.path == "header" && issue.message.contains("4.5"))
    );
}

#[test]
fn snapshot_generation_is_deterministic() {
    let loaded = load_api(&fixture()).expect("fixture should parse");
    let first = generate_api_snapshot(&loaded.api, &loaded.sha256);
    let second = generate_api_snapshot(&loaded.api, &loaded.sha256);
    assert_eq!(first, second);
    assert!(first.contains("pub const ENGINE_CLASS_COUNT: usize = 3;"));
    assert!(first.contains("name: \"ScriptLanguageExtension\""));
    assert!(first.contains(
        "name: \"get_instance_id\", hash: Some(3905245786), call_kind: MethodCallKind::Ptrcall"
    ));
    assert!(
        first.contains(
            "BuiltinSizeSummary { configuration: \"double_64\", name: \"bool\", size: 1 }"
        )
    );
}

#[test]
fn engine_api_generation_is_deterministic_and_typed() {
    let loaded = load_api(&fixture()).expect("fixture should parse");
    let first = generate_engine_api(&loaded.api, &loaded.sha256).expect("engine API");
    let second = generate_engine_api(&loaded.api, &loaded.sha256).expect("engine API");

    assert_eq!(first, second);
    assert!(first.contains("impl super::Inherits<super::Object> for super::ScriptExtension {}"));
    assert!(first.contains("pub trait ObjectApi"));
    assert!(first.contains("fn get_instance_id(&self) -> crate::error::EngineResult<u64>;"));
    assert!(first.contains("R::Class: super::Inherits<super::Object>"));
    assert!(first.contains("let receiver = super::EngineObject::__engine_object(self)?;"));
    assert!(first.contains("ptrcall_type: crate::abi::AbiPtrcallType::U64"));
    assert!(first.contains("value_type: crate::abi::AbiValueType::U64"));
}

#[test]
fn full_public_surface_is_generated_and_covered_entry_by_entry() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    value["global_constants"] = serde_json::json!([{
        "name": "FIXTURE_CONSTANT",
        "value": 42,
        "is_bitfield": false
    }]);
    value["utility_functions"] = serde_json::json!([{
        "name": "fixture_utility",
        "category": "general",
        "is_vararg": false,
        "hash": 7,
        "return_type": "float",
        "arguments": [{"name": "value", "type": "float"}]
    }]);
    value["builtin_classes"] = serde_json::json!([{
        "name": "String",
        "has_destructor": true,
        "is_keyed": true,
        "constructors": [
            {"index": 0},
            {"index": 1, "arguments": [{"name": "from", "type": "String"}]}
        ],
        "operators": [{
            "name": "+",
            "return_type": "String",
            "right_type": "String"
        }],
        "indexing_return_type": "String",
        "methods": [{
            "name": "length",
            "is_const": true,
            "is_static": false,
            "is_vararg": false,
            "hash": 8,
            "return_type": "int"
        }],
        "members": [{"name": "fixture_member", "type": "int"}],
        "constants": [{"name": "FIXTURE_TEXT", "type": "String", "value": "\"fixture\""}],
        "enums": []
    }]);
    value["classes"][0]["signals"] = serde_json::json!([{
        "name": "fixture_changed",
        "arguments": [{"name": "revision", "type": "int"}]
    }]);
    value["classes"][0]["constants"] =
        serde_json::json!([{"name": "FIXTURE_NOTIFICATION", "value": 100}]);
    value["classes"]
        .as_array_mut()
        .expect("classes should be an array")
        .push(serde_json::json!({
            "name": "FixtureService",
            "inherits": "Object",
            "is_refcounted": false,
            "is_instantiable": false,
            "api_type": "core"
        }));
    value["singletons"] = serde_json::json!([{"name": "FixtureService", "type": "FixtureService"}]);

    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");
    let generated = generate_engine_api(&api, "00").expect("full generated API");
    let report = analyze_engine_api(&api).expect("full coverage report");

    verify_engine_api_coverage(&report).expect("every official identity is accounted for");
    assert_eq!(
        report.total_official_entries,
        report.generated_official_entries + report.classified_official_entries
    );
    assert!(
        report
            .coverage
            .iter()
            .all(|entry| !entry.identity.is_empty() && !entry.reason.is_empty())
    );
    assert!(generated.contains("pub const FIXTURE_CONSTANT: i64 = 42_i64;"));
    assert!(generated.contains("pub mod utility {"));
    assert!(generated.contains("pub fn fixture_utility("));
    assert!(generated.contains("pub mod builtin {"));
    assert!(generated.contains("pub mod string {"));
    assert!(generated.contains("pub fn construct_1("));
    assert!(generated.contains("pub fn operator_add_string_0("));
    assert!(generated.contains("pub fn member_get_fixture_member("));
    assert!(generated.contains("pub fn indexed_get("));
    assert!(generated.contains("pub fn keyed_get("));
    assert!(generated.contains("pub fn constant_fixture_text("));
    assert!(generated.contains("pub trait StringBuiltinApi"));
    assert!(generated.contains("fn godot_length(&self)"));
    assert!(generated.contains("pub fn singleton()"));
    assert!(generated.contains("pub fn new_godot()"));
    assert!(generated.contains("pub trait ObjectSignals"));
    assert!(generated.contains("fn signal_fixture_changed(&self)"));
}

#[test]
fn engine_api_report_classifies_every_skipped_method() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    let methods = value["classes"][0]["methods"]
        .as_array_mut()
        .expect("methods should be an array");
    methods.extend([
        serde_json::json!({
            "name": "_virtual_callback",
            "is_const": false,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": true
        }),
        serde_json::json!({
            "name": "_unsafe_virtual_callback",
            "is_const": false,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": true,
            "arguments": [{"name": "state", "type": "void*"}]
        }),
        serde_json::json!({
            "name": "static_method",
            "is_const": false,
            "is_vararg": false,
            "is_static": true,
            "is_virtual": false,
            "hash": 2
        }),
        serde_json::json!({
            "name": "vararg_method",
            "is_const": false,
            "is_vararg": true,
            "is_static": false,
            "is_virtual": false,
            "hash": 3
        }),
        serde_json::json!({
            "name": "hashless_method",
            "is_const": false,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": false
        }),
        serde_json::json!({
            "name": "unsafe_pointer_method",
            "is_const": true,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": false,
            "hash": 4,
            "return_value": {"type": "const void*"}
        }),
        serde_json::json!({
            "name": "unsupported_method",
            "is_const": true,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": false,
            "hash": 5,
            "return_value": {"type": "FutureBuiltin"}
        }),
    ]);
    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");
    let generated = generate_engine_api(&api, "00").expect("static engine API");
    let report = analyze_engine_api(&api).expect("generation report");

    assert_eq!(report.total_methods, 8);
    assert_eq!(report.generated_methods, 3);
    assert_eq!(report.virtual_methods, 2);
    assert_eq!(report.generated_virtual_methods, 1);
    assert_eq!(report.unsupported_virtual_methods, 0);
    assert!(report.unsupported_virtual_types.is_empty());
    assert_eq!(report.unsafe_pointer_virtual_methods, 1);
    assert_eq!(report.unsafe_pointer_virtual_types.len(), 1);
    assert_eq!(report.unsafe_pointer_virtual_types[0].godot_type, "void*");
    assert_eq!(report.static_methods, 1);
    assert_eq!(report.vararg_methods, 1);
    assert_eq!(report.methods_without_hash, 1);
    assert_eq!(report.methods_with_unsupported_types, 1);
    assert_eq!(report.unsupported_types.len(), 1);
    assert_eq!(report.unsupported_types[0].godot_type, "FutureBuiltin");
    assert_eq!(report.unsupported_types[0].blocked_methods, 1);
    assert_eq!(report.unsafe_pointer_methods, 1);
    assert_eq!(report.unsafe_pointer_types.len(), 1);
    assert_eq!(report.unsafe_pointer_types[0].godot_type, "const void*");
    assert_eq!(report.unsafe_pointer_types[0].blocked_methods, 1);
    assert!(generated.contains("pub const TYPE_BLOCKED_ENGINE_METHOD_COUNT: usize = 1;"));
    assert!(generated.contains("pub const UNSAFE_POINTER_ENGINE_METHOD_COUNT: usize = 1;"));
    assert!(generated.contains("pub const UNSUPPORTED_VIRTUAL_OVERRIDE_COUNT: usize = 0;"));
    assert!(generated.contains("pub const UNSAFE_POINTER_VIRTUAL_OVERRIDE_COUNT: usize = 1;"));
    assert!(generated.contains("impl super::Object {"));
    assert!(generated.contains("pub fn static_method() -> crate::error::EngineResult<()>"));
    assert!(generated.contains("reserved_flags: crate::abi::ABI_GODOT_METHOD_STATIC"));
    assert!(generated.contains("super::ObjectRef::<super::Object>::unresolved()"));
    assert!(generated.contains("fn vararg_method(&self, varargs: &[crate::variant::Variant])"));
    assert!(generated.contains("reserved_flags: crate::abi::ABI_GODOT_METHOD_VARARG"));
    assert!(generated.contains(
        "arguments.extend(varargs.iter().map(super::EngineArgument::__into_engine_argument))"
    ));
    let error = verify_engine_api_coverage(&report)
        .expect_err("a future safe type and hashless method must fail the coverage gate");
    assert!(error.to_string().contains("lack the MethodBind hash"));
}

#[test]
fn engine_api_coverage_accepts_intentional_raw_pointer_omissions() {
    let report = godot_rs_codegen::EngineApiGenerationReport {
        total_official_entries: 4,
        generated_official_entries: 2,
        classified_official_entries: 2,
        coverage: vec![
            ApiCoverageEntry {
                category: "engine_method".into(),
                identity: "Object.generated_one".into(),
                disposition: ApiCoverageDisposition::Generated,
                reason: "generated typed MethodBind wrapper".into(),
            },
            ApiCoverageEntry {
                category: "engine_method".into(),
                identity: "Object.generated_two".into(),
                disposition: ApiCoverageDisposition::Generated,
                reason: "generated typed MethodBind wrapper".into(),
            },
            ApiCoverageEntry {
                category: "engine_method".into(),
                identity: "Object.unsafe_method".into(),
                disposition: ApiCoverageDisposition::UnsafeNativePointer,
                reason: "raw native pointer cannot cross the Script Mode ABI".into(),
            },
            ApiCoverageEntry {
                category: "engine_method".into(),
                identity: "Object.unsafe_virtual".into(),
                disposition: ApiCoverageDisposition::UnsafeNativePointer,
                reason: "raw native pointer cannot cross the Script Mode ABI".into(),
            },
        ],
        total_methods: 4,
        generated_methods: 2,
        virtual_methods: 1,
        generated_virtual_methods: 0,
        unsupported_virtual_methods: 0,
        unsupported_virtual_types: Vec::new(),
        unsafe_pointer_virtual_methods: 1,
        unsafe_pointer_virtual_types: vec![godot_rs_codegen::UnsupportedEngineType {
            godot_type: "const uint8_t*".into(),
            blocked_methods: 1,
        }],
        static_methods: 1,
        vararg_methods: 1,
        methods_without_hash: 0,
        methods_with_unsupported_types: 0,
        unsupported_types: Vec::new(),
        unsafe_pointer_methods: 1,
        unsafe_pointer_types: vec![godot_rs_codegen::UnsupportedEngineType {
            godot_type: "void*".into(),
            blocked_methods: 1,
        }],
    };

    verify_engine_api_coverage(&report).expect("raw pointer omissions are intentional");
}

#[test]
fn process_named_virtuals_are_special_only_on_node() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    let classes = value["classes"]
        .as_array_mut()
        .expect("classes should be an array");
    classes.extend([
        serde_json::json!({
            "name": "Node",
            "inherits": "Object",
            "is_refcounted": false,
            "is_instantiable": true,
            "api_type": "scene",
            "methods": [{
                "name": "_process",
                "is_const": false,
                "is_vararg": false,
                "is_static": false,
                "is_virtual": true,
                "arguments": [{"name": "delta", "type": "float"}]
            }]
        }),
        serde_json::json!({
            "name": "Worker",
            "inherits": "Object",
            "is_refcounted": false,
            "is_instantiable": true,
            "api_type": "core",
            "methods": [{
                "name": "_process",
                "is_const": false,
                "is_vararg": false,
                "is_static": false,
                "is_virtual": true,
                "return_value": {"type": "float"},
                "arguments": [{"name": "delta", "type": "float"}]
            }]
        }),
        serde_json::json!({
            "name": "UnsafeWorker",
            "inherits": "Object",
            "is_refcounted": false,
            "is_instantiable": true,
            "api_type": "core",
            "methods": [{
                "name": "_process",
                "is_const": false,
                "is_vararg": false,
                "is_static": false,
                "is_virtual": true,
                "arguments": [{"name": "state", "type": "void*"}]
            }]
        }),
    ]);
    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");
    let generated = generate_engine_api(&api, "00").expect("generated engine API");
    let report = analyze_engine_api(&api).expect("generation report");

    verify_engine_api_coverage(&report).expect("every process method must be classified exactly");
    assert!(generated.contains("pub trait WorkerVirtual"));
    assert!(generated.contains("fn _process(&mut self, delta: f64) -> f64"));
    assert_eq!(report.unsafe_pointer_virtual_methods, 1);
    assert!(report.coverage.iter().any(|entry| {
        entry.identity == "UnsafeWorker._process"
            && entry.disposition == ApiCoverageDisposition::UnsafeNativePointer
    }));
}

#[test]
fn string_name_methods_use_typed_returns_and_utf8_arguments() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    value["classes"][0]["methods"]
        .as_array_mut()
        .expect("methods should be an array")
        .push(serde_json::json!({
            "name": "translate_name",
            "is_const": true,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": false,
            "hash": 42,
            "return_value": {"type": "StringName"},
            "arguments": [{"name": "name", "type": "StringName"}]
        }));
    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");
    let generated = generate_engine_api(&api, "00").expect("StringName engine API");

    assert!(generated.contains(
        "fn translate_name(&self, name: &str) -> crate::error::EngineResult<crate::string_name::StringName>;"
    ));
    assert!(generated.contains("super::string_name_argument(name)"));
    assert!(generated.contains("value_type: crate::abi::AbiValueType::STRING_NAME"));
    assert!(generated.contains("ptrcall_type: crate::abi::AbiPtrcallType::STRING_NAME"));
}

#[test]
fn refcounted_returns_use_owned_godot_refs() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    value["classes"]
        .as_array_mut()
        .expect("classes should be an array")
        .push(serde_json::json!({
            "name": "Resource",
            "inherits": "Object",
            "is_refcounted": true,
            "is_instantiable": true,
            "api_type": "core"
        }));
    value["classes"][0]["methods"]
        .as_array_mut()
        .expect("methods should be an array")
        .push(serde_json::json!({
            "name": "get_resource",
            "is_const": true,
            "is_vararg": false,
            "is_static": false,
            "is_virtual": false,
            "hash": 42,
            "return_value": {"type": "Resource"}
        }));
    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");
    let generated = generate_engine_api(&api, "00").expect("RefCounted engine API");

    assert!(generated.contains(
        "fn get_resource(&self) -> crate::error::EngineResult<Option<super::GodotRef<super::Resource>>>;"
    ));
    assert!(generated.contains("ptrcall_type: crate::abi::AbiPtrcallType::REFCOUNTED_OBJECT"));
}

#[test]
fn duplicate_class_names_are_rejected() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    let classes = value["classes"]
        .as_array_mut()
        .expect("classes should be an array");
    classes.push(classes[0].clone());
    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");

    let issues = validate_api(&api, ExpectedApiVersion { major: 4, minor: 4 });
    assert!(
        issues
            .iter()
            .any(|issue| issue.message == "duplicate name `Object`")
    );
}

#[test]
fn missing_non_virtual_method_hash_is_rejected() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    value["classes"][0]["methods"][0]
        .as_object_mut()
        .expect("method should be an object")
        .remove("hash");
    let api: ExtensionApi = serde_json::from_value(value).expect("modified fixture should parse");

    let issues = validate_api(&api, ExpectedApiVersion { major: 4, minor: 4 });
    assert!(issues.iter().any(|issue| {
        issue.path == "classes.Object.methods.get_instance_id"
            && issue.message.contains("no Method Bind hash")
    }));
}

#[test]
fn class_order_does_not_change_generated_snapshot() {
    let first = load_api(&fixture()).expect("fixture should parse");
    let mut second = load_api(&fixture()).expect("fixture should parse");
    second.api.classes.reverse();

    assert_eq!(
        generate_api_snapshot(&first.api, &first.sha256),
        generate_api_snapshot(&second.api, &first.sha256)
    );
}

#[test]
fn api_diff_reports_method_hash_and_removed_class() {
    let from = load_api(&fixture()).expect("fixture should parse");
    let mut to = load_api(&fixture()).expect("fixture should parse");
    to.api.classes[0].methods[0].hash = Some(1);
    to.api
        .classes
        .retain(|class| class.name != "ScriptExtension");

    let diff = diff_api(&from.api, &to.api);
    assert_eq!(diff.removed_classes, ["ScriptExtension"]);
    assert_eq!(diff.changed_methods.len(), 1);
    let markdown = diff.to_markdown();
    assert!(markdown.contains("Object::get_instance_id"));
    assert!(markdown.contains("删除 Class"));
}

#[test]
fn unknown_official_schema_fields_are_rejected() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    value["classes"][0]["methods"][0]["unexpected_field"] = serde_json::json!(true);

    let error = serde_json::from_value::<ExtensionApi>(value)
        .expect_err("unknown nested fields must stop generation");
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn missing_official_schema_categories_are_rejected() {
    let source = std::fs::read_to_string(fixture()).expect("fixture should be readable");
    let mut value: serde_json::Value =
        serde_json::from_str(&source).expect("fixture should parse as JSON");
    value
        .as_object_mut()
        .expect("API root")
        .remove("native_structures");

    let error = serde_json::from_value::<ExtensionApi>(value)
        .expect_err("missing root categories must stop generation");
    assert!(error.to_string().contains("native_structures"));
}

#[test]
fn inventory_counts_every_modeled_category() {
    let loaded = load_api(&fixture()).expect("fixture should parse");
    let inventory = loaded.api.inventory();
    assert_eq!(inventory.builtin_size_configuration_count, 4);
    assert_eq!(inventory.builtin_offset_configuration_count, 4);
    assert_eq!(inventory.builtin_class_count, 1);
    assert_eq!(inventory.builtin_constructor_count, 1);
    assert_eq!(inventory.engine_class_count, 3);
    assert_eq!(inventory.engine_method_count, 1);
}
