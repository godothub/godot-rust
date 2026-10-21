use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use super::dynamic_value::NativeVariant;
use crate::engine::builtin;
use crate::error::{EngineError, EngineResult};
use crate::variant::{Array, Dictionary, Variant};
use godot_api::abi::{ABI_DYNAMIC_MAX_ELEMENTS, AbiValueType};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
type Registry = RefCell<HashMap<u64, Weak<NativeContainerToken>>>;
thread_local! { static TOKENS: Cell<*mut Registry> = const { Cell::new(core::ptr::null_mut()) }; }

fn with_tokens<R>(operation: impl FnOnce(&Registry) -> R) -> R {
    TOKENS.with(|slot| {
        let pointer = if slot.get().is_null() {
            let pointer = Box::into_raw(Box::new(Registry::new(HashMap::new())));
            slot.set(pointer);
            pointer
        } else {
            slot.get()
        };
        // SAFETY: The owning Godot thread exclusively allocates and accesses this TLS registry.
        operation(unsafe { &*pointer })
    })
}

pub(crate) fn clear_thread_local_state() {
    TOKENS.with(|slot| {
        let pointer = slot.replace(core::ptr::null_mut());
        if !pointer.is_null() {
            // SAFETY: The slot transfers its unique allocation after callbacks have stopped.
            drop(unsafe { Box::from_raw(pointer) });
        }
    });
}

pub(crate) struct NativeContainerToken {
    id: u64,
    kind: AbiValueType,
    value: NativeVariant,
}

impl NativeContainerToken {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(super) fn value(&self) -> &NativeVariant {
        &self.value
    }
    pub(super) fn insert(kind: AbiValueType, value: NativeVariant) -> EngineResult<Rc<Self>> {
        let id = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| {
                EngineError::invalid_result("Native container identity space is exhausted")
            })?;
        let result = Rc::new(Self { id, kind, value });
        with_tokens(|tokens| {
            let mut tokens = tokens.borrow_mut();
            if tokens.len() >= 65_536 {
                return Err(EngineError::invalid_result(
                    "Native container ownership limit is exceeded",
                ));
            }
            tokens.insert(id, Rc::downgrade(&result));
            Ok(())
        })?;
        Ok(result)
    }
    pub(crate) fn snapshot(&self) -> EngineResult<Vec<Variant>> {
        self.value.container_snapshot(self.kind)
    }
    pub(crate) fn assign(self: &Rc<Self>, bytes: &[u8]) -> EngineResult<()> {
        match self.kind {
            AbiValueType::ARRAY => {
                let mut target = Array::<Variant>::from_native(Rc::clone(self));
                if target.is_read_only()? {
                    return Err(EngineError::invalid_argument("Godot Array is read-only"));
                }
                let type_id = builtin::array::get_typed_builtin(&target)?;
                let class = builtin::array::get_typed_class_name(&target)?;
                let element = if type_id == 0 {
                    None
                } else if type_id == 24 {
                    Some(class.as_str())
                } else {
                    crate::variant::builtin_type_name(type_id)
                };
                let source = materialize_container(AbiValueType::ARRAY, bytes, element)?;
                let source = Array::<Variant>::from_native(source);
                builtin::array::assign(&mut target, &source)
            }
            AbiValueType::DICTIONARY => {
                let mut target = Dictionary::from_native(Rc::clone(self));
                if target.is_read_only()? {
                    return Err(EngineError::invalid_argument(
                        "Godot Dictionary is read-only",
                    ));
                }
                let source = materialize_container(AbiValueType::DICTIONARY, bytes, None)?;
                let source = Dictionary::from_native(source);
                builtin::dictionary::assign(&mut target, &source)
            }
            _ => Err(EngineError::invalid_argument(
                "invalid Native container kind",
            )),
        }
    }
}

impl Drop for NativeContainerToken {
    fn drop(&mut self) {
        // Shutdown can already have destroyed the weak registry.
        TOKENS.with(|slot| {
            let pointer = slot.get();
            if !pointer.is_null() {
                // SAFETY: This is the still-live registry owned by the current Godot thread.
                unsafe { &*pointer }.borrow_mut().remove(&self.id);
            }
        });
    }
}

pub(crate) fn retain_container(
    kind: AbiValueType,
    id: u64,
) -> EngineResult<Rc<NativeContainerToken>> {
    with_tokens(|tokens| {
        tokens
            .borrow()
            .get(&id)
            .and_then(Weak::upgrade)
            .filter(|value| value.kind == kind)
            .ok_or_else(|| {
                EngineError::stale_object("Native container token is stale or has the wrong kind")
            })
    })
}

pub(crate) fn materialize_container(
    kind: AbiValueType,
    bytes: &[u8],
    element: Option<&str>,
) -> EngineResult<Rc<NativeContainerToken>> {
    let interface = super::runtime::active_interface().ok_or_else(|| {
        EngineError::unavailable("Native containers require an active Godot callback")
    })?;
    let values = crate::variant::decode_native_snapshot(bytes, kind)?;
    if values.len() > ABI_DYNAMIC_MAX_ELEMENTS * 2 {
        return Err(EngineError::invalid_argument(
            "Native container snapshot exceeds the element limit",
        ));
    }
    let value = NativeVariant::from_container_snapshot(interface, kind, &values, element)?;
    NativeContainerToken::insert(kind, value)
}

pub(super) fn validate_mutation(
    base: Variant,
    member: &str,
    arguments: &[Variant],
) -> EngineResult<()> {
    use crate::variant::VariantKind;
    let constraint_name =
        |id, class: &crate::string_name::StringName| -> EngineResult<Option<String>> {
            if id == 0 {
                Ok(None)
            } else if id == 24 {
                Ok(Some(class.as_str().to_owned()))
            } else {
                crate::variant::builtin_type_name(id)
                    .map(|name| Some(name.to_owned()))
                    .ok_or_else(|| {
                        EngineError::invalid_argument("unknown Godot container type constraint")
                    })
            }
        };
    match base.kind() {
        VariantKind::Array(array) => {
            if member != "make_read_only" && array.is_read_only()? {
                return Err(EngineError::invalid_argument("Godot Array is read-only"));
            }
            let id = builtin::array::get_typed_builtin(array)?;
            if id == 0 {
                return Ok(());
            }
            let class = builtin::array::get_typed_class_name(array)?;
            let name = constraint_name(id, &class)?.expect("typed Array");
            let index = match member {
                "append" | "push_back" | "push_front" | "fill" => Some(0),
                "set" | "insert" | "indexed_set" => Some(1),
                _ => None,
            };
            if let Some(index) = index {
                let value = arguments.get(index).ok_or_else(|| {
                    EngineError::invalid_argument("Array mutation omitted a value")
                })?;
                super::dynamic_value::validate_element(value, &name)?;
            } else if matches!(member, "assign" | "append_array") {
                let Some(VariantKind::Array(source)) = arguments.first().map(Variant::kind) else {
                    return Err(EngineError::invalid_argument(
                        "Array assignment source has the wrong type",
                    ));
                };
                for value in source.to_vec()? {
                    super::dynamic_value::validate_element(&value, &name)?;
                }
            }
        }
        VariantKind::Dictionary(dictionary) => {
            if member != "make_read_only" && dictionary.is_read_only()? {
                return Err(EngineError::invalid_argument(
                    "Godot Dictionary is read-only",
                ));
            }
            let key_id = builtin::dictionary::get_typed_key_builtin(dictionary)?;
            let value_id = builtin::dictionary::get_typed_value_builtin(dictionary)?;
            if key_id == 0 && value_id == 0 {
                return Ok(());
            }
            let key_class = builtin::dictionary::get_typed_key_class_name(dictionary)?;
            let value_class = builtin::dictionary::get_typed_value_class_name(dictionary)?;
            let key = constraint_name(key_id, &key_class)?;
            let value = constraint_name(value_id, &value_class)?;
            let check = |k: &Variant, v: &Variant| -> EngineResult<()> {
                if let Some(name) = &key {
                    super::dynamic_value::validate_element(k, name)?;
                }
                if let Some(name) = &value {
                    super::dynamic_value::validate_element(v, name)?;
                }
                Ok(())
            };
            if matches!(member, "set" | "keyed_set" | "get_or_add") {
                let k = arguments.first().ok_or_else(|| {
                    EngineError::invalid_argument("Dictionary mutation omitted a key")
                })?;
                let v = arguments.get(1).ok_or_else(|| {
                    EngineError::invalid_argument("Dictionary mutation omitted a value")
                })?;
                check(k, v)?;
            } else if matches!(member, "assign" | "merge") {
                let Some(VariantKind::Dictionary(source)) = arguments.first().map(Variant::kind)
                else {
                    return Err(EngineError::invalid_argument(
                        "Dictionary assignment source has the wrong type",
                    ));
                };
                for (k, v) in source.snapshot()?.as_entries() {
                    check(k, v)?;
                }
            }
        }
        _ => {
            return Err(EngineError::invalid_argument(
                "container mutation has a non-container receiver",
            ));
        }
    }
    Ok(())
}
