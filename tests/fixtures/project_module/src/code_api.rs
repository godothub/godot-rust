use std::cell::{Cell, RefCell};
use std::rc::Rc;

use godot_rs::prelude::*;
use godot_rs::signal::SignalConnection;
use godot_rs::variant::VariantConvert;

#[script(base = Resource)]
pub struct Tuning {
    #[export(default = 12)]
    power: i64,
}

#[script]
impl Tuning {
    #[func]
    fn get_power(&self) -> i64 {
        self.power
    }
}

#[script(base = Node)]
pub struct Health {
    #[export(default = 100)]
    points: i64,

    #[export]
    config: Option<GodotRef<Tuning>>,

    #[signal]
    damaged: Signal<fn(amount: i64, remaining: i64)>,

    #[signal]
    config_sent: Signal<fn(config: GodotRef<Tuning>)>,

    #[signal]
    config_dynamic: Signal<fn(config: Variant)>,
}

#[script]
impl Health {
    #[func]
    fn damage(&mut self, amount: i64) -> EngineResult<i64> {
        self.points -= amount;
        self.damaged.emit(amount, self.points)?;
        Ok(self.points)
    }

    #[func]
    fn take_config(&mut self) -> Option<GodotRef<Tuning>> {
        self.config.take()
    }

    #[func]
    fn take_configs(&mut self) -> Array<GodotRef<Tuning>> {
        Array::from_vec(self.config.take().into_iter().collect())
    }

    #[func]
    fn take_config_variant(&mut self) -> Variant {
        Variant::from(self.take_configs())
    }

    #[func]
    fn take_config_dictionary(&mut self) -> Dictionary {
        let mut values = Dictionary::new();
        if let Some(config) = self.config.take() {
            values.insert("config", config);
        }
        values
    }

    #[func]
    fn take_callable(&mut self) -> Callable {
        match self.config.take() {
            Some(config) => Callable::from_godot_ref_method(&config, "get_power"),
            None => Callable::null(),
        }
    }

    #[func]
    fn send_config(&mut self, dynamic: bool) -> EngineResult<bool> {
        let Some(config) = self.config.take() else {
            return Ok(false);
        };
        if dynamic {
            self.config_dynamic
                .emit(Variant::from(Array::from_vec(vec![config])))?;
        } else {
            self.config_sent.emit(config)?;
        }
        Ok(true)
    }

    #[func]
    fn fail_for_context(&self) -> ScriptResult<()> {
        Err(ScriptError::new("fixture typed failure"))
    }
}

#[script(base = Node)]
pub struct Watcher {
    received: i64,
    config_id: i64,
    callback: Callable,
}

#[script]
impl Watcher {
    #[func]
    fn receive(&mut self, amount: i64, remaining: i64) {
        self.received = amount * 1000 + remaining;
    }

    #[func]
    fn last_received(&self) -> i64 {
        self.received
    }

    #[func]
    fn receive_config(&mut self, config: GodotRef<Tuning>) -> EngineResult<()> {
        if config.properties().power()? == 12 {
            self.config_id = config.instance_id() as i64;
        }
        Ok(())
    }

    #[func]
    fn receive_dynamic_config(&mut self, config: Variant) -> EngineResult<()> {
        if let Some(config) =
            Array::<GodotRef<Tuning>>::from_variant(config).and_then(|mut values| values.pop())
        {
            self.receive_config(config)?;
        }
        Ok(())
    }

    #[func]
    fn last_config_id(&self) -> i64 {
        self.config_id
    }

    #[func]
    fn remember_callable(&mut self, callback: Callable) {
        self.callback = callback;
    }

    #[func]
    fn verify_callable(&self) -> EngineResult<bool> {
        Ok(builtin::callable::call(&self.callback, &[])? == Variant::from(12_i64))
    }

    #[func]
    fn release_callable(&mut self) {
        self.callback = Callable::null();
    }
}

#[script(base = Node)]
pub struct CodeApi {
    #[node("Health")]
    health: NodeRef<Health>,

    #[node("Watcher")]
    watcher: NodeRef<Watcher>,

    #[node("Absent")]
    absent: Option<NodeRef<Health>>,

    #[signal]
    finished: Signal<fn(amount: i64, remaining: i64)>,

    retained: Option<GodotRef<Tuning>>,
    connection: Option<SignalConnection>,
    received: Rc<Cell<i64>>,
    received_config: Rc<RefCell<Option<GodotRef<Tuning>>>>,
}

#[script]
impl CodeApi {
    #[func]
    fn verify_calls(&mut self) -> EngineResult<bool> {
        let initial = self.health.properties().points()?;
        let remaining = self.health.damage(7)?;
        self.health.properties().set_points(88)?;
        let updated = self.health.properties().points()?;
        let found = self.base().node::<Health>("Health")?;
        let absent = self.base().try_node::<Health>("Absent")?;
        let wrong_script = self.base().node::<Health>("Watcher").is_err();
        Ok(initial == 100
            && remaining == 93
            && updated == 88
            && found == self.health
            && absent.is_none()
            && self.absent.is_none()
            && wrong_script)
    }

    #[func]
    fn retain_returned_resource(&mut self, mode: i64) -> EngineResult<i64> {
        let config = match mode {
            0 => self.health.take_config()?,
            1 => self.health.take_configs()?.pop(),
            2 => Array::<GodotRef<Tuning>>::from_variant(self.health.take_config_variant()?)
                .and_then(|mut values| values.pop()),
            3 => self
                .health
                .take_config_dictionary()?
                .remove(&Variant::from("config"))
                .and_then(GodotRef::<Tuning>::from_variant),
            _ => None,
        };
        let Some(config) = config else {
            return Ok(0);
        };
        if config.properties().power()? != 12 {
            return Ok(0);
        }
        config.properties().set_power(31)?;
        let id = config.instance_id();
        self.retained = Some(config);
        Ok(id as i64)
    }

    #[func]
    fn verify_retained_resource(&self) -> EngineResult<bool> {
        match &self.retained {
            Some(config) => Ok(config.properties().power()? == 31),
            None => Ok(false),
        }
    }

    #[func]
    fn release_resource(&mut self) {
        self.retained = None;
        *self.received_config.borrow_mut() = None;
    }

    #[func]
    fn retain_returned_callable(&self) -> EngineResult<i64> {
        let callback = self.health.take_callable()?;
        let id = callback.object().instance_id();
        self.watcher.remember_callable(callback)?;
        Ok(id as i64)
    }

    #[func]
    fn start_watch(&mut self) -> EngineResult<()> {
        let signal = self.health.signals().damaged();
        self.connection = Some(signal.connect_scoped(&self.watcher.callbacks().receive())?);
        let waiting = signal.wait();
        let finished = self.finished.bind()?;
        let received = Rc::clone(&self.received);
        self.tasks().spawn(async move {
            match waiting.await {
                Ok((amount, remaining)) => {
                    received.set(amount * 1000 + remaining);
                    if let Err(error) = finished.emit(amount, remaining) {
                        godot_error!("bound signal emission failed: {error}");
                    }
                }
                Err(error) => godot_error!("typed signal wait failed: {error}"),
            }
        });
        Ok(())
    }

    #[func]
    fn verify_watch(&self) -> EngineResult<bool> {
        Ok(self.received.get() == 5083 && self.watcher.last_received()? == 5083)
    }

    #[func]
    fn disconnect_watch(&mut self) {
        self.connection = None;
    }

    #[func]
    fn verify_disconnected(&self) -> EngineResult<bool> {
        Ok(self.watcher.last_received()? == 5083)
    }

    #[func]
    fn start_config_watch(&mut self, dynamic: bool) -> EngineResult<()> {
        if dynamic {
            let signal = self.health.signals().config_dynamic();
            self.connection =
                Some(signal.connect_scoped(&self.watcher.callbacks().receive_dynamic_config())?);
            let waiting = signal.wait();
            let received = Rc::clone(&self.received_config);
            self.tasks().spawn(async move {
                match waiting.await {
                    Ok((value,)) => Self::record_config(
                        received,
                        Array::<GodotRef<Tuning>>::from_variant(value)
                            .and_then(|mut values| values.pop()),
                    ),
                    Err(error) => godot_error!("dynamic resource wait failed: {error}"),
                }
            });
            return Ok(());
        }
        let signal = self.health.signals().config_sent();
        self.connection = Some(signal.connect_scoped(&self.watcher.callbacks().receive_config())?);
        let waiting = signal.wait();
        let received = Rc::clone(&self.received_config);
        self.tasks().spawn(async move {
            match waiting.await {
                Ok((config,)) => Self::record_config(received, Some(config)),
                Err(error) => godot_error!("typed resource wait failed: {error}"),
            }
        });
        Ok(())
    }

    fn record_config(
        received: Rc<RefCell<Option<GodotRef<Tuning>>>>,
        config: Option<GodotRef<Tuning>>,
    ) {
        let Some(config) = config else {
            godot_error!("awaited container omitted its typed resource");
            return;
        };
        if let Err(error) = config.properties().set_power(31) {
            godot_error!("awaited resource access failed: {error}");
        }
        *received.borrow_mut() = Some(config);
    }

    #[func]
    fn verify_received_config(&self, id: i64) -> EngineResult<bool> {
        match self.received_config.borrow().as_ref() {
            Some(config) => Ok(config.instance_id() as i64 == id
                && config.properties().power()? == 31
                && self.watcher.last_config_id()? == id),
            None => Ok(false),
        }
    }

    #[func]
    fn verify_error_context(&self) -> bool {
        match self.health.fail_for_context() {
            Err(error) => {
                error.message().contains("res://code_api/health.rs")
                    && error.message().contains("fail_for_context")
                    && error.message().contains("fixture typed failure")
            }
            Ok(()) => false,
        }
    }

    #[func]
    fn rejects_replaced_script(&self) -> bool {
        self.health.damage(1).is_err() && self.health.properties().points().is_err()
    }

    #[func]
    fn verify_input_defaults(&self) -> EngineResult<bool> {
        let before = Input::is_action_pressed("code_api_jump")?;
        Input::action_press("code_api_jump")?;
        let input = Input::singleton()?;
        let pressed = Input::is_action_pressed("code_api_jump")?;
        let exact = input
            .is_action_pressed_ex("code_api_jump")
            .exact_match(true)
            .done()?;
        let strength = input.get_action_strength_ex("code_api_jump").done()?;
        Input::action_release("code_api_jump")?;
        Ok(!before
            && pressed
            && exact
            && strength == 1.0
            && !Input::is_action_pressed("code_api_jump")?)
    }
}
