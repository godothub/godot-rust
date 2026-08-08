use core::fmt;

/// Logging severity forwarded to the Host once a module is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Info,
    Warning,
}

/// Allocation-free logging entry used by the public macros.
///
/// M3 keeps this as a stable SDK call site; the project-module runtime binds it
/// to `HostApiV1::log` when a module generation starts.
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
