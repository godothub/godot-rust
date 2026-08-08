use core::ffi::{CStr, c_void};
use core::ptr;
use godot_rs_api::{
    GDExtensionConstStringNamePtr, GDExtensionPtrDestructor, GDExtensionVariantOperator,
    GDExtensionVariantType,
};

use crate::interface::EngineInterface;
use crate::value::{LocalGodotString, TextDecodeError};

/// An owned Godot `StringName` constructed from a static Latin-1 string.
///
/// The source bytes live in this extension library, not for the entire engine
/// process. Godot may unload the library before its global StringName table, so
/// the constructor must copy the bytes and this wrapper must release its
/// reference before the extension is unloaded.
#[derive(Debug)]
pub(crate) struct StaticStringName {
    storage: usize,
    destroy: GDExtensionPtrDestructor,
}

impl StaticStringName {
    pub(crate) fn new(interface: EngineInterface, value: &'static CStr) -> Self {
        let mut result = Self {
            storage: 0,
            destroy: interface
                .variant_get_ptr_destructor
                .and_then(|get_destructor| {
                    // SAFETY: StringName is an official Variant builtin type.
                    unsafe {
                        get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME)
                    }
                }),
        };
        let constructor = interface
            .string_name_new_with_latin1_chars
            .expect("required StringName constructor was resolved");
        // SAFETY: `value` is nul-terminated Latin-1/ASCII and the destination
        // is exactly one pointer wide. Passing false makes Godot copy the
        // bytes, so the value remains valid if this library is later unloaded.
        unsafe {
            constructor(
                (&mut result.storage as *mut usize).cast(),
                value.as_ptr(),
                0,
            );
        }
        result
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstStringNamePtr {
        (&self.storage as *const usize).cast()
    }

    pub(crate) fn equals(
        &self,
        interface: EngineInterface,
        other: GDExtensionConstStringNamePtr,
    ) -> bool {
        string_names_equal(interface, self.as_ptr(), other)
    }
}

impl Drop for StaticStringName {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            // SAFETY: `new` initialized this owned StringName and this Drop
            // releases it exactly once while the engine interface is live.
            unsafe { destroy((&mut self.storage as *mut usize).cast()) };
        }
    }
}

/// An owned UTF-8 Godot `StringName` used for generated reflected methods.
pub(crate) struct OwnedStringName {
    interface: EngineInterface,
    storage: usize,
}

impl OwnedStringName {
    pub(crate) fn empty(interface: EngineInterface) -> Option<Self> {
        let get_constructor = interface.variant_get_ptr_constructor?;
        // SAFETY: Constructor zero is the official StringName default constructor.
        let constructor = unsafe {
            get_constructor(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
                0,
            )
        }?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: StringName is one pointer wide and its default constructor
        // takes no arguments.
        unsafe { constructor(result.as_mut_ptr(), ptr::null()) };
        Some(result)
    }

    pub(crate) fn new(interface: EngineInterface, value: &str) -> Option<Self> {
        let constructor = interface.string_name_new_with_utf8_chars_and_len?;
        let length = i64::try_from(value.len()).ok()?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: The source is valid UTF-8, length is exact, and storage is
        // the official pointer-sized StringName representation.
        unsafe {
            constructor(
                (&mut result.storage as *mut usize).cast(),
                value.as_ptr().cast(),
                length,
            );
        }
        Some(result)
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstStringNamePtr {
        (&self.storage as *const usize).cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut c_void {
        (&mut self.storage as *mut usize).cast()
    }

    pub(crate) fn equals(&self, other: GDExtensionConstStringNamePtr) -> bool {
        string_names_equal(self.interface, self.as_ptr(), other)
    }

    pub(crate) fn to_utf8(&self) -> Result<String, TextDecodeError> {
        read_utf8_string_name(self.interface, self.as_ptr())
    }
}

impl Drop for OwnedStringName {
    fn drop(&mut self) {
        let Some(get_destructor) = self.interface.variant_get_ptr_destructor else {
            return;
        };
        // SAFETY: StringName is an official Variant builtin type.
        let Some(destroy) = (unsafe {
            get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME)
        }) else {
            return;
        };
        // SAFETY: This wrapper owns one initialized, non-static StringName.
        unsafe { destroy((&mut self.storage as *mut usize).cast()) };
    }
}

pub(crate) fn read_utf8_string_name(
    interface: EngineInterface,
    value: GDExtensionConstStringNamePtr,
) -> Result<String, TextDecodeError> {
    if value.is_null() {
        return Err(TextDecodeError::NullString);
    }
    let get_constructor = interface
        .variant_get_ptr_constructor
        .ok_or(TextDecodeError::ConversionUnavailable)?;
    let constructor = {
        // SAFETY: String constructor two is the official StringName-to-String
        // conversion constructor.
        unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING, 2) }
    }
    .ok_or(TextDecodeError::ConversionUnavailable)?;
    let arguments = [value.cast()];
    let mut string = LocalGodotString::uninitialized(interface);
    // SAFETY: The destination is uninitialized String storage and `value`
    // points to a live, initialized StringName for this synchronous call.
    unsafe { constructor(string.as_mut_ptr(), arguments.as_ptr()) };
    string.to_utf8()
}

fn string_names_equal(
    interface: EngineInterface,
    left: GDExtensionConstStringNamePtr,
    right: GDExtensionConstStringNamePtr,
) -> bool {
    if left.is_null() || right.is_null() {
        return false;
    }

    let get_evaluator = interface
        .variant_get_ptr_operator_evaluator
        .expect("required operator resolver was loaded");
    // SAFETY: The requested operator and operand types come directly from
    // the official Variant ABI.
    let Some(evaluate) = (unsafe {
        get_evaluator(
            GDExtensionVariantOperator::GDEXTENSION_VARIANT_OP_EQUAL,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING_NAME,
        )
    }) else {
        return false;
    };

    let mut result = 0_u8;
    // SAFETY: Both operands point to initialized StringNames and the result
    // points to the official one-byte GDExtensionBool representation.
    unsafe {
        evaluate(
            left.cast::<c_void>(),
            right.cast::<c_void>(),
            (&mut result as *mut u8).cast(),
        );
    }
    result != 0
}
