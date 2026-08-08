use std::cell::RefCell;
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

static LATEST: OnceLock<Mutex<DebugSnapshot>> = OnceLock::new();

thread_local! {
    static ACTIVE_FRAMES: RefCell<Vec<DebugFrame>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct CallbackScope;

impl CallbackScope {
    pub(crate) fn enter(
        source: &str,
        function: &str,
        instance: usize,
        locals: Vec<(String, String)>,
    ) -> Self {
        let line = crate::script::source_for_path(source)
            .and_then(|source| crate::rust_source::find_function_line(&source, function))
            .and_then(|line| i64::try_from(line).ok())
            .map(|line| line + 1)
            .unwrap_or_default();
        ACTIVE_FRAMES.with(|frames| {
            frames.borrow_mut().push(DebugFrame {
                source: source.to_owned(),
                function: function.to_owned(),
                line,
                instance,
                locals,
            });
        });
        Self
    }
}

impl Drop for CallbackScope {
    fn drop(&mut self) {
        ACTIVE_FRAMES.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

pub(crate) fn record_error(error: String) {
    let frames =
        ACTIVE_FRAMES.with(|frames| frames.borrow().iter().rev().cloned().collect::<Vec<_>>());
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
    let active =
        ACTIVE_FRAMES.with(|frames| frames.borrow().iter().rev().cloned().collect::<Vec<_>>());
    if !active.is_empty() {
        return active;
    }
    LATEST
        .get_or_init(|| Mutex::new(DebugSnapshot::default()))
        .lock()
        .map(|snapshot| snapshot.frames.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_stack_is_innermost_first() {
        let outer = CallbackScope::enter("res://outer.rs", "outer", 1, Vec::new());
        let inner = CallbackScope::enter("res://inner.rs", "inner", 2, Vec::new());
        let frames = frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].function, "inner");
        assert_eq!(frames[1].function, "outer");
        drop(inner);
        drop(outer);
    }
}
