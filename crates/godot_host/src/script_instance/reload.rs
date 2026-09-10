use super::*;
use std::collections::HashMap;

// All old states remain locked until every replacement is ready. Publication
// swaps only Host-owned pointers and never invokes user code under slot locks.
pub(crate) fn install_generation(
    candidate: ModuleGeneration,
) -> Result<usize, GenerationReloadError> {
    if ACTIVE_SCRIPT_INSTANCE.with(|active| !active.get().is_null()) {
        return Err(GenerationReloadError::Busy);
    }
    let Some(current) = crate::module_loader::active_generation() else {
        let exports =
            crate::script::prepare_exports(&candidate).map_err(GenerationReloadError::Rejected)?;
        crate::module_loader::set_active_generation(candidate);
        exports.apply();
        return Ok(0);
    };
    let slots = {
        let registry = LIVE_INSTANCES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .map_err(|_| rejected("live instance registry is poisoned"))?;
        registry
            .iter()
            .filter_map(|address| slot(*address as GDExtensionScriptInstanceDataPtr))
            .collect::<Vec<_>>()
    };
    let instances = slots
        .iter()
        .map(|slot| {
            slot.current
                .lock()
                .map(|current| current.clone())
                .map_err(|_| rejected("instance slot is poisoned"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Acquire every state before running any user constructor/getter. Reentry
    // cannot mutate another instance halfway through the snapshot.
    let states = instances
        .iter()
        .map(|instance| {
            instance
                .state
                .as_ref()
                .ok_or_else(|| rejected("live script state is missing"))?
                .try_lock()
                .map_err(|error| match error {
                    TryLockError::WouldBlock => GenerationReloadError::Busy,
                    TryLockError::Poisoned(_) => rejected("live script state is poisoned"),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let live_uids = states
        .iter()
        .map(|state| state.resource_uid())
        .collect::<Vec<_>>();
    current
        .ensure_reload_compatible(&candidate, &live_uids)
        .map_err(GenerationReloadError::Incompatible)?;
    let exports =
        crate::script::prepare_exports(&candidate).map_err(GenerationReloadError::Rejected)?;
    let mut prepared = Vec::with_capacity(instances.len());
    let mut refreshes = Vec::with_capacity(instances.len());
    for (instance, state) in instances.iter().zip(&states) {
        let interface = instance
            .interface
            .ok_or_else(|| rejected("Godot interface is unavailable"))?;
        let script = candidate
            .script_by_uid(state.resource_uid())
            .ok_or_else(|| rejected("candidate script is missing"))?;
        let mut next =
            with_engine_interface(interface, || script.create_state()).map_err(|error| {
                rejected(format!(
                    "could not create `{}`: {:?}: {}",
                    state.source_path(),
                    error.status,
                    error.message
                ))
            })?;
        migrate_state(instance, state, &mut next)?;
        ensure_instance_attached(instance)?;
        let refresh = OwnerRefresh::prepare(instance, &next)?;
        if next.has_node_fields() && refresh.node_ready {
            crate::node_resolver::inject_node_fields(
                interface,
                instance.owner as GDExtensionObjectPtr,
                &mut next,
            )
            .map_err(|error| {
                GenerationReloadError::Incompatible(format!(
                    "could not resolve node fields for `{}`: {:?}: {}",
                    state.source_path(),
                    error.status,
                    error.message
                ))
            })?;
        }
        let next = prepare_instance(
            interface,
            instance.script as GDExtensionObjectPtr,
            instance.owner as GDExtensionObjectPtr,
            instance.language as GDExtensionScriptLanguagePtr,
            next,
            instance.identity.clone(),
        )
        .ok_or_else(|| {
            rejected(format!(
                "could not prepare metadata for `{}`",
                state.source_path()
            ))
        })?;
        #[allow(clippy::arc_with_non_send_sync)]
        prepared.push(Arc::new(next));
        refreshes.push(refresh);
    }
    // Cancellation runs only after migration succeeds. Futures may release
    // owners; validate the registry again afterwards, outside any registry lock.
    current.cancel_tasks().map_err(|error| {
        if error.status == AbiStatus::Unsupported {
            GenerationReloadError::Busy
        } else {
            rejected(format!(
                "could not cancel active tasks: {:?}: {}",
                error.status, error.message
            ))
        }
    })?;
    for instance in &instances {
        ensure_instance_attached(instance)?;
    }
    let connections = instances
        .iter()
        .zip(&prepared)
        .map(|(old, next)| {
            super::connections::ConnectionCleanup::prepare(old, next).map_err(|error| {
                rejected(format!("could not prepare signal connections: {error:?}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let registry = LIVE_INSTANCES
        .get()
        .expect("initialized registry")
        .lock()
        .map_err(|_| rejected("live instance registry is poisoned"))?;
    let mut locked_slots = slots
        .iter()
        .map(|slot| {
            slot.current.try_lock().map_err(|error| match error {
                TryLockError::WouldBlock => GenerationReloadError::Busy,
                TryLockError::Poisoned(_) => rejected("instance slot is poisoned"),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if registry.len() != slots.len()
        || instances
            .iter()
            .any(|instance| !registry.contains(&instance.identity.address.load(Ordering::Acquire)))
    {
        return Err(GenerationReloadError::Busy);
    }
    for (slot, next) in locked_slots.iter_mut().zip(&prepared) {
        // The instances vector keeps retired states alive until all locks drop.
        **slot = next.clone();
    }
    crate::module_loader::set_active_generation(candidate);
    drop(registry);
    drop(locked_slots);
    drop(states);
    for (instance, connections) in prepared.iter().zip(connections) {
        connections.apply(instance);
    }
    for (instance, refresh) in prepared.iter().zip(refreshes) {
        refresh.apply(instance);
    }
    exports.apply();
    Ok(prepared.len())
}

fn rejected(message: impl Into<String>) -> GenerationReloadError {
    GenerationReloadError::Rejected(message.into())
}

fn ensure_instance_attached(instance: &RustScriptInstance) -> Result<(), GenerationReloadError> {
    if instance.identity.godot_instance.load(Ordering::Acquire) == 0 {
        return Err(GenerationReloadError::Busy);
    }
    Ok(())
}

struct OwnerRefresh {
    node_ready: bool,
    notify: usize,
    processing: Vec<(usize, bool)>,
}

impl OwnerRefresh {
    fn prepare(
        instance: &RustScriptInstance,
        next: &ModuleState,
    ) -> Result<Self, GenerationReloadError> {
        let interface = instance
            .interface
            .ok_or_else(|| rejected("Godot interface is unavailable"))?;
        let resolve = |class, method, hash| {
            crate::runtime::resolve_method(interface, class, method, hash)
                .map(|method| method as usize)
                .map_err(|error| rejected(error.to_string()))
        };
        let notify = resolve(c"Object", c"notify_property_list_changed", 3_218_959_716)?;
        let mut refresh = Self {
            node_ready: false,
            notify,
            processing: Vec::new(),
        };
        if !instance
            .codec
            .as_ref()
            .is_some_and(|codec| codec.object_is_class(instance.owner_id, "Node"))
        {
            return Ok(refresh);
        }
        let ready = resolve(c"Node", c"is_node_ready", 36_873_697)?;
        let ptrcall = interface
            .object_method_bind_ptrcall
            .ok_or_else(|| rejected("Godot ptrcall is unavailable"))?;
        let mut node_ready: GDExtensionBool = 0;
        // SAFETY: The owner is a live Node; is_node_ready returns a native bool.
        unsafe {
            ptrcall(
                ready as GDExtensionMethodBindPtr,
                instance.owner as GDExtensionObjectPtr,
                ptr::null(),
                ptr::from_mut(&mut node_ready).cast(),
            );
        }
        refresh.node_ready = node_ready != 0;
        if refresh.node_ready {
            for (method, old, new) in [
                (
                    c"set_process",
                    instance.available.process,
                    next.has_process(),
                ),
                (
                    c"set_physics_process",
                    instance.available.physics_process,
                    next.has_physics_process(),
                ),
                (
                    c"set_process_input",
                    instance.available.input,
                    next.has_input(),
                ),
                (
                    c"set_process_unhandled_input",
                    instance.available.unhandled_input,
                    next.has_unhandled_input(),
                ),
            ] {
                if old != new {
                    refresh
                        .processing
                        .push((resolve(c"Node", method, 2_586_408_642)?, new));
                }
            }
        }
        Ok(refresh)
    }

    fn apply(self, instance: &RustScriptInstance) {
        if ensure_instance_attached(instance).is_err() {
            return;
        }
        let Some(interface) = instance.interface else {
            return;
        };
        let Some(ptrcall) = interface.object_method_bind_ptrcall else {
            return;
        };
        for (method, enabled) in self.processing {
            let enabled = GDExtensionBool::from(enabled);
            let arguments = [ptr::from_ref(&enabled).cast()];
            // SAFETY: Prepared Node methods each accept one native bool.
            unsafe {
                ptrcall(
                    method as GDExtensionMethodBindPtr,
                    instance.owner as GDExtensionObjectPtr,
                    arguments.as_ptr(),
                    ptr::null_mut(),
                );
            }
        }
        // SAFETY: The owner remains attached. No locks are held while Godot
        // emits property_list_changed, which may reenter or remove this script.
        unsafe {
            ptrcall(
                self.notify as GDExtensionMethodBindPtr,
                instance.owner as GDExtensionObjectPtr,
                ptr::null(),
                ptr::null_mut(),
            );
        }
    }
}

fn migrate_state(
    instance: &RustScriptInstance,
    current: &ModuleState,
    candidate: &mut ModuleState,
) -> Result<(), GenerationReloadError> {
    let codec = instance.codec.as_ref().ok_or_else(|| {
        GenerationReloadError::Rejected("Godot Variant codec is unavailable".into())
    })?;
    let interface = instance
        .interface
        .ok_or_else(|| GenerationReloadError::Rejected("Godot interface is unavailable".into()))?;
    let mut next_fields = (0..candidate.field_count())
        .filter_map(|index| candidate.field(index))
        .map(|field| {
            (
                (field.script_resource_uid(), field.name().to_owned()),
                field,
            )
        })
        .collect::<HashMap<_, _>>();
    with_engine_interface(interface, || {
        for index in 0..current.field_count() {
            let Some(field) = current.field(index) else {
                continue;
            };
            if !field.persists_on_reload() {
                continue;
            }
            let next_field =
                next_fields.remove(&(field.script_resource_uid(), field.name().to_owned()));
            let Some(next_field) = next_field else {
                continue;
            };
            if !next_field.persists_on_reload() {
                continue;
            }
            let expected = next_field.value_type().ok_or_else(|| {
                rejected(format!(
                    "persistent field `{}` has no migration value contract",
                    field.name()
                ))
            })?;
            if field.value_type() != Some(expected)
                || field.typed_array_element() != next_field.typed_array_element()
            {
                return Err(rejected(format!(
                    "field `{}::{}` changed its stored type; keep the type, rename the field, or use #[reload(skip)]",
                    current.source_path(),
                    field.name()
                )));
            }
            let value = current.get_field(&field).map_err(|error| {
                GenerationReloadError::Rejected(format!(
                    "could not capture `{}`: {:?}: {}",
                    field.name(),
                    error.status,
                    error.message
                ))
            })?;
            let property_bag_value = OwnedVariant::from_abi_with_context(
                codec,
                value.borrowed_abi(),
                field.typed_array_element(),
                Some(value.engine_call_context()),
            )
            .map_err(|_| {
                GenerationReloadError::Rejected(format!(
                    "could not store `{}` in the Host property bag",
                    field.name()
                ))
            })?;
            let mut strings = Vec::new();
            let mut math = Vec::new();
            let mut packed = Vec::new();
            let mut dynamic = Vec::new();
            let mut callable = Vec::new();
            let next_owner = candidate.value_owner();
            let mut decoded = codec
                .decode(
                    property_bag_value.as_ptr(),
                    expected,
                    VariantDecodeBacking {
                        strings: &mut strings,
                        math: &mut math,
                        packed: &mut packed,
                        dynamic: &mut dynamic,
                        callable: &mut callable,
                        dynamic_context: Some(next_owner.engine_call_context()),
                    },
                )
                .map_err(|_| {
                    GenerationReloadError::Rejected(format!(
                        "candidate field `{}` rejected the stored Godot value type",
                        field.name()
                    ))
                })?;
            if expected == AbiValueType::OBJECT_ID {
                if let Some(class) = next_field.property_object_class() {
                    if !codec.object_is_class(decoded.payload[0], class) {
                        return Err(rejected(format!(
                            "field `{}` requires a `{class}` object",
                            field.name()
                        )));
                    }
                }
            }
            if expected == AbiValueType::OBJECT_ID && next_field.owns_property_object() {
                let interface = instance.interface.ok_or_else(|| {
                    GenerationReloadError::Rejected("Godot interface is unavailable".into())
                })?;
                decoded = next_owner
                    .engine_call_context()
                    .retain_refcounted_object(interface, decoded.payload[0])
                    .map_err(|error| {
                        GenerationReloadError::Rejected(format!(
                            "could not retain `{}` for the candidate generation: {error:?}",
                            field.name()
                        ))
                    })?;
            }
            candidate.set_field(&next_field, decoded).map_err(|error| {
                GenerationReloadError::Rejected(format!(
                    "could not restore `{}`: {:?}: {}",
                    field.name(),
                    error.status,
                    error.message
                ))
            })?;
        }
        Ok(())
    })
}
