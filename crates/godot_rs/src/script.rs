extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::TypeId;
use core::ffi::c_void;
use core::ptr;

use crate::callable::Callable;
use crate::engine::{GodotClass, GodotRef, Inherits, Node, NodeRef, ObjectRef, Resource};
use crate::error::{EngineResult, ScriptResult, engine_error_callback_message};
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
use crate::signal::Signal;
use crate::string_name::StringName;
use crate::variant::{Array, Dictionary, Variant, VariantConvert};
use godot_api::abi::{
    ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA, ABI_FIELD_EXTENSION_NODE_SCHEMA,
    ABI_FIELD_EXTENSION_PROPERTY_SCHEMA, ABI_FIELD_EXTENSION_SIGNAL_SCHEMA,
    ABI_METHOD_EXTENSION_SCHEMA_V1, ABI_METHOD_SCHEMA_VARARG, ABI_SCRIPT_EXTENSION_FIELD_ACCESS,
    ABI_SCRIPT_EXTENSION_RESOURCE_UID, AbiByteSlice, AbiByteSliceSlice, AbiCallResult,
    AbiFieldDescriptorV1, AbiFieldKind, AbiFixedMathDefaultV1, AbiGodotIntegerDefaultFn,
    AbiGodotIntegerOptionV1, AbiLifecycleSlot, AbiLifecycleTableV1, AbiMethodArgumentDescriptorV1,
    AbiMethodArgumentSlice, AbiMethodDefaultFn, AbiMethodDefaultFnSlice, AbiMethodDescriptorV1,
    AbiMethodExtensionsV1, AbiMethodKind, AbiPropertyType, AbiReceiverKind, AbiReloadPolicy,
    AbiRpcConfigV1, AbiRpcMode, AbiRpcTransferMode, AbiScriptDescriptorV1,
    AbiSignalArgumentDescriptorV1, AbiStatus, AbiValueType, AbiValueTypeSlice, AbiValueV1,
    encode_node_field_class, encode_resource_uid_words,
};

const MAX_UTF8_VALUE_BYTES: usize = 64 * 1024 * 1024;

/// How one field participates in Godot and Host state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Plain,
    Export,
    Node,
    Signal,
}

/// State policy applied when switching project-module generations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadPolicy {
    Default,
    Persist,
    Skip,
}

/// Normalized Godot Inspector metadata generated from `#[export(...)]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PropertyDescriptor {
    pub type_: AbiPropertyType,
    pub hint: u32,
    pub hint_string: &'static str,
    pub typed_array_element: Option<&'static str>,
    #[doc(hidden)]
    pub integer_options: Option<&'static [AbiGodotIntegerOptionV1]>,
    pub usage: u32,
    pub group: Option<&'static str>,
    pub default_value: Option<PropertyDefault>,
    #[doc(hidden)]
    pub encoded: &'static str,
}

/// Typed Inspector default stored in generated static script metadata.
#[derive(Clone, Copy, Debug)]
pub enum PropertyDefault {
    Scalar(AbiValueV1),
    FixedMath(AbiFixedMathDefaultV1),
    String(&'static str),
    StringName(&'static str),
    NodePath(&'static str),
    Empty(AbiValueType),
    #[doc(hidden)]
    GodotInteger(AbiGodotIntegerDefaultFn),
}

impl PartialEq for PropertyDefault {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scalar(left), Self::Scalar(right)) => left == right,
            (Self::FixedMath(left), Self::FixedMath(right)) => left == right,
            (Self::String(left), Self::String(right))
            | (Self::StringName(left), Self::StringName(right))
            | (Self::NodePath(left), Self::NodePath(right)) => left == right,
            (Self::Empty(left), Self::Empty(right)) => left == right,
            (Self::GodotInteger(left), Self::GodotInteger(right)) => {
                core::ptr::fn_addr_eq(*left, *right)
            }
            _ => false,
        }
    }
}

impl Eq for PropertyDefault {}

/// Maps normalized Inspector metadata to the project-module value transport.
#[doc(hidden)]
#[must_use]
pub const fn property_value_type(type_: AbiPropertyType) -> AbiValueType {
    match type_.0 {
        1 => AbiValueType::BOOL,
        2 => AbiValueType::I64,
        3 => AbiValueType::F64,
        4 => AbiValueType::STRING,
        5 => AbiValueType::VECTOR2,
        6 => AbiValueType::VECTOR2I,
        7 => AbiValueType::RECT2,
        8 => AbiValueType::RECT2I,
        9 => AbiValueType::VECTOR3,
        10 => AbiValueType::VECTOR3I,
        11 => AbiValueType::TRANSFORM2D,
        12 => AbiValueType::VECTOR4,
        13 => AbiValueType::VECTOR4I,
        14 => AbiValueType::PLANE,
        15 => AbiValueType::QUATERNION,
        16 => AbiValueType::AABB,
        17 => AbiValueType::BASIS,
        18 => AbiValueType::TRANSFORM3D,
        19 => AbiValueType::PROJECTION,
        20 => AbiValueType::COLOR,
        21 => AbiValueType::STRING_NAME,
        22 => AbiValueType::NODE_PATH,
        23 => AbiValueType::RID,
        24 => AbiValueType::OBJECT_ID,
        25 => AbiValueType::CALLABLE,
        26 => AbiValueType::SIGNAL,
        27 => AbiValueType::DICTIONARY,
        28 => AbiValueType::ARRAY,
        29 => AbiValueType::PACKED_BYTE_ARRAY,
        30 => AbiValueType::PACKED_INT32_ARRAY,
        31 => AbiValueType::PACKED_INT64_ARRAY,
        32 => AbiValueType::PACKED_FLOAT32_ARRAY,
        33 => AbiValueType::PACKED_FLOAT64_ARRAY,
        34 => AbiValueType::PACKED_STRING_ARRAY,
        35 => AbiValueType::PACKED_VECTOR2_ARRAY,
        36 => AbiValueType::PACKED_VECTOR3_ARRAY,
        37 => AbiValueType::PACKED_COLOR_ARRAY,
        38 => AbiValueType::PACKED_VECTOR4_ARRAY,
        _ => AbiValueType::NIL,
    }
}

/// Name and normalized Godot type of one generated signal argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalArgumentDescriptor {
    pub name: &'static str,
    pub type_: AbiValueType,
}

/// Structured signal metadata generated from `#[signal(args(...))]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalDescriptor {
    pub arguments: &'static [SignalArgumentDescriptor],
    #[doc(hidden)]
    pub abi_arguments: &'static [AbiSignalArgumentDescriptorV1],
}

/// Structured target generated from one `#[node("path")]` field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeDescriptor {
    pub path: &'static str,
    pub class_name: &'static str,
    pub optional: bool,
}

/// Compile-time field metadata generated by `#[script]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub rust_type: &'static str,
    pub kind: FieldKind,
    pub options: &'static str,
    pub default: Option<&'static str>,
    pub reload: ReloadPolicy,
    #[doc(hidden)]
    pub reload_value_type: Option<AbiValueType>,
    pub property: Option<PropertyDescriptor>,
    pub node: Option<NodeDescriptor>,
    pub signal: Option<SignalDescriptor>,
}

/// Struct-level metadata for one attachable Rust script.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScriptDescriptor {
    pub name: &'static str,
    pub global_name: Option<&'static str>,
    pub base_script: Option<&'static str>,
    pub module_path: &'static str,
    pub base: &'static str,
    pub tool: bool,
    pub abstract_: bool,
    pub icon_path: Option<&'static str>,
    pub fields: &'static [FieldDescriptor],
}

/// Implemented by the struct form of `#[script]`.
pub trait ScriptClass: Sized + 'static {
    const DESCRIPTOR: ScriptDescriptor;

    /// Creates native Rust state before Inspector values are applied.
    #[doc(hidden)]
    fn __godot_rs_new() -> Self;
}

/// Associates an attachable Rust script with its generated Godot base class.
#[doc(hidden)]
pub trait ScriptBase {
    type Base: GodotClass;
}

/// Compile-time proof that a Rust script may override callbacks declared by
/// `Target`.
///
/// A script cannot implement virtual callbacks from an unrelated Godot class:
///
/// ```compile_fail
/// use godot_rs::prelude::*;
///
/// #[script(base = Resource)]
/// struct Settings;
///
/// #[script]
/// impl NodeVirtual for Settings {
///     fn _get_configuration_warnings(&mut self) -> PackedStringArray {
///         PackedStringArray::default()
///     }
/// }
/// ```
#[doc(hidden)]
pub trait ScriptInherits<Target: GodotClass>: ScriptBase {}

impl<Script, Target> ScriptInherits<Target> for Script
where
    Script: ScriptBase,
    Target: GodotClass,
    Script::Base: crate::engine::Inherits<Target>,
{
}

/// Godot-style access to the next Rust script implementation in an
/// `extends = "res://..."` inheritance chain.
///
/// Return and argument types use the same codecs as ordinary reflected
/// methods. Pass no arguments as `()`, one argument as `(value,)`, or several
/// as a tuple. The return type is normally inferred:
///
/// ```ignore
/// let inherited: i64 = self.call_super("score", (bonus,))?;
/// ```
pub trait ScriptSuper: ScriptClass {
    fn call_super<R, Arguments>(&self, method: &str, arguments: Arguments) -> EngineResult<R>
    where
        R: FromAbiValue,
        Arguments: SuperArguments,
    {
        let arguments = arguments.encode()?;
        let output = crate::module::call_super_method(method, arguments.values())?;
        let decoded = R::from_abi(borrow_module_output(output)).ok_or_else(|| {
            crate::error::EngineError::invalid_result(
                "base Rust script returned an incompatible value",
            )
        });
        crate::module::release_module_output(output);
        decoded
    }
}

impl<T: ScriptClass> ScriptSuper for T {}

fn borrow_module_output(mut value: AbiValueV1) -> AbiValueV1 {
    if matches!(
        value.reserved_flags,
        godot_api::abi::ABI_VALUE_OWNED_UTF8 | godot_api::abi::ABI_VALUE_OWNED_BYTES
    ) {
        value.reserved_flags = 0;
    }
    value
}

/// Tuple transport accepted by [`ScriptSuper::call_super`].
#[doc(hidden)]
pub trait SuperArguments {
    fn encode(self) -> EngineResult<SuperArgumentBuffer>;
}

/// Temporary ABI storage for one base-script method call.
#[doc(hidden)]
pub struct SuperArgumentBuffer {
    values: Vec<AbiValueV1>,
    owned: Vec<AbiValueV1>,
}

impl SuperArgumentBuffer {
    fn values(&self) -> &[AbiValueV1] {
        &self.values
    }

    fn push<T: IntoMethodResult>(&mut self, value: T) -> EngineResult<()> {
        let mut encoded = AbiValueV1::NIL;
        let result = value.write_result(&mut encoded);
        if result.status != AbiStatus::Ok {
            return Err(crate::error::EngineError::from_abi(result));
        }
        let mut borrowed = encoded;
        if matches!(
            encoded.reserved_flags,
            godot_api::abi::ABI_VALUE_OWNED_UTF8 | godot_api::abi::ABI_VALUE_OWNED_BYTES
        ) {
            borrowed.reserved_flags = 0;
            self.owned.push(encoded);
        }
        self.values.push(borrowed);
        Ok(())
    }
}

impl Drop for SuperArgumentBuffer {
    fn drop(&mut self) {
        for value in self.owned.drain(..) {
            crate::module::release_module_output(value);
        }
    }
}

impl SuperArguments for () {
    fn encode(self) -> EngineResult<SuperArgumentBuffer> {
        Ok(SuperArgumentBuffer {
            values: Vec::new(),
            owned: Vec::new(),
        })
    }
}

macro_rules! super_arguments {
    ($($type:ident => $value:ident),+ $(,)?) => {
        impl<$($type),+> SuperArguments for ($($type,)+)
        where
            $($type: IntoMethodResult,)+
        {
            fn encode(self) -> EngineResult<SuperArgumentBuffer> {
                let ($($value,)+) = self;
                let mut buffer = SuperArgumentBuffer {
                    values: Vec::new(),
                    owned: Vec::new(),
                };
                $(buffer.push($value)?;)+
                Ok(buffer)
            }
        }
    };
}

super_arguments!(A1 => a1);
super_arguments!(A1 => a1, A2 => a2);
super_arguments!(A1 => a1, A2 => a2, A3 => a3);
super_arguments!(A1 => a1, A2 => a2, A3 => a3, A4 => a4);
super_arguments!(A1 => a1, A2 => a2, A3 => a3, A4 => a4, A5 => a5);
super_arguments!(A1 => a1, A2 => a2, A3 => a3, A4 => a4, A5 => a5, A6 => a6);
super_arguments!(A1 => a1, A2 => a2, A3 => a3, A4 => a4, A5 => a5, A6 => a6, A7 => a7);
super_arguments!(
    A1 => a1,
    A2 => a2,
    A3 => a3,
    A4 => a4,
    A5 => a5,
    A6 => a6,
    A7 => a7,
    A8 => a8,
);

/// Generated indexed access to Godot-visible script fields.
#[doc(hidden)]
pub trait ScriptFieldAccess: ScriptClass {
    unsafe fn __godot_rs_get_field(
        &self,
        field_index: u32,
        output: *mut AbiValueV1,
    ) -> AbiCallResult;

    unsafe fn __godot_rs_set_field(&mut self, field_index: u32, value: AbiValueV1)
    -> AbiCallResult;
}

/// Well-known Godot callback slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleSlot {
    EnterTree,
    Ready,
    Process,
    PhysicsProcess,
    Input,
    UnhandledInput,
    ExitTree,
}

/// Method exposure at the Godot boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MethodKind {
    Lifecycle(LifecycleSlot),
    Func,
    Rpc,
}

/// Receiver kind used for re-entry and borrow enforcement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiverKind {
    Shared,
    Mutable,
    Static,
}

/// Compile-time name and normalized Godot type of one method argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MethodArgumentDescriptor {
    pub name: &'static str,
    pub type_: AbiValueType,
    pub class_name: Option<&'static str>,
}

/// Which peers may invoke one RPC method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcMode {
    Authority,
    AnyPeer,
}

/// Network transport selected for one RPC method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcTransferMode {
    Unreliable,
    UnreliableOrdered,
    Reliable,
}

/// Structured Godot RPC metadata generated from `#[rpc(...)]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpcConfig {
    pub mode: RpcMode,
    pub call_local: bool,
    pub transfer_mode: RpcTransferMode,
    pub channel: u32,
}

/// Result returned by a statically dispatched lifecycle callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackStatus {
    Ok,
    Error(&'static str),
}

impl CallbackStatus {
    /// Converts an SDK callback result to the stable project-module ABI.
    #[doc(hidden)]
    #[must_use]
    pub const fn into_abi(self) -> AbiCallResult {
        match self {
            Self::Ok => AbiCallResult::OK,
            Self::Error(message) => AbiCallResult::callback_failed(message),
        }
    }
}

/// Converts supported callback return types without dynamic dispatch.
#[doc(hidden)]
pub trait IntoCallbackStatus {
    fn into_callback_status(self) -> CallbackStatus;
}

impl IntoCallbackStatus for () {
    fn into_callback_status(self) -> CallbackStatus {
        CallbackStatus::Ok
    }
}

impl IntoCallbackStatus for ScriptResult<()> {
    fn into_callback_status(self) -> CallbackStatus {
        match self {
            Ok(()) => CallbackStatus::Ok,
            Err(error) => CallbackStatus::Error(error.message()),
        }
    }
}

impl IntoCallbackStatus for EngineResult<()> {
    fn into_callback_status(self) -> CallbackStatus {
        match self {
            Ok(()) => CallbackStatus::Ok,
            Err(error) => CallbackStatus::Error(engine_error_callback_message(error.kind())),
        }
    }
}

/// Contains a user panic before it can unwind through the project-module C ABI.
#[doc(hidden)]
pub fn catch_abi_panic(callback: impl FnOnce() -> AbiCallResult) -> AbiCallResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)).unwrap_or_else(|_| {
        AbiCallResult::failure(AbiStatus::Panic, "Rust script callback panicked")
    })
}

pub type Lifecycle0Callback = unsafe extern "C" fn(*mut c_void) -> AbiCallResult;
pub type LifecycleF64Callback = unsafe extern "C" fn(*mut c_void, f64) -> AbiCallResult;
pub type LifecycleInputCallback = unsafe extern "C" fn(*mut c_void, u64) -> AbiCallResult;

/// Statically typed lifecycle callback. `None` is used by reflected methods
/// until their generated Variant codec is connected to the module ABI.
#[derive(Clone, Copy)]
pub enum MethodCallback {
    None,
    Lifecycle0(Lifecycle0Callback),
    LifecycleF64(LifecycleF64Callback),
    LifecycleInput(LifecycleInputCallback),
}

impl core::fmt::Debug for MethodCallback {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::None => "None",
            Self::Lifecycle0(_) => "Lifecycle0",
            Self::LifecycleF64(_) => "LifecycleF64",
            Self::LifecycleInput(_) => "LifecycleInput",
        })
    }
}

/// Compile-time method metadata generated by the impl form of `#[script]`.
#[derive(Clone, Copy, Debug)]
pub struct MethodDescriptor {
    pub id: u64,
    pub name: &'static str,
    pub rust_signature: &'static str,
    pub kind: MethodKind,
    pub receiver: ReceiverKind,
    pub argument_count: u16,
    pub argument_types: &'static [AbiValueType],
    pub arguments: &'static [MethodArgumentDescriptor],
    #[doc(hidden)]
    pub abi_arguments: &'static [AbiMethodArgumentDescriptorV1],
    #[doc(hidden)]
    pub abi_argument_classes: &'static [AbiByteSlice],
    #[doc(hidden)]
    pub default_arguments: &'static [AbiMethodDefaultFn],
    pub vararg: bool,
    #[doc(hidden)]
    pub abi_extensions: AbiMethodExtensionsV1,
    pub return_type: AbiValueType,
    pub return_class: Option<&'static str>,
    pub options: &'static str,
    pub rpc: Option<RpcConfig>,
    pub callback: MethodCallback,
}

/// Builds the versioned ABI extension attached to generated method metadata.
#[doc(hidden)]
#[must_use]
pub const fn method_extensions(
    argument_classes: &'static [AbiByteSlice],
    return_class: Option<&'static str>,
    default_arguments: &'static [AbiMethodDefaultFn],
    vararg: bool,
) -> AbiMethodExtensionsV1 {
    let return_class = match return_class {
        Some(value) => AbiByteSlice::from_static(value),
        None => AbiByteSlice::EMPTY,
    };
    AbiMethodExtensionsV1 {
        struct_size: AbiMethodExtensionsV1::MINIMUM_SIZE,
        reserved_flags: if vararg { ABI_METHOD_SCHEMA_VARARG } else { 0 },
        argument_classes: AbiByteSliceSlice::from_static(argument_classes),
        return_class,
        default_arguments: AbiMethodDefaultFnSlice::from_static(default_arguments),
        reserved: [0; 4],
    }
}

/// Implemented once by the struct form of `#[script]`.
pub trait ScriptMethods: ScriptClass + ScriptFieldAccess {}

/// Generated dispatch function for one `#[script] impl` block.
#[doc(hidden)]
pub type MethodBlockInvoker = fn(
    state: *mut c_void,
    method_id: u64,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult;

/// Independently registered methods from one `#[script] impl` block.
#[doc(hidden)]
pub struct MethodBlock {
    pub script_type_id: fn() -> TypeId,
    pub methods: &'static [MethodDescriptor],
    pub invoke: MethodBlockInvoker,
}

// SAFETY: Generated blocks only contain function pointers and immutable
// process-lifetime metadata. Raw byte pointers inside ABI descriptors point to
// immutable static strings and are never dereferenced through mutable access.
unsafe impl Sync for MethodBlock {}

inventory::collect!(MethodBlock);

/// Returns a stable runtime type key without requiring const `TypeId::of`.
#[doc(hidden)]
#[must_use]
pub fn script_type_id<T: 'static>() -> TypeId {
    TypeId::of::<T>()
}

fn method_blocks<T: ScriptMethods>() -> impl Iterator<Item = &'static MethodBlock> {
    let expected = TypeId::of::<T>();
    inventory::iter::<MethodBlock>
        .into_iter()
        .filter(move |block| (block.script_type_id)() == expected)
}

/// Returns every method registered for a script in deterministic ABI order.
#[doc(hidden)]
#[must_use]
pub fn registered_methods<T: ScriptMethods>() -> Vec<&'static MethodDescriptor> {
    let mut methods = method_blocks::<T>()
        .flat_map(|block| block.methods)
        .collect::<Vec<_>>();
    methods.sort_unstable_by_key(|method| (method.id, method.name));
    methods
}

fn method_registry_is_valid<T: ScriptMethods>() -> bool {
    let methods = registered_methods::<T>();
    if methods
        .windows(2)
        .any(|pair| pair[0].id == pair[1].id || pair[0].name == pair[1].name)
    {
        return false;
    }

    let mut lifecycle_slots = [false; 7];
    for method in methods {
        let argument_count = usize::from(method.argument_count);
        if method.argument_types.len() != argument_count
            || method.arguments.len() != argument_count
            || method.abi_arguments.len() != argument_count
            || method.abi_argument_classes.len() != argument_count
            || method.default_arguments.len() > argument_count
        {
            return false;
        }
        if (method.vararg || !method.default_arguments.is_empty())
            && matches!(method.kind, MethodKind::Lifecycle(_))
        {
            return false;
        }
        if method.default_arguments.iter().any(Option::is_none) {
            return false;
        }
        for (argument, class_name) in method.arguments.iter().zip(method.abi_argument_classes) {
            let accepts_metadata = matches!(
                argument.type_,
                AbiValueType::OBJECT_ID | AbiValueType::ARRAY
            );
            if (!accepts_metadata && argument.class_name.is_some())
                || (argument.type_ == AbiValueType::OBJECT_ID && argument.class_name.is_none())
                || argument.class_name.map(str::len).unwrap_or_default() != class_name.len
            {
                return false;
            }
        }
        let accepts_return_metadata = matches!(
            method.return_type,
            AbiValueType::OBJECT_ID | AbiValueType::ARRAY
        );
        if (!accepts_return_metadata && method.return_class.is_some())
            || (method.return_type == AbiValueType::OBJECT_ID && method.return_class.is_none())
        {
            return false;
        }
        let MethodKind::Lifecycle(slot) = method.kind else {
            continue;
        };
        let index = match slot {
            LifecycleSlot::EnterTree => 0,
            LifecycleSlot::Ready => 1,
            LifecycleSlot::Process => 2,
            LifecycleSlot::PhysicsProcess => 3,
            LifecycleSlot::Input => 4,
            LifecycleSlot::UnhandledInput => 5,
            LifecycleSlot::ExitTree => 6,
        };
        if lifecycle_slots[index] {
            return false;
        }
        lifecycle_slots[index] = true;
    }
    true
}

/// Decodes one fixed-layout project-module method argument.
#[doc(hidden)]
pub trait FromAbiValue: Sized {
    fn from_abi(value: AbiValueV1) -> Option<Self>;
}

/// Encodes one supported reflected method return value.
#[doc(hidden)]
pub trait IntoMethodResult {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult;
}

macro_rules! integer_value {
    ($type:ty) => {
        impl FromAbiValue for $type {
            fn from_abi(value: AbiValueV1) -> Option<Self> {
                if value.type_ != AbiValueType::I64
                    || value.reserved_flags != 0
                    || value.payload[1] != 0
                {
                    return None;
                }
                <$type>::try_from(value.payload[0] as i64).ok()
            }
        }

        impl IntoMethodResult for $type {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                write_method_value(output, AbiValueV1::from_i64(self.into()))
            }
        }

        impl IntoMethodResult for ScriptResult<$type> {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                match self {
                    Ok(value) => value.write_result(output),
                    Err(error) => AbiCallResult::callback_failed(error.message()),
                }
            }
        }
    };
}

integer_value!(i32);
integer_value!(i64);
integer_value!(i8);
integer_value!(i16);

macro_rules! unsigned_integer_value {
    ($type:ty) => {
        impl FromAbiValue for $type {
            fn from_abi(value: AbiValueV1) -> Option<Self> {
                if value.type_ != AbiValueType::U64
                    || value.reserved_flags != 0
                    || value.payload[1] != 0
                {
                    return None;
                }
                <$type>::try_from(value.payload[0]).ok()
            }
        }

        impl IntoMethodResult for $type {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                write_method_value(output, AbiValueV1::from_u64(self.into()))
            }
        }

        impl IntoMethodResult for ScriptResult<$type> {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                match self {
                    Ok(value) => value.write_result(output),
                    Err(error) => AbiCallResult::callback_failed(error.message()),
                }
            }
        }
    };
}

unsigned_integer_value!(u8);
unsigned_integer_value!(u16);
unsigned_integer_value!(u32);
unsigned_integer_value!(u64);

impl FromAbiValue for char {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let value = u32::from_abi(value)?;
        char::from_u32(value)
    }
}

impl IntoMethodResult for char {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        u32::from(self).write_result(output)
    }
}

impl IntoMethodResult for ScriptResult<char> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl<T: crate::engine::GodotIntegerValue> FromAbiValue for T {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let expected = reflected_integer_value_type::<T>();
        (value.type_ == expected && value.reserved_flags == 0 && value.payload[1] == 0)
            .then(|| T::__from_raw(value.payload[0]))
    }
}

impl<T: crate::engine::GodotIntegerValue> IntoMethodResult for T {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            if T::SIGNED {
                AbiValueV1::from_i64(self.__raw() as i64)
            } else {
                AbiValueV1::from_u64(self.__raw())
            },
        )
    }
}

/// Returns the normalized reflected ABI type for a generated Godot integer.
#[doc(hidden)]
#[must_use]
pub const fn reflected_integer_value_type<T: crate::engine::GodotIntegerValue>() -> AbiValueType {
    if T::SIGNED {
        AbiValueType::I64
    } else {
        AbiValueType::U64
    }
}

impl FromAbiValue for bool {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        if value.type_ != AbiValueType::BOOL || value.reserved_flags != 0 || value.payload[1] != 0 {
            return None;
        }
        match value.payload[0] {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

impl IntoMethodResult for bool {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_bool(self))
    }
}

impl IntoMethodResult for ScriptResult<bool> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for f64 {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        (value.type_ == AbiValueType::F64 && value.reserved_flags == 0 && value.payload[1] == 0)
            .then(|| f64::from_bits(value.payload[0]))
    }
}

impl FromAbiValue for f32 {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let value = f64::from_abi(value)?;
        let converted = value as f32;
        (!value.is_finite() || converted.is_finite()).then_some(converted)
    }
}

impl FromAbiValue for String {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        borrowed_text(value, AbiValueType::STRING)
    }
}

impl FromAbiValue for StringName {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        borrowed_text(value, AbiValueType::STRING_NAME).map(Self::from)
    }
}

impl FromAbiValue for NodePath {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        borrowed_text(value, AbiValueType::NODE_PATH).map(Self::from)
    }
}

impl<T: GodotClass> FromAbiValue for ObjectRef<T> {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        (value.type_ == AbiValueType::OBJECT_ID
            && value.reserved_flags == 0
            && value.payload[0] != 0
            && value.payload[1] == 0)
            .then(|| ObjectRef::__from_instance_id(value.payload[0]))
    }
}

impl<T: GodotClass> FromAbiValue for Option<ObjectRef<T>> {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        if value.type_ != AbiValueType::OBJECT_ID
            || value.reserved_flags != 0
            || value.payload[1] != 0
        {
            return None;
        }
        Some((value.payload[0] != 0).then(|| ObjectRef::__from_instance_id(value.payload[0])))
    }
}

impl<T: GodotClass> FromAbiValue for GodotRef<T> {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let (object_id, ownership) = crate::module::take_owned_object_ref(value)??;
        Some(Self::from_owned_parts(object_id, ownership))
    }
}

impl<T: GodotClass> FromAbiValue for Option<GodotRef<T>> {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        if value.type_ == AbiValueType::OBJECT_ID
            && value.reserved_flags == 0
            && value.payload == [0, 0]
        {
            return Some(None);
        }
        crate::module::take_owned_object_ref(value).map(|value| {
            value.map(|(object_id, ownership)| GodotRef::from_owned_parts(object_id, ownership))
        })
    }
}

impl<T: GodotClass> IntoMethodResult for ObjectRef<T> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_object_id(self.instance_id()))
    }
}

impl<T: GodotClass> IntoMethodResult for Option<ObjectRef<T>> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_object_id(self.map_or(0, ObjectRef::instance_id)),
        )
    }
}

impl<T: GodotClass> IntoMethodResult for GodotRef<T> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        self.object_ref().write_result(output)
    }
}

impl<T: GodotClass> IntoMethodResult for Option<GodotRef<T>> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        self.as_ref().map(GodotRef::object_ref).write_result(output)
    }
}

impl<T: GodotClass> IntoMethodResult for ScriptResult<ObjectRef<T>> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl<T: GodotClass> IntoMethodResult for ScriptResult<Option<ObjectRef<T>>> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

/// Verifies an exported scene-node reference against generated inheritance.
#[doc(hidden)]
pub fn assert_export_node<T: GodotClass + Inherits<Node>>() {
    let _ = core::marker::PhantomData::<NodeRef<T>>;
}

/// Verifies an exported Resource reference against generated inheritance.
#[doc(hidden)]
pub fn assert_export_resource<T: GodotClass + Inherits<Resource>>() {
    let _ = core::marker::PhantomData::<GodotRef<T>>;
}

impl IntoMethodResult for String {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_owned_method_string(output, self)
    }
}

impl IntoMethodResult for StringName {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_owned_method_text(output, AbiValueType::STRING_NAME, self.into_string())
    }
}

impl IntoMethodResult for NodePath {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_owned_method_text(output, AbiValueType::NODE_PATH, self.into_string())
    }
}

impl IntoMethodResult for ScriptResult<NodePath> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl IntoMethodResult for ScriptResult<StringName> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl IntoMethodResult for ScriptResult<String> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Vector2 {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y] = value.vector2()?;
        Some(Self::new(x, y))
    }
}

impl IntoMethodResult for Vector2 {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_vector2(self.x, self.y))
    }
}

impl IntoMethodResult for ScriptResult<Vector2> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Vector3 {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, z] = value.vector3()?;
        Some(Self::new(x, y, z))
    }
}

impl IntoMethodResult for Vector3 {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_vector3(self.x, self.y, self.z))
    }
}

impl IntoMethodResult for ScriptResult<Vector3> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Color {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [r, g, b, a] = value.color()?;
        Some(Self::rgba(r, g, b, a))
    }
}

impl IntoMethodResult for Color {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_color(self.r, self.g, self.b, self.a),
        )
    }
}

impl IntoMethodResult for ScriptResult<Color> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Vector2i {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y] = value.vector2i()?;
        Some(Self::new(x, y))
    }
}

impl IntoMethodResult for Vector2i {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_vector2i(self.x, self.y))
    }
}

impl IntoMethodResult for ScriptResult<Vector2i> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Vector3i {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, z] = value.vector3i()?;
        Some(Self::new(x, y, z))
    }
}

impl IntoMethodResult for Vector3i {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_vector3i(self.x, self.y, self.z))
    }
}

impl IntoMethodResult for ScriptResult<Vector3i> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Rect2 {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, width, height] = value.rect2()?;
        Some(Self::from_components(x, y, width, height))
    }
}

impl IntoMethodResult for Rect2 {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_rect2(self.position.x, self.position.y, self.size.x, self.size.y),
        )
    }
}

impl IntoMethodResult for ScriptResult<Rect2> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Rect2i {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, width, height] = value.rect2i()?;
        Some(Self::from_components(x, y, width, height))
    }
}

impl IntoMethodResult for Rect2i {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_rect2i(self.position.x, self.position.y, self.size.x, self.size.y),
        )
    }
}

impl IntoMethodResult for ScriptResult<Rect2i> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Quaternion {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, z, w] = value.quaternion()?;
        Some(Self::new(x, y, z, w))
    }
}

impl IntoMethodResult for Quaternion {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_quaternion(self.x, self.y, self.z, self.w),
        )
    }
}

impl IntoMethodResult for ScriptResult<Quaternion> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Plane {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, z, d] = value.plane()?;
        Some(Self::from_components(x, y, z, d))
    }
}

impl IntoMethodResult for Plane {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_plane(self.normal.x, self.normal.y, self.normal.z, self.d),
        )
    }
}

impl IntoMethodResult for ScriptResult<Plane> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Vector4 {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, z, w] = value.vector4()?;
        Some(Self::new(x, y, z, w))
    }
}

impl IntoMethodResult for Vector4 {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_vector4(self.x, self.y, self.z, self.w),
        )
    }
}

impl IntoMethodResult for ScriptResult<Vector4> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Vector4i {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let [x, y, z, w] = value.vector4i()?;
        Some(Self::new(x, y, z, w))
    }
}

impl IntoMethodResult for Vector4i {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(
            output,
            AbiValueV1::from_vector4i(self.x, self.y, self.z, self.w),
        )
    }
}

impl IntoMethodResult for ScriptResult<Vector4i> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

macro_rules! reflected_fixed_math {
    ($type:ty, $abi_type:ident, $count:expr, $construct:path) => {
        impl FromAbiValue for $type {
            fn from_abi(value: AbiValueV1) -> Option<Self> {
                copy_f32_components::<$count>(value, AbiValueType::$abi_type).map($construct)
            }
        }

        impl IntoMethodResult for $type {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                write_method_value(
                    output,
                    crate::module::owned_f32_components(
                        AbiValueType::$abi_type,
                        self.__components(),
                    ),
                )
            }
        }

        impl IntoMethodResult for ScriptResult<$type> {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                match self {
                    Ok(value) => value.write_result(output),
                    Err(error) => AbiCallResult::callback_failed(error.message()),
                }
            }
        }
    };
}

reflected_fixed_math!(Transform2D, TRANSFORM2D, 6, Transform2D::__from_components);
reflected_fixed_math!(Aabb, AABB, 6, Aabb::__from_components);
reflected_fixed_math!(Basis, BASIS, 9, Basis::__from_components);
reflected_fixed_math!(Transform3D, TRANSFORM3D, 12, Transform3D::__from_components);
reflected_fixed_math!(Projection, PROJECTION, 16, Projection::__from_components);

macro_rules! reflected_packed_array {
    ($type:ty, $abi_type:ident) => {
        impl FromAbiValue for $type {
            fn from_abi(value: AbiValueV1) -> Option<Self> {
                let bytes = copy_borrowed_bytes(value, AbiValueType::$abi_type)?;
                Self::__from_bytes(&bytes)
            }
        }

        impl IntoMethodResult for $type {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                write_method_value(
                    output,
                    crate::module::owned_bytes(AbiValueType::$abi_type, self.__bytes().to_vec()),
                )
            }
        }

        impl IntoMethodResult for ScriptResult<$type> {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                match self {
                    Ok(value) => value.write_result(output),
                    Err(error) => AbiCallResult::callback_failed(error.message()),
                }
            }
        }
    };
}

reflected_packed_array!(PackedByteArray, PACKED_BYTE_ARRAY);
reflected_packed_array!(PackedInt32Array, PACKED_INT32_ARRAY);
reflected_packed_array!(PackedInt64Array, PACKED_INT64_ARRAY);
reflected_packed_array!(PackedFloat32Array, PACKED_FLOAT32_ARRAY);
reflected_packed_array!(PackedFloat64Array, PACKED_FLOAT64_ARRAY);
reflected_packed_array!(PackedStringArray, PACKED_STRING_ARRAY);
reflected_packed_array!(PackedVector2Array, PACKED_VECTOR2_ARRAY);
reflected_packed_array!(PackedVector3Array, PACKED_VECTOR3_ARRAY);
reflected_packed_array!(PackedColorArray, PACKED_COLOR_ARRAY);
reflected_packed_array!(PackedVector4Array, PACKED_VECTOR4_ARRAY);

fn copy_dynamic_value(
    value: AbiValueV1,
    expected: AbiValueType,
) -> Option<(Vec<u8>, Option<crate::module::HostDynamicValueToken>)> {
    if value.type_ != expected || value.reserved_flags != 0 {
        return None;
    }
    let (pointer, length) = value.byte_range(expected)?;
    // SAFETY: Reflected method arguments synchronously borrow this range from
    // Host-owned call backing.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) }.to_vec();
    let ownership = match godot_api::abi::dynamic_value_ownership_token(&bytes) {
        Some(token) => crate::module::retain_dynamic_value(expected, token).ok()?,
        None => None,
    };
    Some((bytes, ownership))
}

impl FromAbiValue for Variant {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let (bytes, ownership) = copy_dynamic_value(value, AbiValueType::VARIANT)?;
        Self::__from_host_bytes(&bytes, ownership)
    }
}

impl IntoMethodResult for Variant {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self.__bytes() {
            Ok(bytes) => {
                if let Err(error) = crate::module::retain_dynamic_callables_for_transfer(bytes) {
                    return AbiCallResult::callback_failed(engine_error_callback_message(
                        error.kind(),
                    ));
                }
                write_method_value(
                    output,
                    crate::module::owned_bytes(AbiValueType::VARIANT, bytes.to_vec()),
                )
            }
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl IntoMethodResult for ScriptResult<Variant> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl<T: VariantConvert> FromAbiValue for Array<T> {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let (bytes, ownership) = copy_dynamic_value(value, AbiValueType::ARRAY)?;
        Self::__from_host_bytes(&bytes, ownership)
    }
}

impl<T: VariantConvert> IntoMethodResult for Array<T> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self.__bytes() {
            Ok(bytes) => {
                if let Err(error) = crate::module::retain_dynamic_callables_for_transfer(bytes) {
                    return AbiCallResult::callback_failed(engine_error_callback_message(
                        error.kind(),
                    ));
                }
                write_method_value(
                    output,
                    crate::module::owned_bytes(AbiValueType::ARRAY, bytes.to_vec()),
                )
            }
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl<T: VariantConvert> IntoMethodResult for ScriptResult<Array<T>> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Dictionary {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        let (bytes, ownership) = copy_dynamic_value(value, AbiValueType::DICTIONARY)?;
        Self::__from_host_bytes(&bytes, ownership)
    }
}

impl IntoMethodResult for Dictionary {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self.__bytes() {
            Ok(bytes) => {
                if let Err(error) = crate::module::retain_dynamic_callables_for_transfer(bytes) {
                    return AbiCallResult::callback_failed(engine_error_callback_message(
                        error.kind(),
                    ));
                }
                write_method_value(
                    output,
                    crate::module::owned_bytes(AbiValueType::DICTIONARY, bytes.to_vec()),
                )
            }
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl IntoMethodResult for ScriptResult<Dictionary> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Callable {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        if value.type_ != AbiValueType::CALLABLE || value.reserved_flags != 0 {
            return None;
        }
        let (pointer, length) = value.byte_range(AbiValueType::CALLABLE)?;
        // SAFETY: Reflected method arguments synchronously borrow this range
        // from Host-owned call backing.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) }.to_vec();
        let ownership = match godot_api::abi::callable_value_ownership_token(&bytes) {
            Some(token) => crate::module::retain_callable_value(token).ok()?,
            None => None,
        };
        Self::__from_host_bytes(&bytes, ownership)
    }
}

impl IntoMethodResult for Callable {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        let bytes = match self.__bytes() {
            Ok(bytes) => bytes.to_vec(),
            Err(error) => return AbiCallResult::callback_failed(error.message()),
        };
        if let Some(token) = godot_api::abi::callable_value_ownership_token(&bytes) {
            if let Err(error) = crate::module::retain_callable_for_transfer(token) {
                return AbiCallResult::callback_failed(engine_error_callback_message(error.kind()));
            }
        }
        write_method_value(
            output,
            crate::module::owned_bytes(AbiValueType::CALLABLE, bytes),
        )
    }
}

impl IntoMethodResult for ScriptResult<Callable> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl<T> FromAbiValue for Signal<T> {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        if value.type_ != AbiValueType::SIGNAL || value.reserved_flags != 0 {
            return None;
        }
        let (pointer, length) = value.byte_range(AbiValueType::SIGNAL)?;
        // SAFETY: Reflected method arguments synchronously borrow this range
        // from Host-owned call backing.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        Self::__from_bytes(bytes)
    }
}

impl<T> IntoMethodResult for Signal<T> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self.__bytes() {
            Ok(bytes) => write_method_value(
                output,
                crate::module::owned_bytes(AbiValueType::SIGNAL, bytes.to_vec()),
            ),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl<T> IntoMethodResult for ScriptResult<Signal<T>> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

impl FromAbiValue for Rid {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        value.rid().map(Rid::from_raw)
    }
}

impl IntoMethodResult for Rid {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::from_rid(self.id()))
    }
}

impl IntoMethodResult for ScriptResult<Rid> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(value) => value.write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

macro_rules! float_result {
    ($type:ty) => {
        impl IntoMethodResult for $type {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                write_method_value(output, AbiValueV1::from_f64(self.into()))
            }
        }

        impl IntoMethodResult for ScriptResult<$type> {
            fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
                match self {
                    Ok(value) => value.write_result(output),
                    Err(error) => AbiCallResult::callback_failed(error.message()),
                }
            }
        }
    };
}

float_result!(f32);
float_result!(f64);

impl FromAbiValue for () {
    fn from_abi(value: AbiValueV1) -> Option<Self> {
        (value == AbiValueV1::NIL).then_some(())
    }
}

impl IntoMethodResult for () {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        write_method_value(output, AbiValueV1::NIL)
    }
}

impl IntoMethodResult for ScriptResult<()> {
    fn write_result(self, output: *mut AbiValueV1) -> AbiCallResult {
        match self {
            Ok(()) => ().write_result(output),
            Err(error) => AbiCallResult::callback_failed(error.message()),
        }
    }
}

/// Reads one generated argument after validating count and type.
///
/// # Safety
///
/// `arguments` must address `argument_count` initialized ABI values.
#[doc(hidden)]
pub unsafe fn decode_method_argument<T: FromAbiValue>(
    arguments: *const AbiValueV1,
    argument_count: u32,
    index: u32,
) -> Result<T, AbiCallResult> {
    if index >= argument_count || arguments.is_null() {
        return Err(AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "reflected method argument is missing",
        ));
    }
    // SAFETY: Bounds and null were checked against the caller's ABI count.
    let value = unsafe { arguments.add(index as usize).read() };
    T::from_abi(value).ok_or_else(|| {
        AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "reflected method argument type does not match its descriptor",
        )
    })
}

/// Encodes the generated method result into Host-owned ABI storage.
#[doc(hidden)]
pub fn encode_method_result<T: IntoMethodResult>(
    value: T,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    value.write_result(output)
}

fn write_method_value(output: *mut AbiValueV1, value: AbiValueV1) -> AbiCallResult {
    if output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "reflected method output pointer is null",
        );
    }
    // SAFETY: The project-module ABI caller owns one writable output slot.
    unsafe { output.write(value) };
    AbiCallResult::OK
}

fn write_owned_method_string(output: *mut AbiValueV1, value: String) -> AbiCallResult {
    write_owned_method_text(output, AbiValueType::STRING, value)
}

fn write_owned_method_text(
    output: *mut AbiValueV1,
    value_type: AbiValueType,
    value: String,
) -> AbiCallResult {
    if output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "reflected method output pointer is null",
        );
    }
    let value = crate::module::owned_text(value_type, value);
    // SAFETY: Null was rejected before allocating the module-owned value.
    unsafe { output.write(value) };
    AbiCallResult::OK
}

fn borrowed_text(value: AbiValueV1, expected: AbiValueType) -> Option<String> {
    if value.type_ != expected || value.reserved_flags != 0 {
        return None;
    }
    let address = usize::try_from(value.payload[0]).ok()?;
    let length = usize::try_from(value.payload[1]).ok()?;
    if address == 0 || length > MAX_UTF8_VALUE_BYTES {
        return None;
    }
    // SAFETY: Host-to-module strings are borrowed for this synchronous
    // callback. The address is non-null and length is bounded before use.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    core::str::from_utf8(bytes).ok().map(str::to_owned)
}

fn copy_f32_components<const N: usize>(
    value: AbiValueV1,
    expected: AbiValueType,
) -> Option<[f32; N]> {
    let (pointer, length) = value.byte_range(expected)?;
    if length != N * core::mem::size_of::<f32>() {
        return None;
    }
    // SAFETY: The Host keeps reflected input storage live for the complete
    // module callback, and the exact bounded length was validated above.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    Some(core::array::from_fn(|index| {
        let offset = index * 4;
        f32::from_ne_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("f32 byte width"),
        )
    }))
}

fn copy_borrowed_bytes(value: AbiValueV1, expected: AbiValueType) -> Option<Vec<u8>> {
    if value.reserved_flags != 0 {
        return None;
    }
    let (pointer, length) = value.byte_range(expected)?;
    if length > MAX_UTF8_VALUE_BYTES {
        return None;
    }
    // SAFETY: The Host retains reflected input storage for the synchronous
    // module callback. Callers copy it immediately into an owned SDK value.
    Some(unsafe { core::slice::from_raw_parts(pointer, length) }.to_vec())
}

/// Stable compile-time method identifier used after registration.
#[must_use]
pub const fn method_id(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

/// Converts generated method slots into the C ABI table cached by the Host.
#[doc(hidden)]
#[must_use]
pub const fn lifecycle_table(methods: &[MethodDescriptor]) -> AbiLifecycleTableV1 {
    let mut result = AbiLifecycleTableV1::EMPTY;
    let mut index = 0;
    while index < methods.len() {
        let method = methods[index];
        match (method.kind, method.callback) {
            (
                MethodKind::Lifecycle(LifecycleSlot::EnterTree),
                MethodCallback::Lifecycle0(callback),
            ) => result.enter_tree = Some(callback),
            (MethodKind::Lifecycle(LifecycleSlot::Ready), MethodCallback::Lifecycle0(callback)) => {
                result.ready = Some(callback)
            }
            (
                MethodKind::Lifecycle(LifecycleSlot::Process),
                MethodCallback::LifecycleF64(callback),
            ) => result.process = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::PhysicsProcess),
                MethodCallback::LifecycleF64(callback),
            ) => result.physics_process = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::Input),
                MethodCallback::LifecycleInput(callback),
            ) => result.input = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::UnhandledInput),
                MethodCallback::LifecycleInput(callback),
            ) => result.unhandled_input = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::ExitTree),
                MethodCallback::Lifecycle0(callback),
            ) => result.exit_tree = Some(callback),
            _ => {}
        }
        index += 1;
    }
    result
}

fn lifecycle_table_from_registered(methods: &[&MethodDescriptor]) -> AbiLifecycleTableV1 {
    let mut result = AbiLifecycleTableV1::EMPTY;
    for method in methods {
        match (method.kind, method.callback) {
            (
                MethodKind::Lifecycle(LifecycleSlot::EnterTree),
                MethodCallback::Lifecycle0(callback),
            ) => result.enter_tree = Some(callback),
            (MethodKind::Lifecycle(LifecycleSlot::Ready), MethodCallback::Lifecycle0(callback)) => {
                result.ready = Some(callback);
            }
            (
                MethodKind::Lifecycle(LifecycleSlot::Process),
                MethodCallback::LifecycleF64(callback),
            ) => result.process = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::PhysicsProcess),
                MethodCallback::LifecycleF64(callback),
            ) => result.physics_process = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::Input),
                MethodCallback::LifecycleInput(callback),
            ) => result.input = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::UnhandledInput),
                MethodCallback::LifecycleInput(callback),
            ) => result.unhandled_input = Some(callback),
            (
                MethodKind::Lifecycle(LifecycleSlot::ExitTree),
                MethodCallback::Lifecycle0(callback),
            ) => result.exit_tree = Some(callback),
            _ => {}
        }
    }
    result
}

/// Copies one generated field descriptor over the stable module ABI.
#[doc(hidden)]
pub unsafe extern "C" fn abi_get_field<T: ScriptClass>(
    index: u32,
    output: *mut AbiFieldDescriptorV1,
) -> AbiStatus {
    if output.is_null() {
        return AbiStatus::InvalidArgument;
    }
    let Some(field) = T::DESCRIPTOR.fields.get(index as usize) else {
        return AbiStatus::InvalidArgument;
    };
    let default_value = field
        .property
        .as_ref()
        .and_then(|property| match property.default_value {
            Some(PropertyDefault::String(value)) => Some(value),
            Some(PropertyDefault::StringName(value)) => Some(value),
            Some(PropertyDefault::NodePath(value)) => Some(value),
            _ => None,
        })
        .or(field.default)
        .unwrap_or_default();
    let (reserved_extension_flags, abi_options, reserved) = if let Some(property) = field
        .property
        .as_ref()
        .filter(|property| property.integer_options.is_some())
    {
        let options = property
            .integer_options
            .expect("filtered Godot integer options");
        let Some(PropertyDefault::GodotInteger(default)) = property.default_value else {
            return AbiStatus::Internal;
        };
        (
            ABI_FIELD_EXTENSION_GODOT_INTEGER_SCHEMA,
            property.group.unwrap_or_default(),
            [
                usize::from(property.hint == godot_api::abi::ABI_PROPERTY_HINT_ENUM),
                options.as_ptr() as usize,
                options.len(),
                default as *const () as usize,
            ],
        )
    } else if let Some(property) = field.property.as_ref() {
        (
            ABI_FIELD_EXTENSION_PROPERTY_SCHEMA,
            property.encoded,
            [
                property.type_.0 as usize,
                property.hint as usize,
                property.usage as usize,
                property
                    .default_value
                    .as_ref()
                    .and_then(|value| match value {
                        PropertyDefault::Scalar(value) => {
                            Some((value as *const AbiValueV1) as usize)
                        }
                        PropertyDefault::FixedMath(value) => {
                            Some((value as *const AbiFixedMathDefaultV1) as usize)
                        }
                        PropertyDefault::String(_) => None,
                        PropertyDefault::StringName(_) => None,
                        PropertyDefault::NodePath(_)
                        | PropertyDefault::Empty(_)
                        | PropertyDefault::GodotInteger(_) => None,
                    })
                    .unwrap_or(0),
            ],
        )
    } else if let Some(signal) = field.signal.as_ref() {
        (
            ABI_FIELD_EXTENSION_SIGNAL_SCHEMA,
            field.options,
            [
                signal.abi_arguments.as_ptr() as usize,
                signal.abi_arguments.len(),
                0,
                0,
            ],
        )
    } else if let Some(node) = field.node.as_ref() {
        (
            ABI_FIELD_EXTENSION_NODE_SCHEMA,
            field.options,
            [
                node.path.as_ptr() as usize,
                node.path.len(),
                node.class_name.as_ptr() as usize,
                encode_node_field_class(node.class_name.len(), node.optional).unwrap_or(usize::MAX),
            ],
        )
    } else if let Some(value_type) = field.reload_value_type {
        (
            godot_api::abi::ABI_FIELD_EXTENSION_RELOAD_SCHEMA,
            field.options,
            [value_type.0 as usize, 0, 0, 0],
        )
    } else {
        (0, field.options, [0; 4])
    };
    let descriptor = AbiFieldDescriptorV1 {
        struct_size: AbiFieldDescriptorV1::MINIMUM_SIZE,
        reserved_extension_flags,
        name: AbiByteSlice::from_static(field.name),
        rust_type: AbiByteSlice::from_static(field.rust_type),
        kind: match field.kind {
            FieldKind::Plain => AbiFieldKind::Plain,
            FieldKind::Export => AbiFieldKind::Export,
            FieldKind::Node => AbiFieldKind::Node,
            FieldKind::Signal => AbiFieldKind::Signal,
        },
        options: AbiByteSlice::from_static(abi_options),
        default_value: AbiByteSlice::from_static(default_value),
        has_default: u8::from(field.default.is_some()),
        reserved_flags: [0; 3],
        reload: match field.reload {
            ReloadPolicy::Default => AbiReloadPolicy::Default,
            ReloadPolicy::Persist => AbiReloadPolicy::Persist,
            ReloadPolicy::Skip => AbiReloadPolicy::Skip,
        },
        reserved,
    };
    // SAFETY: The caller supplied a non-null writable ABI output slot.
    unsafe { output.write(descriptor) };
    AbiStatus::Ok
}

/// Copies one generated method descriptor over the stable module ABI.
#[doc(hidden)]
pub unsafe extern "C" fn abi_get_method<T: ScriptMethods>(
    index: u32,
    output: *mut AbiMethodDescriptorV1,
) -> AbiStatus {
    if output.is_null() {
        return AbiStatus::InvalidArgument;
    }
    let methods = registered_methods::<T>();
    let Some(method) = methods.get(index as usize) else {
        return AbiStatus::InvalidArgument;
    };
    let (kind, lifecycle) = match method.kind {
        MethodKind::Lifecycle(slot) => (
            AbiMethodKind::Lifecycle,
            match slot {
                LifecycleSlot::EnterTree => AbiLifecycleSlot::EnterTree,
                LifecycleSlot::Ready => AbiLifecycleSlot::Ready,
                LifecycleSlot::Process => AbiLifecycleSlot::Process,
                LifecycleSlot::PhysicsProcess => AbiLifecycleSlot::PhysicsProcess,
                LifecycleSlot::Input => AbiLifecycleSlot::Input,
                LifecycleSlot::UnhandledInput => AbiLifecycleSlot::UnhandledInput,
                LifecycleSlot::ExitTree => AbiLifecycleSlot::ExitTree,
            },
        ),
        MethodKind::Func => (AbiMethodKind::Func, AbiLifecycleSlot::None),
        MethodKind::Rpc => (AbiMethodKind::Rpc, AbiLifecycleSlot::None),
    };
    let descriptor = AbiMethodDescriptorV1 {
        struct_size: AbiMethodDescriptorV1::MINIMUM_SIZE,
        reserved_extension_flags: ABI_METHOD_EXTENSION_SCHEMA_V1,
        id: method.id,
        name: AbiByteSlice::from_static(method.name),
        rust_signature: AbiByteSlice::from_static(method.rust_signature),
        kind,
        lifecycle,
        receiver: match method.receiver {
            ReceiverKind::Shared => AbiReceiverKind::Shared,
            ReceiverKind::Mutable => AbiReceiverKind::Mutable,
            ReceiverKind::Static => AbiReceiverKind::Static,
        },
        argument_count: method.argument_count,
        reserved_flags: 0,
        options: AbiByteSlice::from_static(method.options),
        argument_types: AbiValueTypeSlice::from_static(method.argument_types),
        return_type: method.return_type,
        reserved_value_flags: 0,
        arguments: AbiMethodArgumentSlice::from_static(method.abi_arguments),
        rpc: method
            .rpc
            .map_or(AbiRpcConfigV1::NONE, |rpc| AbiRpcConfigV1 {
                present: 1,
                call_local: u8::from(rpc.call_local),
                reserved_bytes: [0; 2],
                mode: match rpc.mode {
                    RpcMode::Authority => AbiRpcMode::AUTHORITY,
                    RpcMode::AnyPeer => AbiRpcMode::ANY_PEER,
                },
                transfer_mode: match rpc.transfer_mode {
                    RpcTransferMode::Unreliable => AbiRpcTransferMode::UNRELIABLE,
                    RpcTransferMode::UnreliableOrdered => AbiRpcTransferMode::UNRELIABLE_ORDERED,
                    RpcTransferMode::Reliable => AbiRpcTransferMode::RELIABLE,
                },
                channel: rpc.channel,
                reserved_flags: 0,
            }),
        reserved: [
            core::ptr::from_ref(&method.abi_extensions) as usize,
            AbiMethodExtensionsV1::MINIMUM_SIZE as usize,
            0,
            0,
        ],
    };
    // SAFETY: The caller supplied a non-null writable ABI output slot.
    unsafe { output.write(descriptor) };
    AbiStatus::Ok
}

/// Allocates native state for one generated script type.
#[doc(hidden)]
pub unsafe extern "C" fn abi_create_state<T: ScriptMethods>(
    output: *mut *mut c_void,
) -> AbiCallResult {
    if output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "script state output pointer is null",
        );
    }
    catch_abi_panic(|| {
        let state = Box::into_raw(Box::new(T::__godot_rs_new())).cast();
        // SAFETY: The caller supplied a non-null writable state output slot.
        unsafe { output.write(state) };
        AbiCallResult::OK
    })
}

/// Releases state allocated by [`abi_create_state`].
#[doc(hidden)]
pub unsafe extern "C" fn abi_drop_state<T: ScriptMethods>(state: *mut c_void) {
    if !state.is_null() {
        let _ = catch_abi_panic(|| {
            // SAFETY: The module created this pointer for the same script type
            // and the Host releases it exactly once.
            unsafe { drop(Box::from_raw(state.cast::<T>())) };
            AbiCallResult::OK
        });
    }
}

/// Invokes one generated reflected method over the stable value ABI.
#[doc(hidden)]
pub unsafe extern "C" fn abi_call_method<T: ScriptMethods>(
    state: *mut c_void,
    method_id: u64,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    if state.is_null() || output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "reflected method received a null state or output pointer",
        );
    }
    catch_abi_panic(|| {
        let Some(block) = method_blocks::<T>()
            .find(|block| block.methods.iter().any(|method| method.id == method_id))
        else {
            return AbiCallResult::failure(
                AbiStatus::Unsupported,
                "reflected method ID is not present in this script",
            );
        };
        (block.invoke)(state, method_id, arguments, argument_count, output)
    })
}

/// Reads one generated script field through the stable project-module ABI.
#[doc(hidden)]
pub unsafe extern "C" fn abi_get_script_field<T: ScriptFieldAccess>(
    state: *mut c_void,
    field_index: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    if state.is_null() || output.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "field getter received a null state or output pointer",
        );
    }
    catch_abi_panic(|| {
        // SAFETY: The Host calls the descriptor paired with this script state.
        let state = unsafe { &*state.cast::<T>() };
        // SAFETY: The output contract is forwarded unchanged.
        unsafe { state.__godot_rs_get_field(field_index, output) }
    })
}

/// Writes one generated script field through the stable project-module ABI.
#[doc(hidden)]
pub unsafe extern "C" fn abi_set_script_field<T: ScriptFieldAccess>(
    state: *mut c_void,
    field_index: u32,
    value: AbiValueV1,
) -> AbiCallResult {
    if state.is_null() {
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "field setter received a null state pointer",
        );
    }
    catch_abi_panic(|| {
        // SAFETY: The Host calls the descriptor paired with this script state.
        let state = unsafe { &mut *state.cast::<T>() };
        // SAFETY: The fixed-layout value is forwarded unchanged.
        unsafe { state.__godot_rs_set_field(field_index, value) }
    })
}

/// Builds the ABI descriptor returned by a generated module index.
#[doc(hidden)]
#[must_use]
pub fn abi_script_descriptor<T: ScriptMethods>(
    source_path: &'static str,
    resource_uid: &'static str,
) -> AbiScriptDescriptorV1 {
    let field_count = u32::try_from(T::DESCRIPTOR.fields.len()).unwrap_or(u32::MAX);
    let methods = registered_methods::<T>();
    let method_count = u32::try_from(methods.len()).unwrap_or(u32::MAX);
    let uid_words = parse_resource_uid_text(resource_uid).and_then(encode_resource_uid_words);
    let uid_flag = if uid_words.is_some() {
        ABI_SCRIPT_EXTENSION_RESOURCE_UID
    } else {
        0
    };
    let uid_words = uid_words.unwrap_or([0; 2]);
    let (global_flag, global_name) = match T::DESCRIPTOR.global_name {
        Some(name) => (
            godot_api::abi::ABI_SCRIPT_EXTENSION_GLOBAL_CLASS,
            (name.as_ptr() as usize, name.len()),
        ),
        None => (0, (0, 0)),
    };
    let (base_script_flag, base_script) = match T::DESCRIPTOR.base_script {
        Some(path) => (
            godot_api::abi::ABI_SCRIPT_EXTENSION_BASE_SCRIPT,
            (path.as_ptr() as usize, path.len()),
        ),
        None => (0, (0, 0)),
    };
    AbiScriptDescriptorV1 {
        struct_size: AbiScriptDescriptorV1::MINIMUM_SIZE,
        reserved_flags: ABI_SCRIPT_EXTENSION_FIELD_ACCESS
            | uid_flag
            | global_flag
            | base_script_flag,
        source_path: AbiByteSlice::from_static(source_path),
        name: AbiByteSlice::from_static(T::DESCRIPTOR.name),
        base: AbiByteSlice::from_static(T::DESCRIPTOR.base),
        tool: u8::from(T::DESCRIPTOR.tool),
        reserved_bytes: [0; 7],
        field_count,
        method_count,
        get_field: Some(abi_get_field::<T>),
        get_method: Some(abi_get_method::<T>),
        create_state: Some(abi_create_state::<T>),
        drop_state: Some(abi_drop_state::<T>),
        lifecycle: lifecycle_table_from_registered(&methods),
        call_method: Some(abi_call_method::<T>),
        reserved: [
            abi_get_script_field::<T> as *const () as usize,
            abi_set_script_field::<T> as *const () as usize,
            uid_words[0],
            uid_words[1],
            global_name.0,
            global_name.1,
            base_script.0,
            base_script.1,
        ],
    }
}

fn parse_resource_uid_text(text: &str) -> Option<i64> {
    let digits = text.as_bytes().strip_prefix(b"uid://")?;
    if digits.is_empty() || (digits.len() > 1 && digits[0] == b'a') {
        return None;
    }
    let mut uid = 0_u64;
    for digit in digits {
        let value = match *digit {
            b'a'..=b'y' => u64::from(*digit - b'a'),
            b'0'..=b'8' => u64::from(*digit - b'0') + 25,
            _ => return None,
        };
        uid = uid.checked_mul(34)?.checked_add(value)?;
    }
    (uid <= i64::MAX as u64).then_some(uid as i64)
}

/// Writes a generated script descriptor into Host-owned memory.
///
/// # Safety
///
/// `output` must be a writable `AbiScriptDescriptorV1` slot.
#[doc(hidden)]
pub unsafe fn write_abi_script_descriptor<T: ScriptMethods>(
    source_path: &'static str,
    resource_uid: &'static str,
    output: *mut AbiScriptDescriptorV1,
) -> AbiStatus {
    if output.is_null() {
        return AbiStatus::InvalidArgument;
    }
    if !method_registry_is_valid::<T>() {
        return AbiStatus::InvalidArgument;
    }
    let descriptor = abi_script_descriptor::<T>(source_path, resource_uid);
    // SAFETY: The caller supplied a non-null writable descriptor output.
    unsafe { ptr::write(output, descriptor) };
    AbiStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::{FromAbiValue, IntoMethodResult, borrow_module_output, parse_resource_uid_text};
    use crate::abi::{AbiStatus, AbiValueV1};
    use crate::engine::{Node, ObjectRef};
    use crate::math::{Color, Vector2, Vector2i, Vector3, Vector3i};
    use crate::packed_array::{PackedByteArray, PackedStringArray, PackedVector3Array};
    use crate::rid::Rid;
    use crate::string_name::StringName;

    #[test]
    fn resource_uid_text_requires_godots_canonical_base_34_spelling() {
        assert_eq!(parse_resource_uid_text("uid://a"), Some(0));
        assert_eq!(parse_resource_uid_text("uid://c2"), Some(95));
        assert!(parse_resource_uid_text("uid://").is_none());
        assert!(parse_resource_uid_text("uid://aa").is_none());
        assert!(parse_resource_uid_text("uid://z").is_none());
        assert!(parse_resource_uid_text("uid://9").is_none());
        assert!(parse_resource_uid_text("uid://<invalid>").is_none());
        assert!(parse_resource_uid_text("uid://abc\n").is_none());
    }

    #[test]
    fn base_method_outputs_decode_through_a_borrowed_view() {
        let mut owned = AbiValueV1::NIL;
        assert_eq!(
            "Hello, Godot!".to_owned().write_result(&mut owned).status,
            AbiStatus::Ok
        );
        assert_ne!(owned.reserved_flags, 0);
        assert_eq!(
            String::from_abi(borrow_module_output(owned)),
            Some("Hello, Godot!".to_owned())
        );
        assert_eq!(
            // SAFETY: This is the sole release of the module-owned test result.
            unsafe { crate::module::drop_owned_value(owned) },
            AbiStatus::Ok
        );
    }

    #[test]
    fn reflected_math_values_use_the_packed_project_abi() {
        let vector2 = Vector2::new(12.0, -8.5);
        let mut encoded = AbiValueV1::NIL;
        assert_eq!(vector2.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(Vector2::from_abi(encoded), Some(vector2));

        let vector3 = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(vector3.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(Vector3::from_abi(encoded), Some(vector3));

        let color = Color::rgba(0.1, 0.2, 0.3, 0.4);
        assert_eq!(color.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(Color::from_abi(encoded), Some(color));

        let vector2i = Vector2i::new(i32::MIN, i32::MAX);
        assert_eq!(vector2i.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(Vector2i::from_abi(encoded), Some(vector2i));

        let vector3i = Vector3i::new(-1, 2, -3);
        assert_eq!(vector3i.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(Vector3i::from_abi(encoded), Some(vector3i));
    }

    #[test]
    fn reflected_packed_arrays_copy_arguments_and_transfer_owned_results() {
        let bytes = PackedByteArray::from(vec![0, 1, 127, 255]);
        let borrowed = AbiValueV1::from_borrowed_bytes(
            crate::abi::AbiValueType::PACKED_BYTE_ARRAY,
            bytes.__bytes(),
        );
        assert_eq!(PackedByteArray::from_abi(borrowed), Some(bytes));

        let strings = PackedStringArray::from(vec!["你好".into(), "Godot".into()]);
        let borrowed = AbiValueV1::from_borrowed_bytes(
            crate::abi::AbiValueType::PACKED_STRING_ARRAY,
            strings.__bytes(),
        );
        assert_eq!(PackedStringArray::from_abi(borrowed), Some(strings));

        let vectors = PackedVector3Array::from(vec![
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(-4.0, 5.0, -6.0),
        ]);
        let mut encoded = AbiValueV1::NIL;
        assert_eq!(
            vectors.clone().write_result(&mut encoded).status,
            AbiStatus::Ok
        );
        assert_eq!(
            encoded.type_,
            crate::abi::AbiValueType::PACKED_VECTOR3_ARRAY
        );
        assert_eq!(encoded.reserved_flags, crate::abi::ABI_VALUE_OWNED_BYTES);
        let (pointer, length) = encoded
            .byte_range(crate::abi::AbiValueType::PACKED_VECTOR3_ARRAY)
            .expect("owned packed result bytes");
        // SAFETY: The module-owned result remains live until the release below.
        let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
        let borrowed =
            AbiValueV1::from_borrowed_bytes(crate::abi::AbiValueType::PACKED_VECTOR3_ARRAY, bytes);
        assert_eq!(PackedVector3Array::from_abi(borrowed), Some(vectors));
        // SAFETY: This is the sole release of the module-owned result value.
        let released = unsafe { crate::module::drop_owned_value(encoded) };
        assert_eq!(released, AbiStatus::Ok);
    }

    #[test]
    fn reflected_object_values_preserve_nullability_and_instance_ids() {
        let object = ObjectRef::<Node>::__from_instance_id(42);
        let mut encoded = AbiValueV1::NIL;
        assert_eq!(object.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(ObjectRef::<Node>::from_abi(encoded), Some(object));
        assert_eq!(
            Option::<ObjectRef<Node>>::from_abi(encoded),
            Some(Some(object))
        );

        let null = AbiValueV1::from_object_id(0);
        assert_eq!(ObjectRef::<Node>::from_abi(null), None);
        assert_eq!(Option::<ObjectRef<Node>>::from_abi(null), Some(None));
    }

    #[test]
    fn reflected_rid_values_preserve_opaque_identity() {
        let rid = Rid::from_raw(u64::MAX);
        let mut encoded = AbiValueV1::NIL;
        assert_eq!(rid.write_result(&mut encoded).status, AbiStatus::Ok);
        assert_eq!(Rid::from_abi(encoded), Some(rid));
    }

    #[test]
    fn reflected_string_names_preserve_utf8_and_their_exact_type() {
        let borrowed = AbiValueV1::from_borrowed_string_name("玩家/生命值");
        assert_eq!(
            StringName::from_abi(borrowed),
            Some(StringName::from("玩家/生命值"))
        );
        assert_eq!(String::from_abi(borrowed), None);

        let mut encoded = AbiValueV1::NIL;
        assert_eq!(
            StringName::from("玩家/生命值")
                .write_result(&mut encoded)
                .status,
            AbiStatus::Ok
        );
        assert_eq!(encoded.type_, crate::abi::AbiValueType::STRING_NAME);
        assert_eq!(encoded.reserved_flags, crate::abi::ABI_VALUE_OWNED_UTF8);
        // SAFETY: This is the sole release of the module-owned test value.
        let released = unsafe { crate::module::drop_owned_value(encoded) };
        assert_eq!(released, AbiStatus::Ok);
    }
}
