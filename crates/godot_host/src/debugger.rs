use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DebugFrame {
    pub(crate) source: String,
    pub(crate) function: String,
    pub(crate) line: i64,
    pub(crate) instance: usize,
    pub(crate) locals: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
struct DebugSnapshot {
    error: String,
    frames: Vec<DebugFrame>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn initialize(interface: crate::interface::EngineInterface) {
    let active = (|| {
        let singleton = interface.global_get_singleton?;
        let name = crate::string_name::StaticStringName::new(interface, c"EngineDebugger");
        // SAFETY: The singleton name is initialized and owned during the lookup.
        let object = unsafe { singleton(name.as_ptr()) };
        if object.is_null() {
            return None;
        }
        let method = crate::runtime::resolve_method(
            interface,
            c"EngineDebugger",
            c"is_active",
            2_240_911_060,
        )
        .ok()?;
        let call = interface.object_method_bind_ptrcall?;
        let mut active: godot_api::GDExtensionBool = 0;
        // SAFETY: EngineDebugger.is_active has no arguments and returns bool.
        unsafe {
            call(
                method,
                object,
                core::ptr::null(),
                core::ptr::from_mut(&mut active).cast(),
            )
        };
        Some(active != 0)
    })()
    .unwrap_or(false);
    ENABLED.store(active, Ordering::Release);
}

static LATEST: OnceLock<Mutex<DebugSnapshot>> = OnceLock::new();

thread_local! {
    static ACTIVE_FRAMES: RefCell<Vec<DebugFrame>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct CallbackScope {
    active: bool,
}

impl CallbackScope {
    pub(crate) fn enter(
        source: &str,
        function: &str,
        instance: usize,
        locals: impl FnOnce() -> Vec<(String, String)>,
    ) -> Self {
        let active = cfg!(test) || ENABLED.load(Ordering::Relaxed);
        if !active {
            return Self { active: false };
        }
        let locals = locals();
        ACTIVE_FRAMES.with(|frames| {
            frames.borrow_mut().push(DebugFrame {
                source: source.to_owned(),
                function: function.to_owned(),
                line: 0,
                instance,
                locals,
            });
        });
        Self { active: true }
    }
}

impl Drop for CallbackScope {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        ACTIVE_FRAMES.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

pub(crate) fn record_error(error: String) {
    let mut frames =
        ACTIVE_FRAMES.with(|frames| frames.borrow().iter().rev().cloned().collect::<Vec<_>>());
    resolve_lines(&mut frames);
    if let Ok(mut latest) = LATEST
        .get_or_init(|| Mutex::new(DebugSnapshot::default()))
        .lock()
    {
        *latest = DebugSnapshot { error, frames };
    }
}

pub(crate) fn error() -> String {
    LATEST
        .get_or_init(|| Mutex::new(DebugSnapshot::default()))
        .lock()
        .map(|snapshot| snapshot.error.clone())
        .unwrap_or_default()
}

pub(crate) fn frames() -> Vec<DebugFrame> {
    let mut active =
        ACTIVE_FRAMES.with(|frames| frames.borrow().iter().rev().cloned().collect::<Vec<_>>());
    if !active.is_empty() {
        resolve_lines(&mut active);
        return active;
    }
    LATEST
        .get_or_init(|| Mutex::new(DebugSnapshot::default()))
        .lock()
        .map(|snapshot| snapshot.frames.clone())
        .unwrap_or_default()
}

// Source text is consulted only when the debugger requests a stack or a
// callback fails. Ordinary callback entry never copies or scans source text.
fn resolve_lines(frames: &mut [DebugFrame]) {
    for frame in frames {
        frame.line = crate::script::source_for_path(&frame.source)
            .and_then(|source| crate::rust_source::find_function_line(&source, &frame.function))
            .and_then(|line| i64::try_from(line).ok())
            .map(|line| line + 1)
            .unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_stack_is_innermost_first() {
        let outer = CallbackScope::enter("res://outer.rs", "outer", 1, Vec::new);
        let inner = CallbackScope::enter("res://inner.rs", "inner", 2, Vec::new);
        let frames = frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].function, "inner");
        assert_eq!(frames[1].function, "outer");
        drop(inner);
        drop(outer);
    }
}
