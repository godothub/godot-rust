use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::ptr;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use godot_rs::abi::{
    ABI_GODOT_METHOD_VARARG, AbiByteSlice, AbiCallResult, AbiGodotMethodSpecV1, AbiHeader,
    AbiPtrcallType, AbiScriptDescriptorV1, AbiStatus, AbiValueType, AbiValueV1,
    HOST_API_SLOT_CALL_GODOT_METHOD, HostApiV1, ModuleApiV1,
};
use godot_rs::prelude::*;

const RECEIVER_ID: u64 = 42;
const ITERATIONS: usize = 200_000;
const CALLS_PER_ITERATION: usize = 5;
const MAX_NANOSECONDS_PER_CALL: u128 = 25_000;

static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HOST_CALLS: AtomicUsize = AtomicUsize::new(0);
static VARIANT_VALUES: AtomicUsize = AtomicUsize::new(0);
static CONTRACT_MISMATCH: AtomicBool = AtomicBool::new(false);

struct CountingAllocator;

struct ExpectedCall {
    arguments: &'static [AbiValueType],
    ptrcall_arguments: &'static [AbiPtrcallType],
    return_type: AbiValueType,
    ptrcall_return: AbiPtrcallType,
    result: AbiValueV1,
}

// SAFETY: Every operation delegates to the process System allocator without
// changing its pointer or layout contract. The counters are observational.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: The caller supplied the GlobalAlloc layout unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: The caller supplied the GlobalAlloc layout unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: The caller supplied the pointer, old layout, and new size
        // under the GlobalAlloc contract.
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The caller supplied the pointer and matching layout under
        // the GlobalAlloc contract.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe extern "C" fn no_script(_index: u32, _output: *mut AbiScriptDescriptorV1) -> AbiStatus {
    AbiStatus::InvalidArgument
}

unsafe extern "C" fn call_godot_method(
    _context: *mut c_void,
    receiver: u64,
    method: *const AbiGodotMethodSpecV1,
    arguments: *const AbiValueV1,
    argument_count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    if receiver != RECEIVER_ID || method.is_null() || output.is_null() {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "performance Host received an invalid call",
        );
    }
    // SAFETY: Null was rejected and generated method specifications have
    // static storage for the complete synchronous call.
    let method = unsafe { &*method };
    let Ok(argument_count) = usize::try_from(argument_count) else {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "performance Host received too many arguments",
        );
    };
    if argument_count > 0 && arguments.is_null() {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "performance Host received null arguments",
        );
    }
    let arguments = if argument_count == 0 {
        &[]
    } else {
        // SAFETY: The caller keeps exactly `argument_count` fixed-layout
        // values live for this synchronous callback.
        unsafe { core::slice::from_raw_parts(arguments, argument_count) }
    };
    let method_arguments = if method.arguments.len == 0 {
        &[]
    } else if method.arguments.ptr.is_null() {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "generated method argument schema is null",
        );
    } else {
        // SAFETY: Generated method schemas have static storage and advertise
        // their exact element count.
        unsafe { core::slice::from_raw_parts(method.arguments.ptr, method.arguments.len) }
    };
    for type_ in arguments
        .iter()
        .map(|value| value.type_)
        .chain(method_arguments.iter().map(|value| value.value_type))
        .chain(core::iter::once(method.return_value.value_type))
    {
        if type_ == AbiValueType::VARIANT {
            VARIANT_VALUES.fetch_add(1, Ordering::Relaxed);
        }
    }
    for type_ in method_arguments
        .iter()
        .map(|value| value.ptrcall_type)
        .chain(core::iter::once(method.return_value.ptrcall_type))
    {
        if type_ == AbiPtrcallType::VARIANT {
            VARIANT_VALUES.fetch_add(1, Ordering::Relaxed);
        }
    }
    if method.reserved_flags & ABI_GODOT_METHOD_VARARG != 0 {
        VARIANT_VALUES.fetch_add(1, Ordering::Relaxed);
    }

    let name = abi_bytes(method.method_name);
    let expected = match name {
        b"is_processing" => ExpectedCall {
            arguments: &[],
            ptrcall_arguments: &[],
            return_type: AbiValueType::BOOL,
            ptrcall_return: AbiPtrcallType::BOOL,
            result: AbiValueV1::from_bool(true),
        },
        b"set_process" => ExpectedCall {
            arguments: &[AbiValueType::BOOL],
            ptrcall_arguments: &[AbiPtrcallType::BOOL],
            return_type: AbiValueType::NIL,
            ptrcall_return: AbiPtrcallType::VOID,
            result: AbiValueV1::NIL,
        },
        b"get_position" => ExpectedCall {
            arguments: &[],
            ptrcall_arguments: &[],
            return_type: AbiValueType::VECTOR2,
            ptrcall_return: AbiPtrcallType::VECTOR2,
            result: AbiValueV1::from_vector2(12.5, -4.0),
        },
        b"set_position" => ExpectedCall {
            arguments: &[AbiValueType::VECTOR2],
            ptrcall_arguments: &[AbiPtrcallType::VECTOR2],
            return_type: AbiValueType::NIL,
            ptrcall_return: AbiPtrcallType::VOID,
            result: AbiValueV1::NIL,
        },
        b"get_canvas_item" => ExpectedCall {
            arguments: &[],
            ptrcall_arguments: &[],
            return_type: AbiValueType::RID,
            ptrcall_return: AbiPtrcallType::RID,
            result: AbiValueV1::from_rid(91),
        },
        _ => {
            CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
            return AbiCallResult::failure(
                AbiStatus::Unsupported,
                "performance Host received an unexpected generated method",
            );
        }
    };
    let valid_contract = !method.class_name.ptr.is_null()
        && method.class_name.len > 0
        && method_arguments.len() == expected.arguments.len()
        && arguments.len() == expected.arguments.len()
        && method_arguments
            .iter()
            .zip(expected.arguments)
            .all(|(actual, expected)| actual.value_type == *expected)
        && method_arguments
            .iter()
            .zip(expected.ptrcall_arguments)
            .all(|(actual, expected)| actual.ptrcall_type == *expected)
        && arguments
            .iter()
            .zip(expected.arguments)
            .all(|(actual, expected)| actual.type_ == *expected)
        && method.return_value.value_type == expected.return_type
        && method.return_value.ptrcall_type == expected.ptrcall_return;
    if !valid_contract {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "generated method used the wrong value contract",
        );
    }

    HOST_CALLS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Null was rejected and the caller keeps the output slot writable
    // for this synchronous callback.
    unsafe { output.write(expected.result) };
    AbiCallResult::OK
}

fn abi_bytes(value: AbiByteSlice) -> &'static [u8] {
    if value.len == 0 {
        return &[];
    }
    if value.ptr.is_null() {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return &[];
    }
    // SAFETY: Generated ABI text has static storage and an authenticated
    // bounded length.
    unsafe { core::slice::from_raw_parts(value.ptr, value.len) }
}

fn exercise_hot_paths(node: ObjectRef<Node2D>) {
    black_box(node.is_processing().expect("bool getter"));
    node.set_process(black_box(true)).expect("bool setter");
    let position = node.get_position().expect("Vector2 getter");
    assert_eq!(position, Vector2::new(12.5, -4.0));
    node.set_position(black_box(Vector2::new(8.0, 16.0)))
        .expect("Vector2 setter");
    assert_eq!(node.get_canvas_item().expect("RID getter").id(), 91);
}

fn main() {
    let mut reserved = [0; 16];
    reserved[HOST_API_SLOT_CALL_GODOT_METHOD] = call_godot_method as *const () as usize;
    let host = HostApiV1 {
        header: AbiHeader::new(HostApiV1::MINIMUM_SIZE),
        context: ptr::null_mut(),
        log: None,
        reserved,
    };
    let mut module = MaybeUninit::<ModuleApiV1>::uninit();
    // SAFETY: Both ABI tables remain live until shutdown, and `no_script`
    // matches the advertised zero script count.
    let status = unsafe {
        godot_rs::module::initialize(
            ptr::from_ref(&host),
            module.as_mut_ptr(),
            0,
            Some(no_script),
            None,
        )
    };
    assert_eq!(status, AbiStatus::Ok);

    let node = ObjectRef::<Node2D>::__from_instance_id(RECEIVER_ID);
    for _ in 0..10_000 {
        exercise_hot_paths(node);
    }
    HOST_CALLS.store(0, Ordering::Relaxed);
    VARIANT_VALUES.store(0, Ordering::Relaxed);
    CONTRACT_MISMATCH.store(false, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);

    COUNT_ALLOCATIONS.store(true, Ordering::Release);
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        exercise_hot_paths(node);
    }
    let elapsed = started.elapsed();
    COUNT_ALLOCATIONS.store(false, Ordering::Release);

    let calls = HOST_CALLS.load(Ordering::Relaxed);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let variant_values = VARIANT_VALUES.load(Ordering::Relaxed);
    let nanoseconds_per_call = elapsed.as_nanos() / calls as u128;
    println!(
        "generated_engine_api godot_api={} calls={calls} elapsed_ms={} ns_per_call={nanoseconds_per_call} allocations={allocations} variant_values={variant_values}",
        godot_rs::GODOT_API,
        elapsed.as_millis()
    );

    assert_eq!(calls, ITERATIONS * CALLS_PER_ITERATION);
    assert!(!CONTRACT_MISMATCH.load(Ordering::Relaxed));
    assert_eq!(allocations, 0, "generated hot paths allocated");
    assert_eq!(
        variant_values, 0,
        "typed generated methods degraded to Variant transport"
    );
    assert!(
        nanoseconds_per_call <= MAX_NANOSECONDS_PER_CALL,
        "generated calls exceeded {MAX_NANOSECONDS_PER_CALL} ns per call"
    );

    // SAFETY: The initialized module still points to the live Host table and
    // no generated call is active.
    let shutdown_status = unsafe { godot_rs::module::shutdown(ptr::null_mut()) };
    assert_eq!(shutdown_status, AbiStatus::Ok);
}
