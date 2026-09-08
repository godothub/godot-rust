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
    HOST_API_SLOT_CALL_GODOT_METHOD, HOST_API_SLOT_CALL_SCRIPT_MEMBER, HostApiV1, ModuleApiV1,
};
use godot_rs::prelude::*;

const RECEIVER_ID: u64 = 42;
const ITERATIONS: usize = 200_000;
const CALLS_PER_ITERATION: usize = 10;
const MAX_NANOSECONDS_PER_CALL: u128 = 25_000;

static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HOST_CALLS: AtomicUsize = AtomicUsize::new(0);
static VARIANT_VALUES: AtomicUsize = AtomicUsize::new(0);
static CONTRACT_MISMATCH: AtomicBool = AtomicBool::new(false);

#[script(base = Node)]
struct Health {
    #[export(default = 100)]
    points: i64,
}

#[script]
impl Health {
    #[func]
    fn damage(&mut self, amount: i64) -> i64 {
        self.points -= amount;
        self.points
    }
}

unsafe extern "C" fn call_script_member(
    _context: *mut c_void,
    receiver: u64,
    uid: i64,
    member: u64,
    operation: u8,
    arguments: *const AbiValueV1,
    count: u32,
    output: *mut AbiValueV1,
) -> AbiCallResult {
    use godot_rs::abi::{
        ABI_SCRIPT_MEMBER_GET_PROPERTY, ABI_SCRIPT_MEMBER_METHOD, ABI_SCRIPT_MEMBER_SET_PROPERTY,
    };
    if receiver != RECEIVER_ID
        || uid != Health::__script_resource_uid().unwrap().unwrap()
        || output.is_null()
        || (count != 0 && arguments.is_null())
    {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(AbiStatus::InvalidArgument, "invalid typed script receiver");
    }
    let result = match (operation, member, count) {
        (ABI_SCRIPT_MEMBER_METHOD, id, 1) if id == godot_rs::script::method_id("damage") => {
            // SAFETY: The generated proxy retains one argument for this synchronous call.
            assert_eq!(unsafe { *arguments }, AbiValueV1::from_i64(5));
            AbiValueV1::from_i64(95)
        }
        (ABI_SCRIPT_MEMBER_GET_PROPERTY, 0, 0) => AbiValueV1::from_i64(100),
        (ABI_SCRIPT_MEMBER_SET_PROPERTY, 0, 1) => {
            // SAFETY: The generated setter retains one argument for this synchronous call.
            assert_eq!(unsafe { *arguments }, AbiValueV1::from_i64(100));
            AbiValueV1::NIL
        }
        _ => {
            CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
            return AbiCallResult::failure(
                AbiStatus::InvalidArgument,
                "invalid typed script member",
            );
        }
    };
    HOST_CALLS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: The SDK supplies a writable output for this synchronous call.
    unsafe { output.write(result) };
    AbiCallResult::OK
}

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
        b"is_action_pressed" => ExpectedCall {
            arguments: &[AbiValueType::STRING_NAME, AbiValueType::BOOL],
            ptrcall_arguments: &[AbiPtrcallType::STRING_NAME, AbiPtrcallType::BOOL],
            return_type: AbiValueType::BOOL,
            ptrcall_return: AbiPtrcallType::BOOL,
            result: AbiValueV1::from_bool(
                arguments.get(1).is_some_and(|value| value.payload[0] == 1),
            ),
        },
        b"draw_set_transform" => ExpectedCall {
            arguments: &[
                AbiValueType::VECTOR2,
                AbiValueType::F64,
                AbiValueType::VECTOR2,
            ],
            ptrcall_arguments: &[
                AbiPtrcallType::VECTOR2,
                AbiPtrcallType::F32,
                AbiPtrcallType::VECTOR2,
            ],
            return_type: AbiValueType::NIL,
            ptrcall_return: AbiPtrcallType::VOID,
            result: AbiValueV1::NIL,
        },
        b"add_child" => ExpectedCall {
            arguments: &[
                AbiValueType::OBJECT_ID,
                AbiValueType::BOOL,
                AbiValueType::I64,
            ],
            ptrcall_arguments: &[
                AbiPtrcallType::OBJECT,
                AbiPtrcallType::BOOL,
                AbiPtrcallType::I64,
            ],
            return_type: AbiValueType::NIL,
            ptrcall_return: AbiPtrcallType::VOID,
            result: AbiValueV1::NIL,
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
    let valid_defaults = match name {
        b"draw_set_transform" => {
            arguments[1].payload == AbiValueV1::from_f64(0.0).payload
                && arguments[2].payload == AbiValueV1::from_vector2(1.0, 1.0).payload
        }
        b"add_child" => {
            arguments[0].payload[0] == RECEIVER_ID
                && arguments[1].payload[0] == arguments[2].payload[0]
                && arguments[2].payload[0] <= 1
        }
        _ => true,
    };
    if !valid_defaults {
        CONTRACT_MISMATCH.store(true, Ordering::Relaxed);
        return AbiCallResult::failure(
            AbiStatus::InvalidArgument,
            "wrong method default or override",
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
    let input = ObjectRef::<Input>::__from_instance_id(RECEIVER_ID);
    assert!(
        !input
            .is_action_pressed_ex("jump")
            .done()
            .expect("default bool")
    );
    assert!(
        input
            .is_action_pressed_ex("jump")
            .exact_match(true)
            .done()
            .expect("overridden bool")
    );
    node.draw_set_transform_ex(Vector2::ZERO)
        .done()
        .expect("default math value");
    let child = ObjectRef::<Node>::__from_instance_id(RECEIVER_ID);
    node.add_child_ex(child).done().expect("default enum");
    node.add_child_ex(child)
        .force_readable_name(true)
        .internal(godot_rs::engine::node::InternalMode::INTERNAL_MODE_FRONT)
        .done()
        .expect("overridden enum");
}

fn main() {
    let mut reserved = [0; 16];
    reserved[HOST_API_SLOT_CALL_GODOT_METHOD] = call_godot_method as *const () as usize;
    reserved[HOST_API_SLOT_CALL_SCRIPT_MEMBER] = call_script_member as *const () as usize;
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

    let _ = godot_rs::script::abi_script_descriptor::<Health>("res://health.rs", "uid://c2");
    let health = NodeRef::<Health>::__from_instance_id(RECEIVER_ID);
    let exercise_script = || {
        assert_eq!(health.damage(black_box(5)).expect("typed method"), 95);
        assert_eq!(health.properties().points().expect("typed getter"), 100);
        health
            .properties()
            .set_points(black_box(100))
            .expect("typed setter");
    };
    exercise_script();
    HOST_CALLS.store(0, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    COUNT_ALLOCATIONS.store(true, Ordering::Release);
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        exercise_script();
    }
    let elapsed = started.elapsed();
    COUNT_ALLOCATIONS.store(false, Ordering::Release);
    let calls = HOST_CALLS.load(Ordering::Relaxed);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    println!(
        "typed_script_api calls={calls} ns_per_call={} allocations={allocations}",
        elapsed.as_nanos() / calls as u128
    );
    assert_eq!(calls, ITERATIONS * 3);
    assert_eq!(allocations, 0, "typed scalar script calls allocated");
    assert!(!CONTRACT_MISMATCH.load(Ordering::Relaxed));

    // SAFETY: The initialized module still points to the live Host table and
    // no generated call is active.
    let shutdown_status = unsafe { godot_rs::module::shutdown(ptr::null_mut()) };
    assert_eq!(shutdown_status, AbiStatus::Ok);
}
