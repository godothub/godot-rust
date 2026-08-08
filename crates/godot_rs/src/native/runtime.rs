use core::cell::Cell;
use core::ffi::{CStr, c_char, c_void};
use core::ptr;
use std::ffi::CString;

use super::value::GodotStringName;
use super::{GODOT_API, NativeError, NativeResult, api_snapshot, sys};

#[derive(Clone, Copy)]
pub(crate) struct Interface {
    pub library: sys::GDExtensionClassLibraryPtr,
    pub get_proc_address:
        unsafe extern "C" fn(*const c_char) -> sys::GDExtensionInterfaceFunctionPtr,
    pub string_name_new:
        unsafe extern "C" fn(sys::GDExtensionUninitializedStringNamePtr, *const c_char, i64),
    pub string_new:
        unsafe extern "C" fn(sys::GDExtensionUninitializedStringPtr, *const c_char, i64) -> i64,
    pub string_name_destroy: unsafe extern "C" fn(sys::GDExtensionTypePtr),
    pub string_destroy: unsafe extern "C" fn(sys::GDExtensionTypePtr),
    pub string_to_utf8_chars: unsafe extern "C" fn(
        sys::GDExtensionConstStringPtr,
        *mut c_char,
        sys::GDExtensionInt,
    ) -> sys::GDExtensionInt,
    pub classdb_construct_object2:
        unsafe extern "C" fn(sys::GDExtensionConstStringNamePtr) -> sys::GDExtensionObjectPtr,
    pub classdb_get_class_tag:
        unsafe extern "C" fn(sys::GDExtensionConstStringNamePtr) -> *mut c_void,
    pub classdb_get_method_bind: unsafe extern "C" fn(
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionInt,
    ) -> sys::GDExtensionMethodBindPtr,
    pub classdb_unregister_extension_class:
        unsafe extern "C" fn(sys::GDExtensionClassLibraryPtr, sys::GDExtensionConstStringNamePtr),
    pub classdb_register_extension_class_method: unsafe extern "C" fn(
        sys::GDExtensionClassLibraryPtr,
        sys::GDExtensionConstStringNamePtr,
        *const sys::GDExtensionClassMethodInfo,
    ),
    pub classdb_register_extension_class_property: unsafe extern "C" fn(
        sys::GDExtensionClassLibraryPtr,
        sys::GDExtensionConstStringNamePtr,
        *const sys::GDExtensionPropertyInfo,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionConstStringNamePtr,
    ),
    pub classdb_register_extension_class_property_group: unsafe extern "C" fn(
        sys::GDExtensionClassLibraryPtr,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionConstStringPtr,
        sys::GDExtensionConstStringPtr,
    ),
    pub classdb_register_extension_class_property_subgroup: unsafe extern "C" fn(
        sys::GDExtensionClassLibraryPtr,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionConstStringPtr,
        sys::GDExtensionConstStringPtr,
    ),
    pub classdb_register_extension_class_signal: unsafe extern "C" fn(
        sys::GDExtensionClassLibraryPtr,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionConstStringNamePtr,
        *const sys::GDExtensionPropertyInfo,
        sys::GDExtensionInt,
    ),
    pub classdb_register_extension_class: sys::GDExtensionClassdbRegisterExtensionClass,
    pub object_set_instance: unsafe extern "C" fn(
        sys::GDExtensionObjectPtr,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionClassInstancePtr,
    ),
    pub object_set_instance_binding: unsafe extern "C" fn(
        sys::GDExtensionObjectPtr,
        *mut c_void,
        *mut c_void,
        *const sys::GDExtensionInstanceBindingCallbacks,
    ),
    pub object_destroy: unsafe extern "C" fn(sys::GDExtensionObjectPtr),
    pub object_cast_to: unsafe extern "C" fn(
        sys::GDExtensionConstObjectPtr,
        *mut c_void,
    ) -> sys::GDExtensionObjectPtr,
    pub object_get_instance_from_id:
        unsafe extern "C" fn(sys::GDObjectInstanceID) -> sys::GDExtensionObjectPtr,
    pub object_get_instance_id:
        unsafe extern "C" fn(sys::GDExtensionConstObjectPtr) -> sys::GDObjectInstanceID,
    pub object_method_bind_ptrcall: unsafe extern "C" fn(
        sys::GDExtensionMethodBindPtr,
        sys::GDExtensionObjectPtr,
        *const sys::GDExtensionConstTypePtr,
        sys::GDExtensionTypePtr,
    ),
    pub object_method_bind_call: unsafe extern "C" fn(
        sys::GDExtensionMethodBindPtr,
        sys::GDExtensionObjectPtr,
        *const sys::GDExtensionConstVariantPtr,
        sys::GDExtensionInt,
        sys::GDExtensionUninitializedVariantPtr,
        *mut sys::GDExtensionCallError,
    ),
    pub variant_new_copy: unsafe extern "C" fn(
        sys::GDExtensionUninitializedVariantPtr,
        sys::GDExtensionConstVariantPtr,
    ),
    pub variant_new_nil: unsafe extern "C" fn(sys::GDExtensionUninitializedVariantPtr),
    pub variant_destroy: unsafe extern "C" fn(sys::GDExtensionVariantPtr),
    pub get_variant_from_type_constructor:
        unsafe extern "C" fn(
            sys::GDExtensionVariantType,
        ) -> sys::GDExtensionVariantFromTypeConstructorFunc,
    pub get_variant_to_type_constructor:
        unsafe extern "C" fn(
            sys::GDExtensionVariantType,
        ) -> sys::GDExtensionTypeFromVariantConstructorFunc,
    pub variant_get_ptr_constructor:
        unsafe extern "C" fn(sys::GDExtensionVariantType, i32) -> sys::GDExtensionPtrConstructor,
    pub variant_get_ptr_builtin_method: unsafe extern "C" fn(
        sys::GDExtensionVariantType,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionInt,
    )
        -> sys::GDExtensionPtrBuiltInMethod,
    pub variant_get_ptr_operator_evaluator:
        unsafe extern "C" fn(
            sys::GDExtensionVariantOperator,
            sys::GDExtensionVariantType,
            sys::GDExtensionVariantType,
        ) -> sys::GDExtensionPtrOperatorEvaluator,
    pub variant_get_ptr_setter: unsafe extern "C" fn(
        sys::GDExtensionVariantType,
        sys::GDExtensionConstStringNamePtr,
    ) -> sys::GDExtensionPtrSetter,
    pub variant_get_ptr_getter: unsafe extern "C" fn(
        sys::GDExtensionVariantType,
        sys::GDExtensionConstStringNamePtr,
    ) -> sys::GDExtensionPtrGetter,
    pub variant_get_ptr_indexed_setter:
        unsafe extern "C" fn(sys::GDExtensionVariantType) -> sys::GDExtensionPtrIndexedSetter,
    pub variant_get_ptr_indexed_getter:
        unsafe extern "C" fn(sys::GDExtensionVariantType) -> sys::GDExtensionPtrIndexedGetter,
    pub variant_get_ptr_keyed_setter:
        unsafe extern "C" fn(sys::GDExtensionVariantType) -> sys::GDExtensionPtrKeyedSetter,
    pub variant_get_ptr_keyed_getter:
        unsafe extern "C" fn(sys::GDExtensionVariantType) -> sys::GDExtensionPtrKeyedGetter,
    pub variant_get_constant_value: unsafe extern "C" fn(
        sys::GDExtensionVariantType,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionUninitializedVariantPtr,
    ),
    pub variant_get_ptr_utility_function:
        unsafe extern "C" fn(
            sys::GDExtensionConstStringNamePtr,
            sys::GDExtensionInt,
        ) -> sys::GDExtensionPtrUtilityFunction,
    pub variant_get_ptr_destructor:
        unsafe extern "C" fn(sys::GDExtensionVariantType) -> sys::GDExtensionPtrDestructor,
    pub global_get_singleton:
        unsafe extern "C" fn(sys::GDExtensionConstStringNamePtr) -> sys::GDExtensionObjectPtr,
    pub ref_get_object:
        unsafe extern "C" fn(sys::GDExtensionConstRefPtr) -> sys::GDExtensionObjectPtr,
    pub ref_set_object: unsafe extern "C" fn(sys::GDExtensionRefPtr, sys::GDExtensionObjectPtr),
    pub variant_get_type:
        unsafe extern "C" fn(sys::GDExtensionConstVariantPtr) -> sys::GDExtensionVariantType,
    pub variant_get_object_instance_id:
        unsafe extern "C" fn(sys::GDExtensionConstVariantPtr) -> sys::GDObjectInstanceID,
    pub array_operator_index: unsafe extern "C" fn(
        sys::GDExtensionTypePtr,
        sys::GDExtensionInt,
    ) -> sys::GDExtensionVariantPtr,
    pub array_operator_index_const: unsafe extern "C" fn(
        sys::GDExtensionConstTypePtr,
        sys::GDExtensionInt,
    ) -> sys::GDExtensionVariantPtr,
    pub array_set_typed: unsafe extern "C" fn(
        sys::GDExtensionTypePtr,
        sys::GDExtensionVariantType,
        sys::GDExtensionConstStringNamePtr,
        sys::GDExtensionConstVariantPtr,
    ),
    pub dictionary_operator_index: unsafe extern "C" fn(
        sys::GDExtensionTypePtr,
        sys::GDExtensionConstVariantPtr,
    ) -> sys::GDExtensionVariantPtr,
    pub dictionary_operator_index_const: unsafe extern "C" fn(
        sys::GDExtensionConstTypePtr,
        sys::GDExtensionConstVariantPtr,
    ) -> sys::GDExtensionVariantPtr,
    pub variant_from_bool:
        unsafe extern "C" fn(sys::GDExtensionUninitializedVariantPtr, sys::GDExtensionTypePtr),
    pub bool_from_variant:
        unsafe extern "C" fn(sys::GDExtensionUninitializedTypePtr, sys::GDExtensionVariantPtr),
    pub variant_from_int:
        unsafe extern "C" fn(sys::GDExtensionUninitializedVariantPtr, sys::GDExtensionTypePtr),
    pub int_from_variant:
        unsafe extern "C" fn(sys::GDExtensionUninitializedTypePtr, sys::GDExtensionVariantPtr),
    pub variant_from_float:
        unsafe extern "C" fn(sys::GDExtensionUninitializedVariantPtr, sys::GDExtensionTypePtr),
    pub float_from_variant:
        unsafe extern "C" fn(sys::GDExtensionUninitializedTypePtr, sys::GDExtensionVariantPtr),
    pub variant_from_string:
        unsafe extern "C" fn(sys::GDExtensionUninitializedVariantPtr, sys::GDExtensionTypePtr),
    pub string_from_variant:
        unsafe extern "C" fn(sys::GDExtensionUninitializedTypePtr, sys::GDExtensionVariantPtr),
    pub print_error: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, i32, u8),
    postinitialize_method: sys::GDExtensionMethodBindPtr,
}

thread_local! {
    static ACTIVE_INTERFACE: Cell<Option<Interface>> = const { Cell::new(None) };
}

pub(crate) struct ActiveInterfaceGuard {
    previous: Option<Interface>,
}

pub(crate) fn activate_interface(interface: Interface) -> ActiveInterfaceGuard {
    let previous = ACTIVE_INTERFACE.replace(Some(interface));
    ActiveInterfaceGuard { previous }
}

pub(crate) fn active_interface() -> Option<Interface> {
    ACTIVE_INTERFACE.get()
}

impl Drop for ActiveInterfaceGuard {
    fn drop(&mut self) {
        ACTIVE_INTERFACE.set(self.previous);
    }
}

impl Interface {
    pub unsafe fn load(
        get_proc_address: sys::GDExtensionInterfaceGetProcAddress,
        library: sys::GDExtensionClassLibraryPtr,
    ) -> Result<Self, NativeError> {
        let get_proc_address =
            get_proc_address.ok_or_else(|| NativeError::new("Godot omitted get_proc_address"))?;

        macro_rules! load {
            ($name:literal, $type:ty) => {{
                // SAFETY: The entry callback is valid for the library lifetime,
                // and each official symbol is cast to its generated ABI type.
                let raw = unsafe { get_proc_address(c_str(concat!($name, "\0")).as_ptr()) };
                if raw.is_none() {
                    return Err(NativeError::new(concat!(
                        "Godot does not provide required GDExtension interface `",
                        $name,
                        "`"
                    )));
                }
                // SAFETY: Godot documents every lookup result under this exact
                // symbol with `$type`; all function pointers share one ABI.
                unsafe { core::mem::transmute::<sys::GDExtensionInterfaceFunctionPtr, $type>(raw) }
                    .expect("required interface was checked above")
            }};
        }

        let get_godot_version = load!(
            "get_godot_version",
            sys::GDExtensionInterfaceGetGodotVersion
        );
        let mut version = sys::GDExtensionGodotVersion {
            major: 0,
            minor: 0,
            patch: 0,
            string: ptr::null(),
        };
        // SAFETY: `version` is writable and the loaded function has the
        // generated official signature.
        unsafe { get_godot_version(&mut version) };
        validate_engine_version(version.major, version.minor)?;

        let variant_get_ptr_destructor = load!(
            "variant_get_ptr_destructor",
            sys::GDExtensionInterfaceVariantGetPtrDestructor
        );
        let variant_get_ptr_constructor = load!(
            "variant_get_ptr_constructor",
            sys::GDExtensionInterfaceVariantGetPtrConstructor
        );
        let get_variant_from_type_constructor = load!(
            "get_variant_from_type_constructor",
            sys::GDExtensionInterfaceGetVariantFromTypeConstructor
        );
        let get_variant_to_type_constructor = load!(
            "get_variant_to_type_constructor",
            sys::GDExtensionInterfaceGetVariantToTypeConstructor
        );

        macro_rules! destructor {
            ($variant:ident, $label:literal) => {{
                // SAFETY: The enum value is generated from the same interface.
                unsafe { variant_get_ptr_destructor(sys::GDExtensionVariantType::$variant) }
                    .ok_or_else(|| {
                        NativeError::new(concat!("Godot omitted the ", $label, " destructor"))
                    })?
            }};
        }
        macro_rules! from_variant {
            ($variant:ident, $label:literal) => {{
                // SAFETY: The enum value is generated from the same interface.
                unsafe { get_variant_to_type_constructor(sys::GDExtensionVariantType::$variant) }
                    .ok_or_else(|| {
                        NativeError::new(concat!(
                            "Godot omitted the Variant-to-",
                            $label,
                            " constructor"
                        ))
                    })?
            }};
        }
        macro_rules! to_variant {
            ($variant:ident, $label:literal) => {{
                // SAFETY: The enum value is generated from the same interface.
                unsafe { get_variant_from_type_constructor(sys::GDExtensionVariantType::$variant) }
                    .ok_or_else(|| {
                        NativeError::new(concat!(
                            "Godot omitted the ",
                            $label,
                            "-to-Variant constructor"
                        ))
                    })?
            }};
        }

        let _node_path_destructor = destructor!(GDEXTENSION_VARIANT_TYPE_NODE_PATH, "NodePath");
        for (variant_type, constructor, label) in [
            (
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH,
                2,
                "NodePath(String)",
            ),
            (
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING,
                2,
                "String(StringName)",
            ),
            (
                sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING,
                3,
                "String(NodePath)",
            ),
        ] {
            // SAFETY: Constructor indices are generated from the authenticated
            // official API shared by this selected Native target.
            if unsafe { variant_get_ptr_constructor(variant_type, constructor) }.is_none() {
                return Err(NativeError::new(format!(
                    "Godot omitted the required Native {label} constructor"
                )));
            }
        }

        for variant_type in [
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BOOL,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_INT,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_FLOAT,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_OBJECT,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR2I,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RECT2I,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR3I,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM2D,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_VECTOR4I,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PLANE,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_QUATERNION,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_AABB,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_BASIS,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_TRANSFORM3D,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PROJECTION,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_COLOR,
            sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_RID,
        ] {
            // SAFETY: The type comes from the selected generated API.
            if unsafe { get_variant_from_type_constructor(variant_type) }.is_none()
                // SAFETY: The type comes from the selected generated API.
                || unsafe { get_variant_to_type_constructor(variant_type) }.is_none()
            {
                return Err(NativeError::new(format!(
                    "Godot omitted a safe Native value constructor for Variant type {}",
                    variant_type.0
                )));
            }
        }

        let classdb_register_extension_class = {
            // SAFETY: The selected symbol and normalized function signature are
            // emitted together by godot_rs_api from one official ABI generation.
            let raw = unsafe {
                get_proc_address(
                    sys::CLASSDB_REGISTER_EXTENSION_CLASS_SYMBOL
                        .as_ptr()
                        .cast::<c_char>(),
                )
            };
            if raw.is_none() {
                return Err(NativeError::new(format!(
                    "Godot does not provide the class registration interface required by API {GODOT_API}"
                )));
            }
            // SAFETY: The symbol suffix and selected creation-info pointer type
            // are normalized together in godot_rs_api.
            unsafe {
                core::mem::transmute::<
                    sys::GDExtensionInterfaceFunctionPtr,
                    Option<sys::GDExtensionClassdbRegisterExtensionClass>,
                >(raw)
            }
            .expect("selected class registration interface was checked above")
        };

        let mut interface = Self {
            library,
            get_proc_address,
            string_name_new: load!(
                "string_name_new_with_utf8_chars_and_len",
                sys::GDExtensionInterfaceStringNameNewWithUtf8CharsAndLen
            ),
            string_new: load!(
                "string_new_with_utf8_chars_and_len2",
                sys::GDExtensionInterfaceStringNewWithUtf8CharsAndLen2
            ),
            string_name_destroy: destructor!(GDEXTENSION_VARIANT_TYPE_STRING_NAME, "StringName"),
            string_destroy: destructor!(GDEXTENSION_VARIANT_TYPE_STRING, "String"),
            string_to_utf8_chars: load!(
                "string_to_utf8_chars",
                sys::GDExtensionInterfaceStringToUtf8Chars
            ),
            classdb_construct_object2: load!(
                "classdb_construct_object2",
                sys::GDExtensionInterfaceClassdbConstructObject2
            ),
            classdb_get_class_tag: load!(
                "classdb_get_class_tag",
                sys::GDExtensionInterfaceClassdbGetClassTag
            ),
            classdb_get_method_bind: load!(
                "classdb_get_method_bind",
                sys::GDExtensionInterfaceClassdbGetMethodBind
            ),
            classdb_unregister_extension_class: load!(
                "classdb_unregister_extension_class",
                sys::GDExtensionInterfaceClassdbUnregisterExtensionClass
            ),
            classdb_register_extension_class_method: load!(
                "classdb_register_extension_class_method",
                sys::GDExtensionInterfaceClassdbRegisterExtensionClassMethod
            ),
            classdb_register_extension_class_property: load!(
                "classdb_register_extension_class_property",
                sys::GDExtensionInterfaceClassdbRegisterExtensionClassProperty
            ),
            classdb_register_extension_class_property_group: load!(
                "classdb_register_extension_class_property_group",
                sys::GDExtensionInterfaceClassdbRegisterExtensionClassPropertyGroup
            ),
            classdb_register_extension_class_property_subgroup: load!(
                "classdb_register_extension_class_property_subgroup",
                sys::GDExtensionInterfaceClassdbRegisterExtensionClassPropertySubgroup
            ),
            classdb_register_extension_class_signal: load!(
                "classdb_register_extension_class_signal",
                sys::GDExtensionInterfaceClassdbRegisterExtensionClassSignal
            ),
            classdb_register_extension_class,
            object_set_instance: load!(
                "object_set_instance",
                sys::GDExtensionInterfaceObjectSetInstance
            ),
            object_set_instance_binding: load!(
                "object_set_instance_binding",
                sys::GDExtensionInterfaceObjectSetInstanceBinding
            ),
            object_destroy: load!("object_destroy", sys::GDExtensionInterfaceObjectDestroy),
            object_cast_to: load!("object_cast_to", sys::GDExtensionInterfaceObjectCastTo),
            object_get_instance_from_id: load!(
                "object_get_instance_from_id",
                sys::GDExtensionInterfaceObjectGetInstanceFromId
            ),
            object_get_instance_id: load!(
                "object_get_instance_id",
                sys::GDExtensionInterfaceObjectGetInstanceId
            ),
            object_method_bind_ptrcall: load!(
                "object_method_bind_ptrcall",
                sys::GDExtensionInterfaceObjectMethodBindPtrcall
            ),
            object_method_bind_call: load!(
                "object_method_bind_call",
                sys::GDExtensionInterfaceObjectMethodBindCall
            ),
            variant_new_copy: load!("variant_new_copy", sys::GDExtensionInterfaceVariantNewCopy),
            variant_new_nil: load!("variant_new_nil", sys::GDExtensionInterfaceVariantNewNil),
            variant_destroy: load!("variant_destroy", sys::GDExtensionInterfaceVariantDestroy),
            get_variant_from_type_constructor,
            get_variant_to_type_constructor,
            variant_get_ptr_constructor,
            variant_get_ptr_builtin_method: load!(
                "variant_get_ptr_builtin_method",
                sys::GDExtensionInterfaceVariantGetPtrBuiltinMethod
            ),
            variant_get_ptr_operator_evaluator: load!(
                "variant_get_ptr_operator_evaluator",
                sys::GDExtensionInterfaceVariantGetPtrOperatorEvaluator
            ),
            variant_get_ptr_setter: load!(
                "variant_get_ptr_setter",
                sys::GDExtensionInterfaceVariantGetPtrSetter
            ),
            variant_get_ptr_getter: load!(
                "variant_get_ptr_getter",
                sys::GDExtensionInterfaceVariantGetPtrGetter
            ),
            variant_get_ptr_indexed_setter: load!(
                "variant_get_ptr_indexed_setter",
                sys::GDExtensionInterfaceVariantGetPtrIndexedSetter
            ),
            variant_get_ptr_indexed_getter: load!(
                "variant_get_ptr_indexed_getter",
                sys::GDExtensionInterfaceVariantGetPtrIndexedGetter
            ),
            variant_get_ptr_keyed_setter: load!(
                "variant_get_ptr_keyed_setter",
                sys::GDExtensionInterfaceVariantGetPtrKeyedSetter
            ),
            variant_get_ptr_keyed_getter: load!(
                "variant_get_ptr_keyed_getter",
                sys::GDExtensionInterfaceVariantGetPtrKeyedGetter
            ),
            variant_get_constant_value: load!(
                "variant_get_constant_value",
                sys::GDExtensionInterfaceVariantGetConstantValue
            ),
            variant_get_ptr_utility_function: load!(
                "variant_get_ptr_utility_function",
                sys::GDExtensionInterfaceVariantGetPtrUtilityFunction
            ),
            variant_get_ptr_destructor,
            global_get_singleton: load!(
                "global_get_singleton",
                sys::GDExtensionInterfaceGlobalGetSingleton
            ),
            ref_get_object: load!("ref_get_object", sys::GDExtensionInterfaceRefGetObject),
            ref_set_object: load!("ref_set_object", sys::GDExtensionInterfaceRefSetObject),
            variant_get_type: load!("variant_get_type", sys::GDExtensionInterfaceVariantGetType),
            variant_get_object_instance_id: load!(
                "variant_get_object_instance_id",
                sys::GDExtensionInterfaceVariantGetObjectInstanceId
            ),
            array_operator_index: load!(
                "array_operator_index",
                sys::GDExtensionInterfaceArrayOperatorIndex
            ),
            array_operator_index_const: load!(
                "array_operator_index_const",
                sys::GDExtensionInterfaceArrayOperatorIndexConst
            ),
            array_set_typed: load!("array_set_typed", sys::GDExtensionInterfaceArraySetTyped),
            dictionary_operator_index: load!(
                "dictionary_operator_index",
                sys::GDExtensionInterfaceDictionaryOperatorIndex
            ),
            dictionary_operator_index_const: load!(
                "dictionary_operator_index_const",
                sys::GDExtensionInterfaceDictionaryOperatorIndexConst
            ),
            variant_from_bool: to_variant!(GDEXTENSION_VARIANT_TYPE_BOOL, "bool"),
            bool_from_variant: from_variant!(GDEXTENSION_VARIANT_TYPE_BOOL, "bool"),
            variant_from_int: to_variant!(GDEXTENSION_VARIANT_TYPE_INT, "int"),
            int_from_variant: from_variant!(GDEXTENSION_VARIANT_TYPE_INT, "int"),
            variant_from_float: to_variant!(GDEXTENSION_VARIANT_TYPE_FLOAT, "float"),
            float_from_variant: from_variant!(GDEXTENSION_VARIANT_TYPE_FLOAT, "float"),
            variant_from_string: to_variant!(GDEXTENSION_VARIANT_TYPE_STRING, "String"),
            string_from_variant: from_variant!(GDEXTENSION_VARIANT_TYPE_STRING, "String"),
            print_error: load!("print_error", sys::GDExtensionInterfacePrintError),
            postinitialize_method: ptr::null(),
        };

        let notification_hash = method_hash("Object", "notification")?;
        let postinitialize_method = {
            let class_name = GodotStringName::new(&interface, "Object")?;
            let method_name = GodotStringName::new(&interface, "notification")?;
            // SAFETY: Both StringNames are alive for the call and the hash
            // comes from the selected authenticated official API Snapshot.
            unsafe {
                (interface.classdb_get_method_bind)(
                    class_name.as_ptr(),
                    method_name.as_ptr(),
                    notification_hash,
                )
            }
        };
        interface.postinitialize_method = postinitialize_method;
        if interface.postinitialize_method.is_null() {
            return Err(NativeError::new(
                "Godot did not return Object.notification MethodBind",
            ));
        }

        Ok(interface)
    }

    pub fn postinitialize(&self, object: sys::GDExtensionObjectPtr) {
        let notification = 0_i64;
        let reversed = 0_u8;
        let arguments = [
            (&notification as *const i64).cast::<c_void>(),
            (&reversed as *const u8).cast::<c_void>(),
        ];
        // SAFETY: The MethodBind and argument types match the selected
        // Object.notification(int, bool) metadata.
        unsafe {
            (self.object_method_bind_ptrcall)(
                self.postinitialize_method,
                object,
                arguments.as_ptr(),
                ptr::null_mut(),
            );
        }
    }

    pub fn report_error(&self, description: &str, function: &str) {
        let description = c_string(description);
        let function = c_string(function);
        // SAFETY: Every CString remains alive for the duration of the call.
        unsafe {
            (self.print_error)(
                description.as_ptr(),
                function.as_ptr(),
                c"godot_rs".as_ptr(),
                0,
                1,
            );
        }
        eprintln!("godot-rust Native: {}", description.to_string_lossy());
    }
}

fn method_hash(class: &str, method: &str) -> Result<i64, NativeError> {
    let hash = api_snapshot::ENGINE_METHODS
        .iter()
        .find(|candidate| candidate.class == class && candidate.name == method)
        .and_then(|candidate| candidate.hash)
        .ok_or_else(|| {
            NativeError::new(format!(
                "selected Godot {GODOT_API} API has no hash for {class}.{method}"
            ))
        })?;
    i64::try_from(hash).map_err(|_| {
        NativeError::new(format!(
            "selected Godot {GODOT_API} method hash does not fit the official ABI"
        ))
    })
}

fn validate_engine_version(major: u32, minor: u32) -> NativeResult {
    let mut selected = GODOT_API.split('.');
    let selected_major = selected
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .expect("build script emits a validated major");
    let selected_minor = selected
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .expect("build script emits a validated minor");
    if major != selected_major || minor < selected_minor {
        return Err(NativeError::new(format!(
            "this library targets Godot {GODOT_API}, but Godot {major}.{minor} loaded it"
        )));
    }
    Ok(())
}

fn c_str(value: &'static str) -> &'static CStr {
    CStr::from_bytes_with_nul(value.as_bytes())
        .expect("internal interface names include a trailing NUL")
}

fn c_string(value: &str) -> CString {
    CString::new(value.replace('\0', "\u{fffd}")).expect("replacement removes every interior NUL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_snapshot_supplies_the_core_notification_hash() {
        assert!(method_hash("Object", "notification").is_ok());
    }

    #[test]
    fn engine_version_rejects_older_or_different_major_versions() {
        let selected_minor = GODOT_API
            .strip_prefix("4.")
            .and_then(|value| value.parse::<u32>().ok())
            .expect("test target uses a supported Godot 4 API");
        assert!(validate_engine_version(4, selected_minor).is_ok());
        assert!(validate_engine_version(4, selected_minor + 1).is_ok());
        assert!(validate_engine_version(4, selected_minor - 1).is_err());
        assert!(validate_engine_version(5, 0).is_err());
        assert!(validate_engine_version(3, 9).is_err());
    }
}
