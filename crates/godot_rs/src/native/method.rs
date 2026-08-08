use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::class::{
    ClassRegistration, NativeClass, NativeInstance, NativePropertyOptions,
    NativeVirtualRegistration, validate_identifier,
};
use super::runtime::Interface;
use super::value::{GodotString, GodotStringName, GodotValueAbi};
use super::{GDEXTENSION_FALSE, GDEXTENSION_TRUE, NativeError, NativeResult, sys};

pub(crate) trait ErasedMethod {}
impl<T: 'static> ErasedMethod for T {}

/// Internal signature marker for a mutable zero-argument method.
#[doc(hidden)]
pub struct Mutable0<R>(PhantomData<R>);
/// Internal signature marker for a shared zero-argument method.
#[doc(hidden)]
pub struct Shared0<R>(PhantomData<R>);
/// Internal signature marker for a mutable one-argument method.
#[doc(hidden)]
pub struct Mutable1<A, R>(PhantomData<(A, R)>);
/// Internal signature marker for a shared one-argument method.
#[doc(hidden)]
pub struct Shared1<A, R>(PhantomData<(A, R)>);

macro_rules! declare_signature_markers {
    ($mutable:ident, $shared:ident, $($argument:ident),+) => {
        #[doc(hidden)]
        pub struct $mutable<$($argument,)+ R>(PhantomData<($($argument,)+ R)>);
        #[doc(hidden)]
        pub struct $shared<$($argument,)+ R>(PhantomData<($($argument,)+ R)>);
    };
}

declare_signature_markers!(Mutable2, Shared2, A1, A2);
declare_signature_markers!(Mutable3, Shared3, A1, A2, A3);
declare_signature_markers!(Mutable4, Shared4, A1, A2, A3, A4);
declare_signature_markers!(Mutable5, Shared5, A1, A2, A3, A4, A5);
declare_signature_markers!(Mutable6, Shared6, A1, A2, A3, A4, A5, A6);
declare_signature_markers!(Mutable7, Shared7, A1, A2, A3, A4, A5, A6, A7);
declare_signature_markers!(Mutable8, Shared8, A1, A2, A3, A4, A5, A6, A7, A8);

/// Rust functions accepted by [`super::ClassRegistrar::method`].
pub trait NativeMethod<T: NativeClass, Signature>: 'static {
    #[doc(hidden)]
    fn register(
        self,
        registration: MethodRegistration<'_, T>,
        name: &str,
        argument_names: &[&str],
    ) -> NativeResult;
}

/// Rust functions accepted by generated Extension Mode virtual registrars.
///
/// Users normally reach this trait through `engine::native_virtual::*`
/// functions, which provide the official declaring class, Method Hash, and
/// exact Rust signature.
#[doc(hidden)]
pub trait NativeVirtualMethod<T: NativeClass, Signature>: 'static {
    #[doc(hidden)]
    fn register_virtual(
        self,
        registration: MethodRegistration<'_, T>,
        contract: NativeVirtualContract<'_>,
    ) -> NativeResult;
}

/// Generated contract for one exact Godot virtual method.
#[doc(hidden)]
pub struct NativeVirtualContract<'a> {
    declaring_class: &'a str,
    name: &'a str,
    hash: u32,
    argument_count: u32,
    id: u64,
    direct_call: sys::GDExtensionClassCallVirtual,
}

impl<'a> NativeVirtualContract<'a> {
    pub(crate) fn new(
        declaring_class: &'a str,
        name: &'a str,
        hash: u32,
        argument_count: u32,
        id: u64,
        direct_call: sys::GDExtensionClassCallVirtual,
    ) -> Self {
        Self {
            declaring_class,
            name,
            hash,
            argument_count,
            id,
            direct_call,
        }
    }
}

#[doc(hidden)]
pub struct MethodRegistration<'a, T: NativeClass> {
    registration: &'a mut ClassRegistration<T>,
}

impl<'a, T: NativeClass> MethodRegistration<'a, T> {
    pub(crate) fn new(registration: &'a mut ClassRegistration<T>) -> Self {
        Self { registration }
    }
}

struct MethodData<T, F, Signature> {
    interface: Interface,
    function: F,
    marker: PhantomData<fn(T, Signature)>,
}

impl<T, F, Signature> MethodData<T, F, Signature> {
    fn new(interface: Interface, function: F) -> Self {
        Self {
            interface,
            function,
            marker: PhantomData,
        }
    }
}

impl<T, F, R> NativeMethod<T, Mutable0<R>> for F
where
    T: NativeClass,
    F: Fn(&mut T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    fn register(
        self,
        registration: MethodRegistration<'_, T>,
        name: &str,
        argument_names: &[&str],
    ) -> NativeResult {
        bind_method::<T, F, Mutable0<R>, R>(
            registration.registration,
            name,
            argument_names,
            &[],
            self,
            Some(call_mutable_0::<T, F, R>),
            Some(ptrcall_mutable_0::<T, F, R>),
        )
    }
}

impl<T, F, R> NativeVirtualMethod<T, Mutable0<R>> for F
where
    T: NativeClass,
    F: Fn(&mut T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    fn register_virtual(
        self,
        registration: MethodRegistration<'_, T>,
        contract: NativeVirtualContract<'_>,
    ) -> NativeResult {
        bind_virtual::<T, F, Mutable0<R>>(
            registration.registration,
            contract,
            self,
            Some(ptrcall_mutable_0::<T, F, R>),
        )
    }
}

impl<T, F, R> NativeMethod<T, Shared0<R>> for F
where
    T: NativeClass,
    F: Fn(&T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    fn register(
        self,
        registration: MethodRegistration<'_, T>,
        name: &str,
        argument_names: &[&str],
    ) -> NativeResult {
        bind_method::<T, F, Shared0<R>, R>(
            registration.registration,
            name,
            argument_names,
            &[],
            self,
            Some(call_shared_0::<T, F, R>),
            Some(ptrcall_shared_0::<T, F, R>),
        )
    }
}

impl<T, F, A, R> NativeMethod<T, Mutable1<A, R>> for F
where
    T: NativeClass,
    F: Fn(&mut T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    fn register(
        self,
        registration: MethodRegistration<'_, T>,
        name: &str,
        argument_names: &[&str],
    ) -> NativeResult {
        let argument_type = value_metadata::<A>()?;
        bind_method::<T, F, Mutable1<A, R>, R>(
            registration.registration,
            name,
            argument_names,
            &[argument_type],
            self,
            Some(call_mutable_1::<T, F, A, R>),
            Some(ptrcall_mutable_1::<T, F, A, R>),
        )
    }
}

impl<T, F, A, R> NativeVirtualMethod<T, Mutable1<A, R>> for F
where
    T: NativeClass,
    F: Fn(&mut T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    fn register_virtual(
        self,
        registration: MethodRegistration<'_, T>,
        contract: NativeVirtualContract<'_>,
    ) -> NativeResult {
        bind_virtual::<T, F, Mutable1<A, R>>(
            registration.registration,
            contract,
            self,
            Some(ptrcall_mutable_1::<T, F, A, R>),
        )
    }
}

impl<T, F, A, R> NativeMethod<T, Shared1<A, R>> for F
where
    T: NativeClass,
    F: Fn(&T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    fn register(
        self,
        registration: MethodRegistration<'_, T>,
        name: &str,
        argument_names: &[&str],
    ) -> NativeResult {
        let argument_type = value_metadata::<A>()?;
        bind_method::<T, F, Shared1<A, R>, R>(
            registration.registration,
            name,
            argument_names,
            &[argument_type],
            self,
            Some(call_shared_1::<T, F, A, R>),
            Some(ptrcall_shared_1::<T, F, A, R>),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ValueMetadata {
    pub variant_type: sys::GDExtensionVariantType,
    pub class_name: &'static str,
}

fn value_metadata<T: super::GodotValue>() -> Result<ValueMetadata, NativeError> {
    T::__VARIANT_TYPE
        .map(|variant_type| ValueMetadata {
            variant_type,
            class_name: T::__CLASS_NAME,
        })
        .ok_or_else(|| {
            NativeError::new("Native method arguments cannot use the Rust unit type `()`")
        })
}

mod signal_private {
    pub trait Sealed {}
}

/// Tuple of safe Native signal argument types.
#[doc(hidden)]
pub trait NativeSignalArguments: signal_private::Sealed + 'static {
    #[doc(hidden)]
    fn value_metadata() -> Result<Vec<(sys::GDExtensionVariantType, &'static str)>, NativeError>;
}

impl signal_private::Sealed for () {}

impl NativeSignalArguments for () {
    fn value_metadata() -> Result<Vec<(sys::GDExtensionVariantType, &'static str)>, NativeError> {
        Ok(Vec::new())
    }
}

macro_rules! signal_arguments {
    ($($argument:ident),+) => {
        impl<$($argument: GodotValueAbi),+> signal_private::Sealed for ($($argument,)+) {}

        impl<$($argument: GodotValueAbi),+> NativeSignalArguments for ($($argument,)+) {
            fn value_metadata(
            ) -> Result<Vec<(sys::GDExtensionVariantType, &'static str)>, NativeError> {
                Ok(vec![$({
                    let metadata = value_metadata::<$argument>()?;
                    (metadata.variant_type, metadata.class_name)
                }),+])
            }
        }
    };
}

signal_arguments!(A1);
signal_arguments!(A1, A2);
signal_arguments!(A1, A2, A3);
signal_arguments!(A1, A2, A3, A4);
signal_arguments!(A1, A2, A3, A4, A5);
signal_arguments!(A1, A2, A3, A4, A5, A6);
signal_arguments!(A1, A2, A3, A4, A5, A6, A7);
signal_arguments!(A1, A2, A3, A4, A5, A6, A7, A8);

pub(crate) fn register_property<T, V, Getter, Setter>(
    registration: &mut ClassRegistration<T>,
    property_name: &str,
    getter: Getter,
    setter: Setter,
    options: &NativePropertyOptions,
) -> NativeResult
where
    T: NativeClass,
    V: super::GodotValue + Default,
    Getter: NativeMethod<T, Shared0<V>>,
    Setter: NativeMethod<T, Mutable1<V, ()>>,
{
    validate_identifier("property", property_name)?;
    let Some(variant_type) = V::__VARIANT_TYPE else {
        return Err(NativeError::new(
            "Native properties cannot use the Rust unit type `()`",
        ));
    };
    if registration.property_names.contains(property_name) {
        return Err(NativeError::new(format!(
            "Native property `{property_name}` is already registered"
        )));
    }
    let getter_name = format!("__godot_rs_get_{property_name}");
    let setter_name = format!("__godot_rs_set_{property_name}");
    getter.register(MethodRegistration::new(registration), &getter_name, &[])?;
    setter.register(
        MethodRegistration::new(registration),
        &setter_name,
        &[property_name],
    )?;

    let interface = registration.interface;
    let class_name = GodotStringName::new(&interface, &registration.class_name)?;
    let getter_name = GodotStringName::new(&interface, &getter_name)?;
    let setter_name = GodotStringName::new(&interface, &setter_name)?;
    let mut property = PropertyStorage::new(
        &interface,
        property_name,
        ValueMetadata {
            variant_type,
            class_name: V::__CLASS_NAME,
        },
        options,
    )?;
    let property = property.raw();
    // SAFETY: Godot copies the property and StringName metadata during this
    // registration call; both generated accessors are already registered.
    unsafe {
        (interface.classdb_register_extension_class_property)(
            interface.library,
            class_name.as_ptr(),
            &property,
            setter_name.as_ptr(),
            getter_name.as_ptr(),
        );
    }
    registration.property_names.insert(property_name.to_owned());
    Ok(())
}

pub(crate) fn register_signal<T, Arguments>(
    registration: &mut ClassRegistration<T>,
    signal_name: &str,
    supplied_argument_names: &[&str],
) -> NativeResult
where
    T: NativeClass,
    Arguments: NativeSignalArguments,
{
    validate_identifier("signal", signal_name)?;
    if registration.signal_names.contains(signal_name) {
        return Err(NativeError::new(format!(
            "Native signal `{signal_name}` is already registered"
        )));
    }
    let argument_metadata = Arguments::value_metadata()?
        .into_iter()
        .map(|(variant_type, class_name)| ValueMetadata {
            variant_type,
            class_name,
        })
        .collect::<Vec<_>>();
    let argument_names = argument_names(supplied_argument_names, argument_metadata.len())?;
    for name in &argument_names {
        validate_identifier("signal argument", name)?;
    }
    let interface = registration.interface;
    let class_name = GodotStringName::new(&interface, &registration.class_name)?;
    let signal_name_value = GodotStringName::new(&interface, signal_name)?;
    let mut properties = argument_names
        .iter()
        .zip(argument_metadata)
        .map(|(name, metadata)| {
            PropertyStorage::new(
                &interface,
                name,
                metadata,
                &NativePropertyOptions::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let raw_properties = properties
        .iter_mut()
        .map(PropertyStorage::raw)
        .collect::<Vec<_>>();
    // SAFETY: Godot copies the signal metadata during registration and the
    // count exactly matches the live property array.
    unsafe {
        (interface.classdb_register_extension_class_signal)(
            interface.library,
            class_name.as_ptr(),
            signal_name_value.as_ptr(),
            if raw_properties.is_empty() {
                ptr::null()
            } else {
                raw_properties.as_ptr()
            },
            raw_properties.len() as i64,
        );
    }
    registration.signal_names.insert(signal_name.to_owned());
    Ok(())
}

macro_rules! define_method_arity {
    (
        $mutable_marker:ident,
        $shared_marker:ident,
        $call_mutable:ident,
        $call_shared:ident,
        $ptrcall_mutable:ident,
        $ptrcall_shared:ident,
        $count:expr;
        $(($argument_type:ident, $argument:ident, $index:expr)),+
    ) => {
        impl<T, F, R, $($argument_type),+>
            NativeMethod<T, $mutable_marker<$($argument_type,)+ R>> for F
        where
            T: NativeClass,
            F: Fn(&mut T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            fn register(
                self,
                registration: MethodRegistration<'_, T>,
                name: &str,
                argument_names: &[&str],
            ) -> NativeResult {
                let argument_types = [$(value_metadata::<$argument_type>()?),+];
                bind_method::<T, F, $mutable_marker<$($argument_type,)+ R>, R>(
                    registration.registration,
                    name,
                    argument_names,
                    &argument_types,
                    self,
                    Some($call_mutable::<T, F, $($argument_type,)+ R>),
                    Some($ptrcall_mutable::<T, F, $($argument_type,)+ R>),
                )
            }
        }

        impl<T, F, R, $($argument_type),+>
            NativeMethod<T, $shared_marker<$($argument_type,)+ R>> for F
        where
            T: NativeClass,
            F: Fn(&T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            fn register(
                self,
                registration: MethodRegistration<'_, T>,
                name: &str,
                argument_names: &[&str],
            ) -> NativeResult {
                let argument_types = [$(value_metadata::<$argument_type>()?),+];
                bind_method::<T, F, $shared_marker<$($argument_type,)+ R>, R>(
                    registration.registration,
                    name,
                    argument_names,
                    &argument_types,
                    self,
                    Some($call_shared::<T, F, $($argument_type,)+ R>),
                    Some($ptrcall_shared::<T, F, $($argument_type,)+ R>),
                )
            }
        }

        unsafe extern "C" fn $call_mutable<T, F, $($argument_type,)+ R>(
            method_userdata: *mut c_void,
            instance: sys::GDExtensionClassInstancePtr,
            arguments: *const sys::GDExtensionConstVariantPtr,
            argument_count: i64,
            return_value: sys::GDExtensionVariantPtr,
            error: *mut sys::GDExtensionCallError,
        ) where
            T: NativeClass,
            F: Fn(&mut T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            initialize_call_error(error);
            // SAFETY: This callback is registered with this exact MethodData type.
            let Some(data) = (unsafe {
                method_data::<T, F, $mutable_marker<$($argument_type,)+ R>>(method_userdata)
            }) else {
                return;
            };
            let _active_interface = super::runtime::activate_interface(data.interface);
            if !validate_argument_count(argument_count, $count, error) || arguments.is_null() {
                return;
            }
            $(
                // SAFETY: Count validation guarantees this live Variant entry.
                let Some($argument) = (unsafe {
                    decode_variant_argument::<$argument_type>(
                        &data.interface,
                        arguments,
                        $index,
                        error,
                    )
                }) else {
                    return;
                };
            )+
            // SAFETY: Godot dispatches the method with its owning class instance.
            let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
                fail_call(
                    error,
                    sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
                    0,
                    0,
                );
                return;
            };
            let Ok(mut value) = instance.try_borrow_mut() else {
                report_method_failure(data.interface, "reentrant mutable borrow", error);
                return;
            };
            match catch_unwind(AssertUnwindSafe(|| {
                (data.function)(&mut value, $($argument),+)
            })) {
                // SAFETY: Registration metadata declares exactly R.
                Ok(result) => unsafe { result.write_variant(&data.interface, return_value) },
                Err(_) => report_method_failure(data.interface, "Rust method panicked", error),
            }
        }

        unsafe extern "C" fn $call_shared<T, F, $($argument_type,)+ R>(
            method_userdata: *mut c_void,
            instance: sys::GDExtensionClassInstancePtr,
            arguments: *const sys::GDExtensionConstVariantPtr,
            argument_count: i64,
            return_value: sys::GDExtensionVariantPtr,
            error: *mut sys::GDExtensionCallError,
        ) where
            T: NativeClass,
            F: Fn(&T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            initialize_call_error(error);
            // SAFETY: This callback is registered with this exact MethodData type.
            let Some(data) = (unsafe {
                method_data::<T, F, $shared_marker<$($argument_type,)+ R>>(method_userdata)
            }) else {
                return;
            };
            let _active_interface = super::runtime::activate_interface(data.interface);
            if !validate_argument_count(argument_count, $count, error) || arguments.is_null() {
                return;
            }
            $(
                // SAFETY: Count validation guarantees this live Variant entry.
                let Some($argument) = (unsafe {
                    decode_variant_argument::<$argument_type>(
                        &data.interface,
                        arguments,
                        $index,
                        error,
                    )
                }) else {
                    return;
                };
            )+
            // SAFETY: Godot dispatches the method with its owning class instance.
            let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
                fail_call(
                    error,
                    sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
                    0,
                    0,
                );
                return;
            };
            let Ok(value) = instance.try_borrow() else {
                report_method_failure(data.interface, "reentrant shared borrow", error);
                return;
            };
            match catch_unwind(AssertUnwindSafe(|| {
                (data.function)(&value, $($argument),+)
            })) {
                // SAFETY: Registration metadata declares exactly R.
                Ok(result) => unsafe { result.write_variant(&data.interface, return_value) },
                Err(_) => report_method_failure(data.interface, "Rust method panicked", error),
            }
        }

        unsafe extern "C" fn $ptrcall_mutable<T, F, $($argument_type,)+ R>(
            method_userdata: *mut c_void,
            instance: sys::GDExtensionClassInstancePtr,
            arguments: *const sys::GDExtensionConstTypePtr,
            return_value: sys::GDExtensionTypePtr,
        ) where
            T: NativeClass,
            F: Fn(&mut T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            // SAFETY: This callback is registered with this exact MethodData type.
            let Some(data) = (unsafe {
                method_data::<T, F, $mutable_marker<$($argument_type,)+ R>>(method_userdata)
            }) else {
                return;
            };
            let _active_interface = super::runtime::activate_interface(data.interface);
            if arguments.is_null() {
                // SAFETY: Registration metadata provides writable R storage.
                unsafe { R::default().write_ptr(&data.interface, return_value) };
                return;
            }
            $(
                // SAFETY: Ptrcall metadata guarantees this exact list entry.
                let decoded = unsafe {
                    decode_ptr_argument::<$argument_type>(
                        &data.interface,
                        arguments,
                        $index,
                    )
                };
                let Some($argument) = decoded else {
                    // SAFETY: Registration metadata provides writable R storage.
                    unsafe { R::default().write_ptr(&data.interface, return_value) };
                    return;
                };
            )+
            // SAFETY: Godot dispatches the method with its owning class instance.
            let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
                // SAFETY: Registration metadata provides writable R storage.
                unsafe { R::default().write_ptr(&data.interface, return_value) };
                return;
            };
            let Ok(mut value) = instance.try_borrow_mut() else {
                data.interface.report_error("reentrant Native method borrow", "ptrcall");
                // SAFETY: Registration metadata provides writable R storage.
                unsafe { R::default().write_ptr(&data.interface, return_value) };
                return;
            };
            match catch_unwind(AssertUnwindSafe(|| {
                (data.function)(&mut value, $($argument),+)
            })) {
                // SAFETY: Registration metadata provides writable R storage.
                Ok(result) => unsafe { result.write_ptr(&data.interface, return_value) },
                Err(_) => {
                    data.interface.report_error("Rust Native method panicked", "ptrcall");
                    // SAFETY: Registration metadata provides writable R storage.
                    unsafe { R::default().write_ptr(&data.interface, return_value) };
                }
            }
        }

        unsafe extern "C" fn $ptrcall_shared<T, F, $($argument_type,)+ R>(
            method_userdata: *mut c_void,
            instance: sys::GDExtensionClassInstancePtr,
            arguments: *const sys::GDExtensionConstTypePtr,
            return_value: sys::GDExtensionTypePtr,
        ) where
            T: NativeClass,
            F: Fn(&T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            // SAFETY: This callback is registered with this exact MethodData type.
            let Some(data) = (unsafe {
                method_data::<T, F, $shared_marker<$($argument_type,)+ R>>(method_userdata)
            }) else {
                return;
            };
            let _active_interface = super::runtime::activate_interface(data.interface);
            if arguments.is_null() {
                // SAFETY: Registration metadata provides writable R storage.
                unsafe { R::default().write_ptr(&data.interface, return_value) };
                return;
            }
            $(
                // SAFETY: Ptrcall metadata guarantees this exact list entry.
                let decoded = unsafe {
                    decode_ptr_argument::<$argument_type>(
                        &data.interface,
                        arguments,
                        $index,
                    )
                };
                let Some($argument) = decoded else {
                    // SAFETY: Registration metadata provides writable R storage.
                    unsafe { R::default().write_ptr(&data.interface, return_value) };
                    return;
                };
            )+
            // SAFETY: Godot dispatches the method with its owning class instance.
            let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
                // SAFETY: Registration metadata provides writable R storage.
                unsafe { R::default().write_ptr(&data.interface, return_value) };
                return;
            };
            let Ok(value) = instance.try_borrow() else {
                data.interface.report_error("reentrant Native method borrow", "ptrcall");
                // SAFETY: Registration metadata provides writable R storage.
                unsafe { R::default().write_ptr(&data.interface, return_value) };
                return;
            };
            match catch_unwind(AssertUnwindSafe(|| {
                (data.function)(&value, $($argument),+)
            })) {
                // SAFETY: Registration metadata provides writable R storage.
                Ok(result) => unsafe { result.write_ptr(&data.interface, return_value) },
                Err(_) => {
                    data.interface.report_error("Rust Native method panicked", "ptrcall");
                    // SAFETY: Registration metadata provides writable R storage.
                    unsafe { R::default().write_ptr(&data.interface, return_value) };
                }
            }
        }
    };
}

macro_rules! define_virtual_arity {
    (
        $mutable_marker:ident,
        $ptrcall_mutable:ident;
        $($argument_type:ident),+
    ) => {
        impl<T, F, R, $($argument_type),+>
            NativeVirtualMethod<T, $mutable_marker<$($argument_type,)+ R>> for F
        where
            T: NativeClass,
            F: Fn(&mut T, $($argument_type),+) -> R + 'static,
            R: GodotValueAbi + Default,
            $($argument_type: GodotValueAbi),+
        {
            fn register_virtual(
                self,
                registration: MethodRegistration<'_, T>,
                contract: NativeVirtualContract<'_>,
            ) -> NativeResult {
                bind_virtual::<T, F, $mutable_marker<$($argument_type,)+ R>>(
                    registration.registration,
                    contract,
                    self,
                    Some($ptrcall_mutable::<T, F, $($argument_type,)+ R>),
                )
            }
        }
    };
}

define_method_arity!(
    Mutable2,
    Shared2,
    call_mutable_2,
    call_shared_2,
    ptrcall_mutable_2,
    ptrcall_shared_2,
    2;
    (A1, argument_1, 0),
    (A2, argument_2, 1)
);
define_method_arity!(
    Mutable3,
    Shared3,
    call_mutable_3,
    call_shared_3,
    ptrcall_mutable_3,
    ptrcall_shared_3,
    3;
    (A1, argument_1, 0),
    (A2, argument_2, 1),
    (A3, argument_3, 2)
);
define_method_arity!(
    Mutable4,
    Shared4,
    call_mutable_4,
    call_shared_4,
    ptrcall_mutable_4,
    ptrcall_shared_4,
    4;
    (A1, argument_1, 0),
    (A2, argument_2, 1),
    (A3, argument_3, 2),
    (A4, argument_4, 3)
);
define_method_arity!(
    Mutable5,
    Shared5,
    call_mutable_5,
    call_shared_5,
    ptrcall_mutable_5,
    ptrcall_shared_5,
    5;
    (A1, argument_1, 0),
    (A2, argument_2, 1),
    (A3, argument_3, 2),
    (A4, argument_4, 3),
    (A5, argument_5, 4)
);
define_method_arity!(
    Mutable6,
    Shared6,
    call_mutable_6,
    call_shared_6,
    ptrcall_mutable_6,
    ptrcall_shared_6,
    6;
    (A1, argument_1, 0),
    (A2, argument_2, 1),
    (A3, argument_3, 2),
    (A4, argument_4, 3),
    (A5, argument_5, 4),
    (A6, argument_6, 5)
);
define_method_arity!(
    Mutable7,
    Shared7,
    call_mutable_7,
    call_shared_7,
    ptrcall_mutable_7,
    ptrcall_shared_7,
    7;
    (A1, argument_1, 0),
    (A2, argument_2, 1),
    (A3, argument_3, 2),
    (A4, argument_4, 3),
    (A5, argument_5, 4),
    (A6, argument_6, 5),
    (A7, argument_7, 6)
);
define_method_arity!(
    Mutable8,
    Shared8,
    call_mutable_8,
    call_shared_8,
    ptrcall_mutable_8,
    ptrcall_shared_8,
    8;
    (A1, argument_1, 0),
    (A2, argument_2, 1),
    (A3, argument_3, 2),
    (A4, argument_4, 3),
    (A5, argument_5, 4),
    (A6, argument_6, 5),
    (A7, argument_7, 6),
    (A8, argument_8, 7)
);

define_virtual_arity!(Mutable2, ptrcall_mutable_2; A1, A2);
define_virtual_arity!(Mutable3, ptrcall_mutable_3; A1, A2, A3);
define_virtual_arity!(Mutable4, ptrcall_mutable_4; A1, A2, A3, A4);
define_virtual_arity!(Mutable5, ptrcall_mutable_5; A1, A2, A3, A4, A5);
define_virtual_arity!(Mutable6, ptrcall_mutable_6; A1, A2, A3, A4, A5, A6);
define_virtual_arity!(Mutable7, ptrcall_mutable_7; A1, A2, A3, A4, A5, A6, A7);
define_virtual_arity!(Mutable8, ptrcall_mutable_8; A1, A2, A3, A4, A5, A6, A7, A8);

fn bind_virtual<T, F, Signature>(
    registration: &mut ClassRegistration<T>,
    contract: NativeVirtualContract<'_>,
    function: F,
    ptrcall_func: sys::GDExtensionClassMethodPtrCall,
) -> NativeResult
where
    T: NativeClass,
    F: 'static,
    Signature: 'static,
{
    let NativeVirtualContract {
        declaring_class,
        name: method_name,
        hash,
        argument_count,
        id,
        direct_call,
    } = contract;
    validate_identifier("virtual declaring class", declaring_class)?;
    validate_identifier("virtual method", method_name)?;
    if registration.virtual_names.contains(method_name) {
        return Err(NativeError::new(format!(
            "Native virtual method `{method_name}` is already registered"
        )));
    }
    registration.validate_virtual_contract(declaring_class, method_name, hash, argument_count)?;
    let ptrcall = ptrcall_func.ok_or_else(|| {
        NativeError::new(format!(
            "Native virtual method `{method_name}` has no ptrcall implementation"
        ))
    })?;
    let direct_call = direct_call.ok_or_else(|| {
        NativeError::new(format!(
            "Native virtual method `{method_name}` has no direct call implementation"
        ))
    })?;
    let mut data = Box::new(MethodData::<T, F, Signature>::new(
        registration.interface,
        function,
    ));
    let data_pointer = (&mut *data as *mut MethodData<T, F, Signature>).cast::<c_void>();
    registration.methods.push(data);
    let virtual_registration = NativeVirtualRegistration::new(
        &registration.interface,
        method_name,
        hash,
        id,
        data_pointer,
        ptrcall,
        direct_call,
    )?;
    registration.virtuals.push(virtual_registration);
    registration.virtual_names.insert(method_name.to_owned());
    Ok(())
}

fn bind_method<T, F, Signature, R>(
    registration: &mut ClassRegistration<T>,
    method_name: &str,
    supplied_argument_names: &[&str],
    argument_types: &[ValueMetadata],
    function: F,
    call_func: sys::GDExtensionClassMethodCall,
    ptrcall_func: sys::GDExtensionClassMethodPtrCall,
) -> NativeResult
where
    T: NativeClass,
    F: 'static,
    Signature: 'static,
    R: GodotValueAbi,
{
    validate_identifier("method", method_name)?;
    if registration.method_names.contains(method_name) {
        return Err(NativeError::new(format!(
            "Native method `{method_name}` is already registered"
        )));
    }
    let argument_names = argument_names(supplied_argument_names, argument_types.len())?;
    for name in &argument_names {
        validate_identifier("method argument", name)?;
    }

    let interface = registration.interface;
    let method_name_value = GodotStringName::new(&interface, method_name)?;
    let class_name_value = GodotStringName::new(&interface, &registration.class_name)?;

    let mut return_property = R::VARIANT_TYPE
        .map(|variant_type| {
            PropertyStorage::new(
                &interface,
                "",
                ValueMetadata {
                    variant_type,
                    class_name: R::__CLASS_NAME,
                },
                &NativePropertyOptions::default(),
            )
        })
        .transpose()?;
    let mut argument_properties = argument_names
        .iter()
        .zip(argument_types)
        .map(|(name, metadata)| {
            PropertyStorage::new(
                &interface,
                name,
                *metadata,
                &NativePropertyOptions::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut raw_arguments = argument_properties
        .iter_mut()
        .map(PropertyStorage::raw)
        .collect::<Vec<_>>();
    let mut argument_metadata = vec![
        sys::GDExtensionClassMethodArgumentMetadata::GDEXTENSION_METHOD_ARGUMENT_METADATA_NONE;
        argument_types.len()
    ];

    let mut data = Box::new(MethodData::<T, F, Signature>::new(interface, function));
    let data_pointer = (&mut *data as *mut MethodData<T, F, Signature>).cast::<c_void>();
    registration.methods.push(data);

    let mut raw_return_property = return_property.as_mut().map(PropertyStorage::raw);
    let (has_return_value, return_value_info) = if let Some(property) = raw_return_property.as_mut()
    {
        (GDEXTENSION_TRUE, property as *mut _)
    } else {
        (GDEXTENSION_FALSE, ptr::null_mut())
    };
    let info = sys::GDExtensionClassMethodInfo {
        name: method_name_value.as_ptr().cast_mut(),
        method_userdata: data_pointer,
        call_func,
        ptrcall_func,
        method_flags: sys::GDExtensionClassMethodFlags::GDEXTENSION_METHOD_FLAG_NORMAL.0,
        has_return_value,
        return_value_info,
        return_value_metadata:
            sys::GDExtensionClassMethodArgumentMetadata::GDEXTENSION_METHOD_ARGUMENT_METADATA_NONE,
        argument_count: argument_types.len() as u32,
        arguments_info: if raw_arguments.is_empty() {
            ptr::null_mut()
        } else {
            raw_arguments.as_mut_ptr()
        },
        arguments_metadata: if argument_metadata.is_empty() {
            ptr::null_mut()
        } else {
            argument_metadata.as_mut_ptr()
        },
        default_argument_count: 0,
        default_arguments: ptr::null_mut(),
    };

    // SAFETY: Godot copies method metadata during registration. Method
    // userdata remains in a stable Box until the class is unregistered.
    unsafe {
        (interface.classdb_register_extension_class_method)(
            interface.library,
            class_name_value.as_ptr(),
            &info,
        );
    }
    registration.method_names.insert(method_name.to_owned());
    Ok(())
}

fn argument_names(supplied: &[&str], count: usize) -> Result<Vec<String>, NativeError> {
    if !supplied.is_empty() && supplied.len() != count {
        return Err(NativeError::new(format!(
            "method declares {count} argument(s), but {} argument name(s) were supplied",
            supplied.len()
        )));
    }
    if supplied.is_empty() {
        Ok((1..=count).map(|index| format!("arg_{index}")).collect())
    } else {
        Ok(supplied.iter().map(|name| (*name).to_owned()).collect())
    }
}

pub(crate) struct PropertyStorage {
    variant_type: sys::GDExtensionVariantType,
    name: GodotStringName,
    class_name: GodotStringName,
    hint_string: GodotString,
    hint: u32,
    usage: u32,
}

impl PropertyStorage {
    pub(crate) fn new(
        interface: &Interface,
        name: &str,
        metadata: ValueMetadata,
        options: &NativePropertyOptions,
    ) -> Result<Self, NativeError> {
        let hint = u32::try_from(options.hint.ord())
            .map_err(|_| NativeError::new("Native property hint is outside Godot's u32 range"))?;
        let usage = u32::try_from(options.usage.bits())
            .map_err(|_| NativeError::new("Native property usage is outside Godot's u32 range"))?;
        Ok(Self {
            variant_type: metadata.variant_type,
            name: GodotStringName::new(interface, name)?,
            class_name: GodotStringName::new(interface, metadata.class_name)?,
            hint_string: GodotString::new(interface, &options.hint_string)?,
            hint,
            usage,
        })
    }

    pub(crate) fn raw(&mut self) -> sys::GDExtensionPropertyInfo {
        sys::GDExtensionPropertyInfo {
            type_: self.variant_type,
            name: self.name.as_ptr().cast_mut(),
            class_name: self.class_name.as_ptr().cast_mut(),
            hint: self.hint,
            hint_string: self.hint_string.as_ptr().cast_mut(),
            usage: self.usage,
        }
    }
}

fn initialize_call_error(error: *mut sys::GDExtensionCallError) {
    // SAFETY: Godot either supplies writable call-error storage or null.
    if let Some(error) = unsafe { error.as_mut() } {
        error.error = sys::GDExtensionCallErrorType::GDEXTENSION_CALL_OK;
        error.argument = 0;
        error.expected = 0;
    }
}

fn fail_call(
    error: *mut sys::GDExtensionCallError,
    kind: sys::GDExtensionCallErrorType,
    argument: i32,
    expected: i32,
) {
    // SAFETY: Godot either supplies writable call-error storage or null.
    if let Some(error) = unsafe { error.as_mut() } {
        error.error = kind;
        error.argument = argument;
        error.expected = expected;
    }
}

unsafe fn decode_variant_argument<T: GodotValueAbi>(
    interface: &Interface,
    arguments: *const sys::GDExtensionConstVariantPtr,
    index: usize,
    error: *mut sys::GDExtensionCallError,
) -> Option<T> {
    // SAFETY: The caller validated the argument count and non-null list.
    let argument = unsafe { *arguments.add(index) };
    let Some(expected) = T::VARIANT_TYPE else {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_ARGUMENT,
            index as i32,
            0,
        );
        return None;
    };
    if argument.is_null() {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_ARGUMENT,
            index as i32,
            expected.0 as i32,
        );
        return None;
    }
    // SAFETY: The pointer was checked above and Godot owns a live Variant.
    let actual = unsafe { (interface.variant_get_type)(argument) };
    if expected != sys::GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_NIL && actual != expected {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_ARGUMENT,
            index as i32,
            expected.0 as i32,
        );
        return None;
    }
    // SAFETY: The dynamic Variant type exactly matches T.
    Some(unsafe { T::from_variant(interface, argument) })
}

unsafe fn decode_ptr_argument<T: GodotValueAbi>(
    interface: &Interface,
    arguments: *const sys::GDExtensionConstTypePtr,
    index: usize,
) -> Option<T> {
    // SAFETY: The registered ptrcall signature guarantees this list entry.
    let argument = unsafe { *arguments.add(index) };
    if argument.is_null() {
        return None;
    }
    // SAFETY: Registration metadata guarantees exact T storage.
    Some(unsafe { T::from_ptr(interface, argument) })
}

fn validate_argument_count(
    actual: i64,
    expected: i64,
    error: *mut sys::GDExtensionCallError,
) -> bool {
    match actual.cmp(&expected) {
        core::cmp::Ordering::Less => {
            fail_call(
                error,
                sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_FEW_ARGUMENTS,
                0,
                expected as i32,
            );
            false
        }
        core::cmp::Ordering::Greater => {
            fail_call(
                error,
                sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_TOO_MANY_ARGUMENTS,
                0,
                expected as i32,
            );
            false
        }
        core::cmp::Ordering::Equal => true,
    }
}

unsafe fn method_data<T, F, Signature>(
    pointer: *mut c_void,
) -> Option<&'static MethodData<T, F, Signature>> {
    // SAFETY: Callers only pass userdata pointers stored in stable method
    // boxes for the corresponding generic callback.
    unsafe { pointer.cast::<MethodData<T, F, Signature>>().as_ref() }
}

unsafe fn native_instance<T: NativeClass>(
    pointer: sys::GDExtensionClassInstancePtr,
) -> Option<&'static NativeInstance<T>> {
    // SAFETY: Callers only pass instance pointers supplied by Godot to the
    // callback registered for this exact Native class.
    unsafe { pointer.cast::<NativeInstance<T>>().as_ref() }
}

fn report_method_failure(
    interface: Interface,
    method: &str,
    error: *mut sys::GDExtensionCallError,
) {
    interface.report_error(method, "Native method callback");
    fail_call(
        error,
        sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INVALID_METHOD,
        0,
        0,
    );
}

unsafe extern "C" fn call_mutable_0<T, F, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    _arguments: *const sys::GDExtensionConstVariantPtr,
    argument_count: i64,
    return_value: sys::GDExtensionVariantPtr,
    error: *mut sys::GDExtensionCallError,
) where
    T: NativeClass,
    F: Fn(&mut T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    initialize_call_error(error);
    // SAFETY: This callback is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Mutable0<R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    if !validate_argument_count(argument_count, 0, error) {
        return;
    }
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
            0,
            0,
        );
        return;
    };
    let Ok(mut value) = instance.try_borrow_mut() else {
        report_method_failure(data.interface, "reentrant mutable borrow", error);
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&mut value))) {
        // SAFETY: Registration metadata declares exactly R for this output.
        Ok(result) => unsafe { result.write_variant(&data.interface, return_value) },
        Err(_) => report_method_failure(data.interface, "Rust method panicked", error),
    }
}

unsafe extern "C" fn call_shared_0<T, F, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    _arguments: *const sys::GDExtensionConstVariantPtr,
    argument_count: i64,
    return_value: sys::GDExtensionVariantPtr,
    error: *mut sys::GDExtensionCallError,
) where
    T: NativeClass,
    F: Fn(&T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    initialize_call_error(error);
    // SAFETY: This callback is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Shared0<R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    if !validate_argument_count(argument_count, 0, error) {
        return;
    }
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
            0,
            0,
        );
        return;
    };
    let Ok(value) = instance.try_borrow() else {
        report_method_failure(data.interface, "reentrant shared borrow", error);
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&value))) {
        // SAFETY: Registration metadata declares exactly R for this output.
        Ok(result) => unsafe { result.write_variant(&data.interface, return_value) },
        Err(_) => report_method_failure(data.interface, "Rust method panicked", error),
    }
}

unsafe extern "C" fn call_mutable_1<T, F, A, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    arguments: *const sys::GDExtensionConstVariantPtr,
    argument_count: i64,
    return_value: sys::GDExtensionVariantPtr,
    error: *mut sys::GDExtensionCallError,
) where
    T: NativeClass,
    F: Fn(&mut T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    initialize_call_error(error);
    // SAFETY: This callback is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Mutable1<A, R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    if !validate_argument_count(argument_count, 1, error) || arguments.is_null() {
        return;
    }
    // SAFETY: Count validation and the list null check guarantee one entry.
    let Some(argument) =
        (unsafe { decode_variant_argument::<A>(&data.interface, arguments, 0, error) })
    else {
        return;
    };
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
            0,
            0,
        );
        return;
    };
    let Ok(mut value) = instance.try_borrow_mut() else {
        report_method_failure(data.interface, "reentrant mutable borrow", error);
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&mut value, argument))) {
        // SAFETY: Registration metadata declares exactly R for this output.
        Ok(result) => unsafe { result.write_variant(&data.interface, return_value) },
        Err(_) => report_method_failure(data.interface, "Rust method panicked", error),
    }
}

unsafe extern "C" fn call_shared_1<T, F, A, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    arguments: *const sys::GDExtensionConstVariantPtr,
    argument_count: i64,
    return_value: sys::GDExtensionVariantPtr,
    error: *mut sys::GDExtensionCallError,
) where
    T: NativeClass,
    F: Fn(&T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    initialize_call_error(error);
    // SAFETY: This callback is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Shared1<A, R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    if !validate_argument_count(argument_count, 1, error) || arguments.is_null() {
        return;
    }
    // SAFETY: Count validation and the list null check guarantee one entry.
    let Some(argument) =
        (unsafe { decode_variant_argument::<A>(&data.interface, arguments, 0, error) })
    else {
        return;
    };
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        fail_call(
            error,
            sys::GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL,
            0,
            0,
        );
        return;
    };
    let Ok(value) = instance.try_borrow() else {
        report_method_failure(data.interface, "reentrant shared borrow", error);
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&value, argument))) {
        // SAFETY: Registration metadata declares exactly R for this output.
        Ok(result) => unsafe { result.write_variant(&data.interface, return_value) },
        Err(_) => report_method_failure(data.interface, "Rust method panicked", error),
    }
}

unsafe extern "C" fn ptrcall_mutable_0<T, F, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    _arguments: *const sys::GDExtensionConstTypePtr,
    return_value: sys::GDExtensionTypePtr,
) where
    T: NativeClass,
    F: Fn(&mut T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    // SAFETY: This ptrcall is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Mutable0<R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    let Ok(mut value) = instance.try_borrow_mut() else {
        data.interface
            .report_error("reentrant Native method borrow", "ptrcall");
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&mut value))) {
        // SAFETY: Registration metadata provides writable R storage.
        Ok(result) => unsafe { result.write_ptr(&data.interface, return_value) },
        Err(_) => {
            data.interface
                .report_error("Rust Native method panicked", "ptrcall");
            // SAFETY: Registration metadata provides writable R storage.
            unsafe { R::default().write_ptr(&data.interface, return_value) };
        }
    }
}

unsafe extern "C" fn ptrcall_shared_0<T, F, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    _arguments: *const sys::GDExtensionConstTypePtr,
    return_value: sys::GDExtensionTypePtr,
) where
    T: NativeClass,
    F: Fn(&T) -> R + 'static,
    R: GodotValueAbi + Default,
{
    // SAFETY: This ptrcall is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Shared0<R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    let Ok(value) = instance.try_borrow() else {
        data.interface
            .report_error("reentrant Native method borrow", "ptrcall");
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&value))) {
        // SAFETY: Registration metadata provides writable R storage.
        Ok(result) => unsafe { result.write_ptr(&data.interface, return_value) },
        Err(_) => {
            data.interface
                .report_error("Rust Native method panicked", "ptrcall");
            // SAFETY: Registration metadata provides writable R storage.
            unsafe { R::default().write_ptr(&data.interface, return_value) };
        }
    }
}

unsafe extern "C" fn ptrcall_mutable_1<T, F, A, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    arguments: *const sys::GDExtensionConstTypePtr,
    return_value: sys::GDExtensionTypePtr,
) where
    T: NativeClass,
    F: Fn(&mut T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    // SAFETY: This ptrcall is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Mutable1<A, R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    if arguments.is_null() {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    }
    // SAFETY: Ptrcall metadata guarantees one list entry.
    let Some(argument) = (unsafe { decode_ptr_argument::<A>(&data.interface, arguments, 0) })
    else {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    let Ok(mut value) = instance.try_borrow_mut() else {
        data.interface
            .report_error("reentrant Native method borrow", "ptrcall");
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&mut value, argument))) {
        // SAFETY: Registration metadata provides writable R storage.
        Ok(result) => unsafe { result.write_ptr(&data.interface, return_value) },
        Err(_) => {
            data.interface
                .report_error("Rust Native method panicked", "ptrcall");
            // SAFETY: Registration metadata provides writable R storage.
            unsafe { R::default().write_ptr(&data.interface, return_value) };
        }
    }
}

unsafe extern "C" fn ptrcall_shared_1<T, F, A, R>(
    method_userdata: *mut c_void,
    instance: sys::GDExtensionClassInstancePtr,
    arguments: *const sys::GDExtensionConstTypePtr,
    return_value: sys::GDExtensionTypePtr,
) where
    T: NativeClass,
    F: Fn(&T, A) -> R + 'static,
    A: GodotValueAbi,
    R: GodotValueAbi + Default,
{
    // SAFETY: This ptrcall is registered with this exact MethodData type.
    let Some(data) = (unsafe { method_data::<T, F, Shared1<A, R>>(method_userdata) }) else {
        return;
    };
    let _active_interface = super::runtime::activate_interface(data.interface);
    if arguments.is_null() {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    }
    // SAFETY: Ptrcall metadata guarantees one list entry.
    let Some(argument) = (unsafe { decode_ptr_argument::<A>(&data.interface, arguments, 0) })
    else {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    // SAFETY: Godot dispatches the method with its owning class instance.
    let Some(instance) = (unsafe { native_instance::<T>(instance) }) else {
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    let Ok(value) = instance.try_borrow() else {
        data.interface
            .report_error("reentrant Native method borrow", "ptrcall");
        // SAFETY: Registration metadata provides writable R storage.
        unsafe { R::default().write_ptr(&data.interface, return_value) };
        return;
    };
    match catch_unwind(AssertUnwindSafe(|| (data.function)(&value, argument))) {
        // SAFETY: Registration metadata provides writable R storage.
        Ok(result) => unsafe { result.write_ptr(&data.interface, return_value) },
        Err(_) => {
            data.interface
                .report_error("Rust Native method panicked", "ptrcall");
            // SAFETY: Registration metadata provides writable R storage.
            unsafe { R::default().write_ptr(&data.interface, return_value) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::classes;

    struct TestClass;

    impl NativeClass for TestClass {
        type Base = classes::Node;
        const CLASS_NAME: &'static str = "RustNativeMethodTest";

        fn init(_base: crate::native::Base<Self::Base>) -> Self {
            Self
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn eight_arguments(
        _value: &mut TestClass,
        first: i64,
        second: i64,
        third: i64,
        fourth: i64,
        fifth: i64,
        sixth: i64,
        seventh: i64,
        eighth: i64,
    ) -> i64 {
        first + second + third + fourth + fifth + sixth + seventh + eighth
    }

    fn accepts_native_method<M, Signature>(_method: M)
    where
        M: NativeMethod<TestClass, Signature>,
    {
    }

    #[test]
    fn argument_names_are_generated_or_checked_exactly() {
        assert_eq!(argument_names(&[], 2).unwrap(), ["arg_1", "arg_2"]);
        assert_eq!(argument_names(&["amount"], 1).unwrap(), ["amount"]);
        assert!(argument_names(&["first", "second"], 1).is_err());
    }

    #[test]
    fn methods_accept_up_to_eight_typed_arguments() {
        accepts_native_method::<_, Mutable8<i64, i64, i64, i64, i64, i64, i64, i64, i64>>(
            eight_arguments,
        );
    }

    #[test]
    fn unit_is_rejected_as_an_argument_type_without_panicking() {
        let error = value_metadata::<()>().expect_err("unit argument");
        assert!(error.to_string().contains("cannot use"));
    }
}
