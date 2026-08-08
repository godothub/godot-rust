use core::fmt;
use core::mem;
use core::ptr;
use godot_api::{
    GDExtensionClassLibraryPtr, GDExtensionGodotVersion, GDExtensionInterfaceArrayOperatorIndex,
    GDExtensionInterfaceArrayOperatorIndexConst, GDExtensionInterfaceArraySetTyped,
    GDExtensionInterfaceCallableCustomCreate2, GDExtensionInterfaceClassdbConstructObject2,
    GDExtensionInterfaceClassdbGetClassTag, GDExtensionInterfaceClassdbGetMethodBind,
    GDExtensionInterfaceClassdbRegisterExtensionClass4,
    GDExtensionInterfaceClassdbUnregisterExtensionClass,
    GDExtensionInterfaceDictionaryOperatorIndex, GDExtensionInterfaceDictionaryOperatorIndexConst,
    GDExtensionInterfaceFunctionPtr, GDExtensionInterfaceGetGodotVersion,
    GDExtensionInterfaceGetProcAddress, GDExtensionInterfaceGetVariantFromTypeConstructor,
    GDExtensionInterfaceGetVariantGetInternalPtrFunc,
    GDExtensionInterfaceGetVariantToTypeConstructor, GDExtensionInterfaceGlobalGetSingleton,
    GDExtensionInterfaceObjectCastTo, GDExtensionInterfaceObjectDestroy,
    GDExtensionInterfaceObjectGetInstanceFromId, GDExtensionInterfaceObjectGetInstanceId,
    GDExtensionInterfaceObjectGetScriptInstance, GDExtensionInterfaceObjectMethodBindCall,
    GDExtensionInterfaceObjectMethodBindPtrcall, GDExtensionInterfaceObjectSetInstance,
    GDExtensionInterfacePackedStringArrayOperatorIndexConst,
    GDExtensionInterfacePlaceHolderScriptInstanceCreate,
    GDExtensionInterfacePrintScriptErrorWithMessage, GDExtensionInterfaceRefGetObject,
    GDExtensionInterfaceRefSetObject, GDExtensionInterfaceScriptInstanceCreate3,
    GDExtensionInterfaceStringNameNewWithLatin1Chars,
    GDExtensionInterfaceStringNameNewWithUtf8CharsAndLen,
    GDExtensionInterfaceStringNewWithLatin1Chars,
    GDExtensionInterfaceStringNewWithUtf8CharsAndLen2, GDExtensionInterfaceStringToUtf8Chars,
    GDExtensionInterfaceVariantCall, GDExtensionInterfaceVariantDestroy,
    GDExtensionInterfaceVariantGetConstantValue, GDExtensionInterfaceVariantGetNamed,
    GDExtensionInterfaceVariantGetObjectInstanceId, GDExtensionInterfaceVariantGetPtrBuiltinMethod,
    GDExtensionInterfaceVariantGetPtrConstructor, GDExtensionInterfaceVariantGetPtrDestructor,
    GDExtensionInterfaceVariantGetPtrGetter, GDExtensionInterfaceVariantGetPtrIndexedGetter,
    GDExtensionInterfaceVariantGetPtrIndexedSetter, GDExtensionInterfaceVariantGetPtrKeyedGetter,
    GDExtensionInterfaceVariantGetPtrKeyedSetter,
    GDExtensionInterfaceVariantGetPtrOperatorEvaluator, GDExtensionInterfaceVariantGetPtrSetter,
    GDExtensionInterfaceVariantGetPtrUtilityFunction, GDExtensionInterfaceVariantGetType,
    GDExtensionInterfaceVariantNewCopy, GDExtensionInterfaceVariantNewNil,
    GDExtensionInterfaceVariantSetNamed,
};

use crate::version::{EngineVersion, is_supported_godot};

/// Failure while resolving the minimum official Host interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InterfaceLoadError {
    MissingResolver,
    NullLibrary,
    MissingFunction(&'static str),
    UnsupportedVersion(EngineVersion),
}

impl fmt::Display for InterfaceLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingResolver => formatter.write_str("Godot supplied no interface resolver"),
            Self::NullLibrary => formatter.write_str("Godot supplied a null library pointer"),
            Self::MissingFunction(name) => {
                write!(
                    formatter,
                    "required GDExtension interface `{name}` is missing"
                )
            }
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "unsupported Godot version {}.{}.{}",
                version.major, version.minor, version.patch
            ),
        }
    }
}

/// Official engine interface resolved and owned for one Host library lifetime.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EngineInterface {
    pub(crate) get_proc_address: unsafe extern "C" fn(
        function_name: *const core::ffi::c_char,
    ) -> GDExtensionInterfaceFunctionPtr,
    library: usize,
    version: EngineVersion,
    pub(crate) string_name_new_with_latin1_chars: GDExtensionInterfaceStringNameNewWithLatin1Chars,
    pub(crate) string_name_new_with_utf8_chars_and_len:
        GDExtensionInterfaceStringNameNewWithUtf8CharsAndLen,
    pub(crate) string_new_with_latin1_chars: GDExtensionInterfaceStringNewWithLatin1Chars,
    pub(crate) string_new_with_utf8_chars_and_len2:
        GDExtensionInterfaceStringNewWithUtf8CharsAndLen2,
    pub(crate) string_to_utf8_chars: GDExtensionInterfaceStringToUtf8Chars,
    pub(crate) variant_new_nil: GDExtensionInterfaceVariantNewNil,
    pub(crate) variant_new_copy: GDExtensionInterfaceVariantNewCopy,
    pub(crate) variant_destroy: GDExtensionInterfaceVariantDestroy,
    pub(crate) variant_call: GDExtensionInterfaceVariantCall,
    pub(crate) variant_get_named: GDExtensionInterfaceVariantGetNamed,
    pub(crate) variant_set_named: GDExtensionInterfaceVariantSetNamed,
    pub(crate) get_variant_from_type_constructor: GDExtensionInterfaceGetVariantFromTypeConstructor,
    pub(crate) get_variant_to_type_constructor: GDExtensionInterfaceGetVariantToTypeConstructor,
    pub(crate) variant_get_ptr_constructor: GDExtensionInterfaceVariantGetPtrConstructor,
    pub(crate) variant_get_ptr_destructor: GDExtensionInterfaceVariantGetPtrDestructor,
    pub(crate) variant_get_ptr_operator_evaluator:
        GDExtensionInterfaceVariantGetPtrOperatorEvaluator,
    pub(crate) variant_get_ptr_builtin_method: GDExtensionInterfaceVariantGetPtrBuiltinMethod,
    pub(crate) variant_get_ptr_setter: GDExtensionInterfaceVariantGetPtrSetter,
    pub(crate) variant_get_ptr_getter: GDExtensionInterfaceVariantGetPtrGetter,
    pub(crate) variant_get_ptr_indexed_setter: GDExtensionInterfaceVariantGetPtrIndexedSetter,
    pub(crate) variant_get_ptr_indexed_getter: GDExtensionInterfaceVariantGetPtrIndexedGetter,
    pub(crate) variant_get_ptr_keyed_setter: GDExtensionInterfaceVariantGetPtrKeyedSetter,
    pub(crate) variant_get_ptr_keyed_getter: GDExtensionInterfaceVariantGetPtrKeyedGetter,
    pub(crate) variant_get_constant_value: GDExtensionInterfaceVariantGetConstantValue,
    pub(crate) variant_get_ptr_utility_function: GDExtensionInterfaceVariantGetPtrUtilityFunction,
    pub(crate) variant_get_type: GDExtensionInterfaceVariantGetType,
    pub(crate) variant_get_object_instance_id: GDExtensionInterfaceVariantGetObjectInstanceId,
    pub(crate) variant_get_ptr_internal_getter: GDExtensionInterfaceGetVariantGetInternalPtrFunc,
    pub(crate) dictionary_operator_index: GDExtensionInterfaceDictionaryOperatorIndex,
    pub(crate) dictionary_operator_index_const: GDExtensionInterfaceDictionaryOperatorIndexConst,
    pub(crate) array_operator_index: GDExtensionInterfaceArrayOperatorIndex,
    pub(crate) array_operator_index_const: GDExtensionInterfaceArrayOperatorIndexConst,
    pub(crate) array_set_typed: GDExtensionInterfaceArraySetTyped,
    pub(crate) packed_string_array_operator_index_const:
        GDExtensionInterfacePackedStringArrayOperatorIndexConst,
    pub(crate) callable_custom_create2: GDExtensionInterfaceCallableCustomCreate2,
    pub(crate) print_script_error_with_message: GDExtensionInterfacePrintScriptErrorWithMessage,
    pub(crate) classdb_register_extension_class4:
        GDExtensionInterfaceClassdbRegisterExtensionClass4,
    pub(crate) classdb_unregister_extension_class:
        GDExtensionInterfaceClassdbUnregisterExtensionClass,
    pub(crate) classdb_construct_object2: GDExtensionInterfaceClassdbConstructObject2,
    pub(crate) classdb_get_method_bind: GDExtensionInterfaceClassdbGetMethodBind,
    pub(crate) classdb_get_class_tag: GDExtensionInterfaceClassdbGetClassTag,
    pub(crate) object_set_instance: GDExtensionInterfaceObjectSetInstance,
    pub(crate) object_destroy: GDExtensionInterfaceObjectDestroy,
    pub(crate) object_cast_to: GDExtensionInterfaceObjectCastTo,
    pub(crate) object_get_instance_from_id: GDExtensionInterfaceObjectGetInstanceFromId,
    pub(crate) object_get_instance_id: GDExtensionInterfaceObjectGetInstanceId,
    pub(crate) object_method_bind_call: GDExtensionInterfaceObjectMethodBindCall,
    pub(crate) object_method_bind_ptrcall: GDExtensionInterfaceObjectMethodBindPtrcall,
    pub(crate) ref_get_object: GDExtensionInterfaceRefGetObject,
    pub(crate) ref_set_object: GDExtensionInterfaceRefSetObject,
    pub(crate) object_get_script_instance: GDExtensionInterfaceObjectGetScriptInstance,
    pub(crate) global_get_singleton: GDExtensionInterfaceGlobalGetSingleton,
    pub(crate) script_instance_create3: GDExtensionInterfaceScriptInstanceCreate3,
    pub(crate) placeholder_script_instance_create:
        GDExtensionInterfacePlaceHolderScriptInstanceCreate,
}

macro_rules! resolve {
    ($resolver:expr, $name:literal, $target:ty) => {{
        const NAME: &str = $name;
        const NAME_C: &[u8] = concat!($name, "\0").as_bytes();
        // SAFETY: The official resolver accepts a valid nul-terminated name.
        let raw = unsafe { $resolver(NAME_C.as_ptr().cast()) };
        if raw.is_none() {
            return Err(InterfaceLoadError::MissingFunction(NAME));
        }
        // SAFETY: The official header defines the named function with exactly
        // `$target`; both source and target retain the nullable function-pointer
        // representation.
        unsafe { mem::transmute::<GDExtensionInterfaceFunctionPtr, $target>(raw) }
    }};
}

impl EngineInterface {
    /// Resolves the minimum official interface needed to bootstrap the Host.
    ///
    /// # Safety
    ///
    /// The callbacks and pointer must come from a matching Godot
    /// GDExtension entry call and remain valid for the library lifetime.
    pub(crate) unsafe fn load(
        get_proc_address: GDExtensionInterfaceGetProcAddress,
        library: GDExtensionClassLibraryPtr,
    ) -> Result<Self, InterfaceLoadError> {
        let get_proc_address = get_proc_address.ok_or(InterfaceLoadError::MissingResolver)?;
        if library.is_null() {
            return Err(InterfaceLoadError::NullLibrary);
        }

        let get_version = resolve!(
            get_proc_address,
            "get_godot_version",
            GDExtensionInterfaceGetGodotVersion
        )
        .expect("required interface was checked");
        let mut raw_version = GDExtensionGodotVersion {
            major: 0,
            minor: 0,
            patch: 0,
            string: ptr::null(),
        };
        // SAFETY: Godot supplied this function pointer and `raw_version` is writable.
        unsafe { get_version(&mut raw_version) };

        let version = EngineVersion {
            major: raw_version.major,
            minor: raw_version.minor,
            patch: raw_version.patch,
        };
        if !is_supported_godot(version) {
            return Err(InterfaceLoadError::UnsupportedVersion(version));
        }

        Ok(Self {
            get_proc_address,
            library: library as usize,
            version,
            string_name_new_with_latin1_chars: resolve!(
                get_proc_address,
                "string_name_new_with_latin1_chars",
                GDExtensionInterfaceStringNameNewWithLatin1Chars
            ),
            string_name_new_with_utf8_chars_and_len: resolve!(
                get_proc_address,
                "string_name_new_with_utf8_chars_and_len",
                GDExtensionInterfaceStringNameNewWithUtf8CharsAndLen
            ),
            string_new_with_latin1_chars: resolve!(
                get_proc_address,
                "string_new_with_latin1_chars",
                GDExtensionInterfaceStringNewWithLatin1Chars
            ),
            string_new_with_utf8_chars_and_len2: resolve!(
                get_proc_address,
                "string_new_with_utf8_chars_and_len2",
                GDExtensionInterfaceStringNewWithUtf8CharsAndLen2
            ),
            string_to_utf8_chars: resolve!(
                get_proc_address,
                "string_to_utf8_chars",
                GDExtensionInterfaceStringToUtf8Chars
            ),
            variant_new_nil: resolve!(
                get_proc_address,
                "variant_new_nil",
                GDExtensionInterfaceVariantNewNil
            ),
            variant_new_copy: resolve!(
                get_proc_address,
                "variant_new_copy",
                GDExtensionInterfaceVariantNewCopy
            ),
            variant_destroy: resolve!(
                get_proc_address,
                "variant_destroy",
                GDExtensionInterfaceVariantDestroy
            ),
            variant_call: resolve!(
                get_proc_address,
                "variant_call",
                GDExtensionInterfaceVariantCall
            ),
            variant_get_named: resolve!(
                get_proc_address,
                "variant_get_named",
                GDExtensionInterfaceVariantGetNamed
            ),
            variant_set_named: resolve!(
                get_proc_address,
                "variant_set_named",
                GDExtensionInterfaceVariantSetNamed
            ),
            get_variant_from_type_constructor: resolve!(
                get_proc_address,
                "get_variant_from_type_constructor",
                GDExtensionInterfaceGetVariantFromTypeConstructor
            ),
            get_variant_to_type_constructor: resolve!(
                get_proc_address,
                "get_variant_to_type_constructor",
                GDExtensionInterfaceGetVariantToTypeConstructor
            ),
            variant_get_ptr_constructor: resolve!(
                get_proc_address,
                "variant_get_ptr_constructor",
                GDExtensionInterfaceVariantGetPtrConstructor
            ),
            variant_get_ptr_destructor: resolve!(
                get_proc_address,
                "variant_get_ptr_destructor",
                GDExtensionInterfaceVariantGetPtrDestructor
            ),
            variant_get_ptr_operator_evaluator: resolve!(
                get_proc_address,
                "variant_get_ptr_operator_evaluator",
                GDExtensionInterfaceVariantGetPtrOperatorEvaluator
            ),
            variant_get_ptr_builtin_method: resolve!(
                get_proc_address,
                "variant_get_ptr_builtin_method",
                GDExtensionInterfaceVariantGetPtrBuiltinMethod
            ),
            variant_get_ptr_setter: resolve!(
                get_proc_address,
                "variant_get_ptr_setter",
                GDExtensionInterfaceVariantGetPtrSetter
            ),
            variant_get_ptr_getter: resolve!(
                get_proc_address,
                "variant_get_ptr_getter",
                GDExtensionInterfaceVariantGetPtrGetter
            ),
            variant_get_ptr_indexed_setter: resolve!(
                get_proc_address,
                "variant_get_ptr_indexed_setter",
                GDExtensionInterfaceVariantGetPtrIndexedSetter
            ),
            variant_get_ptr_indexed_getter: resolve!(
                get_proc_address,
                "variant_get_ptr_indexed_getter",
                GDExtensionInterfaceVariantGetPtrIndexedGetter
            ),
            variant_get_ptr_keyed_setter: resolve!(
                get_proc_address,
                "variant_get_ptr_keyed_setter",
                GDExtensionInterfaceVariantGetPtrKeyedSetter
            ),
            variant_get_ptr_keyed_getter: resolve!(
                get_proc_address,
                "variant_get_ptr_keyed_getter",
                GDExtensionInterfaceVariantGetPtrKeyedGetter
            ),
            variant_get_constant_value: resolve!(
                get_proc_address,
                "variant_get_constant_value",
                GDExtensionInterfaceVariantGetConstantValue
            ),
            variant_get_ptr_utility_function: resolve!(
                get_proc_address,
                "variant_get_ptr_utility_function",
                GDExtensionInterfaceVariantGetPtrUtilityFunction
            ),
            variant_get_type: resolve!(
                get_proc_address,
                "variant_get_type",
                GDExtensionInterfaceVariantGetType
            ),
            variant_get_object_instance_id: resolve!(
                get_proc_address,
                "variant_get_object_instance_id",
                GDExtensionInterfaceVariantGetObjectInstanceId
            ),
            variant_get_ptr_internal_getter: resolve!(
                get_proc_address,
                "variant_get_ptr_internal_getter",
                GDExtensionInterfaceGetVariantGetInternalPtrFunc
            ),
            dictionary_operator_index: resolve!(
                get_proc_address,
                "dictionary_operator_index",
                GDExtensionInterfaceDictionaryOperatorIndex
            ),
            dictionary_operator_index_const: resolve!(
                get_proc_address,
                "dictionary_operator_index_const",
                GDExtensionInterfaceDictionaryOperatorIndexConst
            ),
            array_operator_index: resolve!(
                get_proc_address,
                "array_operator_index",
                GDExtensionInterfaceArrayOperatorIndex
            ),
            array_operator_index_const: resolve!(
                get_proc_address,
                "array_operator_index_const",
                GDExtensionInterfaceArrayOperatorIndexConst
            ),
            array_set_typed: resolve!(
                get_proc_address,
                "array_set_typed",
                GDExtensionInterfaceArraySetTyped
            ),
            packed_string_array_operator_index_const: resolve!(
                get_proc_address,
                "packed_string_array_operator_index_const",
                GDExtensionInterfacePackedStringArrayOperatorIndexConst
            ),
            callable_custom_create2: resolve!(
                get_proc_address,
                "callable_custom_create2",
                GDExtensionInterfaceCallableCustomCreate2
            ),
            print_script_error_with_message: resolve!(
                get_proc_address,
                "print_script_error_with_message",
                GDExtensionInterfacePrintScriptErrorWithMessage
            ),
            classdb_register_extension_class4: resolve!(
                get_proc_address,
                "classdb_register_extension_class4",
                GDExtensionInterfaceClassdbRegisterExtensionClass4
            ),
            classdb_unregister_extension_class: resolve!(
                get_proc_address,
                "classdb_unregister_extension_class",
                GDExtensionInterfaceClassdbUnregisterExtensionClass
            ),
            classdb_construct_object2: resolve!(
                get_proc_address,
                "classdb_construct_object2",
                GDExtensionInterfaceClassdbConstructObject2
            ),
            classdb_get_method_bind: resolve!(
                get_proc_address,
                "classdb_get_method_bind",
                GDExtensionInterfaceClassdbGetMethodBind
            ),
            classdb_get_class_tag: resolve!(
                get_proc_address,
                "classdb_get_class_tag",
                GDExtensionInterfaceClassdbGetClassTag
            ),
            object_set_instance: resolve!(
                get_proc_address,
                "object_set_instance",
                GDExtensionInterfaceObjectSetInstance
            ),
            object_destroy: resolve!(
                get_proc_address,
                "object_destroy",
                GDExtensionInterfaceObjectDestroy
            ),
            object_cast_to: resolve!(
                get_proc_address,
                "object_cast_to",
                GDExtensionInterfaceObjectCastTo
            ),
            object_get_instance_from_id: resolve!(
                get_proc_address,
                "object_get_instance_from_id",
                GDExtensionInterfaceObjectGetInstanceFromId
            ),
            object_get_instance_id: resolve!(
                get_proc_address,
                "object_get_instance_id",
                GDExtensionInterfaceObjectGetInstanceId
            ),
            object_method_bind_call: resolve!(
                get_proc_address,
                "object_method_bind_call",
                GDExtensionInterfaceObjectMethodBindCall
            ),
            object_method_bind_ptrcall: resolve!(
                get_proc_address,
                "object_method_bind_ptrcall",
                GDExtensionInterfaceObjectMethodBindPtrcall
            ),
            ref_get_object: resolve!(
                get_proc_address,
                "ref_get_object",
                GDExtensionInterfaceRefGetObject
            ),
            ref_set_object: resolve!(
                get_proc_address,
                "ref_set_object",
                GDExtensionInterfaceRefSetObject
            ),
            object_get_script_instance: resolve!(
                get_proc_address,
                "object_get_script_instance",
                GDExtensionInterfaceObjectGetScriptInstance
            ),
            global_get_singleton: resolve!(
                get_proc_address,
                "global_get_singleton",
                GDExtensionInterfaceGlobalGetSingleton
            ),
            script_instance_create3: resolve!(
                get_proc_address,
                "script_instance_create3",
                GDExtensionInterfaceScriptInstanceCreate3
            ),
            placeholder_script_instance_create: resolve!(
                get_proc_address,
                "placeholder_script_instance_create",
                GDExtensionInterfacePlaceHolderScriptInstanceCreate
            ),
        })
    }

    pub(crate) fn library(self) -> GDExtensionClassLibraryPtr {
        self.library as GDExtensionClassLibraryPtr
    }

    pub(crate) const fn version(self) -> EngineVersion {
        self.version
    }

    pub(crate) fn resolved_function_count(self) -> usize {
        [
            self.string_name_new_with_latin1_chars.is_some(),
            self.string_name_new_with_utf8_chars_and_len.is_some(),
            self.string_new_with_latin1_chars.is_some(),
            self.string_new_with_utf8_chars_and_len2.is_some(),
            self.string_to_utf8_chars.is_some(),
            self.variant_new_nil.is_some(),
            self.variant_new_copy.is_some(),
            self.variant_destroy.is_some(),
            self.variant_call.is_some(),
            self.variant_get_named.is_some(),
            self.variant_set_named.is_some(),
            self.get_variant_from_type_constructor.is_some(),
            self.get_variant_to_type_constructor.is_some(),
            self.variant_get_ptr_constructor.is_some(),
            self.variant_get_ptr_destructor.is_some(),
            self.variant_get_ptr_operator_evaluator.is_some(),
            self.variant_get_ptr_builtin_method.is_some(),
            self.variant_get_ptr_setter.is_some(),
            self.variant_get_ptr_getter.is_some(),
            self.variant_get_ptr_indexed_setter.is_some(),
            self.variant_get_ptr_indexed_getter.is_some(),
            self.variant_get_ptr_keyed_setter.is_some(),
            self.variant_get_ptr_keyed_getter.is_some(),
            self.variant_get_constant_value.is_some(),
            self.variant_get_ptr_utility_function.is_some(),
            self.variant_get_type.is_some(),
            self.variant_get_object_instance_id.is_some(),
            self.variant_get_ptr_internal_getter.is_some(),
            self.dictionary_operator_index.is_some(),
            self.packed_string_array_operator_index_const.is_some(),
            self.callable_custom_create2.is_some(),
            self.print_script_error_with_message.is_some(),
            self.classdb_register_extension_class4.is_some(),
            self.classdb_unregister_extension_class.is_some(),
            self.classdb_construct_object2.is_some(),
            self.classdb_get_method_bind.is_some(),
            self.classdb_get_class_tag.is_some(),
            self.object_set_instance.is_some(),
            self.object_destroy.is_some(),
            self.object_cast_to.is_some(),
            self.object_get_instance_from_id.is_some(),
            self.object_get_instance_id.is_some(),
            self.object_method_bind_call.is_some(),
            self.object_method_bind_ptrcall.is_some(),
            self.ref_get_object.is_some(),
            self.ref_set_object.is_some(),
            self.object_get_script_instance.is_some(),
            self.global_get_singleton.is_some(),
            self.script_instance_create3.is_some(),
            self.placeholder_script_instance_create.is_some(),
        ]
        .into_iter()
        .filter(|resolved| *resolved)
        .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::{c_char, c_void};
    use std::sync::OnceLock;

    const VERSION_FUNCTION: &[u8] = b"get_godot_version\0";
    static MOCK_VERSION: OnceLock<EngineVersion> = OnceLock::new();

    unsafe extern "C" fn mock_get_godot_version(version: *mut GDExtensionGodotVersion) {
        let value = MOCK_VERSION.get().copied().unwrap_or(EngineVersion {
            major: 4,
            minor: 4,
            patch: 0,
        });
        // SAFETY: The test resolver only exposes this function to code passing
        // a valid local `GDExtensionGodotVersion`.
        unsafe {
            (*version).major = value.major;
            (*version).minor = value.minor;
            (*version).patch = value.patch;
            (*version).string = ptr::null();
        }
    }

    unsafe extern "C" fn stub() {}

    unsafe extern "C" fn mock_get_proc_address(
        name: *const c_char,
    ) -> GDExtensionInterfaceFunctionPtr {
        // SAFETY: The production code supplies a valid nul-terminated name.
        let name = unsafe { std::ffi::CStr::from_ptr(name) };
        if name.to_bytes_with_nul() == VERSION_FUNCTION {
            // SAFETY: This erases the signature in the same way as the
            // official generic function-pointer resolver.
            unsafe {
                mem::transmute::<GDExtensionInterfaceGetGodotVersion, GDExtensionInterfaceFunctionPtr>(
                    Some(mock_get_godot_version),
                )
            }
        } else {
            Some(stub)
        }
    }

    unsafe extern "C" fn missing_get_proc_address(
        name: *const c_char,
    ) -> GDExtensionInterfaceFunctionPtr {
        // SAFETY: Delegates the same valid C name to the complete mock.
        let name_bytes = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
        if name_bytes == b"object_destroy" {
            None
        } else {
            // SAFETY: Test input has the same resolver preconditions.
            unsafe { mock_get_proc_address(name) }
        }
    }

    #[test]
    fn required_interface_is_resolved_by_name() {
        let library = ptr::dangling_mut::<c_void>();
        // SAFETY: Test callbacks and opaque non-null library pointer satisfy
        // `EngineInterface::load` for this isolated lookup test.
        let interface = unsafe { EngineInterface::load(Some(mock_get_proc_address), library) }
            .expect("mock Godot 4.4 should load");

        assert_eq!(interface.version().minor, 4);
        assert_eq!(interface.library(), library);
        assert!(interface.object_destroy.is_some());
        assert_eq!(interface.resolved_function_count(), 50);
    }

    #[test]
    fn missing_required_function_is_rejected() {
        let library = ptr::dangling_mut::<c_void>();
        // SAFETY: The mock intentionally omits one otherwise valid function.
        let error = unsafe { EngineInterface::load(Some(missing_get_proc_address), library) }
            .expect_err("missing required function must fail");
        assert_eq!(error, InterfaceLoadError::MissingFunction("object_destroy"));
    }
}
