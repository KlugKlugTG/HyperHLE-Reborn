/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `signal.h` — signal dispositions and synchronous signal delivery.
//!
//! touchHLE does not run guest signal handlers on the host. Instead a
//! "signal" is delivered synchronously, on the guest thread that raised
//! it, exactly like a real `raise()`/`pthread_kill()` call does for the
//! calling thread. That is enough for the uses early iOS apps actually
//! have: crash reporters, Unity's unhandled-exception shim, `abort()`
//! implementations, and games that install a handler for `SIGSEGV` or
//! `SIGABRT` and then deliberately `raise()` it.
//!
//! What is *not* modelled (and is documented as such below):
//!
//! * `sigprocmask`/`pthread_sigmask` — signal masks are accepted but
//!   never block delivery, because delivery is synchronous.
//! * `SA_SIGINFO` — handlers receive a synthesized `siginfo_t` and
//!   `ucontext_t` built from the current guest CPU state, not the real
//!   kernel-captured fault context.
//! * Asynchronous signals from outside the process (`kill()` from
//!   another process, timers, faults) — there is no other process.
//!
//! Reference: Apple's `sys/signal.h` and `signal(3)` man pages.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::libc::errno::{set_errno, EINTR, EINVAL};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::Environment;
use std::collections::HashMap;

/// `SIG_DFL`: the default disposition.
const SIG_DFL: u32 = 0;
/// `SIG_IGN`: the signal is ignored.
const SIG_IGN: u32 = 1;
/// `SIG_ERR`: `(void (*)(int))-1`, returned by `signal()` on failure.
const SIG_ERR: u32 = u32::MAX;

// Signal numbers shared by Darwin/iOS (`sys/signal.h`).
const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGQUIT: i32 = 3;
const SIGILL: i32 = 4;
const SIGTRAP: i32 = 5;
pub(crate) const SIGABRT: i32 = 6;
const SIGEMT: i32 = 7;
const SIGFPE: i32 = 8;
const SIGKILL: i32 = 9;
const SIGBUS: i32 = 10;
const SIGSEGV: i32 = 11;
const SIGSYS: i32 = 12;
const SIGPIPE: i32 = 13;
const SIGALRM: i32 = 14;
const SIGTERM: i32 = 15;
const SIGURG: i32 = 16;
const SIGSTOP: i32 = 17;
const SIGTSTP: i32 = 18;
const SIGCONT: i32 = 19;
const SIGCHLD: i32 = 20;
const SIGTTIN: i32 = 21;
const SIGTTOU: i32 = 22;
const SIGIO: i32 = 23;
const SIGXCPU: i32 = 24;
const SIGXFSZ: i32 = 25;
const SIGVTALRM: i32 = 26;
const SIGPROF: i32 = 27;
const SIGWINCH: i32 = 28;
const SIGINFO: i32 = 29;
const SIGUSR1: i32 = 30;
const SIGUSR2: i32 = 31;
/// `NSIG` on Apple platforms (the `_sys_siglist` table has this size).
const NSIG: i32 = 32;

// `sa_flags` bits used below (Darwin's `sys/signal.h`).
/// Restart syscalls interrupted by the signal. Stored, but touchHLE has
/// no restartable guest syscalls, so it has no observable effect.
const SA_RESTART: i32 = 0x0002;
/// Reset the disposition to `SIG_DFL` before running the handler.
const SA_RESETHAND: i32 = 0x0004;
/// The handler expects `(int, siginfo_t *, void *)` arguments.
const SA_SIGINFO: i32 = 0x0040;

/// A per-signal disposition, equivalent to Darwin's `struct sigaction`.
///
/// On 32-bit ARM/iOS the guest-visible layout is three words:
/// `__sigaction_u` (handler or `SIG_DFL`/`SIG_IGN`), `sa_mask`
/// (`sigset_t`, i.e. `uint32_t`) and `sa_flags`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct SignalAction {
    /// `SIG_DFL`, `SIG_IGN`, or the address of a guest handler.
    handler: u32,
    /// `sa_mask`; recorded for `sigaction()` round-trips only.
    mask: u32,
    /// `sa_flags`; only `SA_RESETHAND` and `SA_SIGINFO` are observed.
    flags: i32,
}

/// Per-process signal state.
#[derive(Default)]
pub struct State {
    /// Dispositions set via `signal()` or `sigaction()`. Signals that
    /// were never touched have no entry and behave like `SIG_DFL`.
    actions: HashMap<i32, SignalAction>,
    /// Signals whose handler is running right now. The kernel blocks a
    /// signal while its own handler executes, so a nested `raise()` of
    /// the same signal must not re-enter the handler (it would recurse
    /// forever); re-raising it performs the default action instead.
    handling: Vec<i32>,
    /// Whether a hardware-fault signal was delivered to a guest handler.
    pub(crate) fatal_delivered: bool,
}

// Guest-visible sizes from Darwin's `sys/signal.h` and `mach/arm/_structs.h`
// for 32-bit ARM. Only the fields crash reporters actually read are filled in;
// the rest is zeroed, which is a valid `siginfo_t`/`ucontext_t` as far as the
// guest is concerned.
/// `siginfo_t`: `si_signo`, `si_errno`, `si_code`, `si_pid`, `si_uid`,
/// `si_status`, `si_addr`, `si_value`, `si_band`, `__pad[7]`.
const SIGINFO_SIZE: u32 = 64;
const SIGINFO_SI_SIGNO: u32 = 0;
const SIGINFO_SI_CODE: u32 = 8;
const SIGINFO_SI_ADDR: u32 = 24;
/// `ucontext_t`: `uc_onstack`, `uc_sigmask`, `uc_stack`, `uc_link`,
/// `uc_mcsize`, `uc_mcontext`.
const UCONTEXT_SIZE: u32 = 32;
const UCONTEXT_UC_MCSIZE: u32 = 24;
const UCONTEXT_UC_MCONTEXT: u32 = 28;
/// `_STRUCT_MCONTEXT`: ARM exception state (3 words), thread state
/// (`r[13]`, `sp`, `lr`, `pc`, `cpsr` = 17 words) and VFP state (65 words).
const MCONTEXT_SIZE: u32 = (3 + 17 + 65) * 4;
const MCONTEXT_THREAD_STATE: u32 = 3 * 4;

/// Allocate and populate the `siginfo_t` and `ucontext_t` an `SA_SIGINFO`
/// handler is entitled to.
///
/// Crash reporters (Unity's shim, PLCrashReporter, Crashlytics, ...) read
/// `si_signo`/`si_addr` and walk `ucontext->uc_mcontext->__ss` to produce a
/// backtrace. Handing them garbage or NULL registers turns a survivable
/// delivery into a nil-page fault inside the handler.
fn alloc_siginfo_context(env: &mut Environment, signum: i32) -> (MutVoidPtr, MutVoidPtr) {
    let siginfo: MutVoidPtr = env.mem.alloc(SIGINFO_SIZE);
    env.mem.bytes_at_mut(siginfo.cast(), SIGINFO_SIZE).fill(0);
    let siginfo_words: MutPtr<u32> = siginfo.cast();
    env.mem
        .write(siginfo_words + SIGINFO_SI_SIGNO / 4, signum as u32);
    // si_code 0 == SI_USER: the signal came from `raise()`, which is exactly
    // how touchHLE delivers every signal.
    env.mem.write(siginfo_words + SIGINFO_SI_CODE / 4, 0u32);
    // For a fault signal, the faulting address is the best approximation we
    // have: the PC the guest was executing when it raised.
    let regs = env.cpu.regs();
    let fault_addr = if is_fault_signal(signum) {
        regs[crate::cpu::Cpu::PC]
    } else {
        0
    };
    env.mem
        .write(siginfo_words + SIGINFO_SI_ADDR / 4, fault_addr);

    let mcontext: MutVoidPtr = env.mem.alloc(MCONTEXT_SIZE);
    env.mem.bytes_at_mut(mcontext.cast(), MCONTEXT_SIZE).fill(0);
    let mcontext_words: MutPtr<u32> = mcontext.cast();
    // __ss: r0-r12, sp, lr, pc, cpsr, in that order.
    for i in 0..13 {
        env.mem
            .write(mcontext_words + MCONTEXT_THREAD_STATE / 4 + i, regs[i as usize]);
    }
    let thread_state = mcontext_words + MCONTEXT_THREAD_STATE / 4;
    env.mem.write(thread_state + 13, regs[crate::cpu::Cpu::SP]);
    env.mem.write(thread_state + 14, regs[crate::cpu::Cpu::LR]);
    env.mem.write(thread_state + 15, regs[crate::cpu::Cpu::PC]);
    env.mem.write(thread_state + 16, env.cpu.cpsr());

    let ucontext: MutVoidPtr = env.mem.alloc(UCONTEXT_SIZE);
    env.mem.bytes_at_mut(ucontext.cast(), UCONTEXT_SIZE).fill(0);
    let ucontext_words: MutPtr<u32> = ucontext.cast();
    env.mem
        .write(ucontext_words + UCONTEXT_UC_MCSIZE / 4, MCONTEXT_SIZE);
    env.mem.write(
        ucontext_words + UCONTEXT_UC_MCONTEXT / 4,
        mcontext.to_bits(),
    );

    (siginfo, ucontext)
}

/// Release what [alloc_siginfo_context] allocated, once the handler returned.
fn free_siginfo_context(env: &mut Environment, siginfo: MutVoidPtr, ucontext: MutVoidPtr) {
    let ucontext_words: MutPtr<u32> = ucontext.cast();
    let mcontext: u32 = env.mem.read(ucontext_words + UCONTEXT_UC_MCONTEXT / 4);
    env.mem.free(Ptr::from_bits(mcontext));
    env.mem.free(ucontext);
    env.mem.free(siginfo);
}

/// Whether `signum` is a hardware-fault signal whose Darwin default action
/// terminates the process.
fn is_fault_signal(signum: i32) -> bool {
    matches!(signum, SIGILL | SIGTRAP | SIGFPE | SIGBUS | SIGSEGV)
}

/// Whether `signum` names a real signal on this platform.
fn valid_signal(signum: i32) -> bool {
    signum > 0 && signum < NSIG
}

/// `SIGKILL` and `SIGSTOP` have no catchable or ignorable disposition.
fn uncatchable(signum: i32) -> bool {
    signum == SIGKILL || signum == SIGSTOP
}

/// The current disposition of `signum` (`SIG_DFL` when never set).
fn get_action(env: &Environment, signum: i32) -> SignalAction {
    env.libc_state
        .signal
        .actions
        .get(&signum)
        .copied()
        .unwrap_or_default()
}

/// Install `new` as the disposition of `signum`, returning the old one.
fn set_action(env: &mut Environment, signum: i32, new: SignalAction) -> SignalAction {
    env.libc_state
        .signal
        .actions
        .insert(signum, new)
        .unwrap_or_default()
}

fn sigaction(env: &mut Environment, signum: i32, act: ConstVoidPtr, old_act: MutVoidPtr) -> i32 {
    set_errno(env, 0);
    if !valid_signal(signum) || uncatchable(signum) {
        set_errno(env, EINVAL);
        return -1;
    }

    let old = get_action(env, signum);

    // POSIX fills in `old_act` with the previous disposition.
    if !old_act.is_null() {
        let out: MutPtr<u32> = old_act.cast();
        env.mem.write(out, old.handler);
        env.mem.write(out + 1, old.mask);
        env.mem.write(out + 2, old.flags as u32);
    }

    // A null `act` means "query only".
    if !act.is_null() {
        let src: ConstPtr<u32> = act.cast();
        let handler: u32 = env.mem.read(src);
        let mask: u32 = env.mem.read(src + 1);
        let flags: u32 = env.mem.read(src + 2);
        set_action(
            env,
            signum,
            SignalAction {
                handler,
                mask,
                flags: flags as i32,
            },
        );
    }

    0
}

fn signal(env: &mut Environment, signum: i32, handler: MutVoidPtr) -> MutVoidPtr {
    set_errno(env, 0);
    if !valid_signal(signum) || uncatchable(signum) {
        set_errno(env, EINVAL);
        return MutVoidPtr::from_bits(SIG_ERR);
    }

    // BSD `signal()` is `sigaction()` with `SA_RESTART` and a persistent
    // handler (it does *not* reset to `SIG_DFL` after the first signal).
    let new = SignalAction {
        handler: handler.to_bits(),
        mask: 0,
        flags: SA_RESTART,
    };
    let old = set_action(env, signum, new);
    log_dbg!(
        "signal({}, {:?}) => old handler {:#x}",
        signum,
        handler,
        old.handler
    );
    MutVoidPtr::from_bits(old.handler)
}

/// What [`raise_signal`] did with a signal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum RaiseOutcome {
    /// The signal was ignored (`SIG_IGN`, or an ignorable default).
    Ignored,
    /// A guest handler was called and returned to us.
    HandlerCalled,
    /// The default action was taken; execution was redirected and the
    /// caller must not continue as if nothing happened.
    DefaultActionPerformed,
}

/// The Darwin default action for `signum`, performed with the same
/// controlled guest-termination machinery that `exit()`/`abort()` use.
///
/// Fatal signals end the guest session through the return-to-host boundary.
/// They cannot be recovered by returning to a guest caller frame.
fn default_signal_action(env: &mut Environment, signum: i32) {
    let name = signal_name(signum);
    match signum {
        // Darwin ignores these by default.
        SIGURG | SIGCHLD | SIGIO | SIGWINCH | SIGINFO => {
            log_dbg!("Signal {} ({}) ignored (default action)", signum, name);
        }
        // Stopping would freeze the guest forever, so keep running.
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => {
            log!(
                "Warning: signal {} ({}) would stop the process; continuing.",
                signum,
                name
            );
        }
        // SIGCONT only resumes a stopped process; we never stop.
        SIGCONT => {
            log_dbg!("Signal {} ({}) ignored (process is not stopped)", signum, name);
        }
        // Legacy ad and analytics SDKs frequently omit SIG_IGN/SO_NOSIGPIPE
        // before writing to closed sockets; ignore SIGPIPE in GUI game runs.
        SIGPIPE if !env.options.headless => {
            log!(
                "Warning: ignoring default SIGPIPE so broken socket writes \
                 report EPIPE instead of ending the game."
            );
        }
        // The remaining signals terminate the process, with a core dump
        // for the fault-like ones.
        SIGHUP | SIGINT | SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGEMT | SIGFPE | SIGKILL
        | SIGBUS | SIGSEGV | SIGSYS | SIGPIPE | SIGALRM | SIGTERM | SIGXCPU | SIGXFSZ
        | SIGVTALRM | SIGPROF | SIGUSR1 | SIGUSR2 => {
            log!(
                "Guest received fatal signal {} ({}) via raise().",
                signum,
                name
            );
            crate::libc::stdlib::recover_or_end_guest_termination(
                env,
                &format!("raise({signum}) [signal {name}]"),
                crate::environment::GuestTermination::Signal(signum),
            );
        }
        _ => {
            log!(
                "Warning: unhandled default action for signal {} ({}); ignoring.",
                signum,
                name
            );
        }
    }
}

/// Deliver `signum` to the calling guest thread, as `raise()` does.
///
/// Returns what happened so that callers with extra obligations (notably
/// `abort()`, which must terminate even if a handler returned) can react.
pub(crate) fn raise_signal(env: &mut Environment, signum: i32) -> RaiseOutcome {
    let action = get_action(env, signum);
    match action.handler {
        SIG_DFL => {
            default_signal_action(env, signum);
            RaiseOutcome::DefaultActionPerformed
        }
        SIG_IGN => {
            log_dbg!("Signal {} ignored (SIG_IGN)", signum);
            RaiseOutcome::Ignored
        }
        handler => {
            if env.libc_state.signal.handling.contains(&signum) {
                // Signals are blocked while their own handler runs, so a
                // nested raise() of the same signal cannot re-enter the
                // handler. Real kernels would deliver it again after the
                // handler returned; ending the guest session now is the
                // safe approximation, and it is what the crash-reporting
                // idiom (`signal(sig, SIG_DFL); raise(sig);` inside the
                // handler) would do if the handler had reset first.
                log!(
                    "Warning: signal {} ({}) was raised from inside its own \
                     handler; performing the default action instead of \
                     re-entering the handler.",
                    signum,
                    signal_name(signum)
                );
                default_signal_action(env, signum);
                return RaiseOutcome::DefaultActionPerformed;
            }
            // A one-shot handler is reset *before* the guest code runs,
            // matching the kernel's ordering (the handler itself may
            // re-install a disposition).
            if action.flags & SA_RESETHAND != 0 {
                set_action(env, signum, SignalAction::default());
            }
            log_dbg!(
                "Calling guest signal handler {:#x} for signal {}",
                handler,
                signum
            );
            if matches!(signum, SIGILL | SIGBUS | SIGFPE | SIGSEGV | SIGSYS) {
                env.libc_state.signal.fatal_delivered = true;
            }
            env.libc_state.signal.handling.push(signum);
            let func = GuestFunction::from_addr_with_thumb_bit(handler);
            let siginfo_context = if action.flags & SA_SIGINFO != 0 {
                Some(alloc_siginfo_context(env, signum))
            } else {
                None
            };
            match siginfo_context {
                // `void (*)(int, siginfo_t *, void *)`. Passing nothing for
                // the second and third arguments leaves r1/r2 holding
                // whatever the previous guest code left there, and crash
                // reporters dereference both immediately.
                Some((siginfo, ucontext)) => {
                    let _: () = func.call_from_host(env, (signum, siginfo, ucontext));
                }
                None => {
                    let _: () = func.call_from_host(env, (signum,));
                }
            }
            if let Some((siginfo, ucontext)) = siginfo_context {
                free_siginfo_context(env, siginfo, ucontext);
            }
            // The handler usually returns; if it terminated the guest
            // session instead, this bookkeeping no longer matters.
            if let Some(position) = env
                .libc_state
                .signal
                .handling
                .iter()
                .rposition(|&signal| signal == signum)
            {
                env.libc_state.signal.handling.remove(position);
            }
            RaiseOutcome::HandlerCalled
        }
    }
}

/// `int raise(int sig)` — send `sig` to the calling thread.
///
/// Implemented on top of the disposition store above rather than as a
/// return-0 stub, so handlers installed with `signal()`/`sigaction()`
/// actually run. `errno` is set to `EINVAL` for an invalid signal.
pub(crate) fn raise(env: &mut Environment, signum: i32) -> i32 {
    set_errno(env, 0);
    if !valid_signal(signum) {
        set_errno(env, EINVAL);
        return -1;
    }
    let _ = raise_signal(env, signum);
    0
}

fn sigprocmask(env: &mut Environment, _how: i32, _set: ConstVoidPtr, _old_set: MutVoidPtr) -> i32 {
    // Signal delivery is synchronous, so blocking is not modelled; report
    // success so callers that block signals around critical sections work.
    set_errno(env, 0);
    0
}

fn sigaltstack(env: &mut Environment, _ss: ConstVoidPtr, _old_ss: MutVoidPtr) -> i32 {
    // We never run a handler on an alternate stack; accept and ignore.
    set_errno(env, 0);
    0
}

// `int pause(void)` — suspend until a signal arrives. There is no signal
// delivery thread, so report EINTR immediately rather than hanging the guest.
fn pause(env: &mut Environment) -> i32 {
    set_errno(env, EINTR);
    -1
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sigaction(_, _, _)),
    export_c_func!(signal(_, _)),
    export_c_func!(raise(_)),
    export_c_func!(sigprocmask(_, _, _)), // Строго 3 аргумента (how, set, old_set)
    export_c_func!(sigaltstack(_, _)),    // Строго 2 аргумента (ss, old_ss)
    export_c_func!(pause()),
];

/// Name of `signum` for logging, matching `_sys_siglist` below.
fn signal_name(signum: i32) -> &'static str {
    if signum >= 0 && (signum as usize) < SIG_NAMES.len() {
        SIG_NAMES[signum as usize]
    } else {
        "Unknown"
    }
}

/// `const char *const sys_siglist[NSIG]` — POSIX/BSD signal-name table.
/// On Apple platforms (`sys/signal.h`) the array has `NSIG` (= 32)
/// entries. Each slot is a pointer to a C string describing the
/// signal (`"Terminated"`, `"Segmentation fault"`, …). Per the
/// libc source `<sys/signal.c>`, the values are stable and exposed
/// as a non-lazy symbol.
///
/// We build the table at first reference time by allocating 32
/// guest-side `char *` slots, each pointing at the canonical name
/// from Apple's `sys/signal.h`.
const SIG_NAMES: [&str; 32] = [
    "Unknown",
    "Hangup",
    "Interrupt",
    "Quit",
    "Illegal instruction",
    "Trace/BPT trap",
    "Abort trap",
    "EMT trap",
    "Floating point exception",
    "Killed",
    "Bus error",
    "Segmentation fault",
    "Bad system call",
    "Broken pipe",
    "Alarm clock",
    "Terminated",
    "Urgent I/O condition",
    "Suspended (signal)",
    "Suspended",
    "Continued",
    "Child exited",
    "Stopped (tty input)",
    "Stopped (tty output)",
    "I/O possible",
    "Cputime limit exceeded",
    "Filesize limit exceeded",
    "Virtual timer expired",
    "Profiling timer expired",
    "Window size changes",
    "Information request",
    "User defined signal 1",
    "User defined signal 2",
];

pub const CONSTANTS: ConstantExports = &[(
    "_sys_siglist",
    HostConstant::Custom(|env| {
        let table: crate::mem::MutPtr<u32> = env.mem.alloc(32 * 4).cast();
        for (i, &name) in SIG_NAMES.iter().enumerate() {
            // Allocate a null-terminated C string in guest memory.
            let bytes = name.as_bytes();
            let s = env.mem.alloc((bytes.len() + 1) as u32);
            env.mem
                .bytes_at_mut(s.cast(), (bytes.len() + 1) as u32)
                .copy_from_slice(
                    &bytes
                        .iter()
                        .copied()
                        .chain(std::iter::once(0u8))
                        .collect::<Vec<_>>(),
                );
            env.mem.write(table + (i as u32), s.to_bits());
        }
        Ptr::from_bits(table.to_bits())
    }),
)];
