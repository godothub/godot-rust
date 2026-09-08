use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use godot_api::abi::{
    ABI_CALLABLE_MAGIC, ABI_CALLABLE_OWNED, ABI_CALLABLE_VERSION, ABI_VALUE_OWNED_BYTES, AbiStatus,
    AbiValueType, AbiValueV1, callable_value_ownership_token, validate_callable_value,
};
use godot_api::{
    GDExtensionBool, GDExtensionCallError, GDExtensionCallErrorType,
    GDExtensionCallableCustomInfo2, GDExtensionConstTypePtr, GDExtensionConstVariantPtr,
    GDExtensionInt, GDExtensionObjectPtr, GDExtensionTypePtr, GDExtensionVariantPtr,
    GDExtensionVariantType,
};

use crate::engine_call::EngineCallContext;
use crate::engine_call::value::ValueError;
use crate::interface::EngineInterface;
use crate::string_name::OwnedStringName;
use crate::variant_codec::{OwnedVariant, VariantCodec};

const CALLABLE_HEADER_BYTES: usize = 32;
const GET_OBJECT_ID_HASH: i64 = 3_173_160_232;
const GET_METHOD_HASH: i64 = 1_825_232_092;
static SIGNAL_WAIT_CALLABLE_TOKEN: u8 = 0;

#[repr(C, align(8))]
struct CallableStorage([u8; 16]);

type CallableConstructor = unsafe extern "C" fn(GDExtensionTypePtr, *const GDExtensionConstTypePtr);
type CallableDestructor = unsafe extern "C" fn(GDExtensionTypePtr);
type CallableBuiltinMethod = unsafe extern "C" fn(
    GDExtensionTypePtr,
    *const GDExtensionConstTypePtr,
    GDExtensionTypePtr,
    i32,
);

/// One initialized native Godot Callable.
pub(crate) struct NativeCallable {
    interface: EngineInterface,
    storage: CallableStorage,
    initialized: bool,
    destroy: CallableDestructor,
    copy: CallableConstructor,
    get_object_id: CallableBuiltinMethod,
    get_method: CallableBuiltinMethod,
}

type CapturedSignalArguments = Result<Option<crate::dynamic_value::NativeDynamic>, ValueError>;

/// Shared state written by Godot's custom Callable and read by the frame
/// scheduler. It contains no project-module pointers and remains valid while
/// either Godot or the Host owns a Callable copy.
pub(crate) struct SignalWaitState {
    fired: AtomicBool,
    claimed: AtomicBool,
    active: AtomicBool,
    arguments: Mutex<Option<CapturedSignalArguments>>,
}

impl SignalWaitState {
    pub(crate) const fn new() -> Self {
        Self {
            fired: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
            active: AtomicBool::new(true),
            arguments: Mutex::new(None),
        }
    }

    pub(crate) fn fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    pub(crate) fn take_arguments(
        &self,
    ) -> Result<Option<crate::dynamic_value::NativeDynamic>, ValueError> {
        self.arguments
            .lock()
            .map_err(|_| internal("Signal future arguments are unavailable"))?
            .take()
            .ok_or_else(|| internal("Signal future completed without arguments"))?
    }

    pub(crate) fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }
}

struct SignalWaitCallableData {
    state: Arc<SignalWaitState>,
    identity: u64,
    interface: EngineInterface,
}

pub(crate) struct CallableCallBacking {
    bytes: Box<[u8]>,
    context: *const EngineCallContext,
    token: u64,
}

impl CallableCallBacking {
    pub(crate) fn from_variant(
        codec: &VariantCodec,
        value: GDExtensionConstVariantPtr,
        context: &EngineCallContext,
    ) -> Result<Self, ValueError> {
        let callable = NativeCallable::from_variant(codec, value)?;
        let token = context.retain_callable(callable)?;
        let bytes = match context
            .copy_callable(token)
            .and_then(|copy| copy.to_bytes(token))
        {
            Ok(bytes) => bytes.into_boxed_slice(),
            Err(error) => {
                let _ = context.release_callable(token);
                return Err(error);
            }
        };
        Ok(Self {
            bytes,
            context: ptr::from_ref(context),
            token,
        })
    }

    pub(crate) fn abi(&self) -> AbiValueV1 {
        AbiValueV1::from_borrowed_bytes(AbiValueType::CALLABLE, &self.bytes)
    }
}

impl Drop for CallableCallBacking {
    fn drop(&mut self) {
        // SAFETY: The backing never outlives the module generation that owns
        // this exact EngineCallContext.
        let context = unsafe { &*self.context };
        let _ = context.release_callable(self.token);
    }
}

impl NativeCallable {
    pub(crate) fn empty(interface: EngineInterface) -> Result<Self, ValueError> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 0)?;
        // SAFETY: Storage is correctly aligned and the default constructor has
        // no arguments.
        unsafe { constructor(value.as_mut_ptr(), ptr::null()) };
        value.initialized = true;
        Ok(value)
    }

    pub(crate) fn from_standard(
        interface: EngineInterface,
        object: GDExtensionObjectPtr,
        method: &OwnedStringName,
    ) -> Result<Self, ValueError> {
        let mut value = Self::uninitialized(interface)?;
        let constructor = constructor(interface, 2)?;
        let arguments = [
            ptr::from_ref(&object).cast(),
            method.as_ptr().cast::<core::ffi::c_void>(),
        ];
        // SAFETY: The constructor index and argument layout come from the
        // authenticated Callable API.
        unsafe { constructor(value.as_mut_ptr(), arguments.as_ptr()) };
        value.initialized = true;
        Ok(value)
    }

    pub(crate) fn from_signal_waiter(
        interface: EngineInterface,
        state: Arc<SignalWaitState>,
        identity: u64,
    ) -> Result<Self, ValueError> {
        let mut value = Self::uninitialized(interface)?;
        let create = interface
            .callable_custom_create2
            .ok_or_else(|| internal("Godot custom Callable creation is unavailable"))?;
        let userdata = Box::into_raw(Box::new(SignalWaitCallableData {
            state,
            identity,
            interface,
        }))
        .cast();
        let mut info = GDExtensionCallableCustomInfo2 {
            callable_userdata: userdata,
            token: ptr::addr_of!(SIGNAL_WAIT_CALLABLE_TOKEN).cast_mut().cast(),
            object_id: 0,
            call_func: Some(signal_wait_call),
            is_valid_func: Some(signal_wait_is_valid),
            free_func: Some(signal_wait_free),
            hash_func: Some(signal_wait_hash),
            equal_func: Some(signal_wait_equal),
            less_than_func: None,
            to_string_func: None,
            get_argument_count_func: Some(signal_wait_argument_count),
        };
        // SAFETY: Callable storage is aligned and uninitialized, every
        // callback uses the boxed userdata layout above, and Godot assumes
        // ownership of that userdata through `free_func`.
        unsafe { create(value.as_mut_ptr(), &mut info) };
        value.initialized = true;
        Ok(value)
    }

    pub(crate) fn from_variant(
        codec: &VariantCodec,
        value: GDExtensionConstVariantPtr,
    ) -> Result<Self, ValueError> {
        if codec.variant_type(value)
            != Some(GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE)
        {
            return Err(invalid("Godot Variant is not a Callable"));
        }
        let interface = codec.interface();
        let mut callable = Self::uninitialized(interface)?;
        let get_to = interface
            .get_variant_to_type_constructor
            .ok_or_else(|| internal("Godot Variant-to-Callable constructor is unavailable"))?;
        // SAFETY: Callable is an official Variant builtin type.
        let convert = unsafe { get_to(callable_type()) }
            .ok_or_else(|| internal("Godot Variant-to-Callable conversion is unavailable"))?;
        // SAFETY: Output is aligned Callable storage and the input points to a
        // live Callable Variant.
        unsafe { convert(callable.as_mut_ptr(), value.cast_mut()) };
        callable.initialized = true;
        Ok(callable)
    }

    pub(crate) fn copy_value(&self) -> Result<Self, ValueError> {
        let mut value = Self::uninitialized(self.interface)?;
        let arguments = [self.as_ptr()];
        // SAFETY: Source is a live Callable and destination is uninitialized
        // aligned Callable storage.
        unsafe { (self.copy)(value.as_mut_ptr(), arguments.as_ptr()) };
        value.initialized = true;
        Ok(value)
    }

    pub(crate) fn to_variant(&self, _codec: &VariantCodec) -> Result<OwnedVariant, ValueError> {
        let get_from = self
            .interface
            .get_variant_from_type_constructor
            .ok_or_else(|| internal("Godot Callable-to-Variant constructor is unavailable"))?;
        // SAFETY: Callable is an official Variant builtin type.
        let convert = unsafe { get_from(callable_type()) }
            .ok_or_else(|| internal("Godot Callable-to-Variant conversion is unavailable"))?;
        let mut variant = OwnedVariant::uninitialized(self.interface);
        // SAFETY: Destination is uninitialized Variant storage and source is
        // one live Callable.
        unsafe { convert(variant.as_mut_ptr(), self.as_ptr().cast_mut()) };
        variant.mark_initialized();
        Ok(variant)
    }

    pub(crate) fn object_id(&self) -> u64 {
        let mut object_id = 0_i64;
        // SAFETY: This builtin method takes no arguments and writes an int64
        // return value.
        unsafe {
            (self.get_object_id)(
                self.as_mut_const_ptr(),
                ptr::null(),
                ptr::from_mut(&mut object_id).cast(),
                0,
            );
        }
        object_id as u64
    }

    pub(crate) fn method(&self) -> Result<String, ValueError> {
        let mut method_name = OwnedStringName::empty(self.interface)
            .ok_or_else(|| internal("Godot StringName could not be initialized"))?;
        // SAFETY: This builtin method takes no arguments and writes one
        // initialized StringName result.
        unsafe {
            (self.get_method)(
                self.as_mut_const_ptr(),
                ptr::null(),
                method_name.as_mut_ptr(),
                0,
            );
        }
        method_name
            .to_utf8()
            .map_err(|_| internal("Godot Callable returned invalid UTF-8"))
    }

    pub(crate) fn to_bytes(&self, token: u64) -> Result<Vec<u8>, ValueError> {
        let method = self.method()?;
        let method_length = u32::try_from(method.len())
            .map_err(|_| unsupported("Godot Callable method name exceeds the Host limit"))?;
        let mut bytes = Vec::with_capacity(CALLABLE_HEADER_BYTES + method.len());
        bytes.extend_from_slice(&ABI_CALLABLE_MAGIC);
        bytes.extend_from_slice(&ABI_CALLABLE_VERSION.to_le_bytes());
        bytes.extend_from_slice(
            &if token == 0 {
                0_u16
            } else {
                ABI_CALLABLE_OWNED
            }
            .to_le_bytes(),
        );
        bytes.extend_from_slice(&token.to_le_bytes());
        bytes.extend_from_slice(&self.object_id().to_le_bytes());
        bytes.extend_from_slice(&method_length.to_le_bytes());
        bytes.extend_from_slice(method.as_bytes());
        validate_callable_value(&bytes)
            .then_some(bytes)
            .ok_or_else(|| internal("Godot Callable produced an invalid ABI payload"))
    }

    pub(crate) fn from_abi(
        interface: EngineInterface,
        value: AbiValueV1,
        context: Option<&EngineCallContext>,
        resolve_object: impl FnOnce(u64) -> Result<GDExtensionObjectPtr, ValueError>,
    ) -> Result<Self, ValueError> {
        if value.type_ != AbiValueType::CALLABLE
            || !matches!(value.reserved_flags, 0 | ABI_VALUE_OWNED_BYTES)
        {
            return Err(invalid(
                "Callable argument does not match its generated contract",
            ));
        }
        let (pointer, length) = value
            .byte_range(AbiValueType::CALLABLE)
            .ok_or_else(|| invalid("Callable argument has an invalid byte range"))?;
        // SAFETY: The project module synchronously retains this bounded range.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        if !validate_callable_value(bytes) {
            return Err(invalid("Callable argument has an invalid ABI payload"));
        }
        if let Some(token) = callable_value_ownership_token(bytes) {
            return context
                .ok_or_else(|| invalid("Host Callable context is unavailable"))?
                .copy_callable(token);
        }
        let object_id = u64::from_le_bytes(
            bytes[20..28]
                .try_into()
                .expect("validated Callable object ID"),
        );
        let method = core::str::from_utf8(&bytes[CALLABLE_HEADER_BYTES..])
            .expect("validated Callable UTF-8");
        let object = resolve_object(object_id)?;
        let method = OwnedStringName::new(interface, method)
            .ok_or_else(|| invalid("Callable method name could not be encoded"))?;
        let callable = Self::from_standard(interface, object, &method)?;
        if godot_api::abi::callable_value_retains_target(bytes) {
            callable.retaining_target(object_id, object)
        } else {
            Ok(callable)
        }
    }

    fn retaining_target(
        self,
        object_id: u64,
        object: GDExtensionObjectPtr,
    ) -> Result<Self, ValueError> {
        let interface = self.interface;
        let class = OwnedStringName::new(interface, "RefCounted")
            .ok_or_else(|| internal("RefCounted class name could not be encoded"))?;
        let get_tag = interface
            .classdb_get_class_tag
            .ok_or_else(|| internal("Godot class lookup is unavailable"))?;
        let cast = interface
            .object_cast_to
            .ok_or_else(|| internal("Godot object class validation is unavailable"))?;
        // SAFETY: Both names and pointers belong to the active engine. Only a
        // RefCounted target can be held alive by the bound Variant below.
        let tag = unsafe { get_tag(class.as_ptr()) };
        if tag.is_null() || object.is_null() {
            return Err(invalid(
                "retained Callable target must be a RefCounted object",
            ));
        }
        // SAFETY: Both live pointers were obtained from the active engine above.
        if unsafe { cast(object, tag) }.is_null() {
            return Err(invalid(
                "retained Callable target must be a RefCounted object",
            ));
        }
        let codec = VariantCodec::new(interface)
            .ok_or_else(|| internal("Godot Variant codecs are unavailable"))?;
        let target = OwnedVariant::from_abi(&codec, AbiValueV1::from_object_id(object_id))
            .map_err(|_| invalid("Callable target could not be retained"))?;
        let arguments = crate::dynamic_value::NativeDynamic::from_variant_arguments(
            interface,
            &[target.as_ptr()],
        )?;
        let unbind = builtin_method(interface, c"unbind", 755_001_590)?;
        let bind = builtin_method(interface, c"bindv", 3_564_560_322)?;
        let mut unbound = Self::uninitialized(interface)?;
        let count = 1_i64;
        let unbind_args = [ptr::from_ref(&count).cast()];
        // SAFETY: Authenticated Callable.unbind(int) writes a new Callable.
        unsafe {
            unbind(
                self.as_mut_const_ptr(),
                unbind_args.as_ptr(),
                unbound.as_mut_ptr(),
                1,
            )
        };
        unbound.initialized = true;
        let mut retained = Self::uninitialized(interface)?;
        let bind_args = [arguments.as_const_ptr()];
        // Godot's bind wrapper owns the target. Its matching unbind discards
        // that extra argument before dispatch, preserving the original method.
        // SAFETY: Authenticated Callable.bindv(Array) writes a new Callable.
        unsafe {
            bind(
                unbound.as_mut_const_ptr(),
                bind_args.as_ptr(),
                retained.as_mut_ptr(),
                1,
            )
        };
        retained.initialized = true;
        Ok(retained)
    }

    pub(crate) fn as_ptr(&self) -> GDExtensionConstTypePtr {
        self.storage.0.as_ptr().cast()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> GDExtensionTypePtr {
        self.storage.0.as_mut_ptr().cast()
    }

    fn as_mut_const_ptr(&self) -> GDExtensionTypePtr {
        self.storage.0.as_ptr().cast_mut().cast()
    }

    fn uninitialized(interface: EngineInterface) -> Result<Self, ValueError> {
        Ok(Self {
            interface,
            storage: CallableStorage([0; 16]),
            initialized: false,
            destroy: destructor(interface)?,
            copy: constructor(interface, 1)?,
            get_object_id: builtin_method(interface, c"get_object_id", GET_OBJECT_ID_HASH)?,
            get_method: builtin_method(interface, c"get_method", GET_METHOD_HASH)?,
        })
    }
}

unsafe extern "C" fn signal_wait_call(
    userdata: *mut core::ffi::c_void,
    arguments: *const GDExtensionConstVariantPtr,
    argument_count: GDExtensionInt,
    output: GDExtensionVariantPtr,
    call_error: *mut GDExtensionCallError,
) {
    if userdata.is_null() {
        if !call_error.is_null() {
            // SAFETY: Godot supplied a writable call-error pointer.
            unsafe {
                (*call_error).error =
                    GDExtensionCallErrorType::GDEXTENSION_CALL_ERROR_INSTANCE_IS_NULL;
                (*call_error).argument = 0;
                (*call_error).expected = 0;
            }
        }
        return;
    }
    // SAFETY: The custom Callable owns this exact userdata type until
    // `signal_wait_free`.
    let data = unsafe { &*userdata.cast::<SignalWaitCallableData>() };
    if data.state.active.load(Ordering::Acquire)
        && data
            .state
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        let copied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !(0..=1_024).contains(&argument_count)
                || (argument_count != 0 && arguments.is_null())
            {
                return Err(ValueError::invalid(
                    "Godot signal argument count is invalid",
                ));
            }
            let arguments = if argument_count == 0 {
                &[][..]
            } else {
                // SAFETY: Godot retains the validated argument slice through
                // this callback. Every Variant is copied before returning.
                unsafe { core::slice::from_raw_parts(arguments, argument_count as usize) }
            };
            if arguments.is_empty() {
                Ok(None)
            } else {
                crate::dynamic_value::NativeDynamic::from_variant_arguments(
                    data.interface,
                    arguments,
                )
                .map(Some)
            }
        }))
        .unwrap_or_else(|_| Err(internal("Godot signal argument capture panicked")));
        if let Ok(mut arguments) = data.state.arguments.lock() {
            *arguments = Some(copied);
        }
        data.state.fired.store(true, Ordering::Release);
    }
    if !output.is_null() {
        if let Some(new_nil) = data.interface.variant_new_nil {
            // SAFETY: Godot supplied uninitialized Variant return storage.
            unsafe { new_nil(output) };
        }
    }
    if !call_error.is_null() {
        // SAFETY: Godot supplied a writable call-error pointer.
        unsafe {
            (*call_error).error = GDExtensionCallErrorType::GDEXTENSION_CALL_OK;
            (*call_error).argument = 0;
            (*call_error).expected = 0;
        }
    }
}

unsafe extern "C" fn signal_wait_is_valid(userdata: *mut core::ffi::c_void) -> GDExtensionBool {
    if userdata.is_null() {
        return 0;
    }
    // SAFETY: The custom Callable owns this exact userdata type.
    let data = unsafe { &*userdata.cast::<SignalWaitCallableData>() };
    u8::from(data.state.active.load(Ordering::Acquire))
}

unsafe extern "C" fn signal_wait_free(userdata: *mut core::ffi::c_void) {
    if !userdata.is_null() {
        // SAFETY: `from_signal_waiter` transfers one Box allocation to Godot,
        // which invokes this callback exactly once for the shared custom
        // Callable data.
        unsafe { drop(Box::from_raw(userdata.cast::<SignalWaitCallableData>())) };
    }
}

unsafe extern "C" fn signal_wait_hash(userdata: *mut core::ffi::c_void) -> u32 {
    if userdata.is_null() {
        return 0;
    }
    // SAFETY: The custom Callable owns this exact userdata type.
    let identity = unsafe { (*userdata.cast::<SignalWaitCallableData>()).identity };
    (identity as u32) ^ ((identity >> 32) as u32)
}

unsafe extern "C" fn signal_wait_equal(
    left: *mut core::ffi::c_void,
    right: *mut core::ffi::c_void,
) -> GDExtensionBool {
    if left.is_null() || right.is_null() {
        return u8::from(ptr::eq(left, right));
    }
    // SAFETY: Godot only compares userdata from Callables carrying the same
    // custom token, so both pointers have this exact layout.
    let left = unsafe { (*left.cast::<SignalWaitCallableData>()).identity };
    // SAFETY: See above.
    let right = unsafe { (*right.cast::<SignalWaitCallableData>()).identity };
    u8::from(left == right)
}

unsafe extern "C" fn signal_wait_argument_count(
    _userdata: *mut core::ffi::c_void,
    is_valid: *mut GDExtensionBool,
) -> GDExtensionInt {
    if !is_valid.is_null() {
        // A Signal may have any declared argument list. Reporting an unknown
        // count lets Godot forward it without rejecting a valid connection.
        // SAFETY: Godot supplied writable validity storage.
        unsafe { *is_valid = 0 };
    }
    0
}

impl Drop for NativeCallable {
    fn drop(&mut self) {
        if !self.initialized {
            return;
        }
        // SAFETY: This wrapper owns one initialized Callable.
        unsafe { (self.destroy)(self.as_mut_ptr()) };
    }
}

fn constructor(interface: EngineInterface, index: i32) -> Result<CallableConstructor, ValueError> {
    let get = interface
        .variant_get_ptr_constructor
        .ok_or_else(|| internal("Godot builtin constructors are unavailable"))?;
    // SAFETY: Callable and the constructor indices are authenticated.
    unsafe { get(callable_type(), index) }
        .ok_or_else(|| internal("Godot Callable constructor is unavailable"))
}

fn destructor(interface: EngineInterface) -> Result<CallableDestructor, ValueError> {
    let get = interface
        .variant_get_ptr_destructor
        .ok_or_else(|| internal("Godot builtin destructors are unavailable"))?;
    // SAFETY: Callable is an official builtin type.
    unsafe { get(callable_type()) }
        .ok_or_else(|| internal("Godot Callable destructor is unavailable"))
}

fn builtin_method(
    interface: EngineInterface,
    name: &'static core::ffi::CStr,
    hash: i64,
) -> Result<CallableBuiltinMethod, ValueError> {
    let get = interface
        .variant_get_ptr_builtin_method
        .ok_or_else(|| internal("Godot builtin method lookup is unavailable"))?;
    let name = crate::string_name::StaticStringName::new(interface, name);
    // SAFETY: Name and hash come from the authenticated Callable API.
    unsafe { get(callable_type(), name.as_ptr(), hash) }
        .ok_or_else(|| internal("Godot Callable builtin method is unavailable"))
}

const fn callable_type() -> GDExtensionVariantType {
    GDExtensionVariantType::GDEXTENSION_VARIANT_TYPE_CALLABLE
}

fn invalid(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::InvalidArgument, message)
}

fn unsupported(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Unsupported, message)
}

fn internal(message: &'static str) -> ValueError {
    ValueError::new(AbiStatus::Internal, message)
}
