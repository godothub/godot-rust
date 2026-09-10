use super::*;
use crate::callable_value::NativeCallable;
use crate::dynamic_value::NativeDynamic;
use crate::engine_call::value::ValueError;
use crate::signal_value::NativeSignal;

pub(super) struct ConnectionCleanup {
    connections: Vec<(NativeSignal, NativeCallable)>,
}

impl ConnectionCleanup {
    pub(super) fn prepare(
        old: &RustScriptInstance,
        next: &RustScriptInstance,
    ) -> Result<Self, ValueError> {
        let mut cleanup = Self {
            connections: Vec::new(),
        };
        for signal in &old.signals {
            let name = signal
                .name
                .to_utf8()
                .map_err(|_| internal("invalid signal name"))?;
            let compatible = next.signals.iter().any(|candidate| {
                candidate.name.to_utf8().ok().as_deref() == Some(&name)
                    && candidate.argument_types == signal.argument_types
            });
            if compatible {
                continue;
            }
            let args = [signal.name.as_ptr().cast()];
            cleanup.read_connections(
                old,
                c"get_signal_connection_list",
                3_147_814_860,
                &args,
                |_| Ok(true),
            )?;
        }
        let changed_methods = old
            .methods
            .iter()
            .filter_map(|method| {
                let name = method.method.name();
                let compatible = next
                    .methods
                    .iter()
                    .find(|candidate| candidate.method.name() == name)
                    .is_some_and(|candidate| {
                        let left = &method.method;
                        let right = &candidate.method;
                        left.argument_types() == right.argument_types()
                            && left.minimum_argument_count() == right.minimum_argument_count()
                            && left.is_vararg() == right.is_vararg()
                            && (0..left.argument_types().len()).all(|index| {
                                left.argument_class_name(index) == right.argument_class_name(index)
                            })
                    });
                (!compatible).then_some(name)
            })
            .collect::<HashSet<_>>();
        if !changed_methods.is_empty() {
            cleanup.read_connections(
                old,
                c"get_incoming_connections",
                3_995_934_104,
                &[],
                |callable| {
                    Ok(callable.object_id() == old.owner_id
                        && changed_methods.contains(callable.method()?.as_str()))
                },
            )?;
        }
        Ok(cleanup)
    }

    fn read_connections(
        &mut self,
        instance: &RustScriptInstance,
        method: &'static std::ffi::CStr,
        hash: i64,
        arguments: &[GDExtensionConstTypePtr],
        mut include: impl FnMut(&NativeCallable) -> Result<bool, ValueError>,
    ) -> Result<(), ValueError> {
        let interface = instance
            .interface
            .ok_or_else(|| internal("Godot interface is unavailable"))?;
        let codec = instance
            .codec
            .as_ref()
            .ok_or_else(|| internal("Godot Variant codec is unavailable"))?;
        let method = crate::runtime::resolve_method(interface, c"Object", method, hash)
            .map_err(|_| internal("Godot connection metadata is unavailable"))?;
        let ptrcall = interface
            .object_method_bind_ptrcall
            .ok_or_else(|| internal("Godot ptrcall is unavailable"))?;
        let mut connections = NativeDynamic::empty(interface, AbiValueType::ARRAY)?;
        // SAFETY: Object's connection queries return Array and accept either
        // no arguments or one StringName, as supplied by the two callers.
        unsafe {
            ptrcall(
                method,
                instance.owner as GDExtensionObjectPtr,
                arguments.as_ptr(),
                connections.as_mut_ptr(),
            );
        }
        let NativeDynamic::Array(connections) = connections else {
            unreachable!()
        };
        for index in 0..connections.len()? {
            let connection = connections.get(index)?;
            let callable = named_value(interface, connection, "callable")?;
            let callable = NativeCallable::from_variant(codec, callable.as_ptr())?;
            if !include(&callable)? {
                continue;
            }
            let signal = named_value(interface, connection, "signal")?;
            self.connections.push((
                NativeSignal::from_variant(codec, signal.as_ptr())?,
                callable,
            ));
        }
        Ok(())
    }

    pub(super) fn apply(self, instance: &RustScriptInstance) {
        let Some(interface) = instance.interface else {
            return;
        };
        let Some(lookup) = interface.object_get_instance_from_id else {
            return;
        };
        let mut removed = 0;
        for (mut signal, callable) in self.connections {
            // SAFETY: Resolve the signal owner anew; another notification may
            // have deleted it since the connection snapshot was prepared.
            if unsafe { lookup(signal.object_id()) }.is_null() {
                continue;
            }
            // Reference-counted connections may require multiple disconnects.
            while signal.is_connected(&callable).unwrap_or(false) {
                if signal.disconnect(&callable).is_err() {
                    break;
                }
                removed += 1;
            }
        }
        if removed != 0 {
            crate::logging::module(
                godot_api::abi::AbiLogLevel::Warning,
                format_args!(
                    "godot-rust: disconnected {removed} connection(s) for removed or changed signals/methods in `{}`",
                    instance.source_path
                ),
            );
        }
    }
}

fn named_value(
    interface: EngineInterface,
    value: GDExtensionConstVariantPtr,
    name: &str,
) -> Result<OwnedVariant, ValueError> {
    let get = interface
        .variant_get_named
        .ok_or_else(|| internal("Godot Variant member access is unavailable"))?;
    let name = OwnedStringName::new(interface, name)
        .ok_or_else(|| internal("Godot StringName is unavailable"))?;
    let mut output = OwnedVariant::uninitialized(interface);
    let mut valid = 0;
    // SAFETY: Godot initializes the Variant result even for a missing member.
    unsafe {
        get(value, name.as_ptr(), output.as_mut_ptr(), &mut valid);
    }
    output.mark_initialized();
    if valid == 0 {
        return Err(internal("Godot returned an invalid connection record"));
    }
    Ok(output)
}

fn internal(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Internal, message)
}
