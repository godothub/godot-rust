extern crate alloc;

use alloc::{boxed::Box, string::String};
use core::cell::OnceCell;
use core::marker::PhantomData;

use godot_api::abi::{
    ABI_SIGNAL_MAGIC, ABI_SIGNAL_VERSION, AbiStatus, AbiValueType, AbiValueV1,
    validate_signal_value,
};

use crate::callable::{Callable, CallableMethod};
use crate::engine::{GodotClass, GodotRef, Object, ObjectRef};
use crate::error::{EngineError, EngineResult};
use crate::math::{
    Aabb, Basis, Color, Plane, Projection, Quaternion, Rect2, Rect2i, Transform2D, Transform3D,
    Vector2, Vector2i, Vector3, Vector3i, Vector4, Vector4i,
};
use crate::node_path::NodePath;
use crate::packed_array::{
    PackedByteArray, PackedColorArray, PackedFloat32Array, PackedFloat64Array, PackedInt32Array,
    PackedInt64Array, PackedStringArray, PackedVector2Array, PackedVector3Array,
    PackedVector4Array,
};
use crate::rid::Rid;
use crate::string_name::StringName;
use crate::variant::{Array, Dictionary, Variant, VariantConvert, VariantError};

const MAX_SIGNAL_ARGUMENTS: usize = 8;
const SIGNAL_HEADER_BYTES: usize = 24;

#[doc(hidden)]
pub enum SignalBacking {
    Text(String),
    Math6([f32; 6]),
    Math9([f32; 9]),
    Math12([f32; 12]),
    Math16([f32; 16]),
    Packed(Box<[u8]>),
    Signal(Box<[u8]>),
    Dynamic(Variant),
    Callable(Callable),
    Error(&'static str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SignalSource {
    Unbound,
    ScriptField {
        field_index: u32,
        name: &'static str,
    },
    ScriptObject {
        object: ObjectRef<Object>,
        name: &'static str,
    },
    Godot {
        object: ObjectRef<Object>,
        name: StringName,
    },
}

/// Failure while encoding a Godot Signal for the stable project ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub struct SignalError {
    message: &'static str,
}

impl SignalError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        self.message
    }
}

type SignalWireCache = OnceCell<Result<Box<[u8]>, SignalError>>;

/// A typed Godot signal.
///
/// Declare script fields with `Signal<fn(amount: i64)>` and emit with
/// `signal.emit(10)?`. Tuple signatures such as `Signal<(i64,)>` remain
/// supported. Untyped engine handles can be created with [`Signal::from_object`].
pub struct Signal<T = ()> {
    source: SignalSource,
    encoded: SignalWireCache,
    marker: PhantomData<fn(T)>,
}

impl<T> Signal<T> {
    /// Creates a signal bound to one generated field descriptor.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(field_index: u32, name: &'static str) -> Self {
        Self {
            source: SignalSource::ScriptField { field_index, name },
            encoded: OnceCell::new(),
            marker: PhantomData,
        }
    }

    /// Creates a typed handle for one signal declared by the official engine
    /// API.
    #[doc(hidden)]
    #[must_use]
    pub fn __from_object<C: GodotClass>(object: ObjectRef<C>, name: impl Into<StringName>) -> Self {
        Self {
            source: SignalSource::Godot {
                object: ObjectRef::__from_instance_id(object.instance_id()),
                name: name.into(),
            },
            encoded: OnceCell::new(),
            marker: PhantomData,
        }
    }

    /// Creates an allocation-free handle for a generated Rust script signal.
    #[doc(hidden)]
    #[must_use]
    pub fn __from_script<C: GodotClass>(object: ObjectRef<C>, name: &'static str) -> Self {
        Self {
            source: SignalSource::ScriptObject {
                object: ObjectRef::__from_instance_id(object.instance_id()),
                name,
            },
            encoded: OnceCell::new(),
            marker: PhantomData,
        }
    }

    /// Captures this signal's current owner for use in a task or another callback.
    ///
    /// Call this on a script field during its owning script callback. The
    /// returned handle keeps the object identity, so later use cannot bind to
    /// a different script that happens to be executing.
    pub fn bind(&self) -> EngineResult<Self> {
        let object = self.object_ref()?;
        match &self.source {
            SignalSource::ScriptField { name, .. } => Ok(Self::__from_script(object, name)),
            _ => Ok(self.clone()),
        }
    }

    /// Signal name as Godot sees it.
    #[must_use]
    pub fn name(&self) -> &str {
        match &self.source {
            SignalSource::Unbound => "",
            SignalSource::ScriptField { name, .. } | SignalSource::ScriptObject { name, .. } => {
                name
            }
            SignalSource::Godot { name, .. } => name.as_str(),
        }
    }

    /// Resolves the object that owns this signal.
    pub fn object_ref(&self) -> EngineResult<ObjectRef<Object>> {
        match &self.source {
            SignalSource::Unbound => Err(EngineError::unavailable("Godot Signal is unbound")),
            SignalSource::ScriptField { .. } => crate::engine::current_object(),
            SignalSource::Godot { object, .. } | SignalSource::ScriptObject { object, .. }
                if object.is_resolved() =>
            {
                Ok(*object)
            }
            SignalSource::Godot { .. } | SignalSource::ScriptObject { .. } => Err(
                EngineError::unavailable("Godot Signal target is unresolved"),
            ),
        }
    }

    /// Connects this signal to a Callable through Godot's ordinary Object API.
    pub fn connect(
        &self,
        callable: &Callable,
        flags: u32,
    ) -> EngineResult<crate::engine::global::Error> {
        crate::engine::ObjectApi::connect(&self.object_ref()?, self.name(), callable, flags)
    }

    /// Disconnects a Callable from this signal.
    pub fn disconnect(&self, callable: &Callable) -> EngineResult<()> {
        crate::engine::ObjectApi::disconnect(&self.object_ref()?, self.name(), callable)
    }

    /// Whether this signal is connected to a Callable.
    pub fn is_connected(&self, callable: &Callable) -> EngineResult<bool> {
        crate::engine::ObjectApi::is_connected(&self.object_ref()?, self.name(), callable)
    }

    /// Emits an engine signal using dynamic Godot arguments.
    pub fn emit_variants(
        &self,
        arguments: &[Variant],
    ) -> EngineResult<crate::engine::global::Error> {
        crate::engine::ObjectApi::emit_signal(&self.object_ref()?, self.name(), arguments)
    }

    /// Waits for the next emission and returns all declared arguments as a tuple.
    ///
    /// The connection starts when the future is first polled. Dropping the
    /// future disconnects it. Use [`Self::wait_untyped`] for a dynamically
    /// obtained signal whose argument signature is not known.
    pub fn wait(&self) -> crate::task::SignalFuture<T::Arguments>
    where
        T: SignalSignature,
        T::Arguments: SignalResult,
    {
        crate::task::SignalFuture::new(self)
    }

    /// Waits for an emission without inspecting its arguments.
    pub fn wait_untyped(&self) -> crate::task::SignalFuture {
        crate::task::SignalFuture::new_untyped(self)
    }

    /// Connects a generated method with the same parameter types.
    ///
    /// Godot calls the target synchronously. A callback that tries to mutably
    /// re-enter its currently executing Rust script is rejected by the runtime.
    pub fn connect_method(&self, method: &CallableMethod<T::Arguments>) -> EngineResult<()>
    where
        T: SignalSignature,
    {
        self.connect_checked(method.as_callable(), 0)
    }

    /// Connects a generated method for invocation at idle time.
    pub fn connect_deferred(&self, method: &CallableMethod<T::Arguments>) -> EngineResult<()>
    where
        T: SignalSignature,
    {
        self.connect_checked(method.as_callable(), 1)
    }

    /// Connects until the returned guard is dropped or explicitly disconnected.
    ///
    /// Store the guard in your script to make the connection belong to that
    /// script instance. The guard preserves ownership already held by the
    /// Callable. Object-based targets, including generated script callbacks,
    /// remain non-owning and Godot disconnects them when the target is freed.
    pub fn connect_scoped(
        &self,
        method: &CallableMethod<T::Arguments>,
    ) -> EngineResult<SignalConnection>
    where
        T: SignalSignature,
    {
        self.connect_scoped_with(method, 0)
    }

    /// Creates a connection with explicit idle-time delivery and scoped lifetime.
    pub fn connect_deferred_scoped(
        &self,
        method: &CallableMethod<T::Arguments>,
    ) -> EngineResult<SignalConnection>
    where
        T: SignalSignature,
    {
        self.connect_scoped_with(method, 1)
    }

    fn connect_checked(&self, callable: &Callable, flags: u32) -> EngineResult<()> {
        let error = self.connect(callable, flags)?;
        if error == crate::engine::global::Error::OK {
            Ok(())
        } else {
            let message = format!(
                "Godot rejected signal connection {} -> {}: {error:?}",
                self.name(),
                callable.method()
            );
            Err(EngineError::from_abi(godot_api::abi::AbiCallResult {
                status: AbiStatus::CallbackFailed,
                message: godot_api::abi::AbiByteSlice {
                    ptr: message.as_ptr(),
                    len: message.len(),
                },
            }))
        }
    }

    fn connect_scoped_with(
        &self,
        method: &CallableMethod<T::Arguments>,
        flags: u32,
    ) -> EngineResult<SignalConnection>
    where
        T: SignalSignature,
    {
        let signal = Signal::from_object(self.object_ref()?, self.name());
        signal.connect_checked(method.as_callable(), flags)?;
        Ok(SignalConnection {
            signal,
            callable: method.as_callable().clone(),
            active: true,
        })
    }

    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], SignalError> {
        self.encoded
            .get_or_init(|| self.encode().map(Vec::into_boxed_slice))
            .as_deref()
            .map_err(|error| *error)
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        if !validate_signal_value(bytes) {
            return None;
        }
        let object_id = u64::from_le_bytes(bytes[12..20].try_into().ok()?);
        let name = core::str::from_utf8(&bytes[SIGNAL_HEADER_BYTES..]).ok()?;
        Some(if object_id == 0 {
            Self::default()
        } else {
            Self {
                source: SignalSource::Godot {
                    object: ObjectRef::__from_instance_id(object_id),
                    name: StringName::from(name),
                },
                encoded: OnceCell::new(),
                marker: PhantomData,
            }
        })
    }

    fn encode(&self) -> Result<Vec<u8>, SignalError> {
        let object_id = match &self.source {
            SignalSource::Unbound => 0,
            SignalSource::ScriptField { .. } => crate::engine::current_object::<Object>()
                .map_err(|_| {
                    SignalError::new(
                        "a script-field Signal can only be used during a script callback",
                    )
                })?
                .instance_id(),
            SignalSource::Godot { object, .. } | SignalSource::ScriptObject { object, .. } => {
                object.instance_id()
            }
        };
        let name = self.name();
        if object_id == 0 && !name.is_empty() {
            return Err(SignalError::new("Godot Signal target is unresolved"));
        }
        if object_id != 0 && name.is_empty() {
            return Err(SignalError::new("Godot Signal name is empty"));
        }
        let name_length = u32::try_from(name.len())
            .map_err(|_| SignalError::new("Godot Signal name is too large"))?;
        let mut bytes = Vec::with_capacity(SIGNAL_HEADER_BYTES + name.len());
        bytes.extend_from_slice(&ABI_SIGNAL_MAGIC);
        bytes.extend_from_slice(&ABI_SIGNAL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&object_id.to_le_bytes());
        bytes.extend_from_slice(&name_length.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        validate_signal_value(&bytes)
            .then_some(bytes)
            .ok_or_else(|| SignalError::new("Godot Signal produced an invalid ABI payload"))
    }

    fn emit_arguments<A: SignalArguments>(&self, arguments: A) -> EngineResult<()> {
        let mut encoded = [AbiValueV1::NIL; MAX_SIGNAL_ARGUMENTS];
        let mut backing: [Option<SignalBacking>; MAX_SIGNAL_ARGUMENTS] =
            core::array::from_fn(|_| None);
        let argument_count = arguments.encode(&mut encoded, &mut backing);
        if let Some(message) = backing.iter().find_map(|value| match value {
            Some(SignalBacking::Error(message)) => Some(*message),
            _ => None,
        }) {
            return Err(EngineError::invalid_argument(message));
        }
        let arguments = &encoded[..argument_count];
        if let SignalSource::ScriptField { field_index, .. } = &self.source {
            let result = crate::module::emit_signal(*field_index, arguments);
            return if result.status == AbiStatus::Ok {
                Ok(())
            } else {
                Err(EngineError::from_abi(result))
            };
        }
        crate::engine::emit_signal_arguments(self.object_ref()?, self.name(), arguments)
    }
}

impl Signal<()> {
    /// Creates a handle to a named signal on a Godot object.
    #[must_use]
    pub fn from_object<C: GodotClass>(object: ObjectRef<C>, name: impl Into<StringName>) -> Self {
        Self::__from_object(object, name)
    }
}

/// A connection that disconnects when its owning Rust value is dropped.
#[must_use = "dropping the connection guard disconnects the signal"]
#[derive(Debug)]
pub struct SignalConnection {
    signal: Signal,
    callable: Callable,
    active: bool,
}

impl SignalConnection {
    /// Disconnects now. Calling this again is harmless.
    pub fn disconnect(&mut self) -> EngineResult<()> {
        if self.active {
            if self.signal.is_connected(&self.callable)? {
                self.signal.disconnect(&self.callable)?;
            }
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for SignalConnection {
    fn drop(&mut self) {
        // A target may already have been freed or disconnected by Godot.
        // Explicit disconnect remains fallible for callers that need diagnostics.
        let _ = self.disconnect();
    }
}

/// Argument tuple described by a tuple or a Rust function signature.
#[doc(hidden)]
pub trait SignalSignature {
    type Arguments;
}

impl SignalSignature for () {
    type Arguments = ();
}

impl SignalSignature for fn() {
    type Arguments = ();
}

impl Signal<()> {
    /// Emits a signal declared with the legacy empty tuple signature.
    pub fn emit(&self, arguments: ()) -> EngineResult<()> {
        self.emit_arguments(arguments)
    }
}

impl Signal<fn()> {
    /// Emits this signal synchronously.
    pub fn emit(&self) -> EngineResult<()> {
        self.emit_arguments(())
    }
}

/// Decodes a signal emission without discarding its parameters.
#[doc(hidden)]
pub trait SignalResult: Sized {
    fn from_signal_arguments(arguments: Vec<Variant>) -> EngineResult<Self>;
}

impl SignalResult for () {
    fn from_signal_arguments(arguments: Vec<Variant>) -> EngineResult<Self> {
        if arguments.is_empty() {
            Ok(())
        } else {
            Err(EngineError::invalid_argument(
                "signal emitted an unexpected argument count",
            ))
        }
    }
}

macro_rules! impl_typed_signal {
    ($count:expr; $($type_:ident:$value:ident),+ $(,)?) => {
        impl<$($type_),+> SignalSignature for ($($type_,)+) {
            type Arguments = ($($type_,)+);
        }

        impl<$($type_),+> SignalSignature for fn($($type_),+) {
            type Arguments = ($($type_,)+);
        }

        impl<$($type_: SignalValue),+> Signal<($($type_,)+)> {
            /// Emits this signal synchronously using the legacy tuple signature.
            pub fn emit(&self, arguments: ($($type_,)+)) -> EngineResult<()> {
                self.emit_arguments(arguments)
            }
        }

        impl<$($type_: SignalValue),+> Signal<fn($($type_),+)> {
            /// Emits this signal synchronously with type-checked arguments.
            #[allow(clippy::too_many_arguments)]
            pub fn emit(&self, $($value: $type_),+) -> EngineResult<()> {
                self.emit_arguments(($($value,)+))
            }
        }

        impl<$($type_: VariantConvert),+> SignalResult for ($($type_,)+) {
            fn from_signal_arguments(arguments: Vec<Variant>) -> EngineResult<Self> {
                if arguments.len() != $count {
                    return Err(EngineError::invalid_argument(
                        "signal emitted an unexpected argument count",
                    ));
                }
                let mut arguments = arguments.into_iter();
                Ok(($({
                    let value = arguments.next().expect("signal argument count was checked");
                    $type_::from_variant(value).ok_or_else(|| EngineError::invalid_argument(
                        format!("signal argument has an incompatible type; expected {}",
                            core::any::type_name::<$type_>()),
                    ))?
                },)+))
            }
        }
    };
}

impl_typed_signal!(1; A:a);
impl_typed_signal!(2; A:a, B:b);
impl_typed_signal!(3; A:a, B:b, C:c);
impl_typed_signal!(4; A:a, B:b, C:c, D:d);
impl_typed_signal!(5; A:a, B:b, C:c, D:d, E:e);
impl_typed_signal!(6; A:a, B:b, C:c, D:d, E:e, F:f);
impl_typed_signal!(7; A:a, B:b, C:c, D:d, E:e, F:f, G:g);
impl_typed_signal!(8; A:a, B:b, C:c, D:d, E:e, F:f, G:g, H:h);

impl<T> Default for Signal<T> {
    fn default() -> Self {
        Self {
            source: SignalSource::Unbound,
            encoded: OnceCell::new(),
            marker: PhantomData,
        }
    }
}

impl<T> Clone for Signal<T> {
    /// Script fields capture the callback's owner. Cloning a field outside a
    /// script callback creates an unresolved handle; use `bind()` to report
    /// that error immediately.
    fn clone(&self) -> Self {
        let source = match &self.source {
            SignalSource::ScriptField { name, .. } => SignalSource::Godot {
                object: self
                    .object_ref()
                    .unwrap_or_else(|_| ObjectRef::unresolved()),
                name: StringName::from(*name),
            },
            source => source.clone(),
        };
        Self {
            source,
            encoded: OnceCell::new(),
            marker: PhantomData,
        }
    }
}

impl<T> PartialEq for Signal<T> {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl<T> Eq for Signal<T> {}

impl<T> core::fmt::Debug for Signal<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Signal")
            .field("source", &self.source)
            .finish()
    }
}

/// Converts one supported signal argument into the fixed project ABI.
#[doc(hidden)]
pub trait SignalValue {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1;
}

impl<T: crate::engine::GodotIntegerValue> SignalValue for T {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_i64(self.__raw() as i64)
    }
}

impl<T: GodotClass> SignalValue for ObjectRef<T> {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_object_id(self.instance_id())
    }
}

impl<T: GodotClass> SignalValue for Option<ObjectRef<T>> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        self.unwrap_or_else(ObjectRef::unresolved)
            .into_signal_value(backing)
    }
}

impl<T: GodotClass> SignalValue for GodotRef<T> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        let value = AbiValueV1::from_object_id(self.instance_id());
        *backing = Some(SignalBacking::Dynamic(Variant::from(self)));
        value
    }
}

impl<T: GodotClass> SignalValue for Option<GodotRef<T>> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        self.map_or_else(
            || AbiValueV1::from_object_id(0),
            |value| value.into_signal_value(backing),
        )
    }
}

impl SignalValue for Callable {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        *backing = Some(SignalBacking::Callable(self));
        let Some(SignalBacking::Callable(callable)) = backing.as_ref() else {
            unreachable!("Callable signal backing was installed")
        };
        match callable.__bytes() {
            Ok(bytes) => AbiValueV1::from_borrowed_bytes(AbiValueType::CALLABLE, bytes),
            Err(error) => {
                *backing = Some(SignalBacking::Error(error.message()));
                AbiValueV1::NIL
            }
        }
    }
}

impl SignalValue for bool {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_bool(self)
    }
}

impl SignalValue for i32 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_i64(i64::from(self))
    }
}

impl SignalValue for i64 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_i64(self)
    }
}

impl SignalValue for f32 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_f64(f64::from(self))
    }
}

impl SignalValue for f64 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_f64(self)
    }
}

impl SignalValue for String {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        *backing = Some(SignalBacking::Text(self));
        let Some(SignalBacking::Text(text)) = backing.as_ref() else {
            unreachable!("String signal backing was installed")
        };
        AbiValueV1::from_borrowed_utf8(text)
    }
}

impl SignalValue for &str {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_borrowed_utf8(self)
    }
}

impl SignalValue for StringName {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        *backing = Some(SignalBacking::Text(self.into_string()));
        let Some(SignalBacking::Text(text)) = backing.as_ref() else {
            unreachable!("StringName signal backing was installed")
        };
        AbiValueV1::from_borrowed_string_name(text)
    }
}

impl SignalValue for &StringName {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_borrowed_string_name(self.as_str())
    }
}

impl SignalValue for NodePath {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        *backing = Some(SignalBacking::Text(self.into_string()));
        let Some(SignalBacking::Text(text)) = backing.as_ref() else {
            unreachable!("NodePath signal backing was installed")
        };
        AbiValueV1::from_borrowed_node_path(text)
    }
}

impl SignalValue for &NodePath {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_borrowed_node_path(self.as_str())
    }
}

impl SignalValue for Vector2 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_vector2(self.x, self.y)
    }
}

impl SignalValue for Vector3 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_vector3(self.x, self.y, self.z)
    }
}

impl SignalValue for Color {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_color(self.r, self.g, self.b, self.a)
    }
}

impl SignalValue for Vector2i {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_vector2i(self.x, self.y)
    }
}

impl SignalValue for Vector3i {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_vector3i(self.x, self.y, self.z)
    }
}

impl SignalValue for Rect2 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_rect2(self.position.x, self.position.y, self.size.x, self.size.y)
    }
}

impl SignalValue for Rect2i {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_rect2i(self.position.x, self.position.y, self.size.x, self.size.y)
    }
}

impl SignalValue for Quaternion {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_quaternion(self.x, self.y, self.z, self.w)
    }
}

impl SignalValue for Plane {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_plane(self.normal.x, self.normal.y, self.normal.z, self.d)
    }
}

impl SignalValue for Vector4 {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_vector4(self.x, self.y, self.z, self.w)
    }
}

impl SignalValue for Vector4i {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_vector4i(self.x, self.y, self.z, self.w)
    }
}

macro_rules! fixed_math_signal_value {
    ($type:ty, $abi_type:ident, $backing:ident) => {
        impl SignalValue for $type {
            fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
                *backing = Some(SignalBacking::$backing(
                    self.__components()
                        .try_into()
                        .expect("fixed-size math layout"),
                ));
                let Some(SignalBacking::$backing(components)) = backing.as_ref() else {
                    unreachable!("fixed-size math signal backing was installed")
                };
                AbiValueV1::from_borrowed_f32_components(
                    godot_api::abi::AbiValueType::$abi_type,
                    components,
                )
            }
        }
    };
}

fixed_math_signal_value!(Transform2D, TRANSFORM2D, Math6);
fixed_math_signal_value!(Aabb, AABB, Math6);
fixed_math_signal_value!(Basis, BASIS, Math9);
fixed_math_signal_value!(Transform3D, TRANSFORM3D, Math12);
fixed_math_signal_value!(Projection, PROJECTION, Math16);

macro_rules! packed_signal_value {
    ($type:ty, $abi_type:ident) => {
        impl SignalValue for $type {
            fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
                *backing = Some(SignalBacking::Packed(
                    self.__bytes().to_vec().into_boxed_slice(),
                ));
                let Some(SignalBacking::Packed(bytes)) = backing.as_ref() else {
                    unreachable!("packed-array signal backing was installed")
                };
                AbiValueV1::from_borrowed_bytes(AbiValueType::$abi_type, bytes)
            }
        }
    };
}

packed_signal_value!(PackedByteArray, PACKED_BYTE_ARRAY);
packed_signal_value!(PackedInt32Array, PACKED_INT32_ARRAY);
packed_signal_value!(PackedInt64Array, PACKED_INT64_ARRAY);
packed_signal_value!(PackedFloat32Array, PACKED_FLOAT32_ARRAY);
packed_signal_value!(PackedFloat64Array, PACKED_FLOAT64_ARRAY);
packed_signal_value!(PackedStringArray, PACKED_STRING_ARRAY);
packed_signal_value!(PackedVector2Array, PACKED_VECTOR2_ARRAY);
packed_signal_value!(PackedVector3Array, PACKED_VECTOR3_ARRAY);
packed_signal_value!(PackedColorArray, PACKED_COLOR_ARRAY);
packed_signal_value!(PackedVector4Array, PACKED_VECTOR4_ARRAY);

fn borrowed_dynamic_signal_value(
    value_type: AbiValueType,
    bytes: Result<&[u8], VariantError>,
    backing: &mut Option<SignalBacking>,
) -> AbiValueV1 {
    match bytes {
        Ok(bytes) => AbiValueV1::from_borrowed_bytes(value_type, bytes),
        Err(error) => {
            *backing = Some(SignalBacking::Error(error.message()));
            AbiValueV1::NIL
        }
    }
}

fn owned_dynamic_signal_value(
    value_type: AbiValueType,
    value: Variant,
    backing: &mut Option<SignalBacking>,
) -> AbiValueV1 {
    *backing = Some(SignalBacking::Dynamic(value));
    let Some(SignalBacking::Dynamic(value)) = backing.as_ref() else {
        unreachable!("dynamic signal ownership was installed")
    };
    match value.__bytes() {
        Ok(bytes) => AbiValueV1::from_borrowed_bytes(value_type, bytes),
        Err(error) => {
            *backing = Some(SignalBacking::Error(error.message()));
            AbiValueV1::NIL
        }
    }
}

impl SignalValue for Variant {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        owned_dynamic_signal_value(AbiValueType::VARIANT, self, backing)
    }
}

impl SignalValue for &Variant {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        borrowed_dynamic_signal_value(AbiValueType::VARIANT, self.__bytes(), backing)
    }
}

impl<T: VariantConvert> SignalValue for Array<T> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        owned_dynamic_signal_value(AbiValueType::ARRAY, Variant::from(self), backing)
    }
}

impl<T: VariantConvert> SignalValue for &Array<T> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        borrowed_dynamic_signal_value(AbiValueType::ARRAY, self.__bytes(), backing)
    }
}

impl SignalValue for Dictionary {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        owned_dynamic_signal_value(AbiValueType::DICTIONARY, Variant::from(self), backing)
    }
}

impl SignalValue for &Dictionary {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        borrowed_dynamic_signal_value(AbiValueType::DICTIONARY, self.__bytes(), backing)
    }
}

impl SignalValue for Rid {
    fn into_signal_value(self, _backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        AbiValueV1::from_rid(self.id())
    }
}

fn native_signal_value<T>(signal: &Signal<T>, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
    match signal.__bytes() {
        Ok(bytes) => {
            *backing = Some(SignalBacking::Signal(bytes.to_vec().into_boxed_slice()));
            let Some(SignalBacking::Signal(bytes)) = backing.as_ref() else {
                unreachable!("Signal backing was installed")
            };
            AbiValueV1::from_borrowed_bytes(AbiValueType::SIGNAL, bytes)
        }
        Err(error) => {
            *backing = Some(SignalBacking::Error(error.message()));
            AbiValueV1 {
                type_: AbiValueType::SIGNAL,
                reserved_flags: 0,
                payload: [0; 2],
            }
        }
    }
}

impl<T> SignalValue for Signal<T> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        native_signal_value(&self, backing)
    }
}

impl<T> SignalValue for &Signal<T> {
    fn into_signal_value(self, backing: &mut Option<SignalBacking>) -> AbiValueV1 {
        native_signal_value(self, backing)
    }
}

/// Encodes a signal argument tuple without allocating for scalar values.
#[doc(hidden)]
pub trait SignalArguments {
    fn encode(
        self,
        output: &mut [AbiValueV1; MAX_SIGNAL_ARGUMENTS],
        backing: &mut [Option<SignalBacking>; MAX_SIGNAL_ARGUMENTS],
    ) -> usize;
}

impl SignalArguments for () {
    fn encode(
        self,
        _output: &mut [AbiValueV1; MAX_SIGNAL_ARGUMENTS],
        _backing: &mut [Option<SignalBacking>; MAX_SIGNAL_ARGUMENTS],
    ) -> usize {
        0
    }
}

macro_rules! impl_signal_arguments {
    ($count:expr; $($type_:ident:$value:ident:$index:tt),+ $(,)?) => {
        impl<$($type_: SignalValue),+> SignalArguments for ($($type_,)+) {
            fn encode(
                self,
                output: &mut [AbiValueV1; MAX_SIGNAL_ARGUMENTS],
                backing: &mut [Option<SignalBacking>; MAX_SIGNAL_ARGUMENTS],
            ) -> usize {
                let ($($value,)+) = self;
                $(output[$index] =
                    $value.into_signal_value(&mut backing[$index]);)+
                $count
            }
        }
    };
}

impl_signal_arguments!(1; A:a:0);
impl_signal_arguments!(2; A:a:0, B:b:1);
impl_signal_arguments!(3; A:a:0, B:b:1, C:c:2);
impl_signal_arguments!(4; A:a:0, B:b:1, C:c:2, D:d:3);
impl_signal_arguments!(5; A:a:0, B:b:1, C:c:2, D:d:3, E:e:4);
impl_signal_arguments!(6; A:a:0, B:b:1, C:c:2, D:d:3, E:e:4, F:f:5);
impl_signal_arguments!(7; A:a:0, B:b:1, C:c:2, D:d:3, E:e:4, F:f:5, G:g:6);
impl_signal_arguments!(8; A:a:0, B:b:1, C:c:2, D:d:3, E:e:4, F:f:5, G:g:6, H:h:7);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_and_tuple_signatures_report_emit_failures() {
        let function = Signal::<fn(amount: i64, label: String)>::new(0, "changed");
        let tuple = Signal::<(i64, String)>::new(0, "changed");
        assert!(function.emit(12, "hit".to_owned()).is_err());
        assert!(tuple.emit((12, "hit".to_owned())).is_err());
        assert!(Signal::<fn()>::default().emit().is_err());
        assert!(Signal::<()>::default().emit(()).is_err());
    }

    #[test]
    fn typed_wait_arguments_preserve_values_and_reject_mismatches() {
        let values = vec![Variant::from(7_i64), Variant::from("你好")];
        assert_eq!(
            <(i64, String)>::from_signal_arguments(values).unwrap(),
            (7, "你好".into())
        );
        assert!(<(i64,)>::from_signal_arguments(vec![]).is_err());
        assert!(<(i64,)>::from_signal_arguments(vec![Variant::from("wrong")]).is_err());
        assert!(<()>::from_signal_arguments(vec![Variant::from(1_i64)]).is_err());
        assert!(<(i32,)>::from_signal_arguments(vec![Variant::from(i64::MAX)]).is_err());
    }

    #[test]
    fn owned_and_borrowed_signal_strings_stay_live_for_the_call() {
        let mut output = [AbiValueV1::NIL; MAX_SIGNAL_ARGUMENTS];
        let mut backing = core::array::from_fn(|_| None);
        let count = (String::from("你好"), "Godot").encode(&mut output, &mut backing);
        assert_eq!(count, 2);
        assert_eq!(output[0].reserved_flags, 0);
        assert_eq!(output[1].reserved_flags, 0);
        assert!(matches!(
            backing[0].as_ref(),
            Some(SignalBacking::Text(text)) if text == "你好"
        ));
        assert_eq!(abi_text(output[0]), "你好");
        assert_eq!(abi_text(output[1]), "Godot");

        let name = StringName::from("玩家");
        let count = (name.clone(), &name).encode(&mut output, &mut backing);
        assert_eq!(count, 2);
        assert_eq!(output[0].type_, godot_api::abi::AbiValueType::STRING_NAME);
        assert_eq!(output[1].type_, godot_api::abi::AbiValueType::STRING_NAME);
        assert_eq!(abi_text(output[0]), "玩家");
        assert_eq!(abi_text(output[1]), "玩家");
    }

    #[test]
    fn math_signal_values_are_all_inline() {
        let mut output = [AbiValueV1::NIL; MAX_SIGNAL_ARGUMENTS];
        let mut backing = core::array::from_fn(|_| None);
        let count = (
            Vector2::new(1.0, 2.0),
            Vector3::new(3.0, 4.0, 5.0),
            Color::rgba(0.1, 0.2, 0.3, 0.4),
            Vector2i::new(-1, 2),
            Vector3i::new(3, -4, 5),
            Rid::INVALID,
        )
            .encode(&mut output, &mut backing);
        assert_eq!(count, 6);
        assert_eq!(output[0].vector2(), Some([1.0, 2.0]));
        assert_eq!(output[1].vector3(), Some([3.0, 4.0, 5.0]));
        assert_eq!(output[2].color(), Some([0.1, 0.2, 0.3, 0.4]));
        assert_eq!(output[3].vector2i(), Some([-1, 2]));
        assert_eq!(output[4].vector3i(), Some([3, -4, 5]));
        assert_eq!(output[5].rid(), Some(0));
        assert!(backing.iter().all(Option::is_none));
    }

    #[test]
    fn large_math_signal_values_borrow_call_scoped_backing() {
        let mut output = [AbiValueV1::NIL; MAX_SIGNAL_ARGUMENTS];
        let mut backing = core::array::from_fn(|_| None);
        let count = (Transform3D::IDENTITY, Projection::IDENTITY).encode(&mut output, &mut backing);
        assert_eq!(count, 2);
        assert_eq!(output[0].reserved_flags, 0);
        assert_eq!(output[1].reserved_flags, 0);
        assert_eq!(
            abi_components(output[0]),
            Transform3D::IDENTITY.__components()
        );
        assert_eq!(
            abi_components(output[1]),
            Projection::IDENTITY.__components()
        );
        assert!(matches!(
            backing[0].as_ref(),
            Some(SignalBacking::Math12(_))
        ));
        assert!(matches!(
            backing[1].as_ref(),
            Some(SignalBacking::Math16(_))
        ));
    }

    #[test]
    fn packed_signal_values_retain_exact_call_scoped_bytes() {
        let values = PackedStringArray::from(vec!["你好".into(), "Godot".into()]);
        let expected = values.__bytes().to_vec();
        let mut output = [AbiValueV1::NIL; MAX_SIGNAL_ARGUMENTS];
        let mut backing = core::array::from_fn(|_| None);
        let count = (values,).encode(&mut output, &mut backing);
        assert_eq!(count, 1);
        assert_eq!(
            output[0].type_,
            godot_api::abi::AbiValueType::PACKED_STRING_ARRAY
        );
        assert_eq!(output[0].reserved_flags, 0);
        assert!(matches!(
            backing[0].as_ref(),
            Some(SignalBacking::Packed(bytes)) if bytes.as_ref() == expected
        ));
        let (pointer, length) = output[0]
            .byte_range(godot_api::abi::AbiValueType::PACKED_STRING_ARRAY)
            .expect("packed signal bytes");
        // SAFETY: The call-scoped backing is still retained above.
        let actual = unsafe { core::slice::from_raw_parts(pointer, length) };
        assert_eq!(actual, expected);
    }

    #[test]
    fn engine_signal_handles_round_trip_through_the_stable_wire() {
        let signal = Signal::from_object(
            ObjectRef::<crate::engine::Node>::__from_instance_id(42),
            "已完成",
        );
        let bytes = signal.__bytes().expect("engine Signal wire");
        assert!(validate_signal_value(bytes));
        assert_eq!(u64::from_le_bytes(bytes[12..20].try_into().unwrap()), 42);
        assert_eq!(&bytes[SIGNAL_HEADER_BYTES..], "已完成".as_bytes());
        assert_eq!(Signal::__from_bytes(bytes), Some(signal.clone()));
    }

    #[test]
    fn unbound_signal_is_the_canonical_empty_godot_signal() {
        let signal = Signal::<()>::default();
        let bytes = signal.__bytes().expect("empty Signal wire");
        assert!(validate_signal_value(bytes));
        assert_eq!(bytes.len(), SIGNAL_HEADER_BYTES);
        assert_eq!(Signal::__from_bytes(bytes), Some(signal.clone()));
        assert!(signal.object_ref().is_err());
    }

    fn abi_text(value: AbiValueV1) -> String {
        // SAFETY: The test calls this while owned or borrowed signal backing
        // remains live, and the values were generated from valid Rust UTF-8.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                value.payload[0] as usize as *const u8,
                value.payload[1] as usize,
            )
        };
        core::str::from_utf8(bytes)
            .expect("signal UTF-8")
            .to_owned()
    }

    fn abi_components(value: AbiValueV1) -> Vec<f32> {
        let (pointer, length) = value
            .byte_range(value.type_)
            .expect("signal component bytes");
        assert_eq!(length % core::mem::size_of::<f32>(), 0);
        // SAFETY: The signal backing keeps this exact byte range live.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        bytes
            .chunks_exact(core::mem::size_of::<f32>())
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().expect("f32 byte width")))
            .collect()
    }
}
