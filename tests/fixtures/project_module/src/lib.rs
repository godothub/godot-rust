use godot_rs::prelude::*;

struct EngineSmokeState {
    disabled: bool,
    enabled: bool,
    owner_id: u64,
    math_matches: bool,
    string_matches: bool,
    string_name_matches: bool,
    rid_matches: bool,
    enum_matches: bool,
    bitfield_matches: bool,
    node_path_matches: bool,
    extended_math_matches: bool,
}

#[script(base = Node2D)]
pub struct HostSmoke {
    virtual_draw_called: bool,

    #[export(default = 3, group = "State", range(min = 0, max = 100, step = 1))]
    counter: i64,

    #[export(default = "你好，Godot", group = "Text", multiline)]
    message: String,

    #[export(default = "res://main.tscn", file("*.tscn"))]
    scene_path: String,

    #[export(default = "玩家/默认")]
    display_name: StringName,

    #[export(default = "../Target")]
    navigation_path: NodePath,

    #[export]
    labels: PackedStringArray,

    #[export]
    checkpoints: Array<Vector2>,

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

    #[export(default = Vector2::new(12.0, -4.0), group = "Math")]
    velocity: Vector2,

    #[export(default = Vector3::new(1.0, 2.0, 3.0))]
    target_position: Vector3,

    #[export(default = Vector2i::new(4, -6))]
    grid_cell: Vector2i,

    #[export(default = Vector3i::new(7, 8, -9))]
    voxel_cell: Vector3i,

    #[export(default = Color::rgba(0.25, 0.5, 0.75, 1.0), no_alpha)]
    tint: Color,

    #[export(default = Rect2::from_components(1.0, 2.0, 30.0, 40.0))]
    viewport_region: Rect2,

    #[export(default = Rect2i::from_components(3, 4, 50, 60))]
    atlas_region: Rect2i,

    #[export(default = Quaternion::IDENTITY)]
    orientation: Quaternion,

    #[export(default = Plane::new(Vector3::UP, 2.0))]
    clipping_plane: Plane,

    #[export(default = Vector4::new(1.0, 2.0, 3.0, 4.0))]
    floating_vector: Vector4,

    #[export(default = Vector4i::new(5, 6, 7, 8))]
    integer_vector: Vector4i,

    #[export(default = Transform2D::new(
        Vector2::RIGHT,
        Vector2::DOWN,
        Vector2::new(6.0, -3.0),
    ))]
    canvas_transform: Transform2D,

    #[export(default = Aabb::new(
        Vector3::new(1.0, 2.0, 3.0),
        Vector3::new(4.0, 5.0, 6.0),
    ))]
    bounds: Aabb,

    #[export(default = Basis::IDENTITY)]
    local_basis: Basis,

    #[export(default = Transform3D::new(
        Basis::IDENTITY,
        Vector3::new(3.0, 4.0, 5.0),
    ))]
    world_transform: Transform3D,

    #[export(default = Projection::IDENTITY)]
    camera_projection: Projection,

    #[signal(args(old_value, new_value))]
    counter_changed: Signal<(i64, i64)>,

    #[signal(args(message))]
    message_changed: Signal<(String,)>,

    #[signal(args(name))]
    name_changed: Signal<(StringName,)>,

    #[signal(args(velocity, target_position, tint))]
    motion_changed: Signal<(Vector2, Vector3, Color)>,

    #[signal(args(grid_cell, voxel_cell))]
    integer_motion_changed: Signal<(Vector2i, Vector3i)>,

    #[signal(args(resource))]
    rid_changed: Signal<(Rid,)>,

    #[signal(args(region, orientation, vector))]
    extended_math_changed: Signal<(Rect2, Quaternion, Vector4)>,

    #[signal(args(transform))]
    transform_changed: Signal<(Transform3D,)>,

    #[signal(args(labels, points))]
    packed_values_changed: Signal<(PackedStringArray, PackedVector3Array)>,

    #[node("Target")]
    target: NodeRef<Node>,

    #[node("Target3D")]
    target_3d: NodeRef<GridMap>,

    #[node("TargetVisual")]
    target_visual: NodeRef<MeshInstance3D>,

    #[node("Missing", optional)]
    missing: Option<NodeRef<Node>>,
}

#[script]
impl HostSmoke {
    fn _ready(&mut self) {
        let _ = &self.counter_changed;
        match self.target.get_instance_id() {
            Ok(instance_id)
                if self.missing.is_none()
                    && instance_id != 0
                    && instance_id == self.target.instance_id() =>
            {
                godot_print!("GODOT_RUST_NODE_FIELDS");
            }
            Ok(instance_id) => {
                godot_warn!(
                    "typed node fields returned unexpected state: target_id={instance_id}, missing={:?}",
                    self.missing
                );
            }
            Err(error) => {
                godot_warn!("typed node field check was unavailable: {error}");
            }
        }
        let base = self.base();
        #[cfg(godot_rs_test_api_4_7)]
        match self.verify_signal_ptrcall() {
            Ok(true) => godot_print!("GODOT_RUST_SIGNAL_PTRCALL"),
            Ok(false) => godot_warn!("Signal ptrcall returned an unexpected Tween result"),
            Err(error) => godot_warn!("Signal ptrcall failed: {error}"),
        }
        let engine_state = (|| -> EngineResult<EngineSmokeState> {
            base.set_process(false)?;
            let disabled = base.is_processing()?;
            base.set_process(true)?;
            base.set_process_mode(node::ProcessMode::PROCESS_MODE_ALWAYS)?;
            let enum_matches = base.get_process_mode()? == node::ProcessMode::PROCESS_MODE_ALWAYS;
            base.set_process_mode(node::ProcessMode::PROCESS_MODE_INHERIT)?;
            let thread_messages = node::ProcessThreadMessages::FLAG_PROCESS_THREAD_MESSAGES
                | node::ProcessThreadMessages::FLAG_PROCESS_THREAD_MESSAGES_PHYSICS;
            base.set_process_thread_messages(thread_messages)?;
            let bitfield_matches = base
                .get_process_thread_messages()?
                .contains(node::ProcessThreadMessages::FLAG_PROCESS_THREAD_MESSAGES_ALL);
            base.set_process_thread_messages(node::ProcessThreadMessages::empty())?;
            let position = Vector2::new(18.5, -7.25);
            let modulate = Color::rgba(0.2, 0.4, 0.6, 0.8);
            let target_position = Vector3::new(3.5, -2.0, 9.25);
            base.set_position(position)?;
            base.set_modulate(modulate)?;
            self.target_3d.set_position(target_position)?;
            let description = "你好，godot-rust engine API";
            base.set_editor_description(description)?;
            let math_matches = base.get_position()? == position
                && base.get_modulate()? == modulate
                && self.target_3d.get_position()? == target_position;
            let string_matches = base.get_editor_description()? == description;
            let group_name = "godot-rust/StringName/玩家";
            base.add_to_group(group_name, false)?;
            let string_name_matches = base.is_in_group(group_name)? && !base.get_name()?.is_empty();
            base.remove_from_group(group_name)?;
            let target_path = base.get_path_to(self.target, false)?;
            let node_path_matches = target_path.as_str() == "Target"
                && base
                    .get_node_or_null(target_path.as_str())?
                    .is_some_and(|target| target == self.target);
            let viewport_rect = base.get_viewport_rect()?;
            self.target_3d.set_quaternion(Quaternion::IDENTITY)?;
            let transform_2d = Transform2D::new(
                Vector2::new(1.0, 0.0),
                Vector2::new(0.0, 1.0),
                Vector2::new(6.0, -3.0),
            );
            base.set_transform(&transform_2d)?;
            let transform_3d = Transform3D::new(Basis::IDENTITY, target_position);
            self.target_3d.set_transform(&transform_3d)?;
            let bounds = self.target_visual.get_aabb()?;
            let extended_math_matches = viewport_rect.size.x > 0.0
                && viewport_rect.size.y > 0.0
                && base.get_transform()? == transform_2d
                && self.target_3d.get_basis()? == Basis::IDENTITY
                && self.target_3d.get_transform()? == transform_3d
                && bounds.position.is_finite()
                && bounds.size.is_finite();
            let rid_matches = if let Some(world) = self.target_3d.get_world_3d()? {
                let retained_world = world.clone();
                drop(world);
                let navigation_map = retained_world.get_navigation_map()?;
                self.target_3d.set_navigation_map(navigation_map)?;
                let matches = navigation_map.is_valid()
                    && self.target_3d.get_navigation_map()? == navigation_map;
                self.target_3d.set_navigation_map(Rid::INVALID)?;
                matches
            } else {
                false
            };
            Ok(EngineSmokeState {
                disabled,
                enabled: base.is_processing()?,
                owner_id: base.get_instance_id()?,
                math_matches,
                string_matches,
                string_name_matches,
                rid_matches,
                enum_matches,
                bitfield_matches,
                node_path_matches,
                extended_math_matches,
            })
        })();
        match engine_state {
            Ok(state)
                if !state.disabled
                    && state.enabled
                    && state.owner_id != 0
                    && state.math_matches
                    && state.string_matches
                    && state.string_name_matches
                    && state.rid_matches
                    && state.enum_matches
                    && state.bitfield_matches
                    && state.node_path_matches
                    && state.extended_math_matches =>
            {
                godot_print!("GODOT_RUST_ENGINE_API");
                godot_print!("GODOT_RUST_ENGINE_MATH_API");
                godot_print!("GODOT_RUST_ENGINE_STRING_API");
                godot_print!("GODOT_RUST_ENGINE_STRING_NAME_API");
                godot_print!("GODOT_RUST_ENGINE_RID_API");
                godot_print!("GODOT_RUST_ENGINE_ENUM_API");
                godot_print!("GODOT_RUST_ENGINE_NODE_PATH_API");
                godot_print!("GODOT_RUST_ENGINE_EXTENDED_MATH_API");
            }
            Ok(state) => {
                let EngineSmokeState {
                    disabled,
                    enabled,
                    owner_id,
                    math_matches,
                    string_matches,
                    string_name_matches,
                    rid_matches,
                    enum_matches,
                    bitfield_matches,
                    node_path_matches,
                    extended_math_matches,
                } = state;
                godot_warn!(
                    "generated Godot API returned unexpected state: disabled={disabled}, enabled={enabled}, owner_id={owner_id}, math_matches={math_matches}, string_matches={string_matches}, string_name_matches={string_name_matches}, rid_matches={rid_matches}, enum_matches={enum_matches}, bitfield_matches={bitfield_matches}, node_path_matches={node_path_matches}, extended_math_matches={extended_math_matches}"
                );
            }
            Err(error) => {
                godot_warn!("generated Godot API check was unavailable: {error}");
            }
        }
        godot_print!("GODOT_RUST_MODULE_READY");
    }

    fn _process(&mut self, delta: f64) {
        if delta >= 0.0 {
            godot_print!("GODOT_RUST_MODULE_PROCESS");
        }
    }

    #[func]
    fn sum_values(&self, left: i64, #[default(0)] offset: i64, #[default(22)] right: i64) -> i64 {
        godot_print!("GODOT_RUST_MODULE_METHOD");
        left + offset + right
    }

    #[func]
    fn count_variants(&self, label: String, values: &[Variant]) -> i64 {
        i64::try_from(label.len() + values.len()).unwrap_or(i64::MAX)
    }

    #[func]
    fn failure_for_test(&self) -> ScriptResult<()> {
        godot_print!("GODOT_RUST_EXPECTED_FAILURE_CALLBACK");
        Err(ScriptError::new(
            "intentional callback failure for instance isolation",
        ))
    }

    #[func]
    fn panic_for_test(&self) {
        godot_print!("GODOT_RUST_EXPECTED_PANIC_CALLBACK");
        panic!("intentional callback panic for instance isolation");
    }
}

#[script]
impl HostSmoke {
    #[func]
    fn mirror_node(&self, node: Option<ObjectRef<Node>>) -> Option<ObjectRef<Node>> {
        node
    }

    #[func]
    fn greet(&self, name: String) -> String {
        godot_print!("GODOT_RUST_STRING_VALUES");
        format!("你好，{name}")
    }

    #[func]
    fn echo_string_name(&self, name: StringName) -> StringName {
        name
    }

    #[func]
    fn translate_2d(&self, point: Vector2, offset: Vector2) -> Vector2 {
        point + offset
    }

    #[func]
    fn translate_3d(&self, point: Vector3, offset: Vector3) -> Vector3 {
        point + offset
    }

    #[func]
    fn blend_tint(&self, color: Color) -> Color {
        color * self.tint
    }

    #[func]
    fn translate_integer_2d(&self, point: Vector2i, offset: Vector2i) -> Vector2i {
        point + offset
    }

    #[func]
    fn translate_integer_3d(&self, point: Vector3i, offset: Vector3i) -> Vector3i {
        point + offset
    }

    #[func]
    fn echo_rid(&self, resource: Rid) -> Rid {
        resource
    }

    #[func]
    fn verify_extended_math(
        &self,
        region: Rect2,
        atlas: Rect2i,
        orientation: Quaternion,
        plane: Plane,
        vector: Vector4,
        integer_vector: Vector4i,
    ) -> bool {
        region == self.viewport_region
            && atlas == self.atlas_region
            && orientation == self.orientation
            && plane == self.clipping_plane
            && vector == self.floating_vector
            && integer_vector == self.integer_vector
    }

    #[func]
    fn emit_extended_math(&self, region: Rect2, orientation: Quaternion, vector: Vector4) {
        self.extended_math_changed
            .emit((region, orientation, vector));
    }

    #[func]
    fn verify_large_math(
        &self,
        transform_2d: Transform2D,
        bounds: Aabb,
        basis: Basis,
        transform_3d: Transform3D,
        projection: Projection,
    ) -> bool {
        transform_2d == Transform2D::IDENTITY
            && bounds == Aabb::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(4.0, 5.0, 6.0))
            && basis == Basis::IDENTITY
            && transform_3d == Transform3D::IDENTITY
            && projection == Projection::IDENTITY
    }

    #[func]
    fn echo_transform(&self, transform: Transform3D) -> Transform3D {
        transform
    }

    #[func]
    fn emit_transform(&self, transform: Transform3D) {
        self.transform_changed.emit((transform,));
    }

    #[func]
    fn verify_integer_engine_api(
        &self,
        grid: ObjectRef<AStarGrid2D>,
        grid_map: ObjectRef<GridMap>,
    ) -> ScriptResult<bool> {
        let size = Vector2i::new(7, 9);
        grid.set_size(size)?;
        Ok(grid.get_size()? == size
            && grid_map.local_to_map(Vector3::ZERO)? == Vector3i::ZERO
            && grid_map.map_to_local(Vector3i::ZERO)?.is_finite())
    }

    #[func]
    fn verify_packed_engine_api(
        &self,
        image: ObjectRef<Image>,
        grid: ObjectRef<AStarGrid2D>,
    ) -> ScriptResult<bool> {
        let pixels = PackedByteArray::from(vec![
            255, 0, 0, 255, // opaque red
            0, 128, 255, 64, // translucent blue
        ]);
        image.set_data(2, 1, false, image::Format::FORMAT_RGBA8, &pixels)?;
        let restored = image.get_data()?;

        grid.set_region(Rect2i::from_components(0, 0, 3, 1))?;
        grid.update()?;
        let path = grid.get_point_path(Vector2i::new(0, 0), Vector2i::new(2, 0), false)?;
        Ok(restored == pixels && path.len() == 3 && path.iter().all(|point| point.is_finite()))
    }

    #[func]
    fn verify_packed_scalars(
        &self,
        bytes: PackedByteArray,
        ints: PackedInt32Array,
        wide_ints: PackedInt64Array,
        floats: PackedFloat32Array,
        doubles: PackedFloat64Array,
    ) -> bool {
        bytes.as_ref() == [0, 1, 127, 255]
            && ints.as_ref() == [-2, 0, 4]
            && wide_ints.as_ref() == [i64::MIN, 42, i64::MAX]
            && floats.as_ref() == [0.25, -1.5, 8.0]
            && doubles.as_ref() == [0.125, -2.5, f64::MAX]
    }

    #[func]
    fn verify_packed_sequences(
        &self,
        labels: PackedStringArray,
        points_2d: PackedVector2Array,
        points_3d: PackedVector3Array,
    ) -> bool {
        labels.to_vec() == ["你好", "Godot", "玩家"]
            && points_2d.as_ref() == [Vector2::new(1.0, 2.0), Vector2::new(-3.0, 4.0)]
            && points_3d.as_ref() == [Vector3::new(1.0, 2.0, 3.0), Vector3::new(-4.0, 5.0, -6.0)]
    }

    #[func]
    fn echo_packed_colors(&self, values: PackedColorArray) -> PackedColorArray {
        values
    }

    #[func]
    fn echo_packed_vectors4(&self, values: PackedVector4Array) -> PackedVector4Array {
        values
    }

    #[func]
    fn echo_variant(&self, value: Variant) -> Variant {
        value
    }

    #[func]
    fn echo_array(&self, values: Array) -> Array {
        values
    }

    #[func]
    fn echo_vector_array(&self, values: Array<Vector2>) -> Array<Vector2> {
        values
    }

    #[func]
    fn echo_dictionary(&self, values: Dictionary) -> Dictionary {
        values
    }

    #[func]
    fn echo_callable(&self, callback: Callable) -> Callable {
        callback
    }

    #[func]
    fn echo_signal(&self, signal: Signal) -> Signal {
        signal
    }

    #[func]
    fn verify_signal_engine_api(&self) -> ScriptResult<bool> {
        let base = self.base();
        let owner = base.object_ref()?;
        let signal: Signal = Signal::from_object(owner, "renamed");
        let callback = Callable::from_object_method(owner, "current_counter");
        let status = signal.connect(&callback, 0)?;
        let connected = signal.is_connected(&callback)?;
        signal.disconnect(&callback)?;
        let disconnected = !signal.is_connected(&callback)?;

        let expected = Variant::from(signal);
        base.set_meta("_godot_rust_signal", &expected)?;
        let restored = base.get_meta("_godot_rust_signal", &Variant::nil())?;
        base.remove_meta("_godot_rust_signal")?;

        Ok(status == global::Error::OK && connected && disconnected && restored == expected)
    }

    #[func]
    fn verify_callable_engine_api(&self) -> bool {
        let base = self.base();
        let Ok(owner) = base.object_ref() else {
            godot_warn!("Callable engine test could not resolve its owner");
            return false;
        };
        let callback = Callable::from_object_method(owner, "current_counter");
        let status = match base.connect("renamed", &callback, 0) {
            Ok(status) => status,
            Err(error) => {
                godot_warn!("Callable engine test failed while connecting: {error}");
                return false;
            }
        };
        let Ok(connected) = base.is_connected("renamed", &callback) else {
            godot_warn!("Callable engine test failed while checking the connection");
            return false;
        };
        if base.disconnect("renamed", &callback).is_err() {
            godot_warn!("Callable engine test failed while disconnecting");
            return false;
        }
        status == global::Error::OK
            && connected
            && base
                .is_connected("renamed", &callback)
                .is_ok_and(|value| !value)
    }

    #[func]
    fn virtual_draw_was_called(&self) -> bool {
        self.virtual_draw_called
    }

    #[func]
    fn verify_static_engine_api(&self) -> bool {
        let image = match Image::create(3, 2, false, image::Format::FORMAT_RGBA8) {
            Ok(Some(image)) => image,
            Ok(None) => {
                godot_warn!("Image.create returned null");
                return false;
            }
            Err(error) => {
                godot_warn!("Image.create failed: {error}");
                return false;
            }
        };
        image.get_width() == Ok(3) && image.get_height() == Ok(2)
    }

    #[func]
    fn verify_complete_generated_api(&self) -> ScriptResult<bool> {
        let input = Input::singleton()?;
        let singleton_matches = input.get_instance_id()? != 0;
        let utility_matches = utility::clampf(3.5, -1.0, 2.0)? == 2.0;

        let constructed = builtin::vector2::construct_3(3.0, 4.0)?;
        let builtin_method_matches = builtin::vector2::length(&constructed)? == 5.0;
        let operator_matches =
            builtin::vector2::operator_add_vector2_15(&constructed, Vector2::new(1.0, 2.0))?
                == Vector2::new(4.0, 6.0);
        let member_get_matches = builtin::vector2::member_get_x(&constructed)? == 3.0;
        let mut edited = constructed;
        builtin::vector2::member_set_x(&mut edited, 7.0)?;
        builtin::vector2::indexed_set(&mut edited, 1, 8.0)?;
        let mutation_matches =
            edited == Vector2::new(7.0, 8.0) && builtin::vector2::indexed_get(&edited, 0)? == 7.0;

        let mut dictionary = Dictionary::new();
        let key = Variant::from("answer");
        let expected = Variant::from(42_i64);
        builtin::dictionary::keyed_set(&mut dictionary, &key, &expected)?;
        let keyed_matches = builtin::dictionary::keyed_get(&dictionary, &key)? == expected;

        let builtin_constant_matches = builtin::color::constant_white()? == Color::WHITE;
        let class_constant_matches = Node::NOTIFICATION_READY == 13;

        let image = Image::new_godot()?;
        let construction_matches = image.get_width()? == 0 && image.get_height()? == 0;

        let graph = AStar2D::new_godot()?;
        graph.add_point(7, Vector2::new(2.0, 3.0), 1.0)?;
        let scalar_transport_matches = graph.get_point_weight_scale(7)? == 1.0;

        let base = self.base();
        let owner = base.object_ref()?;
        let draw = base.signal_draw()?;
        let signal_matches =
            draw.name() == "draw" && draw.object_ref()?.instance_id() == owner.instance_id();

        Ok(singleton_matches
            && utility_matches
            && builtin_method_matches
            && operator_matches
            && member_get_matches
            && mutation_matches
            && keyed_matches
            && builtin_constant_matches
            && class_constant_matches
            && construction_matches
            && scalar_transport_matches
            && signal_matches)
    }

    #[func]
    fn verify_vararg_engine_api(&self) -> ScriptResult<bool> {
        let base = self.base();
        let original_priority = base.get_process_priority()?;
        let expected_priority = original_priority.saturating_add(1);
        let set_result = base.call(
            "set_process_priority",
            &[Variant::from(i64::from(expected_priority))],
        )?;
        let observed_priority = base.call("get_process_priority", &[])?;
        base.set_process_priority(original_priority)?;
        Ok(set_result == Variant::nil()
            && observed_priority == Variant::from(i64::from(expected_priority)))
    }

    #[func]
    fn verify_refcounted_array(&self, images: Array<GodotRef<Image>>) -> ScriptResult<bool> {
        let Some(image) = images.first().cloned() else {
            return Ok(false);
        };
        drop(images);
        Ok(image.get_width()? == 2 && image.get_height()? == 1)
    }

    #[func]
    fn verify_dynamic_engine_api(&self) -> ScriptResult<bool> {
        let base = self.base();
        let children = base.get_children(false)?;
        let expected_children = [self.target.instance_id(), self.target_3d.instance_id()];
        let children_match = expected_children.iter().all(|expected| {
            children
                .iter()
                .any(|child| child.instance_id() == *expected)
        });

        let mut metadata = Dictionary::new();
        metadata.insert("message", "你好，Godot");
        metadata.insert("position", Vector3::new(1.0, -2.0, 3.5));
        metadata.insert(
            "values",
            Array::from(vec![Variant::from(42_i64), Variant::from(true)]),
        );
        let metadata = Variant::from(metadata);
        base.set_meta("_godot_rust_dynamic", &metadata)?;
        let restored = base.get_meta("_godot_rust_dynamic", &Variant::nil())?;
        let names = base.get_meta_list()?;
        base.remove_meta("_godot_rust_dynamic")?;

        Ok(children_match
            && restored == metadata
            && names
                .iter()
                .any(|name| name.as_str() == "_godot_rust_dynamic"))
    }

    #[func]
    fn emit_packed_values(&self, labels: PackedStringArray, points: PackedVector3Array) {
        self.packed_values_changed.emit((labels, points));
    }

    #[rpc(authority, reliable)]
    fn set_counter(&mut self, value: i64) {
        self.counter = value;
    }

    #[func]
    fn update_counter(&mut self, value: i64) {
        let old_value = self.counter;
        self.counter = value;
        self.counter_changed.emit((old_value, value));
    }

    #[func]
    fn current_counter(&self) -> i64 {
        self.counter
    }

    #[func]
    fn update_message(&mut self, value: String) {
        self.message = value.clone();
        self.message_changed.emit((value,));
    }

    #[func]
    fn current_message(&self) -> String {
        self.message.clone()
    }

    #[func]
    fn update_display_name(&mut self, value: StringName) {
        self.display_name = value.clone();
        self.name_changed.emit((value,));
    }

    #[func]
    fn current_display_name(&self) -> StringName {
        self.display_name.clone()
    }

    #[func]
    fn update_motion(&mut self, velocity: Vector2, target_position: Vector3, tint: Color) {
        self.velocity = velocity;
        self.target_position = target_position;
        self.tint = tint;
        self.motion_changed.emit((velocity, target_position, tint));
    }

    #[func]
    fn update_integer_motion(&mut self, grid_cell: Vector2i, voxel_cell: Vector3i) {
        self.grid_cell = grid_cell;
        self.voxel_cell = voxel_cell;
        self.integer_motion_changed.emit((grid_cell, voxel_cell));
    }

    #[func]
    fn emit_rid(&self, resource: Rid) {
        self.rid_changed.emit((resource,));
    }
}

#[cfg(godot_rs_test_api_4_7)]
impl HostSmoke {
    fn verify_signal_ptrcall(&self) -> ScriptResult<bool> {
        let base = self.base();
        let owner = base.object_ref()?;
        let signal: Signal = Signal::from_object(owner, "renamed");
        let Some(tween) = base.create_tween()? else {
            return Ok(false);
        };
        let awaiter = tween.tween_await(&signal)?;
        tween.kill()?;
        Ok(awaiter.is_some())
    }
}

#[script]
impl CanvasItemVirtual for HostSmoke {
    fn _draw(&mut self) {
        self.virtual_draw_called = true;
    }
}

godot_rs::script_module! {
    HostSmoke => ("res://sample.rs", "uid://b1"),
}
