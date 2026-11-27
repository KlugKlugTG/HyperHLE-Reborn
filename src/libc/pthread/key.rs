/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Thread-specific data keys.
//!
//! Values live in each thread's guest thread-specific data (TSD) array.
//! TPIDRURO points at that array, so inline reads and writes made by Apple
//! frameworks agree with calls to `pthread_getspecific` and
//! `pthread_setspecific`.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::{EAGAIN, EINVAL};
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::{Environment, ThreadId};

pub struct State {
    /// Allocated dynamic keys and initialized static keys, with destructors.
    /// A NULL destructor still marks a key as allocated.
    keys: [Option<GuestFunction>; TSD_SLOTS as usize],
    /// Each thread's guest TSD array, see the module documentation.
    tsd_arrays: Vec<Option<MutPtr<MutVoidPtr>>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            keys: [None; TSD_SLOTS as usize],
            tsd_arrays: Vec::new(),
        }
    }
}

// Keys and thread IDs are small dense indices. Direct indexing avoids hashing
// in pthread_*specific and at every JIT thread-context transition.

// iOS reserves slots 0..=9 for libsyscall, 10..=255 for frameworks, and
// 256..=511 for pthread_key_create. macOS has a larger dynamic range.
// https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/os/tsd.h
// https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/internal.h
const STATIC_KEY_START: u32 = 10;
const DYNAMIC_KEY_START: u32 = 256;
const TSD_SLOTS: u32 = 512;

/// Get the address of a thread's guest TSD array (allocating it if needed),
/// which is the value of the TPIDRURO register while that thread is running.
pub fn tsd_array_for_thread(env: &mut Environment, thread: ThreadId) -> MutPtr<MutVoidPtr> {
    if let Some(&array) = get_state(env)
        .tsd_arrays
        .get(thread)
        .and_then(Option::as_ref)
    {
        return array;
    }
    // One private byte after the visible array for each key. A set byte
    // means that the host validated a write to this key on this thread.
    let array: MutPtr<MutVoidPtr> = env.mem.calloc(TSD_SLOTS * (4 + 1)).cast();
    assert!(!array.is_null(), "Could not allocate thread-specific data");
    let arrays = &mut get_state(env).tsd_arrays;
    if arrays.len() <= thread {
        arrays.resize(thread + 1, None);
    }
    arrays[thread] = Some(array);
    array
}

fn validated_slot(array: MutPtr<MutVoidPtr>, key: pthread_key_t) -> MutPtr<u8> {
    (array + TSD_SLOTS).cast::<u8>() + key
}

/// ARM entry sequences for the exported functions. Both fall through to the
/// ordinary SVC when host state is needed. Only the getter reads reserved
/// slots. The setter must validate even NULL values on the first set, which
/// can establish a static key's destructor. Deletion clears its flag on all
/// threads before the key can be reused.
/// https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
pub(crate) fn guest_function_prefix(symbol: &str) -> &'static [u32] {
    const _: () = assert!(TSD_SLOTS == 512);
    match symbol {
        "_pthread_getspecific" => &[
            0xe3500c02, // cmp r0, #512
            0x2a000003, // bhs slow
            0xee1d1f70, // mrc p15, 0, r1, c13, c0, 3 (TPIDRURO)
            0xe3510000, // cmp r1, #0
            0x17910100, // ldrne r0, [r1, r0, lsl #2]
            0x112fff1e, // bxne lr
        ],
        "_pthread_setspecific" => &[
            0xe3500c02, // cmp r0, #512
            0x2a000008, // bhs slow
            0xee1d2f70, // mrc p15, 0, r2, c13, c0, 3 (TPIDRURO)
            0xe3520000, // cmp r2, #0
            0x0a000005, // beq slow
            0xe2823b02, // add r3, r2, #2048 (validation bytes)
            0xe7d33000, // ldrb r3, [r3, r0]
            0xe3530000, // cmp r3, #0
            0x17821100, // strne r1, [r2, r0, lsl #2]
            0x13a00000, // movne r0, #0
            0x112fff1e, // bxne lr
        ],
        _ => &[],
    }
}

fn get_state(env: &mut Environment) -> &mut State {
    &mut env.libc_state.pthread.key
}

type pthread_key_t = u32;

fn pthread_key_create(
    env: &mut Environment,
    key_ptr: MutPtr<pthread_key_t>,
    destructor: GuestFunction, // void (*destructor)(void *), may be NULL
) -> i32 {
    let state = get_state(env);
    for key in DYNAMIC_KEY_START..TSD_SLOTS {
        if state.keys[key as usize].is_none() {
            state.keys[key as usize] = Some(destructor);
            env.mem.write(key_ptr, key);
            return 0;
        }
    }
    EAGAIN
}

/// Initialize a framework's fixed key, preserving its existing destructor.
/// https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
fn pthread_key_init_np(env: &mut Environment, key: i32, destructor: GuestFunction) -> i32 {
    if !(STATIC_KEY_START as i32..DYNAMIC_KEY_START as i32).contains(&key) {
        return EINVAL;
    }
    get_state(env).keys[key as usize].get_or_insert(destructor);
    0
}

fn pthread_getspecific(env: &mut Environment, key: pthread_key_t) -> MutVoidPtr {
    // Apple's implementation reads TSD directly, including writes from inlined
    // _pthread_setspecific_direct. Invalid keys have undefined behavior.
    // https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
    if key >= TSD_SLOTS {
        return Ptr::null();
    }
    let current_thread = env.current_thread;
    let Some(&array) = get_state(env)
        .tsd_arrays
        .get(current_thread)
        .and_then(Option::as_ref)
    else {
        return Ptr::null();
    };
    env.mem.read(array + key)
}

fn pthread_setspecific(env: &mut Environment, key: pthread_key_t, value: ConstVoidPtr) -> i32 {
    // Static keys can be used before initialization; dynamic keys must have
    // been allocated. The first static set installs a NULL destructor.
    // https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
    if !(STATIC_KEY_START..TSD_SLOTS).contains(&key) {
        return EINVAL;
    }
    if key < DYNAMIC_KEY_START {
        get_state(env).keys[key as usize].get_or_insert(GuestFunction::null_ptr());
    } else if get_state(env).keys[key as usize].is_none() {
        return EINVAL;
    }
    let current_thread = env.current_thread;
    let array = tsd_array_for_thread(env, current_thread);
    env.mem.write(array + key, value.cast_mut());
    env.mem.write(validated_slot(array, key), 1);
    0
}

fn pthread_key_delete(env: &mut Environment, key: pthread_key_t) -> i32 {
    // Deletion only accepts allocated dynamic keys. It clears values in every
    // thread and releases the key without calling its destructor.
    // https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
    if !(DYNAMIC_KEY_START..TSD_SLOTS).contains(&key)
        || get_state(env).keys[key as usize].take().is_none()
    {
        return EINVAL;
    }
    for &array in env.libc_state.pthread.key.tsd_arrays.iter().flatten() {
        env.mem.write(array + key, Ptr::null());
        env.mem.write(validated_slot(array, key), 0);
    }
    0
}

/// Run thread-exit destructors while the guest stack and TSD remain available.
/// iOS clears the slot before calling its destructor, makes four passes over
/// dynamic then static keys, and destroys the Objective-C GC keys last.
/// https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
pub(crate) fn cleanup_thread_tsd(env: &mut Environment) {
    let current_thread = env.current_thread;
    let Some(&array) = get_state(env)
        .tsd_arrays
        .get(current_thread)
        .and_then(Option::as_ref)
    else {
        return;
    };
    for _ in 0..4 {
        for key in (DYNAMIC_KEY_START..TSD_SLOTS)
            .chain((STATIC_KEY_START..DYNAMIC_KEY_START).filter(|key| !(110..120).contains(key)))
        {
            cleanup_key(env, array, key);
            if env.is_guest_termination_requested() {
                return;
            }
        }
    }
    for _ in 0..4 {
        for key in 110..120 {
            cleanup_key(env, array, key);
            if env.is_guest_termination_requested() {
                return;
            }
        }
    }
}

fn cleanup_key(env: &mut Environment, array: MutPtr<MutVoidPtr>, key: pthread_key_t) {
    let Some(&destructor) = get_state(env).keys[key as usize].as_ref() else {
        return;
    };
    let value: MutVoidPtr = env.mem.read(array + key);
    if value.is_null() {
        return;
    }
    env.mem.write(array + key, Ptr::null());
    if !destructor.to_ptr().is_null() {
        let _: () = destructor.call_from_host(env, (value,));
    }
}

/// Reclaim TSD after the thread's coroutine and final exit handlers have ended.
pub(crate) fn release_thread_tsd(env: &mut Environment, thread: ThreadId) {
    if let Some(array) = get_state(env)
        .tsd_arrays
        .get_mut(thread)
        .and_then(Option::take)
    {
        env.mem.free(array.cast());
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(pthread_key_create(_, _)),
    export_c_func!(pthread_key_init_np(_, _)),
    export_c_func!(pthread_getspecific(_)),
    export_c_func!(pthread_setspecific(_, _)),
    export_c_func!(pthread_key_delete(_)),
];
