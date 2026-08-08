use core::ffi::c_void;
use core::ptr;

use godot_rs_api::abi::{AbiStatus, AbiValueV1};
use godot_rs_api::{GDExtensionConstTypePtr, GDExtensionObjectPtr, GDExtensionVariantType};

use crate::interface::EngineInterface;
use crate::module_loader::{ModuleCallError, ModuleField, ModuleState};
use crate::string_name::OwnedStringName;
use crate::value::LocalGodotString;

const NODE_GET_NODE_OR_NULL_HASH: i64 = 2_734_337_346;
const NODE_PATH_FROM_STRING_CONSTRUCTOR: i32 = 2;

pub(crate) fn inject_node_fields(
    interface: EngineInterface,
    owner: GDExtensionObjectPtr,
    state: &mut ModuleState,
) -> Result<(), ModuleCallError> {
    let fields = (0..state.field_count())
        .filter_map(|index| state.field(index))
        .filter(ModuleField::is_node)
        .collect::<Vec<_>>();
    if fields.is_empty() {
        return Ok(());
    }

    let node_tag = class_tag(interface, "Node")?;
    let cast_to = interface.object_cast_to.ok_or_else(|| {
        internal_error("Godot object class validation is unavailable for node fields")
    })?;
    // SAFETY: The owner is a live Object supplied by Godot and the class tag
    // came from ClassDB in the same engine process.
    let node_owner = unsafe { cast_to(owner, node_tag) };
    if node_owner.is_null() {
        return Err(field_error(
            "Rust scripts with `#[node]` fields must inherit from Godot Node",
        ));
    }

    let method = crate::runtime::resolve_method(
        interface,
        c"Node",
        c"get_node_or_null",
        NODE_GET_NODE_OR_NULL_HASH,
    )
    .map_err(|error| internal_error(error.to_string()))?;
    let ptrcall = interface.object_method_bind_ptrcall.ok_or_else(|| {
        internal_error("Godot ptrcall is unavailable while resolving node fields")
    })?;

    for field in fields {
        let path = field
            .node_path()
            .expect("filtered node field has a validated path")
            .to_owned();
        let class_name = field
            .node_class_name()
            .expect("filtered node field has a validated class")
            .to_owned();
        let optional = field
            .node_optional()
            .expect("filtered node field has validated optionality");
        let node_path = OwnedNodePath::new(interface, &path)?;
        let arguments = [node_path.as_ptr()];
        let mut resolved: GDExtensionObjectPtr = ptr::null_mut();
        // SAFETY: MethodBind and NodePath constructor metadata come from the
        // official Godot 4.4 API. `resolved` is the pointer-sized Object return
        // slot required by this exact ptrcall signature.
        unsafe {
            ptrcall(
                method,
                node_owner,
                arguments.as_ptr(),
                ptr::from_mut(&mut resolved).cast(),
            );
        }

        let instance_id = validate_resolved_node(
            interface,
            resolved,
            field.name(),
            &path,
            &class_name,
            optional,
        )?;
        state.set_field(&field, AbiValueV1::from_object_id(instance_id))?;
    }
    Ok(())
}

fn validate_resolved_node(
    interface: EngineInterface,
    resolved: GDExtensionObjectPtr,
    field_name: &str,
    path: &str,
    class_name: &str,
    optional: bool,
) -> Result<u64, ModuleCallError> {
    if resolved.is_null() {
        if optional {
            return Ok(0);
        }
        return Err(field_error(format!(
            "required node field `{field_name}` could not find `{path}`"
        )));
    }

    let class_tag = class_tag(interface, class_name)?;
    let cast_to = interface.object_cast_to.ok_or_else(|| {
        internal_error("Godot object class validation is unavailable for node fields")
    })?;
    // SAFETY: `resolved` was returned by Node.get_node_or_null and the class
    // tag belongs to the current ClassDB.
    if unsafe { cast_to(resolved, class_tag) }.is_null() {
        return Err(field_error(format!(
            "node field `{field_name}` resolved `{path}`, but the node is not a `{class_name}`"
        )));
    }

    let get_instance_id = interface
        .object_get_instance_id
        .ok_or_else(|| internal_error("Godot object identity is unavailable for node fields"))?;
    // SAFETY: The resolved pointer is a live Node returned synchronously by
    // Godot and its expected class was just validated.
    let instance_id = unsafe { get_instance_id(resolved) };
    if instance_id == 0 {
        return Err(internal_error(format!(
            "node field `{field_name}` resolved `{path}` without an instance ID"
        )));
    }
    Ok(instance_id)
}

fn class_tag(interface: EngineInterface, class_name: &str) -> Result<*mut c_void, ModuleCallError> {
    let class = OwnedStringName::new(interface, class_name).ok_or_else(|| {
        internal_error(format!(
            "could not construct Godot class name `{class_name}` for a node field"
        ))
    })?;
    let get_class_tag = interface
        .classdb_get_class_tag
        .ok_or_else(|| internal_error("Godot ClassDB lookup is unavailable for node fields"))?;
    // SAFETY: `class` is an initialized StringName owned through this call.
    let tag = unsafe { get_class_tag(class.as_ptr()) };
    if tag.is_null() {
        return Err(internal_error(format!(
            "Godot ClassDB has no class `{class_name}` required by a node field"
        )));
    }
    Ok(tag)
}

struct OwnedNodePath {
    interface: EngineInterface,
    storage: usize,
}

impl OwnedNodePath {
    fn new(interface: EngineInterface, path: &str) -> Result<Self, ModuleCallError> {
        let source = LocalGodotString::new_utf8(interface, path).ok_or_else(|| {
            internal_error(format!(
                "could not encode node path `{path}` as a Godot String"
            ))
        })?;
        let get_constructor = interface.variant_get_ptr_constructor.ok_or_else(|| {
            internal_error("Godot builtin constructors are unavailable for node fields")
        })?;
        // SAFETY: Constructor index two is NodePath(String) in the official
        // Godot 4.4 API and remains available in supported later versions.
        let constructor = unsafe {
            get_constructor(
                GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH,
                NODE_PATH_FROM_STRING_CONSTRUCTOR,
            )
        }
        .ok_or_else(|| internal_error("Godot did not expose NodePath(String)"))?;
        let mut result = Self {
            interface,
            storage: 0,
        };
        let arguments: [GDExtensionConstTypePtr; 1] = [source.as_ptr()];
        // SAFETY: NodePath is pointer-sized in all supported official build
        // configurations and the constructor copies the live String argument.
        unsafe {
            constructor(
                ptr::from_mut(&mut result.storage).cast(),
                arguments.as_ptr(),
            );
        }
        Ok(result)
    }

    fn as_ptr(&self) -> GDExtensionConstTypePtr {
        ptr::from_ref(&self.storage).cast()
    }
}

impl Drop for OwnedNodePath {
    fn drop(&mut self) {
        let Some(get_destructor) = self.interface.variant_get_ptr_destructor else {
            return;
        };
        // SAFETY: NodePath is an official Variant builtin with one destructor.
        let Some(destructor) =
            (unsafe { get_destructor(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NODE_PATH) })
        else {
            return;
        };
        // SAFETY: This wrapper owns one initialized NodePath.
        unsafe { destructor(ptr::from_mut(&mut self.storage).cast()) };
    }
}

fn field_error(message: impl Into<String>) -> ModuleCallError {
    ModuleCallError {
        status: AbiStatus::InvalidArgument,
        message: message.into(),
    }
}

fn internal_error(message: impl Into<String>) -> ModuleCallError {
    ModuleCallError {
        status: AbiStatus::Internal,
        message: message.into(),
    }
}
