use core::ptr;

use godot_api::abi::{
    ABI_DYNAMIC_MAGIC as WIRE_MAGIC, ABI_DYNAMIC_MAX_BYTES as MAX_WIRE_BYTES,
    ABI_DYNAMIC_MAX_DEPTH as MAX_NESTING_DEPTH, ABI_DYNAMIC_MAX_ELEMENTS as MAX_CONTAINER_ELEMENTS,
    ABI_DYNAMIC_VERSION as WIRE_VERSION, AbiStatus, AbiValueType, AbiValueV1,
    validate_dynamic_value,
};
use godot_api::{
    GDExtensionConstTypePtr, GDExtensionConstVariantPtr, GDExtensionPtrBuiltInMethod,
    GDExtensionPtrDestructor, GDExtensionTypePtr, GDExtensionVariantPtr, GDExtensionVariantType,
};

use crate::callable_value::NativeCallable;
use crate::engine_call::EngineCallContext;
use crate::engine_call::value::ValueError;
use crate::interface::EngineInterface;
use crate::signal_value::NativeSignal;
use crate::string_name::{OwnedStringName, StaticStringName};
use crate::variant_codec::{OwnedVariant, VariantCodec, VariantDecodeBacking};

const WIRE_HEADER_BYTES: usize = 20;
const NODE_HEADER_BYTES: usize = 16;
const SIZE_HASH: i64 = 3_173_160_232;
const RESIZE_HASH: i64 = 848_867_239;
const KEYS_HASH: i64 = 4_144_163_970;

pub(crate) fn ensure_writable(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
    value: GDExtensionConstTypePtr,
) -> Result<(), ValueError> {
    let read_only = builtin_method(interface, type_, c"is_read_only", 3_918_633_141)?
        .ok_or_else(|| internal("container read-only check is unavailable"))?;
    let mut result = 0_u8;
    // SAFETY: Receiver is an initialized Array/Dictionary and result is a bool slot.
    unsafe {
        read_only(
            value.cast_mut(),
            ptr::null(),
            ptr::from_mut(&mut result).cast(),
            0,
        )
    };
    if result != 0 {
        Err(invalid("Godot container is read-only"))
    } else {
        Ok(())
    }
}

fn container_integer(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
    value: GDExtensionConstTypePtr,
    name: &'static core::ffi::CStr,
) -> Result<i64, ValueError> {
    let method = builtin_method(interface, type_, name, SIZE_HASH)?
        .ok_or_else(|| internal("container metadata is unavailable"))?;
    let mut result = 0_i64;
    // SAFETY: Authenticated zero-argument metadata getter returns int64.
    unsafe {
        method(
            value.cast_mut(),
            ptr::null(),
            ptr::from_mut(&mut result).cast(),
            0,
        )
    };
    Ok(result)
}

fn container_class(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
    value: GDExtensionConstTypePtr,
    name: &'static core::ffi::CStr,
) -> Result<String, ValueError> {
    let method = builtin_method(interface, type_, name, 1_825_232_092)?
        .ok_or_else(|| internal("container class metadata is unavailable"))?;
    let mut result = OwnedStringName::empty(interface)
        .ok_or_else(|| internal("StringName construction failed"))?;
    let destroy = destructor(
        interface,
        GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
    )?
    .ok_or_else(|| internal("StringName destructor is unavailable"))?;
    // SAFETY: Release the empty value, then initialize the exact StringName return slot.
    unsafe {
        destroy(result.as_mut_ptr());
        method(value.cast_mut(), ptr::null(), result.as_mut_ptr(), 0)
    };
    result
        .to_utf8()
        .map_err(|_| internal("container class metadata is invalid UTF-8"))
}

fn check_element(
    codec: &VariantCodec,
    type_: i64,
    class: &str,
    value: GDExtensionConstVariantPtr,
) -> Result<(), ValueError> {
    if type_ == 0 {
        return Ok(());
    }
    let actual = codec
        .variant_type(value)
        .ok_or_else(|| invalid("invalid container element Variant"))?
        .0 as i64;
    if type_ == 24 {
        if actual == 0 {
            return Ok(());
        }
        if actual != 24 {
            return Err(invalid("container element must be an Object"));
        }
        let object = codec
            .read_object_id(value)
            .map_err(|_| invalid("invalid container Object"))?;
        if object == 0 {
            return Ok(());
        }
        let interface = codec.interface();
        let name = OwnedStringName::new(interface, class)
            .ok_or_else(|| internal("class name construction failed"))?;
        let lookup = interface
            .object_get_instance_from_id
            .ok_or_else(|| internal("Object lookup is unavailable"))?;
        let get_tag = interface
            .classdb_get_class_tag
            .ok_or_else(|| internal("class lookup is unavailable"))?;
        let cast = interface
            .object_cast_to
            .ok_or_else(|| internal("Object class check is unavailable"))?;
        // SAFETY: Godot resolves both the stable identity and the initialized class name.
        let valid = unsafe {
            let object = lookup(object);
            let tag = get_tag(name.as_ptr());
            !object.is_null() && !tag.is_null() && !cast(object, tag).is_null()
        };
        if valid {
            return Ok(());
        }
    } else if actual == type_ {
        return Ok(());
    }
    Err(invalid(
        "value does not match the Godot container type constraint",
    ))
}

pub(crate) fn validate_container_mutation(
    codec: &VariantCodec,
    type_: GDExtensionVariantType,
    receiver: GDExtensionConstTypePtr,
    member: &str,
    arguments: &[GDExtensionConstTypePtr],
) -> Result<(), ValueError> {
    if member != "make_read_only" {
        ensure_writable(codec.interface(), type_, receiver)?;
    }
    let interface = codec.interface();
    if type_ == array_type() {
        let type_id = container_integer(interface, type_, receiver, c"get_typed_builtin")?;
        let class = if type_id == 24 {
            container_class(interface, type_, receiver, c"get_typed_class_name")?
        } else {
            String::new()
        };
        if type_id == 0 {
            return Ok(());
        }
        let value_index = match member {
            "append" | "push_back" | "push_front" | "fill" => Some(0),
            "set" | "insert" | "indexed_set" => Some(1),
            _ => None,
        };
        if let Some(index) = value_index {
            let value = arguments
                .get(index)
                .ok_or_else(|| invalid("container mutation omitted a value"))?;
            check_element(codec, type_id, &class, value.cast())?;
        } else if matches!(member, "assign" | "append_array") {
            let source = *arguments
                .first()
                .ok_or_else(|| invalid("container assignment omitted its source"))?;
            let size = builtin_method(interface, array_type(), c"size", SIZE_HASH)?;
            let count = builtin_size(size, source, "invalid Array source size")?;
            let get = interface
                .array_operator_index_const
                .ok_or_else(|| internal("Array index operation is unavailable"))?;
            for index in 0..count {
                // SAFETY: Index is below the source's current size on the Godot thread.
                let value = unsafe { get(source, index as i64) };
                if value.is_null() {
                    return Err(invalid("invalid Array source element"));
                }
                check_element(codec, type_id, &class, value.cast_const())?;
            }
        }
    } else if type_ == dictionary_type() {
        let key_type = container_integer(interface, type_, receiver, c"get_typed_key_builtin")?;
        let value_type = container_integer(interface, type_, receiver, c"get_typed_value_builtin")?;
        let key_class = if key_type == 24 {
            container_class(interface, type_, receiver, c"get_typed_key_class_name")?
        } else {
            String::new()
        };
        let value_class = if value_type == 24 {
            container_class(interface, type_, receiver, c"get_typed_value_class_name")?
        } else {
            String::new()
        };
        if key_type == 0 && value_type == 0 {
            return Ok(());
        }
        if matches!(member, "set" | "keyed_set" | "get_or_add") {
            if let Some(key) = arguments.first() {
                check_element(codec, key_type, &key_class, key.cast())?;
            }
            if let Some(value) = arguments.get(1) {
                check_element(codec, value_type, &value_class, value.cast())?;
            }
        } else if matches!(member, "assign" | "merge") {
            let source = *arguments
                .first()
                .ok_or_else(|| invalid("Dictionary assignment omitted its source"))?;
            let borrowed = BorrowedDictionary {
                interface,
                storage: source,
            };
            let keys = borrowed.keys()?;
            for index in 0..keys.len()? {
                let key = keys.get(index)?;
                check_element(codec, key_type, &key_class, key)?;
                check_element(codec, value_type, &value_class, borrowed.get(key)?)?;
            }
        }
    }
    Ok(())
}

pub(crate) enum NativeDynamic {
    Variant(OwnedVariant),
    Array(OwnedArray),
    Dictionary(OwnedDictionary),
}

pub(crate) struct DynamicCallBacking {
    bytes: Box<[u8]>,
    context: *const EngineCallContext,
    token: u64,
}

pub(crate) struct EncodedDynamic {
    pub(crate) bytes: Vec<u8>,
    pub(crate) callable_tokens: Vec<u64>,
    pub(crate) container_tokens: crate::container::Leases,
}

impl DynamicCallBacking {
    pub(crate) fn abi(&self, value_type: AbiValueType) -> AbiValueV1 {
        AbiValueV1::from_borrowed_bytes(value_type, &self.bytes)
    }
}

impl Drop for DynamicCallBacking {
    fn drop(&mut self) {
        if self.token != 0 && !self.context.is_null() {
            // SAFETY: The backing never outlives the retained module
            // generation that owns this EngineCallContext.
            let _ = unsafe { &*self.context }.release_dynamic(self.token);
        }
    }
}

impl NativeDynamic {
    pub(crate) fn from_shared_variant(
        codec: &VariantCodec,
        kind: AbiValueType,
        value: &OwnedVariant,
    ) -> Result<Self, ValueError> {
        let interface = codec.interface();
        let get_to = interface
            .get_variant_to_type_constructor
            .ok_or_else(|| internal("builtin conversion is unavailable"))?;
        let type_ = builtin_variant_type(match kind {
            AbiValueType::ARRAY => "Array",
            AbiValueType::DICTIONARY => "Dictionary",
            _ => return Err(invalid("invalid container kind")),
        })
        .ok_or_else(|| internal("container Variant type is unavailable"))?;
        if codec.variant_type(value.as_ptr()) != Some(type_) {
            return Err(invalid("container has the wrong native type"));
        }
        // SAFETY: This is an authenticated builtin Variant type.
        let convert = unsafe { get_to(type_) }
            .ok_or_else(|| internal("container conversion is unavailable"))?;
        let mut result = Self::empty(interface, kind)?;
        match &mut result {
            Self::Array(array) => {
                // SAFETY: Replace the initialized empty wrapper with a shared copy of a live Array.
                unsafe {
                    array.destroy.expect("Array destructor")(array.as_mut_ptr());
                    convert(array.as_mut_ptr(), value.as_ptr().cast_mut());
                }
            }
            Self::Dictionary(dictionary) => {
                // SAFETY: Replace the initialized empty wrapper with a shared copy of a live Dictionary.
                unsafe {
                    dictionary.destroy.expect("Dictionary destructor")(dictionary.as_mut_ptr());
                    convert(dictionary.as_mut_ptr(), value.as_ptr().cast_mut());
                }
            }
            _ => unreachable!(),
        }
        Ok(result)
    }

    pub(crate) fn assign_shared(
        codec: &VariantCodec,
        kind: AbiValueType,
        target: &OwnedVariant,
        source: &Self,
    ) -> Result<(), ValueError> {
        let mut target = Self::from_shared_variant(codec, kind, target)?;
        let type_ = match kind {
            AbiValueType::ARRAY => array_type(),
            AbiValueType::DICTIONARY => dictionary_type(),
            _ => return Err(invalid("invalid container kind")),
        };
        ensure_writable(codec.interface(), type_, target.as_const_ptr())?;
        let hash = if kind == AbiValueType::ARRAY {
            2_307_260_970
        } else {
            3_642_266_950
        };
        let assign = builtin_method(codec.interface(), type_, c"assign", hash)?
            .ok_or_else(|| internal("container assign is unavailable"))?;
        let arguments = [source.as_const_ptr()];
        validate_container_mutation(codec, type_, target.as_const_ptr(), "assign", &arguments)?;
        // SAFETY: Receiver and source are live containers of the same authenticated builtin type.
        unsafe { assign(target.as_mut_ptr(), arguments.as_ptr(), ptr::null_mut(), 1) };
        Ok(())
    }
    /// Copies signal arguments before Godot releases its borrowed call frame.
    pub(crate) fn from_variant_arguments(
        interface: EngineInterface,
        arguments: &[GDExtensionConstVariantPtr],
    ) -> Result<Self, ValueError> {
        let mut array = OwnedArray::empty(interface)?;
        array.resize(arguments.len())?;
        let get = interface
            .array_operator_index
            .ok_or_else(|| internal("Godot Array index operation is unavailable"))?;
        for (index, argument) in arguments.iter().enumerate() {
            if argument.is_null() {
                return Err(invalid("Godot signal supplied a null argument"));
            }
            // SAFETY: The array was resized to the exact argument count; each
            // borrowed Godot Variant remains live for this synchronous copy.
            let slot = unsafe { get(array.as_mut_ptr(), index as i64) };
            replace_variant(interface, slot, *argument)?;
        }
        Ok(Self::Array(array))
    }

    pub(crate) fn from_abi(
        interface: EngineInterface,
        expected: AbiValueType,
        value: AbiValueV1,
        typed_array_element: Option<&str>,
        context: Option<&EngineCallContext>,
    ) -> Result<Self, ValueError> {
        if value.type_ != expected || value.reserved_flags != 0 {
            return Err(invalid(
                "dynamic Godot argument does not match its generated contract",
            ));
        }
        let (pointer, length) = value
            .byte_range(expected)
            .ok_or_else(|| invalid("dynamic Godot argument has an invalid byte range"))?;
        if length > MAX_WIRE_BYTES {
            return Err(invalid(
                "dynamic Godot argument exceeds the Host byte limit",
            ));
        }
        // SAFETY: The project module retains this bounded range through the
        // synchronous Host call.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        if !validate_dynamic_value(expected, bytes) {
            return Err(invalid(
                "dynamic Godot argument has an invalid recursive ABI payload",
            ));
        }
        let codec = VariantCodec::new(interface)
            .ok_or_else(|| internal("Godot Variant codecs are unavailable"))?;
        Self::from_abi_with_codec(&codec, expected, value, typed_array_element, context)
    }

    fn from_abi_with_codec(
        codec: &VariantCodec,
        expected: AbiValueType,
        value: AbiValueV1,
        typed_array_element: Option<&str>,
        context: Option<&EngineCallContext>,
    ) -> Result<Self, ValueError> {
        if value.type_ != expected
            || value.reserved_flags & !godot_api::abi::ABI_VALUE_OWNED_BYTES != 0
        {
            return Err(invalid(
                "dynamic Godot argument does not match its generated contract",
            ));
        }
        let (pointer, length) = value
            .byte_range(expected)
            .ok_or_else(|| invalid("dynamic Godot argument has an invalid byte range"))?;
        if length > MAX_WIRE_BYTES {
            return Err(invalid(
                "dynamic Godot argument exceeds the Host byte limit",
            ));
        }
        // SAFETY: The caller retains the borrowed or owned ABI range for this
        // synchronous conversion.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        if !validate_dynamic_value(expected, bytes) {
            return Err(invalid(
                "dynamic Godot argument has an invalid recursive ABI payload",
            ));
        }
        let mut reader = Reader::new(bytes, context)?;
        match expected {
            AbiValueType::VARIANT => {
                let value = reader.variant(codec, 0)?;
                reader.finish()?;
                Ok(Self::Variant(value))
            }
            AbiValueType::ARRAY => {
                let value = reader.array(codec, 0, typed_array_element)?;
                reader.finish()?;
                Ok(Self::Array(value))
            }
            AbiValueType::DICTIONARY => {
                let value = reader.dictionary(codec, 0)?;
                reader.finish()?;
                Ok(Self::Dictionary(value))
            }
            _ => Err(invalid(
                "Host requested an unsupported dynamic Godot argument",
            )),
        }
    }

    pub(crate) fn empty(
        interface: EngineInterface,
        expected: AbiValueType,
    ) -> Result<Self, ValueError> {
        let codec = VariantCodec::new(interface)
            .ok_or_else(|| internal("Godot Variant codecs are unavailable"))?;
        match expected {
            AbiValueType::VARIANT => {
                let value = OwnedVariant::from_abi(&codec, AbiValueV1::NIL)
                    .map_err(|_| internal("Godot Variant output could not be initialized"))?;
                Ok(Self::Variant(value))
            }
            AbiValueType::ARRAY => OwnedArray::empty(interface).map(Self::Array),
            AbiValueType::DICTIONARY => OwnedDictionary::empty(interface).map(Self::Dictionary),
            _ => Err(internal(
                "Host requested unsupported dynamic Godot return storage",
            )),
        }
    }

    pub(crate) fn as_const_ptr(&self) -> GDExtensionConstTypePtr {
        match self {
            Self::Variant(value) => value.as_ptr().cast(),
            Self::Array(value) => value.as_ptr(),
            Self::Dictionary(value) => value.as_ptr(),
        }
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        match self {
            Self::Variant(value) => value.as_mut_ptr().cast(),
            Self::Array(value) => value.as_mut_ptr(),
            Self::Dictionary(value) => value.as_mut_ptr(),
        }
    }

    pub(crate) fn to_bytes(
        &self,
        context: Option<&EngineCallContext>,
    ) -> Result<EncodedDynamic, ValueError> {
        self.to_bytes_with_snapshot(context, false)
    }

    pub(crate) fn to_bytes_snapshot(
        &self,
        context: Option<&EngineCallContext>,
    ) -> Result<EncodedDynamic, ValueError> {
        self.to_bytes_with_snapshot(context, true)
    }

    fn to_bytes_with_snapshot(
        &self,
        context: Option<&EngineCallContext>,
        snapshot: bool,
    ) -> Result<EncodedDynamic, ValueError> {
        let interface = match self {
            Self::Variant(value) => value.interface(),
            Self::Array(value) => value.interface,
            Self::Dictionary(value) => value.interface,
        };
        let codec = VariantCodec::new(interface)
            .ok_or_else(|| internal("Godot Variant codecs are unavailable"))?;
        let mut writer = Writer::new(context);
        writer.snapshot_top = snapshot;
        match self {
            Self::Variant(value) => writer.variant(&codec, value.as_ptr(), 0)?,
            Self::Array(value) => writer.array(&codec, value, 0)?,
            Self::Dictionary(value) => writer.dictionary(&codec, value, 0)?,
        }
        writer.finish()
    }
}

pub(crate) fn decode_dynamic_variant(
    codec: &VariantCodec,
    value: GDExtensionConstVariantPtr,
    expected: AbiValueType,
    context: Option<&EngineCallContext>,
) -> Result<DynamicCallBacking, ValueError> {
    let actual = codec
        .variant_type(value)
        .ok_or_else(|| invalid("Godot supplied an invalid dynamic Variant"))?;
    let retained_value = context.map(|_| copy_variant(codec, value)).transpose()?;
    let mut writer = Writer::new(context);
    match expected {
        AbiValueType::VARIANT => writer.variant(codec, value, 0)?,
        AbiValueType::ARRAY if actual == array_type() => {
            let value = borrowed_builtin(codec.interface(), actual, value)?;
            writer.borrowed_array(codec, value, 0)?;
        }
        AbiValueType::DICTIONARY if actual == dictionary_type() => {
            let value = borrowed_builtin(codec.interface(), actual, value)?;
            writer.borrowed_dictionary(codec, value, 0)?;
        }
        AbiValueType::ARRAY | AbiValueType::DICTIONARY => {
            return Err(invalid(
                "Godot dynamic container does not match its reflected type",
            ));
        }
        _ => return Err(internal("Host requested an unsupported dynamic value type")),
    }
    let encoded = writer.finish()?;
    let mut bytes = encoded.bytes;
    let (context, token) = if let Some(context) = context {
        let retained_value = retained_value
            .ok_or_else(|| internal("dynamic value retention state is unavailable"))?;
        let token = context.retain_dynamic(
            NativeDynamic::Variant(retained_value),
            encoded.callable_tokens,
            encoded.container_tokens,
        )?;
        if let Err(error) = set_dynamic_ownership(&mut bytes, token) {
            let _ = context.release_dynamic(token);
            return Err(error);
        }
        (core::ptr::from_ref(context), token)
    } else {
        if !encoded.callable_tokens.is_empty() {
            return Err(internal(
                "dynamic Callable tokens require an engine-call context",
            ));
        }
        (core::ptr::null(), 0)
    };
    Ok(DynamicCallBacking {
        bytes: bytes.into_boxed_slice(),
        context,
        token,
    })
}

pub(crate) fn construct_dynamic_variant(
    codec: &VariantCodec,
    value: AbiValueV1,
    output: GDExtensionVariantPtr,
    typed_array_element: Option<&str>,
    context: Option<&EngineCallContext>,
) -> Result<(), ValueError> {
    if output.is_null() {
        return Err(internal(
            "Godot supplied null dynamic Variant output storage",
        ));
    }
    let dynamic = NativeDynamic::from_abi_with_codec(
        codec,
        value.type_,
        value,
        typed_array_element,
        context,
    )?;
    let variant = dynamic.into_variant(codec)?;
    let copy = codec
        .interface()
        .variant_new_copy
        .ok_or_else(|| internal("Godot Variant copy constructor is unavailable"))?;
    // SAFETY: ScriptInstance supplies uninitialized Variant output storage and
    // `variant` owns one live initialized source until the copy completes.
    unsafe { copy(output, variant.as_ptr()) };
    Ok(())
}

pub(crate) fn replace_dynamic_variant(
    codec: &VariantCodec,
    value: AbiValueV1,
    output: GDExtensionVariantPtr,
    typed_array_element: Option<&str>,
    context: Option<&EngineCallContext>,
) -> Result<(), ValueError> {
    let dynamic = NativeDynamic::from_abi_with_codec(
        codec,
        value.type_,
        value,
        typed_array_element,
        context,
    )?;
    let variant = dynamic.into_variant(codec)?;
    replace_variant(codec.interface(), output, variant.as_ptr())
}

pub(crate) fn set_dynamic_ownership(bytes: &mut [u8], token: u64) -> Result<(), ValueError> {
    if token == 0
        || bytes.len() < WIRE_HEADER_BYTES
        || read_u16(bytes, 10) != Some(0)
        || read_u64(bytes, 12) != Some(0)
    {
        return Err(internal(
            "Host could not attach dynamic-value ownership metadata",
        ));
    }
    bytes[10..12].copy_from_slice(&godot_api::abi::ABI_DYNAMIC_ROOT_OWNED.to_le_bytes());
    bytes[12..20].copy_from_slice(&token.to_le_bytes());
    Ok(())
}

impl NativeDynamic {
    pub(crate) fn into_variant(self, codec: &VariantCodec) -> Result<OwnedVariant, ValueError> {
        match self {
            Self::Variant(value) => Ok(value),
            Self::Array(value) => value.to_variant(codec),
            Self::Dictionary(value) => value.to_variant(codec),
        }
    }
}

pub(crate) struct OwnedArray {
    interface: EngineInterface,
    storage: usize,
    destroy: GDExtensionPtrDestructor,
    size: GDExtensionPtrBuiltInMethod,
    resize: GDExtensionPtrBuiltInMethod,
}

impl OwnedArray {
    fn validate_type(&self, element: &str) -> Result<(), ValueError> {
        let actual = container_integer(
            self.interface,
            array_type(),
            self.as_ptr(),
            c"get_typed_builtin",
        )?;
        let expected = builtin_variant_type(element).map_or(24, |kind| kind.0 as i64);
        if actual != expected {
            return Err(invalid("Array has incompatible engine element constraints"));
        }
        if expected == 24
            && container_class(
                self.interface,
                array_type(),
                self.as_ptr(),
                c"get_typed_class_name",
            )? != element
        {
            return Err(invalid("Array has incompatible engine class constraints"));
        }
        Ok(())
    }
    fn empty(interface: EngineInterface) -> Result<Self, ValueError> {
        let mut value = Self {
            interface,
            storage: 0,
            destroy: destructor(interface, array_type())?,
            size: builtin_method(interface, array_type(), c"size", SIZE_HASH)?,
            resize: builtin_method(interface, array_type(), c"resize", RESIZE_HASH)?,
        };
        construct_default(interface, array_type(), value.as_mut_ptr())?;
        Ok(value)
    }

    fn resize(&mut self, count: usize) -> Result<(), ValueError> {
        if count > MAX_CONTAINER_ELEMENTS {
            return Err(invalid("Godot Array exceeds the Host element limit"));
        }
        let count =
            i64::try_from(count).map_err(|_| invalid("Godot Array size is out of range"))?;
        let arguments = [ptr::from_ref(&count).cast()];
        let mut error = -1_i64;
        let resize = self
            .resize
            .ok_or_else(|| internal("Godot Array resize method is unavailable"))?;
        // SAFETY: Receiver is initialized; resize accepts one int64 and writes
        // one Error enum represented as int64.
        unsafe {
            resize(
                self.as_mut_ptr(),
                arguments.as_ptr(),
                ptr::from_mut(&mut error).cast(),
                1,
            );
        }
        if error != 0 {
            return Err(invalid("Godot rejected a dynamic Array size"));
        }
        Ok(())
    }

    fn set_typed(&mut self, element: &str) -> Result<(), ValueError> {
        let set_typed = self
            .interface
            .array_set_typed
            .ok_or_else(|| internal("Godot typed-Array operation is unavailable"))?;
        let (type_, class_name) = match builtin_variant_type(element) {
            Some(type_) => (
                type_,
                OwnedStringName::empty(self.interface)
                    .ok_or_else(|| internal("Godot StringName could not be initialized"))?,
            ),
            None => {
                let class_name = OwnedStringName::new(self.interface, element)
                    .ok_or_else(|| invalid("typed Godot Array has an invalid element class"))?;
                let get_tag = self
                    .interface
                    .classdb_get_class_tag
                    .ok_or_else(|| internal("Godot ClassDB lookup is unavailable"))?;
                // SAFETY: class_name owns one initialized StringName.
                if unsafe { get_tag(class_name.as_ptr()) }.is_null() {
                    return Err(invalid(
                        "typed Godot Array references an unavailable element class",
                    ));
                }
                (
                    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
                    class_name,
                )
            }
        };
        let codec = VariantCodec::new(self.interface)
            .ok_or_else(|| internal("Godot Variant codecs are unavailable"))?;
        let script = OwnedVariant::from_abi(&codec, AbiValueV1::NIL)
            .map_err(|_| internal("Godot nil Variant could not be initialized"))?;
        // SAFETY: The Array is initialized, type metadata came from an
        // authenticated generated contract, and Godot's implementation
        // dereferences both metadata pointers even when they are empty.
        unsafe {
            set_typed(
                self.as_mut_ptr(),
                type_,
                class_name.as_ptr(),
                script.as_ptr(),
            );
        }
        Ok(())
    }

    fn set(&mut self, index: usize, value: &OwnedVariant) -> Result<(), ValueError> {
        let get = self
            .interface
            .array_operator_index
            .ok_or_else(|| internal("Godot Array index operation is unavailable"))?;
        let index =
            i64::try_from(index).map_err(|_| invalid("Godot Array index is out of range"))?;
        // SAFETY: The caller resized this same array beyond `index`.
        let slot = unsafe { get(self.as_mut_ptr(), index) };
        replace_variant(self.interface, slot, value.as_ptr())
    }

    pub(crate) fn len(&self) -> Result<usize, ValueError> {
        builtin_size(
            self.size,
            self.as_ptr(),
            "Godot Array returned an invalid size",
        )
    }

    pub(crate) fn get(&self, index: usize) -> Result<GDExtensionConstVariantPtr, ValueError> {
        let get = self
            .interface
            .array_operator_index_const
            .ok_or_else(|| internal("Godot Array const index operation is unavailable"))?;
        let index =
            i64::try_from(index).map_err(|_| internal("Godot Array index is out of range"))?;
        // SAFETY: The caller obtains indices from this array's exact size.
        let value = unsafe { get(self.as_ptr(), index) };
        (!value.is_null())
            .then_some(value.cast_const())
            .ok_or_else(|| internal("Godot returned a null Array element"))
    }

    fn to_variant(&self, codec: &VariantCodec) -> Result<OwnedVariant, ValueError> {
        variant_from_builtin(codec, array_type(), self.as_ptr())
    }

    fn as_ptr(&self) -> GDExtensionConstTypePtr {
        ptr::from_ref(&self.storage).cast()
    }

    fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        ptr::from_mut(&mut self.storage).cast()
    }
}

impl Drop for OwnedArray {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            // SAFETY: This wrapper owns one initialized Array.
            unsafe { destroy(self.as_mut_ptr()) };
        }
    }
}

pub(crate) struct OwnedDictionary {
    interface: EngineInterface,
    storage: usize,
    destroy: GDExtensionPtrDestructor,
}

impl OwnedDictionary {
    pub(crate) fn empty(interface: EngineInterface) -> Result<Self, ValueError> {
        let mut value = Self {
            interface,
            storage: 0,
            destroy: destructor(interface, dictionary_type())?,
        };
        construct_default(interface, dictionary_type(), value.as_mut_ptr())?;
        Ok(value)
    }

    pub(crate) fn insert(
        &mut self,
        key: &OwnedVariant,
        value: &OwnedVariant,
    ) -> Result<(), ValueError> {
        let index = self
            .interface
            .dictionary_operator_index
            .ok_or_else(|| internal("Godot Dictionary index operation is unavailable"))?;
        // SAFETY: Dictionary and key are initialized official values.
        let slot = unsafe { index(self.as_mut_ptr(), key.as_ptr()) };
        replace_variant(self.interface, slot, value.as_ptr())
    }

    fn to_variant(&self, codec: &VariantCodec) -> Result<OwnedVariant, ValueError> {
        variant_from_builtin(codec, dictionary_type(), self.as_ptr())
    }

    fn as_ptr(&self) -> GDExtensionConstTypePtr {
        ptr::from_ref(&self.storage).cast()
    }

    fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        ptr::from_mut(&mut self.storage).cast()
    }

    pub(crate) fn write_copy(&self, output: GDExtensionTypePtr) -> Result<(), ValueError> {
        if output.is_null() {
            return Err(internal("Godot supplied null Dictionary output storage"));
        }
        let get_to = self
            .interface
            .get_variant_to_type_constructor
            .ok_or_else(|| internal("Godot builtin conversion is unavailable"))?;
        // SAFETY: Dictionary is an official Variant type.
        let to_dictionary = unsafe { get_to(dictionary_type()) }
            .ok_or_else(|| internal("Godot Dictionary conversion is unavailable"))?;
        let codec = VariantCodec::new(self.interface)
            .ok_or_else(|| internal("Godot Variant codec is unavailable"))?;
        let variant = self.to_variant(&codec)?;
        // SAFETY: Output is uninitialized Dictionary storage and `variant`
        // contains a live Dictionary until the conversion returns.
        unsafe { to_dictionary(output, variant.as_ptr().cast_mut()) };
        Ok(())
    }
}

impl Drop for OwnedDictionary {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            // SAFETY: This wrapper owns one initialized Dictionary.
            unsafe { destroy(self.as_mut_ptr()) };
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    context: Option<&'a EngineCallContext>,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], context: Option<&'a EngineCallContext>) -> Result<Self, ValueError> {
        let root_flags = read_u16(bytes, 10);
        let token = read_u64(bytes, 12);
        if bytes.len() < WIRE_HEADER_BYTES
            || bytes.len() > MAX_WIRE_BYTES
            || bytes[..8] != WIRE_MAGIC
            || read_u16(bytes, 8) != Some(WIRE_VERSION)
            || !matches!(
                (root_flags, token),
                (Some(0), Some(0))
                    | (
                        Some(godot_api::abi::ABI_DYNAMIC_ROOT_OWNED),
                        Some(1..=u64::MAX)
                    )
            )
        {
            return Err(invalid("dynamic Godot argument has an invalid ABI header"));
        }
        Ok(Self {
            bytes,
            offset: WIRE_HEADER_BYTES,
            context,
        })
    }

    fn finish(&self) -> Result<(), ValueError> {
        (self.offset == self.bytes.len())
            .then_some(())
            .ok_or_else(|| invalid("dynamic Godot argument has trailing ABI bytes"))
    }

    fn variant(&mut self, codec: &VariantCodec, depth: usize) -> Result<OwnedVariant, ValueError> {
        let node = self.node(depth)?;
        if node.flags == godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER {
            let token =
                read_u64(node.payload, 0).ok_or_else(|| invalid("invalid container token"))?;
            return crate::container::get(AbiValueType(node.type_), token)?
                .copy_value()
                .map_err(|_| internal("container Variant copy failed"));
        }
        match node.type_ {
            37 => self
                .array_payload(codec, node, depth, None)?
                .to_variant(codec),
            38 => self
                .dictionary_payload(codec, node, depth)?
                .to_variant(codec),
            39 => {
                let value = AbiValueV1::from_borrowed_bytes(AbiValueType::CALLABLE, node.payload);
                NativeCallable::from_abi(codec.interface(), value, self.context, |object_id| {
                    codec
                        .object_from_id(object_id)
                        .map_err(|_| invalid("dynamic Callable target no longer exists"))
                })?
                .to_variant(codec)
            }
            40 => {
                let value = AbiValueV1::from_borrowed_bytes(AbiValueType::SIGNAL, node.payload);
                NativeSignal::from_abi(codec.interface(), value, |object_id| {
                    codec
                        .object_from_id(object_id)
                        .map_err(|_| invalid("dynamic Signal target no longer exists"))
                })?
                .to_variant()
            }
            21..=25 => variant_from_math_node(codec, node),
            _ => {
                let value = abi_from_node(node)?;
                OwnedVariant::from_abi(codec, value)
                    .map_err(|_| invalid("dynamic Godot argument has an invalid value payload"))
            }
        }
    }

    fn array(
        &mut self,
        codec: &VariantCodec,
        depth: usize,
        typed_array_element: Option<&str>,
    ) -> Result<OwnedArray, ValueError> {
        let node = self.node(depth)?;
        if node.type_ != 37 {
            return Err(invalid("dynamic Godot argument is not an Array"));
        }
        self.array_payload(codec, node, depth, typed_array_element)
    }

    fn dictionary(
        &mut self,
        codec: &VariantCodec,
        depth: usize,
    ) -> Result<OwnedDictionary, ValueError> {
        let node = self.node(depth)?;
        if node.type_ != 38 {
            return Err(invalid("dynamic Godot argument is not a Dictionary"));
        }
        self.dictionary_payload(codec, node, depth)
    }

    fn array_payload(
        &mut self,
        codec: &VariantCodec,
        node: Node<'a>,
        depth: usize,
        typed_array_element: Option<&str>,
    ) -> Result<OwnedArray, ValueError> {
        check_depth(depth)?;
        if node.flags == godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER {
            let token = read_u64(node.payload, 0).ok_or_else(|| invalid("invalid Array token"))?;
            let value = crate::container::get(AbiValueType::ARRAY, token)?;
            let NativeDynamic::Array(array) =
                NativeDynamic::from_shared_variant(codec, AbiValueType::ARRAY, &value)?
            else {
                unreachable!()
            };
            if let Some(element) = typed_array_element {
                array.validate_type(element)?;
            }
            return Ok(array);
        }
        let count = container_count(node.payload)?;
        let payload_end = node.payload_end;
        self.offset = node.payload_start + 8;
        let mut result = OwnedArray::empty(codec.interface())?;
        if let Some(element) = typed_array_element {
            result.set_typed(element)?;
        }
        result.resize(count)?;
        let constraint = if typed_array_element.is_some() {
            let type_id = container_integer(
                codec.interface(),
                array_type(),
                result.as_ptr(),
                c"get_typed_builtin",
            )?;
            let class = if type_id == 24 {
                container_class(
                    codec.interface(),
                    array_type(),
                    result.as_ptr(),
                    c"get_typed_class_name",
                )?
            } else {
                String::new()
            };
            Some((type_id, class))
        } else {
            None
        };
        for index in 0..count {
            let value = self.variant(codec, depth + 1)?;
            if let Some((type_id, class)) = &constraint {
                check_element(codec, *type_id, class, value.as_ptr())?;
            }
            result.set(index, &value)?;
        }
        if self.offset != payload_end {
            return Err(invalid("dynamic Godot Array payload is not canonical"));
        }
        Ok(result)
    }

    fn dictionary_payload(
        &mut self,
        codec: &VariantCodec,
        node: Node<'a>,
        depth: usize,
    ) -> Result<OwnedDictionary, ValueError> {
        check_depth(depth)?;
        if node.flags == godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER {
            let token =
                read_u64(node.payload, 0).ok_or_else(|| invalid("invalid Dictionary token"))?;
            let value = crate::container::get(AbiValueType::DICTIONARY, token)?;
            let NativeDynamic::Dictionary(dictionary) =
                NativeDynamic::from_shared_variant(codec, AbiValueType::DICTIONARY, &value)?
            else {
                unreachable!()
            };
            return Ok(dictionary);
        }
        let count = container_count(node.payload)?;
        let payload_end = node.payload_end;
        self.offset = node.payload_start + 8;
        let mut result = OwnedDictionary::empty(codec.interface())?;
        for _ in 0..count {
            let key = self.variant(codec, depth + 1)?;
            let value = self.variant(codec, depth + 1)?;
            result.insert(&key, &value)?;
        }
        if self.offset != payload_end {
            return Err(invalid("dynamic Godot Dictionary payload is not canonical"));
        }
        Ok(result)
    }

    fn node(&mut self, depth: usize) -> Result<Node<'a>, ValueError> {
        check_depth(depth)?;
        let header_end = self
            .offset
            .checked_add(NODE_HEADER_BYTES)
            .ok_or_else(|| invalid("dynamic Godot argument offset overflowed"))?;
        let header = self
            .bytes
            .get(self.offset..header_end)
            .ok_or_else(|| invalid("dynamic Godot argument is truncated"))?;
        let type_ = u32::from_le_bytes(header[..4].try_into().expect("u32 width"));
        let flags = u32::from_le_bytes(header[4..8].try_into().expect("u32 width"));
        let length = usize::try_from(u64::from_le_bytes(
            header[8..16].try_into().expect("u64 width"),
        ))
        .map_err(|_| invalid("dynamic Godot argument length is out of range"))?;
        if flags != 0 && flags != godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER {
            return Err(invalid("dynamic Godot argument uses unsupported ABI flags"));
        }
        let payload_start = header_end;
        let payload_end = payload_start
            .checked_add(length)
            .ok_or_else(|| invalid("dynamic Godot argument length overflowed"))?;
        let payload = self
            .bytes
            .get(payload_start..payload_end)
            .ok_or_else(|| invalid("dynamic Godot argument payload is truncated"))?;
        if flags == godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER
            && (!matches!(type_, 37 | 38)
                || payload.len() != 8
                || read_u64(payload, 0).is_none_or(|token| token == 0))
        {
            return Err(invalid("invalid shared-container node"));
        }
        self.offset = payload_end;
        Ok(Node {
            type_,
            flags,
            payload,
            payload_start,
            payload_end,
        })
    }
}

struct Node<'a> {
    type_: u32,
    flags: u32,
    payload: &'a [u8],
    payload_start: usize,
    payload_end: usize,
}

struct Writer {
    bytes: Vec<u8>,
    context: *const EngineCallContext,
    callable_tokens: Vec<u64>,
    container_tokens: Vec<u64>,
    snapshot_top: bool,
}

impl Writer {
    fn new(context: Option<&EngineCallContext>) -> Self {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WIRE_MAGIC);
        bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        Self {
            bytes,
            context: context.map_or(ptr::null(), ptr::from_ref),
            callable_tokens: Vec::new(),
            container_tokens: Vec::new(),
            snapshot_top: false,
        }
    }

    fn finish(mut self) -> Result<EncodedDynamic, ValueError> {
        if self.bytes.len() > MAX_WIRE_BYTES {
            return Err(unsupported(
                "dynamic Godot result exceeds the Host byte limit",
            ));
        }
        Ok(EncodedDynamic {
            bytes: core::mem::take(&mut self.bytes),
            callable_tokens: core::mem::take(&mut self.callable_tokens),
            container_tokens: crate::container::Leases(core::mem::take(&mut self.container_tokens)),
        })
    }

    fn variant(
        &mut self,
        codec: &VariantCodec,
        value: GDExtensionConstVariantPtr,
        depth: usize,
    ) -> Result<(), ValueError> {
        check_depth(depth)?;
        let type_ = codec
            .variant_type(value)
            .ok_or_else(|| internal("Godot returned a Variant with an invalid type"))?;
        if type_ == array_type() {
            let array = borrowed_builtin(codec.interface(), type_, value)?;
            return self.borrowed_array(codec, array, depth);
        }
        if type_ == dictionary_type() {
            let dictionary = borrowed_builtin(codec.interface(), type_, value)?;
            return self.borrowed_dictionary(codec, dictionary, depth);
        }
        if type_ == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE {
            let callable = NativeCallable::from_variant(codec, value)?;
            let token = if self.context.is_null() {
                0
            } else {
                // SAFETY: Writer is scoped to the module generation that owns
                // this exact EngineCallContext.
                unsafe { &*self.context }.retain_callable(callable.copy_value()?)?
            };
            let bytes = match callable.to_bytes(token) {
                Ok(bytes) => bytes,
                Err(error) => {
                    if token != 0 {
                        // SAFETY: See the retained-token branch above.
                        let _ = unsafe { &*self.context }.release_callable(token);
                    }
                    return Err(error);
                }
            };
            if token != 0 {
                self.callable_tokens.push(token);
            }
            let header = self.begin(AbiValueType::CALLABLE.0);
            self.bytes.extend_from_slice(&bytes);
            return self.end(header);
        }
        if type_ == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL {
            let signal = NativeSignal::from_variant(codec, value)?;
            let bytes = signal.to_bytes()?;
            let header = self.begin(AbiValueType::SIGNAL.0);
            self.bytes.extend_from_slice(&bytes);
            return self.end(header);
        }
        let abi_type = abi_type(type_)
            .ok_or_else(|| unsupported("Godot returned an unsupported dynamic value type"))?;
        let mut strings = Vec::new();
        let mut math = Vec::new();
        let mut packed = Vec::new();
        let mut dynamic = Vec::new();
        let mut callable = Vec::new();
        let value = codec
            .decode(
                value,
                abi_type,
                VariantDecodeBacking {
                    strings: &mut strings,
                    math: &mut math,
                    packed: &mut packed,
                    dynamic: &mut dynamic,
                    callable: &mut callable,
                    dynamic_context: None,
                },
            )
            .map_err(|_| internal("Godot dynamic result could not be decoded"))?;
        self.abi(value)
    }

    fn array(
        &mut self,
        codec: &VariantCodec,
        value: &OwnedArray,
        depth: usize,
    ) -> Result<(), ValueError> {
        self.borrowed_array(codec, value.as_ptr(), depth)
    }

    fn dictionary(
        &mut self,
        codec: &VariantCodec,
        value: &OwnedDictionary,
        depth: usize,
    ) -> Result<(), ValueError> {
        self.borrowed_dictionary(codec, value.as_ptr(), depth)
    }

    fn borrowed_array(
        &mut self,
        codec: &VariantCodec,
        value: GDExtensionConstTypePtr,
        depth: usize,
    ) -> Result<(), ValueError> {
        if !(self.context.is_null() || self.snapshot_top && depth == 0) {
            return self.container(codec, AbiValueType::ARRAY, value);
        }
        let size = builtin_method(codec.interface(), array_type(), c"size", SIZE_HASH)?;
        let count = builtin_size(size, value, "Godot Array returned an invalid size")?;
        self.array_elements(codec, value, count, depth)
    }

    fn array_elements(
        &mut self,
        codec: &VariantCodec,
        value: GDExtensionConstTypePtr,
        count: usize,
        depth: usize,
    ) -> Result<(), ValueError> {
        check_depth(depth)?;
        if count > MAX_CONTAINER_ELEMENTS {
            return Err(unsupported("Godot Array exceeds the Host element limit"));
        }
        let header = self.begin(37);
        self.bytes.extend_from_slice(&(count as u64).to_le_bytes());
        let get = codec
            .interface()
            .array_operator_index_const
            .ok_or_else(|| internal("Godot Array const index operation is unavailable"))?;
        for index in 0..count {
            // SAFETY: Index is below the size read from the same live Array.
            let item = unsafe { get(value, index as i64) };
            if item.is_null() {
                return Err(internal("Godot returned a null Array element"));
            }
            self.variant(codec, item.cast_const(), depth + 1)?;
        }
        self.end(header)
    }

    fn borrowed_dictionary(
        &mut self,
        codec: &VariantCodec,
        value: GDExtensionConstTypePtr,
        depth: usize,
    ) -> Result<(), ValueError> {
        if !(self.context.is_null() || self.snapshot_top && depth == 0) {
            return self.container(codec, AbiValueType::DICTIONARY, value);
        }
        let borrowed = BorrowedDictionary {
            interface: codec.interface(),
            storage: value,
        };
        self.dictionary_borrowed_entries(codec, &borrowed, depth)
    }

    fn container(
        &mut self,
        codec: &VariantCodec,
        kind: AbiValueType,
        value: GDExtensionConstTypePtr,
    ) -> Result<(), ValueError> {
        let type_ = if kind == AbiValueType::ARRAY {
            array_type()
        } else {
            dictionary_type()
        };
        let native = variant_from_builtin(codec, type_, value)?;
        let token = crate::container::insert(kind, native)?;
        self.container_tokens.push(token);
        let header = self.begin(kind.0);
        self.bytes[header + 4..header + 8]
            .copy_from_slice(&godot_api::abi::ABI_DYNAMIC_NODE_CONTAINER.to_le_bytes());
        self.bytes.extend_from_slice(&token.to_le_bytes());
        self.end(header)
    }

    fn dictionary_borrowed_entries(
        &mut self,
        codec: &VariantCodec,
        value: &BorrowedDictionary,
        depth: usize,
    ) -> Result<(), ValueError> {
        check_depth(depth)?;
        let count = value.len()?;
        if count > MAX_CONTAINER_ELEMENTS {
            return Err(unsupported(
                "Godot Dictionary exceeds the Host element limit",
            ));
        }
        let keys = value.keys()?;
        if keys.len()? != count {
            return Err(internal(
                "Godot Dictionary keys size changed during encoding",
            ));
        }
        let header = self.begin(38);
        self.bytes.extend_from_slice(&(count as u64).to_le_bytes());
        for index in 0..count {
            let key = keys.get(index)?;
            let item = value.get(key)?;
            self.variant(codec, key, depth + 1)?;
            self.variant(codec, item, depth + 1)?;
        }
        self.end(header)
    }

    fn abi(&mut self, value: AbiValueV1) -> Result<(), ValueError> {
        let header = self.begin(value.type_.0);
        match value.type_ {
            AbiValueType::NIL => {}
            AbiValueType::BOOL => self.bytes.push(value.payload[0] as u8),
            AbiValueType::I64 => self
                .bytes
                .extend_from_slice(&(value.payload[0] as i64).to_le_bytes()),
            AbiValueType::F64 => self
                .bytes
                .extend_from_slice(&value.payload[0].to_le_bytes()),
            AbiValueType::OBJECT_ID | AbiValueType::RID => {
                self.bytes
                    .extend_from_slice(&value.payload[0].to_le_bytes());
            }
            AbiValueType::STRING | AbiValueType::STRING_NAME | AbiValueType::NODE_PATH => {
                let text = crate::module_value::utf8(&value)
                    .map_err(|_| internal("Godot returned invalid dynamic UTF-8"))?;
                self.bytes.extend_from_slice(text.as_bytes());
            }
            AbiValueType::VECTOR2 => write_f32s(
                &mut self.bytes,
                &value
                    .vector2()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Vector2"))?,
            ),
            AbiValueType::VECTOR3 => write_f32s(
                &mut self.bytes,
                &value
                    .vector3()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Vector3"))?,
            ),
            AbiValueType::COLOR => write_f32s(
                &mut self.bytes,
                &value
                    .color()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Color"))?,
            ),
            AbiValueType::VECTOR2I => write_i32s(
                &mut self.bytes,
                &value
                    .vector2i()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Vector2i"))?,
            ),
            AbiValueType::VECTOR3I => write_i32s(
                &mut self.bytes,
                &value
                    .vector3i()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Vector3i"))?,
            ),
            AbiValueType::RECT2 => write_f32s(
                &mut self.bytes,
                &value
                    .rect2()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Rect2"))?,
            ),
            AbiValueType::RECT2I => write_i32s(
                &mut self.bytes,
                &value
                    .rect2i()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Rect2i"))?,
            ),
            AbiValueType::QUATERNION => write_f32s(
                &mut self.bytes,
                &value
                    .quaternion()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Quaternion"))?,
            ),
            AbiValueType::PLANE => write_f32s(
                &mut self.bytes,
                &value
                    .plane()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Plane"))?,
            ),
            AbiValueType::VECTOR4 => write_f32s(
                &mut self.bytes,
                &value
                    .vector4()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Vector4"))?,
            ),
            AbiValueType::VECTOR4I => write_i32s(
                &mut self.bytes,
                &value
                    .vector4i()
                    .ok_or_else(|| internal("Godot returned an invalid dynamic Vector4i"))?,
            ),
            AbiValueType::TRANSFORM2D
            | AbiValueType::AABB
            | AbiValueType::BASIS
            | AbiValueType::TRANSFORM3D
            | AbiValueType::PROJECTION => {
                let (pointer, length) = value
                    .byte_range(value.type_)
                    .ok_or_else(|| internal("Godot returned invalid dynamic math bytes"))?;
                // SAFETY: VariantCodec retained this bounded backing through
                // the current encode call.
                let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
                for chunk in bytes.chunks_exact(4) {
                    let component = f32::from_ne_bytes(chunk.try_into().expect("f32 byte width"));
                    self.bytes
                        .extend_from_slice(&component.to_bits().to_le_bytes());
                }
            }
            AbiValueType::PACKED_BYTE_ARRAY
            | AbiValueType::PACKED_INT32_ARRAY
            | AbiValueType::PACKED_INT64_ARRAY
            | AbiValueType::PACKED_FLOAT32_ARRAY
            | AbiValueType::PACKED_FLOAT64_ARRAY
            | AbiValueType::PACKED_STRING_ARRAY
            | AbiValueType::PACKED_VECTOR2_ARRAY
            | AbiValueType::PACKED_VECTOR3_ARRAY
            | AbiValueType::PACKED_COLOR_ARRAY
            | AbiValueType::PACKED_VECTOR4_ARRAY => {
                let (pointer, length) = value
                    .byte_range(value.type_)
                    .ok_or_else(|| internal("Godot returned invalid packed-array bytes"))?;
                // SAFETY: VariantCodec retains this bounded backing through
                // the current encode call.
                let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
                self.bytes.extend_from_slice(bytes);
            }
            AbiValueType::SIGNAL => {
                let (pointer, length) = value
                    .byte_range(AbiValueType::SIGNAL)
                    .ok_or_else(|| internal("Godot returned invalid dynamic Signal bytes"))?;
                // SAFETY: VariantCodec retains this bounded backing through
                // the current encode call.
                let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
                self.bytes.extend_from_slice(bytes);
            }
            _ => return Err(unsupported("Godot returned an unsupported dynamic value")),
        }
        self.end(header)
    }

    fn begin(&mut self, type_: u32) -> usize {
        let header = self.bytes.len();
        self.bytes.extend_from_slice(&type_.to_le_bytes());
        self.bytes.extend_from_slice(&0_u32.to_le_bytes());
        self.bytes.extend_from_slice(&0_u64.to_le_bytes());
        header
    }

    fn end(&mut self, header: usize) -> Result<(), ValueError> {
        let length = self
            .bytes
            .len()
            .checked_sub(header + NODE_HEADER_BYTES)
            .and_then(|length| u64::try_from(length).ok())
            .ok_or_else(|| unsupported("dynamic Godot result length overflowed"))?;
        self.bytes[header + 8..header + 16].copy_from_slice(&length.to_le_bytes());
        if self.bytes.len() > MAX_WIRE_BYTES {
            return Err(unsupported(
                "dynamic Godot result exceeds the Host byte limit",
            ));
        }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        for token in self.container_tokens.drain(..) {
            let _ = crate::container::release(token);
        }
        if self.context.is_null() {
            return;
        }
        for token in self.callable_tokens.drain(..) {
            // SAFETY: Writer is scoped to the module generation that owns
            // this exact EngineCallContext.
            let _ = unsafe { &*self.context }.release_callable(token);
        }
    }
}

struct BorrowedDictionary {
    interface: EngineInterface,
    storage: GDExtensionConstTypePtr,
}

impl BorrowedDictionary {
    fn len(&self) -> Result<usize, ValueError> {
        let size = builtin_method(self.interface, dictionary_type(), c"size", SIZE_HASH)?;
        builtin_size(
            size,
            self.storage,
            "Godot Dictionary returned an invalid size",
        )
    }

    fn keys(&self) -> Result<OwnedArray, ValueError> {
        let keys = builtin_method(self.interface, dictionary_type(), c"keys", KEYS_HASH)?
            .ok_or_else(|| internal("Godot Dictionary keys method is unavailable"))?;
        let mut output = OwnedArray::empty(self.interface)?;
        // SAFETY: Receiver and output are initialized exact builtin values.
        unsafe {
            keys(self.storage.cast_mut(), ptr::null(), output.as_mut_ptr(), 0);
        }
        Ok(output)
    }

    fn get(
        &self,
        key: GDExtensionConstVariantPtr,
    ) -> Result<GDExtensionConstVariantPtr, ValueError> {
        let index = self
            .interface
            .dictionary_operator_index_const
            .ok_or_else(|| internal("Godot Dictionary const index operation is unavailable"))?;
        // SAFETY: Dictionary and key are live official values.
        let value = unsafe { index(self.storage, key) };
        (!value.is_null())
            .then_some(value.cast_const())
            .ok_or_else(|| internal("Godot returned a null Dictionary value"))
    }
}

pub(crate) fn read_string_dictionary(
    interface: EngineInterface,
    storage: GDExtensionConstTypePtr,
) -> Result<Vec<(String, String)>, ValueError> {
    if storage.is_null() {
        return Err(internal("Godot supplied a null Dictionary"));
    }
    let dictionary = BorrowedDictionary { interface, storage };
    let count = dictionary.len()?;
    if count > MAX_CONTAINER_ELEMENTS {
        return Err(unsupported(
            "Godot Dictionary exceeds the Host element limit",
        ));
    }
    let keys = dictionary.keys()?;
    if keys.len()? != count {
        return Err(internal(
            "Godot Dictionary keys size changed while it was being read",
        ));
    }
    let codec = VariantCodec::new(interface)
        .ok_or_else(|| internal("Godot Variant codec is unavailable"))?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let key = keys.get(index)?;
        let value = dictionary.get(key)?;
        let key = codec
            .read_string_value(key)
            .map_err(|_| unsupported("Godot Dictionary key is not a String"))?;
        let value = codec
            .read_string_value(value)
            .map_err(|_| unsupported("Godot Dictionary value is not a String"))?;
        values.push((key, value));
    }
    Ok(values)
}

fn abi_from_node(node: Node<'_>) -> Result<AbiValueV1, ValueError> {
    let payload = node.payload;
    let value = match node.type_ {
        0 if payload.is_empty() => AbiValueV1::NIL,
        1 if payload.len() == 1 && payload[0] <= 1 => AbiValueV1::from_bool(payload[0] != 0),
        2 => AbiValueV1::from_i64(read_i64_exact(payload)?),
        3 => AbiValueV1::from_f64(f64::from_bits(read_u64_exact(payload)?)),
        4 => AbiValueV1::from_object_id(read_u64_exact(payload)?),
        6 => AbiValueV1::from_borrowed_utf8(read_text(payload)?),
        7 => AbiValueV1::from_vector2(read_f32(payload, 0)?, read_f32(payload, 1)?),
        8 => AbiValueV1::from_vector3(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
        ),
        9 => AbiValueV1::from_color(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        ),
        10 => AbiValueV1::from_vector2i(read_i32(payload, 0)?, read_i32(payload, 1)?),
        11 => AbiValueV1::from_vector3i(
            read_i32(payload, 0)?,
            read_i32(payload, 1)?,
            read_i32(payload, 2)?,
        ),
        12 => AbiValueV1::from_rid(read_u64_exact(payload)?),
        13 => AbiValueV1::from_borrowed_string_name(read_text(payload)?),
        14 => AbiValueV1::from_borrowed_node_path(read_text(payload)?),
        15 => AbiValueV1::from_rect2(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        ),
        16 => AbiValueV1::from_rect2i(
            read_i32(payload, 0)?,
            read_i32(payload, 1)?,
            read_i32(payload, 2)?,
            read_i32(payload, 3)?,
        ),
        17 => AbiValueV1::from_quaternion(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        ),
        18 => AbiValueV1::from_plane(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        ),
        19 => AbiValueV1::from_vector4(
            read_f32(payload, 0)?,
            read_f32(payload, 1)?,
            read_f32(payload, 2)?,
            read_f32(payload, 3)?,
        ),
        20 => AbiValueV1::from_vector4i(
            read_i32(payload, 0)?,
            read_i32(payload, 1)?,
            read_i32(payload, 2)?,
            read_i32(payload, 3)?,
        ),
        26..=35 => {
            let type_ = AbiValueType(node.type_);
            AbiValueV1::from_borrowed_bytes(type_, payload)
        }
        40 => AbiValueV1::from_borrowed_bytes(AbiValueType::SIGNAL, payload),
        _ => {
            return Err(invalid(
                "dynamic Godot argument has an invalid type or payload",
            ));
        }
    };
    Ok(value)
}

fn variant_from_math_node(
    codec: &VariantCodec,
    node: Node<'_>,
) -> Result<OwnedVariant, ValueError> {
    let (type_, count) = match node.type_ {
        21 => (AbiValueType::TRANSFORM2D, 6),
        22 => (AbiValueType::AABB, 6),
        23 => (AbiValueType::BASIS, 9),
        24 => (AbiValueType::TRANSFORM3D, 12),
        25 => (AbiValueType::PROJECTION, 16),
        _ => return Err(invalid("dynamic Godot math type is invalid")),
    };
    if node.payload.len() != count * 4 {
        return Err(invalid("dynamic Godot math payload has an invalid size"));
    }
    let components = (0..count)
        .map(|index| read_f32(node.payload, index))
        .collect::<Result<Vec<_>, _>>()?;
    let value = AbiValueV1::from_borrowed_f32_components(type_, &components);
    OwnedVariant::from_abi(codec, value)
        .map_err(|_| invalid("dynamic Godot math payload could not be constructed"))
}

fn variant_from_builtin(
    codec: &VariantCodec,
    type_: GDExtensionVariantType,
    value: GDExtensionConstTypePtr,
) -> Result<OwnedVariant, ValueError> {
    let get = codec
        .interface()
        .get_variant_from_type_constructor
        .ok_or_else(|| internal("Godot Variant constructor lookup is unavailable"))?;
    // SAFETY: Type is the exact initialized builtin supplied by the caller.
    let constructor = unsafe { get(type_) }
        .ok_or_else(|| internal("Godot dynamic Variant constructor is unavailable"))?;
    let mut result = OwnedVariant::uninitialized(codec.interface());
    // SAFETY: Destination is uninitialized Variant storage and source is the
    // exact live builtin for this constructor.
    unsafe { constructor(result.as_mut_ptr(), value.cast_mut()) };
    result.mark_initialized();
    Ok(result)
}

fn copy_variant(
    codec: &VariantCodec,
    value: GDExtensionConstVariantPtr,
) -> Result<OwnedVariant, ValueError> {
    let copy = codec
        .interface()
        .variant_new_copy
        .ok_or_else(|| internal("Godot Variant copy constructor is unavailable"))?;
    let mut result = OwnedVariant::uninitialized(codec.interface());
    // SAFETY: Destination is uninitialized Variant storage and the source is
    // live for this synchronous ScriptInstance callback.
    unsafe { copy(result.as_mut_ptr(), value) };
    result.mark_initialized();
    Ok(result)
}

fn borrowed_builtin(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
    value: GDExtensionConstVariantPtr,
) -> Result<GDExtensionConstTypePtr, ValueError> {
    let get = interface
        .variant_get_ptr_internal_getter
        .ok_or_else(|| internal("Godot Variant internal getter is unavailable"))?;
    // SAFETY: Type is read from this exact Variant.
    let get = unsafe { get(type_) }
        .ok_or_else(|| internal("Godot dynamic builtin getter is unavailable"))?;
    // SAFETY: The caller checked the Variant's exact type.
    let value = unsafe { get(value.cast_mut()) };
    (!value.is_null())
        .then_some(value.cast_const())
        .ok_or_else(|| internal("Godot returned a null dynamic builtin"))
}

fn replace_variant(
    interface: EngineInterface,
    output: GDExtensionVariantPtr,
    value: GDExtensionConstVariantPtr,
) -> Result<(), ValueError> {
    if output.is_null() {
        return Err(internal("Godot returned a null dynamic value slot"));
    }
    let destroy = interface
        .variant_destroy
        .ok_or_else(|| internal("Godot Variant destructor is unavailable"))?;
    let copy = interface
        .variant_new_copy
        .ok_or_else(|| internal("Godot Variant copy constructor is unavailable"))?;
    // SAFETY: Output is an initialized container slot. Destroying it and
    // immediately copy-constructing from the live input preserves ownership.
    unsafe {
        destroy(output);
        copy(output, value);
    }
    Ok(())
}

fn construct_default(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
    output: GDExtensionTypePtr,
) -> Result<(), ValueError> {
    let get = interface
        .variant_get_ptr_constructor
        .ok_or_else(|| internal("Godot builtin constructor lookup is unavailable"))?;
    // SAFETY: Constructor zero is the official default constructor.
    let constructor = unsafe { get(type_, 0) }
        .ok_or_else(|| internal("Godot dynamic builtin constructor is unavailable"))?;
    // SAFETY: Output is uninitialized exact builtin storage.
    unsafe { constructor(output, ptr::null()) };
    Ok(())
}

fn destructor(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
) -> Result<GDExtensionPtrDestructor, ValueError> {
    let get = interface
        .variant_get_ptr_destructor
        .ok_or_else(|| internal("Godot builtin destructor lookup is unavailable"))?;
    // SAFETY: Type is an official owned builtin.
    let destroy = unsafe { get(type_) };
    destroy.ok_or_else(|| internal("Godot dynamic builtin destructor is unavailable"))?;
    Ok(destroy)
}

fn builtin_method(
    interface: EngineInterface,
    type_: GDExtensionVariantType,
    name: &'static core::ffi::CStr,
    hash: i64,
) -> Result<GDExtensionPtrBuiltInMethod, ValueError> {
    let get = interface
        .variant_get_ptr_builtin_method
        .ok_or_else(|| internal("Godot builtin method lookup is unavailable"))?;
    let name = StaticStringName::new(interface, name);
    // SAFETY: Type, name and hash come from the authenticated baseline API.
    let method = unsafe { get(type_, name.as_ptr(), hash) };
    method.ok_or_else(|| internal("Godot dynamic builtin method is unavailable"))?;
    Ok(method)
}

fn builtin_size(
    method: GDExtensionPtrBuiltInMethod,
    value: GDExtensionConstTypePtr,
    message: &'static str,
) -> Result<usize, ValueError> {
    let method = method.ok_or_else(|| internal("Godot size method is unavailable"))?;
    let mut count = 0_i64;
    // SAFETY: Receiver is an initialized builtin and size takes no arguments.
    unsafe {
        method(
            value.cast_mut(),
            ptr::null(),
            ptr::from_mut(&mut count).cast(),
            0,
        );
    }
    let count = usize::try_from(count).map_err(|_| internal(message))?;
    if count > MAX_CONTAINER_ELEMENTS {
        return Err(unsupported(
            "dynamic Godot container exceeds the Host element limit",
        ));
    }
    Ok(count)
}

fn container_count(payload: &[u8]) -> Result<usize, ValueError> {
    let bytes = payload
        .get(..8)
        .ok_or_else(|| invalid("dynamic Godot container count is truncated"))?;
    let count = usize::try_from(u64::from_le_bytes(bytes.try_into().expect("u64 width")))
        .map_err(|_| invalid("dynamic Godot container count is out of range"))?;
    if count > MAX_CONTAINER_ELEMENTS {
        return Err(invalid(
            "dynamic Godot container exceeds the Host element limit",
        ));
    }
    Ok(count)
}

fn abi_type(type_: GDExtensionVariantType) -> Option<AbiValueType> {
    Some(match type_ {
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL => AbiValueType::NIL,
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL => {
            AbiValueType::BOOL
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT => AbiValueType::I64,
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT => {
            AbiValueType::F64
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING => {
            AbiValueType::STRING
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME => {
            AbiValueType::STRING_NAME
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH => {
            AbiValueType::NODE_PATH
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT => {
            AbiValueType::OBJECT_ID
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2 => {
            AbiValueType::VECTOR2
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I => {
            AbiValueType::VECTOR2I
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3 => {
            AbiValueType::VECTOR3
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I => {
            AbiValueType::VECTOR3I
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4 => {
            AbiValueType::VECTOR4
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I => {
            AbiValueType::VECTOR4I
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2 => {
            AbiValueType::RECT2
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I => {
            AbiValueType::RECT2I
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION => {
            AbiValueType::QUATERNION
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE => {
            AbiValueType::PLANE
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D => {
            AbiValueType::TRANSFORM2D
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB => {
            AbiValueType::AABB
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS => {
            AbiValueType::BASIS
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D => {
            AbiValueType::TRANSFORM3D
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION => {
            AbiValueType::PROJECTION
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR => {
            AbiValueType::COLOR
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID => AbiValueType::RID,
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE => {
            AbiValueType::CALLABLE
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL => {
            AbiValueType::SIGNAL
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY => {
            AbiValueType::PACKED_BYTE_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY => {
            AbiValueType::PACKED_INT32_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY => {
            AbiValueType::PACKED_INT64_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY => {
            AbiValueType::PACKED_FLOAT32_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY => {
            AbiValueType::PACKED_FLOAT64_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY => {
            AbiValueType::PACKED_STRING_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY => {
            AbiValueType::PACKED_VECTOR2_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY => {
            AbiValueType::PACKED_VECTOR3_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY => {
            AbiValueType::PACKED_COLOR_ARRAY
        }
        value if value == GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY => {
            AbiValueType::PACKED_VECTOR4_ARRAY
        }
        _ => return None,
    })
}

const fn array_type() -> GDExtensionVariantType {
    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY
}

const fn dictionary_type() -> GDExtensionVariantType {
    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY
}

pub(crate) fn builtin_variant_type(name: &str) -> Option<GDExtensionVariantType> {
    Some(match name {
        "Nil" | "Variant" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL,
        "bool" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL,
        "int" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
        "float" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT,
        "String" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING,
        "Vector2" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2,
        "Vector2i" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I,
        "Rect2" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2,
        "Rect2i" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I,
        "Vector3" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3,
        "Vector3i" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I,
        "Transform2D" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D,
        "Vector4" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4,
        "Vector4i" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I,
        "Plane" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE,
        "Quaternion" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION,
        "AABB" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB,
        "Basis" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS,
        "Transform3D" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D,
        "Projection" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION,
        "Color" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR,
        "StringName" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
        "NodePath" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH,
        "RID" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID,
        "Object" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
        "Callable" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE,
        "Signal" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_SIGNAL,
        "Dictionary" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_DICTIONARY,
        "Array" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_ARRAY,
        "PackedByteArray" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_BYTE_ARRAY,
        "PackedInt32Array" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT32_ARRAY,
        "PackedInt64Array" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_INT64_ARRAY,
        "PackedFloat32Array" => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT32_ARRAY
        }
        "PackedFloat64Array" => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_FLOAT64_ARRAY
        }
        "PackedStringArray" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
        "PackedVector2Array" => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR2_ARRAY
        }
        "PackedVector3Array" => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR3_ARRAY
        }
        "PackedColorArray" => GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_COLOR_ARRAY,
        "PackedVector4Array" => {
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_VECTOR4_ARRAY
        }
        _ => return None,
    })
}

fn check_depth(depth: usize) -> Result<(), ValueError> {
    if depth > MAX_NESTING_DEPTH {
        Err(invalid(
            "dynamic Godot value exceeds the Host nesting-depth limit",
        ))
    } else {
        Ok(())
    }
}

fn read_text(bytes: &[u8]) -> Result<&str, ValueError> {
    core::str::from_utf8(bytes).map_err(|_| invalid("dynamic Godot text is not valid UTF-8"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn read_i64_exact(bytes: &[u8]) -> Result<i64, ValueError> {
    Ok(i64::from_le_bytes(bytes.try_into().map_err(|_| {
        invalid("dynamic Godot integer has an invalid size")
    })?))
}

fn read_u64_exact(bytes: &[u8]) -> Result<u64, ValueError> {
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
        invalid("dynamic Godot integer has an invalid size")
    })?))
}

fn read_i32(bytes: &[u8], index: usize) -> Result<i32, ValueError> {
    let start = index
        .checked_mul(4)
        .ok_or_else(|| invalid("dynamic Godot component index overflowed"))?;
    Ok(i32::from_le_bytes(
        bytes
            .get(start..start + 4)
            .ok_or_else(|| invalid("dynamic Godot component payload is truncated"))?
            .try_into()
            .expect("i32 width"),
    ))
}

fn read_f32(bytes: &[u8], index: usize) -> Result<f32, ValueError> {
    Ok(f32::from_bits(read_i32(bytes, index)? as u32))
}

fn write_f32s(output: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        output.extend_from_slice(&value.to_bits().to_le_bytes());
    }
}

fn write_i32s(output: &mut Vec<u8>, values: &[i32]) {
    for value in values {
        output.extend_from_slice(&value.to_le_bytes());
    }
}

fn invalid(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::InvalidArgument, message)
}

fn unsupported(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Unsupported, message)
}

fn internal(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Internal, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_rejects_invalid_headers_and_noncanonical_nodes() {
        assert!(Reader::new(&[], None).is_err());
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WIRE_MAGIC);
        bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        let mut reader = Reader::new(&bytes, None).expect("header");
        assert!(reader.node(0).is_err());
    }

    #[test]
    fn type_mapping_covers_every_wire_scalar_and_packed_value() {
        for value in 0..=35 {
            if value == 5 {
                continue;
            }
            assert!(
                value <= 35,
                "wire type {value} remains within the public range"
            );
        }
        assert_eq!(array_type().0, 28);
        assert_eq!(dictionary_type().0, 27);
    }
}
