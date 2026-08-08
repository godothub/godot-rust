use core::ffi::{CStr, c_void};
use core::fmt;
use core::ptr;
use godot_api::{
    GDExtensionClassCallVirtual, GDExtensionClassCreationInfo4, GDExtensionClassInstancePtr,
    GDExtensionConstStringNamePtr, GDExtensionConstTypePtr, GDExtensionMethodBindPtr,
    GDExtensionObjectPtr,
};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::interface::EngineInterface;
use crate::string_name::StaticStringName;

const GDEXTENSION_FALSE: u8 = 0;
const GDEXTENSION_TRUE: u8 = 1;
const NOTIFICATION_POSTINITIALIZE: i64 = 0;

pub(crate) type InstanceFactory = fn(EngineInterface, GDExtensionObjectPtr) -> *mut c_void;
pub(crate) type InstanceDropper = unsafe fn(*mut c_void);

#[derive(Clone, Copy)]
pub(crate) struct VirtualMethodSpec {
    pub(crate) name: &'static CStr,
    pub(crate) hash: u32,
    pub(crate) callback: GDExtensionClassCallVirtual,
}

pub(crate) struct ClassSpec {
    pub(crate) name: &'static CStr,
    pub(crate) parent: &'static CStr,
    pub(crate) factory: InstanceFactory,
    pub(crate) dropper: InstanceDropper,
    pub(crate) virtual_methods: &'static [VirtualMethodSpec],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisteredClassId(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegistryError {
    MissingMethodBind {
        class: &'static str,
        method: &'static str,
        hash: i64,
    },
    ObjectConstructionFailed(&'static str),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMethodBind {
                class,
                method,
                hash,
            } => write!(
                formatter,
                "Godot method bind `{class}.{method}` with hash {hash} is unavailable"
            ),
            Self::ObjectConstructionFailed(class) => {
                write!(
                    formatter,
                    "Godot could not construct extension class `{class}`"
                )
            }
        }
    }
}

struct VirtualMethod {
    name: StaticStringName,
    hash: u32,
    callback: GDExtensionClassCallVirtual,
}

struct ClassDescriptor {
    interface: EngineInterface,
    name_text: &'static str,
    name: StaticStringName,
    parent: StaticStringName,
    factory: InstanceFactory,
    dropper: InstanceDropper,
    notification_method: usize,
    virtual_methods: Vec<VirtualMethod>,
}

struct RegisteredClass {
    descriptor: Box<ClassDescriptor>,
}

pub(crate) struct ClassRegistry {
    interface: EngineInterface,
    notification_method: usize,
    classes: Vec<RegisteredClass>,
}

impl ClassRegistry {
    pub(crate) fn new(interface: EngineInterface) -> Result<Self, RegistryError> {
        let object = StaticStringName::new(interface, c"Object");
        let notification = StaticStringName::new(interface, c"notification");
        let method = interface
            .classdb_get_method_bind
            .expect("required ClassDB method resolver was loaded");
        // SAFETY: Both StringNames are initialized and the hash comes from the
        // reviewed official Godot 4.4 extension API.
        let notification_method =
            unsafe { method(object.as_ptr(), notification.as_ptr(), 4_023_243_586) };
        if notification_method.is_null() {
            return Err(RegistryError::MissingMethodBind {
                class: "Object",
                method: "notification",
                hash: 4_023_243_586,
            });
        }

        Ok(Self {
            interface,
            notification_method: notification_method as usize,
            classes: Vec::new(),
        })
    }

    pub(crate) fn register(&mut self, spec: ClassSpec) -> RegisteredClassId {
        let mut descriptor = Box::new(ClassDescriptor {
            interface: self.interface,
            name_text: spec
                .name
                .to_str()
                .expect("Host class names are reviewed ASCII literals"),
            name: StaticStringName::new(self.interface, spec.name),
            parent: StaticStringName::new(self.interface, spec.parent),
            factory: spec.factory,
            dropper: spec.dropper,
            notification_method: self.notification_method,
            virtual_methods: spec
                .virtual_methods
                .iter()
                .map(|method| VirtualMethod {
                    name: StaticStringName::new(self.interface, method.name),
                    hash: method.hash,
                    callback: method.callback,
                })
                .collect(),
        });

        let creation_info = GDExtensionClassCreationInfo4 {
            is_virtual: GDEXTENSION_FALSE,
            is_abstract: GDEXTENSION_FALSE,
            is_exposed: GDEXTENSION_TRUE,
            is_runtime: GDEXTENSION_FALSE,
            icon_path: ptr::null(),
            set_func: None,
            get_func: None,
            get_property_list_func: None,
            free_property_list_func: None,
            property_can_revert_func: None,
            property_get_revert_func: None,
            validate_property_func: None,
            notification_func: None,
            to_string_func: None,
            reference_func: None,
            unreference_func: None,
            create_instance_func: Some(create_instance),
            free_instance_func: Some(free_instance),
            recreate_instance_func: Some(recreate_instance),
            get_virtual_func: Some(get_virtual),
            get_virtual_call_data_func: None,
            call_virtual_with_data_func: None,
            class_userdata: (&mut *descriptor as *mut ClassDescriptor).cast(),
        };

        let register = self
            .interface
            .classdb_register_extension_class4
            .expect("required ClassDB registration function was loaded");
        // SAFETY: The descriptor is boxed and retained until after class
        // unregistration. Godot copies the creation-info structure during this
        // call as guaranteed by the official interface documentation.
        unsafe {
            register(
                self.interface.library(),
                descriptor.name.as_ptr(),
                descriptor.parent.as_ptr(),
                &creation_info,
            );
        }

        let id = RegisteredClassId(self.classes.len());
        self.classes.push(RegisteredClass { descriptor });
        id
    }

    pub(crate) fn instantiate(
        &self,
        id: RegisteredClassId,
    ) -> Result<GDExtensionObjectPtr, RegistryError> {
        let descriptor = &self.classes[id.0].descriptor;
        let construct = self
            .interface
            .classdb_construct_object2
            .expect("required ClassDB constructor was loaded");
        // SAFETY: The registered class name remains initialized and registered.
        let object = unsafe { construct(descriptor.name.as_ptr()) };
        if object.is_null() {
            return Err(RegistryError::ObjectConstructionFailed(
                descriptor.name_text,
            ));
        }
        // `classdb_construct_object2` deliberately omits postinitialization;
        // this public registry operation completes it before exposing the object.
        descriptor.notify_postinitialize(object);
        Ok(object)
    }

    pub(crate) fn unregister_all(&mut self) {
        let unregister = self
            .interface
            .classdb_unregister_extension_class
            .expect("required ClassDB unregistration function was loaded");
        for class in self.classes.iter().rev() {
            // SAFETY: Classes are unregistered from the same library and in the
            // reverse of their dependency/registration order.
            unsafe {
                unregister(self.interface.library(), class.descriptor.name.as_ptr());
            }
        }
        self.classes.clear();
    }
}

impl Drop for ClassRegistry {
    fn drop(&mut self) {
        self.unregister_all();
    }
}

impl ClassDescriptor {
    fn notify_postinitialize(&self, object: GDExtensionObjectPtr) {
        let notification = NOTIFICATION_POSTINITIALIZE;
        let reversed = GDEXTENSION_FALSE;
        let args: [GDExtensionConstTypePtr; 2] = [
            (&notification as *const i64).cast(),
            (&reversed as *const u8).cast(),
        ];
        let ptrcall = self
            .interface
            .object_method_bind_ptrcall
            .expect("required ptrcall interface was loaded");
        // SAFETY: The method bind, object, and encoded arguments match
        // Object.notification(int, bool) in the official API.
        unsafe {
            ptrcall(
                self.notification_method as GDExtensionMethodBindPtr,
                object,
                args.as_ptr(),
                ptr::null_mut(),
            );
        }
    }
}

unsafe extern "C" fn create_instance(
    class_userdata: *mut c_void,
    notify_postinitialize: u8,
) -> GDExtensionObjectPtr {
    match catch_unwind(AssertUnwindSafe(|| {
        if class_userdata.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: ClassDB returns the boxed descriptor supplied at registration.
        let descriptor = unsafe { &*(class_userdata.cast::<ClassDescriptor>()) };
        let construct = descriptor
            .interface
            .classdb_construct_object2
            .expect("required ClassDB constructor was loaded");
        // SAFETY: The registered parent StringName remains valid.
        let object = unsafe { construct(descriptor.parent.as_ptr()) };
        if object.is_null() {
            return ptr::null_mut();
        }

        let instance = (descriptor.factory)(descriptor.interface, object);
        if instance.is_null() {
            let destroy = descriptor
                .interface
                .object_destroy
                .expect("required object destroy function was loaded");
            // SAFETY: `object` was just constructed and has not escaped.
            unsafe { destroy(object) };
            return ptr::null_mut();
        }

        let set_instance = descriptor
            .interface
            .object_set_instance
            .expect("required object instance function was loaded");
        // SAFETY: The object inherits the registered parent and `instance`
        // remains owned by Godot until `free_instance`.
        unsafe { set_instance(object, descriptor.name.as_ptr(), instance) };
        if notify_postinitialize != GDEXTENSION_FALSE {
            descriptor.notify_postinitialize(object);
        }
        object
    })) {
        Ok(object) => object,
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "C" fn free_instance(
    class_userdata: *mut c_void,
    instance: GDExtensionClassInstancePtr,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if class_userdata.is_null() || instance.is_null() {
            return;
        }
        // SAFETY: Both pointers were paired by `create_instance` and remain
        // valid for this required ClassDB teardown callback.
        let descriptor = unsafe { &*(class_userdata.cast::<ClassDescriptor>()) };
        // SAFETY: The per-class dropper matches its factory allocation.
        unsafe { (descriptor.dropper)(instance) };
    }));
}

unsafe extern "C" fn recreate_instance(
    class_userdata: *mut c_void,
    _object: GDExtensionObjectPtr,
) -> GDExtensionClassInstancePtr {
    match catch_unwind(AssertUnwindSafe(|| {
        if class_userdata.is_null() {
            return ptr::null_mut();
        }
        // Godot has already detached and freed the previous Rust instance. The
        // object itself survives and Object::reset_internal_extension installs
        // the pointer returned here as its replacement extension instance.
        // SAFETY: ClassDB supplies the boxed descriptor retained by the registry.
        let descriptor = unsafe { &*(class_userdata.cast::<ClassDescriptor>()) };
        (descriptor.factory)(descriptor.interface, _object)
    })) {
        Ok(instance) => instance,
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "C" fn get_virtual(
    class_userdata: *mut c_void,
    name: GDExtensionConstStringNamePtr,
    hash: u32,
) -> GDExtensionClassCallVirtual {
    if class_userdata.is_null() || name.is_null() {
        return None;
    }
    // SAFETY: ClassDB supplies the boxed descriptor retained by the registry.
    let descriptor = unsafe { &*(class_userdata.cast::<ClassDescriptor>()) };
    descriptor
        .virtual_methods
        .iter()
        .find(|method| method.hash == hash && method.name.equals(descriptor.interface, name))
        .and_then(|method| method.callback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_errors_are_actionable() {
        assert_eq!(
            RegistryError::MissingMethodBind {
                class: "Object",
                method: "notification",
                hash: 42,
            }
            .to_string(),
            "Godot method bind `Object.notification` with hash 42 is unavailable"
        );
        assert_eq!(
            RegistryError::ObjectConstructionFailed("RustLanguage").to_string(),
            "Godot could not construct extension class `RustLanguage`"
        );
    }

    #[test]
    fn class_ids_preserve_registration_index() {
        assert_eq!(RegisteredClassId(0), RegisteredClassId(0));
        assert_ne!(RegisteredClassId(0), RegisteredClassId(1));
    }
}
