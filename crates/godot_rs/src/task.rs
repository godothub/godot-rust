//! Main-thread cooperative tasks for Script Mode.
//!
//! Tasks are polled once per Godot frame by the Script Host. They never move
//! to a worker thread, so generated engine handles remain subject to the same
//! main-thread rules as ordinary script callbacks.

use core::cell::{Cell, RefCell};
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::error::{EngineError, EngineResult};
use crate::signal::Signal;

thread_local! {
    static TASKS: RefCell<Vec<Task>> = const { RefCell::new(Vec::new()) };
    static NEXT_TASK_ID: Cell<u64> = const { Cell::new(1) };
}

struct Task {
    id: u64,
    cancelled: Rc<AtomicBool>,
    future: Option<Pin<Box<dyn Future<Output = ()>>>>,
}

/// A cancellation handle for one cooperative Rust task.
#[derive(Clone)]
pub struct TaskHandle {
    id: u64,
    cancelled: Rc<AtomicBool>,
}

impl TaskHandle {
    /// Requests cancellation. The future is dropped at the next Host frame.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Stable identity within this loaded project-module generation.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }
}

/// Schedules a `!Send` future on the Godot main thread.
///
/// Newly spawned tasks are first polled on the next engine frame. Panicking
/// tasks are isolated and removed without unwinding across the module ABI.
pub fn spawn(future: impl Future<Output = ()> + 'static) -> TaskHandle {
    let id = NEXT_TASK_ID.with(|next| {
        let id = next.get();
        next.set(id.checked_add(1).unwrap_or(1));
        id
    });
    let cancelled = Rc::new(AtomicBool::new(false));
    TASKS.with(|tasks| {
        tasks.borrow_mut().push(Task {
            id,
            cancelled: cancelled.clone(),
            future: Some(Box::pin(future)),
        });
    });
    TaskHandle { id, cancelled }
}

/// Yields until the next Godot frame.
#[must_use]
pub const fn next_frame() -> NextFrame {
    NextFrame { yielded: false }
}

/// Future returned by [`next_frame`].
pub struct NextFrame {
    yielded: bool,
}

impl Future for NextFrame {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Yields until a monotonic duration has elapsed.
#[must_use]
pub fn sleep(duration: Duration) -> Sleep {
    Sleep {
        deadline: Instant::now().checked_add(duration),
    }
}

/// Future returned by [`sleep`].
pub struct Sleep {
    deadline: Option<Instant>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        match self.deadline {
            Some(deadline) if Instant::now() < deadline => Poll::Pending,
            Some(_) => Poll::Ready(()),
            None => Poll::Pending,
        }
    }
}

/// Error returned when [`timeout`] reaches its deadline first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeoutError;

impl core::fmt::Display for TimeoutError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("the Godot task deadline elapsed")
    }
}

impl std::error::Error for TimeoutError {}

/// Applies a monotonic deadline to a cooperative main-thread future.
#[must_use]
pub fn timeout<F: Future>(duration: Duration, future: F) -> Timeout<F> {
    Timeout {
        future: Box::pin(future),
        deadline: Instant::now().checked_add(duration),
    }
}

/// Future returned by [`timeout`].
pub struct Timeout<F: Future> {
    future: Pin<Box<F>>,
    deadline: Option<Instant>,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, TimeoutError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Poll::Ready(output) = this.future.as_mut().poll(context) {
            return Poll::Ready(Ok(output));
        }
        match this.deadline {
            Some(deadline) if Instant::now() >= deadline => Poll::Ready(Err(TimeoutError)),
            Some(_) | None => Poll::Pending,
        }
    }
}

/// Failure reported by a [`spawn_blocking`] worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockingTaskError {
    /// The operating system refused to create the worker thread.
    Spawn(String),
    /// The worker closure panicked.
    Panic,
    /// The worker exited without delivering a result.
    Disconnected,
}

impl core::fmt::Display for BlockingTaskError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Spawn(message) => write!(formatter, "could not start Rust worker: {message}"),
            Self::Panic => formatter.write_str("Rust worker panicked"),
            Self::Disconnected => formatter.write_str("Rust worker exited without a result"),
        }
    }
}

impl std::error::Error for BlockingTaskError {}

/// Runs CPU-bound or blocking Rust work away from the Godot main thread.
///
/// Engine objects and generated Godot APIs must stay on the main thread and
/// therefore must not be moved into this closure. Dropping the returned
/// future detaches an already-running worker; Rust threads cannot be forcibly
/// cancelled safely.
#[must_use]
pub fn spawn_blocking<T, F>(work: F) -> BlockingTask<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawn = std::thread::Builder::new()
        .name("godot-rust-worker".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(work)).map_err(|_| BlockingTaskError::Panic);
            let _ = sender.send(result);
        });
    BlockingTask {
        receiver: spawn.map_or_else(
            |error| Err(BlockingTaskError::Spawn(error.to_string())),
            |_| Ok(receiver),
        ),
    }
}

/// Future returned by [`spawn_blocking`].
pub struct BlockingTask<T> {
    receiver: Result<Receiver<Result<T, BlockingTaskError>>, BlockingTaskError>,
}

impl<T> Unpin for BlockingTask<T> {}

impl<T> Future for BlockingTask<T> {
    type Output = Result<T, BlockingTaskError>;

    fn poll(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        let receiver = match &self.receiver {
            Ok(receiver) => receiver,
            Err(_) => {
                let Err(error) =
                    core::mem::replace(&mut self.receiver, Err(BlockingTaskError::Disconnected))
                else {
                    unreachable!("blocking task retained its startup error")
                };
                return Poll::Ready(Err(error));
            }
        };
        match receiver.try_recv() {
            Ok(result) => Poll::Ready(result),
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => Poll::Ready(Err(BlockingTaskError::Disconnected)),
        }
    }
}

enum SignalFutureState {
    New(Result<Box<[u8]>, EngineError>),
    Watching(u64),
    Done,
}

/// Future that completes after the next emission of one Godot Signal.
///
/// The Host installs an ordinary one-shot Godot connection. Dropping or
/// cancelling the containing task disconnects it deterministically.
#[must_use]
pub struct SignalFuture {
    state: SignalFutureState,
}

impl SignalFuture {
    pub(crate) fn new<T>(signal: &Signal<T>) -> Self {
        let encoded = signal
            .__bytes()
            .map(|bytes| bytes.to_vec().into_boxed_slice())
            .map_err(|error| EngineError::invalid_argument(error.message()));
        Self {
            state: SignalFutureState::New(encoded),
        }
    }
}

impl Future for SignalFuture {
    type Output = EngineResult<()>;

    fn poll(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.state, SignalFutureState::New(_)) {
            let SignalFutureState::New(encoded) =
                core::mem::replace(&mut self.state, SignalFutureState::Done)
            else {
                unreachable!("Signal future state was checked")
            };
            let bytes = match encoded {
                Ok(bytes) => bytes,
                Err(error) => return Poll::Ready(Err(error)),
            };
            let value = godot_rs_api::abi::AbiValueV1::from_borrowed_bytes(
                godot_rs_api::abi::AbiValueType::SIGNAL,
                &bytes,
            );
            match crate::module::watch_signal(value) {
                Ok(token) => {
                    self.state = SignalFutureState::Watching(token);
                    return Poll::Pending;
                }
                Err(error) => return Poll::Ready(Err(error)),
            }
        }

        let SignalFutureState::Watching(token) = self.state else {
            panic!("SignalFuture polled after completion");
        };
        match crate::module::poll_signal(token) {
            Ok(false) => Poll::Pending,
            Ok(true) => {
                self.state = SignalFutureState::Done;
                Poll::Ready(Ok(()))
            }
            Err(error) => {
                self.state = SignalFutureState::Done;
                Poll::Ready(Err(error))
            }
        }
    }
}

impl Drop for SignalFuture {
    fn drop(&mut self) {
        if let SignalFutureState::Watching(token) = self.state {
            crate::module::cancel_signal(token);
        }
    }
}

#[doc(hidden)]
pub fn poll_frame() {
    let mut active = TASKS.with(|tasks| core::mem::take(&mut *tasks.borrow_mut()));
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    for mut task in active.drain(..) {
        if task.cancelled.load(Ordering::Acquire) {
            drop_task_future(&mut task);
            continue;
        }
        let poll = catch_unwind(AssertUnwindSafe(|| {
            task.future
                .as_mut()
                .expect("active task retains its future")
                .as_mut()
                .poll(&mut context)
        }));
        match poll {
            Ok(Poll::Pending) => TASKS.with(|tasks| tasks.borrow_mut().push(task)),
            Ok(Poll::Ready(())) => drop_task_future(&mut task),
            Err(_) => {
                crate::godot_warn!("Rust task {} panicked and was cancelled", task.id);
                drop_task_future(&mut task);
            }
        }
    }
}

#[doc(hidden)]
pub fn cancel_all() {
    let tasks = TASKS.with(|tasks| core::mem::take(&mut *tasks.borrow_mut()));
    for mut task in tasks {
        drop_task_future(&mut task);
    }
}

fn drop_task_future(task: &mut Task) {
    let Some(future) = task.future.take() else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| drop(future))).is_err() {
        crate::godot_warn!(
            "Rust task {} panicked while being cancelled or released",
            task.id
        );
    }
}

fn noop_waker() -> Waker {
    // SAFETY: The vtable never dereferences or owns the null data pointer.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)) }
}

unsafe fn clone_noop(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)
}

unsafe fn wake_noop(_: *const ()) {}

static NOOP_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_noop, wake_noop, wake_noop, wake_noop);

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    #[test]
    fn tasks_yield_and_can_be_cancelled() {
        cancel_all();
        let frames = Rc::new(Cell::new(0));
        let output = frames.clone();
        let handle = spawn(async move {
            output.set(1);
            next_frame().await;
            output.set(2);
        });
        poll_frame();
        assert_eq!(frames.get(), 1);
        handle.cancel();
        poll_frame();
        assert_eq!(frames.get(), 1);
        cancel_all();
    }

    #[test]
    fn panicking_task_does_not_stop_other_tasks() {
        cancel_all();
        let completed = Rc::new(Cell::new(false));
        spawn(async { panic!("task panic fixture") });
        let output = completed.clone();
        spawn(async move { output.set(true) });
        poll_frame();
        assert!(completed.get());
        cancel_all();
    }

    #[test]
    fn panicking_future_drop_does_not_stop_task_cancellation() {
        struct PanicOnDrop;

        impl Drop for PanicOnDrop {
            fn drop(&mut self) {
                panic!("future drop panic fixture");
            }
        }

        cancel_all();
        let first = PanicOnDrop;
        spawn(async move {
            let _guard = first;
            next_frame().await;
        });
        let dropped_normally = Rc::new(Cell::new(false));
        struct MarkDrop(Rc<Cell<bool>>);
        impl Drop for MarkDrop {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let second = MarkDrop(dropped_normally.clone());
        spawn(async move {
            let _guard = second;
            next_frame().await;
        });
        poll_frame();
        cancel_all();
        assert!(dropped_normally.get());
    }

    #[test]
    fn sleep_and_timeout_use_monotonic_deadlines() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        let mut immediate_sleep = Box::pin(sleep(Duration::ZERO));
        assert_eq!(immediate_sleep.as_mut().poll(&mut context), Poll::Ready(()));

        let mut ready = Box::pin(timeout(Duration::ZERO, async { 7_u32 }));
        assert_eq!(ready.as_mut().poll(&mut context), Poll::Ready(Ok(7)));

        let mut elapsed = Box::pin(timeout(Duration::ZERO, core::future::pending::<()>()));
        assert_eq!(
            elapsed.as_mut().poll(&mut context),
            Poll::Ready(Err(TimeoutError))
        );
    }

    #[test]
    fn blocking_tasks_return_values_and_isolate_panics() {
        fn wait_for<T>(future: &mut Pin<Box<BlockingTask<T>>>) -> Result<T, BlockingTaskError> {
            let waker = noop_waker();
            let mut context = Context::from_waker(&waker);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                    return output;
                }
                assert!(Instant::now() < deadline, "blocking task timed out");
                std::thread::yield_now();
            }
        }

        let mut value = Box::pin(spawn_blocking(|| 42_u32));
        assert_eq!(wait_for(&mut value), Ok(42));

        let mut panic = Box::pin(spawn_blocking(|| -> u32 {
            panic!("worker panic fixture");
        }));
        assert_eq!(wait_for(&mut panic), Err(BlockingTaskError::Panic));
    }
}
