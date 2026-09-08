godot_rs::script_module! {}

#[test]
fn module_shutdown_revokes_host_logs_and_waits_for_active_log_calls() {
    use core::ffi::c_void;
    use core::mem::MaybeUninit;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use godot_rs::abi::{
        AbiByteSlice, AbiCancelTasksFn, AbiHeader, AbiLogLevel, AbiStatus, HostApiV1,
        MODULE_API_SLOT_CANCEL_TASKS, ModuleApiV1,
    };
    use std::sync::{Mutex, mpsc};
    use std::time::{Duration, Instant};

    static LOGS: AtomicUsize = AtomicUsize::new(0);
    static LOG_GATE: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>> = Mutex::new(None);

    unsafe extern "C" fn log(
        _context: *mut c_void,
        _level: AbiLogLevel,
        _target: AbiByteSlice,
        _message: AbiByteSlice,
    ) {
        LOGS.fetch_add(1, Ordering::Relaxed);
        let gate = LOG_GATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            let _ = release.recv();
        }
    }

    fn initialize(host: &HostApiV1) -> ModuleApiV1 {
        let mut module = MaybeUninit::<ModuleApiV1>::uninit();
        // SAFETY: The caller retains its Host table until shutdown completes.
        let status = unsafe { godot_rs_module_entry(host, module.as_mut_ptr()) };
        assert_eq!(status, AbiStatus::Ok);
        // SAFETY: A successful entry initialized the complete output table.
        unsafe { module.assume_init() }
    }

    fn finish_workers(shutdown: unsafe extern "C" fn(*mut c_void) -> AbiStatus) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // SAFETY: This fixture has a null context and permits repeated
            // shutdown while its detached worker finishes.
            let status = unsafe { shutdown(core::ptr::null_mut()) };
            if status == AbiStatus::Ok {
                break;
            }
            assert_eq!(status, AbiStatus::Unsupported);
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::yield_now();
        }
    }

    let host = HostApiV1 {
        header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
        context: core::ptr::null_mut(),
        log: Some(log),
        reserved: [0; 16],
    };
    let module = initialize(&host);
    assert_eq!(module.script_count, 0);
    let shutdown = module.shutdown.expect("module shutdown callback");
    // SAFETY: The generated SDK stores AbiCancelTasksFn in this ABI slot.
    let cancel = unsafe {
        core::mem::transmute::<usize, AbiCancelTasksFn>(
            module.reserved[MODULE_API_SLOT_CANCEL_TASKS],
        )
    }
    .expect("module task cancellation callback");

    let (release, wait) = mpsc::sync_channel(0);
    let (logged, after_log) = mpsc::sync_channel(0);
    let worker = godot_rs::task::spawn_blocking(move || {
        wait.recv().unwrap();
        godot_rs::godot_print!("a worker log after Host shutdown uses standard error");
        logged.send(()).unwrap();
    });
    drop(worker);
    struct LogOnDrop;

    impl Drop for LogOnDrop {
        fn drop(&mut self) {
            godot_rs::godot_print!("task cancellation may still log through the Host");
        }
    }

    let drop_log = LogOnDrop;
    let task = godot_rs::task::spawn(async move {
        let _drop_log = drop_log;
        core::future::pending::<()>().await;
    });
    // SAFETY: The callback belongs to this live module generation.
    let status = unsafe { cancel() };
    assert_eq!(status, AbiStatus::Unsupported);
    assert!(
        !task.is_cancelled(),
        "a deferred reload must preserve tasks"
    );
    godot_rs::godot_print!("deferred hot reload retains its active Host");
    assert_eq!(LOGS.load(Ordering::Relaxed), 1);

    // SAFETY: The Host table remains live until shutdown revokes callbacks;
    // Unsupported only requests retaining the project library for its worker.
    let status = unsafe { shutdown(module.context) };
    assert_eq!(status, AbiStatus::Unsupported);
    assert!(
        task.is_cancelled(),
        "engine shutdown must release task state"
    );
    assert_eq!(LOGS.load(Ordering::Relaxed), 2, "future destructor logged");
    godot_rs::godot_print!("the module has revoked its Host callback table");
    assert_eq!(LOGS.load(Ordering::Relaxed), 2);

    release.send(()).unwrap();
    after_log.recv_timeout(Duration::from_secs(2)).unwrap();
    finish_workers(shutdown);
    assert_eq!(
        LOGS.load(Ordering::Relaxed),
        2,
        "worker bypassed retired Host"
    );

    // A second lifecycle verifies an already-running Host log callback must
    // finish before shutdown can return and its callback code can be unloaded.
    let _module = initialize(&host);
    let (entered, inside_log) = mpsc::sync_channel(0);
    let (release_log, wait_for_release) = mpsc::sync_channel(0);
    *LOG_GATE.lock().unwrap() = Some((entered, wait_for_release));
    drop(godot_rs::task::spawn_blocking(|| {
        godot_rs::godot_print!("a Host log callback remains active");
    }));
    inside_log.recv_timeout(Duration::from_secs(2)).unwrap();
    let (closing, started_shutdown) = mpsc::sync_channel(0);
    let (closed, finished_shutdown) = mpsc::sync_channel(0);
    let closing_thread = std::thread::spawn(move || {
        closing.send(()).unwrap();
        // SAFETY: This isolated fixture has no engine values or pending
        // cooperative tasks, and its null context is valid on this test thread.
        let status = unsafe { shutdown(core::ptr::null_mut()) };
        closed.send(status).unwrap();
    });
    started_shutdown
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let early = finished_shutdown.recv_timeout(Duration::from_millis(100));
    release_log.send(()).unwrap();
    let status = match early {
        Ok(status) => status,
        Err(_) => finished_shutdown
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
    };
    closing_thread.join().unwrap();
    assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)));
    assert_eq!(status, AbiStatus::Unsupported);
    finish_workers(shutdown);
    let retired_logs = LOGS.load(Ordering::Relaxed);
    godot_rs::godot_print!("the completed shutdown has no Host log callback");
    assert_eq!(LOGS.load(Ordering::Relaxed), retired_logs);

    // Formatting must run outside the log read lock: Display is user code
    // and can itself initiate the shutdown that needs the write lock.
    struct ShutdownOnDisplay(unsafe extern "C" fn(*mut c_void) -> AbiStatus);

    impl core::fmt::Display for ShutdownOnDisplay {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            // SAFETY: The fixture has no active workers, engine values or
            // tasks, and its Host table remains live during this formatting.
            assert_eq!(unsafe { self.0(core::ptr::null_mut()) }, AbiStatus::Ok);
            formatter.write_str("formatting completed after shutdown")
        }
    }

    let _module = initialize(&host);
    godot_rs::godot_print!("{}", ShutdownOnDisplay(shutdown));
    assert_eq!(LOGS.load(Ordering::Relaxed), retired_logs);
}
