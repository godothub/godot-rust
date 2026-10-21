use godot_rs::prelude::*;
use godot_rs::script::{ScriptClass, registered_methods};

struct Scratch(Vec<i64>);

#[script(base = Node)]
struct Health {
    #[export(default = 100)]
    points: i64,

    #[init(default = Scratch(Vec::with_capacity(16)))]
    scratch: Scratch,

    #[signal]
    damaged: Signal<fn(amount: i64, remaining: i64)>,

    #[property]
    parent: Option<NodeRef<Self>>,

    #[signal]
    linked: Signal<fn(target: NodeRef<Self>)>,
}

#[script]
impl Health {
    #[func]
    fn damage(&mut self, amount: i64) -> EngineResult<i64> {
        self.points = (self.points - amount).max(0);
        self.scratch.0.push(amount);
        self.damaged.emit(amount, self.points)?;
        Ok(self.points)
    }

    #[func(name = "formatted_label")]
    fn label(&self, prefix: String) -> String {
        format!("{prefix}: {}", self.points)
    }

    #[func]
    fn current(&self) -> EngineResult<i64> {
        Ok(self.points)
    }

    #[func]
    fn same(&self, other: NodeRef<Self>) -> NodeRef<Self> {
        other
    }

    #[func]
    fn configure(&mut self, config: GodotRef<WeaponConfig>) -> GodotRef<WeaponConfig> {
        config
    }
}

#[script(base = Resource)]
struct WeaponConfig {
    #[export(default = 25)]
    damage: i64,

    #[property(default = 1, read_only)]
    level: i64,
}

#[script]
impl WeaponConfig {
    #[func]
    fn related(
        &self,
        configs: Array<Array<GodotRef<Self>>>,
        _fallback: Option<GodotRef<Self>>,
    ) -> Array<Array<GodotRef<Self>>> {
        configs
    }
}

#[script(base = Node2D)]
struct Player {
    #[node("%Health")]
    health: NodeRef<Health>,

    #[node("%Camera")]
    camera: Option<NodeRef<Camera2D>>,

    #[export(group = "装备")]
    weapon: Option<GodotRef<WeaponConfig>>,

    #[export]
    weapons: Array<GodotRef<WeaponConfig>>,

    #[export]
    equipment_sets: Array<Array<GodotRef<WeaponConfig>>>,

    #[export(default = 240.0, group = "Movement", range(min = 0, max = 1000))]
    speed: f64,

    #[property(default = 1, read_only)]
    level: i64,

    #[init(default = Vec::with_capacity(32))]
    recent_hits: Vec<i64>,
}

#[script]
impl Player {
    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let direction = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
        self.base()
            .translate(direction * (self.speed * delta) as f32)?;
        Ok(())
    }

    fn _ready(&mut self) -> EngineResult<()> {
        let _ = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
        let _ = Input::is_action_pressed("ui_accept")?;
        let input = Input::singleton()?;
        let _ = input
            .is_action_pressed_ex("ui_accept")
            .exact_match(true)
            .done()?;

        self.health
            .signals()
            .damaged()
            .connect_deferred(&self.callbacks()?.on_damaged())?;
        let _ = self.health.damage(10)?;
        let _ = self.health.properties().points()?;
        self.health.properties().set_points(100)?;
        let _ = self.base().node::<Health>("%Health")?;

        let weapon = load::<WeaponConfig>("res://weapons/sword.tres")?;
        let _ = weapon.properties().damage()?;
        let damaged = self.health.signals().damaged().wait();
        self.tasks().spawn(async move {
            match damaged.await {
                Ok((amount, remaining)) => godot_print!("伤害 {amount}，剩余 {remaining}"),
                Err(error) => godot_warn!("等待中断：{error}"),
            }
        });
        Ok(())
    }

    #[func]
    fn on_damaged(&mut self, amount: i64, remaining: i64) {
        godot_print!("受到 {amount} 点伤害，剩余 {remaining}");
    }
}

mod readme_quickstart {
    use godot_rs::prelude::*;

    #[script(base = Node2D)]
    pub struct Player;

    #[script]
    impl Player {
        fn _process(&mut self, delta: f64) -> EngineResult<()> {
            let direction = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
            self.base().translate(direction * (240.0 * delta) as f32)?;
            Ok(())
        }
    }
}

#[test]
fn declarations_preserve_script_types_and_initialize_plain_state() {
    let mut health = Health::__godot_rs_new();
    assert_eq!(health.points, 100);
    assert_eq!(health.scratch.0.capacity(), 16);
    let _ = &health.damaged;
    assert_eq!(health.linked.name(), "linked");
    assert!(health.__godot_rs_tasks.get().is_none());
    assert!(std::ptr::eq(health.tasks(), health.tasks()));
    let damaged = Health::DESCRIPTOR.fields[2].signal.expect("signal schema");
    assert_eq!(damaged.arguments[0].name, "amount");
    assert_eq!(damaged.arguments[0].type_, godot_rs::abi::AbiValueType::I64);
    assert_eq!(damaged.arguments[1].name, "remaining");
    assert_eq!(
        Health::DESCRIPTOR.fields[3]
            .property
            .expect("self property")
            .hint_string,
        "Node"
    );

    let fields = Player::DESCRIPTOR.fields;
    assert_eq!(fields[0].node.expect("script node").class_name, "Node");
    assert!(!fields[0].node.expect("required node").optional);
    assert!(fields[1].node.expect("optional node").optional);
    assert_eq!(
        fields[2].property.expect("resource property").hint_string,
        "Resource"
    );
    assert_eq!(
        fields[2].property.expect("resource schema").encoded,
        "gdrs-property-v2:6:8:0:装备Resource"
    );
    let player = Player::__godot_rs_new();
    assert!(!player.health.is_resolved());
    assert!(player.camera.is_none());
    assert!(player.weapon.is_none());
    assert!(player.weapons.is_empty().expect("empty default"));
    assert!(player.equipment_sets.is_empty().expect("empty default"));
    assert!(player.recent_hits.is_empty());
    assert_eq!(player.recent_hits.capacity(), 32);
    assert_eq!(
        fields[3].property.expect("resource array").hint_string,
        "24/17:Resource"
    );
    assert_eq!(
        fields[3]
            .property
            .expect("resource array")
            .typed_array_element,
        Some("Resource")
    );
    assert_eq!(
        fields[3].property.expect("resource array schema").encoded,
        "gdrs-property-v2:0:14:8:24/17:ResourceResource"
    );
    assert_eq!(
        fields[4]
            .property
            .expect("nested resource array")
            .hint_string,
        "28:24/17:Resource"
    );
    assert_eq!(
        fields[4]
            .property
            .expect("nested resource array schema")
            .encoded,
        "gdrs-property-v2:0:17:5:28:24/17:ResourceArray"
    );

    let configure = registered_methods::<Health>()
        .into_iter()
        .find(|method| method.name == "configure")
        .expect("resource method");
    assert_eq!(configure.arguments[0].class_name, Some("Resource"));
    assert_eq!(configure.return_class, Some("Resource"));

    let same = registered_methods::<Health>()
        .into_iter()
        .find(|method| method.name == "same")
        .expect("method using Self in its signature");
    assert_eq!(same.arguments[0].class_name, Some("Node"));
    assert_eq!(same.return_class, Some("Node"));
    assert!(same.rust_signature.contains("Self"));

    let current = registered_methods::<Health>()
        .into_iter()
        .find(|method| method.name == "current")
        .expect("fallible reflected method");
    let mut output = godot_rs::abi::AbiValueV1::NIL;
    // SAFETY: The matching Health instance and return slot remain live for this synchronous call.
    let result = unsafe {
        godot_rs::script::abi_call_method::<Health>(
            (&mut health as *mut Health).cast(),
            current.id,
            std::ptr::null(),
            0,
            &mut output,
        )
    };
    assert_eq!(result.status, godot_rs::abi::AbiStatus::Ok);
    assert_eq!(output.type_, godot_rs::abi::AbiValueType::I64);
    assert_eq!(output.payload[0], 100);
}

#[test]
fn proxies_share_object_identity_and_keep_connection_signatures() {
    let health = NodeRef::<Health>::__from_instance_id(42);
    let signal: Signal<fn(amount: i64, remaining: i64)> = health.signals().damaged();
    assert_eq!(signal.name(), "damaged");
    assert_eq!(signal.object_ref().expect("bound signal").instance_id(), 42);
    let target = health.callbacks().damage();
    assert_eq!(target.as_callable().object().instance_id(), 42);
    assert_eq!(target.as_callable().method().as_str(), "damage");
    let _: godot_rs::callable::CallableMethod<(i64,)> = target;
    let renamed = health.callbacks().label();
    assert_eq!(renamed.as_callable().method().as_str(), "formatted_label");
    let _: godot_rs::callable::CallableMethod<(String,)> = renamed;
    let _: godot_rs::callable::CallableMethod<(NodeRef<Health>,)> = health.callbacks().same();
    let _: Signal<fn(target: NodeRef<Health>)> = health.signals().linked();

    // These calls require a live Host; compiling the function checks the public API contract.
    fn game_logic(health: NodeRef<Health>, config: GodotRef<WeaponConfig>) -> EngineResult<()> {
        let _: i64 = health.damage(10)?;
        let _: String = health.label(String::from("HP"))?;
        let _: i64 = health.current()?;
        let _: NodeRef<Health> = health.same(health)?;
        let _: Option<NodeRef<Health>> = health.properties().parent()?;
        health.properties().set_parent(Some(health))?;
        let _: i64 = config.properties().damage()?;
        config.properties().set_damage(30)?;
        let _: i64 = config.properties().level()?;
        let _: Array<Array<GodotRef<WeaponConfig>>> = config.related(Array::default(), None)?;
        let _: GodotRef<WeaponConfig> = health.configure(config)?;
        Ok(())
    }
    let _ = game_logic;
}

#[script(base = Node)]
struct KeywordMembers {
    #[export]
    r#type: i64,

    #[property]
    r#loop: i64,

    #[signal]
    r#match: Signal<fn(r#type: i64)>,
}

#[script]
impl KeywordMembers {
    #[func]
    fn r#type(&self, r#match: i64) -> i64 {
        r#match
    }

    #[func(name = "keyword_callback")]
    fn r#async(&self, r#type: i64) {
        let _ = r#type;
    }
}

#[test]
fn raw_identifiers_keep_typed_proxies_consistent_with_reflection() {
    let instance = KeywordMembers::__godot_rs_new();
    let object = NodeRef::<KeywordMembers>::__from_instance_id(73);
    let signal = object.signals().r#match();
    assert_eq!(instance.r#match.name(), signal.name());
    assert_eq!(signal.name(), KeywordMembers::DESCRIPTOR.fields[2].name);
    assert_eq!(signal.name(), "match");
    assert_eq!(signal.object_ref().expect("signal owner").instance_id(), 73);

    let callback = object.callbacks().r#type();
    let _: &godot_rs::callable::CallableMethod<(i64,)> = &callback;
    assert_eq!(callback.as_callable().method().as_str(), "type");
    assert_eq!(
        object.callbacks().r#async().as_callable().method().as_str(),
        "keyword_callback"
    );

    // Compile the property and method proxies without requiring a live Host.
    fn access(object: NodeRef<KeywordMembers>) -> EngineResult<()> {
        let _: i64 = object.r#type(4)?;
        let _: i64 = object.properties().r#type()?;
        object.properties().set_type(9)?;
        let _: i64 = object.properties().r#loop()?;
        object.properties().set_loop(2)?;
        Ok(())
    }
    let _ = access;
}
