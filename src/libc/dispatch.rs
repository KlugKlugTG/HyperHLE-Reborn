/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Grand Central Dispatch (GCD) — `dispatch/dispatch.h`
//!
//! Main-queue submissions are scheduled on the main thread's run loop.
//! Background queues run on reusable guest workers, with FIFO serial queues.

mod queues;

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{
    export_c_func, export_c_func_aliased, ConstantExports, FunctionExports, HostConstant,
};
use crate::libc::semaphore::{host_create_semaphore, host_destroy_semaphore, sem_t};
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{id, objc_classes, ClassExports, TrivialHostObject};
use crate::{Environment, ThreadId};
use std::collections::{HashMap, VecDeque};

use super::dispatch_source::{
    self, dispatch_resume, dispatch_source_cancel, dispatch_source_create,
    dispatch_source_get_data, dispatch_source_get_handle, dispatch_source_get_mask,
    dispatch_source_merge_data, dispatch_source_set_cancel_handler,
    dispatch_source_set_cancel_handler_f, dispatch_source_set_event_handler,
    dispatch_source_set_event_handler_f, dispatch_source_set_timer, dispatch_source_testcancel,
    dispatch_suspend,
};

// MARK: - Types

/// `dispatch_once_t` is a `long` (i32 on 32-bit) used as a flag.
#[allow(non_camel_case_types)]
pub type dispatch_once_t = i32;

/// Opaque queue handle — we use a small integer sentinel.
#[allow(non_camel_case_types)]
pub type dispatch_queue_t = MutVoidPtr;

/// Opaque group handle.
#[allow(non_camel_case_types)]
pub type dispatch_group_t = MutVoidPtr;

/// Opaque semaphore handle.
#[allow(non_camel_case_types)]
pub type dispatch_semaphore_t = MutVoidPtr;

/// Opaque source handle.
#[allow(non_camel_case_types)]
pub type dispatch_source_t = MutVoidPtr;

/// Opaque data handle.
#[allow(non_camel_case_types)]
pub type dispatch_data_t = MutVoidPtr;

/// Block context passed to dispatch functions: `void (^block)(void)`.
#[allow(non_camel_case_types)]
pub type dispatch_block_t = MutVoidPtr;

/// Priority levels for `dispatch_get_global_queue`.
const DISPATCH_QUEUE_PRIORITY_HIGH: i32 = 2;
const DISPATCH_QUEUE_PRIORITY_DEFAULT: i32 = 0;
const DISPATCH_QUEUE_PRIORITY_LOW: i32 = -2;
const DISPATCH_QUEUE_PRIORITY_BACKGROUND: i32 = i32::MIN;

/// Queue attribute constants.
const DISPATCH_QUEUE_SERIAL: i32 = 0;
const DISPATCH_QUEUE_CONCURRENT: i32 = 1;

/// `dispatch_time_t` constants.
const DISPATCH_TIME_NOW: u64 = 0;
pub(super) const DISPATCH_TIME_FOREVER: u64 = u64::MAX;

/// Dispatch objects are Objective-C objects on iOS 6 and later. C and
/// Objective-C retention must share the same lifetime.
/// https://github.com/apple-oss-distributions/libdispatch/blob/main/os/object.h
pub(super) fn alloc_dispatch_object(env: &mut Environment, name: &str) -> MutVoidPtr {
    let class = env.objc.get_known_class(name, &mut env.mem);
    env.objc
        .alloc_object(class, Box::new(TrivialHostObject), &mut env.mem)
        .cast()
}

pub(super) fn main_queue(env: &mut Environment) -> u32 {
    if let Some(handle) = get_state(env).main_queue_handle {
        return handle;
    }
    let handle = alloc_dispatch_object(env, "OS_dispatch_queue").to_bits();
    get_state(env).main_queue_handle = Some(handle);
    handle
}

fn global_queue(env: &mut Environment, priority: i32) -> u32 {
    if let Some(&handle) = get_state(env).global_queues.get(&priority) {
        return handle;
    }
    let handle = alloc_dispatch_object(env, "OS_dispatch_queue").to_bits();
    let mut queue = queues::WorkerQueue::new(true);
    queue.make_immortal();
    get_state(env).worker_queues.insert(handle, queue);
    get_state(env).global_queues.insert(priority, handle);
    handle
}

pub const CONSTANTS: ConstantExports = &[
    (
        "__dispatch_main_q",
        HostConstant::Custom(|env| Ptr::from_bits(main_queue(env))),
    ),
    (
        "__dispatch_queue_main",
        HostConstant::Custom(|env| Ptr::from_bits(main_queue(env))),
    ),
    (
        "__dispatch_queue_attr_concurrent",
        HostConstant::Custom(|env| env.mem.alloc_and_write(1u32).cast().cast_const()),
    ),
];

// MARK: - State

#[derive(Default)]
pub struct State {
    /// Backing POSIX semaphore for each dispatch semaphore handle. Dispatch
    /// semaphores must really block, so they are backed by the same host
    /// semaphore objects as `sem_t` rather than a bare counter.
    semaphores: HashMap<u32, (MutPtr<sem_t>, u32)>,
    main_queue_handle: Option<u32>,
    global_queues: HashMap<i32, u32>,
    /// Named queues keyed by label.
    named_queues: HashMap<String, u32>,
    /// Cached guest C-string labels keyed by queue handle (`0` unused).
    queue_labels: HashMap<u32, crate::mem::ConstPtr<u8>>,
    /// Cached empty guest C-string for unknown queue handles.
    empty_label: Option<crate::mem::ConstPtr<u8>>,
    /// Queue-specific values set via `dispatch_queue_set_specific`, keyed by
    /// `(queue handle bits, key pointer bits)` and storing the context
    /// pointer bits. See
    /// <https://developer.apple.com/documentation/dispatch/1452828-dispatch_queue_set_specific>.
    queue_specifics: HashMap<(u32, u32), u32>,
    /// Per-thread stacks of queue handles currently executing a callback, so
    /// `dispatch_get_specific` can resolve against the running queue. When
    /// the stack is empty, see [current_queue].
    current_queue_stacks: HashMap<ThreadId, Vec<u32>>,
    /// FIFO work awaiting execution on the main thread.
    main_queue: VecDeque<MainQueueTask>,
    /// Nested run loops must not execute another main-queue callback while
    /// one is still running: the main queue is serial.
    draining_main_queue: bool,
    worker_queues: HashMap<u32, queues::WorkerQueue>,
    pending_worker_queues: std::collections::HashSet<u32>,
    worker_entry: Option<GuestFunction>,
    delayed: std::collections::BTreeMap<std::time::Instant, Vec<(dispatch_queue_t, MainQueueTask)>>,
    groups: HashMap<u32, DispatchGroup>,
    /// Active dispatch sources keyed by their opaque guest handles.
    pub(super) sources: HashMap<u32, dispatch_source::DispatchSource>,
}

pub(super) fn get_state(env: &mut Environment) -> &mut State {
    &mut env.libc_state.dispatch
}

pub(crate) fn ensure_worker_capacity(env: &mut Environment) {
    while let Some((&due, _)) = get_state(env).delayed.first_key_value() {
        if due > env.guest_clock.now() {
            break;
        }
        for (queue, task) in get_state(env).delayed.remove(&due).unwrap() {
            queues::enqueue(env, queue, task);
            dispatch_release(env, queue);
        }
    }
    queues::ensure_capacity(env);
}

pub(crate) fn next_deadline(env: &mut Environment) -> Option<std::time::Instant> {
    let due = get_state(env)
        .delayed
        .first_key_value()
        .map(|(&due, _)| due);
    due.map(|due| env.guest_clock.host_deadline(due))
}

/// The queue running the caller. Outside of a submitted block, Apple's
/// `dispatch_get_current_queue` "returns the main queue if the call is
/// executed from the main thread. If the call is made from any other thread,
/// this function returns the default concurrent queue."
/// <https://developer.apple.com/documentation/dispatch/dispatch_get_current_queue()>
/// <https://opensource.apple.com/source/libdispatch/libdispatch-84.5/man/dispatch_queue_create.3.auto.html>
///
/// Cut the Rope loads textures on an `NSThread` and only makes GL calls when
/// `dispatch_get_current_queue() == &_dispatch_main_q`, so answering "main
/// queue" there made it call GL on a thread with no EAGLContext.
fn current_queue(env: &mut Environment) -> u32 {
    let thread = env.current_thread;
    let fallback = if env.current_thread == 0 {
        main_queue(env)
    } else {
        global_queue(env, DISPATCH_QUEUE_PRIORITY_DEFAULT)
    };
    get_state(env)
        .current_queue_stacks
        .get(&thread)
        .and_then(|stack| stack.last())
        .copied()
        .unwrap_or(fallback)
}

// MARK: - dispatch_once

/// `dispatch_once(dispatch_once_t *predicate, dispatch_block_t block)`
///
/// Calls `block` at most once per initialized `dispatch_once_t` (`0` -> `~0`).
/// Reading the guest `predicate` word directly matches Clang's inline fast path
/// (`*predicate == ~0l`) and allows heap/ivar `dispatch_once_t` slots to work
/// when freed memory is later reallocated at the same address.
fn dispatch_once(
    env: &mut Environment,
    predicate: MutPtr<dispatch_once_t>,
    block: dispatch_block_t,
) {
    if predicate.is_null() || env.mem.read(predicate) == -1 {
        return;
    }
    // Mark the predicate so native code that reads it directly sees "done".
    env.mem.write(predicate, -1i32); // ~0 == done on Apple platforms

    if !block.is_null() {
        call_void_block(env, block);
    }
}

/// `dispatch_once_f(dispatch_once_t *predicate, void *context, dispatch_function_t work)`
fn dispatch_once_f(
    env: &mut Environment,
    predicate: MutPtr<dispatch_once_t>,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if predicate.is_null() || env.mem.read(predicate) == -1 {
        return;
    }
    env.mem.write(predicate, -1i32); // ~0 == done on Apple platforms

    if work.addr_with_thumb_bit() != 0 {
        let _: () = work.call_from_host(env, (context,));
    }
}
/// Swift's `swift_once` has the same once-token semantics as
/// `dispatch_once_f`: the Swift runtime passes a token, a context, and a
/// no-return-value callback. The ABI is documented in Swift's open-source
/// runtime (`stdlib/public/runtime/Once.cpp`).
fn swift_once(
    env: &mut Environment,
    predicate: MutPtr<dispatch_once_t>,
    function: GuestFunction,
    context: MutVoidPtr,
) {
    dispatch_once_f(env, predicate, context, function);
}

// MARK: - Queue creation / retrieval

fn dispatch_get_main_queue(env: &mut Environment) -> dispatch_queue_t {
    MutVoidPtr::from_bits(main_queue(env))
}

fn dispatch_get_global_queue(
    env: &mut Environment,
    identifier: i32,
    flags: u32,
) -> dispatch_queue_t {
    if flags != 0
        || !matches!(
            identifier,
            DISPATCH_QUEUE_PRIORITY_HIGH
                | DISPATCH_QUEUE_PRIORITY_DEFAULT
                | DISPATCH_QUEUE_PRIORITY_LOW
                | DISPATCH_QUEUE_PRIORITY_BACKGROUND
        )
    {
        return MutVoidPtr::null();
    }
    MutVoidPtr::from_bits(global_queue(env, identifier))
}

fn dispatch_queue_create(
    env: &mut Environment,
    label: ConstVoidPtr, // const char*
    attr: i32,           // DISPATCH_QUEUE_SERIAL / DISPATCH_QUEUE_CONCURRENT
) -> dispatch_queue_t {
    let raw_label = if label.is_null() {
        String::new()
    } else {
        env.mem
            .cstr_at_utf8(label.cast())
            .unwrap_or_default()
            .to_owned()
    };
    log_dbg!(
        "dispatch_queue_create({:?}, attr={}) -> stub",
        if raw_label.is_empty() {
            "<anonymous>"
        } else {
            raw_label.as_str()
        },
        attr
    );
    let label_cstr = env
        .mem
        .alloc_and_write_cstr(raw_label.as_bytes())
        .cast_const();
    let handle = alloc_dispatch_object(env, "OS_dispatch_queue").to_bits();
    let st = get_state(env);
    if !raw_label.is_empty() {
        st.named_queues.insert(raw_label, handle);
    }
    st.queue_labels.insert(handle, label_cstr);
    st.worker_queues.insert(
        handle,
        queues::WorkerQueue::new(attr != DISPATCH_QUEUE_SERIAL),
    );
    MutVoidPtr::from_bits(handle)
}

fn dispatch_queue_attr_make_with_qos_class(
    _env: &mut Environment,
    attr: dispatch_queue_t,
    _qos_class: u32,
    _relative_priority: i32,
) -> dispatch_queue_t {
    attr
}

fn dispatch_queue_get_label(env: &mut Environment, queue: dispatch_queue_t) -> ConstVoidPtr {
    // DISPATCH_CURRENT_QUEUE_LABEL is NULL (0): resolve to the running queue.
    let handle = if queue.is_null() {
        current_queue(env)
    } else {
        queue.to_bits()
    };
    if let Some(&ptr) = get_state(env).queue_labels.get(&handle) {
        return ptr.cast_void();
    }
    if handle == main_queue(env) {
        let ptr = env
            .mem
            .alloc_and_write_cstr(b"com.apple.main-thread")
            .cast_const();
        get_state(env).queue_labels.insert(handle, ptr);
        return ptr.cast_void();
    }
    if handle == global_queue(env, DISPATCH_QUEUE_PRIORITY_DEFAULT) {
        let ptr = env
            .mem
            .alloc_and_write_cstr(b"com.apple.root.default-priority")
            .cast_const();
        get_state(env).queue_labels.insert(handle, ptr);
        return ptr.cast_void();
    }
    if let Some(empty) = get_state(env).empty_label {
        return empty.cast_void();
    }
    let empty = env.mem.alloc_and_write_cstr(b"").cast_const();
    get_state(env).empty_label = Some(empty);
    empty.cast_void()
}

fn dispatch_get_current_queue(env: &mut Environment) -> dispatch_queue_t {
    let current = current_queue(env);
    MutVoidPtr::from_bits(current)
}

fn dispatch_queue_retain(env: &mut Environment, queue: dispatch_queue_t) -> dispatch_queue_t {
    dispatch_retain(env, queue);
    queue
}

fn dispatch_queue_release(env: &mut Environment, queue: dispatch_queue_t) {
    dispatch_release(env, queue);
}

fn dispatch_retain(env: &mut Environment, obj: MutVoidPtr) {
    if get_state(env).main_queue_handle == Some(obj.to_bits()) {
        return;
    }
    if let Some((_, references)) = get_state(env).semaphores.get_mut(&obj.to_bits()) {
        *references = references
            .checked_add(1)
            .expect("dispatch semaphore retain overflow");
        return;
    }
    if queues::retain(env, obj.to_bits()) {
        return;
    }
    if let Some(group) = get_state(env).groups.get_mut(&obj.to_bits()) {
        group.references = group
            .references
            .checked_add(1)
            .expect("dispatch group retain overflow");
        return;
    }
    dispatch_source::retain_source(env, obj);
}

fn dispatch_release(env: &mut Environment, obj: MutVoidPtr) {
    if get_state(env).main_queue_handle == Some(obj.to_bits()) {
        return;
    }
    if queues::release(env, obj.to_bits()) {
        return;
    }
    if let Some(group) = get_state(env).groups.get_mut(&obj.to_bits()) {
        group.references = group
            .references
            .checked_sub(1)
            .expect("dispatch group over-release");
        if group.references == 0 && group.pending == 0 {
            get_state(env).groups.remove(&obj.to_bits());
            env.objc.dealloc_object(obj.cast(), &mut env.mem);
        }
        return;
    }
    if !release_if_semaphore(env, obj) {
        dispatch_source::release_source(env, obj);
    }
}

// MARK: - Dispatch async / sync

enum MainQueueWork {
    Block(dispatch_block_t),
    Function(GuestFunction, MutVoidPtr),
}

struct MainQueueTask {
    work: MainQueueWork,
    /// Async blocks are copied and owned by the queue. Sync blocks remain
    /// owned by their blocked caller, which is signalled after execution.
    completion: Option<MutPtr<sem_t>>,
    group: Option<u32>,
    barrier: bool,
}

#[derive(Default)]
struct DispatchGroup {
    pending: u32,
    references: usize,
    generation: u64,
    waiters: Vec<ThreadId>,
    notifications: Vec<(dispatch_queue_t, MainQueueTask)>,
}

/// Apple documents main-queue tasks as serial work on the main thread,
/// serviced by its run loop. Take a bounded batch so callbacks that submit
/// more work cannot indefinitely starve the rest of that run loop.
/// <https://developer.apple.com/library/archive/documentation/General/Conceptual/ConcurrencyProgrammingGuide/OperationQueues/OperationQueues.html>
pub(crate) fn drain_main_queue(env: &mut Environment) {
    assert_eq!(env.current_thread, 0);
    let state = get_state(env);
    if state.draining_main_queue || state.main_queue.is_empty() {
        return;
    }
    state.draining_main_queue = true;
    let count = state.main_queue.len();
    for _ in 0..count {
        let task = get_state(env).main_queue.pop_front().unwrap();
        let queue = Ptr::from_bits(main_queue(env));
        with_current_queue(env, queue, |env| match task.work {
            MainQueueWork::Block(block) => {
                call_void_block(env, block);
                if task.completion.is_none() {
                    crate::libc::blocks::_Block_release(env, block.cast_const());
                }
            }
            MainQueueWork::Function(work, context) => {
                let _: () = work.call_from_host(env, (context,));
            }
        });
        if let Some(completion) = task.completion {
            assert!(env.sem_increment(completion));
        }
        if let Some(group) = task.group {
            dispatch_group_leave(env, Ptr::from_bits(group));
            dispatch_release(env, Ptr::from_bits(group));
        }
    }
    get_state(env).draining_main_queue = false;
}

fn dispatch_main_sync(env: &mut Environment, work: MainQueueWork) {
    // Apple documents dispatch_sync to the main queue from the main thread
    // as a deadlock. Report the misuse instead of executing it inline.
    assert_ne!(
        env.current_thread, 0,
        "dispatch_sync to the main queue from the main thread would deadlock"
    );
    let completion = host_create_semaphore(env, 0);
    get_state(env).main_queue.push_back(MainQueueTask {
        work,
        completion: Some(completion),
        group: None,
        barrier: false,
    });
    assert!(env.sem_decrement(completion, true));
    host_destroy_semaphore(env, completion);
}

fn dispatch_async(env: &mut Environment, queue: dispatch_queue_t, block: dispatch_block_t) {
    if block.is_null() {
        return;
    }
    let block = crate::libc::blocks::_Block_copy(env, block.cast_const()).cast_mut();
    queues::enqueue(
        env,
        queue,
        MainQueueTask {
            work: MainQueueWork::Block(block),
            completion: None,
            group: None,
            barrier: false,
        },
    );
}

fn dispatch_sync(env: &mut Environment, queue: dispatch_queue_t, block: dispatch_block_t) {
    if block.is_null() {
        return;
    }
    if queue.to_bits() == main_queue(env) {
        dispatch_main_sync(env, MainQueueWork::Block(block));
    } else if queues::requires_sync_worker(env, queue) {
        queues::sync(env, queue, MainQueueWork::Block(block));
    } else {
        with_current_queue(env, queue, |env| call_void_block(env, block));
    }
}

fn dispatch_async_f(
    env: &mut Environment,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() {
        return;
    }
    queues::enqueue(
        env,
        queue,
        MainQueueTask {
            work: MainQueueWork::Function(work, context),
            completion: None,
            group: None,
            barrier: false,
        },
    );
}

fn dispatch_sync_f(
    env: &mut Environment,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() {
        return;
    }
    if queue.to_bits() == main_queue(env) {
        dispatch_main_sync(env, MainQueueWork::Function(work, context));
    } else if queues::requires_sync_worker(env, queue) {
        queues::sync(env, queue, MainQueueWork::Function(work, context));
    } else {
        with_current_queue(env, queue, |env| {
            let _: () = work.call_from_host(env, (context,));
        });
    }
}

// MARK: - dispatch_apply

fn dispatch_apply(
    env: &mut Environment,
    iterations: u32,
    _queue: dispatch_queue_t,
    block: MutVoidPtr,
) {
    if block.is_null() {
        return;
    }
    let invoke_ptr = env.mem.read(block.cast::<u32>() + 3u32);
    if invoke_ptr == 0 {
        return;
    }
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_ptr);
    for i in 0..iterations {
        let _: () = invoke.call_from_host(env, (block, i));
    }
}

fn dispatch_apply_f(
    env: &mut Environment,
    iterations: u32,
    _queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() {
        return;
    }
    for i in 0..iterations {
        let _: () = work.call_from_host(env, (context, i));
    }
}

// MARK: - dispatch_after

/// Convert a `dispatch_time_t` into a delay in seconds, relative to now.
///
/// touchHLE's `dispatch_time(DISPATCH_TIME_NOW, delta)` and
/// `dispatch_walltime(_, delta)` both return the nanosecond `delta` directly
/// (because `DISPATCH_TIME_NOW` is 0), so a `when` produced by them is simply
/// "nanoseconds from now". We clamp to a sane range: never negative, and at
/// most 60s so a bogus/absolute value can't wedge a timer decades out.
pub(super) fn dispatch_after_delay_secs(when: u64) -> f64 {
    if when == DISPATCH_TIME_FOREVER {
        return f64::INFINITY;
    }
    let secs = (when as f64) / 1_000_000_000.0;
    secs.clamp(0.0, 86_400.0)
}

/// `dispatch_after(when, queue, block)`
///
/// Crucially this must be *asynchronous*: the block has to run on a later run
/// loop turn, not inline. Calling it synchronously (as touchHLE used to) breaks
/// the extremely common pattern of a method re-arming itself with
/// `dispatch_after` (blink/poll/retry animations) — that recurses into itself
/// without bound and either overflows the native stack or, with the
/// objc_msgSend recursion guard, corrupts state and starves the render loop.
/// Doodle Jump's `-[JumpAppDelegate blinkTournamentButton:]` is one such case.
///
/// The scheduler submits the copied block to its queue after the deadline;
/// the submitting thread does not need a running NSRunLoop.
fn dispatch_after(
    env: &mut Environment,
    when: u64,
    queue: dispatch_queue_t,
    block: dispatch_block_t,
) {
    if block.is_null() || when == DISPATCH_TIME_FOREVER {
        return;
    }
    let block = crate::libc::blocks::_Block_copy(env, block.cast_const()).cast_mut();
    schedule_after(env, when, queue, MainQueueWork::Block(block));
}

fn dispatch_after_f(
    env: &mut Environment,
    when: u64,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() || when == DISPATCH_TIME_FOREVER {
        return;
    }
    schedule_after(env, when, queue, MainQueueWork::Function(work, context));
}

fn schedule_after(env: &mut Environment, when: u64, queue: dispatch_queue_t, work: MainQueueWork) {
    // Dispatch deadlines do not depend on the caller having a running
    // NSRunLoop. The scheduler wakes for the deadline and submits to the
    // requested queue, which then executes with its normal queue identity.
    // https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_after.3
    let due =
        env.guest_clock.now() + std::time::Duration::from_secs_f64(dispatch_after_delay_secs(when));
    dispatch_retain(env, queue);
    get_state(env).delayed.entry(due).or_default().push((
        queue,
        MainQueueTask {
            work,
            completion: None,
            group: None,
            barrier: false,
        },
    ));
}

/// Host-only helper for dispatch source timer and event callbacks.
pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);

@implementation OS_object: NSObject
- (id)retain {
    dispatch_retain(env, this.cast());
    this
}
- (())release {
    dispatch_release(env, this.cast());
}
- (id)autorelease {
    let _: () = crate::objc::msg_class![env; NSAutoreleasePool addObject:this];
    this
}
@end

@implementation OS_dispatch_object: OS_object
@end
@implementation OS_dispatch_queue: OS_dispatch_object
@end
@implementation OS_dispatch_group: OS_dispatch_object
@end
@implementation OS_dispatch_semaphore: OS_dispatch_object
@end
@implementation OS_dispatch_source: OS_dispatch_object
@end

@implementation _touchHLE_DispatchAfter: NSObject

+ (())_touchHLE_dispatchSourceTimerFired:(id)timer {
    super::dispatch_source::source_timer_fired(env, timer);
}

+ (())_touchHLE_dispatchSourceEventFired:(id)timer {
    super::dispatch_source::source_event_fired(env, timer);
}

@end
};

// MARK: - dispatch_time

fn dispatch_time(_env: &mut Environment, when: u64, delta: i64) -> u64 {
    if when == DISPATCH_TIME_FOREVER {
        return DISPATCH_TIME_FOREVER;
    }
    if delta >= 0 {
        when.saturating_add(delta as u64)
            .min(DISPATCH_TIME_FOREVER - 1)
    } else {
        when.saturating_sub(delta.unsigned_abs())
    }
}

fn dispatch_walltime(_env: &mut Environment, _spec: ConstVoidPtr, delta: i64) -> u64 {
    // Return a time `delta` nanoseconds from now (approximate).
    delta.max(0) as u64
}

// MARK: - dispatch_group

fn dispatch_group_create(env: &mut Environment) -> dispatch_group_t {
    let handle = alloc_dispatch_object(env, "OS_dispatch_group").to_bits();
    let st = get_state(env);
    st.groups.insert(
        handle,
        DispatchGroup {
            references: 1,
            ..Default::default()
        },
    );
    MutVoidPtr::from_bits(handle)
}

fn dispatch_group_async(
    env: &mut Environment,
    group: dispatch_group_t,
    queue: dispatch_queue_t,
    block: dispatch_block_t,
) {
    if block.is_null() {
        return;
    }
    let block = crate::libc::blocks::_Block_copy(env, block.cast_const()).cast_mut();
    dispatch_retain(env, group);
    dispatch_group_enter(env, group);
    queues::enqueue(
        env,
        queue,
        MainQueueTask {
            work: MainQueueWork::Block(block),
            completion: None,
            group: Some(group.to_bits()),
            barrier: false,
        },
    );
}

fn dispatch_group_async_f(
    env: &mut Environment,
    group: dispatch_group_t,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() {
        return;
    }
    dispatch_retain(env, group);
    dispatch_group_enter(env, group);
    queues::enqueue(
        env,
        queue,
        MainQueueTask {
            work: MainQueueWork::Function(work, context),
            completion: None,
            group: Some(group.to_bits()),
            barrier: false,
        },
    );
}

fn dispatch_group_enter(env: &mut Environment, group: dispatch_group_t) {
    let group = get_state(env)
        .groups
        .get_mut(&group.to_bits())
        .expect("invalid dispatch group");
    if group.pending == 0 {
        group.references = group
            .references
            .checked_add(1)
            .expect("dispatch group retain overflow");
    }
    group.pending = group
        .pending
        .checked_add(1)
        .expect("dispatch group overflow");
}

fn dispatch_group_leave(env: &mut Environment, group: dispatch_group_t) {
    let handle = group;
    let group = get_state(env)
        .groups
        .get_mut(&group.to_bits())
        .expect("invalid dispatch group");
    group.pending = group
        .pending
        .checked_sub(1)
        .expect("unbalanced dispatch_group_leave");
    if group.pending != 0 {
        return;
    }
    group.generation = group.generation.wrapping_add(1);
    let waiters = std::mem::take(&mut group.waiters);
    let notifications = std::mem::take(&mut group.notifications);
    for thread in waiters {
        env.threads[thread].blocked_by = crate::environment::ThreadBlock::NotBlocked;
    }
    for (queue, task) in notifications {
        queues::enqueue(env, queue, task);
        dispatch_release(env, queue);
        dispatch_release(env, handle);
    }
    // The first enter retains the group until the final leave.
    dispatch_release(env, handle);
}

fn dispatch_group_wait(env: &mut Environment, group: dispatch_group_t, timeout: u64) -> i32 {
    let tid = env.current_thread;
    if get_state(env).groups[&group.to_bits()].pending == 0 {
        return 0;
    }
    if timeout == DISPATCH_TIME_NOW {
        return 1;
    }
    let generation = get_state(env).groups[&group.to_bits()].generation;
    get_state(env)
        .groups
        .get_mut(&group.to_bits())
        .unwrap()
        .waiters
        .push(tid);
    if timeout == DISPATCH_TIME_FOREVER {
        env.suspend_thread(tid);
    } else {
        let due = env.guest_clock.now() + std::time::Duration::from_nanos(timeout);
        env.yield_thread(crate::environment::ThreadBlock::GuestSleeping(due));
    }
    let group = get_state(env).groups.get_mut(&group.to_bits()).unwrap();
    group.waiters.retain(|&waiter| waiter != tid);
    i32::from(group.pending != 0 && group.generation == generation)
}

fn dispatch_group_notify(
    env: &mut Environment,
    group: dispatch_group_t,
    queue: dispatch_queue_t,
    block: dispatch_block_t,
) {
    if block.is_null() {
        return;
    }
    let block = crate::libc::blocks::_Block_copy(env, block.cast_const()).cast_mut();
    group_notify(env, group, queue, MainQueueWork::Block(block));
}

fn dispatch_group_notify_f(
    env: &mut Environment,
    group: dispatch_group_t,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work != GuestFunction::null_ptr() {
        group_notify(env, group, queue, MainQueueWork::Function(work, context));
    }
}

fn group_notify(
    env: &mut Environment,
    group: dispatch_group_t,
    queue: dispatch_queue_t,
    work: MainQueueWork,
) {
    // Notifications are queued once all entered work completes, including
    // when a callback sleeps or waits for another guest thread.
    // https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_group_create.3
    let task = MainQueueTask {
        work,
        completion: None,
        group: None,
        barrier: false,
    };
    dispatch_retain(env, queue);
    let group = get_state(env)
        .groups
        .get_mut(&group.to_bits())
        .expect("invalid dispatch group");
    if group.pending == 0 {
        queues::enqueue(env, queue, task);
        dispatch_release(env, queue);
    } else {
        group.references = group
            .references
            .checked_add(1)
            .expect("dispatch group retain overflow");
        group.notifications.push((queue, task));
    }
}

// MARK: - dispatch_semaphore

fn dispatch_semaphore_create(env: &mut Environment, value: i32) -> dispatch_semaphore_t {
    // A dispatch semaphore has to be able to actually block a waiter, so back
    // it with the same host semaphore object `sem_t` uses: that machinery is
    // already integrated with the thread scheduler
    // (`ThreadBlock::Semaphore`). A bare counter cannot block, which turns a
    // worker thread's "wait for work" into a busy spin and deadlocks any guest
    // that hands work to a thread pool.
    if value < 0 {
        return MutVoidPtr::null();
    }
    let backing = host_create_semaphore(env, value as u32);
    let handle = alloc_dispatch_object(env, "OS_dispatch_semaphore").to_bits();
    get_state(env).semaphores.insert(handle, (backing, 1));
    MutVoidPtr::from_bits(handle)
}

fn dispatch_semaphore_wait(env: &mut Environment, sem: dispatch_semaphore_t, timeout: u64) -> i32 {
    let key = sem.to_bits();
    let Some(&(backing, _)) = get_state(env).semaphores.get(&key) else {
        log!("dispatch_semaphore_wait: unknown semaphore {:?}", sem);
        return -1;
    };
    // `DISPATCH_TIME_NOW` means "poll": return non-zero instead of blocking
    // when the semaphore is unavailable. Any other timeout blocks; we do not
    // model a finite timeout yet, so those wait until signalled (documented
    // below) rather than spinning.
    if timeout == DISPATCH_TIME_NOW {
        if env.sem_decrement(backing, false) {
            0
        } else {
            // Non-zero return == timed out, per dispatch_semaphore_wait(3).
            -1
        }
    } else if timeout == DISPATCH_TIME_FOREVER {
        // Block until signalled. (Must stay a real block: Telltale's
        // JobScheduler counts jobs via dispatch semaphores and does not
        // tolerate a wait returning without a real signal.)
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DISPATCHSEM") {
            log!(
                "DSEM t={} wait-enter sem={:#x} lr={:#x}",
                env.current_thread,
                sem.to_bits(),
                env.cpu.regs()[14]
            );
        }
        env.sem_decrement(backing, true);
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DISPATCHSEM") {
            log!(
                "DSEM t={} wait-exit sem={:#x}",
                env.current_thread,
                sem.to_bits()
            );
        }
        0
    } else {
        // Finite timeout: wait up to `timeout` and return non-zero (timed out)
        // if no signal arrives. This MUST be honored, not treated as infinite:
        // touchHLE's `dispatch_time(DISPATCH_TIME_NOW, ns)` returns the
        // relative nanosecond delta (it adds to a zero base), so `timeout` is
        // that duration. Telltale's `PlatformSemaphore::TimedWait` — the
        // render thread's frame handshake — depends on the wait actually
        // timing out so the render loop can poll and advance; treating it as
        // infinite deadlocked `RenderThread::FinishFrame` (the frozen-spinner
        // hang). Poll like semaphore_timedwait does.
        if crate::env_flag_cached!("TOUCHHLE_TRACE_DISPATCHSEM") {
            log!(
                "DSEM t={} timedwait-enter sem={:#x} timeout_ns={} lr={:#x}",
                env.current_thread,
                sem.to_bits(),
                timeout,
                env.cpu.regs()[14]
            );
        }
        let timeout_ns = timeout.min(3_600_000_000_000); // cap at 1 hour
        let deadline = env.guest_clock.now() + std::time::Duration::from_nanos(timeout_ns);
        const POLL: std::time::Duration = std::time::Duration::from_millis(1);
        loop {
            if env.sem_decrement(backing, false) {
                return 0;
            }
            let now = env.guest_clock.now();
            if now >= deadline {
                // Timed out: dispatch_semaphore_wait returns non-zero.
                return 1;
            }
            env.sleep_guest((deadline - now).min(POLL));
        }
    }
}

fn dispatch_semaphore_signal(env: &mut Environment, sem: dispatch_semaphore_t) -> i32 {
    let key = sem.to_bits();
    let Some(&(backing, _)) = get_state(env).semaphores.get(&key) else {
        log!("dispatch_semaphore_signal: unknown semaphore {:?}", sem);
        return 0;
    };
    if crate::env_flag_cached!("TOUCHHLE_TRACE_DISPATCHSEM") {
        log!(
            "DSEM t={} signal sem={:#x} lr={:#x}",
            env.current_thread,
            sem.to_bits(),
            env.cpu.regs()[14]
        );
    }
    env.sem_increment(backing);
    // Returns non-zero if a thread was woken. We do not track that here, and
    // every in-tree caller ignores the result.
    0
}

/// Release the backing semaphore for a dispatch semaphore handle, if `obj` is
/// one. Returns whether it was.
fn release_if_semaphore(env: &mut Environment, obj: MutVoidPtr) -> bool {
    let key = obj.to_bits();
    let Some((backing, references)) = get_state(env).semaphores.get_mut(&key) else {
        return false;
    };
    *references = references
        .checked_sub(1)
        .expect("dispatch semaphore over-release");
    if *references != 0 {
        return true;
    }
    let backing = *backing;
    get_state(env).semaphores.remove(&key);
    host_destroy_semaphore(env, backing);
    env.objc.dealloc_object(obj.cast(), &mut env.mem);
    true
}

// MARK: - dispatch_source

// MARK: - dispatch_barrier

fn dispatch_barrier_async(env: &mut Environment, queue: dispatch_queue_t, block: dispatch_block_t) {
    if block.is_null() {
        return;
    }
    let block = crate::libc::blocks::_Block_copy(env, block.cast_const()).cast_mut();
    queues::enqueue(
        env,
        queue,
        MainQueueTask {
            work: MainQueueWork::Block(block),
            completion: None,
            group: None,
            barrier: true,
        },
    );
}

fn dispatch_barrier_sync(env: &mut Environment, queue: dispatch_queue_t, block: dispatch_block_t) {
    if block.is_null() {
        return;
    }
    if queue.to_bits() == main_queue(env)
        || queue.to_bits() == global_queue(env, DISPATCH_QUEUE_PRIORITY_DEFAULT)
    {
        dispatch_sync(env, queue, block);
    } else {
        queues::sync_barrier(env, queue, MainQueueWork::Block(block));
    }
}

fn dispatch_barrier_async_f(
    env: &mut Environment,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() {
        return;
    }
    queues::enqueue(
        env,
        queue,
        MainQueueTask {
            work: MainQueueWork::Function(work, context),
            completion: None,
            group: None,
            barrier: true,
        },
    );
}

fn dispatch_barrier_sync_f(
    env: &mut Environment,
    queue: dispatch_queue_t,
    context: MutVoidPtr,
    work: GuestFunction,
) {
    if work == GuestFunction::null_ptr() {
        return;
    }
    if queue.to_bits() == main_queue(env)
        || queue.to_bits() == global_queue(env, DISPATCH_QUEUE_PRIORITY_DEFAULT)
    {
        dispatch_sync_f(env, queue, context, work);
    } else {
        queues::sync_barrier(env, queue, MainQueueWork::Function(work, context));
    }
}

// MARK: - dispatch_set_context / dispatch_get_context

fn dispatch_set_context(env: &mut Environment, object: MutVoidPtr, context: MutVoidPtr) {
    dispatch_source::set_context(env, object, context);
}

fn dispatch_get_context(env: &mut Environment, object: MutVoidPtr) -> MutVoidPtr {
    dispatch_source::get_context(env, object).unwrap_or_else(MutVoidPtr::null)
}

/// `void dispatch_queue_set_specific(dispatch_queue_t queue, const void *key,
///                                   void *context,
///                                   dispatch_function_t destructor)`
///
/// Associates `context` with `queue` under the unique pointer `key`. Passing
/// a NULL `context` clears any existing association (Apple documents NULL as
/// "remove the value"). The `destructor` is the cleanup callback GCD would run
/// when the value is replaced or the queue is destroyed; touchHLE does not own
/// the guest's context allocation, so it is intentionally not invoked.
/// <https://developer.apple.com/documentation/dispatch/1452828-dispatch_queue_set_specific>
fn dispatch_queue_set_specific(
    env: &mut Environment,
    queue: dispatch_queue_t,
    key: ConstVoidPtr,
    context: MutVoidPtr,
    _destructor: GuestFunction,
) {
    let qk = (queue.to_bits(), key.to_bits());
    let state = get_state(env);
    if context.is_null() {
        state.queue_specifics.remove(&qk);
    } else {
        state.queue_specifics.insert(qk, context.to_bits());
    }
}

/// `void *dispatch_queue_get_specific(dispatch_queue_t queue, const void *key)`
///
/// Returns the value previously associated with `key` on `queue`, or NULL.
/// <https://developer.apple.com/documentation/dispatch/1453028-dispatch_queue_get_specific>
fn dispatch_queue_get_specific(
    env: &mut Environment,
    queue: dispatch_queue_t,
    key: ConstVoidPtr,
) -> MutVoidPtr {
    let qk = (queue.to_bits(), key.to_bits());
    match get_state(env).queue_specifics.get(&qk) {
        Some(&bits) => MutVoidPtr::from_bits(bits),
        None => MutVoidPtr::null(),
    }
}

/// `void *dispatch_get_specific(const void *key)`
///
/// Returns the value associated with `key` on the queue currently executing
/// the caller. We track the running queue via per-thread stacks
/// (pushed/popped around `dispatch_sync`/`dispatch_async` block execution);
/// when nothing is running we resolve against the main queue on the main
/// thread (and the default global queue elsewhere, see [current_queue]),
/// matching the behaviour callers rely on for "am I on queue X?" checks.
/// <https://developer.apple.com/documentation/dispatch/1453099-dispatch_get_specific>
fn dispatch_get_specific(env: &mut Environment, key: ConstVoidPtr) -> MutVoidPtr {
    let current = current_queue(env);
    match get_state(env)
        .queue_specifics
        .get(&(current, key.to_bits()))
    {
        Some(&bits) => MutVoidPtr::from_bits(bits),
        None => MutVoidPtr::null(),
    }
}

fn dispatch_set_finalizer_f(
    _env: &mut Environment,
    _object: MutVoidPtr,
    _finalizer: GuestFunction,
) {
}

fn dispatch_set_target_queue(
    _env: &mut Environment,
    _object: MutVoidPtr,
    _queue: dispatch_queue_t,
) {
}

// MARK: - dispatch_main

fn dispatch_main(_env: &mut Environment) {
    log!("dispatch_main: stubbed (returning immediately)");
    // Cannot spin forever here since GuestRet requires (). Just return.
}

/// `void dispatch_debug(dispatch_object_t object, const char *message, ...)`
///
/// Per Apple docs: "Programmatically logs debug information about a dispatch
/// object." This is a debugging aid that apps rarely call in production,
/// but some Unity-based games (Frontline Commando 2) reference it. Since
/// we don't have real dispatch object internals to inspect, we simply no-op.
fn dispatch_debug(_env: &mut Environment, _object: MutVoidPtr, _message: ConstVoidPtr) {
    // No-op. The message is a printf-style format string with varargs that
    // we intentionally ignore.
}

// MARK: - Helpers

pub(super) fn with_current_queue(
    env: &mut Environment,
    queue: dispatch_queue_t,
    callback: impl FnOnce(&mut Environment),
) {
    let thread = env.current_thread;
    get_state(env)
        .current_queue_stacks
        .entry(thread)
        .or_default()
        .push(queue.to_bits());
    callback(env);
    let state = get_state(env);
    let stack = state.current_queue_stacks.get_mut(&thread).unwrap();
    assert_eq!(stack.pop(), Some(queue.to_bits()));
    if stack.is_empty() {
        state.current_queue_stacks.remove(&thread);
    }
}

/// Invoke a GCD block (`void (^)(void)`).
/// On ARM32 the fourth word of a block is the invoke function pointer.
/// Layout: [isa, flags, reserved, invoke, descriptor, captures...]
pub(crate) fn call_void_block(env: &mut Environment, block: dispatch_block_t) {
    let invoke_ptr = env.mem.read(block.cast::<u32>() + 3u32);
    if invoke_ptr == 0 {
        return;
    }
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_ptr);
    let _: () = invoke.call_from_host(env, (block,));
}

pub const FUNCTIONS: FunctionExports = &[
    // once
    export_c_func!(dispatch_once(_, _)),
    export_c_func!(dispatch_once_f(_, _, _)),
    export_c_func_aliased!("swift_once", swift_once(_, _, _)),
    // queues
    export_c_func!(dispatch_get_main_queue()),
    export_c_func!(dispatch_get_global_queue(_, _)),
    export_c_func!(dispatch_get_current_queue()),
    export_c_func!(dispatch_queue_create(_, _)),
    export_c_func!(dispatch_queue_attr_make_with_qos_class(_, _, _)),
    export_c_func!(dispatch_queue_get_label(_)),
    export_c_func!(dispatch_queue_retain(_)),
    export_c_func!(dispatch_queue_release(_)),
    export_c_func!(dispatch_retain(_)),
    export_c_func!(dispatch_release(_)),
    // async / sync
    export_c_func!(dispatch_async(_, _)),
    export_c_func!(dispatch_sync(_, _)),
    export_c_func!(dispatch_async_f(_, _, _)),
    export_c_func!(dispatch_sync_f(_, _, _)),
    // apply
    export_c_func!(dispatch_apply(_, _, _)),
    export_c_func!(dispatch_apply_f(_, _, _, _)),
    // after
    export_c_func!(dispatch_after(_, _, _)),
    export_c_func!(dispatch_after_f(_, _, _, _)),
    // time
    export_c_func!(dispatch_time(_, _)),
    export_c_func!(dispatch_walltime(_, _)),
    // group
    export_c_func!(dispatch_group_create()),
    export_c_func!(dispatch_group_async(_, _, _)),
    export_c_func!(dispatch_group_async_f(_, _, _, _)),
    export_c_func!(dispatch_group_enter(_)),
    export_c_func!(dispatch_group_leave(_)),
    export_c_func!(dispatch_group_wait(_, _)),
    export_c_func!(dispatch_group_notify(_, _, _)),
    export_c_func!(dispatch_group_notify_f(_, _, _, _)),
    // semaphore
    export_c_func!(dispatch_semaphore_create(_)),
    export_c_func!(dispatch_semaphore_wait(_, _)),
    export_c_func!(dispatch_semaphore_signal(_)),
    // source
    export_c_func!(dispatch_source_create(_, _, _, _)),
    export_c_func!(dispatch_source_set_event_handler(_, _)),
    export_c_func!(dispatch_source_set_event_handler_f(_, _)),
    export_c_func!(dispatch_source_set_cancel_handler(_, _)),
    export_c_func!(dispatch_source_set_cancel_handler_f(_, _)),
    export_c_func!(dispatch_source_cancel(_)),
    export_c_func!(dispatch_source_testcancel(_)),
    export_c_func!(dispatch_source_get_handle(_)),
    export_c_func!(dispatch_source_get_mask(_)),
    export_c_func!(dispatch_source_get_data(_)),
    export_c_func!(dispatch_source_merge_data(_, _)),
    export_c_func!(dispatch_source_set_timer(_, _, _, _)),
    // resume / suspend
    export_c_func!(dispatch_resume(_)),
    export_c_func!(dispatch_suspend(_)),
    // barrier
    export_c_func!(dispatch_barrier_async(_, _)),
    export_c_func!(dispatch_barrier_sync(_, _)),
    export_c_func!(dispatch_barrier_async_f(_, _, _)),
    export_c_func!(dispatch_barrier_sync_f(_, _, _)),
    // context
    export_c_func!(dispatch_set_context(_, _)),
    export_c_func!(dispatch_get_context(_)),
    export_c_func!(dispatch_queue_set_specific(_, _, _, _)),
    export_c_func!(dispatch_queue_get_specific(_, _)),
    export_c_func!(dispatch_get_specific(_)),
    export_c_func!(dispatch_set_finalizer_f(_, _)),
    export_c_func!(dispatch_set_target_queue(_, _)),
    // main
    export_c_func!(dispatch_main()),
    // debug
    export_c_func!(dispatch_debug(_, _)),
];
