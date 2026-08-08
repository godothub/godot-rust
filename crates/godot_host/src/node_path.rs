use core::ffi::c_void;
use core::ptr;

use godot_api::{GDExtensionConstTypePtr, GDExtensionTypePtr, GDExtensionVariantType};

use crate::interface::EngineInterface;
use crate::value::{LocalGodotString, TextDecodeError};

/// One owned native Godot `NodePath` used only around ptrcall.
pub(crate) struct OwnedNodePath {
    interface: EngineInterface,
    storage: usize,
}

impl OwnedNodePath {
    pub(crate) fn empty(interface: EngineInterface) -> Option<Self> {
        let get_constructor = interface.variant_get_ptr_constructor?;
        // SAFETY: Constructor zero is the official NodePath default
        // constructor in every supported API target.
        let constructor = unsafe {
            get_constructor(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH,
                0,
            )
        }?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: NodePath is one pointer wide for the current architecture in
        // every authenticated official build configuration, and its default
        // constructor has no arguments.
        unsafe { constructor(result.as_mut_ptr(), ptr::null()) };
        Some(result)
    }

    pub(crate) fn new(interface: EngineInterface, value: &str) -> Option<Self> {
        let string = LocalGodotString::new_utf8(interface, value)?;
        let get_constructor = interface.variant_get_ptr_constructor?;
        // SAFETY: Constructor two is the authenticated String-to-NodePath
        // conversion constructor in every supported API target.
        let constructor = unsafe {
            get_constructor(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH,
                2,
            )
        }?;
        let arguments = [string.as_ptr()];
        let mut result = Self {
            interface,
            storage: 0,
        };
        // SAFETY: The destination is uninitialized NodePath storage and the
        // source String remains live for this constructor call.
        unsafe { constructor(result.as_mut_ptr(), arguments.as_ptr()) };
        Some(result)
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstTypePtr {
        (&self.storage as *const usize).cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        (&mut self.storage as *mut usize).cast()
    }

    pub(crate) fn to_utf8(&self) -> Result<String, TextDecodeError> {
        let get_constructor = self
            .interface
            .variant_get_ptr_constructor
            .ok_or(TextDecodeError::ConversionUnavailable)?;
        let constructor = {
            // SAFETY: String constructor three is the authenticated
            // NodePath-to-String conversion constructor.
            unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING, 3) }
        }
        .ok_or(TextDecodeError::ConversionUnavailable)?;
        let arguments = [self.as_ptr()];
        let mut string = LocalGodotString::uninitialized(self.interface);
        // SAFETY: The destination is uninitialized String storage and this
        // owned NodePath stays live for the synchronous conversion.
        unsafe { constructor(string.as_mut_ptr(), arguments.as_ptr()) };
        string.to_utf8()
    }
}

impl Drop for OwnedNodePath {
    fn drop(&mut self) {
        let Some(get_destructor) = self.interface.variant_get_ptr_destructor else {
            return;
        };
        // SAFETY: NodePath is an official Variant builtin type.
        let Some(destroy) =
            (unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH) })
        else {
            return;
        };
        // SAFETY: This wrapper owns one initialized NodePath and releases it
        // exactly once while the engine interface remains live.
        unsafe { destroy((&mut self.storage as *mut usize).cast::<c_void>()) };
    }
}

pub(crate) fn read_utf8_node_path(
    interface: EngineInterface,
    value: GDExtensionConstTypePtr,
) -> Result<String, TextDecodeError> {
    if value.is_null() {
        return Err(TextDecodeError::ConversionUnavailable);
    }
    let get_constructor = interface
        .variant_get_ptr_constructor
        .ok_or(TextDecodeError::ConversionUnavailable)?;
    let constructor = {
        // SAFETY: String constructor three is the authenticated
        // NodePath-to-String conversion constructor.
        unsafe { get_constructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_STRING, 3) }
    }
    .ok_or(TextDecodeError::ConversionUnavailable)?;
    let arguments = [value];
    let mut string = LocalGodotString::uninitialized(interface);
    // SAFETY: Destination storage is uninitialized and the borrowed NodePath
    // remains live for this synchronous conversion.
    unsafe { constructor(string.as_mut_ptr(), arguments.as_ptr()) };
    string.to_utf8()
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_storage_matches_authenticated_layouts_for_this_architecture() {
        let architecture = if usize::BITS == 64 { "_64" } else { "_32" };
        let sizes = godot_api::api_snapshot::BUILTIN_SIZES
            .iter()
            .filter(|summary| {
                summary.name == "NodePath" && summary.configuration.ends_with(architecture)
            })
            .collect::<Vec<_>>();
        assert_eq!(sizes.len(), 2);
        for summary in sizes {
            assert_eq!(
                summary.size as usize,
                core::mem::size_of::<usize>(),
                "{} reports an incompatible NodePath size",
                summary.configuration
            );
        }
    }
}
