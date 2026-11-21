/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Native crash diagnostics.
//!
//! touchHLE's own errors and guest errors are always logged, but a *host*
//! crash (SIGSEGV/SIGBUS/SIGILL from e.g. a JIT bug or a graphics driver)
//! kills the process silently, which makes bug reports undiagnosable ("it
//! just closes with no error"). These handlers write a one-line marker with
//! the fatal signal and faulting address to the same log file the user can
//! share, then restore the default disposition and re-raise so the platform's
//! own crash reporting (Android tombstones etc.) still works.

/// Append a message to the log file (and stderr). Safe to call from a panic
/// hook; uses file-level locking via try_lock so re-entrant panics don't
/// deadlock — on contention the message is dropped rather than deadlocked.
#[cfg(unix)]
fn raw_fd() -> i32 {
    imp::log_fd()
}

#[cfg(not(unix))]
fn raw_fd() -> i32 {
    -1
}

#[cfg(unix)]
fn write_size_t(n: usize) -> libc::size_t {
    n as libc::size_t
}
#[cfg(windows)]
fn write_size_t(n: usize) -> libc::c_uint {
    n as libc::c_uint
}

pub fn append_to_log(msg: &str) {
    use std::io::Write;
    // First try the normal locked path...
    if let Ok(mut log_file) = crate::log::get_log_file().try_lock() {
        let _ = log_file.write_all(msg.as_bytes());
        let _ = log_file.write_all(b"\n");
        let _ = log_file.flush();
    }
    // ...but if the mutex is held by some other thread (very likely right
    // before a crash, since logging is what the guest threads spend their
    // time on), fall back to a raw fd write. O_APPEND-style writes of whole
    // small lines are atomic enough for diagnostics.
    let fd = raw_fd();
    if fd >= 0 {
        let mut line = msg.as_bytes().to_vec();
        line.push(b'\n');
        let mut written = 0usize;
        while written < line.len() {
            let n = unsafe {
                libc::write(
                    fd,
                    line.as_ptr().add(written) as *const libc::c_void,
                    write_size_t(line.len() - written),
                )
            };
            if n <= 0 {
                break;
            }
            written += n as usize;
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = std::io::stderr().write_all(msg.as_bytes());
        let _ = std::io::stderr().write_all(b"\n");
    }
}

/// Install a Rust panic hook that mirrors panic messages into the touchHLE
/// log file. The default hook only writes to stderr/logcat, which users
/// rarely capture, so panics look like silent aborts (especially on Android,
/// where a panic unwinding out of a guest-thread coroutine ends in
/// SIGABRT — see the FATAL SIGNAL marker in the log).
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("<unnamed>");
            let msg = format!(
                "HyperHLE: PANIC in thread \"{}\" at {}: {}\n(panic is followed by unwinding; if this appears right before a FATAL SIGNAL line, the panic crossed a coroutine boundary and aborted the process)",
                thread_name,
                info.location()
                    .map(|l| format!("{}:{}", l.file(), l.line()))
                    .unwrap_or_else(|| "<unknown>".to_string()),
                info.payload()
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic payload".to_string()),
            );
            append_to_log(&msg);
        }));
    });
}

/// Symbol and argument registers of the most recent guest-to-host function
/// dispatch, as a one-line summary for native crash reports.
///
/// dyld records these just before every host call (see
/// `environment::LAST_HOST_CALL_*`), so when the process dies inside a host
/// function this names that function and the guest-supplied arguments, even
/// though the crash handler cannot unwind the guest's stack. Returns [None] if
/// no host function has been dispatched yet.
#[cfg(unix)]
pub(crate) fn last_host_call_summary() -> Option<String> {
    use std::sync::atomic::Ordering;
    // The name is stored as a (pointer, length) pair by two separate relaxed
    // stores, so the length is clamped and the bytes are UTF-8 validated. The
    // bytes come from dyld's static symbol table (or a name dyld deliberately
    // leaks), so they stay mapped for the whole process lifetime and a
    // concurrent dispatch cannot free them.
    const MAX_SYMBOL_LEN: usize = 512;
    let ptr = crate::environment::LAST_HOST_CALL_SYMBOL_PTR.load(Ordering::Relaxed);
    let len = crate::environment::LAST_HOST_CALL_SYMBOL_LEN.load(Ordering::Relaxed);
    if ptr == 0 || len == 0 {
        return None;
    }
    let symbol = if len > MAX_SYMBOL_LEN {
        "<over-long symbol>"
    } else {
        // SAFETY: see the comment above.
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        std::str::from_utf8(bytes).unwrap_or("<unparsable symbol>")
    };
    let mut regs = String::new();
    for (i, reg) in crate::environment::LAST_HOST_CALL_REGS.iter().enumerate() {
        regs.push_str(&format!(" r{}={:#x}", i, reg.load(Ordering::Relaxed)));
    }
    Some(format!(
        "last host call: {} (guest thread {}):{}\n",
        symbol,
        crate::environment::LAST_HOST_CALL_THREAD.load(Ordering::Relaxed),
        regs
    ))
}

/// Whether an `(PC, SP)` pair from the interrupted signal frame can be a real
/// AArch64 user-mode register state.
///
/// `interrupted_frame` decodes the signal handler's `ucontext_t` by field name,
/// and the layout of that structure differs between C libraries and versions;
/// when the read is wrong it produces values that cannot be registers at all
/// (a stack pointer of zero, or one that is not 16-byte aligned, which the
/// AAPCS forbids). Reporting those values as if they identified the crash site
/// sends readers chasing garbage, so they are called out instead.
#[cfg(unix)]
fn frame_is_plausible(pc: usize, sp: usize) -> bool {
    pc != 0 && sp != 0 && sp.is_multiple_of(16)
}

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

    /// Raw fd of the touchHLE log file, so the signal handler (which cannot
    /// safely use the `Mutex<File>` in `log::get_log_file()`) can append to it.
    static LOG_FD: AtomicI32 = AtomicI32::new(-1);

    /// Runtime-resolved `backtrace()` (bionic exposes it on Android 12+,
    /// glibc on desktop; resolving via dlsym avoids build-target cfg).
    static BACKTRACE_SYM: AtomicUsize = AtomicUsize::new(0);

    pub fn resolve_backtrace() {
        unsafe {
            let sym = libc::dlsym(
                libc::RTLD_DEFAULT,
                b"backtrace\0".as_ptr() as *const libc::c_char,
            );
            BACKTRACE_SYM.store(sym as usize, Ordering::SeqCst);
        }
    }

    /// The interrupted thread's `(PC, LR, SP)`, when the platform hands them
    /// to the signal handler (see `interrupted_frame`).
    pub type InterruptedFrame = Option<(usize, usize, usize)>;

    pub fn native_backtrace_lines(fault: InterruptedFrame) -> String {
        // bionic's backtrace() cannot unwind past the signal frame, so on
        // Android every frame it reports belongs to *this handler*. The
        // register state of the interrupted thread — which is what actually
        // identifies the crash — only comes from the `ucontext_t` the kernel
        // passed to the SA_SIGINFO handler.
        let mut lines = match fault {
            Some((pc, lr, sp)) => {
                let mut s = format!(
                    "faulting frame: PC {:#x}, LR {:#x}, SP {:#x}\n",
                    pc, lr, sp
                );
                if pc == 0 {
                    s.push_str(
                        "(PC is 0: control was transferred through a NULL \
                         function pointer, e.g. a bogus vtable/`extern` slot \
                         — this is not a data access)\n",
                    );
                }
                if !super::frame_is_plausible(pc, sp) {
                    s.push_str(
                        "(this register state cannot be real: a user-mode SP \
                         is never zero and always 16-byte aligned, so the \
                         signal frame was not decoded correctly on this build \
                         — treat PC and LR above as garbage)\n",
                    );
                }
                s.push_str(
                    "(the frames below are the crash handler's own; \
                     symbolize PC/LR against the maps entry that contains \
                     them)\n",
                );
                s
            }
            None => String::new(),
        };
        let fptr = BACKTRACE_SYM.load(Ordering::Relaxed);
        if fptr == 0 {
            lines.push_str("(native backtrace() not available on this device)\n");
            lines.push_str(&maps_for(&fault_addrs(fault)));
            return lines;
        }
        type BacktraceFn = unsafe extern "C" fn(*mut *mut libc::c_void, libc::c_int) -> libc::c_int;
        let bt: BacktraceFn = unsafe { std::mem::transmute(fptr) };
        let mut addrs = [std::ptr::null_mut::<libc::c_void>(); 64];
        let n = unsafe { bt(addrs.as_mut_ptr(), 64) };
        lines.push_str(&format!("native backtrace ({} frames):\n", n));
        for i in 0..n as usize {
            lines.push_str(&format!("  #{}: {:#x}\n", i, addrs[i] as usize));
        }
        let mut map_addrs = addrs[..n as usize].to_vec();
        map_addrs.extend_from_slice(&fault_addrs(fault));
        lines.push_str(&maps_for(&map_addrs));
        lines
    }

    /// The interrupted PC/LR as addresses, so `maps_for` attributes them to a
    /// loaded library (and reports the libtouchHLE.so load base needed to
    /// symbolize them).
    fn fault_addrs(fault: InterruptedFrame) -> Vec<*mut libc::c_void> {
        match fault {
            Some((pc, lr, _)) => vec![pc as *mut libc::c_void, lr as *mut libc::c_void],
            None => Vec::new(),
        }
    }

    /// Dump the /proc/self/maps lines whose ranges contain one of `addrs`,
    /// so each backtrace frame can be attributed to a loaded library.
    /// Only open/read/close are used — good enough in an abort handler.
    fn maps_for(addrs: &[*mut libc::c_void]) -> String {
        let path = b"/proc/self/maps\0";
        let fd = unsafe { libc::open(path.as_ptr() as *const libc::c_char, libc::O_RDONLY) };
        if fd < 0 {
            return String::new();
        }
        // /proc/self/maps on Android with a loaded GPU driver stack is large;
        // grow until EOF so late-loaded libs (GLES driver, openal) are present.
        let mut buf: Vec<u8> = vec![0u8; 1 << 16];
        let mut off = 0usize;
        loop {
            if off >= buf.len() {
                buf.resize(buf.len() * 2, 0);
            }
            let n = unsafe {
                libc::read(
                    fd,
                    buf.as_mut_ptr().add(off) as *mut libc::c_void,
                    buf.len() - off,
                )
            };
            if n <= 0 {
                break;
            }
            off += n as usize;
        }
        unsafe { libc::close(fd) };
        let text = String::from_utf8_lossy(&buf[..off]).into_owned();
        let mut out = String::from("relevant /proc/self/maps entries:\n");
        // Track the lowest mapping of libtouchHLE.so together with its file
        // offset, so native backtrace frames can be converted to ELF file
        // addresses for offline symbolization:
        //   file_vaddr = frame_addr - load_base, load_base = map_start - map_offset
        let mut touchhle_load_base: Option<(usize, usize)> = None;
        for line in text.lines() {
            // "start-end perms offset dev inode path"
            let mut it = line.splitn(2, ' ');
            if let Some(range) = it.next() {
                let mut parts = range.split('-');
                let (Some(start), Some(end)) = (parts.next(), parts.next()) else {
                    continue;
                };
                let (Ok(start), Ok(end)) = (
                    usize::from_str_radix(start, 16),
                    usize::from_str_radix(end, 16),
                ) else {
                    continue;
                };
                if line.contains("libtouchHLE.so") && touchhle_load_base.is_none() {
                    // Offset is the third field.
                    let offset = line
                        .split_whitespace()
                        .nth(2)
                        .and_then(|o| usize::from_str_radix(o, 16).ok())
                        .unwrap_or(0);
                    touchhle_load_base = Some((start.saturating_sub(offset), offset));
                }
                if addrs.iter().any(|a| {
                    let a = *a as usize;
                    a >= start && a < end
                }) {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if let Some((base, _)) = touchhle_load_base {
            out.push_str(&format!(
                "libtouchHLE.so load base: {:#x} (symbolize native frames with: llvm-symbolizer --obj=libtouchHLE.so <frame - base>)\n",
                base
            ));
        }
        out
    }

    pub fn log_fd() -> i32 {
        LOG_FD.load(Ordering::SeqCst)
    }

    /// Register the raw fd of the log file. Called after the log file is
    /// created; async-signal-safe `write(2)` then targets it.
    pub fn set_log_fd(fd: i32) {
        LOG_FD.store(fd, Ordering::SeqCst);
    }

    const NAME_SEGV: &[u8] = b"SIGSEGV\0";
    const NAME_BUS: &[u8] = b"SIGBUS\0";
    const NAME_ILL: &[u8] = b"SIGILL\0";
    const NAME_ABORT: &[u8] = b"SIGABRT\0";

    /// The register state of the interrupted thread, as `(PC, LR, SP)`.
    ///
    /// This is the only reliable way to learn *where* the host died: the
    /// guest PC ring tracks the emulated CPU, and `backtrace()` on bionic
    /// stops at the signal frame (so it returns this handler's own frames).
    #[cfg(all(target_os = "android", target_arch = "aarch64"))]
    fn interrupted_frame(uc: *mut libc::c_void) -> InterruptedFrame {
        if uc.is_null() {
            return None;
        }
        // The kernel/bionic arm64 `struct sigcontext`, which is what
        // `mcontext_t` is a typedef of. Read through this mirror rather than
        // through `libc::mcontext_t`'s own field names, which are not stable
        // across libc versions. The field order is the ABI and must be kept
        // even for the fields this handler does not report.
        #[repr(C)]
        #[allow(dead_code)]
        struct Aarch64Sigcontext {
            fault_address: u64,
            regs: [u64; 31],
            sp: u64,
            pc: u64,
            pstate: u64,
        }
        // SAFETY: the kernel passes a valid `ucontext_t` as the third
        // argument of a SA_SIGINFO handler, and `uc_mcontext` is this
        // architecture's `sigcontext`. Only reads are performed, and the
        // mirror covers a prefix of that (larger) kernel structure.
        let ctx = unsafe { &*(uc as *const libc::ucontext_t) };
        let sc = unsafe { &*(&ctx.uc_mcontext as *const _ as *const Aarch64Sigcontext) };
        // x30 is the link register on arm64.
        Some((sc.pc as usize, sc.regs[30] as usize, sc.sp as usize))
    }

    /// Platforms whose `ucontext_t` layout this handler does not know: no
    /// extra information, but the rest of the report is unchanged.
    #[cfg(not(all(target_os = "android", target_arch = "aarch64")))]
    fn interrupted_frame(_uc: *mut libc::c_void) -> InterruptedFrame {
        None
    }

    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, uc: *mut libc::c_void) {
        static IN_HANDLER: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if IN_HANDLER.swap(true, Ordering::SeqCst) {
            unsafe {
                let mut dfl: libc::sigaction = std::mem::zeroed();
                dfl.sa_sigaction = libc::SIG_DFL;
                libc::sigaction(sig, &dfl, std::ptr::null_mut());
                libc::raise(sig);
                libc::_exit(128 + sig);
            }
        }
        let name: &[u8] = match sig {
            libc::SIGSEGV => NAME_SEGV,
            libc::SIGBUS => NAME_BUS,
            libc::SIGILL => NAME_ILL,
            libc::SIGABRT => NAME_ABORT,
            _ => b"SIGNAL\0",
        };
        // si_addr is the faulting memory address for SIGSEGV/SIGBUS/SIGILL.
        let addr = unsafe {
            match sig {
                libc::SIGABRT => 0,
                _ => (*(info as *const libc::siginfo_t)).si_addr() as usize,
            }
        };
        let location = if sig == libc::SIGABRT {
            " (abort; no fault address)".to_string()
        } else {
            format!(" at address {:#x}", addr)
        };
        let msg = format!(
            "HyperHLE: FATAL: native host crash: {}{} — the signal alone does not identify the root cause. Check preceding panic, loader and guest-fault messages. The process will now terminate.\n",
            std::str::from_utf8(&name[..name.len() - 1]).unwrap_or("SIGNAL"),
            location
        );
        let msg = format!(
            "{}last guest PC: {:#x}, LR: {:#x}\n",
            msg,
            crate::environment::LAST_GUEST_PC.load(Ordering::Relaxed),
            crate::environment::LAST_GUEST_LR.load(Ordering::Relaxed)
        );
        // Identify the aborting OS thread and dump a NATIVE backtrace:
        // the guest PC ring only tracks the emulation thread, so if the
        // crash came from a worker (audio/net/JIT helper), this is the only
        // way to see where it actually died.
        let msg = format!(
            "{}aborting thread: name={:?} tid={}\n",
            msg,
            std::thread::current().name().unwrap_or("<unnamed>"),
            // libc::gettid is Linux-only; pthread_self works everywhere.
            unsafe { libc::pthread_self() as u64 }
        );
        // Name the host function the guest was inside. The values are
        // snapshotted before the host call runs, so if this crash is inside a
        // host stub (very common: a wild pointer built from guest-supplied
        // arguments) this identifies it without symbolizing the library.
        let msg = match super::last_host_call_summary() {
            Some(summary) => format!("{}{}", msg, summary),
            None => msg,
        };
        let msg = format!(
            "{}{}",
            msg,
            native_backtrace_lines(interrupted_frame(uc))
        );
        let msg = format!(
            "{}recent guest PCs:{}\n",
            msg,
            (0..32)
                .map(|i| {
                    let oldest_idx = crate::environment::GUEST_PC_RING_IDX
                        .load(Ordering::Relaxed)
                        % 32;
                    let idx = oldest_idx.wrapping_add(i) % 32;
                    format!(
                        " {:#x}",
                        crate::environment::GUEST_PC_RING[idx].load(Ordering::Relaxed)
                    )
                })
                .collect::<String>()
        );
        let bytes = msg.as_bytes();
        unsafe {
            // Best-effort write to both stderr and the log file. write(2) is
            // async-signal-safe.
            let _ = libc::write(2, bytes.as_ptr() as *const libc::c_void, bytes.len());
            let fd = LOG_FD.load(Ordering::SeqCst);
            if fd >= 0 {
                let _ = libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
            }
            // On Android, the most common cause of this abort is a JNI
            // fatal error raised by ART ("JNI DETECTED ERROR IN APPLICATION:
            // ..."), whose message only goes to logcat. Fork + exec
            // `logcat -d --pid=<ours>` to append our own log entries (the
            // JNI error text included) to the log file before dying.
            #[cfg(target_os = "android")]
            {
                let my_pid = libc::getpid();
                let child = libc::fork();
                if child == 0 {
                    // Child: redirect stdout/stderr into the log file and
                    // dump the logcat ring buffer for this process.
                    let lfd = LOG_FD.load(Ordering::SeqCst);
                    if lfd >= 0 {
                        libc::dup2(lfd, libc::STDOUT_FILENO);
                        libc::dup2(lfd, libc::STDERR_FILENO);
                    }
                    let pid_str = std::fmt::format(format_args!("{}", my_pid));
                    let mut pid_c = pid_str.into_bytes();
                    pid_c.push(0);
                    libc::execl(
                        b"/system/bin/logcat\0".as_ptr() as *const libc::c_char,
                        b"logcat\0".as_ptr() as *const libc::c_char,
                        b"-d\0".as_ptr() as *const libc::c_char,
                        b"--pid\0".as_ptr() as *const libc::c_char,
                        pid_c.as_ptr() as *const libc::c_char,
                        std::ptr::null::<libc::c_char>(),
                    );
                    libc::_exit(127);
                } else if child > 0 {
                    let mut status: libc::c_int = 0;
                    libc::waitpid(child, &mut status, 0);
                }
            }
            // Restore the default disposition and re-raise so the platform's
            // crash reporter (Android tombstone, core dumps) still sees it.
            let mut dfl: libc::sigaction = std::mem::zeroed();
            dfl.sa_sigaction = libc::SIG_DFL;
            libc::sigaction(sig, &dfl, std::ptr::null_mut());
            libc::raise(sig);
        }
    }

    /// Install the diagnostic handlers for the fatal native signals.
    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            resolve_backtrace();
            let mut act: libc::sigaction = unsafe { std::mem::zeroed() };
            act.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER;
            act.sa_sigaction = handler as usize;
            for &sig in &[libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGABRT] {
                unsafe {
                    libc::sigaction(sig, &act, std::ptr::null_mut());
                }
            }
        });
    }
}

#[cfg(unix)]
pub use imp::{install, native_backtrace_lines, set_log_fd};

#[cfg(not(unix))]
pub fn set_log_fd(_fd: i32) {}

#[cfg(all(not(unix), not(windows)))]
pub fn install() {}

/// Windows native crash diagnostics via a Vectored Exception Handler.
///
/// The Unix handler above is a POSIX-signal handler and does nothing on the
/// MSVC build, so a host access violation (e.g. a guest wild branch that lands
/// dynarmic on a bad host pointer) used to kill the process with no log at all
/// — the log just stopped mid-line. This VEH prints the faulting address and
/// the last guest PC/LR plus the recent guest-PC ring (both updated every CPU
/// batch in `Environment::run`, independent of OS) so the guest instruction
/// that triggered the fault can be identified, then returns
/// EXCEPTION_CONTINUE_SEARCH so the process still dies as before.
#[cfg(windows)]
mod win_imp {
    use std::ffi::c_void;
    use std::io::Write;
    use std::sync::atomic::Ordering;

    #[repr(C)]
    struct ExceptionRecord {
        exception_code: u32,
        exception_flags: u32,
        nested: *mut ExceptionRecord,
        exception_address: *mut c_void,
        number_parameters: u32,
        exception_information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        exception_record: *mut ExceptionRecord,
        context_record: *mut c_void,
    }

    const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
    const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xC000_001D;
    const EXCEPTION_STACK_OVERFLOW: u32 = 0xC000_00FD;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    extern "system" {
        fn AddVectoredExceptionHandler(
            first: u32,
            handler: extern "system" fn(*mut ExceptionPointers) -> i32,
        ) -> *mut c_void;
    }

    extern "system" fn handler(info: *mut ExceptionPointers) -> i32 {
        // Only report the fatal, non-recoverable exceptions; let everything
        // else (including the guard-page probes dynarmic may rely on) pass
        // straight through untouched.
        let rec = unsafe { &*(*info).exception_record };
        let code = rec.exception_code;
        let fatal = matches!(
            code,
            EXCEPTION_ACCESS_VIOLATION | EXCEPTION_ILLEGAL_INSTRUCTION | EXCEPTION_STACK_OVERFLOW
        );
        if !fatal {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let kind = match code {
            EXCEPTION_ACCESS_VIOLATION => "ACCESS_VIOLATION",
            EXCEPTION_ILLEGAL_INSTRUCTION => "ILLEGAL_INSTRUCTION",
            EXCEPTION_STACK_OVERFLOW => "STACK_OVERFLOW",
            _ => "FATAL",
        };
        let (access, fault_addr) = if code == EXCEPTION_ACCESS_VIOLATION {
            let kind = match rec.exception_information[0] {
                0 => "read",
                1 => "write",
                8 => "execute",
                _ => "?",
            };
            (kind, rec.exception_information[1])
        } else {
            ("", 0)
        };

        let mut msg = String::new();
        msg.push_str(&format!(
            "\nHyperHLE: FATAL host exception {} ({:#010x}) at host addr {:?}",
            kind, code, rec.exception_address
        ));
        if code == EXCEPTION_ACCESS_VIOLATION {
            msg.push_str(&format!(" — {} of guest/host addr {:#x}", access, fault_addr));
        }
        msg.push('\n');
        msg.push_str(&format!(
            "last guest PC: {:#x}, LR: {:#x}\n",
            crate::environment::LAST_GUEST_PC.load(Ordering::Relaxed),
            crate::environment::LAST_GUEST_LR.load(Ordering::Relaxed),
        ));
        let oldest = crate::environment::GUEST_PC_RING_IDX.load(Ordering::Relaxed) % 32;
        let mut ring = String::from("recent guest PCs:");
        for i in 0..32 {
            let idx = oldest.wrapping_add(i) % 32;
            ring.push_str(&format!(
                " {:#x}",
                crate::environment::GUEST_PC_RING[idx].load(Ordering::Relaxed)
            ));
        }
        msg.push_str(&ring);
        msg.push('\n');

        if let Ok(mut err) = std::io::stderr().lock().write_all(msg.as_bytes()).map(|_| ()) {
            let _ = &mut err;
        }
        let _ = std::io::stderr().flush();
        // Let the default handler run so the process still terminates (and any
        // attached debugger / WER still sees the original exception).
        EXCEPTION_CONTINUE_SEARCH
    }

    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // first = 0: run us LAST, after the memory subsystem's own
            // lazy-commit VEH (registered with first=1) has had its chance to
            // commit a reserved-but-uncommitted guest page and resume. We only
            // want to report faults that nobody could recover — i.e. the real
            // fatal crash — not the routine lazy-commit page faults.
            unsafe {
                AddVectoredExceptionHandler(0, handler);
            }
        });
    }
}

#[cfg(windows)]
pub use win_imp::install;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn implausible_frames_are_rejected() {
        // A user-mode AArch64 stack pointer is never zero and always 16-byte
        // aligned, so a frame that fails either test cannot be a real register
        // state (see the Republique crash report, which had SP = 0).
        assert!(frame_is_plausible(0x74b3_0000, 0x7fff_1230));
        assert!(!frame_is_plausible(0x74b3_0000, 0));
        assert!(!frame_is_plausible(0x74b3_0000, 0x7fff_1238));
        assert!(!frame_is_plausible(0, 0x7fff_1230));
    }

    // Both cases share the global LAST_HOST_CALL_* state, so they run in one
    // test: separate tests could interleave and read each other's values.
    #[test]
    fn last_host_call_summary_reports_symbol_and_registers() {
        static NAME: &str = "glDrawElements";
        crate::environment::LAST_HOST_CALL_SYMBOL_PTR
            .store(NAME.as_ptr() as usize, Ordering::Relaxed);
        crate::environment::LAST_HOST_CALL_SYMBOL_LEN.store(NAME.len(), Ordering::Relaxed);
        crate::environment::LAST_HOST_CALL_THREAD.store(7, Ordering::Relaxed);
        for (i, reg) in crate::environment::LAST_HOST_CALL_REGS.iter().enumerate() {
            reg.store(0x1000 + i as u32, Ordering::Relaxed);
        }

        let summary = last_host_call_summary().expect("a summary after a recorded call");
        assert!(summary.contains("glDrawElements"), "{}", summary);
        assert!(summary.contains("guest thread 7"), "{}", summary);
        assert!(summary.contains("r0=0x1000"), "{}", summary);
        assert!(summary.contains("r7=0x1007"), "{}", summary);

        // The pointer and length are stored separately, so a stale length can
        // point into bytes that are not a UTF-8 string (or past the end of the
        // name). The summary must stay printable instead of panicking.
        static NOT_UTF8: &[u8] = b"not\xffutf8";
        crate::environment::LAST_HOST_CALL_SYMBOL_PTR
            .store(NOT_UTF8.as_ptr() as usize, Ordering::Relaxed);
        crate::environment::LAST_HOST_CALL_SYMBOL_LEN.store(NOT_UTF8.len(), Ordering::Relaxed);
        let summary = last_host_call_summary().expect("a summary after a recorded call");
        assert!(summary.contains("<unparsable symbol>"), "{}", summary);
    }
}
