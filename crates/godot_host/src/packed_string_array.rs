use godot_api::{
    GDExtensionConstTypePtr, GDExtensionPtrBuiltInMethod, GDExtensionTypePtr,
    GDExtensionVariantType,
};

use crate::interface::EngineInterface;
use crate::string_name::StaticStringName;
use crate::value::{LocalGodotString, read_utf8_string, write_default_builtin};

const PUSH_BACK_HASH: i64 = 816_187_996;
const SIZE_HASH: i64 = 3_173_160_232;
const MAX_READ_VALUES: usize = 256;

/// Official `PackedStringArray` operations shared by Host editor and resource
/// callbacks.
#[derive(Clone, Copy)]
pub(crate) struct PackedStringArrayWriter {
    interface: EngineInterface,
    push_back: GDExtensionPtrBuiltInMethod,
}

impl PackedStringArrayWriter {
    pub(crate) fn new(interface: EngineInterface) -> Option<Self> {
        let get_method = interface.variant_get_ptr_builtin_method?;
        let method_name = StaticStringName::new(interface, c"push_back");
        // SAFETY: The type, method, and hash come from the authenticated
        // official Godot 4.4 extension API.
        let push_back = unsafe {
            get_method(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
                method_name.as_ptr(),
                PUSH_BACK_HASH,
            )
        };
        push_back?;
        Some(Self {
            interface,
            push_back,
        })
    }

    pub(crate) fn write_empty(self, result: GDExtensionTypePtr) {
        write_default_builtin(
            self.interface,
            result,
            GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
        );
    }

    pub(crate) fn write(self, result: GDExtensionTypePtr, values: &[&str]) -> bool {
        self.write_empty(result);
        if result.is_null() {
            return false;
        }
        let Some(push_back) = self.push_back else {
            return false;
        };
        for value in values {
            let Some(value) = LocalGodotString::new_utf8(self.interface, value) else {
                return false;
            };
            let arguments = [value.as_ptr()];
            let mut failed = 0_u8;
            // SAFETY: The output is an initialized PackedStringArray, the
            // argument is an initialized String, and the official method
            // returns one ptrcall bool.
            unsafe {
                push_back(
                    result,
                    arguments.as_ptr(),
                    (&mut failed as *mut u8).cast(),
                    1,
                );
            }
            if failed != 0 {
                return false;
            }
        }
        true
    }
}

/// Bounded reader for PackedStringArray values received from Godot editor
/// callbacks.
#[derive(Clone, Copy)]
pub(crate) struct PackedStringArrayReader {
    interface: EngineInterface,
    size: GDExtensionPtrBuiltInMethod,
}

impl PackedStringArrayReader {
    pub(crate) fn new(interface: EngineInterface) -> Option<Self> {
        let get_method = interface.variant_get_ptr_builtin_method?;
        let method_name = StaticStringName::new(interface, c"size");
        // SAFETY: The type, method and hash come from the authenticated Godot
        // 4.4 extension API.
        let size = unsafe {
            get_method(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_PACKED_STRING_ARRAY,
                method_name.as_ptr(),
                SIZE_HASH,
            )
        };
        size?;
        interface.packed_string_array_operator_index_const?;
        Some(Self { interface, size })
    }

    pub(crate) fn read(self, value: GDExtensionConstTypePtr) -> Option<Vec<String>> {
        if value.is_null() {
            return None;
        }
        let size = self.size?;
        let mut count = 0_i64;
        // SAFETY: `value` is an initialized PackedStringArray from the
        // official virtual call and `size` is its zero-argument const method.
        unsafe {
            size(
                value.cast_mut(),
                core::ptr::null(),
                (&mut count as *mut i64).cast(),
                0,
            )
        };
        let count = usize::try_from(count).ok()?;
        if count > MAX_READ_VALUES {
            return None;
        }
        let index = self.interface.packed_string_array_operator_index_const?;
        let mut result = Vec::with_capacity(count);
        for position in 0..count {
            let position = i64::try_from(position).ok()?;
            // SAFETY: The index is strictly below the size read from the same
            // live PackedStringArray.
            let string = unsafe { index(value, position) };
            if string.is_null() {
                return None;
            }
            result.push(read_utf8_string(self.interface, string).ok()?);
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_back_hash_matches_authenticated_godot_4_4_api() {
        assert_eq!(PUSH_BACK_HASH, 816_187_996);
        assert_eq!(SIZE_HASH, 3_173_160_232);
        assert_eq!(MAX_READ_VALUES, 256);
    }
}
