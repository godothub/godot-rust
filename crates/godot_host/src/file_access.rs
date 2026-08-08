use godot_api::{GDExtensionConstTypePtr, GDExtensionMethodBindPtr};

use crate::interface::EngineInterface;
use crate::runtime::resolve_method;
use crate::value::LocalGodotString;

pub(crate) fn exists(
    interface: EngineInterface,
    path: GDExtensionConstTypePtr,
) -> Result<bool, String> {
    if path.is_null() {
        return Ok(false);
    }
    let file_exists = resolve_method(interface, c"FileAccess", c"file_exists", 2_323_990_056)
        .map_err(|error| error.to_string())?;
    let ptrcall = interface
        .object_method_bind_ptrcall
        .ok_or_else(|| "Godot did not expose method bind ptrcall".to_owned())?;
    let arguments = [path];
    let mut result = 0_u8;
    // SAFETY: FileAccess.file_exists is an official static method with one
    // String argument and a bool return value.
    unsafe {
        ptrcall(
            file_exists as GDExtensionMethodBindPtr,
            std::ptr::null_mut(),
            arguments.as_ptr(),
            (&mut result as *mut u8).cast(),
        );
    }
    Ok(result != 0)
}

pub(crate) fn read_text(
    interface: EngineInterface,
    path: GDExtensionConstTypePtr,
) -> Result<String, String> {
    if path.is_null() {
        return Err("Godot resource path is null".to_owned());
    }
    let get_file_as_string = resolve_method(
        interface,
        c"FileAccess",
        c"get_file_as_string",
        1_703_090_593,
    )
    .map_err(|error| error.to_string())?;
    let ptrcall = interface
        .object_method_bind_ptrcall
        .ok_or_else(|| "Godot did not expose method bind ptrcall".to_owned())?;
    let arguments = [path];
    let mut result = LocalGodotString::new(interface, c"")
        .ok_or_else(|| "could not create Godot file content String".to_owned())?;
    // SAFETY: FileAccess.get_file_as_string is an official static method with
    // one String argument and a String return value.
    unsafe {
        ptrcall(
            get_file_as_string as GDExtensionMethodBindPtr,
            std::ptr::null_mut(),
            arguments.as_ptr(),
            result.as_mut_ptr(),
        );
    }
    result
        .to_utf8()
        .map_err(|error| format!("could not decode Godot resource as UTF-8: {error}"))
}
