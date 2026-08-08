use core::ptr;
use godot_api::abi::{
    ABI_PROPERTY_USAGE_GROUP, AbiPropertyType, AbiReceiverKind, AbiRpcMode, AbiRpcTransferMode,
    AbiValueType,
};
use godot_api::{
    GDExtensionCallError, GDExtensionCallErrorType, GDExtensionConstTypePtr,
    GDExtensionConstVariantPtr, GDExtensionPtrConstructor, GDExtensionPtrDestructor,
    GDExtensionTypeFromVariantConstructorFunc, GDExtensionTypePtr,
    GDExtensionVariantFromTypeConstructorFunc, GDExtensionVariantPtr, GDExtensionVariantType,
};

use crate::interface::EngineInterface;
use crate::module_loader::{ModuleField, ModuleMethod, ModuleScript};
use crate::string_name::OwnedStringName;
use crate::value::LocalGodotString;

const RPC_MODE_ANY_PEER: i64 = 1;
const RPC_MODE_AUTHORITY: i64 = 2;
pub(crate) const PROPERTY_USAGE_DEFAULT: u32 = 6;
pub(crate) const METHOD_FLAG_NORMAL: u32 = 1;
pub(crate) const METHOD_FLAG_CONST: u32 = 4;
pub(crate) const METHOD_FLAG_VARARG: u32 = 16;
pub(crate) const METHOD_FLAG_STATIC: u32 = 32;

#[derive(Clone, Copy)]
struct MetadataCodec {
    interface: EngineInterface,
    dictionary_new: GDExtensionPtrConstructor,
    dictionary_drop: GDExtensionPtrDestructor,
    dictionary_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    dictionary_from_variant: GDExtensionTypeFromVariantConstructorFunc,
    array_new: GDExtensionPtrConstructor,
    array_drop: GDExtensionPtrDestructor,
    array_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    array_from_variant: GDExtensionTypeFromVariantConstructorFunc,
    string_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    string_name_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    bool_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    int_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    float_to_variant: GDExtensionVariantFromTypeConstructorFunc,
    color_to_variant: GDExtensionVariantFromTypeConstructorFunc,
}

impl MetadataCodec {
    fn new(interface: EngineInterface) -> Option<Self> {
        let get_constructor = interface.variant_get_ptr_constructor?;
        let get_destructor = interface.variant_get_ptr_destructor?;
        let get_from = interface.get_variant_from_type_constructor?;
        let get_to = interface.get_variant_to_type_constructor?;
        // SAFETY: Every requested type and constructor index comes from the
        // official Godot 4.4 builtin ABI.
        let dictionary_new = unsafe {
            get_constructor(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
                0,
            )
        };
        // SAFETY: Dictionary is an official builtin with a paired destructor.
        let dictionary_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY) };
        // SAFETY: Array uses the same official builtin constructor contract.
        let array_new =
            unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY, 0) };
        // SAFETY: Array is an official builtin with a paired destructor.
        let array_drop =
            unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY) };
        // SAFETY: These are official Variant types with from-type constructors.
        let dictionary_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY) };
        // SAFETY: Array has an official from-type Variant constructor.
        let array_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY) };
        // SAFETY: These official converters initialize raw builtin outputs
        // from a Variant of the matching type.
        let dictionary_from_variant =
            unsafe { get_to(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY) };
        // SAFETY: See above.
        let array_from_variant =
            unsafe { get_to(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY) };
        // SAFETY: String has an official from-type Variant constructor.
        let string_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING) };
        // SAFETY: See above.
        let string_name_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME) };
        // SAFETY: See above.
        let bool_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL) };
        // SAFETY: See above.
        let int_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT) };
        // SAFETY: Float has an official from-type Variant constructor.
        let float_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT) };
        // SAFETY: Color has an official from-type Variant constructor.
        let color_to_variant =
            unsafe { get_from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR) };
        dictionary_new?;
        dictionary_drop?;
        dictionary_to_variant?;
        dictionary_from_variant?;
        array_new?;
        array_drop?;
        array_to_variant?;
        array_from_variant?;
        string_to_variant?;
        string_name_to_variant?;
        bool_to_variant?;
        int_to_variant?;
        float_to_variant?;
        color_to_variant?;
        interface.dictionary_operator_index?;
        interface.variant_destroy?;
        interface.variant_call?;
        Some(Self {
            interface,
            dictionary_new,
            dictionary_drop,
            dictionary_to_variant,
            dictionary_from_variant,
            array_new,
            array_drop,
            array_to_variant,
            array_from_variant,
            string_to_variant,
            string_name_to_variant,
            bool_to_variant,
            int_to_variant,
            float_to_variant,
            color_to_variant,
        })
    }
}

struct OwnedDictionary {
    codec: MetadataCodec,
    storage: usize,
}

impl OwnedDictionary {
    fn new(codec: MetadataCodec) -> Option<Self> {
        let mut dictionary = Self { codec, storage: 0 };
        let constructor = codec.dictionary_new?;
        // SAFETY: Storage is the official pointer-sized Dictionary layout and
        // constructor zero takes no arguments.
        unsafe { constructor(dictionary.as_mut_ptr(), ptr::null()) };
        Some(dictionary)
    }

    fn insert_bool(&mut self, key: &str, value: bool) -> bool {
        let raw = u8::from(value);
        self.insert_raw(key, self.codec.bool_to_variant, (&raw as *const u8).cast())
    }

    fn insert_i64(&mut self, key: &str, value: i64) -> bool {
        self.insert_raw(
            key,
            self.codec.int_to_variant,
            (&value as *const i64).cast(),
        )
    }

    fn insert_string_name(&mut self, key: &str, value: &str) -> bool {
        let Some(value) = OwnedStringName::new(self.codec.interface, value) else {
            return false;
        };
        self.insert_raw(
            key,
            self.codec.string_name_to_variant,
            value.as_ptr().cast(),
        )
    }

    fn insert_string(&mut self, key: &str, value: &str) -> bool {
        let Some(value) = LocalGodotString::new_utf8(self.codec.interface, value) else {
            return false;
        };
        self.insert_raw(key, self.codec.string_to_variant, value.as_ptr().cast())
    }

    fn insert_dictionary(&mut self, key: &str, value: &Self) -> bool {
        self.insert_raw(key, self.codec.dictionary_to_variant, value.as_ptr().cast())
    }

    fn insert_array(&mut self, key: &str, value: &OwnedArray) -> bool {
        self.insert_raw(key, self.codec.array_to_variant, value.as_ptr().cast())
    }

    fn insert_variant(&mut self, key: &str, value: &OwnedVariant) -> bool {
        let Some(index) = self.codec.interface.dictionary_operator_index else {
            return false;
        };
        let (Some(destroy), Some(copy)) = (
            self.codec.interface.variant_destroy,
            self.codec.interface.variant_new_copy,
        ) else {
            return false;
        };
        let Some(key) = OwnedVariant::string_name(self.codec, key) else {
            return false;
        };
        // SAFETY: Dictionary and key are initialized official values.
        let slot = unsafe { index(self.as_mut_ptr(), key.as_ptr()) };
        if slot.is_null() {
            return false;
        }
        // SAFETY: The indexed slot and source are initialized Variants. The
        // old slot is destroyed before Godot copy-constructs its replacement.
        unsafe {
            destroy(slot);
            copy(slot, value.as_ptr());
        }
        true
    }

    fn insert_raw(
        &mut self,
        key: &str,
        constructor: GDExtensionVariantFromTypeConstructorFunc,
        value: GDExtensionConstTypePtr,
    ) -> bool {
        let Some(index) = self.codec.interface.dictionary_operator_index else {
            return false;
        };
        let Some(destroy) = self.codec.interface.variant_destroy else {
            return false;
        };
        let Some(constructor) = constructor else {
            return false;
        };
        let Some(key) = OwnedVariant::string_name(self.codec, key) else {
            return false;
        };
        // SAFETY: Dictionary and key are initialized official values.
        let slot = unsafe { index(self.as_mut_ptr(), key.as_ptr()) };
        if slot.is_null() {
            return false;
        }
        // SAFETY: Dictionary indexing returns an initialized Variant slot. It
        // is destroyed and immediately reconstructed from the supplied value.
        unsafe {
            destroy(slot);
            constructor(slot, value.cast_mut());
        }
        true
    }

    fn write_variant(&self, output: GDExtensionVariantPtr) -> bool {
        if output.is_null() {
            return false;
        }
        let Some(constructor) = self.codec.dictionary_to_variant else {
            return false;
        };
        // SAFETY: ScriptExtension supplies uninitialized Variant return storage
        // and this constructor copies the live Dictionary.
        unsafe { constructor(output, self.as_ptr().cast_mut()) };
        true
    }

    fn write_dictionary(&self, output: GDExtensionTypePtr) -> bool {
        if output.is_null() {
            return false;
        }
        let Some(mut variant) = OwnedVariant::dictionary(self.codec, self) else {
            return false;
        };
        let Some(constructor) = self.codec.dictionary_from_variant else {
            return false;
        };
        // SAFETY: The virtual return storage is uninitialized Dictionary
        // storage and the Variant contains a Dictionary.
        unsafe { constructor(output, variant.as_mut_ptr()) };
        true
    }

    fn as_ptr(&self) -> GDExtensionConstTypePtr {
        (&self.storage as *const usize).cast()
    }

    fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        (&mut self.storage as *mut usize).cast()
    }
}

impl Drop for OwnedDictionary {
    fn drop(&mut self) {
        if let Some(destroy) = self.codec.dictionary_drop {
            // SAFETY: This wrapper owns one initialized Dictionary.
            unsafe { destroy(self.as_mut_ptr()) };
        }
    }
}

struct OwnedArray {
    codec: MetadataCodec,
    storage: usize,
}

impl OwnedArray {
    fn new(codec: MetadataCodec) -> Option<Self> {
        let mut array = Self { codec, storage: 0 };
        let constructor = codec.array_new?;
        // SAFETY: Storage is the official pointer-sized Array layout and
        // constructor zero takes no arguments.
        unsafe { constructor(array.as_mut_ptr(), ptr::null()) };
        Some(array)
    }

    fn push_dictionary(&mut self, value: &OwnedDictionary) -> bool {
        let Some(value) = OwnedVariant::dictionary(self.codec, value) else {
            return false;
        };
        self.push_variant(value.as_ptr())
    }

    fn push_string(&mut self, value: &str) -> bool {
        let Some(value) = OwnedVariant::string(self.codec, value) else {
            return false;
        };
        self.push_variant(value.as_ptr())
    }

    fn push_string_name(&mut self, value: &str) -> bool {
        let Some(value) = OwnedVariant::string_name(self.codec, value) else {
            return false;
        };
        self.push_variant(value.as_ptr())
    }

    fn set_typed_string_name(&mut self) -> bool {
        let Some(set_typed) = self.codec.interface.array_set_typed else {
            return false;
        };
        let Some(class_name) = OwnedStringName::empty(self.codec.interface) else {
            return false;
        };
        let Some(script) = OwnedVariant::nil(self.codec) else {
            return false;
        };
        // SAFETY: The Array is initialized. StringName is an official builtin
        // type and Godot requires live empty class/script metadata pointers.
        unsafe {
            set_typed(
                self.as_mut_ptr(),
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
                class_name.as_ptr(),
                script.as_ptr(),
            );
        }
        true
    }

    fn push_variant(&mut self, value: GDExtensionConstVariantPtr) -> bool {
        if value.is_null() {
            return false;
        }
        let Some(call) = self.codec.interface.variant_call else {
            return false;
        };
        let Some(mut array) = OwnedVariant::array(self.codec, self) else {
            return false;
        };
        let Some(method) = OwnedStringName::new(self.codec.interface, "push_back") else {
            return false;
        };
        let arguments: [GDExtensionConstVariantPtr; 1] = [value];
        let mut result = OwnedVariant::uninitialized(self.codec.interface);
        let mut error = GDExtensionCallError {
            error: GDExtensionCallErrorType::GDEXTENSION_CALL_OK,
            argument: 0,
            expected: 0,
        };
        // SAFETY: All inputs are initialized official values. `variant_call`
        // initializes the result Variant even when it reports a call error.
        unsafe {
            call(
                array.as_mut_ptr(),
                method.as_ptr(),
                arguments.as_ptr(),
                1,
                result.as_mut_ptr(),
                &mut error,
            )
        };
        result.mark_initialized();
        error.error == GDExtensionCallErrorType::GDEXTENSION_CALL_OK
    }

    fn write_array(&self, output: GDExtensionTypePtr) -> bool {
        if output.is_null() {
            return false;
        }
        let Some(mut variant) = OwnedVariant::array(self.codec, self) else {
            return false;
        };
        let Some(constructor) = self.codec.array_from_variant else {
            return false;
        };
        // SAFETY: The virtual return storage is uninitialized Array storage
        // and the Variant contains an Array.
        unsafe { constructor(output, variant.as_mut_ptr()) };
        true
    }

    fn as_ptr(&self) -> GDExtensionConstTypePtr {
        (&self.storage as *const usize).cast()
    }

    fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        (&mut self.storage as *mut usize).cast()
    }
}

impl Drop for OwnedArray {
    fn drop(&mut self) {
        if let Some(destroy) = self.codec.array_drop {
            // SAFETY: This wrapper owns one initialized Array.
            unsafe { destroy(self.as_mut_ptr()) };
        }
    }
}

#[repr(C, align(8))]
struct VariantStorage([u8; 40]);

struct OwnedVariant {
    interface: EngineInterface,
    storage: VariantStorage,
    initialized: bool,
}

impl OwnedVariant {
    fn nil(codec: MetadataCodec) -> Option<Self> {
        let constructor = codec.interface.variant_new_nil?;
        let mut variant = Self::uninitialized(codec.interface);
        // SAFETY: The destination is uninitialized Variant storage.
        unsafe { constructor(variant.as_mut_ptr()) };
        variant.mark_initialized();
        Some(variant)
    }

    fn string(codec: MetadataCodec, value: &str) -> Option<Self> {
        let string = LocalGodotString::new_utf8(codec.interface, value)?;
        Self::from_raw(
            codec.interface,
            codec.string_to_variant,
            string.as_ptr().cast(),
        )
    }

    fn bool(codec: MetadataCodec, value: bool) -> Option<Self> {
        let value = u8::from(value);
        Self::from_raw(
            codec.interface,
            codec.bool_to_variant,
            (&value as *const u8).cast(),
        )
    }

    fn int(codec: MetadataCodec, value: i64) -> Option<Self> {
        Self::from_raw(
            codec.interface,
            codec.int_to_variant,
            (&value as *const i64).cast(),
        )
    }

    fn float(codec: MetadataCodec, value: f64) -> Option<Self> {
        Self::from_raw(
            codec.interface,
            codec.float_to_variant,
            (&value as *const f64).cast(),
        )
    }

    fn color(codec: MetadataCodec, value: [f32; 4]) -> Option<Self> {
        Self::from_raw(
            codec.interface,
            codec.color_to_variant,
            value.as_ptr().cast(),
        )
    }

    fn string_name(codec: MetadataCodec, value: &str) -> Option<Self> {
        let name = OwnedStringName::new(codec.interface, value)?;
        Self::from_raw(
            codec.interface,
            codec.string_name_to_variant,
            name.as_ptr().cast(),
        )
    }

    fn dictionary(codec: MetadataCodec, value: &OwnedDictionary) -> Option<Self> {
        Self::from_raw(codec.interface, codec.dictionary_to_variant, value.as_ptr())
    }

    fn array(codec: MetadataCodec, value: &OwnedArray) -> Option<Self> {
        Self::from_raw(codec.interface, codec.array_to_variant, value.as_ptr())
    }

    fn from_raw(
        interface: EngineInterface,
        constructor: GDExtensionVariantFromTypeConstructorFunc,
        value: GDExtensionConstTypePtr,
    ) -> Option<Self> {
        let constructor = constructor?;
        let mut variant = Self::uninitialized(interface);
        // SAFETY: Variant storage covers every official Godot 4.4 layout and
        // the input points to a live builtin matching this constructor.
        unsafe { constructor(variant.as_mut_ptr(), value.cast_mut()) };
        variant.mark_initialized();
        Some(variant)
    }

    fn uninitialized(interface: EngineInterface) -> Self {
        Self {
            interface,
            storage: VariantStorage([0; 40]),
            initialized: false,
        }
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn as_ptr(&self) -> GDExtensionConstVariantPtr {
        self.storage.0.as_ptr().cast()
    }

    fn as_mut_ptr(&mut self) -> GDExtensionVariantPtr {
        self.storage.0.as_mut_ptr().cast()
    }
}

pub(crate) struct ValidationIssue<'a> {
    pub(crate) path: &'a str,
    pub(crate) line: i64,
    pub(crate) column: i64,
    pub(crate) message: &'a str,
}

pub(crate) struct CompletionOption<'a> {
    pub(crate) kind: i64,
    pub(crate) display: &'a str,
    pub(crate) insert_text: &'a str,
    pub(crate) location: i64,
}

pub(crate) struct LookupResult<'a> {
    pub(crate) result: i64,
    pub(crate) type_: i64,
    pub(crate) script_path: &'a str,
    pub(crate) location: i64,
    pub(crate) class_name: &'a str,
    pub(crate) class_member: &'a str,
    pub(crate) description: &'a str,
}

pub(crate) fn write_validation_result(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    valid: bool,
    functions: &[String],
    errors: &[ValidationIssue<'_>],
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let (Some(mut result), Some(mut function_values), Some(mut error_values)) = (
        OwnedDictionary::new(codec),
        OwnedArray::new(codec),
        OwnedArray::new(codec),
    ) else {
        return false;
    };
    for function in functions {
        if !function_values.push_string(function) {
            return false;
        }
    }
    for error in errors {
        let Some(mut value) = OwnedDictionary::new(codec) else {
            return false;
        };
        if !value.insert_string("path", error.path)
            || !value.insert_i64("line", error.line)
            || !value.insert_i64("column", error.column)
            || !value.insert_string("message", error.message)
            || !error_values.push_dictionary(&value)
        {
            return false;
        }
    }
    result.insert_bool("valid", valid)
        && result.insert_array("functions", &function_values)
        && result.insert_array("errors", &error_values)
        && result.write_dictionary(output)
}

pub(crate) fn write_completion_result(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    options: &[CompletionOption<'_>],
    force: bool,
    call_hint: &str,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let (Some(mut result), Some(mut option_values)) =
        (OwnedDictionary::new(codec), OwnedArray::new(codec))
    else {
        return false;
    };
    let Some(nil) = OwnedVariant::nil(codec) else {
        return false;
    };
    for option in options {
        let Some(mut value) = OwnedDictionary::new(codec) else {
            return false;
        };
        // A transparent font color asks Godot to use the current editor
        // theme. `icon` and `default_value` intentionally contain nil.
        let Some(font_color) = OwnedVariant::color(codec, [0.0, 0.0, 0.0, 0.0]) else {
            return false;
        };
        if !value.insert_i64("kind", option.kind)
            || !value.insert_string("display", option.display)
            || !value.insert_string("insert_text", option.insert_text)
            || !value.insert_variant("font_color", &font_color)
            || !value.insert_variant("icon", &nil)
            || !value.insert_variant("default_value", &nil)
            || !value.insert_i64("location", option.location)
            || !option_values.push_dictionary(&value)
        {
            return false;
        }
    }
    result.insert_i64("result", 0)
        && result.insert_array("options", &option_values)
        && result.insert_bool("force", force)
        && result.insert_string("call_hint", call_hint)
        && result.write_dictionary(output)
}

pub(crate) fn write_lookup_result(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    lookup: &LookupResult<'_>,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut result) = OwnedDictionary::new(codec) else {
        return false;
    };
    result.insert_i64("result", lookup.result)
        && result.insert_i64("type", lookup.type_)
        && result.insert_string("script_path", lookup.script_path)
        && result.insert_i64("location", lookup.location)
        && result.insert_string("class_name", lookup.class_name)
        && result.insert_string("class_member", lookup.class_member)
        && result.insert_string("description", lookup.description)
        && result.insert_bool("is_deprecated", false)
        && result.insert_string("deprecated_message", "")
        && result.insert_string("experimental_message", "")
        && result.insert_string("doc_type", "")
        && result.insert_string("enumeration", "")
        && result.insert_bool("is_bitfield", false)
        && result.insert_string("value", "")
        && result.write_dictionary(output)
}

pub(crate) fn write_script_members(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    script: &ModuleScript,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut members) = OwnedArray::new(codec) else {
        return false;
    };
    if !members.set_typed_string_name() {
        return false;
    }
    let mut names = std::collections::BTreeSet::new();
    for index in 0..script.field_count() {
        let Some(field) = script.field(index) else {
            return false;
        };
        names.insert(field.name().to_owned());
    }
    for index in 0..script.method_count() {
        let Some(method) = script.method(index) else {
            return false;
        };
        if method.kind() != godot_api::abi::AbiMethodKind::Lifecycle {
            names.insert(method.name().to_owned());
        }
    }
    for name in names {
        if !members.push_string_name(&name) {
            return false;
        }
    }
    members.write_array(output)
}

pub(crate) fn write_script_constants(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    constants: &[crate::rust_source::SourceConstant],
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut result) = OwnedDictionary::new(codec) else {
        return false;
    };
    for constant in constants {
        let value = match &constant.value {
            crate::rust_source::SourceConstantValue::Bool(value) => {
                OwnedVariant::bool(codec, *value)
            }
            crate::rust_source::SourceConstantValue::I64(value) => OwnedVariant::int(codec, *value),
            crate::rust_source::SourceConstantValue::F64(value) => {
                OwnedVariant::float(codec, *value)
            }
            crate::rust_source::SourceConstantValue::String(value) => {
                OwnedVariant::string(codec, value)
            }
        };
        let Some(value) = value else {
            return false;
        };
        if !result.insert_variant(&constant.name, &value) {
            return false;
        }
    }
    result.write_dictionary(output)
}

pub(crate) fn write_script_documentation(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    script: &ModuleScript,
    source: &str,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let (
        Some(mut documents),
        Some(mut class),
        Some(mut methods),
        Some(mut properties),
        Some(mut signals),
    ) = (
        OwnedArray::new(codec),
        OwnedDictionary::new(codec),
        OwnedArray::new(codec),
        OwnedArray::new(codec),
        OwnedArray::new(codec),
    )
    else {
        return false;
    };
    let class_name = script.global_name().unwrap_or_else(|| script.name());
    let description = crate::rust_source::find_identifier_line(source, script.name())
        .map(|line| crate::rust_source::documentation_before_line(source, line))
        .unwrap_or_default();
    let brief = description.lines().next().unwrap_or("");

    for index in 0..script.method_count() {
        let Some(method) = script.method(index) else {
            return false;
        };
        if method.kind() == godot_api::abi::AbiMethodKind::Lifecycle {
            continue;
        }
        let Some(mut value) = OwnedDictionary::new(codec) else {
            return false;
        };
        let Some(mut arguments) = OwnedArray::new(codec) else {
            return false;
        };
        for (name, type_) in method.arguments() {
            let Some(mut argument) = OwnedDictionary::new(codec) else {
                return false;
            };
            if !argument.insert_string("name", name)
                || !argument.insert_string("type", abi_value_name(type_))
                || !arguments.push_dictionary(&argument)
            {
                return false;
            }
        }
        let method_description = crate::rust_source::find_identifier_line(source, method.name())
            .map(|line| crate::rust_source::documentation_before_line(source, line))
            .unwrap_or_default();
        let qualifiers = if method.receiver() == AbiReceiverKind::Static {
            "static"
        } else {
            ""
        };
        if !value.insert_string("name", method.name())
            || !value.insert_string("return_type", abi_value_name(method.return_type()))
            || !value.insert_string("qualifiers", qualifiers)
            || !value.insert_string("description", &method_description)
            || !value.insert_string("keywords", method.rust_signature())
            || !value.insert_array("arguments", &arguments)
            || !methods.push_dictionary(&value)
        {
            return false;
        }
    }

    for index in 0..script.field_count() {
        let Some(field) = script.field(index) else {
            return false;
        };
        if field.is_signal() {
            let Some(mut value) = OwnedDictionary::new(codec) else {
                return false;
            };
            if !value.insert_string("name", field.name())
                || !value.insert_string(
                    "description",
                    &crate::rust_source::find_identifier_line(source, field.name())
                        .map(|line| crate::rust_source::documentation_before_line(source, line))
                        .unwrap_or_default(),
                )
                || !signals.push_dictionary(&value)
            {
                return false;
            }
        }
        if let Some(type_) = field.property_type() {
            let Some(mut value) = OwnedDictionary::new(codec) else {
                return false;
            };
            if !value.insert_string("name", field.name())
                || !value.insert_string("type", property_type_name(type_))
                || !value.insert_string(
                    "description",
                    &crate::rust_source::find_identifier_line(source, field.name())
                        .map(|line| crate::rust_source::documentation_before_line(source, line))
                        .unwrap_or_default(),
                )
                || !properties.push_dictionary(&value)
            {
                return false;
            }
        }
    }

    if !class.insert_string("name", class_name)
        || !class.insert_string("inherits", script.base())
        || !class.insert_string("brief_description", brief)
        || !class.insert_string("description", &description)
        || !class.insert_bool("is_script_doc", true)
        || !class.insert_string("script_path", script.source_path())
        || !class.insert_array("methods", &methods)
        || !class.insert_array("properties", &properties)
        || !class.insert_array("signals", &signals)
        || !documents.push_dictionary(&class)
    {
        return false;
    }
    documents.write_array(output)
}

pub(crate) fn write_debug_stack_info(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    frames: &[crate::debugger::DebugFrame],
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut stack) = OwnedArray::new(codec) else {
        return false;
    };
    for frame in frames {
        let Some(mut entry) = OwnedDictionary::new(codec) else {
            return false;
        };
        if !entry.insert_string("file", &frame.source)
            || !entry.insert_string("func", &frame.function)
            || !entry.insert_i64("line", frame.line)
            || !stack.push_dictionary(&entry)
        {
            return false;
        }
    }
    stack.write_array(output)
}

pub(crate) fn write_debug_string_values(
    interface: EngineInterface,
    output: GDExtensionTypePtr,
    values: &[(String, String)],
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut dictionary) = OwnedDictionary::new(codec) else {
        return false;
    };
    for (name, value) in values {
        if !dictionary.insert_string(name, value) {
            return false;
        }
    }
    dictionary.write_dictionary(output)
}

fn abi_value_name(type_: AbiValueType) -> &'static str {
    match type_ {
        AbiValueType::NIL => "void",
        AbiValueType::BOOL => "bool",
        AbiValueType::I64 => "int",
        AbiValueType::F64 => "float",
        AbiValueType::STRING => "String",
        AbiValueType::OBJECT_ID => "Object",
        AbiValueType::VECTOR2 => "Vector2",
        AbiValueType::VECTOR3 => "Vector3",
        AbiValueType::COLOR => "Color",
        AbiValueType::VECTOR2I => "Vector2i",
        AbiValueType::VECTOR3I => "Vector3i",
        AbiValueType::VECTOR4 => "Vector4",
        AbiValueType::VECTOR4I => "Vector4i",
        AbiValueType::RECT2 => "Rect2",
        AbiValueType::RECT2I => "Rect2i",
        AbiValueType::QUATERNION => "Quaternion",
        AbiValueType::PLANE => "Plane",
        AbiValueType::TRANSFORM2D => "Transform2D",
        AbiValueType::AABB => "AABB",
        AbiValueType::BASIS => "Basis",
        AbiValueType::TRANSFORM3D => "Transform3D",
        AbiValueType::PROJECTION => "Projection",
        AbiValueType::STRING_NAME => "StringName",
        AbiValueType::NODE_PATH => "NodePath",
        AbiValueType::RID => "RID",
        AbiValueType::PACKED_BYTE_ARRAY => "PackedByteArray",
        AbiValueType::PACKED_INT32_ARRAY => "PackedInt32Array",
        AbiValueType::PACKED_INT64_ARRAY => "PackedInt64Array",
        AbiValueType::PACKED_FLOAT32_ARRAY => "PackedFloat32Array",
        AbiValueType::PACKED_FLOAT64_ARRAY => "PackedFloat64Array",
        AbiValueType::PACKED_STRING_ARRAY => "PackedStringArray",
        AbiValueType::PACKED_VECTOR2_ARRAY => "PackedVector2Array",
        AbiValueType::PACKED_VECTOR3_ARRAY => "PackedVector3Array",
        AbiValueType::PACKED_COLOR_ARRAY => "PackedColorArray",
        AbiValueType::PACKED_VECTOR4_ARRAY => "PackedVector4Array",
        AbiValueType::VARIANT => "Variant",
        AbiValueType::ARRAY => "Array",
        AbiValueType::DICTIONARY => "Dictionary",
        AbiValueType::CALLABLE => "Callable",
        AbiValueType::SIGNAL => "Signal",
        _ => "Variant",
    }
}

fn property_type_name(type_: AbiPropertyType) -> &'static str {
    match property_variant_type(type_) {
        Some(value) => match value {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL => "bool",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT => "int",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT => "float",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING => "String",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME => "StringName",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH => "NodePath",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT => "Object",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY => "Array",
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY => "Dictionary",
            _ => "Variant",
        },
        None => "Variant",
    }
}

impl Drop for OwnedVariant {
    fn drop(&mut self) {
        if self.initialized {
            if let Some(destroy) = self.interface.variant_destroy {
                // SAFETY: Constructors initialize every OwnedVariant before
                // it is returned, and this wrapper destroys it exactly once.
                unsafe { destroy(self.as_mut_ptr()) };
            }
        }
    }
}

fn method_property_dictionary(
    codec: MetadataCodec,
    name: &str,
    type_: AbiValueType,
    class_name: Option<&str>,
) -> Option<OwnedDictionary> {
    let mut property = OwnedDictionary::new(codec)?;
    if !property.insert_i64("type", i64::from(variant_type(type_).0))
        || !property.insert_string_name("name", name)
        || class_name
            .is_some_and(|class_name| !property.insert_string_name("class_name", class_name))
    {
        return None;
    }
    Some(property)
}

fn method_dictionary(codec: MetadataCodec, method: &ModuleMethod) -> Option<OwnedDictionary> {
    let mut arguments = OwnedArray::new(codec)?;
    for (index, (name, type_)) in method.arguments().enumerate() {
        let argument =
            method_property_dictionary(codec, name, type_, method.argument_class_name(index))?;
        if !arguments.push_dictionary(&argument) {
            return None;
        }
    }
    let mut default_arguments = OwnedArray::new(codec)?;
    let variant_codec = crate::variant_codec::VariantCodec::new(codec.interface)?;
    let default_start = method.minimum_argument_count();
    for (offset, value) in method.default_values().ok()?.iter().enumerate() {
        let argument_index = default_start + offset;
        let typed_array_element = (method.argument_types()[argument_index] == AbiValueType::ARRAY)
            .then(|| method.argument_class_name(argument_index))
            .flatten();
        let default = crate::variant_codec::OwnedVariant::from_abi_with_context(
            &variant_codec,
            value.abi(),
            typed_array_element,
            Some(method.engine_call_context()),
        )
        .ok()?;
        if !default_arguments.push_variant(default.as_ptr()) {
            return None;
        }
    }
    let return_value =
        method_property_dictionary(codec, "", method.return_type(), method.return_class_name())?;
    let mut info = OwnedDictionary::new(codec)?;
    if !info.insert_string_name("name", method.name())
        || !info.insert_array("args", &arguments)
        || !info.insert_array("default_args", &default_arguments)
        || !info.insert_i64(
            "flags",
            i64::from(method_flags(method.receiver(), method.is_vararg())),
        )
        || !info.insert_dictionary("return", &return_value)
    {
        return None;
    }
    Some(info)
}

fn signal_dictionary(codec: MetadataCodec, field: &ModuleField) -> Option<OwnedDictionary> {
    let mut arguments = OwnedArray::new(codec)?;
    for (name, type_) in field.signal_arguments() {
        let argument = method_property_dictionary(codec, name, type_, None)?;
        if !arguments.push_dictionary(&argument) {
            return None;
        }
    }
    let default_arguments = OwnedArray::new(codec)?;
    let return_value = method_property_dictionary(codec, "", AbiValueType::NIL, None)?;
    let mut info = OwnedDictionary::new(codec)?;
    if !info.insert_string_name("name", field.name())
        || !info.insert_array("args", &arguments)
        || !info.insert_array("default_args", &default_arguments)
        || !info.insert_i64("flags", i64::from(METHOD_FLAG_NORMAL))
        || !info.insert_dictionary("return", &return_value)
    {
        return None;
    }
    Some(info)
}

fn inspector_property_dictionary(
    codec: MetadataCodec,
    field: &ModuleField,
) -> Option<OwnedDictionary> {
    let type_ = property_variant_type(field.property_type()?)?;
    let mut property = OwnedDictionary::new(codec)?;
    if !property.insert_i64("type", i64::from(type_.0))
        || !property.insert_string_name("name", field.name())
        || !property.insert_i64("hint", i64::from(field.property_hint()?))
        || !property.insert_string("hint_string", field.property_hint_string()?)
        || !property.insert_i64("usage", i64::from(field.property_usage()?))
    {
        return None;
    }
    Some(property)
}

fn property_group_dictionary(codec: MetadataCodec, name: &str) -> Option<OwnedDictionary> {
    let mut group = OwnedDictionary::new(codec)?;
    if !group.insert_i64(
        "type",
        i64::from(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL.0),
    ) || !group.insert_string_name("name", name)
        || !group.insert_i64("hint", 0)
        || !group.insert_string("hint_string", "")
        || !group.insert_i64("usage", i64::from(ABI_PROPERTY_USAGE_GROUP))
    {
        return None;
    }
    Some(group)
}

pub(crate) fn write_method_list(
    interface: EngineInterface,
    script: &ModuleScript,
    output: GDExtensionTypePtr,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut methods) = OwnedArray::new(codec) else {
        return false;
    };
    for index in 0..script.method_count() {
        let Some(method) = script.method(index) else {
            return false;
        };
        let Some(info) = method_dictionary(codec, &method) else {
            return false;
        };
        if !methods.push_dictionary(&info) {
            return false;
        }
    }
    methods.write_array(output)
}

pub(crate) fn write_method_info(
    interface: EngineInterface,
    method: &ModuleMethod,
    output: GDExtensionTypePtr,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(info) = method_dictionary(codec, method) else {
        return false;
    };
    info.write_dictionary(output)
}

pub(crate) fn write_property_list(
    interface: EngineInterface,
    script: &ModuleScript,
    output: GDExtensionTypePtr,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut properties) = OwnedArray::new(codec) else {
        return false;
    };
    let mut current_group: Option<String> = None;
    for index in 0..script.field_count() {
        let Some(field) = script.field(index) else {
            return false;
        };
        if field.property_type().is_none() {
            continue;
        }
        let next_group = field.property_group();
        if current_group.as_deref() != next_group {
            let Some(group) = property_group_dictionary(codec, next_group.unwrap_or("")) else {
                return false;
            };
            if !properties.push_dictionary(&group) {
                return false;
            }
            current_group = next_group.map(str::to_owned);
        }
        let Some(property) = inspector_property_dictionary(codec, &field) else {
            return false;
        };
        if !properties.push_dictionary(&property) {
            return false;
        }
    }
    properties.write_array(output)
}

pub(crate) fn write_signal_list(
    interface: EngineInterface,
    script: &ModuleScript,
    output: GDExtensionTypePtr,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut signals) = OwnedArray::new(codec) else {
        return false;
    };
    for index in 0..script.field_count() {
        let Some(field) = script.field(index) else {
            return false;
        };
        if !field.is_signal() {
            continue;
        }
        let Some(info) = signal_dictionary(codec, &field) else {
            return false;
        };
        if !signals.push_dictionary(&info) {
            return false;
        }
    }
    signals.write_array(output)
}

pub(crate) fn variant_type(type_: AbiValueType) -> GDExtensionVariantType {
    match type_ {
        AbiValueType::NIL => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
        AbiValueType::BOOL => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL,
        AbiValueType::I64 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
        AbiValueType::F64 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT,
        AbiValueType::STRING => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING,
        AbiValueType::STRING_NAME => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
        AbiValueType::NODE_PATH => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH,
        AbiValueType::OBJECT_ID => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
        AbiValueType::VECTOR2 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2,
        AbiValueType::VECTOR2I => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I,
        AbiValueType::VECTOR3 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3,
        AbiValueType::VECTOR3I => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I,
        AbiValueType::VECTOR4 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4,
        AbiValueType::VECTOR4I => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I,
        AbiValueType::RECT2 => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2,
        AbiValueType::RECT2I => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I,
        AbiValueType::QUATERNION => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION,
        AbiValueType::PLANE => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE,
        AbiValueType::TRANSFORM2D => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D,
        AbiValueType::AABB => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB,
        AbiValueType::BASIS => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS,
        AbiValueType::TRANSFORM3D => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D,
        AbiValueType::PROJECTION => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION,
        AbiValueType::PACKED_BYTE_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY
        }
        AbiValueType::PACKED_INT32_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY
        }
        AbiValueType::PACKED_INT64_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY
        }
        AbiValueType::PACKED_FLOAT32_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY
        }
        AbiValueType::PACKED_FLOAT64_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY
        }
        AbiValueType::PACKED_STRING_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY
        }
        AbiValueType::PACKED_VECTOR2_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY
        }
        AbiValueType::PACKED_VECTOR3_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY
        }
        AbiValueType::PACKED_COLOR_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY
        }
        AbiValueType::PACKED_VECTOR4_ARRAY => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY
        }
        AbiValueType::COLOR => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR,
        AbiValueType::RID => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID,
        AbiValueType::VARIANT => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
        AbiValueType::ARRAY => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        AbiValueType::DICTIONARY => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        AbiValueType::CALLABLE => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE,
        AbiValueType::SIGNAL => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL,
        _ => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
    }
}

pub(crate) fn property_variant_type(type_: AbiPropertyType) -> Option<GDExtensionVariantType> {
    match type_ {
        AbiPropertyType::NIL => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL),
        AbiPropertyType::BOOL => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL),
        AbiPropertyType::INT => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT),
        AbiPropertyType::FLOAT => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT),
        AbiPropertyType::STRING => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING),
        AbiPropertyType::STRING_NAME => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME)
        }
        AbiPropertyType::NODE_PATH => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH)
        }
        AbiPropertyType::RID => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID),
        AbiPropertyType::OBJECT => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT),
        AbiPropertyType::CALLABLE => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE)
        }
        AbiPropertyType::SIGNAL => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL),
        AbiPropertyType::DICTIONARY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY)
        }
        AbiPropertyType::ARRAY => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY),
        AbiPropertyType::PACKED_BYTE_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY)
        }
        AbiPropertyType::PACKED_INT32_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY)
        }
        AbiPropertyType::PACKED_INT64_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY)
        }
        AbiPropertyType::PACKED_FLOAT32_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY)
        }
        AbiPropertyType::PACKED_FLOAT64_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY)
        }
        AbiPropertyType::PACKED_STRING_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY)
        }
        AbiPropertyType::PACKED_VECTOR2_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY)
        }
        AbiPropertyType::PACKED_VECTOR3_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY)
        }
        AbiPropertyType::PACKED_COLOR_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY)
        }
        AbiPropertyType::PACKED_VECTOR4_ARRAY => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY)
        }
        AbiPropertyType::VECTOR2 => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2),
        AbiPropertyType::VECTOR2I => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I)
        }
        AbiPropertyType::VECTOR3 => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3),
        AbiPropertyType::VECTOR3I => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I)
        }
        AbiPropertyType::VECTOR4 => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4),
        AbiPropertyType::VECTOR4I => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I)
        }
        AbiPropertyType::RECT2 => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2),
        AbiPropertyType::RECT2I => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I),
        AbiPropertyType::QUATERNION => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION)
        }
        AbiPropertyType::PLANE => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE),
        AbiPropertyType::TRANSFORM2D => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D)
        }
        AbiPropertyType::AABB => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB),
        AbiPropertyType::BASIS => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS),
        AbiPropertyType::TRANSFORM3D => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D)
        }
        AbiPropertyType::PROJECTION => {
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION)
        }
        AbiPropertyType::COLOR => Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR),
        _ => None,
    }
}

pub(crate) fn method_flags(receiver: AbiReceiverKind, vararg: bool) -> u32 {
    let flags = match receiver {
        AbiReceiverKind::Shared => METHOD_FLAG_NORMAL | METHOD_FLAG_CONST,
        AbiReceiverKind::Mutable => METHOD_FLAG_NORMAL,
        AbiReceiverKind::Static => METHOD_FLAG_NORMAL | METHOD_FLAG_STATIC,
    };
    flags | if vararg { METHOD_FLAG_VARARG } else { 0 }
}

pub(crate) fn write_rpc_config(
    interface: EngineInterface,
    script: &ModuleScript,
    output: GDExtensionVariantPtr,
) -> bool {
    let Some(codec) = MetadataCodec::new(interface) else {
        return false;
    };
    let Some(mut config) = OwnedDictionary::new(codec) else {
        return false;
    };
    for index in 0..script.method_count() {
        let Some(method) = script.method(index) else {
            return false;
        };
        let Some(rpc) = method.rpc_config() else {
            continue;
        };
        let Some(mut method_config) = OwnedDictionary::new(codec) else {
            return false;
        };
        if !method_config.insert_i64("rpc_mode", rpc_mode(rpc.mode))
            || !method_config.insert_bool("call_local", rpc.call_local != 0)
            || !method_config.insert_i64("transfer_mode", transfer_mode(rpc.transfer_mode))
            || !method_config.insert_i64("channel", i64::from(rpc.channel))
            || !config.insert_dictionary(method.name(), &method_config)
        {
            return false;
        }
    }
    config.write_variant(output)
}

fn rpc_mode(mode: AbiRpcMode) -> i64 {
    if mode == AbiRpcMode::ANY_PEER {
        RPC_MODE_ANY_PEER
    } else {
        RPC_MODE_AUTHORITY
    }
}

fn transfer_mode(mode: AbiRpcTransferMode) -> i64 {
    i64::from(mode.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_enums_match_godot_4_4_values() {
        assert_eq!(rpc_mode(AbiRpcMode::ANY_PEER), 1);
        assert_eq!(rpc_mode(AbiRpcMode::AUTHORITY), 2);
        assert_eq!(transfer_mode(AbiRpcTransferMode::UNRELIABLE), 0);
        assert_eq!(transfer_mode(AbiRpcTransferMode::UNRELIABLE_ORDERED), 1);
        assert_eq!(transfer_mode(AbiRpcTransferMode::RELIABLE), 2);
    }

    #[test]
    fn opaque_variant_storage_covers_every_official_4_4_configuration() {
        assert_eq!(core::mem::size_of::<VariantStorage>(), 40);
        assert_eq!(core::mem::align_of::<VariantStorage>(), 8);
    }

    #[test]
    fn method_metadata_uses_official_variant_types_and_flags() {
        assert_eq!(
            variant_type(AbiValueType::BOOL),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL
        );
        assert_eq!(
            variant_type(AbiValueType::I64),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT
        );
        assert_eq!(
            variant_type(AbiValueType::F64),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT
        );
        assert_eq!(
            variant_type(AbiValueType::STRING),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING
        );
        assert_eq!(
            variant_type(AbiValueType::STRING_NAME),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME
        );
        assert_eq!(
            variant_type(AbiValueType::VECTOR2),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2
        );
        assert_eq!(
            variant_type(AbiValueType::VECTOR2I),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I
        );
        assert_eq!(
            variant_type(AbiValueType::VECTOR3),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3
        );
        assert_eq!(
            variant_type(AbiValueType::VECTOR3I),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I
        );
        assert_eq!(
            variant_type(AbiValueType::COLOR),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR
        );
        assert_eq!(
            variant_type(AbiValueType::RID),
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID
        );
        assert_eq!(
            method_flags(AbiReceiverKind::Shared, false),
            METHOD_FLAG_NORMAL | METHOD_FLAG_CONST
        );
        assert_eq!(
            method_flags(AbiReceiverKind::Static, false),
            METHOD_FLAG_NORMAL | METHOD_FLAG_STATIC
        );
        assert_eq!(
            method_flags(AbiReceiverKind::Mutable, false),
            METHOD_FLAG_NORMAL
        );
        assert_eq!(
            method_flags(AbiReceiverKind::Shared, true),
            METHOD_FLAG_NORMAL | METHOD_FLAG_CONST | METHOD_FLAG_VARARG
        );
    }

    #[test]
    fn property_metadata_uses_official_variant_types() {
        assert_eq!(
            property_variant_type(AbiPropertyType::BOOL),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::INT),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::STRING),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::STRING_NAME),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::VECTOR2),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::VECTOR2I),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::VECTOR3),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::VECTOR3I),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I)
        );
        assert_eq!(
            property_variant_type(AbiPropertyType::COLOR),
            Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR)
        );
        assert_eq!(property_variant_type(AbiPropertyType(999)), None);
    }
}
