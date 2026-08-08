use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProfileEntry {
    pub(crate) signature: String,
    pub(crate) call_count: u64,
    pub(crate) total_time: u64,
    pub(crate) self_time: u64,
    pub(crate) internal_time: u64,
}

#[derive(Default)]
struct ProfileState {
    accumulated: HashMap<String, ProfileEntry>,
    current_frame: HashMap<String, ProfileEntry>,
    last_frame: HashMap<String, ProfileEntry>,
    save_native_calls: bool,
}

struct ActiveCall {
    signature: String,
    started: Instant,
    child_time: u64,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: OnceLock<Mutex<ProfileState>> = OnceLock::new();

thread_local! {
    static ACTIVE_CALLS: RefCell<Vec<ActiveCall>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct ProfileScope {
    active: bool,
}

impl ProfileScope {
    pub(crate) fn enter(source: &str, function: &str) -> Self {
        Self::enter_signature(format!("{source}::{function}"), true)
    }

    pub(crate) fn enter_native(owner: &str, member: &str) -> Self {
        Self::enter_signature(format!("[native] {owner}.{member}"), saves_native_calls())
    }

    fn enter_signature(signature: String, allowed: bool) -> Self {
        let active = allowed && ENABLED.load(Ordering::Relaxed);
        if active {
            ACTIVE_CALLS.with(|calls| {
                calls.borrow_mut().push(ActiveCall {
                    signature,
                    started: Instant::now(),
                    child_time: 0,
                });
            });
        }
        Self { active }
    }
}

impl Drop for ProfileScope {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let completed = ACTIVE_CALLS.with(|calls| {
            let mut calls = calls.borrow_mut();
            let completed = calls.pop()?;
            let elapsed =
                u64::try_from(completed.started.elapsed().as_micros()).unwrap_or(u64::MAX);
            if let Some(parent) = calls.last_mut() {
                parent.child_time = parent.child_time.saturating_add(elapsed);
            }
            Some((
                completed.signature,
                elapsed,
                elapsed.saturating_sub(completed.child_time),
            ))
        });
        let Some((signature, total_time, self_time)) = completed else {
            return;
        };
        if let Ok(mut state) = STATE
            .get_or_init(|| Mutex::new(ProfileState::default()))
            .lock()
        {
            record(&mut state.accumulated, &signature, total_time, self_time);
            record(&mut state.current_frame, &signature, total_time, self_time);
        }
    }
}

fn record(
    entries: &mut HashMap<String, ProfileEntry>,
    signature: &str,
    total_time: u64,
    self_time: u64,
) {
    let entry = entries
        .entry(signature.to_owned())
        .or_insert_with(|| ProfileEntry {
            signature: signature.to_owned(),
            ..ProfileEntry::default()
        });
    entry.call_count = entry.call_count.saturating_add(1);
    entry.total_time = entry.total_time.saturating_add(total_time);
    entry.self_time = entry.self_time.saturating_add(self_time);
}

pub(crate) fn start() {
    if let Ok(mut state) = STATE
        .get_or_init(|| Mutex::new(ProfileState::default()))
        .lock()
    {
        state.accumulated.clear();
        state.current_frame.clear();
        state.last_frame.clear();
    }
    ENABLED.store(true, Ordering::Release);
}

pub(crate) fn stop() {
    ENABLED.store(false, Ordering::Release);
}

pub(crate) fn set_save_native_calls(enabled: bool) {
    if let Ok(mut state) = STATE
        .get_or_init(|| Mutex::new(ProfileState::default()))
        .lock()
    {
        state.save_native_calls = enabled;
    }
}

pub(crate) fn saves_native_calls() -> bool {
    STATE
        .get_or_init(|| Mutex::new(ProfileState::default()))
        .lock()
        .map(|state| state.save_native_calls)
        .unwrap_or(false)
}

pub(crate) fn next_frame() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    if let Ok(mut state) = STATE
        .get_or_init(|| Mutex::new(ProfileState::default()))
        .lock()
    {
        state.last_frame = std::mem::take(&mut state.current_frame);
    }
}

pub(crate) fn accumulated() -> Vec<ProfileEntry> {
    snapshot(|state| &state.accumulated)
}

pub(crate) fn frame() -> Vec<ProfileEntry> {
    snapshot(|state| &state.last_frame)
}

fn snapshot(
    select: impl FnOnce(&ProfileState) -> &HashMap<String, ProfileEntry>,
) -> Vec<ProfileEntry> {
    let mut entries = STATE
        .get_or_init(|| Mutex::new(ProfileState::default()))
        .lock()
        .map(|state| select(&state).values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    entries.sort_unstable_by(|left, right| left.signature.cmp(&right.signature));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiling_counts_calls_and_rolls_frame_data() {
        start();
        {
            let _scope = ProfileScope::enter("res://player.rs", "_process");
        }
        assert_eq!(accumulated()[0].call_count, 1);
        next_frame();
        assert_eq!(frame()[0].call_count, 1);
        assert!(frame()[0].self_time <= frame()[0].total_time);
        stop();
    }
}
