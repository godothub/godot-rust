use godot_rs::prelude::*;
use godot_rs::script::{
    FieldKind, LifecycleSlot, MethodCallback, MethodKind, ReloadPolicy, ScriptClass,
    ScriptFieldAccess, registered_methods,
};

#[script(base = CharacterBody2D, class_name = PlayerController, tool)]
struct Player {
    #[export(
        default = 240.0,
        group = "Movement",
        range(min = 0.0, max = 1000.0, step = 1.0, suffix = "px/s")
    )]
    speed: f32,

    #[node("%Sprite")]
    sprite: NodeRef<AnimatedSprite2D>,

    #[node("%Camera", optional)]
    camera: Option<NodeRef<Camera2D>>,

    #[signal(args(old_health, new_health))]
    health_changed: Signal<(i32, i32)>,

    #[signal(args(message))]
    message_sent: Signal<(String,)>,

    #[reload(skip, default = 7)]
    transient_counter: i32,

    ready: bool,

    last_name: String,

    #[export(default = "等待构建", multiline)]
    status_text: String,

    #[export(default = "*.tscn", file("*.tscn"))]
    scene_filter: String,

    #[export]
    empty_text: String,

    #[export(default = "../Spawn")]
    spawn_path: NodePath,

    #[export]
    labels: PackedStringArray,

    #[export]
    patrol_points: Array<Vector2>,

    #[export]
    settings: Dictionary,

    #[export(group = "Objects")]
    selected_node: Option<NodeRef<Node>>,

    #[export]
    gradient: Option<GodotRef<Gradient>>,

    #[export(default = node::ProcessMode::PROCESS_MODE_ALWAYS, group = "Behavior")]
    worker_mode: node::ProcessMode,

    #[export]
    thread_messages: node::ProcessThreadMessages,

    #[export(default = Vector2::new(240.0, 120.0), group = "Movement")]
    spawn_point: Vector2,

    #[export(default = Vector3::new(4.0, 8.0, 12.0))]
    target_position: Vector3,

    #[export(default = Color::rgba(0.2, 0.4, 0.8, 0.5), no_alpha)]
    accent_color: Color,

    #[export(default = Transform2D::new(
        Vector2::RIGHT,
        Vector2::DOWN,
        Vector2::new(12.0, -6.0),
    ))]
    canvas_transform: Transform2D,

    #[export(default = Aabb::new(
        Vector3::new(1.0, 2.0, 3.0),
        Vector3::new(4.0, 5.0, 6.0),
    ))]
    bounds: Aabb,

    #[export(default = Basis::IDENTITY)]
    basis: Basis,

    #[export(default = Transform3D::new(
        Basis::IDENTITY,
        Vector3::new(7.0, 8.0, 9.0),
    ))]
    world_transform: Transform3D,

    #[export(default = Projection::IDENTITY)]
    projection: Projection,

    #[signal(args(point, position, color))]
    motion_changed: Signal<(Vector2, Vector3, Color)>,
}

#[script]
impl Player {
    fn _ready(&mut self) -> ScriptResult<()> {
        self.ready = true;
        Ok(())
    }

    fn _physics_process(&mut self, delta: f64) {
        self.transient_counter += delta as i32;
    }

    fn _input(&mut self, event: InputEventRef) {
        let _ = event;
    }

    fn _exit_tree(&mut self) {}

    #[func]
    fn take_damage(&mut self, amount: i32) {
        self.transient_counter -= amount;
    }

    #[func]
    fn current_counter(&self) -> i32 {
        self.transient_counter
    }

    #[func]
    fn greet(&mut self, #[default(String::from("Godot"))] name: String) -> String {
        self.last_name = name.clone();
        format!("你好，{name}")
    }

    #[func]
    fn count_variants(&self, values: &[Variant]) -> i64 {
        i64::try_from(values.len()).unwrap_or(i64::MAX)
    }

    #[func]
    fn offset_point(&self, point: Vector2, offset: Vector2) -> Vector2 {
        point + offset
    }

    #[func]
    fn offset_position(&self, point: Vector3, offset: Vector3) -> Vector3 {
        point + offset
    }

    #[func]
    fn modulate(&self, color: Color, tint: Color) -> Color {
        color * tint
    }

    #[rpc(authority, call_local, reliable, channel = 0)]
    fn sync_state(&mut self, value: i32) {
        self.transient_counter = value;
    }

    fn internal_only(&self) -> bool {
        self.ready
    }
}

#[script]
impl Player {
    #[func]
    fn on_health_changed(&mut self, old_health: i32, new_health: i32) {
        self.transient_counter = new_health - old_health;
    }

    #[func]
    fn mirror_node(&self, node: Option<ObjectRef<Node>>) -> Option<ObjectRef<Node>> {
        node
    }

    #[func]
    fn mirror_process_mode(&self, mode: node::ProcessMode) -> node::ProcessMode {
        mode
    }

    #[func]
    fn mirror_process_flags(
        &self,
        flags: node::ProcessThreadMessages,
    ) -> node::ProcessThreadMessages {
        flags
    }
}

#[script(base = Node)]
struct PanickingScript;

#[script]
impl PanickingScript {
    fn _ready(&mut self) {
        panic!("expected callback panic");
    }

    #[func]
    fn explode(&self) -> i32 {
        panic!("expected reflected method panic");
    }
}

#[script(base = Node)]
struct RawIdentifierScript;

#[script]
impl RawIdentifierScript {
    #[func]
    fn r#type(&self) -> String {
        "Rust".into()
    }
}

#[script(base = CharacterBody2D, extends = "res://src/scripts/player.rs")]
struct DerivedPlayer;

godot_rs::script_module!(
    Player => ("res://src/scripts/player.rs", "uid://c2"),
);

#[test]
fn struct_macro_generates_defaults_and_field_metadata() {
    let mut player = Player::__godot_rs_new();
    let descriptor = Player::DESCRIPTOR;

    assert_eq!(descriptor.name, "Player");
    assert_eq!(descriptor.global_name, Some("PlayerController"));
    assert_eq!(descriptor.base_script, None);
    assert_eq!(descriptor.base, "CharacterBody2D");
    assert!(descriptor.tool);
    assert_eq!(descriptor.fields.len(), 28);
    assert_eq!(player.speed, 240.0);
    assert!(!player.sprite.is_resolved());
    assert!(player.camera.is_none());
    assert_eq!(player.transient_counter, 7);
    assert!(!player.ready);
    assert!(player.last_name.is_empty());
    assert_eq!(player.status_text, "等待构建");
    assert_eq!(player.scene_filter, "*.tscn");
    assert!(player.empty_text.is_empty());
    assert_eq!(player.spawn_path, NodePath::from("../Spawn"));
    assert!(player.labels.is_empty());
    assert!(player.patrol_points.is_empty());
    assert!(player.settings.is_empty());
    assert!(player.selected_node.is_none());
    assert!(player.gradient.is_none());
    assert_eq!(player.worker_mode, node::ProcessMode::PROCESS_MODE_ALWAYS);
    assert!(player.thread_messages.is_empty());
    assert_eq!(player.spawn_point, Vector2::new(240.0, 120.0));
    assert_eq!(player.target_position, Vector3::new(4.0, 8.0, 12.0));
    assert_eq!(player.accent_color, Color::rgba(0.2, 0.4, 0.8, 0.5));
    assert_eq!(
        player.canvas_transform,
        Transform2D::new(Vector2::RIGHT, Vector2::DOWN, Vector2::new(12.0, -6.0))
    );
    assert_eq!(
        player.bounds,
        Aabb::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(4.0, 5.0, 6.0))
    );
    assert_eq!(player.basis, Basis::IDENTITY);
    assert_eq!(
        player.world_transform,
        Transform3D::new(Basis::IDENTITY, Vector3::new(7.0, 8.0, 9.0))
    );
    assert_eq!(player.projection, Projection::IDENTITY);
    assert!(!player.internal_only());

    let speed = descriptor
        .fields
        .iter()
        .find(|field| field.name == "speed")
        .expect("speed descriptor");
    assert_eq!(speed.kind, FieldKind::Export);
    assert_eq!(speed.default, Some("240.0"));
    assert!(speed.options.contains("Movement"));
    let property = speed.property.expect("normalized Inspector schema");
    assert_eq!(property.type_, godot_rs::abi::AbiPropertyType::FLOAT);
    assert_eq!(property.hint, godot_rs::abi::ABI_PROPERTY_HINT_RANGE);
    assert_eq!(property.hint_string, "0.0,1000.0,1.0,suffix:px/s");
    assert_eq!(property.group, Some("Movement"));
    assert_eq!(
        property.usage,
        godot_rs::abi::ABI_PROPERTY_USAGE_SCRIPT_DEFAULT
    );
    assert_eq!(
        property.default_value,
        Some(godot_rs::script::PropertyDefault::Scalar(
            godot_rs::abi::AbiValueV1::from_f64(240.0)
        ))
    );

    let sprite = descriptor
        .fields
        .iter()
        .find(|field| field.name == "sprite")
        .and_then(|field| field.node)
        .expect("structured node descriptor");
    assert_eq!(sprite.path, "%Sprite");
    assert_eq!(sprite.class_name, "AnimatedSprite2D");
    assert!(!sprite.optional);

    let camera = descriptor
        .fields
        .iter()
        .find(|field| field.name == "camera")
        .and_then(|field| field.node)
        .expect("optional node descriptor");
    assert_eq!(camera.path, "%Camera");
    assert_eq!(camera.class_name, "Camera2D");
    assert!(camera.optional);

    let transient = descriptor
        .fields
        .iter()
        .find(|field| field.name == "transient_counter")
        .expect("transient descriptor");
    assert_eq!(transient.reload, ReloadPolicy::Skip);

    let signal = descriptor
        .fields
        .iter()
        .find(|field| field.name == "health_changed")
        .and_then(|field| field.signal)
        .expect("normalized signal schema");
    assert_eq!(signal.arguments.len(), 2);
    assert_eq!(signal.arguments[0].name, "old_health");
    assert_eq!(signal.arguments[0].type_, godot_rs::abi::AbiValueType::I64);
    assert_eq!(signal.arguments[1].name, "new_health");

    let status_text = descriptor
        .fields
        .iter()
        .find(|field| field.name == "status_text")
        .expect("String export descriptor");
    let property = status_text.property.expect("String property schema");
    assert_eq!(property.type_, godot_rs::abi::AbiPropertyType::STRING);
    assert_eq!(
        property.hint,
        godot_rs::abi::ABI_PROPERTY_HINT_MULTILINE_TEXT
    );
    assert_eq!(
        property.default_value,
        Some(godot_rs::script::PropertyDefault::String("等待构建"))
    );

    let scene_filter = descriptor
        .fields
        .iter()
        .find(|field| field.name == "scene_filter")
        .and_then(|field| field.property)
        .expect("file String property schema");
    assert_eq!(scene_filter.hint, godot_rs::abi::ABI_PROPERTY_HINT_FILE);
    assert_eq!(scene_filter.hint_string, "*.tscn");
    let empty_text = descriptor
        .fields
        .iter()
        .find(|field| field.name == "empty_text")
        .and_then(|field| field.property)
        .expect("empty String property schema");
    assert_eq!(
        empty_text.default_value,
        Some(godot_rs::script::PropertyDefault::String(""))
    );
    let spawn_path = descriptor
        .fields
        .iter()
        .find(|field| field.name == "spawn_path")
        .and_then(|field| field.property)
        .expect("NodePath property schema");
    assert_eq!(spawn_path.type_, godot_rs::abi::AbiPropertyType::NODE_PATH);
    assert_eq!(
        spawn_path.default_value,
        Some(godot_rs::script::PropertyDefault::NodePath("../Spawn"))
    );
    let labels = descriptor
        .fields
        .iter()
        .find(|field| field.name == "labels")
        .and_then(|field| field.property)
        .expect("PackedStringArray property schema");
    assert_eq!(
        labels.type_,
        godot_rs::abi::AbiPropertyType::PACKED_STRING_ARRAY
    );
    assert_eq!(
        labels.default_value,
        Some(godot_rs::script::PropertyDefault::Empty(
            godot_rs::abi::AbiValueType::PACKED_STRING_ARRAY
        ))
    );
    let patrol_points = descriptor
        .fields
        .iter()
        .find(|field| field.name == "patrol_points")
        .and_then(|field| field.property)
        .expect("typed Array property schema");
    assert_eq!(patrol_points.type_, godot_rs::abi::AbiPropertyType::ARRAY);
    assert_eq!(
        patrol_points.hint,
        godot_rs::abi::ABI_PROPERTY_HINT_TYPE_STRING
    );
    assert_eq!(patrol_points.hint_string, "5:");
    assert_eq!(patrol_points.typed_array_element, Some("Vector2"));
    let settings = descriptor
        .fields
        .iter()
        .find(|field| field.name == "settings")
        .and_then(|field| field.property)
        .expect("Dictionary property schema");
    assert_eq!(settings.type_, godot_rs::abi::AbiPropertyType::DICTIONARY);
    assert_eq!(
        settings.default_value,
        Some(godot_rs::script::PropertyDefault::Empty(
            godot_rs::abi::AbiValueType::DICTIONARY
        ))
    );
    let selected_node = descriptor
        .fields
        .iter()
        .find(|field| field.name == "selected_node")
        .and_then(|field| field.property)
        .expect("Node object property schema");
    assert_eq!(selected_node.type_, godot_rs::abi::AbiPropertyType::OBJECT);
    assert_eq!(
        selected_node.hint,
        godot_rs::abi::ABI_PROPERTY_HINT_NODE_TYPE
    );
    assert_eq!(selected_node.hint_string, "Node");
    assert_eq!(
        selected_node.usage,
        godot_rs::abi::ABI_PROPERTY_USAGE_SCRIPT_DEFAULT
            | godot_rs::abi::ABI_PROPERTY_USAGE_NODE_PATH_FROM_SCENE_ROOT
    );
    let gradient = descriptor
        .fields
        .iter()
        .find(|field| field.name == "gradient")
        .and_then(|field| field.property)
        .expect("Resource object property schema");
    assert_eq!(gradient.type_, godot_rs::abi::AbiPropertyType::OBJECT);
    assert_eq!(
        gradient.hint,
        godot_rs::abi::ABI_PROPERTY_HINT_RESOURCE_TYPE
    );
    assert_eq!(gradient.hint_string, "Gradient");
    assert_eq!(
        gradient.default_value,
        Some(godot_rs::script::PropertyDefault::Scalar(
            godot_rs::abi::AbiValueV1::from_object_id(0)
        ))
    );
    let worker_mode = descriptor
        .fields
        .iter()
        .find(|field| field.name == "worker_mode")
        .and_then(|field| field.property)
        .expect("generated enum property schema");
    assert_eq!(worker_mode.type_, godot_rs::abi::AbiPropertyType::INT);
    assert_eq!(worker_mode.hint, godot_rs::abi::ABI_PROPERTY_HINT_ENUM);
    assert_eq!(
        worker_mode.integer_options,
        Some(<node::ProcessMode as godot_rs::engine::GodotIntegerValue>::PROPERTY_OPTIONS)
    );
    let thread_messages = descriptor
        .fields
        .iter()
        .find(|field| field.name == "thread_messages")
        .and_then(|field| field.property)
        .expect("generated bitfield property schema");
    assert_eq!(thread_messages.type_, godot_rs::abi::AbiPropertyType::INT);
    assert_eq!(thread_messages.hint, godot_rs::abi::ABI_PROPERTY_HINT_FLAGS);
    assert_eq!(
        thread_messages.integer_options,
        Some(
            <node::ProcessThreadMessages as godot_rs::engine::GodotIntegerValue>::PROPERTY_OPTIONS
        )
    );

    let spawn_point = descriptor
        .fields
        .iter()
        .find(|field| field.name == "spawn_point")
        .and_then(|field| field.property)
        .expect("Vector2 property schema");
    assert_eq!(spawn_point.type_, godot_rs::abi::AbiPropertyType::VECTOR2);
    assert_eq!(
        spawn_point.default_value,
        Some(godot_rs::script::PropertyDefault::Scalar(
            godot_rs::abi::AbiValueV1::from_vector2(240.0, 120.0)
        ))
    );

    let target_position = descriptor
        .fields
        .iter()
        .find(|field| field.name == "target_position")
        .and_then(|field| field.property)
        .expect("Vector3 property schema");
    assert_eq!(
        target_position.type_,
        godot_rs::abi::AbiPropertyType::VECTOR3
    );
    assert_eq!(
        target_position.default_value,
        Some(godot_rs::script::PropertyDefault::Scalar(
            godot_rs::abi::AbiValueV1::from_vector3(4.0, 8.0, 12.0)
        ))
    );

    let accent_color = descriptor
        .fields
        .iter()
        .find(|field| field.name == "accent_color")
        .and_then(|field| field.property)
        .expect("Color property schema");
    assert_eq!(accent_color.type_, godot_rs::abi::AbiPropertyType::COLOR);
    assert_eq!(
        accent_color.hint,
        godot_rs::abi::ABI_PROPERTY_HINT_COLOR_NO_ALPHA
    );
    assert_eq!(
        accent_color.default_value,
        Some(godot_rs::script::PropertyDefault::Scalar(
            godot_rs::abi::AbiValueV1::from_color(0.2, 0.4, 0.8, 0.5)
        ))
    );

    let expected_fixed_math = [
        (
            "canvas_transform",
            godot_rs::abi::AbiPropertyType::TRANSFORM2D,
            6,
            vec![1.0, 0.0, 0.0, 1.0, 12.0, -6.0],
        ),
        (
            "bounds",
            godot_rs::abi::AbiPropertyType::AABB,
            6,
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        ),
        (
            "basis",
            godot_rs::abi::AbiPropertyType::BASIS,
            9,
            vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        ),
        (
            "world_transform",
            godot_rs::abi::AbiPropertyType::TRANSFORM3D,
            12,
            vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 7.0, 8.0, 9.0],
        ),
        (
            "projection",
            godot_rs::abi::AbiPropertyType::PROJECTION,
            16,
            vec![
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        ),
    ];
    for (name, type_, count, expected) in expected_fixed_math {
        let property = descriptor
            .fields
            .iter()
            .find(|field| field.name == name)
            .and_then(|field| field.property)
            .expect("fixed math property schema");
        assert_eq!(property.type_, type_);
        let Some(godot_rs::script::PropertyDefault::FixedMath(default)) = property.default_value
        else {
            panic!("fixed math default");
        };
        assert_eq!(default.component_count, count);
        assert_eq!(
            default.component_bits[..count as usize]
                .iter()
                .copied()
                .map(f32::from_bits)
                .collect::<Vec<_>>(),
            expected
        );
    }

    let motion_changed = descriptor
        .fields
        .iter()
        .find(|field| field.name == "motion_changed")
        .and_then(|field| field.signal)
        .expect("math signal schema");
    assert_eq!(motion_changed.arguments.len(), 3);
    assert_eq!(
        motion_changed.arguments[0].type_,
        godot_rs::abi::AbiValueType::VECTOR2
    );
    assert_eq!(
        motion_changed.arguments[1].type_,
        godot_rs::abi::AbiValueType::VECTOR3
    );
    assert_eq!(
        motion_changed.arguments[2].type_,
        godot_rs::abi::AbiValueType::COLOR
    );

    player.health_changed.emit((100, 90));
    player.message_sent.emit((String::from("构建完成"),));
    player.motion_changed.emit((
        Vector2::new(1.0, 2.0),
        Vector3::new(3.0, 4.0, 5.0),
        Color::rgba(0.25, 0.5, 0.75, 1.0),
    ));
    player.take_damage(10);
    player.sync_state(9);
    assert_eq!(player.transient_counter, 9);
}

#[test]
fn script_macro_records_a_canonical_base_script() {
    assert_eq!(
        DerivedPlayer::DESCRIPTOR.base_script,
        Some("res://src/scripts/player.rs")
    );
    assert_eq!(DerivedPlayer::DESCRIPTOR.base, "CharacterBody2D");
}

#[test]
fn absent_optional_script_extensions_zero_their_abi_slots() {
    let descriptor = godot_rs::script::abi_script_descriptor::<RawIdentifierScript>(
        "res://raw_identifier.rs",
        "uid://a2",
    );
    assert_eq!(descriptor.reserved[4..8], [0; 4]);
}

#[test]
fn raw_identifiers_export_their_godot_method_name() {
    let method = registered_methods::<RawIdentifierScript>()
        .into_iter()
        .next()
        .expect("raw identifier method");
    assert_eq!(method.name, "type");
    assert_eq!(method.id, godot_rs::script::method_id("type"));
}

#[test]
fn script_base_proxy_uses_generated_godot_methods() {
    let player = Player::__godot_rs_new();
    let base = player.base();
    assert!(format!("{base:?}").contains("CharacterBody2D"));

    let error = base
        .set_process(true)
        .expect_err("unit test has no initialized Host");
    assert_eq!(error.kind(), godot_rs::error::EngineErrorKind::Unsupported);
}

#[test]
fn impl_macro_collects_only_godot_facing_methods() {
    let methods = registered_methods::<Player>();
    assert_eq!(methods.len(), 16);
    assert!(!methods.iter().any(|method| method.name == "internal_only"));
    assert!(
        methods
            .windows(2)
            .all(|pair| (pair[0].id, pair[0].name) < (pair[1].id, pair[1].name))
    );

    let ready = methods
        .iter()
        .find(|method| method.name == "_ready")
        .expect("ready descriptor");
    assert_eq!(ready.kind, MethodKind::Lifecycle(LifecycleSlot::Ready));
    assert_eq!(ready.argument_count, 0);
    assert_eq!(ready.id, godot_rs::script::method_id("_ready"));

    let rpc = methods
        .iter()
        .find(|method| method.name == "sync_state")
        .expect("RPC descriptor");
    assert_eq!(rpc.kind, MethodKind::Rpc);
    assert!(rpc.options.contains("authority"));
    assert!(rpc.options.contains("reliable"));

    let take_damage = methods
        .iter()
        .find(|method| method.name == "take_damage")
        .expect("function descriptor");
    assert_eq!(
        take_damage.argument_types,
        &[godot_rs::abi::AbiValueType::I64]
    );
    assert_eq!(take_damage.arguments.len(), 1);
    assert_eq!(take_damage.arguments[0].name, "amount");
    assert_eq!(
        take_damage.arguments[0].type_,
        godot_rs::abi::AbiValueType::I64
    );
    assert_eq!(take_damage.return_type, godot_rs::abi::AbiValueType::NIL);

    let current = methods
        .iter()
        .find(|method| method.name == "current_counter")
        .expect("returning function descriptor");
    assert_eq!(current.return_type, godot_rs::abi::AbiValueType::I64);

    let greet = methods
        .iter()
        .find(|method| method.name == "greet")
        .expect("String function descriptor");
    assert_eq!(greet.argument_types, &[godot_rs::abi::AbiValueType::STRING]);
    assert_eq!(greet.return_type, godot_rs::abi::AbiValueType::STRING);
    assert_eq!(greet.default_arguments.len(), 1);
    assert!(!greet.vararg);
    let mut default_output = godot_rs::abi::AbiValueV1::NIL;
    // SAFETY: Generated default callbacks initialize one ABI output value.
    let default_result = unsafe {
        greet.default_arguments[0].expect("generated default callback")(&mut default_output)
    };
    assert_eq!(default_result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(default_output.type_, godot_rs::abi::AbiValueType::STRING);
    // SAFETY: This is the sole release of the generated owned default.
    let release_status = unsafe { godot_rs::module::drop_owned_value(default_output) };
    assert_eq!(release_status, godot_rs::abi::AbiStatus::Ok);
    let count_variants = methods
        .iter()
        .find(|method| method.name == "count_variants")
        .expect("variable-argument descriptor");
    assert!(count_variants.argument_types.is_empty());
    assert!(count_variants.default_arguments.is_empty());
    assert!(count_variants.vararg);

    let offset_point = methods
        .iter()
        .find(|method| method.name == "offset_point")
        .expect("Vector2 function descriptor");
    assert_eq!(
        offset_point.argument_types,
        &[
            godot_rs::abi::AbiValueType::VECTOR2,
            godot_rs::abi::AbiValueType::VECTOR2,
        ]
    );
    assert_eq!(
        offset_point.return_type,
        godot_rs::abi::AbiValueType::VECTOR2
    );

    let offset_position = methods
        .iter()
        .find(|method| method.name == "offset_position")
        .expect("Vector3 function descriptor");
    assert_eq!(
        offset_position.argument_types,
        &[
            godot_rs::abi::AbiValueType::VECTOR3,
            godot_rs::abi::AbiValueType::VECTOR3,
        ]
    );
    assert_eq!(
        offset_position.return_type,
        godot_rs::abi::AbiValueType::VECTOR3
    );

    let modulate = methods
        .iter()
        .find(|method| method.name == "modulate")
        .expect("Color function descriptor");
    assert_eq!(
        modulate.argument_types,
        &[
            godot_rs::abi::AbiValueType::COLOR,
            godot_rs::abi::AbiValueType::COLOR,
        ]
    );
    assert_eq!(modulate.return_type, godot_rs::abi::AbiValueType::COLOR);

    let mirror_node = methods
        .iter()
        .find(|method| method.name == "mirror_node")
        .expect("object function descriptor");
    assert_eq!(
        mirror_node.argument_types,
        &[godot_rs::abi::AbiValueType::OBJECT_ID]
    );
    assert_eq!(mirror_node.arguments[0].class_name, Some("Node"));

    let mirror_process_mode = methods
        .iter()
        .find(|method| method.name == "mirror_process_mode")
        .expect("enum function descriptor");
    assert_eq!(
        mirror_process_mode.argument_types,
        &[godot_rs::abi::AbiValueType::I64]
    );
    assert_eq!(
        mirror_process_mode.return_type,
        godot_rs::abi::AbiValueType::I64
    );

    let mirror_process_flags = methods
        .iter()
        .find(|method| method.name == "mirror_process_flags")
        .expect("bitfield function descriptor");
    assert_eq!(
        mirror_process_flags.argument_types,
        &[godot_rs::abi::AbiValueType::U64]
    );
    assert_eq!(
        mirror_process_flags.return_type,
        godot_rs::abi::AbiValueType::U64
    );

    let rpc = rpc.rpc.expect("structured RPC configuration");
    assert_eq!(rpc.mode, godot_rs::script::RpcMode::Authority);
    assert!(rpc.call_local);
    assert_eq!(
        rpc.transfer_mode,
        godot_rs::script::RpcTransferMode::Reliable
    );
    assert_eq!(rpc.channel, 0);
}

#[test]
fn multiple_script_impl_blocks_share_one_dispatch_table() {
    let mut player = Player::__godot_rs_new();
    let state = (&mut player as *mut Player).cast();
    let arguments = [
        godot_rs::abi::AbiValueV1::from_i64(80),
        godot_rs::abi::AbiValueV1::from_i64(55),
    ];
    let mut output = godot_rs::abi::AbiValueV1::NIL;

    // SAFETY: State, arguments and output match the generated callback ABI.
    let result = unsafe {
        godot_rs::script::abi_call_method::<Player>(
            state,
            godot_rs::script::method_id("on_health_changed"),
            arguments.as_ptr(),
            arguments.len() as u32,
            &mut output,
        )
    };

    assert_eq!(result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(player.transient_counter, -25);
}

#[test]
fn reflected_object_methods_preserve_nullable_instance_ids() {
    let mut player = Player::__godot_rs_new();
    let state = (&mut player as *mut Player).cast();
    let arguments = [godot_rs::abi::AbiValueV1::from_object_id(42)];
    let mut output = godot_rs::abi::AbiValueV1::NIL;

    // SAFETY: State, object ID and output follow the generated method ABI.
    let result = unsafe {
        godot_rs::script::abi_call_method::<Player>(
            state,
            godot_rs::script::method_id("mirror_node"),
            arguments.as_ptr(),
            arguments.len() as u32,
            &mut output,
        )
    };

    assert_eq!(result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(output.type_, godot_rs::abi::AbiValueType::OBJECT_ID);
    assert_eq!(output.payload, [42, 0]);
}

#[test]
fn reflected_enums_and_bitfields_preserve_unknown_compatible_values() {
    let mut player = Player::__godot_rs_new();
    let state = (&mut player as *mut Player).cast();
    let mut output = godot_rs::abi::AbiValueV1::NIL;

    let mode = node::ProcessMode::from(99);
    let arguments = [godot_rs::abi::AbiValueV1::from_i64(i64::from(mode))];
    // SAFETY: State, enum argument and output match the generated method ABI.
    let result = unsafe {
        godot_rs::script::abi_call_method::<Player>(
            state,
            godot_rs::script::method_id("mirror_process_mode"),
            arguments.as_ptr(),
            1,
            &mut output,
        )
    };
    assert_eq!(result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(output, arguments[0]);

    let flags = node::ProcessThreadMessages::from_bits_retain(1_u64 << 63);
    let arguments = [godot_rs::abi::AbiValueV1::from_u64(flags.bits())];
    // SAFETY: State, bitfield argument and output match the generated method ABI.
    let result = unsafe {
        godot_rs::script::abi_call_method::<Player>(
            state,
            godot_rs::script::method_id("mirror_process_flags"),
            arguments.as_ptr(),
            1,
            &mut output,
        )
    };
    assert_eq!(result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(output, arguments[0]);
}

#[test]
fn lifecycle_slots_dispatch_without_string_lookup() {
    let mut player = Player::__godot_rs_new();
    let state = (&mut player as *mut Player).cast();

    let ready = registered_methods::<Player>()
        .iter()
        .find_map(|method| match method.callback {
            MethodCallback::Lifecycle0(callback)
                if method.kind == MethodKind::Lifecycle(LifecycleSlot::Ready) =>
            {
                Some(callback)
            }
            _ => None,
        })
        .expect("ready callback");
    // SAFETY: `state` points to the matching live Player for this generated slot.
    assert_eq!(unsafe { ready(state) }.status, godot_rs::abi::AbiStatus::Ok);
    assert!(player.ready);

    let physics = registered_methods::<Player>()
        .iter()
        .find_map(|method| match method.callback {
            MethodCallback::LifecycleF64(callback)
                if method.kind == MethodKind::Lifecycle(LifecycleSlot::PhysicsProcess) =>
            {
                Some(callback)
            }
            _ => None,
        })
        .expect("physics callback");
    // SAFETY: `state` remains a unique pointer to the matching live Player.
    let physics_status = unsafe { physics(state, 3.0) }.status;
    assert_eq!(physics_status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(player.transient_counter, 10);
}

#[test]
fn exported_fields_dispatch_by_validated_index() {
    let mut player = Player::__godot_rs_new();
    let mut output = godot_rs::abi::AbiValueV1::NIL;
    // SAFETY: Index zero is the generated `speed` export and output is live.
    let get = unsafe { player.__godot_rs_get_field(0, &mut output) };
    assert_eq!(get.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(output, godot_rs::abi::AbiValueV1::from_f64(240.0));

    // SAFETY: The fixed value type matches the generated f32 field schema.
    let set = unsafe { player.__godot_rs_set_field(0, godot_rs::abi::AbiValueV1::from_f64(480.0)) };
    assert_eq!(set.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(player.speed, 480.0);

    // SAFETY: Unsupported indices are rejected without accessing state.
    let rejected = unsafe { player.__godot_rs_get_field(1, &mut output) };
    assert_eq!(rejected.status, godot_rs::abi::AbiStatus::Unsupported);

    // SAFETY: Index eight is the generated String export and output is live.
    let get = unsafe { player.__godot_rs_get_field(8, &mut output) };
    assert_eq!(get.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(output.type_, godot_rs::abi::AbiValueType::STRING);
    assert_eq!(abi_value_text(output), "等待构建");
    // SAFETY: This is the sole release of the generated owned field value.
    let release = unsafe { godot_rs::module::drop_owned_value(output) };
    assert_eq!(release, godot_rs::abi::AbiStatus::Ok);

    let replacement = String::from("编译成功");
    // SAFETY: The borrowed UTF-8 value remains live for this synchronous set.
    let set = unsafe {
        player.__godot_rs_set_field(
            8,
            godot_rs::abi::AbiValueV1::from_borrowed_utf8(&replacement),
        )
    };
    assert_eq!(set.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(player.status_text, "编译成功");
}

#[test]
fn lifecycle_panics_are_contained_at_the_c_abi() {
    let mut script = PanickingScript::__godot_rs_new();
    let state = (&mut script as *mut PanickingScript).cast();
    let ready = registered_methods::<PanickingScript>()
        .iter()
        .find_map(|method| match method.callback {
            MethodCallback::Lifecycle0(callback) => Some(callback),
            _ => None,
        })
        .expect("ready callback");

    // SAFETY: `state` points to the generated callback's matching script type.
    let result = unsafe { ready(state) };
    assert_eq!(result.status, godot_rs::abi::AbiStatus::Panic);
    assert_eq!(abi_text(result.message), "Rust script callback panicked");
}

#[test]
fn reflected_method_panics_are_contained_at_the_c_abi() {
    let mut script = PanickingScript::__godot_rs_new();
    let state = (&mut script as *mut PanickingScript).cast();
    let mut output = godot_rs::abi::AbiValueV1::NIL;

    // SAFETY: State and output match the generated PanickingScript method ABI.
    let result = unsafe {
        godot_rs::script::abi_call_method::<PanickingScript>(
            state,
            godot_rs::script::method_id("explode"),
            core::ptr::null(),
            0,
            &mut output,
        )
    };
    assert_eq!(result.status, godot_rs::abi::AbiStatus::Panic);
    assert_eq!(abi_text(result.message), "Rust script callback panicked");
}

#[test]
fn reflected_methods_copy_utf8_inputs_and_return_owned_utf8() {
    let mut player = Player::__godot_rs_new();
    let state = (&mut player as *mut Player).cast();
    let greet = registered_methods::<Player>()
        .into_iter()
        .find(|method| method.name == "greet")
        .expect("greet descriptor");
    let name = String::from("世界");
    let arguments = [godot_rs::abi::AbiValueV1::from_borrowed_utf8(&name)];
    let mut output = godot_rs::abi::AbiValueV1::NIL;

    // SAFETY: State, method ID, borrowed argument and output all follow the
    // generated Player method contract for this synchronous call.
    let result = unsafe {
        godot_rs::script::abi_call_method::<Player>(
            state,
            greet.id,
            arguments.as_ptr(),
            arguments.len() as u32,
            &mut output,
        )
    };
    assert_eq!(result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(player.last_name, "世界");
    assert_eq!(output.type_, godot_rs::abi::AbiValueType::STRING);
    assert_eq!(output.reserved_flags, godot_rs::abi::ABI_VALUE_OWNED_UTF8);
    // SAFETY: The owned output promises this live buffer until its release.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            output.payload[0] as usize as *const u8,
            output.payload[1] as usize,
        )
    };
    assert_eq!(core::str::from_utf8(bytes), Ok("你好，世界"));
    // SAFETY: This is the sole release of the value returned above.
    let release = unsafe { godot_rs::module::drop_owned_value(output) };
    assert_eq!(release, godot_rs::abi::AbiStatus::Ok);
}

#[test]
fn generated_module_exports_c_abi_descriptors_and_state() {
    use core::ffi::c_void;
    use core::mem::MaybeUninit;
    use core::ptr;
    use godot_rs::abi::{
        AbiByteSlice, AbiCallResult, AbiHeader, AbiLogLevel, AbiStatus, AbiValueType, AbiValueV1,
        HostApiV1, ModuleApiV1,
    };

    #[derive(Default)]
    struct SignalCapture {
        field_index: u32,
        arguments: Vec<AbiValueV1>,
        strings: Vec<String>,
    }

    unsafe extern "C" fn ignore_log(
        _context: *mut c_void,
        _level: AbiLogLevel,
        _target: AbiByteSlice,
        _message: AbiByteSlice,
    ) {
    }

    unsafe extern "C" fn capture_signal(
        context: *mut c_void,
        field_index: u32,
        arguments: *const AbiValueV1,
        argument_count: u32,
    ) -> AbiCallResult {
        if context.is_null() || (argument_count != 0 && arguments.is_null()) {
            return AbiCallResult::failure(AbiStatus::InvalidArgument, "invalid test capture");
        }
        // SAFETY: This synchronous test callback receives its live capture and
        // argument slice from the generated SDK bridge.
        let capture = unsafe { &mut *context.cast::<SignalCapture>() };
        capture.field_index = field_index;
        // SAFETY: Null was rejected for non-empty slices and the SDK keeps the
        // fixed scalar buffer live for the duration of this callback.
        let arguments = unsafe { core::slice::from_raw_parts(arguments, argument_count as usize) };
        capture.arguments = arguments.to_vec();
        capture.strings = arguments
            .iter()
            .filter(|value| value.type_ == AbiValueType::STRING)
            .map(|value| {
                // SAFETY: Signal strings remain borrowed for this synchronous
                // Host callback and their lengths came from the SDK.
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        value.payload[0] as usize as *const u8,
                        value.payload[1] as usize,
                    )
                };
                core::str::from_utf8(bytes)
                    .expect("signal argument UTF-8")
                    .to_owned()
            })
            .collect();
        AbiCallResult::OK
    }

    let mut signal_capture = SignalCapture::default();
    let mut reserved = [0; 16];
    reserved[0] = capture_signal as *const () as usize;
    let host = HostApiV1 {
        header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
        context: ptr::from_mut(&mut signal_capture).cast(),
        log: Some(ignore_log),
        reserved,
    };
    let mut module = MaybeUninit::<ModuleApiV1>::uninit();
    // SAFETY: Both local ABI tables are live and correctly sized.
    let status = unsafe { godot_rs_module_entry(&host, module.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: A successful entry call initialized the whole table.
    let module = unsafe { module.assume_init() };
    assert_eq!(module.script_count, 1);
    assert_ne!(
        module.reserved_flags & godot_rs::abi::ABI_MODULE_EXTENSION_GODOT_API,
        0
    );
    assert_eq!(
        module.reserved[godot_rs::abi::MODULE_API_SLOT_GODOT_API_MAJOR],
        4
    );
    let selected_minor = godot_rs::GODOT_API
        .strip_prefix("4.")
        .expect("Godot 4 API")
        .parse::<usize>()
        .expect("Godot API minor");
    assert_eq!(
        module.reserved[godot_rs::abi::MODULE_API_SLOT_GODOT_API_MINOR],
        selected_minor
    );

    let mut script = MaybeUninit::uninit();
    // SAFETY: The generated getter receives a valid index and output slot.
    let status = unsafe { module.get_script.expect("script getter")(0, script.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let script = unsafe { script.assume_init() };
    assert_eq!(abi_text(script.source_path), "res://src/scripts/player.rs");
    assert_ne!(
        script.reserved_flags & godot_rs::abi::ABI_SCRIPT_EXTENSION_RESOURCE_UID,
        0
    );
    assert_eq!(
        godot_rs::abi::decode_resource_uid_words([script.reserved[2], script.reserved[3]]),
        Some(95)
    );
    assert_eq!(abi_text(script.base), "CharacterBody2D");
    assert_eq!(script.field_count, 28);
    assert_eq!(script.method_count, 16);

    let mut field = MaybeUninit::uninit();
    // SAFETY: Index zero and output slot are valid for this descriptor.
    let status = unsafe { script.get_field.expect("field getter")(0, field.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let field = unsafe { field.assume_init() };
    assert_eq!(abi_text(field.name), "speed");
    assert_eq!(
        field.reserved_extension_flags,
        godot_rs::abi::ABI_FIELD_EXTENSION_PROPERTY_SCHEMA
    );
    assert_eq!(
        field.reserved[0],
        godot_rs::abi::AbiPropertyType::FLOAT.0 as usize
    );
    assert_eq!(
        field.reserved[1],
        godot_rs::abi::ABI_PROPERTY_HINT_RANGE as usize
    );
    assert_eq!(
        field.reserved[2],
        godot_rs::abi::ABI_PROPERTY_USAGE_SCRIPT_DEFAULT as usize
    );
    assert_ne!(field.reserved[3], 0);

    let mut enum_field = MaybeUninit::uninit();
    // SAFETY: Index seventeen is the generated enum export field.
    let status = unsafe { script.get_field.expect("field getter")(17, enum_field.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let enum_field = unsafe { enum_field.assume_init() };
    assert_eq!(abi_text(enum_field.name), "worker_mode");
    assert_eq!(
        enum_field.reserved_extension_flags,
        godot_rs::abi::ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA
    );
    assert_eq!(enum_field.reserved[0], 1);
    assert_eq!(
        enum_field.reserved[2],
        <node::ProcessMode as godot_rs::engine::GodotIntegerValue>::PROPERTY_OPTIONS.len()
    );
    // SAFETY: The specialized field extension stores this exact generated C
    // function pointer for the lifetime of the project module.
    let default: godot_rs::abi::AbiGodotIntegerDefaultFn =
        unsafe { core::mem::transmute(enum_field.reserved[3]) };
    // SAFETY: The generated callback takes no arguments and returns raw bits.
    assert_eq!(unsafe { default() }, 3);

    let mut fixed_math_field = MaybeUninit::uninit();
    // SAFETY: Index twenty-two is the generated Transform2D export field.
    let status =
        unsafe { script.get_field.expect("field getter")(22, fixed_math_field.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let fixed_math_field = unsafe { fixed_math_field.assume_init() };
    assert_eq!(abi_text(fixed_math_field.name), "canvas_transform");
    assert_eq!(
        fixed_math_field.reserved[0],
        godot_rs::abi::AbiPropertyType::TRANSFORM2D.0 as usize
    );
    // SAFETY: Fixed math property descriptors keep this static ABI value live.
    let default =
        unsafe { &*(fixed_math_field.reserved[3] as *const godot_rs::abi::AbiFixedMathDefaultV1) };
    assert_eq!(default.component_count, 6);
    assert_eq!(
        default.component_bits[..6]
            .iter()
            .copied()
            .map(f32::from_bits)
            .collect::<Vec<_>>(),
        [1.0, 0.0, 0.0, 1.0, 12.0, -6.0]
    );

    let mut node_field = MaybeUninit::uninit();
    // SAFETY: Index one is the generated required `sprite` node field.
    let status = unsafe { script.get_field.expect("field getter")(1, node_field.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let node_field = unsafe { node_field.assume_init() };
    assert_eq!(abi_text(node_field.name), "sprite");
    assert_eq!(
        node_field.reserved_extension_flags,
        godot_rs::abi::ABI_FIELD_EXTENSION_NODE_SCHEMA
    );
    assert_eq!(
        abi_text(AbiByteSlice {
            ptr: node_field.reserved[0] as *const u8,
            len: node_field.reserved[1],
        }),
        "%Sprite"
    );
    let (class_length, optional) = godot_rs::abi::decode_node_field_class(node_field.reserved[3]);
    assert!(!optional);
    assert_eq!(
        abi_text(AbiByteSlice {
            ptr: node_field.reserved[2] as *const u8,
            len: class_length,
        }),
        "AnimatedSprite2D"
    );

    let mut signal_field = MaybeUninit::uninit();
    // SAFETY: Index three is the generated `health_changed` field.
    let status = unsafe { script.get_field.expect("field getter")(3, signal_field.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let signal_field = unsafe { signal_field.assume_init() };
    assert_eq!(abi_text(signal_field.name), "health_changed");
    assert_eq!(
        signal_field.reserved_extension_flags,
        godot_rs::abi::ABI_FIELD_EXTENSION_SIGNAL_SCHEMA
    );
    assert_ne!(signal_field.reserved[0], 0);
    assert_eq!(signal_field.reserved[1], 2);
    assert_eq!(signal_field.reserved[2..], [0; 2]);

    let mut string_field = MaybeUninit::uninit();
    // SAFETY: Index eight is the generated `status_text` String export.
    let status = unsafe { script.get_field.expect("field getter")(8, string_field.as_mut_ptr()) };
    assert_eq!(status, AbiStatus::Ok);
    // SAFETY: The successful getter initialized the descriptor.
    let string_field: godot_rs::abi::AbiFieldDescriptorV1 = unsafe { string_field.assume_init() };
    assert_eq!(
        string_field.reserved[0],
        godot_rs::abi::AbiPropertyType::STRING.0 as usize
    );
    assert_eq!(
        string_field.reserved[1],
        godot_rs::abi::ABI_PROPERTY_HINT_MULTILINE_TEXT as usize
    );
    assert_eq!(string_field.reserved[3], 0);
    assert_eq!(abi_text(string_field.default_value), "等待构建");

    let mut state = ptr::null_mut();
    // SAFETY: The state output slot is writable.
    let created = unsafe { script.create_state.expect("state creator")(&mut state) };
    assert_eq!(created.status, AbiStatus::Ok);
    assert!(!state.is_null());
    // SAFETY: The generated node setter receives the matching live Player and
    // one validated Object ID value from the Host.
    let set_node = unsafe {
        (*state.cast::<Player>()).__godot_rs_set_field(1, AbiValueV1::from_object_id(42))
    };
    assert_eq!(set_node.status, AbiStatus::Ok);
    // SAFETY: The state remains the live generated Player.
    assert_eq!(unsafe { &*state.cast::<Player>() }.sprite.instance_id(), 42);
    // SAFETY: The generated state is a live Player until the drop callback.
    unsafe { &*state.cast::<Player>() }
        .health_changed
        .emit((100, 90));
    assert_eq!(signal_capture.field_index, 3);
    assert_eq!(signal_capture.arguments.len(), 2);
    assert_eq!(signal_capture.arguments[0].type_, AbiValueType::I64);
    assert_eq!(signal_capture.arguments[0].payload[0] as i64, 100);
    assert_eq!(signal_capture.arguments[1].type_, AbiValueType::I64);
    assert_eq!(signal_capture.arguments[1].payload[0] as i64, 90);

    // SAFETY: State was created for Player and remains live until its paired
    // drop callback at the end of this test.
    let player = unsafe { &*state.cast::<Player>() };
    player.message_sent.emit((String::from("保存成功"),));
    assert_eq!(signal_capture.field_index, 4);
    assert_eq!(signal_capture.strings, ["保存成功"]);
    // SAFETY: The generated state remains live and its signal field carries
    // inline, allocation-free math ABI values.
    unsafe { &*state.cast::<Player>() }.motion_changed.emit((
        Vector2::new(1.0, 2.0),
        Vector3::new(3.0, 4.0, 5.0),
        Color::rgba(0.25, 0.5, 0.75, 1.0),
    ));
    assert_eq!(signal_capture.field_index, 27);
    assert_eq!(signal_capture.arguments[0].vector2(), Some([1.0, 2.0]));
    assert_eq!(signal_capture.arguments[1].vector3(), Some([3.0, 4.0, 5.0]));
    assert_eq!(
        signal_capture.arguments[2].color(),
        Some([0.25, 0.5, 0.75, 1.0])
    );
    // SAFETY: The generated lifecycle table and state type match.
    let ready = unsafe { script.lifecycle.ready.expect("ready slot")(state) };
    assert_eq!(ready.status, AbiStatus::Ok);

    let take_damage = find_abi_method(&script, "take_damage");
    assert_eq!(
        take_damage.struct_size,
        godot_rs::abi::AbiMethodDescriptorV1::MINIMUM_SIZE
    );
    assert_eq!(take_damage.arguments.len, 1);
    // SAFETY: The generated argument slice is static for the module lifetime.
    let take_damage_argument = unsafe { *take_damage.arguments.ptr };
    assert_eq!(abi_text(take_damage_argument.name), "amount");
    assert_eq!(take_damage_argument.type_, godot_rs::abi::AbiValueType::I64);

    let greet = find_abi_method(&script, "greet");
    assert_eq!(
        greet.reserved_extension_flags,
        godot_rs::abi::ABI_METHOD_EXTENSION_SCHEMA_V1
    );
    assert_ne!(greet.reserved[0], 0);
    assert_eq!(
        greet.reserved[1],
        godot_rs::abi::AbiMethodExtensionsV1::MINIMUM_SIZE as usize
    );
    // SAFETY: The generated versioned method extension is static for the
    // complete project-module lifetime.
    let greet_extensions =
        unsafe { &*(greet.reserved[0] as *const godot_rs::abi::AbiMethodExtensionsV1) };
    assert_eq!(greet_extensions.default_arguments.len, 1);
    assert_eq!(greet_extensions.reserved_flags, 0);

    let count_variants = find_abi_method(&script, "count_variants");
    // SAFETY: The same generated extension contract applies to every method.
    let count_extensions =
        unsafe { &*(count_variants.reserved[0] as *const godot_rs::abi::AbiMethodExtensionsV1) };
    assert_ne!(
        count_extensions.reserved_flags & godot_rs::abi::ABI_METHOD_SCHEMA_VARARG,
        0
    );
    assert_eq!(count_variants.argument_count, 0);
    let mut output = godot_rs::abi::AbiValueV1::NIL;
    let argument = godot_rs::abi::AbiValueV1::from_i64(3);
    // SAFETY: State, method ID, argument and output belong to this descriptor.
    let called = unsafe {
        script.call_method.expect("method callback")(
            state,
            take_damage.id,
            &argument,
            1,
            &mut output,
        )
    };
    assert_eq!(called.status, AbiStatus::Ok);
    assert_eq!(output, godot_rs::abi::AbiValueV1::NIL);

    let current = find_abi_method(&script, "current_counter");
    // SAFETY: The zero-argument call uses the same live state and output slot.
    let called = unsafe {
        script.call_method.expect("method callback")(state, current.id, ptr::null(), 0, &mut output)
    };
    assert_eq!(called.status, AbiStatus::Ok);
    assert_eq!(output.type_, godot_rs::abi::AbiValueType::I64);
    assert_eq!(output.payload[0] as i64, 4);

    let offset_point = find_abi_method(&script, "offset_point");
    let arguments = [
        godot_rs::abi::AbiValueV1::from_vector2(10.0, -4.0),
        godot_rs::abi::AbiValueV1::from_vector2(2.5, 9.0),
    ];
    // SAFETY: Both packed Vector2 arguments match the generated descriptor.
    let called = unsafe {
        script.call_method.expect("method callback")(
            state,
            offset_point.id,
            arguments.as_ptr(),
            2,
            &mut output,
        )
    };
    assert_eq!(called.status, AbiStatus::Ok);
    assert_eq!(output.vector2(), Some([12.5, 5.0]));

    let modulate = find_abi_method(&script, "modulate");
    let arguments = [
        godot_rs::abi::AbiValueV1::from_color(0.8, 0.5, 0.25, 1.0),
        godot_rs::abi::AbiValueV1::from_color(0.5, 0.25, 1.0, 0.75),
    ];
    // SAFETY: Both packed Color arguments match the generated descriptor.
    let called = unsafe {
        script.call_method.expect("method callback")(
            state,
            modulate.id,
            arguments.as_ptr(),
            2,
            &mut output,
        )
    };
    assert_eq!(called.status, AbiStatus::Ok);
    assert_eq!(output.color(), Some([0.4, 0.125, 0.25, 0.75]));

    let sync_state = find_abi_method(&script, "sync_state");
    assert_eq!(sync_state.rpc.present, 1);
    assert_eq!(sync_state.rpc.call_local, 1);
    assert_eq!(sync_state.rpc.mode, godot_rs::abi::AbiRpcMode::AUTHORITY);
    assert_eq!(
        sync_state.rpc.transfer_mode,
        godot_rs::abi::AbiRpcTransferMode::RELIABLE
    );

    let mirror_node = find_abi_method(&script, "mirror_node");
    assert_eq!(
        mirror_node.reserved_extension_flags,
        godot_rs::abi::ABI_METHOD_EXTENSION_SCHEMA_V1
    );
    // SAFETY: The generated method descriptor retains static versioned
    // metadata for the complete module lifetime.
    let mirror_extensions =
        unsafe { &*(mirror_node.reserved[0] as *const godot_rs::abi::AbiMethodExtensionsV1) };
    assert_eq!(mirror_extensions.argument_classes.len, 1);
    // SAFETY: The extension's argument class slice has the declared length.
    let class_name = unsafe { *mirror_extensions.argument_classes.ptr };
    assert_eq!(abi_text(class_name), "Node");
    assert_eq!(abi_text(mirror_extensions.return_class), "Node");

    let synchronized = godot_rs::abi::AbiValueV1::from_i64(99);
    // SAFETY: The generated RPC method uses the same scalar ABI contract.
    let called = unsafe {
        script.call_method.expect("method callback")(
            state,
            sync_state.id,
            &synchronized,
            1,
            &mut output,
        )
    };
    assert_eq!(called.status, AbiStatus::Ok);
    // SAFETY: The state is the live Player allocated by the descriptor.
    assert_eq!(unsafe { &*state.cast::<Player>() }.transient_counter, 99);

    let wrong_argument = godot_rs::abi::AbiValueV1::from_bool(true);
    // SAFETY: The deliberately wrong typed value still has a valid ABI layout.
    let rejected = unsafe {
        script.call_method.expect("method callback")(
            state,
            take_damage.id,
            &wrong_argument,
            1,
            &mut output,
        )
    };
    assert_eq!(rejected.status, AbiStatus::InvalidArgument);
    // SAFETY: The generated drop slot owns this state exactly once.
    unsafe { script.drop_state.expect("state dropper")(state) };
    // SAFETY: Shutdown receives the generated module context.
    let shutdown = unsafe { module.shutdown.expect("shutdown")(module.context) };
    assert_eq!(shutdown, AbiStatus::Ok);
}

fn find_abi_method(
    script: &godot_rs::abi::AbiScriptDescriptorV1,
    expected_name: &str,
) -> godot_rs::abi::AbiMethodDescriptorV1 {
    for index in 0..script.method_count {
        let mut method = core::mem::MaybeUninit::zeroed();
        // SAFETY: The index is bounded by this descriptor's method count.
        let status =
            unsafe { script.get_method.expect("method getter")(index, method.as_mut_ptr()) };
        assert_eq!(status, godot_rs::abi::AbiStatus::Ok);
        // SAFETY: The successful getter initialized the output.
        let method = unsafe { method.assume_init() };
        if abi_text(method.name) == expected_name {
            return method;
        }
    }
    panic!("missing ABI method descriptor `{expected_name}`");
}

fn abi_text(value: godot_rs::abi::AbiByteSlice) -> &'static str {
    if value.len == 0 {
        return "";
    }
    // SAFETY: Generated descriptors borrow static, validated UTF-8 literals.
    let bytes = unsafe { core::slice::from_raw_parts(value.ptr, value.len) };
    // SAFETY: Macro-generated descriptor strings originate as Rust UTF-8.
    unsafe { core::str::from_utf8_unchecked(bytes) }
}

fn abi_value_text(value: godot_rs::abi::AbiValueV1) -> String {
    // SAFETY: Tests call this before releasing an SDK-produced owned UTF-8
    // value, and its length comes from the generated output.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            value.payload[0] as usize as *const u8,
            value.payload[1] as usize,
        )
    };
    core::str::from_utf8(bytes)
        .expect("ABI value UTF-8")
        .to_owned()
}
