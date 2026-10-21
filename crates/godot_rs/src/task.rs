//! Main-thread cooperative tasks for Script Mode.
//!
//! Ready tasks are polled at most once per Godot frame by the Script Host. They never move
//! to a worker thread, so generated engine handles remain subject to the same
//! main-thread rules as ordinary script callbacks.

use core::cell::{Cell, RefCell};
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::task::Wake;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{EngineError, EngineResult};
use crate::signal::{Signal, SignalResult};

thread_local! {
    static TASKS: RefCell<HashMap<u64, Task>> = RefCell::new(HashMap::new());
    static READY: Arc<Mutex<VecDeque<u64>>> = Arc::new(Mutex::new(VecDeque::new()));
    static TIMERS: RefCell<BinaryHeap<TimerWake>> = const { RefCell::new(BinaryHeap::new()) };
    static NEXT_TASK_ID: Cell<u64> = const { Cell::new(1) };
    static CANCELLING_TASKS: Cell<bool> = const { Cell::new(false) };
}

static BLOCKING_WORKERS: Mutex<BlockingPool> = Mutex::new(BlockingPool::new());

struct Task {
    id: u64,
    cancelled: Rc<AtomicBool>,
    scope_cancelled: Option<Rc<ScopeState>>,
    future: Option<Pin<Box<dyn Future<Output = ()>>>>,
    wake: Arc<TaskWake>,
}

/// A cancellation handle for one cooperative Rust task.
#[derive(Clone)]
pub struct TaskHandle {
    wake: Arc<TaskWake>,
    id: u64,
    cancelled: Rc<AtomicBool>,
    scope_cancelled: Option<Rc<ScopeState>>,
}

impl TaskHandle {
    /// Requests cancellation. The future is dropped at the next Host frame.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.wake_by_ref();
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .scope_cancelled
                .as_ref()
                .is_some_and(|scope| scope.cancelled.load(Ordering::Acquire))
    }

    /// Stable identity within this loaded project-module generation.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }
}

/// Owns cooperative tasks that must stop when their script instance is dropped.
///
/// A task never borrows the script across an await point. Move data or checked
/// object handles into it; the scope prevents any later poll after its owner
/// has been destroyed. Cancelling does not stop an already-running worker.
#[derive(Debug, Default)]
pub struct TaskScope {
    cancelled: Rc<ScopeState>,
}

#[derive(Debug, Default)]
struct ScopeState {
    cancelled: AtomicBool,
    tasks: RefCell<Vec<Weak<TaskWake>>>,
    compact_at: Cell<usize>,
    #[cfg(test)]
    compaction_visits: Cell<usize>,
}

#[derive(Debug)]
struct TaskWake {
    id: u64,
    queued: AtomicBool,
    alive: AtomicBool,
    ready: Arc<Mutex<VecDeque<u64>>>,
}
impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if self.alive.load(Ordering::Acquire) && !self.queued.swap(true, Ordering::AcqRel) {
            self.ready
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(self.id);
        }
    }
}

struct TimerRegistration {
    wake: Waker,
}
struct TimerWake {
    deadline: Instant,
    registration: Weak<TimerRegistration>,
}
impl PartialEq for TimerWake {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}
impl Eq for TimerWake {}
impl PartialOrd for TimerWake {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerWake {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other.deadline.cmp(&self.deadline)
    }
}

impl TaskScope {
    /// Creates an independent task lifetime.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedules a main-thread future belonging to this lifetime.
    pub fn spawn(&self, future: impl Future<Output = ()> + 'static) -> TaskHandle {
        spawn_in_scope(future, Some(self.cancelled.clone()))
    }

    /// Permanently cancels this scope and every task spawned through it.
    pub fn cancel(&self) {
        self.cancelled.cancelled.store(true, Ordering::Release);
        for task in self
            .cancelled
            .tasks
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
        {
            task.wake_by_ref();
        }
    }
}

impl Drop for TaskScope {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Schedules a `!Send` future on the Godot main thread.
///
/// Newly spawned tasks are first polled on the next engine frame. Panicking
/// tasks are isolated and removed without unwinding across the module ABI.
pub fn spawn(future: impl Future<Output = ()> + 'static) -> TaskHandle {
    spawn_in_scope(future, None)
}

fn spawn_in_scope(
    future: impl Future<Output = ()> + 'static,
    scope_cancelled: Option<Rc<ScopeState>>,
) -> TaskHandle {
    let id = NEXT_TASK_ID.with(|next| {
        let id = next.get();
        next.set(id.checked_add(1).unwrap_or(1));
        id
    });
    let wake = Arc::new(TaskWake {
        id,
        queued: AtomicBool::new(false),
        alive: AtomicBool::new(true),
        ready: READY.with(Arc::clone),
    });
    if let Some(scope) = &scope_cancelled {
        let mut tasks = scope.tasks.borrow_mut();
        if tasks.len() >= scope.compact_at.get() {
            #[cfg(test)]
            scope
                .compaction_visits
                .set(scope.compaction_visits.get() + tasks.len());
            // Double the live population between scans. A scope with many
            // concurrent tasks must not rescan every registration per spawn.
            // Completed tasks may still have handles, so weak ownership alone
            // does not identify registrations that need cancellation wakes.
            tasks.retain(|task| {
                task.upgrade()
                    .is_some_and(|task| task.alive.load(Ordering::Acquire))
            });
            scope.compact_at.set(tasks.len().saturating_mul(2).max(128));
        }
        tasks.push(Arc::downgrade(&wake));
    }
    let cancelled = Rc::new(AtomicBool::new(false));
    let mut task = Task {
        id,
        cancelled: cancelled.clone(),
        scope_cancelled: scope_cancelled.clone(),
        future: Some(Box::pin(future)),
        wake: wake.clone(),
    };
    if CANCELLING_TASKS.get()
        || scope_cancelled
            .as_ref()
            .is_some_and(|scope| scope.cancelled.load(Ordering::Acquire))
    {
        cancelled.store(true, Ordering::Release);
        drop_task_future(&mut task);
    } else {
        TASKS.with(|tasks| tasks.borrow_mut().insert(id, task));
        wake.wake_by_ref();
    }
    TaskHandle {
        wake,
        id,
        cancelled,
        scope_cancelled,
    }
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
        registered: None,
    }
}

/// Future returned by [`sleep`].
pub struct Sleep {
    deadline: Option<Instant>,
    registered: Option<Arc<TimerRegistration>>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match self.deadline {
            Some(deadline) if Instant::now() < deadline => {
                if self
                    .registered
                    .as_ref()
                    .is_none_or(|wake| !wake.wake.will_wake(context.waker()))
                {
                    let registration = Arc::new(TimerRegistration {
                        wake: context.waker().clone(),
                    });
                    TIMERS.with(|timers| {
                        timers.borrow_mut().push(TimerWake {
                            deadline,
                            registration: Arc::downgrade(&registration),
                        })
                    });
                    self.registered = Some(registration);
                }
                Poll::Pending
            }
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
        timer: sleep(duration),
    }
}

/// Future returned by [`timeout`].
pub struct Timeout<F: Future> {
    future: Pin<Box<F>>,
    timer: Sleep,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, TimeoutError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Poll::Ready(output) = this.future.as_mut().poll(context) {
            return Poll::Ready(Ok(output));
        }
        match Pin::new(&mut this.timer).poll(context) {
            Poll::Ready(()) => Poll::Ready(Err(TimeoutError)),
            Poll::Pending => Poll::Pending,
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
    /// The bounded worker queue is full.
    QueueFull,
}

impl core::fmt::Display for BlockingTaskError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Spawn(message) => write!(formatter, "could not start Rust worker: {message}"),
            Self::Panic => formatter.write_str("Rust worker panicked"),
            Self::Disconnected => formatter.write_str("Rust worker exited without a result"),
            Self::QueueFull => formatter.write_str("Rust worker queue is full"),
        }
    }
}

impl std::error::Error for BlockingTaskError {}

/// Runs CPU-bound or blocking Rust work away from the Godot main thread.
/// Uses at most eight reusable workers and a 1024-job queue; a full queue
/// returns [`BlockingTaskError::QueueFull`] instead of creating more threads.
///
/// Engine objects and generated Godot APIs must stay on the main thread and
/// therefore must not be moved into this closure. Dropping the returned
/// future detaches an already-running worker; Rust threads cannot be forcibly
/// cancelled safely. Hot reload is deferred until these workers finish. At
/// engine shutdown, a module with running workers remains loaded until the
/// process exits, but its Host callbacks are revoked. Subsequent worker logs
/// use standard error instead of calling the retired Host.
#[must_use]
pub fn spawn_blocking<T, F>(work: F) -> BlockingTask<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    spawn_blocking_with(&BLOCKING_WORKERS, work)
}

type BlockingJob = Box<dyn FnOnce() + Send>;
struct BlockingPool {
    sender: Option<mpsc::SyncSender<BlockingJob>>,
    workers: Vec<JoinHandle<()>>,
    pending: Option<Arc<core::sync::atomic::AtomicUsize>>,
}
impl BlockingPool {
    const fn new() -> Self {
        Self {
            sender: None,
            workers: Vec::new(),
            pending: None,
        }
    }
    fn start(&mut self) -> Result<(), BlockingTaskError> {
        if self.sender.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::sync_channel::<BlockingJob>(1024);
        let receiver = Arc::new(Mutex::new(receiver));
        let pending = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let count = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .clamp(1, 8);
        for _ in 0..count {
            let receiver = receiver.clone();
            let pending = pending.clone();
            let worker = std::thread::Builder::new()
                .name("godot-rust-worker".to_owned())
                .spawn(move || {
                    loop {
                        let job = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(job) = job else {
                            break;
                        };
                        // Jobs isolate closure panics and always release their count.
                        let _ = catch_unwind(AssertUnwindSafe(job));
                        pending.fetch_sub(1, Ordering::Release);
                    }
                });
            match worker {
                Ok(worker) => self.workers.push(worker),
                Err(error) => {
                    drop(sender);
                    for worker in self.workers.drain(..) {
                        let _ = worker.join();
                    }
                    return Err(BlockingTaskError::Spawn(error.to_string()));
                }
            }
        }
        self.pending = Some(pending);
        self.sender = Some(sender);
        Ok(())
    }
}

fn spawn_blocking_with<T, F>(registry: &Mutex<BlockingPool>, work: F) -> BlockingTask<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let wake = Arc::new(Mutex::new(None::<Waker>));
    let completed = wake.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut pool = registry.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = pool.start() {
        drop(pool);
        return BlockingTask {
            receiver: Err(error),
            wake,
        };
    }
    let pending = pool
        .pending
        .as_ref()
        .expect("started pool has a counter")
        .clone();
    pending.fetch_add(1, Ordering::Relaxed);
    let job = Box::new(move || {
        let result = catch_unwind(AssertUnwindSafe(work)).map_err(|_| BlockingTaskError::Panic);
        let _ = sender.send(result);
        let waker = completed.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(waker) = waker {
            waker.wake();
        }
    });
    let submitted = pool
        .sender
        .as_ref()
        .expect("started pool has a sender")
        .try_send(job);
    // Rejecting a queued closure drops user-owned captures; do that after
    // releasing the registry lock so their destructors can submit other work.
    drop(pool);
    let receiver = match submitted {
        Ok(()) => Ok(receiver),
        Err(error) => {
            pending.fetch_sub(1, Ordering::Release);
            Err(match error {
                mpsc::TrySendError::Full(_) => BlockingTaskError::QueueFull,
                mpsc::TrySendError::Disconnected(_) => BlockingTaskError::Disconnected,
            })
        }
    };
    BlockingTask { receiver, wake }
}

pub(crate) fn blocking_workers_finished() -> bool {
    blocking_workers_finished_in(&BLOCKING_WORKERS)
}

fn blocking_workers_finished_in(registry: &Mutex<BlockingPool>) -> bool {
    let workers = {
        let mut pool = registry.lock().unwrap_or_else(|e| e.into_inner());
        if pool
            .pending
            .as_ref()
            .is_some_and(|pending| pending.load(Ordering::Acquire) != 0)
        {
            return false;
        }
        // Idle workers also execute module code: close and join them before
        // telling the Host that this generation can safely be unloaded.
        pool.sender.take();
        pool.pending.take();
        core::mem::take(&mut pool.workers)
    };
    for worker in workers {
        let _ = worker.join();
    }
    true
}

/// Future returned by [`spawn_blocking`].
pub struct BlockingTask<T> {
    wake: Arc<Mutex<Option<Waker>>>,
    receiver: Result<Receiver<Result<T, BlockingTaskError>>, BlockingTaskError>,
}

impl<T> Unpin for BlockingTask<T> {}

impl<T> Future for BlockingTask<T> {
    type Output = Result<T, BlockingTaskError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        *self.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(context.waker().clone());
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
pub struct SignalFuture<Args = ()> {
    state: SignalFutureState,
    typed: bool,
    marker: core::marker::PhantomData<fn() -> Args>,
}

impl<Args> SignalFuture<Args> {
    pub(crate) fn new<T>(signal: &Signal<T>) -> Self {
        let encoded = signal
            .__bytes()
            .map(|bytes| bytes.to_vec().into_boxed_slice())
            .map_err(|error| EngineError::invalid_argument(error.message()));
        Self {
            state: SignalFutureState::New(encoded),
            typed: true,
            marker: core::marker::PhantomData,
        }
    }
}

impl SignalFuture {
    pub(crate) fn new_untyped<T>(signal: &Signal<T>) -> Self {
        let mut future = Self::new(signal);
        future.typed = false;
        future
    }
}

impl<Args: SignalResult> Future for SignalFuture<Args> {
    type Output = EngineResult<Args>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
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
            let value = godot_api::abi::AbiValueV1::from_borrowed_bytes(
                godot_api::abi::AbiValueType::SIGNAL,
                &bytes,
            );
            match crate::module::watch_signal(value) {
                Ok(token) => {
                    self.state = SignalFutureState::Watching(token);
                    context.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Err(error) => return Poll::Ready(Err(error)),
            }
        }

        let SignalFutureState::Watching(token) = self.state else {
            panic!("SignalFuture polled after completion");
        };
        let result = if self.typed {
            crate::module::poll_signal_values(token)
        } else {
            crate::module::poll_signal(token).map(|fired| fired.then(Vec::new))
        };
        match result {
            Ok(None) => {
                context.waker().wake_by_ref();
                Poll::Pending
            }
            Ok(Some(arguments)) => {
                self.state = SignalFutureState::Done;
                Poll::Ready(Args::from_signal_arguments(arguments))
            }
            Err(error) => {
                crate::module::cancel_signal(token);
                self.state = SignalFutureState::Done;
                Poll::Ready(Err(error))
            }
        }
    }
}

impl<Args> Drop for SignalFuture<Args> {
    fn drop(&mut self) {
        if let SignalFutureState::Watching(token) = self.state {
            crate::module::cancel_signal(token);
        }
    }
}

#[doc(hidden)]
pub fn poll_frame() {
    let now = Instant::now();
    let active_count = TASKS.with(|tasks| tasks.borrow().len());
    TIMERS.with(|timers| {
        let mut timers = timers.borrow_mut();
        // Cancelled/completed timers retain weak entries only. Compact them
        // amortized, so long timeouts cannot accumulate dead registrations.
        if timers.len() > active_count.saturating_mul(2).saturating_add(64) {
            timers.retain(|timer| timer.registration.strong_count() != 0);
        }
    });
    loop {
        let timer = TIMERS.with(|timers| {
            let mut timers = timers.borrow_mut();
            if timers.peek().is_some_and(|timer| {
                timer.deadline <= now || timer.registration.strong_count() == 0
            }) {
                timers.pop()
            } else {
                None
            }
        });
        let Some(timer) = timer else {
            break;
        };
        if let Some(registration) = timer.registration.upgrade() {
            registration.wake.wake_by_ref();
        }
    }
    let ready = READY.with(Arc::clone);
    let count = ready.lock().unwrap_or_else(|e| e.into_inner()).len();
    // Snapshot the frame's work count. Self-wakes and tasks spawned while
    // polling belong to the next frame, preserving next_frame semantics.
    for _ in 0..count {
        let id = ready.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        let Some(id) = id else {
            break;
        };
        let Some(mut task) = TASKS.with(|tasks| tasks.borrow_mut().remove(&id)) else {
            continue;
        };
        task.wake.queued.store(false, Ordering::Release);
        if task.cancelled.load(Ordering::Acquire)
            || task
                .scope_cancelled
                .as_ref()
                .is_some_and(|scope| scope.cancelled.load(Ordering::Acquire))
        {
            drop_task_future(&mut task);
            continue;
        }
        let waker = Waker::from(task.wake.clone());
        let mut context = Context::from_waker(&waker);
        let poll = catch_unwind(AssertUnwindSafe(|| {
            task.future
                .as_mut()
                .expect("active task retains its future")
                .as_mut()
                .poll(&mut context)
        }));
        match poll {
            Ok(Poll::Pending) => {
                TASKS.with(|tasks| tasks.borrow_mut().insert(id, task));
            }
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
    struct CancellationScope(bool);

    impl Drop for CancellationScope {
        fn drop(&mut self) {
            CANCELLING_TASKS.set(self.0);
        }
    }

    let _scope = CancellationScope(CANCELLING_TASKS.replace(true));
    let tasks = TASKS.with(|tasks| core::mem::take(&mut *tasks.borrow_mut()));
    for (_, mut task) in tasks {
        task.cancelled.store(true, Ordering::Release);
        drop_task_future(&mut task);
    }
    READY.with(|ready| ready.lock().unwrap_or_else(|e| e.into_inner()).clear());
    TIMERS.with(|timers| timers.borrow_mut().clear());
}

fn drop_task_future(task: &mut Task) {
    task.wake.alive.store(false, Ordering::Release);
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

#[cfg(test)]
fn noop_waker() -> Waker {
    Waker::noop().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    #[test]
    fn dormant_tasks_wait_for_wake_and_cancellation_wakes_them() {
        cancel_all();
        let polls = Rc::new(Cell::new(0));
        let scope = TaskScope::new();
        for _ in 0..1000 {
            let polls = polls.clone();
            scope.spawn(std::future::poll_fn(move |_| {
                polls.set(polls.get() + 1);
                Poll::<()>::Pending
            }));
        }
        poll_frame();
        assert_eq!(polls.get(), 1000);
        for _ in 0..100 {
            poll_frame();
        }
        assert_eq!(
            polls.get(),
            1000,
            "dormant tasks must not be polled every frame"
        );
        scope.cancel();
        poll_frame();
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));
    }

    #[test]
    fn registering_live_scoped_tasks_has_linear_compaction_work() {
        cancel_all();
        const TASK_COUNT: usize = 4096;
        let scope = TaskScope::new();
        for _ in 0..TASK_COUNT {
            scope.spawn(std::future::pending());
        }
        assert_eq!(scope.cancelled.tasks.borrow().len(), TASK_COUNT);
        assert!(
            scope.cancelled.compaction_visits.get() < TASK_COUNT * 2,
            "scope registration scans must grow linearly with concurrent tasks"
        );
        poll_frame();
        scope.cancel();
        poll_frame();
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));
    }

    #[test]
    fn completed_scoped_tasks_do_not_accumulate_with_retained_handles() {
        cancel_all();
        const TASK_COUNT: usize = 4096;
        let scope = TaskScope::new();
        let mut handles = Vec::with_capacity(TASK_COUNT);
        for _ in 0..TASK_COUNT {
            handles.push(scope.spawn(async {}));
            poll_frame();
        }
        assert!(
            scope.cancelled.tasks.borrow().len() <= 128,
            "completed tasks must be pruned even while their handles remain alive"
        );
        assert!(scope.cancelled.compaction_visits.get() <= TASK_COUNT * 2);
        scope.cancel();
        assert!(handles.iter().all(TaskHandle::is_cancelled));
        poll_frame();
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));
    }

    #[test]
    fn a_worker_wake_reschedules_a_main_thread_future() {
        cancel_all();
        let wake = Arc::new(Mutex::new(None::<Waker>));
        let captured = wake.clone();
        let polls = Rc::new(Cell::new(0));
        let observed = polls.clone();
        spawn(std::future::poll_fn(move |context| {
            observed.set(observed.get() + 1);
            *captured.lock().unwrap() = Some(context.waker().clone());
            if observed.get() == 2 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));
        poll_frame();
        poll_frame();
        assert_eq!(polls.get(), 1);
        std::thread::spawn(move || wake.lock().unwrap().take().unwrap().wake())
            .join()
            .unwrap();
        poll_frame();
        assert_eq!(polls.get(), 2);
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));
    }

    #[test]
    fn cancelled_timers_release_wakers_and_completed_timeouts_do_not_accumulate() {
        cancel_all();
        for _ in 0..256 {
            spawn(async {
                let _ = timeout(Duration::from_secs(86_400), next_frame()).await;
            });
        }
        poll_frame();
        poll_frame();
        poll_frame();
        TIMERS.with(|timers| assert!(timers.borrow().is_empty()));
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));
    }

    #[test]
    fn timers_resume_sleeping_tasks_and_time_out_dormant_futures() {
        cancel_all();
        let completed = Rc::new(Cell::new(0));
        let observed = completed.clone();
        spawn(async move {
            sleep(Duration::from_millis(1)).await;
            assert_eq!(
                timeout(Duration::from_millis(1), std::future::pending::<()>()).await,
                Err(TimeoutError)
            );
            observed.set(1);
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while completed.get() == 0 {
            assert!(Instant::now() < deadline);
            poll_frame();
            std::thread::yield_now();
        }
        cancel_all();
    }

    #[test]
    fn dropping_scope_cancels_only_its_tasks_before_another_poll() {
        cancel_all();
        let scope = TaskScope::new();
        let scoped_runs = Rc::new(Cell::new(0));
        let output = scoped_runs.clone();
        let handle = scope.spawn(async move {
            output.set(1);
            next_frame().await;
            output.set(2);
        });
        let unscoped_ran = Rc::new(Cell::new(false));
        let output = unscoped_ran.clone();
        spawn(async move {
            next_frame().await;
            output.set(true);
        });
        poll_frame();
        assert_eq!(scoped_runs.get(), 1);
        drop(scope);
        assert!(handle.is_cancelled());
        poll_frame();
        assert_eq!(scoped_runs.get(), 1);
        assert!(unscoped_ran.get());
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));
    }

    #[test]
    fn cancelled_scope_drops_new_futures_without_polling_them() {
        struct MarkDrop(Rc<Cell<bool>>);
        impl Drop for MarkDrop {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let guard = MarkDrop(dropped.clone());
        let scope = TaskScope::new();
        scope.cancel();
        let handle = scope.spawn(async move {
            let _guard = guard;
            panic!("cancelled scope must not poll futures");
        });
        assert!(handle.is_cancelled());
        assert!(dropped.get());
    }

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
    fn cancellation_also_drops_tasks_spawned_by_future_destructors() {
        struct SpawnOnDrop(Rc<Cell<bool>>);

        impl Drop for SpawnOnDrop {
            fn drop(&mut self) {
                struct MarkDrop(Rc<Cell<bool>>);

                impl Drop for MarkDrop {
                    fn drop(&mut self) {
                        self.0.set(true);
                    }
                }

                let mark = MarkDrop(self.0.clone());
                let nested = spawn(async move {
                    let _mark = mark;
                    core::future::pending::<()>().await;
                });
                assert!(nested.is_cancelled());
            }
        }

        cancel_all();
        let dropped = Rc::new(Cell::new(false));
        let guard = SpawnOnDrop(dropped.clone());
        let handle = spawn(async move {
            let _guard = guard;
            core::future::pending::<()>().await;
        });
        cancel_all();
        assert!(handle.is_cancelled());
        assert!(dropped.get());
        TASKS.with(|tasks| assert!(tasks.borrow().is_empty()));

        let completed = Rc::new(Cell::new(false));
        let output = completed.clone();
        spawn(async move { output.set(true) });
        poll_frame();
        assert!(completed.get());
    }

    #[test]
    fn blocking_work_reuses_a_bounded_pool() {
        let registry = Mutex::new(BlockingPool::new());
        let gate = Arc::new(AtomicBool::new(false));
        let mut tasks = Vec::new();
        for value in 0..64 {
            let gate = gate.clone();
            tasks.push(Box::pin(spawn_blocking_with(&registry, move || {
                while !gate.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                value
            })));
        }
        assert!(registry.lock().unwrap().workers.len() <= 8);
        gate.store(true, Ordering::Release);
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        let deadline = Instant::now() + Duration::from_secs(2);
        for (index, mut task) in tasks.into_iter().enumerate() {
            loop {
                if let Poll::Ready(value) = task.as_mut().poll(&mut context) {
                    assert_eq!(value, Ok(index));
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        while !blocking_workers_finished_in(&registry) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn detached_blocking_workers_prevent_unloading_until_they_finish() {
        let registry = Mutex::new(BlockingPool::new());
        let (release, wait) = mpsc::sync_channel(0);
        let task = spawn_blocking_with(&registry, move || wait.recv().unwrap());
        drop(task);
        assert!(!blocking_workers_finished_in(&registry));
        release.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while !blocking_workers_finished_in(&registry) {
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::yield_now();
        }
        assert!(registry.lock().unwrap().workers.is_empty());
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

        let registry = Mutex::new(BlockingPool::new());
        let mut value = Box::pin(spawn_blocking_with(&registry, || 42_u32));
        assert_eq!(wait_for(&mut value), Ok(42));

        let mut panic = Box::pin(spawn_blocking_with(&registry, || -> u32 {
            panic!("worker panic fixture");
        }));
        assert_eq!(wait_for(&mut panic), Err(BlockingTaskError::Panic));

        // Receiving a result can precede thread exit. Join both workers before
        // the test ends, using an isolated registry so parallel tests cannot
        // reap these handles or mistake these workers for live module tasks.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !blocking_workers_finished_in(&registry) {
            assert!(Instant::now() < deadline, "workers did not finish");
            std::thread::yield_now();
        }
        assert!(registry.lock().unwrap().workers.is_empty());
    }
}
