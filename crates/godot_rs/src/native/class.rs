use core::cell::{Cell, Ref, RefCell, RefMut};
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use std::collections::HashMap;
use std::collections::{BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::dynamic_value::NativeVariant;
use super::method::{
    ErasedMethod, MethodRegistration, Mutable1, NativeMethod, NativeSignalArguments,
    NativeVirtualContract, NativeVirtualMethod, PropertyStorage, Shared0, ValueMetadata,
    register_property, register_signal,
};
use super::runtime::Interface;
use super::value::{GodotString, GodotStringName};
use super::{
    GDEXTENSION_FALSE, GDEXTENSION_TRUE, InitializationContext, InitializationLevel, NativeError,
    NativeResult, sys,
};
use crate::engine::Inherits;
use crate::script::RpcConfig;
use crate::variant::{Dictionary, Variant};

pub use crate::engine::GodotClass;

macro_rules! engine_classes {
    ($($name:ident),+ $(,)?) => {
        /// Generated Godot ClassDB markers shared with the high-level SDK.
        pub mod classes {
            pub use crate::engine::{$($name),+};
        }
    };
}

godot_api::godot_rs_for_each_engine_class!(engine_classes);

/// Typed access to the Godot object that owns one Rust Native class instance.
///
/// `Base` is non-owning and deliberately neither `Send` nor `Sync`; Godot
/// object access stays on the engine thread that invoked the callback.
pub struct Base<B: GodotClass> {
    object: NonNull<c_void>,
    interface: Interface,
    marker: PhantomData<(B, Rc<()>)>,
}

impl<B: GodotClass> Copy for Base<B> {}

impl<B: GodotClass> Clone for Base<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: GodotClass> Base<B> {
    fn new(object: sys::GDExtensionObjectPtr, interface: Interface) -> Option<Self> {
        Some(Self {
            object: NonNull::new(object)?,
            interface,
            marker: PhantomData,
        })
    }

    /// Returns Godot's stable instance identifier for this object.
    #[must_use]
    pub fn instance_id(self) -> u64 {
        // SAFETY: `Base` is only created for a live object and remains
        // non-owning for the duration of its Native class instance.
        unsafe { (self.interface.object_get_instance_id)(self.object.as_ptr()) }
    }

    #[doc(hidden)]
    pub fn raw_object(self) -> sys::GDExtensionObjectPtr {
        self.object.as_ptr()
    }
}

impl<B: GodotClass> crate::engine::EngineObject for Base<B> {
    type Class = B;

    fn __engine_object(&self) -> crate::error::EngineResult<crate::engine::ObjectRef<B>> {
        Ok(crate::engine::ObjectRef::__from_instance_id(
            self.instance_id(),
        ))
    }
}

/// A Rust type registered as a Godot ClassDB class in Extension Mode.
pub trait NativeClass: Sized + 'static {
    /// Built-in Godot class inherited by this Rust class.
    type Base: GodotClass;

    /// Exact ClassDB name exposed to scenes and GDScript.
    const CLASS_NAME: &'static str;

    /// Prevents direct construction while keeping the class available as a
    /// typed ClassDB base.
    const IS_ABSTRACT: bool = false;

    /// Marks a class whose methods are intended to be overridden by derived
    /// extension classes.
    const IS_VIRTUAL: bool = false;

    /// Controls whether the class is shown to scenes and scripting languages.
    const IS_EXPOSED: bool = true;

    /// Marks a runtime-only class that is not serialized by the editor.
    const IS_RUNTIME: bool = false;

    /// Optional canonical `res://` icon shown by the Godot editor.
    const ICON_PATH: Option<&'static str> = None;

    /// Constructs Rust state for a newly created Godot object.
    fn init(base: Base<Self::Base>) -> Self;

    /// Registers methods exposed through ClassDB.
    fn register_methods(_registrar: &mut ClassRegistrar<'_, Self>) -> NativeResult {
        Ok(())
    }

    /// Registers generated Godot virtual overrides before ClassDB creates the
    /// extension class.
    ///
    /// Godot resolves virtual functions during class registration, so these
    /// callbacks have a dedicated phase. Use the generated
    /// `engine::native_virtual::*` functions here.
    fn register_virtuals(_registrar: &mut NativeVirtualRegistrar<'_, Self>) -> NativeResult {
        Ok(())
    }

    /// Receives Godot Object notifications.
    fn on_notification(&mut self, _what: i32) {}

    /// Restores runtime-only state after Godot recreates this Rust instance.
    ///
    /// Godot restores registered storage properties before invoking this
    /// callback. Fields that are not exposed as storage properties start with
    /// the values produced by [`Self::init`] and can be rebuilt here.
    fn on_extension_reloaded(&mut self) -> NativeResult {
        Ok(())
    }

    /// Handles a dynamic property that is not backed by a registered accessor.
    fn set_property(&mut self, _name: &str, _value: Variant) -> Result<bool, NativeError> {
        Ok(false)
    }

    /// Reads a dynamic property that is not backed by a registered accessor.
    fn get_property(&self, _name: &str) -> Result<Option<Variant>, NativeError> {
        Ok(None)
    }

    /// Returns dynamic Inspector properties for this instance.
    fn property_list(&self) -> Result<Vec<NativeProperty>, NativeError> {
        Ok(Vec::new())
    }

    /// Returns whether the Inspector may restore this property.
    fn property_can_revert(&self, _name: &str) -> bool {
        false
    }

    /// Returns the value restored by the Inspector.
    fn property_get_revert(&self, _name: &str) -> Result<Option<Variant>, NativeError> {
        Ok(None)
    }

    /// Adjusts hint and usage metadata before Godot shows a property.
    fn validate_property(
        &mut self,
        _property: &mut NativePropertyValidation,
    ) -> Result<(), NativeError> {
        Ok(())
    }

    /// Supplies the value shown by `str(object)` and the Godot debugger.
    fn to_godot_string(&self) -> Option<String> {
        None
    }
}

/// Inspector metadata for one registered Extension Mode property.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePropertyOptions {
    pub hint: crate::engine::global::PropertyHint,
    pub hint_string: String,
    pub usage: crate::engine::global::PropertyUsageFlags,
}

impl NativePropertyOptions {
    /// Creates property metadata using the supplied official Godot values.
    #[must_use]
    pub fn new(
        hint: crate::engine::global::PropertyHint,
        hint_string: impl Into<String>,
        usage: crate::engine::global::PropertyUsageFlags,
    ) -> Self {
        Self {
            hint,
            hint_string: hint_string.into(),
            usage,
        }
    }
}

impl Default for NativePropertyOptions {
    fn default() -> Self {
        Self {
            hint: crate::engine::global::PropertyHint::PROPERTY_HINT_NONE,
            hint_string: String::new(),
            usage: crate::engine::global::PropertyUsageFlags::PROPERTY_USAGE_STORAGE
                | crate::engine::global::PropertyUsageFlags::PROPERTY_USAGE_EDITOR,
        }
    }
}

/// One dynamic property returned from [`NativeClass::property_list`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProperty {
    pub name: String,
    pub variant_type: sys::GDExtensionVariantType,
    pub class_name: &'static str,
    pub options: NativePropertyOptions,
}

impl NativeProperty {
    /// Creates a dynamic property using one supported Native value type.
    pub fn new<V: super::GodotValue>(
        name: impl Into<String>,
        options: NativePropertyOptions,
    ) -> Result<Self, NativeError> {
        let variant_type = V::__VARIANT_TYPE.ok_or_else(|| {
            NativeError::new("Native dynamic properties cannot use the Rust unit type `()`")
        })?;
        let name = name.into();
        validate_identifier("dynamic property", &name)?;
        Ok(Self {
            name,
            variant_type,
            class_name: V::__CLASS_NAME,
            options,
        })
    }
}

/// Mutable Inspector metadata passed to [`NativeClass::validate_property`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePropertyValidation {
    pub name: String,
    pub variant_type: sys::GDExtensionVariantType,
    pub hint: crate::engine::global::PropertyHint,
    pub usage: crate::engine::global::PropertyUsageFlags,
}

/// Builder used by [`NativeClass::register_methods`].
pub struct ClassRegistrar<'a, T: NativeClass> {
    registration: &'a mut ClassRegistration<T>,
}

/// Builder for generated Godot virtual overrides in Extension Mode.
///
/// This registrar is supplied to [`NativeClass::register_virtuals`]. The
/// generated `engine::native_virtual::*` functions are the public entry point;
/// they authenticate the declaring class, Method Hash, and Rust signature.
pub struct NativeVirtualRegistrar<'a, T: NativeClass> {
    registration: &'a mut ClassRegistration<T>,
}

impl<T: NativeClass> ClassRegistrar<'_, T> {
    /// Registers a method. Argument names default to `arg_1`, `arg_2`, and so on.
    pub fn method<M, Signature>(&mut self, name: &str, method: M) -> Result<&mut Self, NativeError>
    where
        M: NativeMethod<T, Signature>,
    {
        method.register(MethodRegistration::new(self.registration), name, &[])?;
        Ok(self)
    }

    /// Registers a method with explicit Godot argument names.
    pub fn method_with_arguments<M, Signature, const N: usize>(
        &mut self,
        name: &str,
        argument_names: [&str; N],
        method: M,
    ) -> Result<&mut Self, NativeError>
    where
        M: NativeMethod<T, Signature>,
    {
        method.register(
            MethodRegistration::new(self.registration),
            name,
            &argument_names,
        )?;
        Ok(self)
    }

    /// Registers a typed Node RPC method with Godot multiplayer metadata.
    pub fn rpc_method<M, Signature>(
        &mut self,
        name: &str,
        config: RpcConfig,
        method: M,
    ) -> Result<&mut Self, NativeError>
    where
        T::Base: Inherits<crate::engine::Node>,
        M: NativeMethod<T, Signature>,
    {
        method.register(MethodRegistration::new(self.registration), name, &[])?;
        self.registration.rpc_methods.push(NativeRpcMethod {
            name: name.to_owned(),
            config,
        });
        Ok(self)
    }

    /// Registers a typed Node RPC method with explicit argument names.
    pub fn rpc_method_with_arguments<M, Signature, const N: usize>(
        &mut self,
        name: &str,
        argument_names: [&str; N],
        config: RpcConfig,
        method: M,
    ) -> Result<&mut Self, NativeError>
    where
        T::Base: Inherits<crate::engine::Node>,
        M: NativeMethod<T, Signature>,
    {
        method.register(
            MethodRegistration::new(self.registration),
            name,
            &argument_names,
        )?;
        self.registration.rpc_methods.push(NativeRpcMethod {
            name: name.to_owned(),
            config,
        });
        Ok(self)
    }

    /// Registers an Inspector-visible property backed by typed Rust accessors.
    pub fn property<V, Getter, Setter>(
        &mut self,
        name: &str,
        getter: Getter,
        setter: Setter,
    ) -> Result<&mut Self, NativeError>
    where
        V: super::GodotValue + Default,
        Getter: NativeMethod<T, Shared0<V>>,
        Setter: NativeMethod<T, Mutable1<V, ()>>,
    {
        register_property::<T, V, Getter, Setter>(
            self.registration,
            name,
            getter,
            setter,
            &NativePropertyOptions::default(),
        )?;
        Ok(self)
    }

    /// Registers an Inspector property with exact Godot hint and usage flags.
    pub fn property_with_options<V, Getter, Setter>(
        &mut self,
        name: &str,
        options: NativePropertyOptions,
        getter: Getter,
        setter: Setter,
    ) -> Result<&mut Self, NativeError>
    where
        V: super::GodotValue + Default,
        Getter: NativeMethod<T, Shared0<V>>,
        Setter: NativeMethod<T, Mutable1<V, ()>>,
    {
        register_property::<T, V, Getter, Setter>(
            self.registration,
            name,
            getter,
            setter,
            &options,
        )?;
        Ok(self)
    }

    /// Starts an Inspector property group.
    pub fn property_group(&mut self, name: &str, prefix: &str) -> Result<&mut Self, NativeError> {
        register_property_group(self.registration, name, prefix, false)?;
        Ok(self)
    }

    /// Starts an Inspector property subgroup.
    pub fn property_subgroup(
        &mut self,
        name: &str,
        prefix: &str,
    ) -> Result<&mut Self, NativeError> {
        register_property_group(self.registration, name, prefix, true)?;
        Ok(self)
    }

    /// Registers a typed ClassDB signal.
    ///
    /// Use `()` for no arguments or a tuple such as `(String, i64)` for typed
    /// arguments. Explicit argument names must match the tuple length.
    pub fn signal<Arguments, const N: usize>(
        &mut self,
        name: &str,
        argument_names: [&str; N],
    ) -> Result<&mut Self, NativeError>
    where
        Arguments: NativeSignalArguments,
    {
        register_signal::<T, Arguments>(self.registration, name, &argument_names)?;
        Ok(self)
    }
}

impl<T: NativeClass> NativeVirtualRegistrar<'_, T> {
    /// Registers one generated Godot virtual override.
    #[doc(hidden)]
    pub fn __virtual_method<const ID: u64, M, Signature>(
        &mut self,
        declaring_class: &str,
        name: &str,
        hash: u32,
        argument_count: u32,
        method: M,
    ) -> Result<&mut Self, NativeError>
    where
        M: NativeVirtualMethod<T, Signature>,
    {
        method.register_virtual(
            MethodRegistration::new(self.registration),
            NativeVirtualContract::new(
                declaring_class,
                name,
                hash,
                argument_count,
                ID,
                Some(direct_virtual_call::<T, ID>),
            ),
        )?;
        Ok(self)
    }
}

fn register_property_group<T: NativeClass>(
    registration: &ClassRegistration<T>,
    name: &str,
    prefix: &str,
    subgroup: bool,
) -> NativeResult {
    if name.is_empty() {
        return Err(NativeError::new(
            "Native property group name cannot be empty",
        ));
    }
    let interface = registration.interface;
    let class_name = GodotStringName::new(&interface, &registration.class_name)?;
    let group_name = GodotString::new(&interface, name)?;
    let prefix = GodotString::new(&interface, prefix)?;
    // SAFETY: Godot copies the group name and prefix during registration.
    unsafe {
        if subgroup {
            (interface.classdb_register_extension_class_property_subgroup)(
                interface.library,
                class_name.as_ptr(),
                group_name.as_ptr(),
                prefix.as_ptr(),
            );
        } else {
            (interface.classdb_register_extension_class_property_group)(
                interface.library,
                class_name.as_ptr(),
                group_name.as_ptr(),
                prefix.as_ptr(),
            );
        }
    }
    Ok(())
}

pub(crate) struct NativeInstance<T: NativeClass> {
    pub value: RefCell<T>,
    pub interface: Interface,
    registration: NonNull<ClassRegistration<T>>,
    poisoned: Cell<bool>,
    pending_notifications: RefCell<VecDeque<i32>>,
    draining_notifications: Cell<bool>,
}

const MAX_PENDING_NOTIFICATIONS: usize = 1_024;
const MAX_NOTIFICATIONS_PER_DRAIN: usize = 4_096;
/// Godot `Object.NOTIFICATION_EXTENSION_RELOADED`.
pub const NOTIFICATION_EXTENSION_RELOADED: i32 = 2;

pub(crate) struct NativeValueRef<'a, T: NativeClass> {
    instance: &'a NativeInstance<T>,
    value: Option<Ref<'a, T>>,
}

impl<T: NativeClass> Deref for NativeValueRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value
            .as_deref()
            .expect("Native value guard must contain its shared borrow")
    }
}

impl<T: NativeClass> Drop for NativeValueRef<'_, T> {
    fn drop(&mut self) {
        drop(self.value.take());
        self.instance.drain_notifications();
    }
}

pub(crate) struct NativeValueRefMut<'a, T: NativeClass> {
    instance: &'a NativeInstance<T>,
    value: Option<RefMut<'a, T>>,
}

impl<T: NativeClass> Deref for NativeValueRefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value
            .as_deref()
            .expect("Native value guard must contain its mutable borrow")
    }
}

impl<T: NativeClass> DerefMut for NativeValueRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
            .as_deref_mut()
            .expect("Native value guard must contain its mutable borrow")
    }
}

impl<T: NativeClass> Drop for NativeValueRefMut<'_, T> {
    fn drop(&mut self) {
        drop(self.value.take());
        self.instance.drain_notifications();
    }
}

impl<T: NativeClass> NativeInstance<T> {
    pub(crate) fn try_borrow(&self) -> Result<NativeValueRef<'_, T>, ()> {
        if self.poisoned.get() {
            return Err(());
        }
        self.value
            .try_borrow()
            .map(|value| NativeValueRef {
                instance: self,
                value: Some(value),
            })
            .map_err(|_| ())
    }

    pub(crate) fn try_borrow_mut(&self) -> Result<NativeValueRefMut<'_, T>, ()> {
        if self.poisoned.get() {
            return Err(());
        }
        self.value
            .try_borrow_mut()
            .map(|value| NativeValueRefMut {
                instance: self,
                value: Some(value),
            })
            .map_err(|_| ())
    }

    fn dispatch_notification(&self, what: i32) {
        if self.poisoned.get() {
            return;
        }
        let Ok(mut value) = self.value.try_borrow_mut() else {
            self.queue_notification(what);
            return;
        };
        self.call_notification(&mut value, what);
        drop(value);
        self.drain_notifications();
    }

    fn queue_notification(&self, what: i32) {
        let Ok(mut pending) = self.pending_notifications.try_borrow_mut() else {
            self.interface.report_error(
                "Native notification queue was borrowed reentrantly",
                T::CLASS_NAME,
            );
            return;
        };
        if pending.len() >= MAX_PENDING_NOTIFICATIONS {
            self.interface.report_error(
                "Native notification queue exceeded its safety limit",
                T::CLASS_NAME,
            );
            return;
        }
        pending.push_back(what);
    }

    fn drain_notifications(&self) {
        if self.draining_notifications.replace(true) {
            return;
        }
        let mut processed = 0;
        loop {
            let next = {
                let Ok(mut pending) = self.pending_notifications.try_borrow_mut() else {
                    self.interface.report_error(
                        "Native notification queue could not be drained",
                        T::CLASS_NAME,
                    );
                    break;
                };
                pending.pop_front()
            };
            let Some(what) = next else {
                break;
            };
            if processed >= MAX_NOTIFICATIONS_PER_DRAIN {
                self.queue_notification(what);
                self.interface.report_error(
                    "Native notification drain exceeded its safety limit",
                    T::CLASS_NAME,
                );
                break;
            }
            let Ok(mut value) = self.value.try_borrow_mut() else {
                self.queue_notification(what);
                break;
            };
            self.call_notification(&mut value, what);
            processed += 1;
        }
        self.draining_notifications.set(false);
    }

    fn call_notification(&self, value: &mut T, what: i32) {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _active_interface = super::runtime::activate_interface(self.interface);
            if what == NOTIFICATION_EXTENSION_RELOADED {
                value.on_extension_reloaded()?;
            }
            value.on_notification(what);
            Ok::<(), NativeError>(())
        }));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.poisoned.set(true);
                self.interface.report_error(
                    &format!("Native extension reload callback failed: {error}"),
                    T::CLASS_NAME,
                );
            }
            Err(_) => {
                self.poisoned.set(true);
                self.interface.report_error(
                    "Native notification callback panicked; this instance was disabled",
                    T::CLASS_NAME,
                );
            }
        }
    }
}

pub(crate) struct ClassRegistration<T: NativeClass> {
    pub interface: Interface,
    pub class_name: String,
    pub methods: Vec<Box<dyn ErasedMethod>>,
    pub method_names: BTreeSet<String>,
    pub property_names: BTreeSet<String>,
    pub signal_names: BTreeSet<String>,
    pub virtual_names: BTreeSet<String>,
    pub virtuals: Vec<NativeVirtualRegistration>,
    rpc_methods: Vec<NativeRpcMethod>,
    live_instances: AtomicUsize,
    registered: bool,
    marker: PhantomData<T>,
}

impl<T: NativeClass> ClassRegistration<T> {
    fn new(interface: Interface) -> Self {
        Self {
            interface,
            class_name: T::CLASS_NAME.to_owned(),
            methods: Vec::new(),
            method_names: BTreeSet::new(),
            property_names: BTreeSet::new(),
            signal_names: BTreeSet::new(),
            virtual_names: BTreeSet::new(),
            virtuals: Vec::new(),
            rpc_methods: Vec::new(),
            live_instances: AtomicUsize::new(0),
            registered: false,
            marker: PhantomData,
        }
    }

    fn increment_instances(&self) {
        self.live_instances.fetch_add(1, Ordering::Relaxed);
    }

    fn decrement_instances(&self) {
        let result =
            self.live_instances
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    count.checked_sub(1)
                });
        if result.is_err() {
            self.interface.report_error(
                "Godot attempted to free an untracked Native class instance",
                "free_instance",
            );
        }
    }

    pub(crate) fn validate_virtual_contract(
        &self,
        declaring_class: &str,
        method_name: &str,
        hash: u32,
        argument_count: u32,
    ) -> NativeResult {
        let mut current = Some(T::Base::CLASS_NAME);
        let mut inherits_declaring_class = false;
        while let Some(class_name) = current {
            if class_name == declaring_class {
                inherits_declaring_class = true;
                break;
            }
            current = super::api_snapshot::ENGINE_CLASSES
                .iter()
                .find(|class| class.name == class_name)
                .and_then(|class| class.inherits);
        }
        if !inherits_declaring_class {
            return Err(NativeError::new(format!(
                "Native class `{}` with base `{}` cannot override `{}.{method_name}`",
                T::CLASS_NAME,
                T::Base::CLASS_NAME,
                declaring_class,
            )));
        }
        let contract = super::api_snapshot::ENGINE_METHODS.iter().find(|method| {
            method.class == declaring_class
                && method.name == method_name
                && method.call_kind == super::api_snapshot::MethodCallKind::Virtual
        });
        let Some(contract) = contract else {
            return Err(NativeError::new(format!(
                "`{declaring_class}.{method_name}` is not a virtual method in Godot {}",
                super::GODOT_API,
            )));
        };
        let Some(contract_hash) = contract.hash.and_then(|value| u32::try_from(value).ok()) else {
            return Err(NativeError::new(format!(
                "`{declaring_class}.{method_name}` has no usable Method Hash"
            )));
        };
        if contract_hash != hash || contract.argument_count != argument_count {
            return Err(NativeError::new(format!(
                "generated Native virtual contract for `{declaring_class}.{method_name}` \
                 does not match Godot {}",
                super::GODOT_API,
            )));
        }
        Ok(())
    }
}

pub(crate) struct NativeVirtualRegistration {
    name: GodotStringName,
    hash: u32,
    id: u64,
    method_userdata: *mut c_void,
    ptrcall: unsafe extern "C" fn(
        *mut c_void,
        sys::GDExtensionClassInstancePtr,
        *const sys::GDExtensionConstTypePtr,
        sys::GDExtensionTypePtr,
    ),
    direct_call: unsafe extern "C" fn(
        sys::GDExtensionClassInstancePtr,
        *const sys::GDExtensionConstTypePtr,
        sys::GDExtensionTypePtr,
    ),
}

struct NativePropertyListAllocation {
    raw: Vec<sys::GDExtensionPropertyInfo>,
    _storage: Vec<PropertyStorage>,
}

thread_local! {
    static NATIVE_PROPERTY_LISTS: RefCell<HashMap<usize, Box<NativePropertyListAllocation>>> =
        RefCell::new(HashMap::new());
}

impl NativeVirtualRegistration {
    pub(crate) fn new(
        interface: &Interface,
        name: &str,
        hash: u32,
        id: u64,
        method_userdata: *mut c_void,
        ptrcall: unsafe extern "C" fn(
            *mut c_void,
            sys::GDExtensionClassInstancePtr,
            *const sys::GDExtensionConstTypePtr,
            sys::GDExtensionTypePtr,
        ),
        direct_call: unsafe extern "C" fn(
            sys::GDExtensionClassInstancePtr,
            *const sys::GDExtensionConstTypePtr,
            sys::GDExtensionTypePtr,
        ),
    ) -> Result<Self, NativeError> {
        Ok(Self {
            name: GodotStringName::new(interface, name)?,
            hash,
            id,
            method_userdata,
            ptrcall,
            direct_call,
        })
    }

    unsafe fn matches(&self, name: sys::GDExtensionConstStringNamePtr, hash: u32) -> bool {
        // SAFETY: Godot supplies a live StringName for this lookup callback.
        self.hash == hash && unsafe { self.name.matches_ptr(name) }
    }

    unsafe fn call(
        &self,
        instance: sys::GDExtensionClassInstancePtr,
        arguments: *const sys::GDExtensionConstTypePtr,
        return_value: sys::GDExtensionTypePtr,
    ) {
        // SAFETY: Registration pairs this userdata and ptrcall thunk with one
        // exact generated Native virtual signature.
        unsafe { (self.ptrcall)(self.method_userdata, instance, arguments, return_value) };
    }
}

#[derive(Clone, Debug)]
struct NativeRpcMethod {
    name: String,
    config: RpcConfig,
}

pub(crate) trait ErasedClassRegistration {
    fn class_name(&self) -> &str;
    fn unregister(&mut self);
    fn has_live_instances(&self) -> bool;
}

impl<T: NativeClass> ErasedClassRegistration for ClassRegistration<T> {
    fn class_name(&self) -> &str {
        &self.class_name
    }

    fn unregister(&mut self) {
        if !self.registered {
            return;
        }
        match GodotStringName::new(&self.interface, &self.class_name) {
            Ok(class_name) => {
                // SAFETY: The class was registered by this library and the
                // StringName remains alive for the call.
                unsafe {
                    (self.interface.classdb_unregister_extension_class)(
                        self.interface.library,
                        class_name.as_ptr(),
                    );
                }
                self.registered = false;
            }
            Err(error) => self.interface.report_error(
                &format!("could not unregister {}: {error}", self.class_name),
                "unregister_class",
            ),
        }
    }

    fn has_live_instances(&self) -> bool {
        self.live_instances.load(Ordering::Relaxed) != 0
    }
}

pub(crate) struct RegisteredClass {
    pub level: InitializationLevel,
    pub registration: Box<dyn ErasedClassRegistration>,
}

impl InitializationContext {
    /// Registers a Native Rust class at the current Godot initialization level.
    pub fn register_class<T: NativeClass>(&self) -> NativeResult {
        let level = self.active_level.get().ok_or_else(|| {
            NativeError::new("register_class may only be called from a level initializer")
        })?;
        validate_identifier("class", T::CLASS_NAME)?;
        validate_identifier("base class", T::Base::CLASS_NAME)?;

        let mut registrations = self.registrations.try_borrow_mut().map_err(|_| {
            NativeError::new("Native class registration is already active (reentrant call)")
        })?;
        if registrations
            .iter()
            .any(|entry| entry.registration.class_name() == T::CLASS_NAME)
        {
            return Err(NativeError::new(format!(
                "Native class `{}` is already registered",
                T::CLASS_NAME
            )));
        }

        let mut registration = Box::new(ClassRegistration::<T>::new(self.interface));
        T::register_virtuals(&mut NativeVirtualRegistrar {
            registration: &mut registration,
        })?;
        let registration_pointer =
            (&mut *registration as *mut ClassRegistration<T>).cast::<c_void>();
        let class_name = GodotStringName::new(&self.interface, T::CLASS_NAME)?;
        let base_class_name = GodotStringName::new(&self.interface, T::Base::CLASS_NAME)?;
        let icon_path = T::ICON_PATH
            .map(|path| {
                if !path.starts_with("res://") {
                    return Err(NativeError::new(format!(
                        "Native class icon must use a canonical res:// path: `{path}`"
                    )));
                }
                GodotString::new(&self.interface, path)
            })
            .transpose()?;
        let creation_info = creation_info::<T>(
            registration_pointer,
            icon_path.as_ref().map_or(ptr::null(), GodotString::as_ptr),
        );

        // SAFETY: Names and creation info remain alive for registration; the
        // boxed class userdata remains at a stable address until unregister.
        unsafe {
            (self.interface.classdb_register_extension_class)(
                self.interface.library,
                class_name.as_ptr(),
                base_class_name.as_ptr(),
                &creation_info,
            );
        }
        registration.registered = true;

        let methods_result = T::register_methods(&mut ClassRegistrar {
            registration: &mut registration,
        });
        if let Err(error) = methods_result {
            registration.unregister();
            return Err(error);
        }

        registrations.push(RegisteredClass {
            level,
            registration,
        });
        Ok(())
    }

    pub(crate) fn unregister_level(&self, level: InitializationLevel) {
        let Ok(mut registrations) = self.registrations.try_borrow_mut() else {
            self.poisoned
                .store(true, std::sync::atomic::Ordering::Release);
            self.interface.report_error(
                "Native class registry was borrowed during level shutdown",
                "unregister_level",
            );
            return;
        };
        let matching: Vec<_> = registrations
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (entry.level == level).then_some(index))
            .collect();
        for index in matching.into_iter().rev() {
            let mut entry = registrations.remove(index);
            entry.registration.unregister();
            if entry.registration.has_live_instances() {
                self.poisoned
                    .store(true, std::sync::atomic::Ordering::Release);
                self.interface.report_error(
                    &format!(
                        "Native class `{}` still has live instances during shutdown; \
                         preserving callback state to avoid dangling pointers",
                        entry.registration.class_name()
                    ),
                    "unregister_level",
                );
                let _ = Box::leak(entry.registration);
            }
        }
    }
}

fn creation_info<T: NativeClass>(
    class_userdata: *mut c_void,
    icon_path: sys::GDExtensionConstStringPtr,
) -> sys::GDExtensionClassCreationInfo {
    sys::GDExtensionClassCreationInfo {
        is_virtual: if T::IS_VIRTUAL {
            GDEXTENSION_TRUE
        } else {
            GDEXTENSION_FALSE
        },
        is_abstract: if T::IS_ABSTRACT {
            GDEXTENSION_TRUE
        } else {
            GDEXTENSION_FALSE
        },
        is_exposed: if T::IS_EXPOSED {
            GDEXTENSION_TRUE
        } else {
            GDEXTENSION_FALSE
        },
        is_runtime: if T::IS_RUNTIME {
            GDEXTENSION_TRUE
        } else {
            GDEXTENSION_FALSE
        },
        icon_path,
        set_func: Some(set_property::<T>),
        get_func: Some(get_property::<T>),
        get_property_list_func: Some(get_property_list::<T>),
        free_property_list_func: Some(free_property_list::<T>),
        property_can_revert_func: Some(property_can_revert::<T>),
        property_get_revert_func: Some(property_get_revert::<T>),
        validate_property_func: Some(validate_property::<T>),
        notification_func: Some(notification::<T>),
        to_string_func: Some(to_string::<T>),
        reference_func: None,
        unreference_func: None,
        create_instance_func: Some(create_instance::<T>),
        free_instance_func: Some(free_instance::<T>),
        recreate_instance_func: Some(recreate_instance::<T>),
        get_virtual_func: Some(get_virtual::<T>),
        get_virtual_call_data_func: Some(get_virtual_call_data::<T>),
        call_virtual_with_data_func: Some(call_virtual_with_data),
        class_userdata,
    }
}

unsafe extern "C" fn get_virtual<T: NativeClass>(
    class_userdata: *mut c_void,
    name: sys::GDExtensionConstStringNamePtr,
    hash: u32,
) -> sys::GDExtensionClassCallVirtual {
    // SAFETY: Godot returns the stable class userdata installed for T.
    let registration = unsafe { class_userdata.cast::<ClassRegistration<T>>().as_ref() }?;
    registration
        .virtuals
        .iter()
        .find(|virtual_| {
            // SAFETY: Godot supplies a live StringName for this lookup callback.
            unsafe { virtual_.matches(name, hash) }
        })
        .map(|virtual_| virtual_.direct_call)
}

unsafe extern "C" fn direct_virtual_call<T: NativeClass, const ID: u64>(
    instance: sys::GDExtensionClassInstancePtr,
    arguments: *const sys::GDExtensionConstTypePtr,
    return_value: sys::GDExtensionTypePtr,
) {
    // SAFETY: Godot calls this thunk only for a T instance.
    let Some(instance_state) = (unsafe { callback_instance::<T>(instance) }) else {
        return;
    };
    // SAFETY: Every Native instance retains the stable registration Box for
    // its class until Godot frees the instance.
    let registration = unsafe { instance_state.registration.as_ref() };
    let Some(virtual_) = registration
        .virtuals
        .iter()
        .find(|virtual_| virtual_.id == ID)
    else {
        instance_state
            .interface
            .report_error("Native virtual call data is missing", T::CLASS_NAME);
        return;
    };
    // SAFETY: The const ID selects the same generated registration and
    // ptrcall signature used to create this thunk.
    unsafe { virtual_.call(instance, arguments, return_value) };
}

unsafe fn callback_instance<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
) -> Option<&'static NativeInstance<T>> {
    // SAFETY: Godot supplies the class instance pointer created for this T.
    unsafe { instance.cast::<NativeInstance<T>>().as_ref() }
}

fn report_callback_error(interface: Interface, callback: &str, error: impl core::fmt::Display) {
    interface.report_error(&error.to_string(), callback);
}

unsafe extern "C" fn set_property<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    name: sys::GDExtensionConstStringNamePtr,
    value: sys::GDExtensionConstVariantPtr,
) -> sys::GDExtensionBool {
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return GDEXTENSION_FALSE;
    };
    // SAFETY: Godot supplies a live StringName for this property callback.
    let name = match unsafe { GodotStringName::copy_ptr_to_rust(&instance.interface, name) } {
        Ok(name) => name,
        Err(error) => {
            report_callback_error(instance.interface, "set_property", error);
            return GDEXTENSION_FALSE;
        }
    };
    let value = match NativeVariant::copy_from(instance.interface, value).to_rust(0) {
        Ok(value) => value,
        Err(error) => {
            report_callback_error(instance.interface, "set_property", error);
            return GDEXTENSION_FALSE;
        }
    };
    let Ok(mut state) = instance.try_borrow_mut() else {
        report_callback_error(
            instance.interface,
            "set_property",
            "reentrant mutable Native property access",
        );
        return GDEXTENSION_FALSE;
    };
    match catch_unwind(AssertUnwindSafe(|| state.set_property(&name, value))) {
        Ok(Ok(true)) => GDEXTENSION_TRUE,
        Ok(Ok(false)) => GDEXTENSION_FALSE,
        Ok(Err(error)) => {
            report_callback_error(instance.interface, "set_property", error);
            GDEXTENSION_FALSE
        }
        Err(_) => {
            report_callback_error(instance.interface, "set_property", "Rust callback panicked");
            GDEXTENSION_FALSE
        }
    }
}

unsafe extern "C" fn get_property<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    name: sys::GDExtensionConstStringNamePtr,
    return_value: sys::GDExtensionVariantPtr,
) -> sys::GDExtensionBool {
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return GDEXTENSION_FALSE;
    };
    // SAFETY: Godot supplies a live StringName for this property callback.
    let name = match unsafe { GodotStringName::copy_ptr_to_rust(&instance.interface, name) } {
        Ok(name) => name,
        Err(error) => {
            report_callback_error(instance.interface, "get_property", error);
            return GDEXTENSION_FALSE;
        }
    };
    let Ok(state) = instance.try_borrow() else {
        report_callback_error(
            instance.interface,
            "get_property",
            "reentrant Native property access",
        );
        return GDEXTENSION_FALSE;
    };
    let value = match catch_unwind(AssertUnwindSafe(|| state.get_property(&name))) {
        Ok(Ok(Some(value))) => value,
        Ok(Ok(None)) => return GDEXTENSION_FALSE,
        Ok(Err(error)) => {
            report_callback_error(instance.interface, "get_property", error);
            return GDEXTENSION_FALSE;
        }
        Err(_) => {
            report_callback_error(instance.interface, "get_property", "Rust callback panicked");
            return GDEXTENSION_FALSE;
        }
    };
    match NativeVariant::from_rust(instance.interface, &value, 0)
        .and_then(|native| native.copy_to_variant(return_value))
    {
        Ok(()) => GDEXTENSION_TRUE,
        Err(error) => {
            report_callback_error(instance.interface, "get_property", error);
            GDEXTENSION_FALSE
        }
    }
}

unsafe extern "C" fn get_property_list<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    count: *mut u32,
) -> *const sys::GDExtensionPropertyInfo {
    if !count.is_null() {
        // SAFETY: Godot provides writable count storage.
        unsafe { count.write(0) };
    }
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return ptr::null();
    };
    let Ok(state) = instance.try_borrow() else {
        report_callback_error(
            instance.interface,
            "get_property_list",
            "reentrant Native property access",
        );
        return ptr::null();
    };
    let properties = match catch_unwind(AssertUnwindSafe(|| state.property_list())) {
        Ok(Ok(properties)) => properties,
        Ok(Err(error)) => {
            report_callback_error(instance.interface, "get_property_list", error);
            return ptr::null();
        }
        Err(_) => {
            report_callback_error(
                instance.interface,
                "get_property_list",
                "Rust callback panicked",
            );
            return ptr::null();
        }
    };
    if properties.is_empty() {
        return ptr::null();
    }
    let property_count = match u32::try_from(properties.len()) {
        Ok(count) => count,
        Err(_) => {
            report_callback_error(
                instance.interface,
                "get_property_list",
                "Native property list exceeds Godot's u32 limit",
            );
            return ptr::null();
        }
    };
    let mut storage = match properties
        .iter()
        .map(|property| {
            PropertyStorage::new(
                &instance.interface,
                &property.name,
                ValueMetadata {
                    variant_type: property.variant_type,
                    class_name: property.class_name,
                },
                &property.options,
            )
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(storage) => storage,
        Err(error) => {
            report_callback_error(instance.interface, "get_property_list", error);
            return ptr::null();
        }
    };
    let raw = storage.iter_mut().map(PropertyStorage::raw).collect();
    let allocation = Box::new(NativePropertyListAllocation {
        raw,
        _storage: storage,
    });
    let pointer = allocation.raw.as_ptr();
    NATIVE_PROPERTY_LISTS.with(|allocations| {
        allocations
            .borrow_mut()
            .insert(pointer as usize, allocation);
    });
    if !count.is_null() {
        // SAFETY: Godot provides writable count storage.
        unsafe { count.write(property_count) };
    }
    pointer
}

unsafe extern "C" fn free_property_list<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    list: *const sys::GDExtensionPropertyInfo,
    count: u32,
) {
    if list.is_null() {
        return;
    }
    let removed =
        NATIVE_PROPERTY_LISTS.with(|allocations| allocations.borrow_mut().remove(&(list as usize)));
    if let Some(allocation) = removed {
        if usize::try_from(count).ok() != Some(allocation.raw.len()) {
            // SAFETY: This callback is installed only for T.
            if let Some(instance) = unsafe { callback_instance::<T>(instance) } {
                report_callback_error(
                    instance.interface,
                    "free_property_list",
                    "Godot returned a mismatched Native property list count",
                );
            }
        }
    } else {
        // SAFETY: This callback is installed only for T.
        if let Some(instance) = unsafe { callback_instance::<T>(instance) } {
            report_callback_error(
                instance.interface,
                "free_property_list",
                "Godot returned an unknown Native property list",
            );
        }
    }
}

unsafe extern "C" fn property_can_revert<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    name: sys::GDExtensionConstStringNamePtr,
) -> sys::GDExtensionBool {
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return GDEXTENSION_FALSE;
    };
    // SAFETY: Godot supplies a live StringName for this property callback.
    let Ok(name) = (unsafe { GodotStringName::copy_ptr_to_rust(&instance.interface, name) }) else {
        return GDEXTENSION_FALSE;
    };
    let Ok(state) = instance.try_borrow() else {
        return GDEXTENSION_FALSE;
    };
    match catch_unwind(AssertUnwindSafe(|| state.property_can_revert(&name))) {
        Ok(true) => GDEXTENSION_TRUE,
        _ => GDEXTENSION_FALSE,
    }
}

unsafe extern "C" fn property_get_revert<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    name: sys::GDExtensionConstStringNamePtr,
    return_value: sys::GDExtensionVariantPtr,
) -> sys::GDExtensionBool {
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return GDEXTENSION_FALSE;
    };
    // SAFETY: Godot supplies a live StringName for this property callback.
    let Ok(name) = (unsafe { GodotStringName::copy_ptr_to_rust(&instance.interface, name) }) else {
        return GDEXTENSION_FALSE;
    };
    let Ok(state) = instance.try_borrow() else {
        return GDEXTENSION_FALSE;
    };
    let value = match catch_unwind(AssertUnwindSafe(|| state.property_get_revert(&name))) {
        Ok(Ok(Some(value))) => value,
        Ok(Ok(None)) => return GDEXTENSION_FALSE,
        Ok(Err(error)) => {
            report_callback_error(instance.interface, "property_get_revert", error);
            return GDEXTENSION_FALSE;
        }
        Err(_) => {
            report_callback_error(
                instance.interface,
                "property_get_revert",
                "Rust callback panicked",
            );
            return GDEXTENSION_FALSE;
        }
    };
    match NativeVariant::from_rust(instance.interface, &value, 0)
        .and_then(|native| native.copy_to_variant(return_value))
    {
        Ok(()) => GDEXTENSION_TRUE,
        Err(error) => {
            report_callback_error(instance.interface, "property_get_revert", error);
            GDEXTENSION_FALSE
        }
    }
}

unsafe extern "C" fn validate_property<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    property: *mut sys::GDExtensionPropertyInfo,
) -> sys::GDExtensionBool {
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return GDEXTENSION_FALSE;
    };
    // SAFETY: Godot supplies one writable property descriptor.
    let Some(property) = (unsafe { property.as_mut() }) else {
        return GDEXTENSION_FALSE;
    };
    // SAFETY: Godot supplies a live StringName inside this writable descriptor.
    let name = match unsafe {
        GodotStringName::copy_ptr_to_rust(&instance.interface, property.name.cast_const())
    } {
        Ok(name) => name,
        Err(error) => {
            report_callback_error(instance.interface, "validate_property", error);
            return GDEXTENSION_FALSE;
        }
    };
    let mut validation = NativePropertyValidation {
        name,
        variant_type: property.type_,
        hint: crate::engine::global::PropertyHint::from_ord(i64::from(property.hint)),
        usage: crate::engine::global::PropertyUsageFlags::from_bits_retain(u64::from(
            property.usage,
        )),
    };
    let Ok(mut state) = instance.try_borrow_mut() else {
        return GDEXTENSION_FALSE;
    };
    match catch_unwind(AssertUnwindSafe(|| {
        state.validate_property(&mut validation)
    })) {
        Ok(Ok(())) => {
            let Ok(hint) = u32::try_from(validation.hint.ord()) else {
                report_callback_error(
                    instance.interface,
                    "validate_property",
                    "Native property hint is outside Godot's u32 range",
                );
                return GDEXTENSION_FALSE;
            };
            let Ok(usage) = u32::try_from(validation.usage.bits()) else {
                report_callback_error(
                    instance.interface,
                    "validate_property",
                    "Native property usage is outside Godot's u32 range",
                );
                return GDEXTENSION_FALSE;
            };
            property.type_ = validation.variant_type;
            property.hint = hint;
            property.usage = usage;
            GDEXTENSION_TRUE
        }
        Ok(Err(error)) => {
            report_callback_error(instance.interface, "validate_property", error);
            GDEXTENSION_FALSE
        }
        Err(_) => {
            report_callback_error(
                instance.interface,
                "validate_property",
                "Rust callback panicked",
            );
            GDEXTENSION_FALSE
        }
    }
}

unsafe extern "C" fn to_string<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    valid: *mut sys::GDExtensionBool,
    output: sys::GDExtensionStringPtr,
) {
    if !valid.is_null() {
        // SAFETY: Godot provides writable validity storage.
        unsafe { valid.write(GDEXTENSION_FALSE) };
    }
    // SAFETY: This callback is installed only for T.
    let Some(instance) = (unsafe { callback_instance::<T>(instance) }) else {
        return;
    };
    let Ok(state) = instance.try_borrow() else {
        return;
    };
    let value = match catch_unwind(AssertUnwindSafe(|| state.to_godot_string())) {
        Ok(Some(value)) => value,
        _ => return,
    };
    let value = match GodotString::new(&instance.interface, &value) {
        Ok(value) => value,
        Err(error) => {
            report_callback_error(instance.interface, "to_string", error);
            return;
        }
    };
    // SAFETY: Godot provides uninitialized String return storage.
    unsafe { value.move_into_ptr(output) };
    if !valid.is_null() {
        // SAFETY: Godot provides writable validity storage.
        unsafe { valid.write(GDEXTENSION_TRUE) };
    }
}

unsafe extern "C" fn get_virtual_call_data<T: NativeClass>(
    class_userdata: *mut c_void,
    name: sys::GDExtensionConstStringNamePtr,
    hash: u32,
) -> *mut c_void {
    // SAFETY: Godot returns the stable class userdata installed for T.
    let Some(registration) = (unsafe { class_userdata.cast::<ClassRegistration<T>>().as_ref() })
    else {
        return ptr::null_mut();
    };
    registration
        .virtuals
        .iter()
        .find(|virtual_| {
            // SAFETY: Godot supplies a live StringName for this lookup callback.
            unsafe { virtual_.matches(name, hash) }
        })
        .map_or(ptr::null_mut(), |virtual_| {
            ptr::from_ref(virtual_).cast_mut().cast::<c_void>()
        })
}

unsafe extern "C" fn call_virtual_with_data(
    instance: sys::GDExtensionClassInstancePtr,
    _name: sys::GDExtensionConstStringNamePtr,
    virtual_userdata: *mut c_void,
    arguments: *const sys::GDExtensionConstTypePtr,
    return_value: sys::GDExtensionTypePtr,
) {
    // SAFETY: `get_virtual_call_data` only returns stable pointers to this
    // exact registration type, retained until the class is unregistered.
    let Some(virtual_) = (unsafe {
        virtual_userdata
            .cast::<NativeVirtualRegistration>()
            .as_ref()
    }) else {
        return;
    };
    // SAFETY: Godot selected this call data using the matching name and Hash.
    unsafe { virtual_.call(instance, arguments, return_value) };
}

unsafe extern "C" fn create_instance<T: NativeClass>(
    class_userdata: *mut c_void,
    notify_postinitialize: sys::GDExtensionBool,
) -> sys::GDExtensionObjectPtr {
    // SAFETY: Godot returns the stable class userdata pointer installed with
    // `creation_info::<T>` for this exact callback.
    let Some(registration) = (unsafe { class_userdata.cast::<ClassRegistration<T>>().as_ref() })
    else {
        return ptr::null_mut();
    };
    match catch_unwind(AssertUnwindSafe(|| {
        create_instance_inner(registration, notify_postinitialize != 0)
    })) {
        Ok(object) => object,
        Err(_) => {
            registration
                .interface
                .report_error("Native class constructor panicked", T::CLASS_NAME);
            ptr::null_mut()
        }
    }
}

fn create_instance_inner<T: NativeClass>(
    registration: &ClassRegistration<T>,
    notify_postinitialize: bool,
) -> sys::GDExtensionObjectPtr {
    let interface = registration.interface;
    let base_name = match GodotStringName::new(&interface, T::Base::CLASS_NAME) {
        Ok(name) => name,
        Err(error) => {
            interface.report_error(&error.to_string(), T::CLASS_NAME);
            return ptr::null_mut();
        }
    };
    // SAFETY: The base class name is generated/validated and alive.
    let object = unsafe { (interface.classdb_construct_object2)(base_name.as_ptr()) };
    if object.is_null() {
        interface.report_error(
            &format!(
                "Godot could not construct base class `{}` for `{}`",
                T::Base::CLASS_NAME,
                T::CLASS_NAME
            ),
            "create_instance",
        );
        return ptr::null_mut();
    }

    let instance_pointer = create_rust_instance(registration, object);
    if instance_pointer.is_null() {
        // SAFETY: The base Object was never handed to Godot.
        unsafe { (interface.object_destroy)(object) };
        return ptr::null_mut();
    }
    let class_name = match GodotStringName::new(&interface, T::CLASS_NAME) {
        Ok(name) => name,
        Err(error) => {
            // SAFETY: The allocation was created immediately above and has
            // not yet been handed to Godot.
            drop(unsafe { Box::from_raw(instance_pointer.cast::<NativeInstance<T>>()) });
            registration.decrement_instances();
            // SAFETY: This uninitialized base Object is still ours.
            unsafe { (interface.object_destroy)(object) };
            interface.report_error(&error.to_string(), T::CLASS_NAME);
            return ptr::null_mut();
        }
    };
    // SAFETY: Object, class name, instance data, and callbacks all match the
    // registered class. Godot owns the object after this point.
    unsafe {
        (interface.object_set_instance)(object, class_name.as_ptr(), instance_pointer);
    }
    if notify_postinitialize {
        interface.postinitialize(object);
    }
    object
}

fn create_rust_instance<T: NativeClass>(
    registration: &ClassRegistration<T>,
    object: sys::GDExtensionObjectPtr,
) -> sys::GDExtensionClassInstancePtr {
    let interface = registration.interface;
    let Some(base) = Base::<T::Base>::new(object, interface) else {
        interface.report_error("Godot supplied a null object for Rust state", T::CLASS_NAME);
        return ptr::null_mut();
    };
    let value = match catch_unwind(AssertUnwindSafe(|| {
        let _active_interface = super::runtime::activate_interface(interface);
        T::init(base)
    })) {
        Ok(value) => value,
        Err(_) => {
            interface.report_error("Native class constructor panicked", T::CLASS_NAME);
            return ptr::null_mut();
        }
    };
    let instance = Box::new(NativeInstance {
        value: RefCell::new(value),
        interface,
        registration: NonNull::from(registration),
        poisoned: Cell::new(false),
        pending_notifications: RefCell::new(VecDeque::new()),
        draining_notifications: Cell::new(false),
    });
    let instance_pointer = Box::into_raw(instance).cast::<c_void>();
    // Godot clears the previous binding before calling the recreate callback,
    // so installing the new generation's pointer is valid for both paths.
    // SAFETY: Object, binding and callbacks belong to this live library.
    unsafe {
        (interface.object_set_instance_binding)(
            object,
            interface.library,
            instance_pointer,
            &INSTANCE_BINDING_CALLBACKS,
        );
    }
    registration.increment_instances();
    {
        let _active_interface = super::runtime::activate_interface(interface);
        if let Err(error) = apply_rpc_configs(registration, object) {
            interface.report_error(&error.to_string(), "Native RPC configuration");
        }
    }
    instance_pointer
}

unsafe extern "C" fn recreate_instance<T: NativeClass>(
    class_userdata: *mut c_void,
    object: sys::GDExtensionObjectPtr,
) -> sys::GDExtensionClassInstancePtr {
    // SAFETY: Godot returns the class userdata registered for this callback.
    let Some(registration) = (unsafe { class_userdata.cast::<ClassRegistration<T>>().as_ref() })
    else {
        return ptr::null_mut();
    };
    if object.is_null() {
        registration.interface.report_error(
            "Godot requested recreation for a null object",
            T::CLASS_NAME,
        );
        return ptr::null_mut();
    }
    match catch_unwind(AssertUnwindSafe(|| {
        create_rust_instance(registration, object)
    })) {
        Ok(instance) => instance,
        Err(_) => {
            registration
                .interface
                .report_error("Native class recreation panicked", T::CLASS_NAME);
            ptr::null_mut()
        }
    }
}

fn apply_rpc_configs<T: NativeClass>(
    registration: &ClassRegistration<T>,
    object: sys::GDExtensionObjectPtr,
) -> crate::error::EngineResult<()> {
    if registration.rpc_methods.is_empty() {
        return Ok(());
    }
    // Registration only records RPC methods when T::Base is generation-proven
    // to inherit Node. Object identity is revalidated by the generated call.
    // SAFETY: Godot passed a live object that this constructor just created.
    let node = crate::engine::ObjectRef::<crate::engine::Node>::__from_instance_id(unsafe {
        (registration.interface.object_get_instance_id)(object)
    });
    for method in &registration.rpc_methods {
        crate::engine::NodeApi::rpc_config(
            &node,
            &method.name,
            &rpc_config_variant(method.config),
        )?;
    }
    Ok(())
}

fn rpc_config_variant(config: RpcConfig) -> Variant {
    let mut value = Dictionary::new();
    value.insert(
        "rpc_mode",
        match config.mode {
            crate::script::RpcMode::Authority => 0_i64,
            crate::script::RpcMode::AnyPeer => 1_i64,
        },
    );
    value.insert("call_local", config.call_local);
    value.insert(
        "transfer_mode",
        match config.transfer_mode {
            crate::script::RpcTransferMode::Unreliable => 0_i64,
            crate::script::RpcTransferMode::UnreliableOrdered => 1_i64,
            crate::script::RpcTransferMode::Reliable => 2_i64,
        },
    );
    value.insert("channel", i64::from(config.channel));
    Variant::from(value)
}

unsafe extern "C" fn free_instance<T: NativeClass>(
    class_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
) {
    if instance.is_null() {
        return;
    }
    // SAFETY: Godot returns the stable class userdata pointer installed with
    // the free callback; null is handled by `Option`.
    let registration = unsafe { class_userdata.cast::<ClassRegistration<T>>().as_ref() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Godot returns the exact allocation installed by create or
        // recreate and calls this callback once.
        drop(unsafe { Box::from_raw(instance.cast::<NativeInstance<T>>()) });
    }));
    if let Some(registration) = registration {
        registration.decrement_instances();
        if result.is_err() {
            registration
                .interface
                .report_error("Native class destructor panicked", T::CLASS_NAME);
        }
    }
}

unsafe extern "C" fn notification<T: NativeClass>(
    instance: sys::GDExtensionClassInstancePtr,
    what: i32,
    _reversed: sys::GDExtensionBool,
) {
    // SAFETY: Godot passes the exact NativeInstance<T> pointer installed for
    // the registered class; null is handled by `Option`.
    let Some(instance) = (unsafe { instance.cast::<NativeInstance<T>>().as_ref() }) else {
        return;
    };
    instance.dispatch_notification(what);
}

static INSTANCE_BINDING_CALLBACKS: sys::GDExtensionInstanceBindingCallbacks =
    sys::GDExtensionInstanceBindingCallbacks {
        create_callback: None,
        free_callback: None,
        reference_callback: None,
    };

pub(crate) fn validate_identifier(kind: &str, value: &str) -> NativeResult {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err(NativeError::new(format!("{kind} name must not be empty")));
    };
    if first != '_' && !first.is_alphabetic() {
        return Err(NativeError::new(format!(
            "{kind} `{value}` must start with a Unicode letter or `_`"
        )));
    }
    if characters.any(|character| character != '_' && !character.is_alphanumeric()) {
        return Err(NativeError::new(format!(
            "{kind} `{value}` may only contain Unicode letters, numbers, or `_`"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn godot_identifiers_are_validated_before_classdb_calls() {
        assert!(validate_identifier("class", "RustCounter2D").is_ok());
        assert!(validate_identifier("class", "中文节点").is_ok());
        assert!(validate_identifier("class", "").is_err());
        assert!(validate_identifier("class", "2Counter").is_err());
        assert!(validate_identifier("class", "Rust-Counter").is_err());
    }

    #[test]
    fn native_rpc_metadata_uses_godots_node_configuration_keys() {
        let config = rpc_config_variant(RpcConfig {
            mode: crate::script::RpcMode::AnyPeer,
            call_local: true,
            transfer_mode: crate::script::RpcTransferMode::Reliable,
            channel: 7,
        });
        let crate::variant::VariantKind::Dictionary(config) = config.kind() else {
            panic!("RPC config must be a Dictionary");
        };
        assert_eq!(
            config.get(&Variant::from("rpc_mode")),
            Some(&Variant::from(1_i64))
        );
        assert_eq!(
            config.get(&Variant::from("call_local")),
            Some(&Variant::from(true))
        );
        assert_eq!(
            config.get(&Variant::from("transfer_mode")),
            Some(&Variant::from(2_i64))
        );
        assert_eq!(
            config.get(&Variant::from("channel")),
            Some(&Variant::from(7_i64))
        );
    }
}
