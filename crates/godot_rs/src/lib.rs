#![doc = "Public Rust SDK for the godot-rust plugin."]

extern crate self as godot_rs;

pub mod callable;
pub mod engine;
pub mod error;
pub mod log;
pub mod math;
#[doc(hidden)]
pub mod module;
pub mod native;
pub mod node_path;
pub mod packed_array;
pub mod rid;
pub mod script;
pub mod signal;
pub mod string_name;
pub mod task;
pub mod variant;

pub use godot_macro::script;
#[doc(hidden)]
pub use inventory;

/// SDK version compiled into the project module.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Minimum Godot Major/Minor API selected for this project build.
pub const GODOT_API: &str = godot_api::SELECTED_GODOT_API;

/// Host ABI used by generated Script Mode modules.
#[doc(hidden)]
pub mod abi {
    pub use godot_api::abi::*;
}

/// Imports used by ordinary Rust scripts.
pub mod prelude {
    pub use crate::callable::Callable;
    pub use crate::engine::*;
    pub use crate::error::{EngineError, EngineErrorKind, EngineResult, ScriptError, ScriptResult};
    pub use crate::math::{
        Aabb, Basis, Color, Plane, Projection, Quaternion, Rect2, Rect2i, Transform2D, Transform3D,
        Vector2, Vector2i, Vector3, Vector3i, Vector4, Vector4i,
    };
    pub use crate::node_path::NodePath;
    pub use crate::packed_array::{
        PackedByteArray, PackedColorArray, PackedFloat32Array, PackedFloat64Array,
        PackedInt32Array, PackedInt64Array, PackedStringArray, PackedVector2Array,
        PackedVector3Array, PackedVector4Array,
    };
    pub use crate::rid::Rid;
    pub use crate::script;
    pub use crate::script::ScriptSuper;
    pub use crate::signal::Signal;
    pub use crate::string_name::StringName;
    pub use crate::task::{
        BlockingTask, BlockingTaskError, SignalFuture, TaskHandle, Timeout, TimeoutError,
        next_frame, sleep, spawn, spawn_blocking, timeout,
    };
    pub use crate::variant::{Array, Dictionary, Variant, VariantKind};
    pub use crate::{godot_print, godot_warn};
}
