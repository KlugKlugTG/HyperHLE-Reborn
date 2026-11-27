/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach/semaphore.h`
//!
//! Implemented as a wrapper around libc semaphore

#![allow(non_camel_case_types)]

use std::time::Duration;

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::mach::arm::task::task_t;
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::thread_info::{kern_return_t, KERN_INVALID_ARGUMENT, KERN_SUCCESS};
use crate::libc::semaphore::{sem_destroy, sem_init, sem_post, sem_t, Signal, WaitResult};
use crate::mem::MutPtr;

// Opaque type. Reusing sem_t for convenience
// TODO: `semaphore_t` should be `mach_port_t`
type semaphore = sem_t;
type semaphore_t = MutPtr<semaphore>;

/// `KERN_OPERATION_TIMED_OUT` from `mach/kern_return.h`: what the timed waits
/// below return when the timeout expires before the semaphore is acquired.
const KERN_OPERATION_TIMED_OUT: kern_return_t = 49;

const KERN_TERMINATED: kern_return_t = 37;
const KERN_NOT_WAITING: kern_return_t = 48;

fn mach_wait_result(result: WaitResult) -> kern_return_t {
    match result {
        WaitResult::Acquired => KERN_SUCCESS,
        WaitResult::TimedOut => KERN_OPERATION_TIMED_OUT,
        WaitResult::Invalid => KERN_INVALID_ARGUMENT,
        WaitResult::Destroyed => KERN_TERMINATED,
    }
}

/// DIAG (TOUCHHLE_TRACE_MACHSEM=1): trace Mach semaphore traffic. FMOD drives
/// its async/bank-loader threads entirely through these, so a lost signal shows
/// up here as a wait with no matching signal.
fn machsem_trace(env: &Environment, op: &str, sem: semaphore_t) {
    if !crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        return;
    }
    let value = env
        .libc_state
        .semaphore
        .open_semaphores
        .get(&sem)
        .map(|rc| (**rc).borrow().value);
    let waiters = env
        .libc_state
        .semaphore
        .open_semaphores
        .get(&sem)
        .map(|rc| (**rc).borrow().waiting.len());
    log!(
        "MACHSEM t={} {} sem={:?} value={:?} waiters={:?} lr={:#x}",
        env.current_thread,
        op,
        sem,
        value,
        waiters,
        env.cpu.regs()[14]
    );
}

fn semaphore_create(
    env: &mut Environment,
    task: task_t,
    semaphore: MutPtr<semaphore_t>,
    policy: i32,
    value: i32,
) -> kern_return_t {
    assert_eq!(task, MACH_TASK_SELF);
    assert_eq!(policy, 0);

    let open_semaphore: semaphore_t = env.mem.alloc_and_write(0);
    let res = sem_init(env, open_semaphore, 0, value.try_into().unwrap());
    assert_eq!(res, 0);

    env.mem.write(semaphore, open_semaphore);
    let caller_lr = env.cpu.regs()[14];
    machsem_trace(env, "create", open_semaphore);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        // Log the creator's LR so we can trace which code path owns each semaphore
        log!("  MACHSEM-CREATE-BT lr={:#x}", caller_lr);
    }
    let result = KERN_SUCCESS;
    log_dbg!(
        "semaphore_create({:?}, {:?}, {:?}, {:?}) -> {:?}",
        task,
        semaphore,
        policy,
        value,
        result
    );
    result
}

fn semaphore_signal(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    // Mirror `sem_post`: a valid semaphore signals successfully (KERN_SUCCESS),
    // an invalid handle maps to KERN_INVALID_ARGUMENT rather than aborting the
    // process. (Mach's `semaphore_signal` returns KERN_INVALID_ARGUMENT for a
    // bad semaphore port.)
    let caller_lr = env.cpu.regs()[14];
    machsem_trace(env, "signal", semaphore);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        // Also log a symbolized backtrace for every signal, so we can trace
        // which code path is (or isn't) waking the worker.
        log!("  MACHSEM-BT signal lr={:#x}", caller_lr);
    }
    let result = if sem_post(env, semaphore) == 0 {
        KERN_SUCCESS
    } else {
        KERN_INVALID_ARGUMENT
    };
    log_dbg!("semaphore_signal({:?}) -> {:?}", semaphore, result);
    result
}

/// `kern_return_t semaphore_signal_all(semaphore_t semaphore)`
///
/// XNU wakes all current waiters, or preposts one unit if nobody is waiting.
fn semaphore_signal_all(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    let Some(object) = env.libc_state.semaphore.open_semaphores.get(&semaphore) else {
        return KERN_INVALID_ARGUMENT;
    };
    object.borrow_mut().signal(&env.guest_clock, Signal::All);
    KERN_SUCCESS
}

/// A null thread port selects any waiter. A targeted signal never preposts.
/// https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/sync_sema.c
fn semaphore_signal_thread(
    env: &mut Environment,
    semaphore: semaphore_t,
    thread: u32,
) -> kern_return_t {
    let Some(object) = env.libc_state.semaphore.open_semaphores.get(&semaphore) else {
        return KERN_INVALID_ARGUMENT;
    };
    let target = if thread == 0 {
        None
    } else {
        let Some(id) = crate::libc::pthread::thread::thread_id_from_mach_thread_port(
            thread,
            env.threads.len(),
        ) else {
            return KERN_INVALID_ARGUMENT;
        };
        Some(id)
    };
    if object
        .borrow_mut()
        .signal(&env.guest_clock, Signal::Thread(target))
    {
        KERN_SUCCESS
    } else {
        KERN_NOT_WAITING
    }
}

// Opt-in deadlock-breaker for the otherwise-infinite `semaphore_wait`,
// enabled with `--mach-semaphore-spurious-wake=<ms>`.
//
// FMOD's worker threads (its AsyncManager command pump and the Studio
// BankLoader queue) park on a Mach semaphore that the producer is supposed to
// signal when it posts work. In Minecraft: Story Mode
// (com.telltalegames.MC100) that producer-side signal never reaches the
// workers under our cooperative
// scheduler, so `Studio::System::loadBankFile` blocks forever in
// `system_checkBlockingBank` during `SoundSystem::Initialize` and the game
// never renders a second frame. Those two FMOD loops re-check their own
// queue state on wake, so for that app a spurious wake is harmless.
//
// This must NOT be on by default. Returning KERN_SUCCESS without having
// acquired the semaphore tells the waiter that a resource is ready when it
// is not. Mono/Unity build their thread synchronisation on Mach semaphores,
// so a lying wait lets a worker touch half-initialized state; the failure
// then appears far away as a managed `NullReferenceException` followed by
// Unity's `trap`/`exit(1)`, with nothing in the log pointing back here.
// (Observed in Turbo Dismount after this breaker was first added
// unconditionally.)
fn semaphore_wait(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    machsem_trace(env, "wait-enter", semaphore);
    if !is_known_semaphore(env, semaphore) {
        log!(
            "Warning: semaphore_wait({:?}) called with an unknown semaphore; \
             returning KERN_INVALID_ARGUMENT.",
            semaphore
        );
        return KERN_INVALID_ARGUMENT;
    }
    // By default this is a true blocking wait: it returns only once a real
    // signal arrives. The spurious-wake deadlock-breaker is opt-in per app
    // (see the doc comment above `--mach-semaphore-spurious-wake=`).
    //
    // IMPORTANT: apply the breaker ONLY to the main thread (thread 0). The
    // blanket version spuriously wakes the FMOD/Wwise *worker* threads, which
    // then loop on an empty command queue and leak memory until OOM (observed
    // in Deus Ex: The Fall: host RSS climbs to 5+ GB). Waking only the main
    // thread lets it time out of a blocking audio call (e.g. a FaceFX dialogue
    // waiting on VO whose Wwise .wem could not be decoded) and continue, while
    // the idle audio workers keep blocking normally and never spin/leak.
    let spurious_wake_after = env
        .options
        .mach_semaphore_spurious_wake_ms
        .filter(|_| env.current_thread == 0)
        .map(Duration::from_millis);
    let deadline = spurious_wake_after.map(|after| env.guest_clock.now() + after);
    let result = match env.wait_semaphore(semaphore, deadline, None) {
        WaitResult::TimedOut => {
            log!("Warning: semaphore_wait({:?}) on thread {} reporting a spurious KERN_SUCCESS (--mach-semaphore-spurious-wake).", semaphore, env.current_thread);
            KERN_SUCCESS
        }
        result => mach_wait_result(result),
    };
    if crate::env_flag_cached!("TOUCHHLE_TRACE_MACHSEM") {
        let ret_lr = env.cpu.regs()[14];
        let ret_pc = env.cpu.regs()[15];
        log!(
            "MACHSEM t={} wait-exit sem={:?} result={:?} returning-to-lr={:#x} pc={:#x}",
            env.current_thread,
            semaphore,
            result,
            ret_lr,
            ret_pc
        );
    }
    log_dbg!("semaphore_wait({:?}) -> {:?}", semaphore, result);
    result
}

/// `kern_return_t semaphore_trywait(semaphore_t semaphore)`
///
/// Non-blocking acquisition: KERN_SUCCESS if a unit was available, otherwise
/// KERN_OPERATION_TIMED_OUT (per `mach/semaphore.h`, which documents that a
/// failed try-wait reports the same code as a timeout).
fn semaphore_trywait(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    if !is_known_semaphore(env, semaphore) {
        log!(
            "Warning: semaphore_trywait({:?}) called with an unknown semaphore; \
             returning KERN_INVALID_ARGUMENT.",
            semaphore
        );
        return KERN_INVALID_ARGUMENT;
    }
    let result = if env.sem_decrement(semaphore, false) {
        KERN_SUCCESS
    } else {
        KERN_OPERATION_TIMED_OUT
    };
    log_dbg!("semaphore_trywait({:?}) -> {:?}", semaphore, result);
    result
}

/// Is `semaphore` a live (not destroyed, not never-created) semaphore handle?
/// Checked before attempting a decrement so `sem_decrement` doesn't spam its
/// "unknown semaphore" warning for handles we already know are bogus.
fn is_known_semaphore(env: &Environment, semaphore: semaphore_t) -> bool {
    env.libc_state
        .semaphore
        .open_semaphores
        .contains_key(&semaphore)
}

/// `kern_return_t semaphore_timedwait(semaphore_t semaphore, mach_timespec_t wait_time)`
///
/// Blocking wait with a **relative** timeout. `mach_timespec_t` is
/// `{ unsigned int tv_sec; clock_res_t tv_nsec; }`; being an 8-byte composite
/// with 4-byte alignment it arrives in two consecutive registers per AAPCS32
/// (r1/r2 after the semaphore port in r0), so it is declared here as two scalar
/// parameters — the host-function ABI has no by-value struct support, and
/// `mach/host.rs`'s `clock_get_time` passes the same struct by pointer for the
/// same reason.
///
/// This used to be missing entirely, which meant the dyld fallback installed a
/// generic "return 0" stub for it. That stub reports KERN_SUCCESS *without
/// blocking and without consuming a semaphore unit*, i.e. it tells the caller
/// "your event fired" every single time. Engines whose worker threads pace
/// themselves with timed waits — the streaming/decoding threads of audio
/// middleware such as FMOD Ex (Geometry Dash), which feed long music tracks
/// while one-shot samples are decoded up-front — then never synchronise with
/// their producer/consumer ring buffers: streamed audio stays silent even
/// though the output device itself works.
fn semaphore_timedwait(
    env: &mut Environment,
    semaphore: semaphore_t,
    tv_sec: u32,
    tv_nsec: u32,
) -> kern_return_t {
    timedwait(env, semaphore, None, tv_sec, tv_nsec)
}

fn timedwait(
    env: &mut Environment,
    semaphore: semaphore_t,
    signal: Option<semaphore_t>,
    tv_sec: u32,
    tv_nsec: u32,
) -> kern_return_t {
    if tv_nsec >= 1_000_000_000 {
        return KERN_INVALID_ARGUMENT;
    }
    let timeout = Duration::new(u64::from(tv_sec), tv_nsec);
    let deadline = env.guest_clock.now() + timeout;
    mach_wait_result(env.wait_semaphore(semaphore, Some(deadline), signal))
}

/// XNU enqueues the wait and signals the peer before blocking the caller.
fn semaphore_wait_signal(
    env: &mut Environment,
    wait: semaphore_t,
    signal: semaphore_t,
) -> kern_return_t {
    mach_wait_result(env.wait_semaphore(wait, None, Some(signal)))
}

fn semaphore_timedwait_signal(
    env: &mut Environment,
    wait: semaphore_t,
    signal: semaphore_t,
    tv_sec: u32,
    tv_nsec: u32,
) -> kern_return_t {
    timedwait(env, wait, Some(signal), tv_sec, tv_nsec)
}

// https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/man/semaphore_destroy.html
// The task port comes before the semaphore port. Treating r0 as the semaphore
// freed the task port instead of the FMOD semaphore passed in r1.
fn semaphore_destroy(env: &mut Environment, task: task_t, semaphore: semaphore_t) -> kern_return_t {
    if task != MACH_TASK_SELF || !is_known_semaphore(env, semaphore) {
        return KERN_INVALID_ARGUMENT;
    }
    let result = sem_destroy(env, semaphore);
    debug_assert_eq!(result, 0);
    env.mem.free(semaphore.cast());
    let result = KERN_SUCCESS;
    log_dbg!(
        "semaphore_destroy({:?}, {:?}) -> {:?}",
        task,
        semaphore,
        result
    );
    result
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(semaphore_create(_, _, _, _)),
    export_c_func!(semaphore_signal(_)),
    export_c_func!(semaphore_signal_all(_)),
    export_c_func!(semaphore_signal_thread(_, _)),
    export_c_func!(semaphore_wait(_)),
    export_c_func!(semaphore_trywait(_)),
    export_c_func!(semaphore_timedwait(_, _, _)),
    export_c_func!(semaphore_wait_signal(_, _)),
    export_c_func!(semaphore_timedwait_signal(_, _, _, _)),
    export_c_func!(semaphore_destroy(_, _)),
];
