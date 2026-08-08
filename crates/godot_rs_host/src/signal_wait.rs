use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};

use godot_rs_api::abi::{AbiCallResult, AbiStatus, AbiValueV1};

use crate::callable_value::{NativeCallable, SignalWaitState};
use crate::engine_call::value::ValueError;
use crate::signal_value::NativeSignal;

const CONNECT_ONE_SHOT: i64 = 4;
const MAX_SIGNAL_WATCHES: usize = 65_536;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static WATCHES: OnceLock<Mutex<HashMap<u64, SignalWatch>>> = OnceLock::new();

struct SignalWatch {
    context: usize,
    state: Arc<SignalWaitState>,
    signal: NativeSignal,
    callable: NativeCallable,
}

pub(crate) unsafe extern "C" fn watch_signal_from_module(
    context: *mut c_void,
    signal: AbiValueV1,
    output_token: *mut u64,
) -> AbiCallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        watch_signal(context, signal, output_token)
    })) {
        Ok(Ok(())) => AbiCallResult::OK,
        Ok(Err(error)) => AbiCallResult::failure(error.status(), error.message()),
        Err(_) => AbiCallResult::failure(
            AbiStatus::Panic,
            "godot-rust caught a panic while connecting a Signal future",
        ),
    }
}

pub(crate) unsafe extern "C" fn poll_signal_from_module(
    context: *mut c_void,
    token: u64,
    output_fired: *mut u8,
) -> AbiCallResult {
    match catch_unwind(AssertUnwindSafe(|| {
        poll_signal(context, token, output_fired)
    })) {
        Ok(Ok(())) => AbiCallResult::OK,
        Ok(Err(error)) => AbiCallResult::failure(error.status(), error.message()),
        Err(_) => AbiCallResult::failure(
            AbiStatus::Panic,
            "godot-rust caught a panic while polling a Signal future",
        ),
    }
}

pub(crate) unsafe extern "C" fn cancel_signal_from_module(
    context: *mut c_void,
    token: u64,
) -> AbiStatus {
    match catch_unwind(AssertUnwindSafe(|| cancel_signal(context, token))) {
        Ok(Ok(())) => AbiStatus::Ok,
        Ok(Err(error)) => error.status(),
        Err(_) => AbiStatus::Panic,
    }
}

fn watch_signal(
    context: *mut c_void,
    signal: AbiValueV1,
    output_token: *mut u64,
) -> Result<(), ValueError> {
    if context.is_null() {
        return Err(ValueError::new(
            AbiStatus::InvalidArgument,
            "Signal future Host context is null",
        ));
    }
    if output_token.is_null() {
        return Err(ValueError::new(
            AbiStatus::InvalidArgument,
            "Signal future token output is null",
        ));
    }
    let interface = crate::script_instance::active_engine_interface().ok_or_else(|| {
        ValueError::new(
            AbiStatus::Unsupported,
            "Godot Signals can only be awaited from a cooperative main-thread task",
        )
    })?;
    let get_object = interface.object_get_instance_from_id.ok_or_else(|| {
        ValueError::new(
            AbiStatus::Internal,
            "Godot Object instance lookup is unavailable",
        )
    })?;
    let mut signal = NativeSignal::from_abi(interface, signal, |instance_id| {
        // SAFETY: The interface is live in the cooperative task scope.
        let object = unsafe { get_object(instance_id) };
        if object.is_null() {
            Err(ValueError::new(
                AbiStatus::StaleHandle,
                "Godot Signal owner no longer exists",
            ))
        } else {
            Ok(object)
        }
    })?;
    if signal.object_id() == 0 {
        return Err(ValueError::new(
            AbiStatus::InvalidArgument,
            "an unbound Godot Signal cannot be awaited",
        ));
    }

    let token = next_token();
    let state = Arc::new(SignalWaitState::new());
    let callable = NativeCallable::from_signal_waiter(interface, state.clone(), token)?;
    let error = signal.connect(&callable, CONNECT_ONE_SHOT)?;
    if error != 0 {
        state.deactivate();
        return Err(ValueError::new(
            AbiStatus::CallbackFailed,
            "Godot rejected the one-shot Signal connection",
        ));
    }

    let watches = WATCHES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut watches = watches.lock().map_err(|_| {
        ValueError::new(AbiStatus::Internal, "Signal future registry is unavailable")
    })?;
    if watches.len() >= MAX_SIGNAL_WATCHES {
        state.deactivate();
        drop(watches);
        let _ = signal.disconnect(&callable);
        return Err(ValueError::new(
            AbiStatus::Unsupported,
            "the project generation has too many pending Signal futures",
        ));
    }
    watches.insert(
        token,
        SignalWatch {
            context: context as usize,
            state,
            signal,
            callable,
        },
    );
    // SAFETY: Null was rejected and the project module owns writable storage
    // for the synchronous callback.
    unsafe { *output_token = token };
    Ok(())
}

fn poll_signal(context: *mut c_void, token: u64, output_fired: *mut u8) -> Result<(), ValueError> {
    if context.is_null() || token == 0 || output_fired.is_null() {
        return Err(ValueError::new(
            AbiStatus::InvalidArgument,
            "Signal future poll arguments are invalid",
        ));
    }
    let watches = WATCHES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut watches = watches.lock().map_err(|_| {
        ValueError::new(AbiStatus::Internal, "Signal future registry is unavailable")
    })?;
    let watch = watches.get(&token).ok_or_else(|| {
        ValueError::new(
            AbiStatus::StaleHandle,
            "Signal future token is no longer active",
        )
    })?;
    if watch.context != context as usize {
        return Err(ValueError::new(
            AbiStatus::StaleHandle,
            "Signal future token belongs to a different module generation",
        ));
    }
    let fired = watch.state.fired();
    // SAFETY: Null was rejected and the project module owns writable storage
    // for the synchronous callback.
    unsafe { *output_fired = u8::from(fired) };
    if fired {
        if let Some(watch) = watches.remove(&token) {
            watch.state.deactivate();
        }
    }
    Ok(())
}

fn cancel_signal(context: *mut c_void, token: u64) -> Result<(), ValueError> {
    if context.is_null() || token == 0 {
        return Err(ValueError::new(
            AbiStatus::InvalidArgument,
            "Signal future cancellation arguments are invalid",
        ));
    }
    let watches = WATCHES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut watches = watches.lock().map_err(|_| {
        ValueError::new(AbiStatus::Internal, "Signal future registry is unavailable")
    })?;
    let watch = watches.get(&token).ok_or_else(|| {
        ValueError::new(
            AbiStatus::StaleHandle,
            "Signal future token is no longer active",
        )
    })?;
    if watch.context != context as usize {
        return Err(ValueError::new(
            AbiStatus::StaleHandle,
            "Signal future token belongs to a different module generation",
        ));
    }
    let mut watch = watches
        .remove(&token)
        .expect("validated Signal future remains registered");
    drop(watches);
    watch.state.deactivate();
    watch.signal.disconnect(&watch.callable)
}

pub(crate) fn cancel_context(context: usize) {
    let Some(watches) = WATCHES.get() else {
        return;
    };
    let Ok(mut watches) = watches.lock() else {
        return;
    };
    let tokens = watches
        .iter()
        .filter_map(|(token, watch)| (watch.context == context).then_some(*token))
        .collect::<Vec<_>>();
    let mut removed = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let Some(watch) = watches.remove(&token) {
            removed.push(watch);
        }
    }
    drop(watches);
    for mut watch in removed {
        watch.state.deactivate();
        let _ = watch.signal.disconnect(&watch.callable);
    }
}

fn next_token() -> u64 {
    loop {
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        if token != 0 {
            return token;
        }
    }
}
