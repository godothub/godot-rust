use super::game_tuning::GameTuning;
use godot_rs::prelude::*;
use std::time::Duration;

#[godot_enum]
enum GreetingStyle {
    Friendly = 0,
    Formal = 1,
}

/// Demonstrates a Godot node script with Inspector fields and typed signals.
#[script(
    base = Node,
    extends = "res://src/scripts/base_greeting.rs",
    class_name = RustHello
)]
pub struct Hello {
    #[export(default = 1, group = "Greeting", range(min = 1, max = 10, step = 1))]
    greeting_count: i64,

    #[export(default = "Hello from Rust", multiline)]
    greeting: String,

    #[export(default = GreetingStyle::Friendly, group = "Greeting")]
    greeting_style: GreetingStyle,

    #[property(default = 0, read_only)]
    greetings_sent: i64,

    #[export(default = Vector2::new(128.0, 64.0), group = "Movement")]
    spawn_point: Vector2,

    #[export(default = Vector2i::new(4, -6))]
    grid_cell: Vector2i,

    #[export(default = Color::rgba(0.2, 0.6, 1.0, 1.0), no_alpha)]
    tint: Color,

    #[export(group = "Gameplay")]
    tuning: Option<GodotRef<GameTuning>>,

    #[signal]
    greeted: Signal<fn(message: String)>,

    #[signal]
    styled: Signal<fn(position: Vector2, tint: Color)>,

    #[signal]
    grid_selected: Signal<fn(cell: Vector2i)>,

    #[signal]
    canvas_ready: Signal<fn(canvas: Rid)>,

    #[node("%Target")]
    target: NodeRef<Node>,
}

#[script]
impl Hello {
    pub const MAX_GREETING_COUNT: i64 = 10;

    fn _ready(&mut self) -> EngineResult<()> {
        self.call_super::<(), _>("_ready", ())?;
        let owner_id = self.base().get_instance_id()?;
        let target_id = self.target.get_instance_id()?;
        for index in 1..=self.greeting_count {
            godot_print!(
                "Hello from Rust object {owner_id} with target {target_id}! ({index}/{})",
                self.greeting_count
            );
        }
        if let Some(tuning) = &self.tuning {
            godot_print!(
                "Loaded custom Rust Resource object {}.",
                tuning.object_ref().instance_id()
            );
        }
        let Some(viewport) = self.base().get_viewport()? else {
            return Ok(());
        };
        let Some(world) = viewport.find_world_2d()? else {
            return Ok(());
        };
        let canvas = world.get_canvas()?;
        if canvas.is_valid() {
            self.canvas_ready.emit(canvas)?;
            godot_print!("Canvas RID: {}", canvas.id());
        }
        let inherited: String = self.call_super("format_greeting", ())?;
        godot_print!("Inherited Rust script returned: {inherited}");
        self.tasks().spawn(async move {
            next_frame().await;
            godot_print!("Rust task resumed for object {owner_id} on the next Godot frame.");
        });
        if let Some(tree) = self.base().get_tree()? {
            let next_process_frame = tree.signal_process_frame()?.wait();
            self.tasks().spawn(async move {
                match timeout(Duration::from_secs(1), next_process_frame).await {
                    Ok(Ok(())) => godot_print!("Rust awaited SceneTree.process_frame."),
                    Ok(Err(error)) => godot_warn!("Could not await Godot signal: {error}"),
                    Err(error) => godot_warn!("Godot signal wait timed out: {error}"),
                }
            });
        }
        Ok(())
    }

    #[func]
    fn add(&self, left: i64, right: i64) -> i64 {
        left + right
    }

    #[func(name = "greet_player")]
    fn greet(&mut self, name: String) -> EngineResult<String> {
        self.greetings_sent += 1;
        let message = if self.greeting_style == GreetingStyle::Formal {
            format!("{}，{name}。", self.greeting)
        } else {
            format!("{}, {name}!", self.greeting)
        };
        self.greeted.emit(message.clone())?;
        Ok(message)
    }

    #[func]
    fn offset_position(&mut self, offset: Vector2) -> EngineResult<Vector2> {
        let position = self.spawn_point + offset;
        self.styled.emit(position, self.tint)?;
        Ok(position)
    }

    #[func]
    fn select_grid_cell(&mut self, offset: Vector2i) -> EngineResult<Vector2i> {
        let cell = self.grid_cell + offset;
        self.grid_selected.emit(cell)?;
        Ok(cell)
    }

    #[func]
    fn echo_rid(&self, resource: Rid) -> Rid {
        resource
    }
}
