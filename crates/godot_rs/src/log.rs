use core::fmt;

/// Logging severity forwarded to the Host once a module is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

/// Unified logging entry used by the public macros.
///
/// Script modules bind this call to the Host. Native extensions route it
/// through Godot utilities, with a standard-error fallback outside Godot.
pub fn write(level: Level, arguments: fmt::Arguments<'_>) {
    crate::module::write_log(level, arguments);
}

/// Prints a formatted message through the godot-rust Host.
#[macro_export]
macro_rules! godot_print {
    ($($argument:tt)*) => {
        $crate::log::write(
            $crate::log::Level::Info,
            core::format_args!($($argument)*),
        )
    };
}

/// Prints diagnostic details when verbose Godot output is enabled.
#[macro_export]
macro_rules! godot_debug {
    ($($argument:tt)*) => {
        $crate::log::write(
            $crate::log::Level::Debug,
            core::format_args!($($argument)*),
        )
    };
}

/// Prints a formatted warning through the godot-rust Host.
#[macro_export]
macro_rules! godot_warn {
    ($($argument:tt)*) => {
        $crate::log::write(
            $crate::log::Level::Warning,
            core::format_args!($($argument)*),
        )
    };
}

/// Reports a formatted error through Godot's error output.
#[macro_export]
macro_rules! godot_error {
    ($($argument:tt)*) => {
        $crate::log::write(
            $crate::log::Level::Error,
            core::format_args!($($argument)*),
        )
    };
}
