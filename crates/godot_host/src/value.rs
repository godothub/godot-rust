use core::ffi::{CStr, c_char};
use core::fmt;
use core::ptr;
use godot_api::{
    GDExtensionConstStringPtr, GDExtensionConstTypePtr, GDExtensionTypePtr, GDExtensionVariantType,
};

use crate::interface::EngineInterface;

const MAX_HOST_TEXT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextDecodeError {
    NullString,
    ConversionUnavailable,
    NegativeLength(i64),
    TooLarge(i64),
    InvalidUtf8,
}

impl fmt::Display for TextDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullString => formatter.write_str("Godot supplied a null String"),
            Self::ConversionUnavailable => {
                formatter.write_str("Godot String conversion is unavailable")
            }
            Self::NegativeLength(length) => {
                write!(
                    formatter,
                    "Godot reported a negative UTF-8 length: {length}"
                )
            }
            Self::TooLarge(length) => write!(
                formatter,
                "Godot String is too large for Host decoding: {length} bytes"
            ),
            Self::InvalidUtf8 => formatter.write_str("Godot String contains invalid UTF-8"),
        }
    }
}

pub(crate) fn write_latin1_string(
    interface: EngineInterface,
    result: GDExtensionTypePtr,
    value: &'static CStr,
) {
    let Some(constructor) = interface.string_new_with_latin1_chars else {
        return;
    };
    if result.is_null() {
        return;
    }
    // SAFETY: Godot supplies uninitialized String storage and the source is a
    // valid nul-terminated Latin-1/ASCII string.
    unsafe { constructor(result, value.as_ptr()) };
}

pub(crate) fn write_utf8_string(
    interface: EngineInterface,
    result: GDExtensionTypePtr,
    value: &str,
) -> bool {
    let Some(constructor) = interface.string_new_with_utf8_chars_and_len2 else {
        return false;
    };
    if result.is_null() {
        return false;
    }
    let Ok(length) = i64::try_from(value.len()) else {
        return false;
    };
    // SAFETY: The source is valid UTF-8 for exactly `length` bytes and Godot
    // supplies correctly sized uninitialized String storage.
    unsafe { constructor(result, value.as_ptr().cast(), length) == 0 }
}

pub(crate) fn read_utf8_string(
    interface: EngineInterface,
    value: GDExtensionConstStringPtr,
) -> Result<String, TextDecodeError> {
    if value.is_null() {
        return Err(TextDecodeError::NullString);
    }
    let converter = interface
        .string_to_utf8_chars
        .expect("required String UTF-8 converter was resolved");
    // SAFETY: `value` points to an initialized Godot String and a null output
    // buffer requests only the encoded length.
    let length = unsafe { converter(value, ptr::null_mut(), 0) };
    if length < 0 {
        return Err(TextDecodeError::NegativeLength(length));
    }
    let length = usize::try_from(length).map_err(|_| TextDecodeError::TooLarge(length))?;
    if length > MAX_HOST_TEXT_BYTES {
        return Err(TextDecodeError::TooLarge(
            i64::try_from(length).unwrap_or(i64::MAX),
        ));
    }

    let mut bytes = vec![0_u8; length];
    if length != 0 {
        // SAFETY: The buffer has exactly `length` writable bytes and the first
        // query established the required encoded size.
        let written = unsafe {
            converter(
                value,
                bytes.as_mut_ptr().cast::<c_char>(),
                i64::try_from(length).expect("bounded text length fits i64"),
            )
        };
        if written != i64::try_from(length).expect("bounded text length fits i64") {
            return Err(TextDecodeError::TooLarge(written));
        }
    }
    String::from_utf8(bytes).map_err(|_| TextDecodeError::InvalidUtf8)
}

pub(crate) fn write_string_name(
    interface: EngineInterface,
    result: GDExtensionTypePtr,
    value: &'static CStr,
) {
    let Some(constructor) = interface.string_name_new_with_latin1_chars else {
        return;
    };
    if result.is_null() {
        return;
    }
    // SAFETY: Setting `p_is_static` false makes a regular owned StringName that
    // Godot may safely destroy after consuming the virtual return value.
    unsafe { constructor(result, value.as_ptr(), 0) };
}

pub(crate) fn write_utf8_string_name(
    interface: EngineInterface,
    result: GDExtensionTypePtr,
    value: &str,
) -> bool {
    let Some(constructor) = interface.string_name_new_with_utf8_chars_and_len else {
        return false;
    };
    if result.is_null() {
        return false;
    }
    let Ok(length) = i64::try_from(value.len()) else {
        return false;
    };
    // SAFETY: The source is valid UTF-8 for exactly `length` bytes and Godot
    // supplies correctly sized uninitialized StringName storage.
    unsafe {
        constructor(result, value.as_ptr().cast(), length);
    }
    true
}

pub(crate) fn write_default_builtin(
    interface: EngineInterface,
    result: GDExtensionTypePtr,
    type_: GDExtensionVariantType,
) {
    let Some(get_constructor) = interface.variant_get_ptr_constructor else {
        return;
    };
    if result.is_null() {
        return;
    }
    // SAFETY: Constructor zero is the official default constructor.
    let Some(constructor) = (unsafe { get_constructor(type_, 0) }) else {
        return;
    };
    // SAFETY: Godot supplies correctly sized uninitialized return storage and
    // a default constructor takes no arguments.
    unsafe { constructor(result, ptr::null()) };
}

pub(crate) fn write_nil_variant(interface: EngineInterface, result: GDExtensionTypePtr) {
    let Some(constructor) = interface.variant_new_nil else {
        return;
    };
    if result.is_null() {
        return;
    }
    // SAFETY: Godot supplies uninitialized Variant return storage.
    unsafe { constructor(result) };
}

pub(crate) struct LocalGodotString {
    interface: EngineInterface,
    storage: usize,
}

impl LocalGodotString {
    pub(crate) const fn uninitialized(interface: EngineInterface) -> Self {
        Self {
            interface,
            storage: 0,
        }
    }

    pub(crate) fn empty(interface: EngineInterface) -> Option<Self> {
        let get_constructor = interface.variant_get_ptr_constructor?;
        let constructor = {
            // SAFETY: Constructor zero is the official String default constructor.
            unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING, 0) }
        }?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: Storage is exactly one pointer wide in every supported
        // official build and the default constructor takes no arguments.
        unsafe { constructor(result.as_mut_ptr(), ptr::null()) };
        Some(result)
    }

    pub(crate) fn new(interface: EngineInterface, value: &'static CStr) -> Option<Self> {
        let constructor = interface.string_new_with_latin1_chars?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: String is one pointer wide in every supported official build
        // configuration and the source is valid Latin-1/ASCII.
        unsafe {
            constructor((&mut result.storage as *mut usize).cast(), value.as_ptr());
        }
        Some(result)
    }

    pub(crate) fn new_utf8(interface: EngineInterface, value: &str) -> Option<Self> {
        let constructor = interface.string_new_with_utf8_chars_and_len2?;
        let length = i64::try_from(value.len()).ok()?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: The source contains exactly `length` valid UTF-8 bytes and
        // String is one pointer wide in every supported official build.
        let error = unsafe {
            constructor(
                (&mut result.storage as *mut usize).cast(),
                value.as_ptr().cast(),
                length,
            )
        };
        (error == 0).then_some(result)
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstTypePtr {
        (&self.storage as *const usize).cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        (&mut self.storage as *mut usize).cast()
    }

    pub(crate) fn to_utf8(&self) -> Result<String, TextDecodeError> {
        read_utf8_string(self.interface, self.as_ptr())
    }
}

impl Drop for LocalGodotString {
    fn drop(&mut self) {
        let Some(get_destructor) = self.interface.variant_get_ptr_destructor else {
            return;
        };
        // SAFETY: String has an official destructor for every supported build.
        let Some(destructor) =
            (unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING) })
        else {
            return;
        };
        // SAFETY: The storage contains the initialized String owned here.
        unsafe { destructor((&mut self.storage as *mut usize).cast()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_errors_are_actionable() {
        assert_eq!(
            TextDecodeError::NegativeLength(-1).to_string(),
            "Godot reported a negative UTF-8 length: -1"
        );
        assert_eq!(
            TextDecodeError::TooLarge(70_000_000).to_string(),
            "Godot String is too large for Host decoding: 70000000 bytes"
        );
    }
}
