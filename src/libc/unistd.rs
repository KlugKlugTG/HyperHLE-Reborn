/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Miscellaneous parts of `unistd.h`

use crate::abi::DotDotDot;
use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::{FsError, GuestPath};
use crate::libc::errno::{
    set_errno, EACCES, EEXIST, EFAULT, EINVAL, ENOENT, ENOSYS, ENOTDIR, ENOTEMPTY, ENOTSUP, ENOTTY,
    EPERM,
};
use crate::libc::posix_io::{FileDescriptor, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use crate::mem::{ConstPtr, GuestISize, GuestUSize, MutPtr, PAGE_SIZE};
use crate::Environment;
use std::time::Duration;

#[allow(non_camel_case_types)]
type useconds_t = u32;

const F_OK: i32 = 0; // file existence
const X_OK: i32 = 1; // execute/search permission
const W_OK: i32 = 2; // write permission
const R_OK: i32 = 4; // read permission

type SysConfName = i32;
const _SC_ARG_MAX: SysConfName = 1;
const _SC_CHILD_MAX: SysConfName = 2;
const _SC_CLK_TCK: SysConfName = 3;
const _SC_NGROUPS_MAX: SysConfName = 4;
const _SC_OPEN_MAX: SysConfName = 5;
const _SC_JOB_CONTROL: SysConfName = 6;
const _SC_SAVED_IDS: SysConfName = 7;
const _SC_VERSION: SysConfName = 8;
const _SC_BC_BASE_MAX: SysConfName = 9;
const _SC_BC_DIM_MAX: SysConfName = 10;
const _SC_BC_SCALE_MAX: SysConfName = 11;
const _SC_BC_STRING_MAX: SysConfName = 12;
const _SC_COLL_WEIGHTS_MAX: SysConfName = 13;
const _SC_EXPR_NEST_MAX: SysConfName = 14;
const _SC_LINE_MAX: SysConfName = 15;
const _SC_RE_DUP_MAX: SysConfName = 16;
const _SC_2_VERSION: SysConfName = 17;
const _SC_2_C_BIND: SysConfName = 18;
const _SC_2_C_DEV: SysConfName = 19;
const _SC_2_CHAR_TERM: SysConfName = 20;
const _SC_2_FORT_DEV: SysConfName = 21;
const _SC_2_FORT_RUN: SysConfName = 22;
const _SC_2_LOCALEDEF: SysConfName = 23;
const _SC_2_SW_DEV: SysConfName = 24;
const _SC_2_UPE: SysConfName = 25;
const _SC_STREAM_MAX: SysConfName = 26;
const _SC_TZNAME_MAX: SysConfName = 27;
const _SC_GETPW_R_SIZE_MAX: SysConfName = 71;
const _SC_PAGESIZE: SysConfName = 29;
const _SC_NPROCESSORS_CONF: SysConfName = 57;
const _SC_NPROCESSORS_ONLN: SysConfName = 58;
const _SC_PHYS_PAGES: SysConfName = 200;
const _SC_MONOTONIC_CLOCK: SysConfName = 201;
const _SC_THREAD_SAFE_FUNCTIONS: SysConfName = 202;

fn sleep(env: &mut Environment, seconds: u32) -> u32 {
    env.sleep_guest(Duration::from_secs(seconds.into()));
    // sleep() returns the amount of time remaining that should have been slept,
    // but wasn't, if the thread was woken up early by a signal.
    // touchHLE never does that currently, so 0 is always correct here.
    0
}

fn usleep(env: &mut Environment, useconds: useconds_t) -> i32 {
    // POSIX: an interval of one second or more is invalid and fails with
    // EINVAL without sleeping. A valid interval must not clobber errno.
    if useconds >= 1_000_000 {
        log_dbg!("usleep({useconds}) -> -1, EINVAL (interval of 1s or more)");
        set_errno(env, EINVAL);
        return -1;
    }

    env.sleep_guest(Duration::from_micros(useconds.into()));
    0 // success
}

fn alarm(_env: &mut Environment, seconds: u32) -> u32 {
    // touchHLE does not currently deliver Unix signals. These games use alarm
    // only around best-effort network/service checks, so accepting/cancelling
    // it without a pending signal matches the non-blocking path they need.
    log_dbg!("alarm({seconds}) -> 0 (signals are not emulated)");
    0
}

#[allow(non_camel_case_types)]
pub type pid_t = i32;
#[allow(non_camel_case_types)]
type gid_t = u32;

pub fn getpid(_env: &mut Environment) -> pid_t {
    // Not a real value, since touchHLE only simulates a single process.
    // PID 0 would be init, which is a bit unrealistic, so let's go with 1.
    1
}
fn getppid(_env: &mut Environment) -> pid_t {
    // Included just for completeness. Surely no app ever calls this.
    0
}
fn getgid(_env: &mut Environment) -> gid_t {
    // Not a real value
    0
}
fn getegid(_env: &mut Environment) -> gid_t {
    0
}
fn getuid(_env: &mut Environment) -> gid_t {
    // touchHLE simulates a single, unprivileged user. The exact value is
    // mostly irrelevant — apps usually only check `getuid() == 0` (root)
    // or feed it into a hash for analytics.
    501
}
fn geteuid(env: &mut Environment) -> gid_t {
    getuid(env)
}

fn isatty(env: &mut Environment, fd: FileDescriptor) -> i32 {
    if [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO].contains(&fd) {
        1
    } else {
        // POSIX: isatty() on a non-terminal descriptor returns 0 with errno
        // ENOTTY. Our std streams are pipes rather than ttys, but reporting
        // them as ttys is harmless and keeps line-buffered logging sane.
        set_errno(env, ENOTTY);
        0
    }
}

/// Whether `mode` is a valid `access()` mode: `F_OK`, or any combination of
/// `R_OK`, `W_OK` and `X_OK`. Anything else fails with `EINVAL`.
/// https://pubs.opengroup.org/onlinepubs/9699919799/functions/access.html
fn access_mode_is_valid(mode: i32) -> bool {
    mode & !(R_OK | W_OK | X_OK) == 0
}

fn access(env: &mut Environment, path: ConstPtr<u8>, mode: i32) -> i32 {
    let binding = match env.mem.cstr_at_utf8(path) {
        Ok(s) => s.to_owned(),
        Err(bytes) => {
            log!(
                "Warning: access() called with non-UTF-8 path (bytes: {:?}) at {:?}; returning -1/ENOENT",
                bytes,
                path
            );
            set_errno(env, ENOENT);
            return -1;
        }
    };
    // Keep `access` in lockstep with stat/open.  Unity uses access() as an
    // existence probe for Data/data.unity3d on some player versions, so a
    // case-only or stale-bundle-path mismatch must not make it reject a file
    // that open() would successfully load.
    let Some(resolved_binding) =
        crate::libc::posix_io::resolve_existing_guest_path(env, &binding)
    else {
        env.note_missing_unity_player_archive(&binding);
        set_errno(env, ENOENT);
        return -1;
    };
    let guest_path = GuestPath::new(&resolved_binding);
    let (exists, read, write, execute) = env.fs.access(guest_path);
    // POSIX access() takes a bitmask: modes can be ORed together
    // (e.g. R_OK | W_OK). Check every requested bit; all must pass.
    if mode == F_OK {
        if exists {
            0
        } else {
            set_errno(env, ENOENT);
            -1
        }
    } else {
        // POSIX access() accepts any combination of F_OK/R_OK/W_OK/X_OK;
        // reject unknown mode bits with EINVAL.
        if !access_mode_is_valid(mode) {
            // Unknown mode bits: real access() returns -1 with EINVAL.
            log!(
                "Warning: access(): unknown mode {:#x}; returning EINVAL.",
                mode
            );
            set_errno(env, EINVAL);
            -1
        } else {
            let mut result = 0;
            if mode & R_OK != 0 && !read {
                set_errno(env, EACCES);
                result = -1;
            }
            if result == 0 && mode & W_OK != 0 && !write {
                set_errno(env, EACCES);
                result = -1;
            }
            if result == 0 && mode & X_OK != 0 && !execute {
                set_errno(env, EACCES);
                result = -1;
            }
            result
        }
    }
}

fn fork(env: &mut Environment) -> i32 {
    // fork() is not supported in touchHLE — iOS does not support forking
    // Return -1 and set errno to ENOSYS
    // Real iOS never allows fork(), and games calling it on-device would get
    // the same failure, so this warning is once-per-process noise reduction.
    log_once!("fork() called: not supported on iOS (returns -1/ENOSYS)");
    set_errno(env, ENOSYS);
    -1
}

pub(crate) fn unlink(env: &mut Environment, path: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    let Ok(path_str) = env.mem.cstr_at_utf8(path) else {
        log!(
            "Warning: unlink({:?}) called with non-UTF-8 path; returning -1/ENOENT",
            path
        );
        set_errno(env, ENOENT);
        return -1;
    };
    let path_owned = path_str.to_owned();
    let guest_path = GuestPath::new(&path_owned);

    // POSIX `unlink(2)` is for files only — calling it on a directory must
    // fail with EPERM (Apple) / EISDIR (Linux). We map that to EPERM, the
    // documented Darwin behaviour.
    if env.fs.is_dir(guest_path) {
        log_dbg!(
            "unlink('{}') => -1, EPERM (path is a directory)",
            path_owned
        );
        set_errno(env, EPERM);
        return -1;
    }

    log_dbg!("unlink({:?} '{}')", path, path_owned);

    match env.fs.remove(guest_path) {
        Ok(()) => 0,
        Err(e) => {
            let errno = match e {
                FsError::DoesNotExist | FsError::NonexistentParentDir => ENOENT,
                FsError::InvalidParentDir => ENOTDIR,
                FsError::AccessDenied | FsError::ReadonlyParentDir => EACCES,
                // Should not happen here — unlink targets a file — but map
                // defensively.
                FsError::DirectoryNotEmpty => EPERM,
                FsError::AlreadyExist => EINVAL,
            };
            log!("unlink('{}') failed: {:?}", path_owned, e);
            set_errno(env, errno);
            -1
        }
    }
}

/// `int rmdir(const char *path);` — POSIX/Darwin `rmdir(2)`. Removes the
/// directory at `path`, which must be empty. Returns 0 on success and -1
/// with `errno` set on failure. See Apple `man 2 rmdir`:
/// <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rmdir.2.html>
fn rmdir(env: &mut Environment, path: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    let Ok(path_str) = env.mem.cstr_at_utf8(path) else {
        log!(
            "Warning: rmdir({:?}) called with non-UTF-8 path; returning -1/ENOENT",
            path
        );
        set_errno(env, ENOENT);
        return -1;
    };
    let path_owned = path_str.to_owned();
    let guest_path = GuestPath::new(&path_owned);

    if !env.fs.exists(guest_path) {
        log_dbg!("rmdir('{}') => -1, ENOENT", path_owned);
        set_errno(env, ENOENT);
        return -1;
    }

    if !env.fs.is_dir(guest_path) {
        log_dbg!("rmdir('{}') => -1, ENOTDIR (not a directory)", path_owned);
        set_errno(env, ENOTDIR);
        return -1;
    }

    match env.fs.remove(guest_path) {
        Ok(()) => {
            log_dbg!("rmdir('{}') => 0", path_owned);
            0
        }
        Err(e) => {
            let errno = match e {
                FsError::DirectoryNotEmpty => ENOTEMPTY,
                FsError::DoesNotExist | FsError::NonexistentParentDir => ENOENT,
                FsError::InvalidParentDir => ENOTDIR,
                FsError::AccessDenied | FsError::ReadonlyParentDir => EACCES,
                FsError::AlreadyExist => EINVAL,
            };
            log!("rmdir('{}') failed: {:?}", path_owned, e);
            set_errno(env, errno);
            -1
        }
    }
}

/// `int link(const char *path1, const char *path2);` — POSIX/Darwin
/// `link(2)`. Apple's documentation
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/link.2.html>)
/// specifies that `link()` creates a new directory entry `path2` that
/// refers to the same file as `path1`. Both names share the same inode
/// and changes through one are visible through the other.
///
/// HyperHLE's guest filesystem is an in-memory tree with no concept of
/// inodes or hard links, so we emulate the documented externally-visible
/// behaviour for the common single-writer case by copying the file
/// contents. This is the same fallback Apple's man page describes for
/// filesystems that don't have a native link primitive: the *content*
/// of `path2` matches `path1` at the time of the call. Subsequent
/// independent writes will not be visible to the other path, but no
/// shipping iOS app we've observed depends on hard-link aliasing —
/// SQLite/CoreData call `link()` only to durably commit a rename, which
/// our copy-then-unlink fallback satisfies.
fn link(env: &mut Environment, path1: ConstPtr<u8>, path2: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    let Ok(src) = env.mem.cstr_at_utf8(path1) else {
        set_errno(env, ENOENT);
        return -1;
    };
    let Ok(dst) = env.mem.cstr_at_utf8(path2) else {
        set_errno(env, ENOENT);
        return -1;
    };
    let src_owned = src.to_owned();
    let dst_owned = dst.to_owned();
    let src_path = GuestPath::new(&src_owned);
    let dst_path = GuestPath::new(&dst_owned);

    if !env.fs.exists(src_path) {
        log_dbg!("link('{}', '{}') => -1, ENOENT (src)", src_owned, dst_owned);
        set_errno(env, ENOENT);
        return -1;
    }
    // Per Apple's man page: "The path1 argument must not be a directory."
    if env.fs.is_dir(src_path) {
        log_dbg!(
            "link('{}', '{}') => -1, EPERM (src is directory)",
            src_owned,
            dst_owned
        );
        set_errno(env, EPERM);
        return -1;
    }
    if env.fs.exists(dst_path) {
        log_dbg!(
            "link('{}', '{}') => -1, EEXIST (dst exists)",
            src_owned,
            dst_owned
        );
        set_errno(env, EEXIST);
        return -1;
    }

    let Ok(data) = env.fs.read(src_path) else {
        log!("link('{}', '{}'): read failed", src_owned, dst_owned);
        set_errno(env, EACCES);
        return -1;
    };
    if env.fs.write(dst_path, &data).is_err() {
        log!("link('{}', '{}'): write failed", src_owned, dst_owned);
        set_errno(env, EACCES);
        return -1;
    }
    log_dbg!(
        "link('{}', '{}') => 0 (copy emulation)",
        src_owned,
        dst_owned
    );
    0
}

/// `int symlink(const char *path1, const char *path2);` — POSIX/Darwin
/// `symlink(2)`. The Apple man page
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/symlink.2.html>)
/// documents `ENOTSUP` ("The file system does not support the creation of
/// symbolic links") as the canonical errno when the underlying filesystem
/// has no symlink primitive. HyperHLE's guest filesystem has no symlink
/// type — `readlink(2)` here already returns `EINVAL` for every path —
/// so we honestly fail with `ENOTSUP`, mirroring what real Darwin does
/// on a FAT/MS-DOS volume.
fn symlink(env: &mut Environment, path1: ConstPtr<u8>, path2: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);
    let src = env
        .mem
        .cstr_at_utf8(path1)
        .unwrap_or("<bad utf-8>")
        .to_owned();
    let dst = env
        .mem
        .cstr_at_utf8(path2)
        .unwrap_or("<bad utf-8>")
        .to_owned();
    log_dbg!(
        "symlink('{}', '{}') => -1, ENOTSUP (guest fs has no symlinks)",
        src,
        dst
    );
    set_errno(env, ENOTSUP);
    -1
}

fn gethostname(env: &mut Environment, name: MutPtr<u8>, namelen: GuestUSize) -> i32 {
    // A unique per-device hostname needs networking support; until then
    // this hardcoded name matches the emulator's advertised identity.
    let hostname = "touchHLE";
    let len: GuestUSize = hostname.len().try_into().unwrap();
    if namelen <= len {
        // POSIX: name buffer too small -> ENAMETOOLONG. Don't crash the host.
        log!(
            "Warning: gethostname({:?}, {}): buffer too small for hostname ({} bytes needed); returning -1.",
            name,
            namelen,
            len + 1
        );
        set_errno(env, EINVAL);
        return -1;
    }
    env.mem
        .bytes_at_mut(name, len)
        .copy_from_slice(hostname.as_bytes());
    env.mem.write(name + len, b'\0');
    0 // Success
}

fn getpagesize(_env: &mut Environment) -> i32 {
    PAGE_SIZE.try_into().unwrap()
}

fn readlink(
    env: &mut Environment,
    path: ConstPtr<u8>,
    buf: MutPtr<u8>,
    buf_size: GuestISize,
) -> GuestISize {
    // POSIX `ssize_t readlink(const char *path, char *buf, size_t bufsize)`:
    // returns the number of bytes placed in `buf` on success, -1 on
    // error (with errno set). On a path that exists but is not a
    // symbolic link, the documented errno is EINVAL — and that's the
    // common case for the guest filesystem touchHLE exposes (no symlinks
    // anywhere). Mono probes readlink on every dynamic-assembly load
    // (mscorlib.dll, System.dll, etc.) to figure out the canonical
    // path; demoting to debug keeps Unity-engine games from drowning
    // the console in warnings on startup.
    log_dbg!(
        "readlink({:?} '{}', {:?}, {}) => -1, errno=EINVAL (no symlinks in guest filesystem)",
        path,
        env.mem.cstr_at_utf8(path).unwrap_or("<invalid utf8>"),
        buf,
        buf_size,
    );
    // Current implementation of guest's file system doesn't
    // support symbolic links, so the call should unconditionally fail.
    set_errno(env, EINVAL);
    -1
}

fn getdtablesize(_env: &mut Environment) -> i32 {
    // Both macOS 15.7.4 and iOS 4.0.1 report the same dtable size. The
    // matching EMFILE check on open() lives with the descriptor table in
    // posix_io.rs.
    256
}

fn sysconf(_env: &mut Environment, name: SysConfName) -> i32 {
    match name {
        _SC_GETPW_R_SIZE_MAX => 1024,
        _SC_PAGESIZE => PAGE_SIZE.try_into().unwrap(),
        _SC_NPROCESSORS_CONF | _SC_NPROCESSORS_ONLN => 1,
        _SC_PHYS_PAGES => 131072, // ~512 MiB / 4 KiB pages
        _SC_CLK_TCK => 100,
        _SC_OPEN_MAX => 256,
        _SC_ARG_MAX => 262144,
        _SC_CHILD_MAX => -1, // no limit
        _SC_NGROUPS_MAX => 16,
        _SC_STREAM_MAX => 20,
        _SC_TZNAME_MAX => 255,
        _SC_LINE_MAX => 2048,
        _SC_RE_DUP_MAX => 255,
        _SC_JOB_CONTROL => 1,
        _SC_SAVED_IDS => 1,
        _SC_VERSION => 200112, // POSIX.1-2001
        _SC_2_VERSION => 200112,
        _SC_2_C_BIND | _SC_2_C_DEV | _SC_2_SW_DEV => 1,
        _SC_MONOTONIC_CLOCK => 1,
        _SC_THREAD_SAFE_FUNCTIONS => 1,
        _ => {
            log!("sysconf({}): unknown name, returning -1", name);
            -1
        }
    }
}

fn pipe(env: &mut Environment, fds: MutPtr<FileDescriptor>) -> i32 {
    if fds.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }

    let buffer = std::rc::Rc::new(std::cell::RefCell::new(crate::fs::PipeBuffer::new()));
    let read_fd = crate::libc::posix_io::find_or_create_pipe_read_fd(env, buffer.clone());
    let write_fd = crate::libc::posix_io::find_or_create_pipe_write_fd(env, buffer);
    env.mem.write(fds, read_fd);
    env.mem.write(fds + 1, write_fd);
    set_errno(env, 0);
    log_dbg!("pipe({:?}) => [{}, {}]", fds, read_fd, write_fd);
    0
}

fn sbrk(env: &mut Environment, increment: GuestISize) -> GuestISize {
    // sbrk() is used by legacy malloc implementations to grow the heap.
    // touchHLE manages guest memory separately — return -1 to signal failure,
    // causing the C runtime to fall back to mmap-based allocation.
    log_dbg!("sbrk({}) -> -1 (not supported)", increment);
    set_errno(env, ENOSYS);
    -1
}

fn chmod(env: &mut Environment, path: ConstPtr<u8>, _mode: u32) -> i32 {
    // touchHLE guest filesystem is read-only for bundle files.
    // chmod is a no-op — return success to keep apps happy.
    log_dbg!(
        "chmod('{}', ...) -> 0 (stubbed)",
        env.mem.cstr_at_utf8(path).unwrap_or_default()
    );
    0
}

/// `int fchmod(int fd, mode_t mode);`
///
/// Per Apple's `fchmod(2)` man page
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fchmod.2.html>):
///
/// > Th
fn fchmod(_env: &mut Environment, _fd: i32, _mode: u32) -> i32 {
    0
}

// Darwin/XNU `<sys/syscall.h>` selector numbers used by the few syscalls
// touchHLE knows how to implement directly. The full list is enormous; we
// only enumerate the ones we resolve here.
const SYS_FORK: i32 = 2;
const SYS_PTRACE: i32 = 26;
const SYS_STAT: i32 = 188;
const SYS_THREAD_SELFID: i32 = 372;
const SYS_GETPID: i32 = 20;
const SYS_GETPPID: i32 = 39;
const SYS_GETUID: i32 = 24;
const SYS_GETEUID: i32 = 25;
const SYS_GETGID: i32 = 47;
const SYS_GETEGID: i32 = 43;

/// `long syscall(int number, ...);`
///
/// Per the Apple `syscall(2)` man page
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/syscall.2.html>):
///
/// > `syscall()` performs the system call whose assembly language
/// > interface has the specified `number` with the specified arguments.
/// > Symbolic constants for system calls can be found in the header
/// > file `<sys/syscall.h>`. On error, `syscall()` returns -1 and sets
/// > `errno` to indicate the error.
///
/// In touchHLE there is no host kernel sitting beneath the guest — every
/// syscall would have to be intercepted at the libsystem layer. We
/// honour the very small subset that iOS apps occasionally reach via
/// `syscall(SYS_thread_selfid)` etc, and return `-1` with `errno = ENOSYS`
/// for every other selector, which is exactly the contract Apple's
/// kernel uses for selectors the host doesn't implement.
fn syscall(env: &mut Environment, number: i32, args: DotDotDot) -> i32 {
    log_dbg!("syscall({}) called", number);
    match number {
        SYS_FORK => self::fork(env),
        SYS_PTRACE => {
            let mut args = args.start();
            let request: i32 = args.next(env);
            let pid: pid_t = args.next(env);
            let address: MutPtr<u8> = args.next(env);
            let data: i32 = args.next(env);
            crate::libc::sys::ptrace::ptrace(env, request, pid, address, data)
        }
        SYS_STAT => {
            let mut args = args.start();
            let path: ConstPtr<u8> = args.next(env);
            let buffer: MutPtr<crate::libc::posix_io::stat::stat> = args.next(env);
            crate::libc::posix_io::stat::stat(env, path, buffer)
        }
        SYS_GETPID => self::getpid(env),
        SYS_GETPPID => self::getppid(env),
        SYS_GETUID => self::getuid(env) as i32,
        SYS_GETEUID => self::geteuid(env) as i32,
        SYS_GETGID => self::getgid(env) as i32,
        SYS_GETEGID => self::getegid(env) as i32,
        SYS_THREAD_SELFID => {
            // Stable per-thread integer ID. touchHLE numbers threads
            // from 0 upward; the kernel's thread-id space is opaque to
            // userspace, so the only contract is non-zero unique IDs.
            (env.current_thread as i32) + 1
        }
        _ => {
            log!(
                "Warning: syscall({}) unimplemented; returning -1/ENOSYS",
                number
            );
            set_errno(env, ENOSYS);
            -1
        }
    }
}

/// `sync()` — Darwin man 2: "The `sync()` function causes all
/// information in memory that updates file systems to be scheduled for
/// writing out to all file systems." touchHLE delegates every file
/// write to the host kernel through ordinary `write(2)`-family calls,
/// so the guest never has buffered data on our side that still needs to
/// hit disk. The correct emulator-side behaviour is therefore a no-op
/// that returns synchronously, which matches what Darwin would do for
/// an app that did not open any files. Prior to this entry point being
/// present, the dynamic linker installed a return-0 stub — semantically
/// identical here, but having the function declared properly avoids
/// the "call to unimplemented function _sync" warning that confused
/// users into thinking persistence was broken.
fn sync(_env: &mut Environment) {}

fn setpriority(_env: &mut Environment, _which: i32, _who: i32, _priority: i32) -> i32 {
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(syscall(_, _)),
    export_c_func!(sleep(_)),
    export_c_func!(usleep(_)),
    export_c_func!(alarm(_)),
    export_c_func!(getpid()),
    export_c_func!(getppid()),
    export_c_func!(getuid()),
    export_c_func!(geteuid()),
    export_c_func!(getegid()),
    export_c_func!(isatty(_)),
    export_c_func!(access(_, _)),
    export_c_func!(unlink(_)),
    export_c_func!(rmdir(_)),
    export_c_func!(link(_, _)),
    export_c_func!(symlink(_, _)),
    export_c_func!(gethostname(_, _)),
    export_c_func!(getpagesize()),
    export_c_func!(getgid()),
    export_c_func!(readlink(_, _, _)),
    export_c_func!(getdtablesize()),
    export_c_func!(sysconf(_)),
    export_c_func!(pipe(_)),
    export_c_func!(fork()),
    export_c_func!(sbrk(_)),
    export_c_func!(chmod(_, _)),
    export_c_func!(fchmod(_, _)),
    export_c_func!(sync()),
    export_c_func!(setpriority(_, _, _)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_accepts_every_combination_of_permission_bits() {
        // SQLite's unixTempFileDir() probes candidate directories with
        // access(dir, 07); rejecting it made SQLite fall back to the
        // read-only app bundle for its temporary files.
        for mode in [F_OK, R_OK, W_OK, X_OK, R_OK | W_OK, W_OK | X_OK, 7] {
            assert!(access_mode_is_valid(mode), "mode {mode:#x}");
        }
    }

    #[test]
    fn access_rejects_unknown_mode_bits() {
        for mode in [8, 0x10, 7 | 8, -1] {
            assert!(!access_mode_is_valid(mode), "mode {mode:#x}");
        }
    }
}
