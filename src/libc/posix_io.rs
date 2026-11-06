/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! POSIX I/O functions (`fcntl.h`, parts of `unistd.h`, etc)

pub mod stat;
pub mod statvfs;

use crate::abi::DotDotDot;
use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::{GuestFile, GuestOpenOptions, GuestPath};
use crate::libc::errno::{
    set_errno, EAGAIN, EBADF, EFAULT, EINTR, EINVAL, EIO, EISDIR, EMFILE, EOVERFLOW, EPIPE,
    ESPIPE,
};
use crate::libc::sys::socket::{close_socket, duplicate_socket, read_socket, write_socket};
use crate::libc::unistd::pid_t;
use crate::mem::{
    ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead,
};
use crate::Environment;
use std::io::{Read, Seek, SeekFrom, Write};

#[derive(Default)]
pub struct State {
    /// File descriptors _other than stdin, stdout, and stderr_
    files: Vec<Option<PosixFileHostObject>>,
}
impl State {
    fn file_for_fd(&mut self, fd: FileDescriptor) -> Option<&mut PosixFileHostObject> {
        if fd < NORMAL_FILENO_BASE {
            return None;
        }
        self.files
            .get_mut(fd_to_file_idx(fd))
            .and_then(|file_or_none| file_or_none.as_mut())
    }

    /// Whether `fd` refers to a currently-open regular file descriptor (i.e.
    /// not one of the std streams, and not a closed/invalid descriptor).
    pub fn is_fd_open(&self, fd: FileDescriptor) -> bool {
        if fd < NORMAL_FILENO_BASE {
            return false;
        }
        self.files
            .get(fd_to_file_idx(fd))
            .is_some_and(|file_or_none| file_or_none.is_some())
    }

    /// Открытый файл как `GuestFile` — нужно для poll(2), чтобы
    /// различать обычные файлы, pipe-концы и сокеты. None для stdio
    /// (fd < [NORMAL_FILENO_BASE]) и закрытых/несуществующих дескрипторов.
    pub fn guest_file(&self, fd: FileDescriptor) -> Option<&GuestFile> {
        if fd < NORMAL_FILENO_BASE {
            return None;
        }
        self.files
            .get(fd_to_file_idx(fd))
            .and_then(|file_or_none| file_or_none.as_ref())
            .map(|file_obj| &file_obj.file)
    }
}

/// A single byte-range advisory lock recorded against a file descriptor.
///
/// The byte range is `start..end` (half-open) with `end == i64::MAX`
/// representing "to the end of the file" — the convention POSIX uses when
/// `flock::len == 0`. See Apple `man 2 fcntl` ("File Locking").
#[derive(Clone, Copy, Debug)]
pub struct LockRange {
    pub start: i64,
    /// Exclusive end; `i64::MAX` means "until EOF / unlimited".
    pub end: i64,
    pub lock_type: i16,
}

impl LockRange {
    fn overlaps(&self, other: &LockRange) -> bool {
        self.start < other.end && other.start < self.end
    }
}

pub struct PosixFileHostObject {
    pub file: GuestFile,
    pub needs_flush: bool,
    reached_eof: bool,
    /// FD flags (FD_CLOEXEC etc.)
    flags: i32,
    /// File status flags (O_RDONLY, O_WRONLY, O_RDWR, O_APPEND, O_NONBLOCK)
    status_flags: i32,
    /// Guest path this fd was opened with (for F_GETPATH)
    path: Option<String>,
    /// Advisory byte-range locks currently held on this descriptor (via
    /// `fcntl` F_SETLK / F_SETLKW). Released on `close(2)`.
    locks: Vec<LockRange>,
    /// Whole-file advisory lock held via `flock(2)`. `Some(F_RDLCK)` for
    /// `LOCK_SH`, `Some(F_WRLCK)` for `LOCK_EX`, `None` for unlocked.
    flock_state: Option<i16>,
}

// TODO: stdin/stdout/stderr handling somehow
fn file_idx_to_fd(idx: usize) -> FileDescriptor {
    FileDescriptor::try_from(idx)
        .unwrap()
        .checked_add(NORMAL_FILENO_BASE)
        .unwrap()
}
fn fd_to_file_idx(fd: FileDescriptor) -> usize {
    fd.checked_sub(NORMAL_FILENO_BASE).unwrap_or(0) as usize
}

/// File descriptor type.
/// This alias is for readability, POSIX just uses `int`.
pub type FileDescriptor = i32;
pub const STDIN_FILENO: FileDescriptor = 0;
pub const STDOUT_FILENO: FileDescriptor = 1;
pub const STDERR_FILENO: FileDescriptor = 2;
/// Значения `events`/`revents` для poll(2) (sys/poll.h, Darwin).
pub(crate) const POLLIN: i16 = 0x0001;
pub(crate) const POLLPRI: i16 = 0x0002;
pub(crate) const POLLOUT: i16 = 0x0004;
pub(crate) const POLLERR: i16 = 0x0008;
pub(crate) const POLLHUP: i16 = 0x0010;
pub(crate) const POLLNVAL: i16 = 0x0020;
pub(crate) const POLLRDNORM: i16 = 0x0040;
pub(crate) const POLLWRNORM: i16 = 0x0100;

/// `struct pollfd { int fd; short events; short revents; }` (32-битный гость).
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct PollFd {
    fd: FileDescriptor,
    events: i16,
    revents: i16,
}
unsafe impl SafeRead for PollFd {}

const NORMAL_FILENO_BASE: FileDescriptor = STDERR_FILENO + 1;

/// Flags bitfield for `open`.
/// This alias is for readability, POSIX just uses `int`.
pub type OpenFlag = i32;
pub const O_RDONLY: OpenFlag = 0x0;
pub const O_WRONLY: OpenFlag = 0x1;
pub const O_RDWR: OpenFlag = 0x2;
pub const O_ACCMODE: OpenFlag = O_RDWR | O_WRONLY | O_RDONLY;

pub const O_NONBLOCK: OpenFlag = 0x4;
pub const O_APPEND: OpenFlag = 0x8;
pub const O_SHLOCK: OpenFlag = 0x10;
pub const O_NOFOLLOW: OpenFlag = 0x100;
pub const O_CREAT: OpenFlag = 0x200;
pub const O_TRUNC: OpenFlag = 0x400;
pub const O_EXCL: OpenFlag = 0x800;
pub const O_NOCTTY: OpenFlag = 0x20000;

/// File control command flags.
/// This alias is for readability, POSIX just uses `int`.
pub type FileControlCommand = i32;
const F_DUPFD: FileControlCommand = 0;
const F_GETFD: FileControlCommand = 1;
const F_SETFD: FileControlCommand = 2;
const F_GETFL: FileControlCommand = 3;
const F_SETFL: FileControlCommand = 4;
const F_SETLK: FileControlCommand = 8;
const F_SETLKW: FileControlCommand = 9;
const F_GETLK: FileControlCommand = 7;
const F_CHKCLEAN: FileControlCommand = 41;
const F_PREALLOCATE: FileControlCommand = 42;
const F_SETSIZE: FileControlCommand = 43;
const F_RDADVISE: FileControlCommand = 44;
const F_RDAHEAD: FileControlCommand = 45;
const F_TRUNCATEOVERSIZE: FileControlCommand = 46;
const F_GETPATH: FileControlCommand = 50;
const F_FULLFSYNC: FileControlCommand = 51;
const F_PATHPKG_CHECK: FileControlCommand = 52;
const F_ADDSIGS: FileControlCommand = 59;
const F_ADDFILESIGS: FileControlCommand = 61;
const F_DUPFD_CLOEXEC: FileControlCommand = 67;
const F_SETNOSIGPIPE: FileControlCommand = 73;
const F_GETNOSIGPIPE: FileControlCommand = 74;
const F_ADDFILESIGS_FOR_DYLD_SIM: FileControlCommand = 83;
const F_BARRIERFSYNC: FileControlCommand = 85;
const F_ADDFILESIGS_RETURN: FileControlCommand = 97;
const F_ADDFILESUPPL: FileControlCommand = 99;
const F_NOCACHE: FileControlCommand = 48;
const F_PEOFPOSMODE: FileControlCommand = 3;
// used as seek whence, not fcntl cmd
const F_VOLPOSMODE: FileControlCommand = 4;
// same

/// File Descriptor flags.
/// This alias is for readability, POSIX just uses `int`.
pub type FDFlag = i32;
pub const FD_CLOEXEC: FDFlag = 1;

/// Record Locking flags.
/// This alias is for readability, POSIX just uses `short`
pub type RecordLockingFlag = i16;
pub const F_RDLCK: RecordLockingFlag = 1;
pub const F_UNLCK: RecordLockingFlag = 2;
pub const F_WRLCK: RecordLockingFlag = 3;

#[repr(C, packed)]
#[derive(Debug)]
#[allow(non_camel_case_types)]
struct flock {
    start: off_t,
    len: off_t,
    pid: pid_t,
    lock_type: i16,
    whence: i16,
}
unsafe impl SafeRead for flock {}

pub type FLockFlag = i32;
pub const LOCK_SH: FLockFlag = 1;
pub const LOCK_EX: FLockFlag = 2;
pub const LOCK_NB: FLockFlag = 4;
pub const LOCK_UN: FLockFlag = 8;

#[repr(C, packed)]
struct iovec {
    iov_base: ConstPtr<u8>,
    iov_len: GuestUSize,
}
unsafe impl SafeRead for iovec {}

/// Implementation shared by `dup()`, `dup2()` and `fcntl(F_DUPFD)`.
///
/// The new descriptor shares the underlying file description (seek position,
/// status flags) with `fd`, but gets its own descriptor flags. Sockets need
/// special handling: their `GuestFile` is only a placeholder and the host
/// socket is registered per-descriptor in `sys::socket`, so it is duplicated
/// there once the new descriptor number is known (see `duplicate_socket`).
/// Clone the file description behind `fd` into a fresh descriptor entry.
///
/// Returns the new [PosixFileHostObject] and whether the source descriptor is
/// a socket (whose host side has to be re-registered under the new number by
/// the caller). Returns [None] and sets `errno` on failure.
fn clone_file_object(
    env: &mut Environment,
    fd: FileDescriptor,
    cloexec: bool,
) -> Option<(PosixFileHostObject, bool)> {
    let src_is_socket = is_socket(env, fd);
    let Some(src_file) = env.libc_state.posix_io.file_for_fd(fd) else {
        set_errno(env, EBADF);
        return None;
    };
    let cloned = if src_is_socket {
        GuestFile::Socket
    } else {
        match src_file.file.try_clone() {
            Ok(f) => f,
            Err(_e) => {
                log!("dup({}) — try_clone failed: {}", fd, _e);
                set_errno(env, EMFILE);
                return None;
            }
        }
    };
    let src_status_flags = src_file.status_flags;
    let src_path = src_file.path.clone();
    let new_flags = if cloexec { FD_CLOEXEC } else { 0 };
    Some((
        PosixFileHostObject {
            file: cloned,
            needs_flush: false,
            reached_eof: false,
            flags: new_flags,
            status_flags: src_status_flags,
            path: src_path,
            locks: Vec::new(),
            flock_state: None,
        },
        src_is_socket,
    ))
}

fn duplicate_fd(
    env: &mut Environment,
    fd: FileDescriptor,
    min_fd: FileDescriptor,
    cloexec: bool,
) -> FileDescriptor {
    let Some((host_object, src_is_socket)) = clone_file_object(env, fd, cloexec) else {
        return -1;
    };
    // Find the lowest free fd >= min_fd
    let min_idx = if min_fd >= NORMAL_FILENO_BASE {
        fd_to_file_idx(min_fd)
    } else {
        0
    };
    let files = &mut env.libc_state.posix_io.files;
    let new_idx = files
        .iter()
        .enumerate()
        .skip(min_idx)
        .find(|(_, slot)| slot.is_none())
        .map(|(idx, _)| idx);
    let idx = match new_idx {
        Some(idx) => {
            files[idx] = Some(host_object);
            idx
        }
        None => {
            let idx = files.len();
            if idx < min_idx {
                // Extend with None slots up to min_idx
                files.resize_with(min_idx, || None);
                files.push(Some(host_object));
                min_idx
            } else {
                files.push(Some(host_object));
                idx
            }
        }
    };
    let new_fd = file_idx_to_fd(idx);
    if src_is_socket {
        duplicate_socket(env, fd, new_fd);
    }
    new_fd
}

/// `int dup(int fd)`: lowest-numbered free descriptor sharing `fd`'s file
/// description (seek position and status flags stay shared with the guest's
/// copy of POSIX semantics, since the host file handle is cloned).
fn dup(env: &mut Environment, fd: FileDescriptor) -> FileDescriptor {
    set_errno(env, 0);
    if fd >= 0 && fd < NORMAL_FILENO_BASE {
        // The guest's stdio descriptors are modelled on the host side rather
        // than in `posix_io.files`, so there is no file description to clone.
        log!(
            "Warning: dup({}) on a stdio descriptor, returning -1 (not modelled)",
            fd
        );
        set_errno(env, EBADF);
        return -1;
    }
    let new_fd = duplicate_fd(env, fd, 0, false);
    log_dbg!("dup({}) => {}", fd, new_fd);
    new_fd
}

/// `int dup2(int fd, int fd2)`: make `fd2` refer to `fd`'s file description,
/// closing `fd2` first if it is open.
fn dup2(env: &mut Environment, fd: FileDescriptor, new_fd: FileDescriptor) -> FileDescriptor {
    set_errno(env, 0);
    if fd == new_fd {
        if env.libc_state.posix_io.file_for_fd(fd).is_none() {
            set_errno(env, EBADF);
            return -1;
        }
        return new_fd;
    }
    if fd < 0 || new_fd < 0 || new_fd < NORMAL_FILENO_BASE {
        log!(
            "Warning: dup2({}, {}) — stdio descriptors are not modelled, returning -1",
            fd,
            new_fd
        );
        set_errno(env, EBADF);
        return -1;
    }
    let Some((host_object, src_is_socket)) = clone_file_object(env, fd, false) else {
        return -1;
    };
    let idx = fd_to_file_idx(new_fd);
    let old = {
        let files = &mut env.libc_state.posix_io.files;
        if files.len() <= idx {
            files.resize_with(idx + 1, || None);
        }
        files[idx].take()
    };
    if let Some(mut old) = old {
        old.file.close_pipe_endpoint();
        if matches!(old.file, GuestFile::Socket) {
            close_socket(env, new_fd);
        } else if old.needs_flush {
            let _ = old.file.sync_all();
        }
    }
    env.libc_state.posix_io.files[idx] = Some(host_object);
    if src_is_socket {
        duplicate_socket(env, fd, new_fd);
    }
    log_dbg!("dup2({}, {}) => {}", fd, new_fd, new_fd);
    new_fd
}

fn open(env: &mut Environment, path: ConstPtr<u8>, flags: i32, _args: DotDotDot) -> FileDescriptor {
    set_errno(env, 0);
    self::open_direct(env, path, flags)
}

fn open_dprotected_np(
    env: &mut Environment,
    path: ConstPtr<u8>,
    flags: i32,
    _class: i32,
    _dpflags: i32,
    args: DotDotDot,
) -> FileDescriptor {
    open(env, path, flags, args)
}

fn creat(env: &mut Environment, path: ConstPtr<u8>, _mode: u32) -> i32 {
    // creat(path, mode) == open(path, O_WRONLY|O_CREAT|O_TRUNC)
    // O_WRONLY=0x0001, O_CREAT=0x0200, O_TRUNC=0x0400
    let flags = 0x0001 | 0x0200 | 0x0400;
    open_direct(env, path, flags)
}

/// Return the part of an absolute path after its first `.app` component.
///
/// The component comparison deliberately follows the app-volume's
/// case-insensitive semantics.  This lets a player preserve a stale bundle name
/// (or spelling such as `GRANNY.APP`) without accidentally treating a filename
/// like `something.app.backup` as an app bundle.
fn path_after_app_bundle_component(path: &str) -> Option<&str> {
    let mut component_start = 0;
    for component in path.split('/') {
        let component_end = component_start + component.len();
        if component
            .get(component.len().saturating_sub(".app".len())..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".app"))
        {
            return path
                .get(component_end..)
                .map(|relative| relative.trim_start_matches('/'));
        }
        // `split('/')` advances past exactly one slash between components.
        component_start = component_end + 1;
    }
    None
}

/// Unreal's IPhoneTOC records cooked assets under `../UDKGame/`,
/// while iOS mounts them at the app bundle root.
fn strip_udk_game_bundle_prefix(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let path = normalized.strip_prefix("./").unwrap_or(&normalized);
    let relative = path.strip_prefix("../")?;
    let (root, contents) = relative.split_once('/').unwrap_or((relative, ""));
    root.eq_ignore_ascii_case("UDKGame")
        .then(|| contents.to_string())
}

/// Resolve an existing file path as iPhone OS would see it.
///
/// Bundle files live on a case-insensitive volume on the devices this emulator
/// targets.  In addition, several Unity players use a stale absolute bundle
/// path or omit the `Data/` prefix when probing their player archive.  Keep the
/// compatibility search in one place so `open`, `stat`, and `access` agree
/// about whether a resource is mounted.
pub(crate) fn resolve_existing_guest_path(env: &Environment, path: &str) -> Option<String> {
    let resolve = |candidate: &str| {
        env.fs
            .resolve_case_insensitive_path(GuestPath::new(candidate))
            .map(Into::<String>::into)
    };

    if let Some(resolved) = resolve(path) {
        return Some(resolved);
    }

    let bundle_root = env.bundle.bundle_path().as_str().trim_end_matches('/');
    let relative_path = if path == bundle_root {
        Some("")
    } else if let Some(relative) = path
        .strip_prefix(bundle_root)
        .filter(|relative| relative.starts_with('/'))
    {
        Some(relative.trim_start_matches('/'))
    } else if path.starts_with('/') {
        // A few engines preserve the original `.app` directory in a saved
        // path, even though the VFS names the mounted bundle from
        // CFBundleName. Only remap a complete app-bundle component; never
        // redirect arbitrary absolute filesystem paths.
        path_after_app_bundle_component(path)
    } else {
        Some(path.trim_start_matches("./"))
    }?;
    let relative_path =
        strip_udk_game_bundle_prefix(relative_path).unwrap_or_else(|| relative_path.to_string());

    let data_relative_path = relative_path
        .split_once('/')
        .and_then(|(component, remainder)| {
            component.eq_ignore_ascii_case("Data").then_some(remainder)
        })
        .unwrap_or(relative_path.as_str());
    let mut candidates = vec![
        format!("{bundle_root}/{relative_path}"),
        format!("{bundle_root}/Data/{data_relative_path}"),
    ];
    // A few repackaged Unity players place the archive at the bundle root
    // while their executable still probes Data/data.unity3d. Prefer the real
    // Data path above, then accept that layout as a last resource-only alias.
    if data_relative_path != relative_path {
        candidates.push(format!("{bundle_root}/{data_relative_path}"));
    }
    candidates.iter().find_map(|candidate| resolve(candidate))
}

pub fn open_direct(env: &mut Environment, path: ConstPtr<u8>, flags: i32) -> FileDescriptor {
    let known_flags = O_ACCMODE
        | O_NONBLOCK
        | O_APPEND
        | O_SHLOCK
        | O_NOFOLLOW
        | O_CREAT
        | O_TRUNC
        | O_EXCL
        | O_NOCTTY;
    let unknown_flags = flags & !known_flags;
    if unknown_flags != 0 {
        log!(
            "Warning: open(): ignoring unrecognized open flags {:#x} (full flags: {:#x}).",
            unknown_flags,
            flags
        );
    }
    // ИСПРАВЛЕНИЕ 1: убран assert!(flags & O_EXCL == 0).
    // O_EXCL — валидный флаг (создание файла с проверкой на существование).
    // Вместо паники — корректная обработка ниже, после разрешения пути.

    if path.is_null() {
        log_dbg!("open({:?}, {:#x}) => -1", path, flags);
        return -1;
    }

    let mut needs_flush = false;
    let mut options = GuestOpenOptions::new();
    match flags & O_ACCMODE {
        O_RDONLY => {
            options.read();
        }
        O_WRONLY => {
            options.write();
            needs_flush = true;
        }
        O_RDWR => {
            options.read().write();
            needs_flush = true;
        }
        other => {
            // flags & O_ACCMODE is at most O_ACCMODE wide, so the four
            // arms above cover all valid values. If a guest passes a
            // weird value (e.g. an uninitialised buffer), behave like
            // real libc and return EINVAL instead of crashing the host.
            log!(
                "Warning: open(): unknown access mode {:#x}; returning EINVAL.",
                other
            );
            set_errno(env, EINVAL);
            return -1;
        }
    };
    if (flags & O_APPEND) != 0 {
        options.append();
    }
    if (flags & O_CREAT) != 0 {
        options.create();
    }
    if (flags & O_EXCL) != 0 {
        options.create_new();
    }
    if (flags & O_TRUNC) != 0 {
        options.truncate();
    }

    // POSIX paths are byte strings: neither the kernel nor libc rejects a name
    // because it is not valid UTF-8, so open() must not fail here either.
    // Unity's shader cache appends an uninitialised tail to a cache path, and
    // treating that as "invalid UTF-8" made every such open() fail with a log
    // line. The emulated filesystem is keyed by strings, so the name is
    // carried with replacement characters; the same bytes always map to the
    // same name, which keeps repeated opens of one cache entry consistent.
    let path_string = env.mem.cstr_at_utf8_lossy(path).into_owned();

    if flags & O_NOFOLLOW != 0 {
        log!("Ignoring O_NOFOLLOW when opening {:?}", path_string);
    }

    // `/dev/random` and `/dev/urandom` are identical non-blocking CSPRNG
    // character devices on Apple platforms (see the `random(4)` manual page).
    // The emulated filesystem has no such node, so opening it used to fail;
    // apps — and libc++'s `std::random_device` — then read from the resulting
    // fd -1, which failed and made libc++ throw `std::system_error`, aborting
    // launch (e.g. Minecraft PE). Back these paths with a real random source.
    if path_string == "/dev/urandom" || path_string == "/dev/random" {
        let host_object = PosixFileHostObject {
            file: GuestFile::random(),
            needs_flush: false,
            reached_eof: false,
            flags: 0,
            status_flags: flags & (O_ACCMODE | O_APPEND | O_NONBLOCK),
            path: Some(path_string.clone()),
            locks: Vec::new(),
            flock_state: None,
        };
        let fd = find_or_create_fd(env, host_object);
        log_dbg!("open({:?}, {:#x}) => {:?} (random device)", path, flags, fd);
        return fd;
    }

    let actual_path_string = if (flags & O_CREAT) == 0 {
        resolve_existing_guest_path(env, &path_string)
    } else {
        // O_CREAT still needs case-insensitive matching for an existing file
        // (notably O_CREAT|O_EXCL), but must not redirect a new file into the
        // app bundle's read-only resource tree.
        env.fs
            .resolve_case_insensitive_path(GuestPath::new(&path_string))
            .map(Into::<String>::into)
    }
    .unwrap_or_else(|| path_string.clone());

    // ИСПРАВЛЕНИЕ 2: корректная реализация O_EXCL.
    // O_CREAT|O_EXCL означает «создать файл, но вернуть ошибку, если он уже
    // есть».
    // Без этой проверки приложения, использующие O_EXCL как lock-файл,
    // получали паник вместо штатного EEXIST.
    use crate::libc::errno::EEXIST;
    if (flags & O_EXCL) != 0
        && (flags & O_CREAT) != 0
        && env.fs.exists(GuestPath::new(&actual_path_string))
    {
        set_errno(env, EEXIST);
        log_dbg!(
            "open({:?} {:?}, {:#x}) => -1 (O_EXCL: file exists)",
            path,
            actual_path_string,
            flags
        );
        return -1;
    }

    let res = match env
        .fs
        .open_with_options(GuestPath::new(&actual_path_string), options)
    {
        Ok(file) => {
            let host_object = PosixFileHostObject {
                file,
                needs_flush,
                reached_eof: false,
                flags: 0,
                status_flags: flags & (O_ACCMODE | O_APPEND | O_NONBLOCK),
                path: Some(actual_path_string.clone()),
                locks: Vec::new(),
                flock_state: None,
            };
            find_or_create_fd(env, host_object)
        }
        Err(()) => -1,
    };
    if res == -1 && (flags & O_CREAT) == 0 {
        env.note_missing_unity_player_archive(&path_string);
    }
    // DIAG (TOUCHHLE_TRACE_OPEN=1): trace every file open with its result, so a
    // guest stuck waiting on data can be traced back to the file it wanted.
    if crate::env_flag_cached!("TOUCHHLE_TRACE_OPEN") {
        log!(
            "OPEN-DIAG t={} req={:?} resolved={:?} flags={:#x} => fd={}",
            env.current_thread,
            path_string,
            actual_path_string,
            flags,
            res
        );
    }
    if res != -1 && (flags & O_SHLOCK) != 0 {
        flock(env, res, LOCK_SH);
    }
    log_dbg!(
        "open({:?} {:?}, {:#x}) => {:?}",
        path,
        actual_path_string,
        flags,
        res
    );
    let lower_path = path_string.to_ascii_lowercase();
    if lower_path.ends_with(".fnt")
        || lower_path.ends_with(".png")
        || lower_path.ends_with(".plist")
        || lower_path.ends_with(".jpg")
        || lower_path.ends_with(".jpeg")
    {
        static RESOURCE_TRACE_COUNT: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        if crate::env_flag_cached!("TOUCHHLE_TRACE_RESOURCES")
            && RESOURCE_TRACE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 300
        {
            log!(
                "[RESOURCE-TRACE] requested={:?} resolved={:?} flags={:#x} fd={}",
                path_string,
                actual_path_string,
                flags,
                res
            );
        }
    }
    res
}

pub fn read(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: MutVoidPtr,
    size: GuestUSize,
) -> GuestISize {
    set_errno(env, 0);
    if buffer.is_null() {
        return -1;
    }

    // Sockets are not backed by a real `GuestFile` (see
    // `GuestFile::Socket`): the host socket lives in `sys::socket`, so
    // generic reads (`read(2)`, `fread()`, `NSFileHandle`) are served there.
    // Before this dispatch every one of them failed with "read not supported
    // on socket via GuestFile".
    if is_socket(env, fd) {
        let bytes_read = read_socket(env, fd, buffer, size);
        if bytes_read == 0 && size != 0 {
            // The peer shut the connection down: that is EOF for the guest.
            if let Some(file) = env.libc_state.posix_io.file_for_fd(fd) {
                file.reached_eof = true;
            }
        }
        return bytes_read;
    }

    // Keep the descriptor borrow inside this block: after an unreadable Unity
    // archive is detected, we need the whole Environment to record it before
    // the guest can reach its fatal exit path.
    let (read_result, unusable_player_archive) = {
        let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
            log!(
                "Warning: read({:?}, {:?}, {:#x}) called with unknown fd, returning -1",
                fd,
                buffer,
                size
            );
            set_errno(env, EBADF);
            return -1;
        };

        // A zero-length first read of Unity's mandatory archive means that a
        // ZIP entry was registered but could not be decompressed. Do not treat
        // a normal EOF after valid data as a mount failure.
        let starts_at_beginning = file.file.stream_position().ok() == Some(0);
        let archive_path = file.path.clone();
        let read_result = {
            let buffer_slice = env.mem.bytes_at_mut(buffer.cast(), size);
            file.file.read(buffer_slice)
        };
        if let Ok(bytes_read) = &read_result {
            if *bytes_read == 0 && size != 0 {
                file.reached_eof = true;
            }
        }
        let unusable_player_archive = match &read_result {
            Ok(bytes_read) if *bytes_read == 0 && size != 0 && starts_at_beginning => archive_path,
            // An I/O error at any offset makes this required archive unusable;
            // note it even when the caller has already read its header.
            Err(_) => archive_path,
            _ => None,
        };
        (read_result, unusable_player_archive)
    };

    if let Some(path) = unusable_player_archive {
        env.note_missing_unity_player_archive(&path);
    }

    match read_result {
        Ok(bytes_read) => {
            // Do not emit a warning for normal EOF. Many apps read files one
            // byte at a time until the end; that is routine POSIX behavior.
            if bytes_read == 0 {
                log_dbg!("read({:?}, {:?}, {:#x}) => 0 (EOF)", fd, buffer, size);
            } else if bytes_read < size as usize {
                // POSIX read(2) returning fewer bytes than requested is normal
                // (e.g. near EOF or for non-regular files). Demote to debug log.
                log_dbg!(
                    "read({:?}, {:?}, {:#x}) read only {:#x} bytes",
                    fd,
                    buffer,
                    size,
                    bytes_read
                );
            } else {
                log_dbg!(
                    "read({:?}, {:?}, {:#x}) => {:#x}",
                    fd,
                    buffer,
                    size,
                    bytes_read
                );
            }
            bytes_read.try_into().unwrap_or(-1)
        }
        Err(e) => {
            let res = match e.kind() {
                std::io::ErrorKind::IsADirectory => {
                    set_errno(env, EISDIR);
                    0
                }
                std::io::ErrorKind::WouldBlock => {
                    set_errno(env, EAGAIN);
                    -1
                }
                std::io::ErrorKind::BrokenPipe => {
                    set_errno(env, EPIPE);
                    -1
                }
                _ => -1,
            };
            log!(
                "Warning: read({:?}, {:?}, {:#x}) encountered error {:?}, \
                 returning {}",
                fd,
                buffer,
                size,
                e,
                res
            );
            res
        }
    }
}

pub fn pread(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: MutVoidPtr,
    size: GuestUSize,
    offset: off_t,
) -> GuestISize {
    let original_position = lseek(env, fd, 0, SEEK_CUR);
    if original_position == -1 {
        return -1;
    }

    if lseek(env, fd, offset, SEEK_SET) == -1 {
        return -1;
    }

    let bytes_read = read(env, fd, buffer, size);

    assert!(lseek(env, fd, original_position, SEEK_SET) != -1);
    bytes_read
}

pub(super) fn eof(env: &mut Environment, fd: FileDescriptor) -> i32 {
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        return 1;
    };
    if file.reached_eof {
        1
    } else {
        0
    }
}

pub(super) fn clearerr(env: &mut Environment, fd: FileDescriptor) {
    set_errno(env, 0);
    if let Some(file) = env.libc_state.posix_io.file_for_fd(fd) {
        file.reached_eof = false;
    }
}

pub(super) fn fflush(env: &mut Environment, fd: FileDescriptor) -> i32 {
    set_errno(env, 0);
    // Socket writes are unbuffered (they go straight to the host socket in
    // `sys::socket`), so there is never anything to flush and flushing must
    // not fail — `fclose()` and `fflush()` on a socket stream report success.
    if is_socket(env, fd) {
        return 0;
    }
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        return -1;
    };
    match file.file.flush() {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub fn write(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: ConstVoidPtr,
    size: GuestUSize,
) -> GuestISize {
    set_errno(env, 0);
    // ПЕРЕХВАТ КОНСОЛИ! Ловим stdout и stderr от Unity.
    if fd == STDOUT_FILENO || fd == STDERR_FILENO {
        let buffer_slice = env.mem.bytes_at(buffer.cast(), size);
        let msg = String::from_utf8_lossy(buffer_slice);
        print!("{}", msg);
        if fd == STDERR_FILENO {
            // A guest's C library and assertion machinery report failures
            // straight to fd 2: dlmalloc prints "free(): invalid pointer" and
            // friends, and libc prints uncaught-abort reasons. The host
            // process's stdout is not captured into touchHLE_log.txt, so a
            // crash report would lose the one line that explains it. Mirror
            // stderr into the file log; stdout is left alone because games
            // print there constantly.
            let text = msg.trim_end_matches('\n');
            if !text.is_empty() {
                log_file_only!("[guest stderr] {}", text);
            }
        }
        return size as GuestISize;
    }

    // See the note in `read()`: a socket has no `GuestFile` backing, so the
    // guest's `write(2)`/`fwrite()`/`NSFileHandle` writes go to `sys::socket`
    // instead of failing with "write not supported on socket via GuestFile".
    if is_socket(env, fd) {
        return write_socket(env, fd, buffer, size);
    }

    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        set_errno(env, EBADF);
        return -1;
    };

    let buffer_slice = env.mem.bytes_at(buffer.cast(), size);
    match file.file.write(buffer_slice) {
        Ok(bytes_written) => {
            if bytes_written < buffer_slice.len() {
                log!(
                    "Warning: write({:?}, {:?}, {:#x}) wrote only {:#x} bytes",
                    fd,
                    buffer,
                    size,
                    bytes_written
                );
            } else {
                log_dbg!(
                    "write({:?}, {:?}, {:#x}) => {:#x}",
                    fd,
                    buffer,
                    size,
                    bytes_written
                );
            }
            bytes_written.try_into().unwrap_or(-1)
        }
        Err(e) => {
            match e.kind() {
                std::io::ErrorKind::WouldBlock => set_errno(env, EAGAIN),
                std::io::ErrorKind::BrokenPipe => set_errno(env, EPIPE),
                _ => {}
            }
            log!(
                "Warning: write({:?}, {:?}, {:#x}) encountered error {:?}, \
                 returning -1",
                fd,
                buffer,
                size,
                e
            );
            -1
        }
    }
}

pub fn pwrite(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: ConstVoidPtr,
    size: GuestUSize,
    offset: off_t,
) -> GuestISize {
    let original_position = lseek(env, fd, 0, SEEK_CUR);
    if original_position == -1 {
        return -1;
    }
    if lseek(env, fd, offset, SEEK_SET) == -1 {
        return -1;
    }
    let bytes_written = write(env, fd, buffer, size);
    assert!(lseek(env, fd, original_position, SEEK_SET) != -1);
    bytes_written
}

#[allow(non_camel_case_types)]
pub type off_t = i64;
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

pub fn lseek(env: &mut Environment, fd: FileDescriptor, offset: off_t, whence: i32) -> off_t {
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        log!("lseek({:?}, {:#x}, {}) => {}", fd, offset, whence, -1);
        set_errno(env, EBADF);
        return -1;
    };

    if !file.file.is_seekable() {
        log!(
            "Warning: lseek({:?}, {:#x}, {}) => -1. Called with unseekable fd.",
            fd,
            offset,
            whence
        );
        set_errno(env, ESPIPE);
        return -1;
    }

    let start_position = match whence {
        SEEK_SET => 0,
        SEEK_CUR => match file.file.stream_position() {
            Ok(pos) => pos,
            Err(seek_error) => {
                match seek_error.kind() {
                    std::io::ErrorKind::IsADirectory => set_errno(env, EISDIR),
                    _ => {
                        log!(
                            "Warning: lseek encountered unexpected seek error {:?}; returning EIO.",
                            seek_error
                        );
                        set_errno(env, EIO);
                    }
                }
                return -1;
            }
        },
        SEEK_END => match file.file.stream_len() {
            Ok(len) => len,
            Err(seek_error) => {
                match seek_error.kind() {
                    std::io::ErrorKind::IsADirectory => set_errno(env, EISDIR),
                    _ => {
                        log!(
                            "Warning: lseek encountered unexpected seek error {:?}; returning EIO.",
                            seek_error
                        );
                        set_errno(env, EIO);
                    }
                }
                return -1;
            }
        },
        _ => {
            log!(
                "Warning: lseek({:?}, {:#x}, {}) => -1. Called with invalid \
                 \"whence\".",
                fd,
                offset,
                whence
            );
            set_errno(env, EINVAL);
            return -1;
        }
    };

    let seek_position = match start_position.checked_add_signed(offset) {
        Some(position) => position,
        None => {
            let (error_msg, errno) = if offset >= 0 {
                ("Seek position does not fit in off_t.", EOVERFLOW)
            } else {
                ("Negative seek position.", EINVAL)
            };
            log!(
                "Warning: lseek({:?}, {:#x}, {}) => -1. {}",
                fd,
                offset,
                whence,
                error_msg
            );
            set_errno(env, errno);
            return -1;
        }
    };
    if seek_position > off_t::MAX as u64 {
        log!(
            "Warning: lseek({:?}, {:#x}, {}) => -1. Seek position does not fit \
             in off_t.",
            fd,
            offset,
            whence
        );
        set_errno(env, EOVERFLOW);
        return -1;
    }

    let res = match file.file.seek(SeekFrom::Start(seek_position)) {
        Ok(new_offset) => {
            file.reached_eof = false;
            new_offset.try_into().unwrap_or(-1)
        }
        Err(seek_error) => {
            match seek_error.kind() {
                std::io::ErrorKind::InvalidInput => set_errno(env, EINVAL),
                std::io::ErrorKind::IsADirectory => set_errno(env, EISDIR),
                _ => {
                    log!(
                        "Warning: lseek encountered unexpected seek error {:?}; returning EIO.",
                        seek_error
                    );
                    set_errno(env, EIO);
                }
            }
            log!(
                "Warning: lseek({:?}, {:#x}, {}) failed with error: {:?}, \
                 returning -1",
                fd,
                offset,
                whence,
                seek_error
            );
            return -1;
        }
    };
    log_dbg!("lseek({:?}, {:#x}, {}) => {}", fd, offset, whence, res);
    res
}

pub fn close(env: &mut Environment, fd: FileDescriptor) -> i32 {
    let signed_fd = fd;

    if signed_fd < 0 {
        log_dbg!(
            "close({}) failed: invalid fd (negative), returning -1 (EBADF)",
            signed_fd
        );
        set_errno(env, EBADF);
        return -1;
    }

    if fd < NORMAL_FILENO_BASE {
        // Игнорируем попытки закрыть стандартные потоки (stdin=0, stdout=1,
        // stderr=2)
        log_dbg!("close({}): ignored standard stream", fd);
        return 0;
    }

    // Берем слот по индексу FD
    if let Some(file_obj_slot) = env.libc_state.posix_io.files.get_mut(fd_to_file_idx(fd)) {
        // Честно извлекаем объект (take заменяет его на None в массиве,
        // освобождая FD)
        if let Some(mut file_obj) = file_obj_slot.take() {
            file_obj.file.close_pipe_endpoint();
            // Если это был сокет, ОБЯЗАТЕЛЬНО удаляем его из таблицы в
            // socket.rs
            if matches!(file_obj.file, GuestFile::Socket) {
                close_socket(env, fd);
            }
            // Если это обычный файл с правами на запись, честно сбрасываем
            // буфер
            else if file_obj.needs_flush {
                let _ = file_obj.file.sync_all();
            }

            log_dbg!("close({}) -> success", fd);
            return 0;
        }
    }

    log_dbg!(
        "close({}) failed: fd not open or doesn't exist, returning -1 (EBADF)",
        fd
    );
    set_errno(env, EBADF);
    -1
}

fn rename(env: &mut Environment, old: ConstPtr<u8>, new: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);
    let old_str = env.mem.cstr_at_utf8(old).unwrap_or_default();
    let new_str = env.mem.cstr_at_utf8(new).unwrap_or_default();
    let res = match env
        .fs
        .rename(GuestPath::new(&old_str), GuestPath::new(&new_str))
    {
        Ok(_) => 0,
        Err(_) => -1,
    };
    log_dbg!("rename('{}', '{}') => {}", old_str, new_str, res);
    res
}

pub fn getcwd(env: &mut Environment, buf_ptr: MutPtr<u8>, buf_size: GuestUSize) -> MutPtr<u8> {
    let working_directory = env.fs.working_directory();
    if !env.fs.is_dir(working_directory) {
        log!(
            "Warning: getcwd({:?}, {:#x}) failed, returning NULL",
            buf_ptr,
            buf_size
        );
        return Ptr::null();
    }

    let working_directory = env.fs.working_directory().as_str().as_bytes();

    if buf_ptr.is_null() {
        let res = env.mem.alloc_and_write_cstr(working_directory);
        log_dbg!("getcwd(NULL, _) => {:?} ({:?})", res, working_directory);
        return res;
    }

    let res_size: GuestUSize = u32::try_from(working_directory.len()).unwrap_or(0) + 1;
    if buf_size < res_size {
        log!(
            "Warning: getcwd({:?}, {:#x}) failed, returning NULL",
            buf_ptr,
            buf_size
        );
        return Ptr::null();
    }

    let buf = env.mem.bytes_at_mut(buf_ptr, res_size);
    buf[..(res_size - 1) as usize].copy_from_slice(working_directory);
    buf[(res_size - 1) as usize] = b'\0';

    log_dbg!(
        "getcwd({:?}, {:#x}) => {:?}, wrote {:?} ({:#x} bytes)",
        buf_ptr,
        buf_size,
        buf_ptr,
        working_directory,
        res_size
    );
    buf_ptr
}

fn chdir(env: &mut Environment, path_ptr: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    let path_str = env.mem.cstr_at_utf8(path_ptr).unwrap_or_default();
    // POSIX: chdir("") must fail with ENOENT. Treating it as success
    // (which previously silently chdir'd to "/") confuses some apps that
    // rely on errno propagation — most notably Farm Frenzy.
    if path_str.is_empty() {
        use crate::libc::errno::ENOENT;
        set_errno(env, ENOENT);
        log!("Warning: chdir(\"\") rejected, returning -1 (ENOENT)");
        return -1;
    }
    let resolved_path = resolve_existing_guest_path(env, &path_str);
    let path = GuestPath::new(resolved_path.as_deref().unwrap_or(&path_str));
    match env.fs.change_working_directory(path) {
        Ok(new) => {
            log_dbg!(
                "chdir({:?}) => 0, new working directory: {:?}",
                path_ptr,
                new
            );
            0
        }
        Err(()) => {
            log!(
                "Warning: chdir({:?}) failed, could not change working \
                 directory to {:?}, returning -1",
                path_ptr,
                path
            );
            -1
        }
    }
}

fn fcntl(
    env: &mut Environment,
    fd: FileDescriptor,
    cmd: FileControlCommand,
    args: DotDotDot,
) -> i32 {
    set_errno(env, 0);
    if fd >= NORMAL_FILENO_BASE
        && env
            .libc_state
            .posix_io
            .files
            .get(fd_to_file_idx(fd))
            .is_none()
    {
        set_errno(env, EBADF);
        return -1;
    }

    // Apple `man 2 fcntl`: stdin/stdout/stderr are always open file
    // descriptors on iOS, so `F_GETFD`/`F_GETFL` must succeed for them.
    // Returning EBADF here breaks managed runtimes (Mono's `System.Console`
    // initialiser queries `F_GETFL` on stderr and bails out if it fails).
    // Report sane defaults: no FD_CLOEXEC, access mode matches the stream's
    // role (read-only for stdin, write-only for stdout/stderr).
    if matches!(fd, STDIN_FILENO | STDOUT_FILENO | STDERR_FILENO) {
        match cmd {
            F_GETFD => return 0,
            F_GETFL => {
                return if fd == STDIN_FILENO {
                    O_RDONLY
                } else {
                    O_WRONLY
                };
            }
            // Other operations are silently accepted as no-ops for std
            // streams (e.g. F_SETFD with FD_CLOEXEC=0).
            _ => return 0,
        }
    }

    match cmd {
        // ----------------------------------------------------------------
        // File descriptor flags
        // ----------------------------------------------------------------
        F_GETFD => {
            let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
                set_errno(env, EBADF);
                return -1;
            };
            return file.flags;
        }
        F_SETFD => {
            let flags: i32 = args.start().next(env);
            // FD_CLOEXEC (close-on-exec) is a no-op in HyperHLE because the
            // emulator is a single guest process that never calls exec().
            // Per Apple's `man 2 fcntl`, FD_CLOEXEC causes the descriptor to
            // be closed when a new process image is created via exec — this
            // is irrelevant in a single-process emulator. We store the flag
            // value so that F_GETFD returns it correctly (apps like SQLite
            // set FD_CLOEXEC and then verify it with F_GETFD).
            log_dbg!(
                "fcntl({}, F_SETFD, {:#x}) — stored (CLOEXEC is no-op in \
                 single-process emulator)",
                fd,
                flags
            );
            if let Some(file) = env.libc_state.posix_io.file_for_fd(fd) {
                file.flags = flags;
            }
        }

        // ----------------------------------------------------------------
        // File status flags
        // ----------------------------------------------------------------
        F_GETFL => {
            let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
                set_errno(env, EBADF);
                return -1;
            };
            return file.status_flags;
        }
        F_SETFL => {
            let flags: i32 = args.start().next(env);
            log_dbg!("fcntl({}, F_SETFL, {:#x})", fd, flags);
            if let Some(file) = env.libc_state.posix_io.file_for_fd(fd) {
                let access = file.status_flags & O_ACCMODE;
                file.status_flags = access | (flags & !O_ACCMODE);
            }
        }

        // ----------------------------------------------------------------
        // Advisory record locking
        // ----------------------------------------------------------------
        F_GETLK => {
            // POSIX `fcntl(F_GETLK)` reports whether a lock that would
            // block the request is held by **another process**. HyperHLE
            // is a single guest process, so there is no external
            // contender — per Apple `man 2 fcntl` we always report
            // F_UNLCK after validating the request struct.
            let lock_ptr: MutPtr<flock> = args.start().next(env);
            let mut lock = env.mem.read(lock_ptr);
            if let Err(error_code) = resolve_lock_range(env, fd, &lock) {
                set_errno(env, error_code);
                return -1;
            }
            lock.lock_type = F_UNLCK;
            env.mem.write(lock_ptr, lock);
        }
        F_SETLK | F_SETLKW => {
            // POSIX advisory locks are owned by the process, not the fd:
            // locks held by the same process **never** conflict with each
            // other (`man 2 fcntl`, "File Locking"). HyperHLE is a single
            // guest process, so there are no inter-process conflicts to
            // detect. We still validate the request struct and track the
            // lock per-fd so it is released on `close(2)`.
            let lock_ptr: MutPtr<flock> = args.start().next(env);
            let lock = env.mem.read(lock_ptr);
            let requested = match resolve_lock_range(env, fd, &lock) {
                Ok(r) => r,
                Err(error_code) => {
                    set_errno(env, error_code);
                    return -1;
                }
            };

            if let Some(file) = env.libc_state.posix_io.file_for_fd(fd) {
                if requested.lock_type == F_UNLCK {
                    release_range_from_locks(&mut file.locks, &requested);
                } else {
                    // Replace any existing lock on the overlapping range
                    // (POSIX: a new lock from the same owner promotes /
                    // demotes the existing one).
                    release_range_from_locks(&mut file.locks, &requested);
                    file.locks.push(requested);
                }
            }
            log_dbg!(
                "fcntl({}, {}, {:?} [{}, {})) => 0",
                fd,
                cmd,
                requested.lock_type,
                requested.start,
                requested.end
            );
        }

        // ----------------------------------------------------------------
        // Duplicate file descriptor
        // ----------------------------------------------------------------
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let min_fd: i32 = args.start().next(env);
            // Per Apple `man 2 fcntl`: F_DUPFD returns a new file descriptor
            // that is the lowest numbered available descriptor >= min_fd.
            // F_DUPFD_CLOEXEC does the same but sets FD_CLOEXEC on the new fd.
            let new_fd = duplicate_fd(env, fd, min_fd, cmd == F_DUPFD_CLOEXEC);
            log_dbg!(
                "fcntl({}, {}, {}) => {} (duplicated fd)",
                fd,
                cmd,
                min_fd,
                new_fd
            );
            return new_fd;
        }

        // ----------------------------------------------------------------
        // Darwin I/O hints — all advisory, all ignored
        // ----------------------------------------------------------------
        F_NOCACHE => {
            let arg: i32 = args.start().next(env);
            log_dbg!("fcntl({}, F_NOCACHE, {}) — ignored", fd, arg);
        }
        F_RDADVISE => {
            log_dbg!("fcntl({}, F_RDADVISE) — ignored", fd);
        }
        F_RDAHEAD => {
            let arg: i32 = args.start().next(env);
            log_dbg!("fcntl({}, F_RDAHEAD, {}) — ignored", fd, arg);
        }
        F_PREALLOCATE => {
            log_dbg!("fcntl({}, F_PREALLOCATE) — ignored", fd);
        }
        F_TRUNCATEOVERSIZE => {
            let _size: i64 = args.start().next(env);
            log_dbg!("fcntl({}, F_TRUNCATEOVERSIZE) — ignored", fd);
        }
        F_SETSIZE => {
            let size: i64 = args.start().next(env);
            log_dbg!("fcntl({}, F_SETSIZE, {}) — ignored", fd, size);
        }
        F_FULLFSYNC => {
            log_dbg!("fcntl({}, F_FULLFSYNC) — no-op", fd);
        }
        F_BARRIERFSYNC => {
            log_dbg!("fcntl({}, F_BARRIERFSYNC) — no-op", fd);
        }
        F_GETPATH => {
            let buf: MutPtr<u8> = args.start().next(env);
            let path_opt = env
                .libc_state
                .posix_io
                .files
                .get(fd_to_file_idx(fd))
                .and_then(|s| s.as_ref())
                .and_then(|f| f.path.clone());
            if let Some(path) = path_opt {
                let bytes = path.as_bytes();
                let len = bytes.len().min(1023);
                let dst = env.mem.bytes_at_mut(buf, (len + 1) as u32);
                dst[..len].copy_from_slice(&bytes[..len]);
                dst[len] = 0;
            } else {
                log!("fcntl({}, F_GETPATH) — path unknown, zeroing buffer", fd);
                env.mem.bytes_at_mut(buf, 1024).fill(0);
            }
        }
        F_PATHPKG_CHECK => {
            log_dbg!("fcntl({}, F_PATHPKG_CHECK) — returning 0", fd);
        }
        F_CHKCLEAN => {
            log_dbg!("fcntl({}, F_CHKCLEAN) — returning 0", fd);
        }
        F_ADDSIGS
        | F_ADDFILESIGS
        | F_ADDFILESIGS_FOR_DYLD_SIM
        | F_ADDFILESIGS_RETURN
        | F_ADDFILESUPPL => {
            log_dbg!("fcntl({}, {:#x}) code-signing — ignored", fd, cmd);
        }
        F_SETNOSIGPIPE => {
            let arg: i32 = args.start().next(env);
            log_dbg!("fcntl({}, F_SETNOSIGPIPE, {}) — ignored", fd, arg);
        }
        F_GETNOSIGPIPE => {
            return 0;
        }
        _ => {
            log!(
                "Warning: fcntl({}, {:#x}) — unhandled cmd, returning -1",
                fd,
                cmd
            );
            set_errno(env, EINVAL);
            return -1;
        }
    }
    0
}

/// `int flock(int fd, int operation);` — BSD-style whole-file advisory
/// locking, as documented by Apple `man 2 flock`:
/// <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html>
///
/// `flock(2)` advisory locks only contend between *different* processes.
/// HyperHLE is a single guest process, so any number of locks within the
/// guest can coexist. We still validate the operation, track the lock
/// state per fd for diagnostics, and release on `close(2)`.
fn flock(env: &mut Environment, fd: FileDescriptor, operation: FLockFlag) -> i32 {
    set_errno(env, 0);

    if env.libc_state.posix_io.file_for_fd(fd).is_none() {
        set_errno(env, EBADF);
        return -1;
    }

    let op = operation & !LOCK_NB;
    let new_state: Option<i16> = match op {
        LOCK_UN => None,
        LOCK_SH => Some(F_RDLCK),
        LOCK_EX => Some(F_WRLCK),
        _ => {
            set_errno(env, EINVAL);
            return -1;
        }
    };

    if let Some(file) = env.libc_state.posix_io.file_for_fd(fd) {
        file.flock_state = new_state;
    }
    log_dbg!("flock({}, {}) => 0", fd, operation);
    0
}

pub fn fsync(env: &mut Environment, fd: FileDescriptor) -> i32 {
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        log!(
            "Warning: fsync({:?}) called with unknown fd, returning -1",
            fd
        );
        set_errno(env, EBADF);
        return -1;
    };

    match file.file.sync_all() {
        Ok(()) => 0,
        Err(error) => {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    log!(
                        "Warning: fsync({:?}) sync failed with error: {:?}, \
                         returning 0",
                        fd,
                        error
                    );
                    return 0;
                }
                std::io::ErrorKind::Unsupported => set_errno(env, EINVAL),
                std::io::ErrorKind::Interrupted => set_errno(env, EINTR),
                _ => set_errno(env, EIO),
            }

            log!(
                "Warning: fsync({:?}) sync failed with error: {:?}, returning -1",
                fd,
                error
            );
            -1
        }
    }
}

pub fn ftruncate(env: &mut Environment, fd: FileDescriptor, len: off_t) -> i32 {
    set_errno(env, 0);
    // POSIX: ftruncate(2) is only meaningful for regular files; on a socket it
    // fails with EINVAL.
    if is_socket(env, fd) {
        log_dbg!("ftruncate({}, {}) on socket => -1 (EINVAL)", fd, len);
        set_errno(env, EINVAL);
        return -1;
    }
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        set_errno(env, EBADF);
        return -1;
    };
    match file.file.set_len(len as u64) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn writev(
    env: &mut Environment,
    fd: FileDescriptor,
    iov: ConstPtr<iovec>,
    iovcnt: i32,
) -> GuestISize {
    let mut i = 0;
    let mut written_bytes: GuestISize = 0;
    while i != iovcnt {
        let iovec = env.mem.read(iov + i as u32);
        let bytes_written = write(env, fd, iovec.iov_base.cast(), iovec.iov_len);
        if bytes_written == -1 {
            return -1;
        }
        written_bytes += bytes_written;
        i += 1
    }
    written_bytes
}

fn truncate(env: &mut Environment, path_ptr: ConstPtr<u8>, len: off_t) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let path_string = match env.mem.cstr_at_utf8(path_ptr) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            return -1; // TODO: set errno
        }
    };

    let fd = open_direct(env, path_ptr, O_WRONLY);
    if fd < 0 {
        log_dbg!("truncate('{}', {}) => -1", path_string, len);
        return -1;
    }

    let res = ftruncate(env, fd, len);

    close(env, fd);

    log_dbg!("truncate('{}', {}) => {}", path_string, len, res);
    res
}

/// Оценить readiness одного fd для poll(2): возвращает `revents`.
/// Нормальные биты (POLLIN/POLLOUT/…) отдаются только если запросили их в
/// `events`; POLLERR/POLLHUP/POLLNVAL — всегда (POSIX).
fn evaluate_poll_fd(env: &mut Environment, fd: FileDescriptor, events: i16) -> i16 {
    if fd < 0 {
        // Отрицательные fd игнорируются: revents=0, в счёт не идут.
        return 0;
    }
    if fd < NORMAL_FILENO_BASE {
        // stdin/stdout/stderr: запись всегда возможна; данных для чтения
        // в HLE нет, поэтому POLLIN не сообщается.
        let mut revents = 0;
        if fd == STDOUT_FILENO || fd == STDERR_FILENO {
            revents |= POLLOUT | POLLWRNORM;
        }
        return revents & events;
    }
    if is_socket(env, fd) {
        return crate::libc::sys::socket::socket_poll_events(env, fd, events);
    }
    let Some(file_obj) = env.libc_state.posix_io.guest_file(fd) else {
        // Файл не открыт → POLLNVAL (не ошибка, а готовый fd, как в POSIX).
        return POLLNVAL;
    };
    // file_obj: &GuestFile (из guest_file).
    match file_obj {
        GuestFile::PipeRead(pipe) => {
            let pipe = pipe.borrow();
            let mut revents = 0;
            if pipe.poll_has_data() {
                revents |= POLLIN | POLLRDNORM;
            }
            if !pipe.poll_has_writers() {
                // Писателей нет → EOF на читающем конце (HUP не зависит
                // от events, но POLLIN сообщаем только если запросили).
                return (revents & events) | POLLHUP;
            }
            revents & events
        }
        GuestFile::PipeWrite(pipe) => {
            let pipe = pipe.borrow();
            if !pipe.poll_has_readers() {
                // Читателей нет → запись невозможна → POLLERR.
                return POLLERR;
            }
            (POLLOUT | POLLWRNORM) & events
        }
        // Обычные файлы/каталоги/случайные числа: POSIX гарантирует —
        // никогда не блокируются, всегда готовы и на чтение, и на запись.
        _ => (POLLIN | POLLOUT | POLLRDNORM | POLLWRNORM) & events,
    }
}

/// `int poll(struct pollfd *fds, nfds_t nfds, int timeout)` — полная
/// реализация: готовность сокетов (через socket_poll_events), обычных
/// файлов и pipe-концов; блокировка с yield'ом гостевого ранлайна
/// (env.sleep, как в select); записи `revents` всегда по возврату.
fn poll(env: &mut Environment, fds: MutPtr<PollFd>, nfds: u32, timeout: i32) -> i32 {
    set_errno(env, 0);
    if nfds > 0 && fds.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }
    // POSIX: EINVAL, если nfds больше {OPEN_MAX} (на iOS = 10240).
    if nfds > 10240 {
        set_errno(env, EINVAL);
        return -1;
    }

    // timeout < 0 — ждать бессрочно; 0 — ровно один опрос.
    let deadline = if timeout < 0 {
        None
    } else {
        Some(
            std::time::Instant::now() + std::time::Duration::from_millis(timeout as u64),
        )
    };
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    let mut revents = vec![0i16; nfds as usize];
    // count читается после цикла — инициализируем сразу, т.к. присваиваем
    // на каждой итерации (mut-переменная, ей же и управляется break).
    let mut count = 0i32;
    loop {
        let mut ready = 0i32;
        for i in 0..nfds {
            let pfd: PollFd = env.mem.read(fds + i);
            let re = evaluate_poll_fd(env, pfd.fd, pfd.events);
            revents[i as usize] = re;
            if re != 0 {
                ready += 1;
            }
        }
        count = ready;
        if ready > 0 || timeout == 0 {
            break;
        }
        // env.sleep() уступает гостевому ранлайну — остальные потоки идут.
        match deadline {
            Some(deadline) => {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                env.sleep(POLL_INTERVAL.min(deadline - now));
            }
            None => env.sleep(POLL_INTERVAL),
        }
    }

    // revents пишутся всегда (в том числе по таймауту — там нули).
    for i in 0..nfds {
        let p = fds + i;
        let pfd: PollFd = env.mem.read(p);
        // Собираем структуру заново: запись в поля упакованного struct
        // через place-выражение здесь не используется нарочно.
        env.mem.write(
            p,
            PollFd {
                fd: pfd.fd,
                events: pfd.events,
                revents: revents[i as usize],
            },
        );
    }
    count
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(open(_, _, _)),
    export_c_func!(open_dprotected_np(_, _, _, _, _)),
    export_c_func!(creat(_, _)),
    export_c_func!(truncate(_, _)),
    export_c_func!(read(_, _, _)),
    export_c_func!(pread(_, _, _, _)),
    export_c_func!(write(_, _, _)),
    export_c_func!(pwrite(_, _, _, _)),
    export_c_func!(lseek(_, _, _)),
    export_c_func!(close(_)),
    export_c_func!(dup(_)),
    export_c_func!(dup2(_, _)),
    export_c_func!(rename(_, _)),
    export_c_func!(getcwd(_, _)),
    export_c_func!(chdir(_)),
    export_c_func!(fcntl(_, _, _)),
    export_c_func!(poll(_, _, _)),
    export_c_func!(flock(_, _)),
    export_c_func!(fsync(_)),
    export_c_func!(ftruncate(_, _)),
    export_c_func!(writev(_, _, _)),
];

fn find_or_create_fd(env: &mut Environment, host_object: PosixFileHostObject) -> FileDescriptor {
    let idx = if let Some(free_idx) = env
        .libc_state
        .posix_io
        .files
        .iter()
        .position(|f| f.is_none())
    {
        env.libc_state.posix_io.files[free_idx] = Some(host_object);
        free_idx
    } else {
        let idx = env.libc_state.posix_io.files.len();
        env.libc_state.posix_io.files.push(Some(host_object));
        idx
    };
    file_idx_to_fd(idx)
}

pub fn find_or_create_pipe_read_fd(
    env: &mut Environment,
    buffer: std::rc::Rc<std::cell::RefCell<crate::fs::PipeBuffer>>,
) -> FileDescriptor {
    find_or_create_fd(
        env,
        PosixFileHostObject {
            file: GuestFile::PipeRead(buffer),
            needs_flush: false,
            reached_eof: false,
            flags: 0,
            status_flags: O_RDONLY,
            path: None,
            locks: Vec::new(),
            flock_state: None,
        },
    )
}

pub fn find_or_create_pipe_write_fd(
    env: &mut Environment,
    buffer: std::rc::Rc<std::cell::RefCell<crate::fs::PipeBuffer>>,
) -> FileDescriptor {
    find_or_create_fd(
        env,
        PosixFileHostObject {
            file: GuestFile::PipeWrite(buffer),
            needs_flush: false,
            reached_eof: false,
            flags: 0,
            status_flags: O_WRONLY,
            path: None,
            locks: Vec::new(),
            flock_state: None,
        },
    )
}

pub fn find_or_create_socket(env: &mut Environment) -> FileDescriptor {
    let host_object = PosixFileHostObject {
        file: GuestFile::Socket,
        needs_flush: false,
        reached_eof: false,
        flags: 0,
        status_flags: O_RDWR,
        path: None,
        locks: Vec::new(),
        flock_state: None,
    };
    find_or_create_fd(env, host_object)
}

pub fn is_socket(env: &mut Environment, fd: FileDescriptor) -> bool {
    if fd < NORMAL_FILENO_BASE {
        return false;
    }
    if let Some(Some(file_obj)) = env.libc_state.posix_io.files.get(fd_to_file_idx(fd)) {
        matches!(file_obj.file, GuestFile::Socket)
    } else {
        false
    }
}

/// Set or clear `O_NONBLOCK` on `fd`, like `fcntl(F_SETFL)` and
/// `ioctl(FIONBIO)` do.
///
/// The socket implementation honours this when deciding whether `read(2)`,
/// `write(2)` and `recvfrom(2)` may wait for the peer, so a socket switched to
/// non-blocking mode with `ioctl()` behaves the same as one switched with
/// `fcntl()`.
pub fn set_nonblocking(env: &mut Environment, fd: FileDescriptor, nonblocking: bool) {
    if fd < NORMAL_FILENO_BASE {
        return;
    }
    if let Some(Some(file_obj)) = env.libc_state.posix_io.files.get_mut(fd_to_file_idx(fd)) {
        if nonblocking {
            file_obj.status_flags |= O_NONBLOCK;
        } else {
            file_obj.status_flags &= !O_NONBLOCK;
        }
    }
}

/// Whether `fd` is open with `O_NONBLOCK` (set via `open()` or
/// `fcntl(F_SETFL)`).
///
/// The socket implementation uses this to decide whether `read(2)`, `write(2)`
/// or `recvfrom(2)` may wait for the peer or must report `EAGAIN` immediately
/// (see [crate::libc::sys::socket::read_socket]). Unknown and closed
/// descriptors are reported as blocking, which is the guest's default.
pub fn is_nonblocking(env: &Environment, fd: FileDescriptor) -> bool {
    if fd < NORMAL_FILENO_BASE {
        return false;
    }
    env.libc_state
        .posix_io
        .files
        .get(fd_to_file_idx(fd))
        .and_then(|file_or_none| file_or_none.as_ref())
        .is_some_and(|file_obj| file_obj.status_flags & O_NONBLOCK != 0)
}

/// Resolve an `flock` request to an absolute byte range
/// `[start, end)` (with `end == i64::MAX` for "to EOF / unlimited").
///
/// Mirrors the logic in `validate_lock` but returns the resolved range so the
/// fcntl(F_SETLK/F_SETLKW/F_GETLK) handlers can store it. Returns the same
/// error codes as `validate_lock`.
fn resolve_lock_range(
    env: &mut Environment,
    fd: FileDescriptor,
    lock: &flock,
) -> Result<LockRange, i32> {
    let lock_type = lock.lock_type;
    if !matches!(lock_type, F_RDLCK | F_UNLCK | F_WRLCK) {
        return Err(EINVAL);
    }

    let whence = lock.whence as i32;
    let lock_start = match whence {
        SEEK_SET => lock.start,
        SEEK_CUR => {
            let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
                return Err(EBADF);
            };
            let file_position = file.file.stream_position().unwrap_or(0);
            file_position as i64 + lock.start
        }
        SEEK_END => {
            let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
                return Err(EBADF);
            };
            let size: i64 = file.file.stream_len().unwrap_or(0).try_into().unwrap_or(0);
            size + lock.start
        }
        _ => return Err(EINVAL),
    };

    if lock_start < 0 {
        return Err(EINVAL);
    }

    // POSIX: `len == 0` means "lock from `start` to end of file / no
    // upper bound". Negative `len` means the range extends backwards from
    // `start` — also valid per Apple `man 2 fcntl`.
    let (start, end) = if lock.len == 0 {
        (lock_start, i64::MAX)
    } else if lock.len > 0 {
        (
            lock_start,
            lock_start.checked_add(lock.len).ok_or(EOVERFLOW)?,
        )
    } else {
        let new_start = lock_start.checked_add(lock.len).ok_or(EINVAL)?;
        if new_start < 0 {
            return Err(EINVAL);
        }
        (new_start, lock_start)
    };

    Ok(LockRange {
        start,
        end,
        lock_type,
    })
}

/// Drop the portion of every range in `locks` that overlaps `release`,
/// splitting ranges as necessary. Used to implement F_UNLCK (which may
/// release a sub-range of a previously-held lock).
fn release_range_from_locks(locks: &mut Vec<LockRange>, release: &LockRange) {
    let mut new_locks: Vec<LockRange> = Vec::with_capacity(locks.len());
    for existing in locks.drain(..) {
        if !existing.overlaps(release) {
            new_locks.push(existing);
            continue;
        }
        // Keep the portion strictly before `release`, if any.
        if existing.start < release.start {
            new_locks.push(LockRange {
                start: existing.start,
                end: release.start,
                lock_type: existing.lock_type,
            });
        }
        // Keep the portion strictly after `release`, if any.
        if existing.end > release.end {
            new_locks.push(LockRange {
                start: release.end,
                end: existing.end,
                lock_type: existing.lock_type,
            });
        }
    }
    *locks = new_locks;
}

#[cfg(test)]
mod tests {
    use super::{path_after_app_bundle_component, strip_udk_game_bundle_prefix};

    #[test]
    fn finds_case_insensitive_complete_app_bundle_components() {
        assert_eq!(
            path_after_app_bundle_component(
                "/var/mobile/Applications/old-id/GRANNY.APP/Data/data.unity3d"
            ),
            Some("Data/data.unity3d")
        );
        assert_eq!(
            path_after_app_bundle_component("/var/mobile/Applications/old-id/Granny.app"),
            Some("")
        );
        assert_eq!(
            path_after_app_bundle_component(
                "/var/mobile/Applications/old-id/Granny.app.backup/Data/data.unity3d"
            ),
            None
        );
    }

    #[test]
    fn maps_unreal_cooked_paths_from_project_root_to_bundle_root() {
        assert_eq!(
            strip_udk_game_bundle_prefix("../UDKGame/CookedIPhone/Core.xxx").as_deref(),
            Some("CookedIPhone/Core.xxx")
        );
        assert_eq!(
            strip_udk_game_bundle_prefix(r"..\udkgame\CookedIPhone\Core.xxx").as_deref(),
            Some("CookedIPhone/Core.xxx")
        );
        assert_eq!(
            strip_udk_game_bundle_prefix("../OtherGame/CookedIPhone/Core.xxx"),
            None
        );
    }
}
