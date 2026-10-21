//! Godot's Object callbacks and post-commit script restoration.
use super::*;
use crate::value::{read_utf8_string, write_utf8_string};

pub(super) struct Methods {
    indices: [Option<usize>; 9],
}
impl Methods {
    pub(super) fn new(methods: &[ReflectedMethod]) -> Self {
        let mut result = Self { indices: [None; 9] };
        for (index, method) in methods.iter().enumerate() {
            if let Some(slot) = Self::slot(method.method.name()) {
                result.indices[slot] = Some(index);
            }
        }
        result
    }
    fn slot(name: &str) -> Option<usize> {
        match name {
            "_notification" => Some(0),
            "_to_string" => Some(1),
            "_get" => Some(2),
            "_set" => Some(3),
            "_get_property_list" => Some(4),
            "_validate_property" => Some(5),
            "_property_can_revert" => Some(6),
            "_property_get_revert" => Some(7),
            "_reloaded" => Some(8),
            _ => None,
        }
    }
    fn find(&self, name: &str) -> Option<usize> {
        self.indices[Self::slot(name)?]
    }
}

pub(super) fn invoke(
    instance: &RustScriptInstance,
    name: &str,
    arguments: &[GDExtensionConstVariantPtr],
) -> Option<OwnedVariant> {
    let method = instance.methods.get(instance.object_hooks.find(name)?)?;
    if instance.faults.is_fused() || instance.identity.godot_instance.load(Ordering::Acquire) == 0 {
        return None;
    }
    let mut result = OwnedVariant::from_abi(instance.codec.as_ref()?, AbiValueV1::NIL).ok()?;
    let mut error = GDExtensionCallError {
        error: GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
        argument: 0,
        expected: 0,
    };
    // SAFETY: The instance, cached name, argument Variants and output remain
    // alive for this synchronous ScriptInstance dispatch.
    unsafe {
        call(
            instance.identity.address.load(Ordering::Acquire) as GDExtensionScriptInstanceDataPtr,
            method.name.as_ptr(),
            arguments.as_ptr(),
            arguments.len() as i64,
            result.as_mut_ptr(),
            &mut error,
        )
    };
    result.mark_initialized();
    (error.error == GDExtensionCallErrorType::GDEXTENSION_CALL_OK).then_some(result)
}

fn from_name(
    instance: &RustScriptInstance,
    name: GDExtensionConstStringNamePtr,
) -> Option<OwnedVariant> {
    if name.is_null() {
        return None;
    }
    let interface = instance.interface?;
    let constructor = interface.get_variant_from_type_constructor?;
    let value_type = GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME;
    // SAFETY: Godot's resolver accepts this official StringName type discriminant.
    let constructor = unsafe { constructor(value_type) }?;
    let mut value = OwnedVariant::uninitialized(interface);
    // SAFETY: Godot supplies a live StringName; output is uninitialized Variant storage.
    unsafe { constructor(value.as_mut_ptr(), name.cast_mut()) };
    value.mark_initialized();
    Some(value)
}

pub(super) fn get(
    instance: &RustScriptInstance,
    name: GDExtensionConstStringNamePtr,
    result: GDExtensionVariantPtr,
) -> bool {
    let Some(name) = from_name(instance, name) else {
        return false;
    };
    let Some(value) = invoke(instance, "_get", &[name.as_ptr()]) else {
        return false;
    };
    let Some(codec) = instance.codec.as_ref() else {
        return false;
    };
    if codec.variant_type(value.as_ptr())
        == Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL)
    {
        return false;
    }
    copy_output(&value, result)
}

pub(super) fn set(
    instance: &RustScriptInstance,
    name: GDExtensionConstStringNamePtr,
    value: GDExtensionConstVariantPtr,
) -> bool {
    let Some(name) = from_name(instance, name) else {
        return false;
    };
    invoke(instance, "_set", &[name.as_ptr(), value])
        .and_then(|value| instance.codec.as_ref()?.read_bool(value.as_ptr()).ok())
        .unwrap_or(false)
}

pub(super) fn notification(instance: &RustScriptInstance, what: i32) {
    // Avoid constructing argument storage for the common no-hook case.
    if instance.object_hooks.find("_notification").is_none() {
        return;
    }
    let Some(codec) = instance.codec.as_ref() else {
        return;
    };
    let Ok(value) = OwnedVariant::from_abi(codec, AbiValueV1::from_i64(what.into())) else {
        return;
    };
    invoke(instance, "_notification", &[value.as_ptr()]);
}

pub(super) fn to_string(instance: &RustScriptInstance, result: GDExtensionStringPtr) -> bool {
    let Some(value) = invoke(instance, "_to_string", &[]) else {
        return false;
    };
    let Some(codec) = instance.codec.as_ref() else {
        return false;
    };
    let Ok(text) = codec.read_string_value(value.as_ptr()) else {
        return false;
    };
    // ScriptInstanceExtension supplies an initialized Godot String here.
    replace_text(instance, result, &text, false)
}

pub(super) fn can_revert(
    instance: &RustScriptInstance,
    name: GDExtensionConstStringNamePtr,
) -> Option<bool> {
    let name = from_name(instance, name)?;
    let value = invoke(instance, "_property_can_revert", &[name.as_ptr()])?;
    instance.codec.as_ref()?.read_bool(value.as_ptr()).ok()
}

pub(super) fn get_revert(
    instance: &RustScriptInstance,
    name: GDExtensionConstStringNamePtr,
    result: GDExtensionVariantPtr,
) -> bool {
    let Some(name) = from_name(instance, name) else {
        return false;
    };
    let Some(value) = invoke(instance, "_property_get_revert", &[name.as_ptr()]) else {
        return false;
    };
    copy_output(&value, result)
}

fn copy_output(value: &OwnedVariant, result: GDExtensionVariantPtr) -> bool {
    let Some(copy) = value.interface().variant_new_copy else {
        return false;
    };
    if result.is_null() {
        return false;
    }
    let Some(destroy) = value.interface().variant_destroy else {
        return false;
    };
    // SAFETY: ScriptInstance callbacks receive initialized Variant outputs;
    // release the previous value before placement-constructing its replacement.
    unsafe {
        destroy(result);
        copy(result, value.as_ptr());
    };
    true
}

fn named(instance: &RustScriptInstance, value: &OwnedVariant, field: &str) -> Option<OwnedVariant> {
    let interface = instance.interface?;
    let getter = interface.variant_get_named?;
    let name = OwnedStringName::new(interface, field)?;
    let mut result = OwnedVariant::uninitialized(interface);
    let mut valid = 0;
    // SAFETY: The Dictionary and StringName are initialized; Godot initializes
    // the result Variant even when the requested key is absent.
    unsafe {
        getter(
            value.as_ptr(),
            name.as_ptr(),
            result.as_mut_ptr(),
            &mut valid,
        )
    };
    result.mark_initialized();
    (valid != 0).then_some(result)
}

fn named_integer(instance: &RustScriptInstance, value: &OwnedVariant, field: &str) -> Option<u32> {
    let value = named(instance, value, field)?;
    u32::try_from(instance.codec.as_ref()?.read_i64(value.as_ptr()).ok()?).ok()
}

fn text(instance: &RustScriptInstance, value: &OwnedVariant) -> Option<String> {
    let codec = instance.codec.as_ref()?;
    if let Ok(text) = codec.read_string_value(value.as_ptr()) {
        return Some(text);
    }
    if codec.variant_type(value.as_ptr())
        != Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME)
    {
        return None;
    }
    let interface = instance.interface?;
    let convert = interface.get_variant_to_type_constructor?;
    // SAFETY: STRING_NAME is an official builtin type.
    let convert = unsafe { convert(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME) }?;
    let mut name = OwnedStringName::empty(interface)?;
    let destroy = interface.variant_get_ptr_destructor?;
    // SAFETY: Release the initialized empty StringName before replacing it
    // with the matching from-Variant constructor.
    unsafe {
        destroy(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME)?(name.as_mut_ptr());
        convert(name.as_mut_ptr(), value.as_ptr().cast_mut());
    }
    name.to_utf8().ok()
}

fn named_text(instance: &RustScriptInstance, value: &OwnedVariant, field: &str) -> Option<String> {
    text(instance, &named(instance, value, field)?)
}

fn put(
    instance: &RustScriptInstance,
    dict: &mut OwnedVariant,
    field: &str,
    value: AbiValueV1,
) -> Option<()> {
    let interface = instance.interface?;
    let setter = interface.variant_set_named?;
    let name = OwnedStringName::new(interface, field)?;
    let value = OwnedVariant::from_abi(instance.codec.as_ref()?, value).ok()?;
    let mut valid = 0;
    // SAFETY: The mutable Dictionary, name and value remain live for this call.
    unsafe { setter(dict.as_mut_ptr(), name.as_ptr(), value.as_ptr(), &mut valid) };
    (valid != 0).then_some(())
}

struct PropertyBacking {
    name: OwnedStringName,
    class_name: OwnedStringName,
    hint_string: LocalGodotString,
    type_: GDExtensionVariantType,
    hint: u32,
    usage: u32,
}

pub(super) struct PropertyList {
    // Names/hints remain alive until Godot releases exactly this list.
    _backing: Vec<PropertyBacking>,
    pub(super) infos: Vec<GDExtensionPropertyInfo>,
    _generation: Arc<RustScriptInstance>,
}

fn dynamic_properties(instance: &Arc<RustScriptInstance>) -> Option<Vec<PropertyBacking>> {
    let value = invoke(instance, "_get_property_list", &[])?;
    let codec = instance.codec.as_ref()?;
    let interface = instance.interface?;
    let array = crate::dynamic_value::NativeDynamic::from_shared_variant(
        codec,
        AbiValueType::ARRAY,
        &value,
    )
    .ok()?;
    let call = interface.variant_call?;
    let name = StaticStringName::new(interface, c"size");
    let mut count = OwnedVariant::uninitialized(interface);
    let mut error = GDExtensionCallError {
        error: GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
        argument: 0,
        expected: 0,
    };
    // SAFETY: Array.size() has no arguments and initializes one Variant result.
    unsafe {
        call(
            value.as_ptr().cast_mut(),
            name.as_ptr(),
            ptr::null(),
            0,
            count.as_mut_ptr(),
            &mut error,
        )
    };
    count.mark_initialized();
    let count = usize::try_from(codec.read_i64(count.as_ptr()).ok()?).ok()?;
    if error.error != GDExtensionCallErrorType::GDEXTENSION_CALL_OK || count > 4096 {
        return None;
    }
    let getter = interface.array_operator_index_const?;
    let copy = interface.variant_new_copy?;
    let mut backing = Vec::with_capacity(count);
    let mut names = HashSet::new();
    for index in 0..count {
        let mut property = OwnedVariant::uninitialized(interface);
        // SAFETY: Index is below the validated array size. Copy the live
        // element into this uninitialized, independently owned Variant.
        let element = unsafe { getter(array.as_const_ptr(), index as i64) };
        if element.is_null() {
            return None;
        }
        // SAFETY: The array retains this element while its Variant is copied.
        unsafe { copy(property.as_mut_ptr(), element as GDExtensionConstVariantPtr) };
        property.mark_initialized();
        if codec.variant_type(property.as_ptr())
            != Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY)
        {
            return None;
        }
        let name = named_text(instance, &property, "name")?;
        if name.is_empty()
            || !names.insert(name.clone())
            || instance
                .properties
                .iter()
                .any(|property| property.name.to_utf8().ok().as_deref() == Some(&name))
        {
            return None;
        }
        let type_ = named_integer(instance, &property, "type")?;
        if type_ >= GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VARIANT_MAX.0 {
            return None;
        }
        backing.push(PropertyBacking {
            name: OwnedStringName::new(interface, &name)?,
            class_name: OwnedStringName::new(
                interface,
                &named_text(instance, &property, "class_name").unwrap_or_default(),
            )?,
            hint_string: LocalGodotString::new_utf8(
                interface,
                &named_text(instance, &property, "hint_string").unwrap_or_default(),
            )?,
            type_: GDExtensionVariantType(type_),
            hint: named_integer(instance, &property, "hint").unwrap_or_default(),
            usage: named_integer(instance, &property, "usage").unwrap_or(PROPERTY_USAGE_DEFAULT),
        });
    }
    Some(backing)
}

pub(super) fn property_list(instance: &Arc<RustScriptInstance>) -> Option<PropertyList> {
    let validates = instance.object_hooks.find("_validate_property").is_some();
    let dynamic = if instance.object_hooks.find("_get_property_list").is_some() {
        dynamic_properties(instance)?
    } else {
        Vec::new()
    };
    if !validates && dynamic.is_empty() {
        return None;
    }
    let interface = instance.interface?;
    let mut backing = Vec::new();
    if validates {
        // Validation belongs to the instance. Copy names/hints into this lease
        // so it cannot modify the generation's shared reflected metadata.
        for info in &instance.property_infos {
            let name = text(instance, &from_name(instance, info.name)?)?;
            let class_name = text(instance, &from_name(instance, info.class_name)?)?;
            let hint_string = read_utf8_string(interface, info.hint_string).ok()?;
            backing.push(PropertyBacking {
                name: OwnedStringName::new(interface, &name)?,
                class_name: OwnedStringName::new(interface, &class_name)?,
                hint_string: LocalGodotString::new_utf8(interface, &hint_string)?,
                type_: info.type_,
                hint: info.hint,
                usage: info.usage,
            });
        }
    }
    backing.extend(dynamic);
    let mut infos = if validates {
        Vec::new()
    } else {
        instance.property_infos.clone()
    };
    // Derive writable pointers from exclusive borrows for in-place validation.
    infos.extend(backing.iter_mut().map(|property| GDExtensionPropertyInfo {
        type_: property.type_,
        name: property.name.as_mut_ptr(),
        class_name: property.class_name.as_mut_ptr(),
        hint: property.hint,
        hint_string: property.hint_string.as_mut_ptr(),
        usage: property.usage,
    }));
    if validates {
        for property in infos.iter_mut().take(instance.property_infos.len()) {
            validate(instance, property);
        }
    }
    Some(PropertyList {
        _backing: backing,
        infos,
        _generation: instance.clone(),
    })
}

pub(super) fn validate(
    instance: &RustScriptInstance,
    property: *mut GDExtensionPropertyInfo,
) -> bool {
    if property.is_null() || instance.object_hooks.find("_validate_property").is_none() {
        return false;
    }
    // SAFETY: Godot retains this writable, initialized property for this callback.
    let property = unsafe { &mut *property };
    let Some(interface) = instance.interface else {
        return false;
    };
    let Some(codec) = instance.codec.as_ref() else {
        return false;
    };
    let Ok(mut dict) =
        crate::dynamic_value::NativeDynamic::empty(interface, AbiValueType::DICTIONARY)
            .and_then(|dict| dict.into_variant(codec))
    else {
        return false;
    };
    let Some(name) = from_name(instance, property.name) else {
        return false;
    };
    let name = text(instance, &name).unwrap_or_default();
    let class_name = from_name(instance, property.class_name)
        .and_then(|name| text(instance, &name))
        .unwrap_or_default();
    let hint_string = read_utf8_string(interface, property.hint_string).unwrap_or_default();
    let fields = [
        ("name", AbiValueV1::from_borrowed_utf8(&name)),
        ("class_name", AbiValueV1::from_borrowed_utf8(&class_name)),
        ("type", AbiValueV1::from_i64(property.type_.0.into())),
        ("hint", AbiValueV1::from_i64(property.hint.into())),
        ("hint_string", AbiValueV1::from_borrowed_utf8(&hint_string)),
        ("usage", AbiValueV1::from_i64(property.usage.into())),
    ];
    for (name, value) in fields {
        if put(instance, &mut dict, name, value).is_none() {
            return false;
        }
    }
    if invoke(instance, "_validate_property", &[dict.as_ptr()]).is_none() {
        return false;
    }
    let Some(type_) = named_integer(instance, &dict, "type")
        .filter(|type_| *type_ < GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VARIANT_MAX.0)
    else {
        return false;
    };
    let Some(hint) = named_integer(instance, &dict, "hint") else {
        return false;
    };
    let Some(usage) = named_integer(instance, &dict, "usage") else {
        return false;
    };
    let Some(name) = named_text(instance, &dict, "name") else {
        return false;
    };
    let Some(class_name) = named_text(instance, &dict, "class_name") else {
        return false;
    };
    let Some(hint_string) = named_text(instance, &dict, "hint_string") else {
        return false;
    };
    property.type_ = GDExtensionVariantType(type_);
    property.hint = hint;
    property.usage = usage;
    replace_text(instance, property.name, &name, true)
        && replace_text(instance, property.class_name, &class_name, true)
        && replace_text(instance, property.hint_string, &hint_string, false)
}

fn replace_text(
    instance: &RustScriptInstance,
    destination: GDExtensionTypePtr,
    text: &str,
    name: bool,
) -> bool {
    if destination.is_null() {
        return false;
    }
    let Some(interface) = instance.interface else {
        return false;
    };
    let type_ = if name {
        GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME
    } else {
        GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING
    };
    let Some(get_destructor) = interface.variant_get_ptr_destructor else {
        return false;
    };
    // SAFETY: Godot supplies initialized storage of the selected builtin type.
    let Some(destroy) = (unsafe { get_destructor(type_) }) else {
        return false;
    };
    // SAFETY: Destination is initialized and exclusively borrowed for this callback.
    unsafe { destroy(destination) };
    if name {
        crate::value::write_utf8_string_name(interface, destination, text)
    } else {
        write_utf8_string(interface, destination, text)
    }
}
