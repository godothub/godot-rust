use godot_rs::engine::{
    AStar2D, Engine, builtin,
    global::{PropertyHint, PropertyUsageFlags},
    native_virtual, utility,
};
use godot_rs::native::{
    Base, ClassRegistrar, ExtensionLibrary, GODOT_API, InitializationContext, InitializationLevel,
    NativeClass, NativeError, NativeProperty, NativePropertyOptions, NativePropertyValidation,
    NativeResult, RpcConfig, RpcMode, RpcTransferMode,
    classes::{Node, Node2D},
};
use godot_rs::prelude::{
    AStar2DApi, Array, Callable, Dictionary, Node2DApi, NodeApi, NodePath, ObjectApi, ObjectRef,
    PackedInt64Array, Signal, StringName, Variant, VariantKind, Vector2,
};

const NATIVE_GENERATION: &str = "before";

struct NativeSmoke;

fn print_lifecycle_marker(marker: &str) -> NativeResult {
    utility::print(&Variant::from(format!("{marker} api={GODOT_API}")), &[]).map_err(|error| {
        NativeError::new(format!("failed to print Native lifecycle marker: {error}"))
    })
}

struct GodotRsNativeSmoke {
    base: Base<Node2D>,
    value: i64,
    metadata: Dictionary,
    reload_marker: i64,
    process_frames: i64,
    dynamic_note: String,
}

impl GodotRsNativeSmoke {
    fn add(&mut self, amount: i64) -> i64 {
        self.value += amount;
        self.value
    }

    fn current(&self) -> i64 {
        self.value
    }

    fn set_value(&mut self, value: i64) {
        self.value = value;
    }

    fn metadata(&self) -> Dictionary {
        self.metadata.clone()
    }

    fn set_metadata(&mut self, metadata: Dictionary) {
        self.metadata = metadata;
    }

    fn generation(&self) -> String {
        NATIVE_GENERATION.to_owned()
    }

    fn reload_marker(&self) -> i64 {
        self.reload_marker
    }

    fn process_frames(&self) -> i64 {
        self.process_frames
    }

    fn process(&mut self, _delta: f64) {
        self.process_frames += 1;
    }

    fn is_positive(&self) -> bool {
        self.value > 0
    }

    fn instance_id(&self) -> i64 {
        i64::try_from(self.base.instance_id()).unwrap_or(i64::MAX)
    }

    fn describe_offset(&self, label: String, offset: Vector2) -> String {
        format!("{label}:{}:{}", offset.x, offset.y)
    }

    fn is_same_node(&self, node: Option<ObjectRef<Node>>) -> bool {
        node.is_some_and(|node| node.instance_id() == self.base.instance_id())
    }

    fn owner(&self) -> ObjectRef<Node> {
        ObjectRef::__from_instance_id(self.base.instance_id())
    }

    fn enable_processing(&self) -> bool {
        self.base.set_process(true).is_ok() && self.base.is_processing().unwrap_or(false)
    }

    fn generated_api_round_trip(&self) -> String {
        if self.base.set_name("NativeGenerated").is_err()
            || self.base.set_position(Vector2::new(12.5, -4.0)).is_err()
            || self.base.set_process_priority(2_047).is_err()
        {
            return "generated call failed".to_owned();
        }
        let Ok(name) = self.base.get_name() else {
            return "generated name failed".to_owned();
        };
        let Ok(position) = self.base.get_position() else {
            return "generated position failed".to_owned();
        };
        let Ok(priority) = self.base.get_process_priority() else {
            return "generated priority failed".to_owned();
        };
        if priority != 2_047 {
            return format!("generated priority mismatch: {priority}");
        }
        format!("{name}:{}:{}", position.x, position.y)
    }

    fn generated_global_api_round_trip(&self) -> bool {
        let result = || {
            let vector = builtin::vector2::construct_3(3.0, 4.0)?;
            let length = builtin::vector2::length(&vector)?;
            let degrees = utility::rad_to_deg(core::f64::consts::PI)?;
            let singleton = Engine::singleton()?;
            Ok::<_, godot_rs::error::EngineError>(
                (length - 5.0).abs() < f64::EPSILON
                    && (degrees - 180.0).abs() < f64::EPSILON
                    && singleton.is_resolved(),
            )
        };
        match result() {
            Ok(value) => value,
            Err(error) => {
                eprintln!("GODOT_RS_NATIVE_GLOBAL_ERROR {error}");
                false
            }
        }
    }

    fn generated_refcounted_round_trip(&self) -> bool {
        let result = || {
            let graph = AStar2D::new_godot()?;
            graph.add_point(7, Vector2::new(2.0, 3.0), 1.0)?;
            let point_ids = graph.get_point_ids()?;
            let mut builtin_ids = PackedInt64Array::from_vec(vec![2, 3]);
            // Godot's packed-array append returns its internal failure flag:
            // `false` means that the append succeeded.
            let append_failed = builtin::packed_int64_array::append(&mut builtin_ids, 5)?;
            let instance_id = graph.instance_id();
            let point_count = graph.get_point_count()?;
            let has_point = graph.has_point(7)?;
            let weight = graph.get_point_weight_scale(7)?;
            let valid = instance_id != 0
                && point_count == 1
                && has_point
                && weight == 1.0
                && point_ids.as_ref() == [7]
                && !append_failed
                && builtin_ids.as_ref() == [2, 3, 5];
            if !valid {
                eprintln!(
                    "GODOT_RS_NATIVE_REFCOUNTED_VALUES id={instance_id} \
                     count={point_count} has={has_point} weight={weight} ids={:?} \
                     append_failed={append_failed} builtin={:?}",
                    point_ids.as_ref(),
                    builtin_ids.as_ref(),
                );
            }
            Ok::<_, godot_rs::error::EngineError>(valid)
        };
        match result() {
            Ok(value) => value,
            Err(error) => {
                eprintln!("GODOT_RS_NATIVE_REFCOUNTED_ERROR {error}");
                false
            }
        }
    }

    fn generated_dynamic_api_round_trip(&self) -> bool {
        let mut array = Array::from_vec(vec![Variant::from(11_i64)]);
        if let Err(error) = builtin::array::append(&mut array, &Variant::from("dynamic")) {
            eprintln!("GODOT_RS_NATIVE_DYNAMIC_ERROR array.append: {error}");
            return false;
        }
        let last = match builtin::array::back(&array) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("GODOT_RS_NATIVE_DYNAMIC_ERROR array.back: {error}");
                return false;
            }
        };

        let mut dictionary = Dictionary::new();
        let key = Variant::from("answer");
        if let Err(error) =
            builtin::dictionary::keyed_set(&mut dictionary, &key, &Variant::from(42_i64))
        {
            eprintln!("GODOT_RS_NATIVE_DYNAMIC_ERROR dictionary.keyed_set: {error}");
            return false;
        }
        let answer = match builtin::dictionary::keyed_get(&dictionary, &key) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("GODOT_RS_NATIVE_DYNAMIC_ERROR dictionary.keyed_get: {error}");
                return false;
            }
        };

        let called_name = match ObjectApi::call(&self.base, "get_name", &[]) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("GODOT_RS_NATIVE_DYNAMIC_ERROR Node.call: {error}");
                return false;
            }
        };
        let valid = array.len() == 2
            && matches!(last.kind(), VariantKind::String("dynamic"))
            && matches!(answer.kind(), VariantKind::Int(42))
            && matches!(
                called_name.kind(),
                VariantKind::StringName(value) if value.as_str() == "NativeGenerated"
            );
        if !valid {
            eprintln!(
                "GODOT_RS_NATIVE_DYNAMIC_VALUES len={} last={:?} answer={:?} name={:?}",
                array.len(),
                last.kind(),
                answer.kind(),
                called_name.kind(),
            );
        }
        valid
    }

    fn generated_callable_signal_round_trip(&self) -> bool {
        let result = || {
            let object = ObjectRef::__from_instance_id(self.base.instance_id());
            let callable = builtin::callable::construct_2(object, "current")?;
            let current = builtin::callable::call(&callable, &[])?;
            let bound_source = builtin::callable::construct_2(object, "describe_offset")?;
            let bound = builtin::callable::bind(
                &bound_source,
                &[
                    Variant::from("bound"),
                    Variant::from(Vector2::new(3.0, 4.0)),
                ],
            )?;
            let bound_result = builtin::callable::call(&bound, &[])?;

            let signal = builtin::signal::construct_2(object, "offset_described")?;
            let signal_name = builtin::signal::get_name(&signal)?;
            let signal_object_id = builtin::signal::get_object_id(&signal)?;

            let mut nested = Array::from_vec(vec![
                Variant::from(bound.clone()),
                Variant::from(signal.clone()),
            ]);
            let nested_callable = builtin::array::pop_front(&mut nested)?;
            let nested_result = match nested_callable.kind() {
                VariantKind::Callable(callable) => builtin::callable::call(callable, &[])?,
                _ => return Ok::<_, godot_rs::error::EngineError>(false),
            };
            let nested_signal = builtin::array::front(&nested)?;
            let nested_name = match nested_signal.kind() {
                VariantKind::Signal(signal) => builtin::signal::get_name(signal)?,
                _ => return Ok(false),
            };

            Ok(matches!(current.kind(), VariantKind::Int(64))
                && matches!(bound_result.kind(), VariantKind::String("bound:3:4"))
                && matches!(nested_result.kind(), VariantKind::String("bound:3:4"))
                && signal_name.as_str() == "offset_described"
                && nested_name.as_str() == "offset_described"
                && signal_object_id as u64 == self.base.instance_id())
        };
        match result() {
            Ok(value) => value,
            Err(error) => {
                eprintln!("GODOT_RS_NATIVE_CALLABLE_ERROR {error}");
                false
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn native_owned_values(
        &self,
        values: Array<Variant>,
        dictionary: Dictionary,
        packed: PackedInt64Array,
        callable: Callable,
        signal: Signal,
        variant: Variant,
        path: NodePath,
        name: StringName,
    ) -> bool {
        values.len() == 2
            && dictionary.get(&Variant::from("answer")) == Some(&Variant::from(42_i64))
            && packed.as_ref() == [3, 5, 8]
            && callable.method().as_str() == "current"
            && signal.name() == "offset_described"
            && matches!(variant.kind(), VariantKind::String("any Variant"))
            && path.as_str() == "Child/Camera"
            && name.as_str() == "NativeName"
    }

    fn native_enum_round_trip(
        &self,
        value: godot_rs::engine::global::Error,
    ) -> godot_rs::engine::global::Error {
        value
    }
}

impl NativeClass for GodotRsNativeSmoke {
    type Base = Node2D;

    const CLASS_NAME: &'static str = "GodotRsNativeSmoke";

    fn init(base: Base<Self::Base>) -> Self {
        Self {
            base,
            value: 41,
            metadata: Dictionary::new(),
            reload_marker: 0,
            process_frames: 0,
            dynamic_note: "default note".to_owned(),
        }
    }

    fn on_extension_reloaded(&mut self) -> NativeResult {
        self.reload_marker = 99;
        Ok(())
    }

    fn set_property(&mut self, name: &str, value: Variant) -> Result<bool, NativeError> {
        if name != "dynamic_note" {
            return Ok(false);
        }
        let VariantKind::String(value) = value.kind() else {
            return Err(NativeError::new("dynamic_note requires a String"));
        };
        self.dynamic_note = value.to_owned();
        Ok(true)
    }

    fn get_property(&self, name: &str) -> Result<Option<Variant>, NativeError> {
        Ok((name == "dynamic_note").then(|| Variant::from(self.dynamic_note.clone())))
    }

    fn property_list(&self) -> Result<Vec<NativeProperty>, NativeError> {
        Ok(vec![NativeProperty::new::<String>(
            "dynamic_note",
            NativePropertyOptions::new(
                PropertyHint::PROPERTY_HINT_MULTILINE_TEXT,
                "",
                PropertyUsageFlags::PROPERTY_USAGE_STORAGE
                    | PropertyUsageFlags::PROPERTY_USAGE_EDITOR,
            ),
        )?])
    }

    fn property_can_revert(&self, name: &str) -> bool {
        name == "dynamic_note"
    }

    fn property_get_revert(&self, name: &str) -> Result<Option<Variant>, NativeError> {
        Ok((name == "dynamic_note").then(|| Variant::from("default note")))
    }

    fn validate_property(
        &mut self,
        property: &mut NativePropertyValidation,
    ) -> Result<(), NativeError> {
        if property.name == "metadata" {
            property.usage |= PropertyUsageFlags::PROPERTY_USAGE_STORE_IF_NULL;
        }
        Ok(())
    }

    fn to_godot_string(&self) -> Option<String> {
        Some(format!("GodotRsNativeSmoke(value={})", self.value))
    }

    fn register_virtuals(
        registrar: &mut godot_rs::native::NativeVirtualRegistrar<'_, Self>,
    ) -> NativeResult {
        native_virtual::node::_process(registrar, Self::process)?;
        Ok(())
    }

    fn register_methods(registrar: &mut ClassRegistrar<'_, Self>) -> NativeResult {
        registrar
            .method_with_arguments("add", ["amount"], Self::add)?
            .method("current", Self::current)?
            .method("is_positive", Self::is_positive)?
            .method("instance_id", Self::instance_id)?
            .method_with_arguments(
                "describe_offset",
                ["label", "offset"],
                Self::describe_offset,
            )?
            .property_group("State", "")?
            .property_with_options(
                "value",
                NativePropertyOptions::new(
                    PropertyHint::PROPERTY_HINT_RANGE,
                    "0,100,1",
                    PropertyUsageFlags::PROPERTY_USAGE_STORAGE
                        | PropertyUsageFlags::PROPERTY_USAGE_EDITOR,
                ),
                Self::current,
                Self::set_value,
            )?
            .property("metadata", Self::metadata, Self::set_metadata)?
            .method("generation", Self::generation)?
            .method("reload_marker", Self::reload_marker)?
            .method("process_frames", Self::process_frames)?
            .signal::<(String, Vector2), 2>("offset_described", ["label", "offset"])?
            .method_with_arguments("is_same_node", ["node"], Self::is_same_node)?
            .method("owner", Self::owner)?
            .method("enable_processing", Self::enable_processing)?
            .method("generated_api_round_trip", Self::generated_api_round_trip)?
            .method(
                "generated_global_api_round_trip",
                Self::generated_global_api_round_trip,
            )?
            .method(
                "generated_refcounted_round_trip",
                Self::generated_refcounted_round_trip,
            )?
            .method(
                "generated_dynamic_api_round_trip",
                Self::generated_dynamic_api_round_trip,
            )?
            .method(
                "generated_callable_signal_round_trip",
                Self::generated_callable_signal_round_trip,
            )?
            .method_with_arguments(
                "native_owned_values",
                [
                    "values",
                    "dictionary",
                    "packed",
                    "callable",
                    "signal",
                    "variant",
                    "path",
                    "name",
                ],
                Self::native_owned_values,
            )?
            .method_with_arguments(
                "native_enum_round_trip",
                ["value"],
                Self::native_enum_round_trip,
            )?
            .rpc_method_with_arguments(
                "network_add",
                ["amount"],
                RpcConfig {
                    mode: RpcMode::AnyPeer,
                    call_local: true,
                    transfer_mode: RpcTransferMode::Reliable,
                    channel: 3,
                },
                Self::add,
            )?;
        Ok(())
    }
}

impl ExtensionLibrary for NativeSmoke {
    fn on_level_initialize(
        context: &InitializationContext,
        level: InitializationLevel,
    ) -> NativeResult {
        if level == InitializationLevel::Scene {
            context.register_class::<GodotRsNativeSmoke>()?;
            print_lifecycle_marker("GODOT_RS_NATIVE_READY")?;
        }
        Ok(())
    }

    fn on_level_deinitialize(
        _context: &InitializationContext,
        level: InitializationLevel,
    ) -> NativeResult {
        if level == InitializationLevel::Scene {
            print_lifecycle_marker("GODOT_RS_NATIVE_STOPPED")?;
        }
        Ok(())
    }
}

godot_rs::gdextension!(NativeSmoke);
