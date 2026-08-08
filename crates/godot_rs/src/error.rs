use core::fmt;
use godot_rs_api::abi::{AbiByteSlice, AbiCallResult, AbiStatus};

/// Error returned by a fallible Godot script callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScriptError {
    message: &'static str,
}

impl ScriptError {
    /// Creates an allocation-free script error.
    #[must_use]
    pub const fn new(message: &'static str) -> Self {
        Self { message }
    }

    /// User-facing error text.
    #[must_use]
    pub const fn message(self) -> &'static str {
        self.message
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

/// Result type understood by generated lifecycle dispatch.
pub type ScriptResult<T> = Result<T, ScriptError>;

/// Stable category for a failed Godot engine operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineErrorKind {
    InvalidArgument,
    Unsupported,
    StaleObject,
    ReentrantCall,
    Panic,
    Internal,
    CallFailed,
}

/// Error returned by a generated Godot API method.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineError {
    kind: EngineErrorKind,
    message: String,
}

impl EngineError {
    /// Machine-readable failure category.
    #[must_use]
    pub const fn kind(&self) -> EngineErrorKind {
        self.kind
    }

    /// Diagnostic text copied from the Host during the failed call.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn unavailable(message: &'static str) -> Self {
        Self {
            kind: EngineErrorKind::Unsupported,
            message: message.to_owned(),
        }
    }

    pub(crate) fn invalid_result(message: impl Into<String>) -> Self {
        Self {
            kind: EngineErrorKind::Internal,
            message: message.into(),
        }
    }

    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self {
            kind: EngineErrorKind::InvalidArgument,
            message: message.into(),
        }
    }

    pub(crate) fn stale_object(message: impl Into<String>) -> Self {
        Self {
            kind: EngineErrorKind::StaleObject,
            message: message.into(),
        }
    }

    pub(crate) fn from_abi(result: AbiCallResult) -> Self {
        let kind = match result.status {
            AbiStatus::InvalidArgument => EngineErrorKind::InvalidArgument,
            AbiStatus::Unsupported => EngineErrorKind::Unsupported,
            AbiStatus::StaleHandle => EngineErrorKind::StaleObject,
            AbiStatus::ReentrantCall => EngineErrorKind::ReentrantCall,
            AbiStatus::Panic => EngineErrorKind::Panic,
            AbiStatus::Internal | AbiStatus::Ok => EngineErrorKind::Internal,
            AbiStatus::CallbackFailed => EngineErrorKind::CallFailed,
        };
        Self {
            kind,
            message: copy_abi_message(result.message),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

/// Result returned by generated, type-safe Godot API methods.
pub type EngineResult<T> = Result<T, EngineError>;

impl From<EngineError> for ScriptError {
    fn from(error: EngineError) -> Self {
        Self::new(engine_error_callback_message(error.kind))
    }
}

pub(crate) const fn engine_error_callback_message(kind: EngineErrorKind) -> &'static str {
    match kind {
        EngineErrorKind::InvalidArgument => "Godot engine call received an invalid argument",
        EngineErrorKind::Unsupported => "Godot engine call is not supported by this Host",
        EngineErrorKind::StaleObject => "Godot object no longer exists",
        EngineErrorKind::ReentrantCall => "Godot engine call attempted invalid script re-entry",
        EngineErrorKind::Panic => "Godot engine call panicked",
        EngineErrorKind::Internal => "godot-rust could not complete the Godot engine call",
        EngineErrorKind::CallFailed => "Godot rejected the engine call",
    }
}

fn copy_abi_message(message: AbiByteSlice) -> String {
    const MAX_ERROR_BYTES: usize = 4096;
    if message.len == 0 {
        return "Godot engine call failed without a diagnostic".to_owned();
    }
    if message.ptr.is_null() || message.len > MAX_ERROR_BYTES {
        return "Godot engine call returned invalid diagnostic text".to_owned();
    }
    // SAFETY: Host callback messages are borrowed for the synchronous call;
    // the SDK bounds and copies the bytes before returning to user code.
    let bytes = unsafe { core::slice::from_raw_parts(message.ptr, message.len) };
    core::str::from_utf8(bytes).map_or_else(
        |_| "Godot engine call returned non-UTF-8 diagnostic text".to_owned(),
        str::to_owned,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_errors_own_host_diagnostics_and_preserve_categories() {
        let text = String::from("object no longer exists");
        let error = EngineError::from_abi(AbiCallResult {
            status: AbiStatus::StaleHandle,
            message: AbiByteSlice {
                ptr: text.as_ptr(),
                len: text.len(),
            },
        });
        drop(text);

        assert_eq!(error.kind(), EngineErrorKind::StaleObject);
        assert_eq!(error.message(), "object no longer exists");
        assert_eq!(error.to_string(), "object no longer exists");
    }

    #[test]
    fn invalid_host_diagnostics_fail_closed() {
        let error = EngineError::from_abi(AbiCallResult {
            status: AbiStatus::Internal,
            message: AbiByteSlice {
                ptr: core::ptr::null(),
                len: 1,
            },
        });
        assert_eq!(
            error.message(),
            "Godot engine call returned invalid diagnostic text"
        );
    }

    #[test]
    fn engine_errors_convert_to_callback_safe_static_messages() {
        let error = EngineError {
            kind: EngineErrorKind::StaleObject,
            message: "dynamic Host detail".into(),
        };
        let script_error = ScriptError::from(error);
        assert_eq!(script_error.message(), "Godot object no longer exists");
    }
}
