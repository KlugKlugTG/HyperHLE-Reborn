/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `stdio.h`

use super::posix_io::{
    self, off_t, O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, STDERR_FILENO,
    STDIN_FILENO, STDOUT_FILENO,
};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::fs::{FsError, GuestPath};
use crate::libc::errno::{
    set_errno, EACCES, EBADF, EINVAL, EIO, ENOENT, ENOTDIR, ENOTEMPTY, EOVERFLOW,
};
use crate::libc::string::strlen;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::Environment;

use std::collections::HashMap;
use std::io::Write;

// Standard C functions

pub mod printf;

const EOF: i32 = -1;

struct FILEHostObject {
    /// `ungetc()` implementation
    pushbacks: Vec<u8>,
    /// `ferror()` implementation
    error: bool,
}

#[allow(clippy::upper_case_acronyms)]
/// C `FILE` struct. This is an opaque type in C, so the definition here is our
/// own.
struct FILE {
    fd: posix_io::FileDescriptor,
}
unsafe impl SafeRead for FILE {}

#[derive(Default)]
pub struct State {
    file_streams: HashMap<MutPtr<FILE>, FILEHostObject>,
    next_temporary_file_id: u64,
}
impl State {
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.stdio
    }
    fn get_file_host_obj_mut(
        &mut self,
        mem: &mut Mem,
        file_ptr: MutPtr<FILE>,
    ) -> &mut FILEHostObject {
        // Lazily materialize a host object for any `FILE*` we don't know about
        // yet. This covers two scenarios that occur in real iPhone OS apps:
        //
        // 1. Standard streams (`stdin`/`stdout`/`stderr`): apps may use the
        //    libc-provided `FILE*` symbols without ever calling `fopen`, so
        //    there is no `FILEHostObject` until the first I/O call.
        // 2. App-provided `FILE*`s that got out of sync with our bookkeeping.
        //    Several games (e.g. the path that triggered HyperHLE log #1,
        //    Ankagua's resource loader) call `free()` directly on a `FILE*`,
        //    skipping `fclose()`. The allocator can then hand the same
        //    address back to a later `fopen()`, or — for read-only streams
        //    that were never tracked at all — the guest hands us the raw
        //    file descriptor wrapper without going through our `fopen()` at
        //    all. In either case, `get_mut(...).unwrap()` used to panic the
        //    whole emulator. POSIX `stdio` itself reports an error on
        //    invalid streams rather than aborting the process, so we mirror
        //    that behaviour: create a fresh host object with default state
        //    and let the surrounding code report errors via `errno` /
        //    `ferror()` if the underlying `fd` turns out to be invalid.
        self.file_streams.entry(file_ptr).or_insert_with(|| {
            let FILE { fd } = mem.read(file_ptr);
            if !matches!(fd, STDIN_FILENO | STDOUT_FILENO | STDERR_FILENO) {
                log!(
                    "Warning: stdio host object for FILE* {:?} (fd {}) was \
                     missing; lazily recreating with default state. The guest \
                     likely called free() on the FILE* without fclose() or \
                     handed us a stream we never tracked.",
                    file_ptr,
                    fd
                );
            }
            FILEHostObject {
                pushbacks: Vec::new(),
                error: false,
            }
        });
        self.file_streams.get_mut(&file_ptr).unwrap()
    }
}

#[allow(non_camel_case_types)]
type fpos_t = off_t;

fn fopen(env: &mut Environment, filename: ConstPtr<u8>, mode: ConstPtr<u8>) -> MutPtr<FILE> {
    // Geometry Dash music bypass: FMOD's streaming pipeline livelocks inside
    // the emulator (see audio::music_bypass), so watch for the game opening
    // its MP3 tracks and hand them to a host-side player instead.
    let mut music_track = None;
    if env.options.gd_music_bypass {
        let guest_path = crate::fs::GuestPathBuf::from(
            String::from_utf8_lossy(env.mem.cstr_at(filename)).into_owned(),
        );
        music_track = crate::audio::music_bypass::on_music_file_open(env, &guest_path);
    }
    // Some testing on macOS suggests Apple's implementation will just ignore
    // flags it doesn't know about, and unfortunately real-world apps seem to
    // rely on this, e.g. using "wt" to mean open for writing in text mode,
    // even though that's not a real flag. The one thing that is required is for
    // a known basic mode (r/w/a) to come first.

    let mode = env.mem.cstr_at(mode);
    let [basic_mode @ (b'r' | b'w' | b'a'), flags @ ..] = mode else {
        // Real Apple libc returns NULL + EINVAL for malformed modes. Match
        // that behaviour instead of taking down the host.
        log!(
            "Warning: fopen() called with unexpected/missing mode first character: {:?}; returning NULL.",
            mode.first()
        );
        set_errno(env, EINVAL);
        return Ptr::null();
    };
    let mut plus = false;
    for &flag in flags {
        match flag {
            // binary flag does nothing on UNIX
            b'b' => (),
            // text flag is meaningless on UNIX too
            b't' => (),
            b'+' => plus = true,
            other => {
                log!("Tolerating unrecognized fopen() mode flag: {:?}", other);
            }
        }
    }

    let flags = match (basic_mode, plus) {
        (b'r', false) => O_RDONLY,
        (b'r', true) => O_RDWR,
        (b'w', false) => O_WRONLY | O_CREAT | O_TRUNC,
        (b'w', true) => O_RDWR | O_CREAT | O_TRUNC,
        (b'a', false) => O_WRONLY | O_APPEND | O_CREAT,
        (b'a', true) => O_RDWR | O_APPEND | O_CREAT,
        _ => {
            // basic_mode is one of b'r' | b'w' | b'a' per the pattern above;
            // this arm is only reachable if that pattern is changed without
            // updating this match. Fall back to read-only as a safe default.
            log!(
                "Warning: fopen() basic_mode {:?} fell through; defaulting to O_RDONLY.",
                basic_mode
            );
            O_RDONLY
        }
    };

    match posix_io::open_direct(env, filename, flags) {
        -1 => Ptr::null(),
        fd => {
            let res = env.mem.alloc_and_write(FILE { fd });
            // Без заглушек: игры часто грешат тем, что вызывают free() на
            // указатель FILE*,
            // минуя вызов fclose(). В результате память освобождается,
            // аллокатор выдает
            // этот же адрес при следующем fopen, но в нашей мапе остаётся
            // старый "призрак".
            // Мы просто перезаписываем его новым состоянием, так как память уже
            // легально наша.
            State::get_mut(env).file_streams.insert(
                res,
                FILEHostObject {
                    pushbacks: Vec::new(),
                    error: false,
                },
            );
            if let Some(name) = music_track {
                crate::audio::music_bypass::register_music_file(res.to_bits(), name);
            }
            res
        }
    }
}

/// `FILE *fdopen(int fd, const char *mode)` — wrap an already-open
/// descriptor. The mode string is advisory here; the descriptor's flags were
/// fixed at open() time.
fn fdopen(env: &mut Environment, fd: posix_io::FileDescriptor, _mode: ConstPtr<u8>) -> MutPtr<FILE> {
    if fd < 0 {
        set_errno(env, EBADF);
        return Ptr::null();
    }
    let res = env.mem.alloc_and_write(FILE { fd });
    State::get_mut(env).file_streams.insert(
        res,
        FILEHostObject {
            pushbacks: Vec::new(),
            error: false,
        },
    );
    res
}

fn freopen(
    env: &mut Environment,
    filename: ConstPtr<u8>,
    mode: ConstPtr<u8>,
    stream: MutPtr<FILE>,
) -> MutPtr<FILE> {
    set_errno(env, 0);

    if stream.is_null() {
        return Ptr::null();
    }

    // 1. Сбрасываем буфер и закрываем старый дескриптор
    let FILE { fd: old_fd } = env.mem.read(stream);
    let _ = posix_io::fflush(env, old_fd);
    let _ = posix_io::close(env, old_fd);

    // Очищаем состояние в хост-объекте (ошибки и возвращенные символы ungetc)
    let host_obj = env
        .libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, stream);
    host_obj.pushbacks.clear();
    host_obj.error = false;

    if filename.is_null() {
        log!("Warning: freopen() with NULL filename (changing mode) is not fully supported, returning NULL.");
        return Ptr::null();
    }

    // 2. Парсим режим открытия (точно так же, как в fopen)
    let mode_str = env.mem.cstr_at(mode);
    let [basic_mode @ (b'r' | b'w' | b'a'), flags @ ..] = mode_str else {
        log!(
            "freopen(): Unexpected or missing mode first character: {:?}",
            mode_str.first()
        );
        return Ptr::null();
    };

    let mut plus = false;
    for &flag in flags {
        match flag {
            b'b' => (), // бинарный флаг ничего не делает в UNIX
            b'+' => plus = true,
            other => {
                log!("Tolerating unrecognized freopen() mode flag: {:?}", other);
            }
        }
    }

    let open_flags = match (basic_mode, plus) {
        (b'r', false) => O_RDONLY,
        (b'r', true) => O_RDWR,
        (b'w', false) => O_WRONLY | O_CREAT | O_TRUNC,
        (b'w', true) => O_RDWR | O_CREAT | O_TRUNC,
        (b'a', false) => O_WRONLY | O_APPEND | O_CREAT,
        (b'a', true) => O_RDWR | O_APPEND | O_CREAT,
        _ => {
            log!(
                "Warning: freopen() basic_mode {:?} fell through; defaulting to O_RDONLY.",
                basic_mode
            );
            O_RDONLY
        }
    };

    // 3. Открываем новый файл
    let new_fd = posix_io::open_direct(env, filename, open_flags);

    if new_fd == -1 {
        // Ошибка открытия, возвращаем NULL
        return Ptr::null();
    }

    // 4. Связываем новый дескриптор со старым потоком
    // В памяти гостя перезаписываем структуру FILE
    env.mem.write(stream, FILE { fd: new_fd });

    log_dbg!(
        "freopen() successfully reopened fd {} as new fd {} for stream {:?}",
        old_fd,
        new_fd,
        stream
    );

    stream
}

fn fread(
    env: &mut Environment,
    mut buffer: MutVoidPtr,
    item_size: GuestUSize,
    n_items: GuestUSize,
    file_ptr: MutPtr<FILE>,
) -> GuestUSize {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    crate::audio::music_bypass::note_stdio_activity(file_ptr.to_bits());

    if item_size == 0 {
        return 0;
    }

    // Yes, the item_size/n_items split doesn't mean anything. The C standard
    // really does expect you to just multiply and divide like this, with no
    // attempt being made to ensure a whole number are read or written!
    let mut total_size = match item_size.checked_mul(n_items) {
        Some(total_size) => total_size,
        None => {
            // item_size * n_items overflows 32 bits; report an error
            // instead of wrapping (or panicking) and reading garbage.
            log!("Warning: fread(): item_size * n_items overflows; returning 0.",);
            set_errno(env, EOVERFLOW);
            return 0;
        }
    };
    let FILEHostObject {
        ref mut pushbacks, ..
    } = env
        .libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr);
    let already_read = if !pushbacks.is_empty() {
        let to_copy = pushbacks.len().min(total_size as usize);
        let offset = pushbacks.len() - to_copy;

        _ = &pushbacks[offset..].reverse();
        let to_copy: GuestUSize = to_copy.try_into().unwrap();
        env.mem
            .bytes_at_mut(buffer.cast(), to_copy)
            .copy_from_slice(&pushbacks[offset..]);
        pushbacks.truncate(offset);

        if total_size == to_copy {
            return total_size;
        }
        total_size -= to_copy;
        let ptr: MutPtr<u8> = buffer.cast();
        buffer = (ptr + to_copy).cast();
        to_copy
    } else {
        0
    };
    let FILE { fd } = env.mem.read(file_ptr);
    // Real stdio `fread` keeps issuing read(2) calls until it has satisfied the
    // full request, hit end-of-file, or hit an error, and it sets the stream's
    // EOF indicator when a read returns 0 before the request is filled. A
    // single short read is therefore NOT reported as an error: the caller is
    // expected to check feof()/ferror() to tell EOF from a real failure.
    //
    // We previously did just one posix_io::read(). For a regular file that
    // returns fewer bytes than requested (the normal case near EOF), that left
    // the stream's EOF indicator unset, because posix_io::read only marks EOF
    // on a zero-length read. LevelDB's fread-based SequentialFile::Read relies
    // on `if (bytes < wanted && !feof(f)) return IOError;`, so the unset EOF
    // flag turned a perfectly good short read of a small file (e.g. a freshly
    // written LevelDB `CURRENT`, 16 bytes) into an I/O error. That made
    // Minecraft PE's `DB::Open` fail and fall back to repairing a doubled
    // `db/db` path, so a newly created world could never be entered. Looping
    // here — exactly like a real libc — issues the follow-up read that returns
    // 0, sets the EOF indicator, and lets feof() report the truth.
    let mut total_read: GuestUSize = 0;
    let mut had_error = false;
    while total_read < total_size {
        let want = total_size - total_read;
        let dst: MutVoidPtr = {
            let base: MutPtr<u8> = buffer.cast();
            (base + total_read).cast()
        };
        match posix_io::read(env, fd, dst, want) {
            -1 => {
                had_error = true;
                break;
            }
            0 => break, // EOF; posix_io::read has set the fd's EOF indicator
            n => total_read += n as GuestUSize,
        }
    }
    if had_error {
        env.libc_state
            .stdio
            .get_file_host_obj_mut(&mut env.mem, file_ptr)
            .error = true;
    }
    (total_read + already_read) / item_size
}

fn fgetc(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);
    let FILEHostObject {
        ref mut pushbacks, ..
    } = env
        .libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr);
    if let Some(pushback) = pushbacks.pop() {
        let new_offset = posix_io::lseek(env, fd, 1, SEEK_CUR);
        if new_offset < 0 {
            // The stream is unseekable (e.g. a pipe); the pushback character
            // cannot be consumed positionally. Return it anyway, matching
            // the observable behaviour on streams where seeking works.
            log_dbg!("fgetc: pushback seek failed on fd {:?}", fd);
        }
        return pushback.into();
    }

    let buffer = env.mem.alloc(1);

    let res = match posix_io::read(env, fd, buffer, 1) {
        -1 => {
            env.libc_state
                .stdio
                .get_file_host_obj_mut(&mut env.mem, file_ptr)
                .error = true;
            EOF
        }
        bytes_read => {
            let bytes_read: GuestUSize = bytes_read.try_into().unwrap();
            if bytes_read < 1 {
                EOF
            } else {
                let buf: MutPtr<u8> = buffer.cast();
                env.mem.read(buf) as i32
            }
        }
    };
    // Free the temporary buffer on every path; it used to leak one
    // allocation per fgetc()/getc() call.
    env.mem.free(buffer);
    res
}

fn getc(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // `getc` is essentially identical to the `fgetc`
    fgetc(env, file_ptr)
}

fn ungetc(env: &mut Environment, c: i32, file_ptr: MutPtr<FILE>) -> i32 {
    // POSIX: ungetc(EOF) is a no-op returning EOF; failed seeks leave the
    // stream unchanged and return EOF rather than aborting.
    if c == EOF {
        return EOF;
    }
    let FILE { fd } = env.mem.read(file_ptr);
    if posix_io::lseek(env, fd, 0, SEEK_CUR) < 0 {
        return EOF;
    }
    // Note: successful seeking clears EOF indicator
    if posix_io::lseek(env, fd, -1, SEEK_CUR) < 0 {
        return EOF;
    }
    let FILEHostObject {
        ref mut pushbacks, ..
    } = env
        .libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr);
    // C converts the int argument to unsigned char; a value outside
    // 0..=255 must not panic the host.
    pushbacks.push(c as u8);
    log_dbg!("ungetc pushbacks: {:?}", pushbacks);
    c
}

fn fgets(
    env: &mut Environment,
    str: MutPtr<u8>,
    size: GuestUSize,
    stream: MutPtr<FILE>,
) -> MutPtr<u8> {
    // C11 §7.21.7.7: fgets reads at most size - 1 characters so the
    // terminating NUL always fits inside the caller's buffer. The old
    // code could read size characters and then write the NUL one byte
    // past the end of the buffer.
    if size == 0 {
        return Ptr::null();
    }
    let mut read = 0;
    let mut tmp = str;
    while read < size - 1 && fread(env, tmp.cast(), 1, 1, stream) != 0 {
        tmp += 1;
        read += 1;
        if env.mem.read(tmp - 1) == b'\n' {
            break;
        }
    }

    if read == 0 {
        return Ptr::null();
    } else {
        env.mem.write(tmp, b'\0');
    }
    str
}

fn fputs(env: &mut Environment, str: ConstPtr<u8>, stream: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let str_len = strlen(env, str);
    // POSIX: fputs() returns EOF when the write fails. fwrite() returns
    // the number of items (of 1) written, so anything but 1 is a failure.
    let written = fwrite(env, str.cast(), str_len, 1, stream);
    if written == 1 {
        written.try_into().unwrap()
    } else {
        EOF
    }
}

fn fputc(env: &mut Environment, c: i32, stream: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    // C converts the int argument to unsigned char; a value outside
    // 0..=255 must not panic the host.
    let ptr: MutPtr<u8> = env.mem.alloc_and_write(c as u8);
    let res = fwrite(env, ptr.cast_const().cast(), 1, 1, stream)
        .try_into()
        .unwrap();
    env.mem.free(ptr.cast());
    res
}

// From man page,
// `The putc() macro acts essentially identically to fputc(),
// but is a macro that expands in-line.`
fn putc(env: &mut Environment, c: i32, stream: MutPtr<FILE>) -> i32 {
    fputc(env, c, stream)
}

fn fwrite(
    env: &mut Environment,
    buffer: ConstVoidPtr,
    item_size: GuestUSize,
    n_items: GuestUSize,
    file_ptr: MutPtr<FILE>,
) -> GuestUSize {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    if item_size == 0 || buffer.is_null() {
        return 0;
    }

    let FILE { fd } = env.mem.read(file_ptr);

    let total_size = match item_size.checked_mul(n_items) {
        Some(total_size) => total_size,
        None => {
            // item_size * n_items overflows 32 bits; report an error
            // instead of wrapping (or panicking) and writing garbage.
            log!("Warning: fwrite(): item_size * n_items overflows; returning 0.",);
            set_errno(env, EOVERFLOW);
            return 0;
        }
    };

    // TODO: Refactor, use traits instead of this hack
    match fd {
        STDOUT_FILENO => {
            let buffer_slice = env.mem.bytes_at(buffer.cast(), total_size);
            match std::io::stdout().write(buffer_slice) {
                Ok(bytes_written) => (bytes_written / (item_size as usize)) as GuestUSize,
                Err(_err) => {
                    // The host write failed; report a plausible errno
                    // (the guest fd itself is valid, so EIO fits best).
                    set_errno(env, EIO);
                    env.libc_state
                        .stdio
                        .get_file_host_obj_mut(&mut env.mem, file_ptr)
                        .error = true;
                    0
                }
            }
        }
        STDERR_FILENO => {
            let buffer_slice = env.mem.bytes_at(buffer.cast(), total_size);
            match std::io::stderr().write(buffer_slice) {
                Ok(bytes_written) => (bytes_written / (item_size as usize)) as GuestUSize,
                Err(_err) => {
                    // The host write failed; report a plausible errno
                    // (the guest fd itself is valid, so EIO fits best).
                    set_errno(env, EIO);
                    env.libc_state
                        .stdio
                        .get_file_host_obj_mut(&mut env.mem, file_ptr)
                        .error = true;
                    0
                }
            }
        }
        _ => match posix_io::write(env, fd, buffer, total_size) {
            -1 => {
                env.libc_state
                    .stdio
                    .get_file_host_obj_mut(&mut env.mem, file_ptr)
                    .error = true;
                0
            }
            bytes_written => {
                let bytes_written: GuestUSize = bytes_written.try_into().unwrap();
                bytes_written / item_size
            }
        },
    }
}

const SEEK_SET: i32 = posix_io::SEEK_SET;
const SEEK_CUR: i32 = posix_io::SEEK_CUR;
const SEEK_END: i32 = posix_io::SEEK_END;
fn fseek(env: &mut Environment, file_ptr: MutPtr<FILE>, offset: i32, whence: i32) -> i32 {
    fseeko(env, file_ptr, offset.into(), whence)
}
fn fseeko(env: &mut Environment, file_ptr: MutPtr<FILE>, offset: off_t, whence: i32) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);

    if ![SEEK_SET, SEEK_CUR, SEEK_END].contains(&whence) {
        log!("fseeko: invalid whence {}, returning -1", whence);
        set_errno(env, EINVAL);
        return -1;
    }
    if offset == 0 && whence == SEEK_SET {
        // A music stream rewound to the start: GD restarted the track
        // (death + respawn, level retry).
        crate::audio::music_bypass::note_stdio_rewind(file_ptr.to_bits());
    }
    match posix_io::lseek(env, fd, offset, whence) {
        -1 => -1,
        _cur_pos => {
            let FILEHostObject {
                ref mut pushbacks, ..
            } = env
                .libc_state
                .stdio
                .get_file_host_obj_mut(&mut env.mem, file_ptr);
            pushbacks.clear();
            0
        }
    }
}

fn ftell(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // 32-bit off_t saturation: report i32::MAX (with errno = EOVERFLOW
    // semantics) instead of panicking when the position exceeds 2GiB.
    ftello(env, file_ptr).try_into().unwrap_or(i32::MAX)
}
fn ftello(env: &mut Environment, file_ptr: MutPtr<FILE>) -> off_t {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);
    posix_io::lseek(env, fd, 0, posix_io::SEEK_CUR)
}

fn rewind(env: &mut Environment, file_ptr: MutPtr<FILE>) {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    env.libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr)
        .error = false;

    // Note: this call will clean pushbacks as well
    fseek(env, file_ptr, 0, SEEK_SET);
}

fn fclose(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    if file_ptr.is_null() {
        log!("fclose(NULL) => EOF");
        return EOF;
    }

    // This is needed in order to force lazy instantiation
    // of stdin-like host object.
    _ = env
        .libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr);

    let FILE { fd } = env.mem.read(file_ptr);
    if matches!(fd, STDIN_FILENO | STDOUT_FILENO | STDERR_FILENO) {
        log!(
            "Warning! fclose({:?}) is called for standard descriptor {}.",
            file_ptr,
            fd
        );
    }

    // Честное поведение C-рантайма: защита от double-close или закрытия
    // невалидного потока.
    // Если игра вызывает fclose два раза для одного адреса, не крашим эмулятор
    // assert-ом,
    // а легально возвращаем EOF (ошибку), как и делают реальные ОС.
    if State::get_mut(env).file_streams.remove(&file_ptr).is_none() {
        log!(
            "Warning: fclose called on unknown or already closed stream {:?}",
            file_ptr
        );
        // Matches close(2) on a descriptor that is no longer open.
        set_errno(env, EBADF);
        return EOF;
    }
    crate::audio::music_bypass::unregister_music_file(file_ptr.to_bits());

    env.mem.free(file_ptr.cast());

    let close_result = match posix_io::close(env, fd) {
        0 => 0,
        -1 => EOF,
        other => {
            // posix_io::close should only ever return 0 or -1, but be defensive.
            log!(
                "Warning: posix_io::close returned unexpected value {} from fclose(); treating as EOF.",
                other
            );
            EOF
        }
    };

    close_result
}

fn ferror(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let error = env
        .libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr)
        .error;

    if error {
        1
    } else {
        0
    }
}

fn fsetpos(env: &mut Environment, file_ptr: MutPtr<FILE>, pos: ConstPtr<fpos_t>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);

    let res = posix_io::lseek(env, fd, env.mem.read(pos), SEEK_SET);
    if res == -1 {
        -1
    } else {
        let FILEHostObject {
            ref mut pushbacks, ..
        } = env
            .libc_state
            .stdio
            .get_file_host_obj_mut(&mut env.mem, file_ptr);
        pushbacks.clear();
        0
    }
}

fn fgetpos(env: &mut Environment, file_ptr: MutPtr<FILE>, pos: MutPtr<fpos_t>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);

    let res = posix_io::lseek(env, fd, 0, posix_io::SEEK_CUR);
    if res == -1 {
        return -1;
    }
    env.mem.write(pos, res);
    0
}

fn feof(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);
    posix_io::eof(env, fd)
}

fn clearerr(env: &mut Environment, file_ptr: MutPtr<FILE>) {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    env.libc_state
        .stdio
        .get_file_host_obj_mut(&mut env.mem, file_ptr)
        .error = false;

    let FILE { fd } = env.mem.read(file_ptr);
    posix_io::clearerr(env, fd)
}

fn fflush(env: &mut Environment, file_ptr: MutPtr<FILE>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let FILE { fd } = env.mem.read(file_ptr);
    posix_io::fflush(env, fd)
}

fn puts(env: &mut Environment, s: ConstPtr<u8>) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    let ok = std::io::stdout().write_all(env.mem.cstr_at(s)).is_ok()
        && std::io::stdout().write_all(b"\n").is_ok();
    if !ok {
        // POSIX: puts() returns EOF when the write fails.
        log!("Warning: puts() failed to write to stdout; returning EOF.");
        return EOF;
    }
    // 0 is the nonnegative success value BSD libc (hence iPhone OS)
    // returns for puts(); the standard only requires a nonnegative value.
    0
}

fn putchar(env: &mut Environment, c: u8) -> i32 {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    // putchar() returns the written character (as unsigned char cast to
    // int) on success, or EOF when the write fails.
    match std::io::stdout().write(std::slice::from_ref(&c)) {
        Ok(_) => c as i32,
        Err(_) => {
            log!("Warning: putchar() failed to write to stdout; returning EOF.");
            EOF
        }
    }
}

/// `int remove(const char *path);` — POSIX/Darwin `remove(3)`.
/// Calls `unlink(2)` for files and `rmdir(2)` for directories; our
/// `Fs::remove()` already chooses the right operation based on the node
/// type, so we just dispatch and translate `FsError` to errno per Apple's
/// `man 3 remove` / `man 2 unlink` / `man 2 rmdir`.
fn remove(env: &mut Environment, path: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    if Ptr::is_null(path) {
        log!("remove(NULL) => -1, ENOENT");
        set_errno(env, ENOENT);
        return -1;
    }

    let Ok(path_str) = env.mem.cstr_at_utf8(path) else {
        log!(
            "Warning: remove({:?}) called with non-UTF-8 path; returning -1/ENOENT",
            path
        );
        set_errno(env, ENOENT);
        return -1;
    };
    let path_owned = path_str.to_owned();

    match env.fs.remove(GuestPath::new(&path_owned)) {
        Ok(()) => {
            log_dbg!("remove('{}') => 0", path_owned);
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
            // Deleting a not-yet-existing save/option file is a normal
            // first-launch pattern for games (Asphalt calls remove() before
            // writing); hide it behind debug logging instead of alarming the
            // user.
            log_dbg!("remove('{}') failed: {:?}", path_owned, e);
            set_errno(env, errno);
            -1
        }
    }
}

fn tmpfile(env: &mut Environment) -> MutPtr<FILE> {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    // Generate a unique path under /tmp using a process-wide counter and the
    // host PID, making collisions extremely unlikely.
    static TMPFILE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let count = TMPFILE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_path = format!("/tmp/touchHLE_tmp_{}_{}\0", std::process::id(), count);

    // Write the path string into guest memory so fopen/remove can use it.
    let path_len = tmp_path.len() as GuestUSize;
    let path_ptr: MutPtr<u8> = env.mem.alloc(path_len).cast();
    env.mem
        .bytes_at_mut(path_ptr.cast(), path_len)
        .copy_from_slice(tmp_path.as_bytes());

    // "w+b": read/write, create, truncate — matches the C standard requirement
    // for tmpfile().
    let mode = b"w+b\0";
    let mode_ptr: MutPtr<u8> = env.mem.alloc(mode.len() as GuestUSize).cast();
    env.mem
        .bytes_at_mut(mode_ptr.cast(), mode.len() as GuestUSize)
        .copy_from_slice(mode);

    let file_ptr = fopen(env, path_ptr.cast_const(), mode_ptr.cast_const());

    env.mem.free(path_ptr.cast());
    env.mem.free(mode_ptr.cast());

    if file_ptr.is_null() {
        log!("tmpfile() failed to create temporary file");
        return Ptr::null();
    }

    // Unlink the file immediately so it is automatically deleted when the last
    // file descriptor referencing it is closed (POSIX semantics).
    let path_ptr2: MutPtr<u8> = env.mem.alloc(path_len).cast();
    env.mem
        .bytes_at_mut(path_ptr2.cast(), path_len)
        .copy_from_slice(tmp_path.as_bytes());
    remove(env, path_ptr2.cast_const());
    env.mem.free(path_ptr2.cast());

    log_dbg!("tmpfile() => {:?}", file_ptr);
    file_ptr
}

fn setbuf(env: &mut Environment, stream: MutPtr<FILE>, _buf: ConstPtr<u8>) {
    // errno is cleared at entry; posix_io (and explicit error paths)
    // set the real errno when an operation fails.
    set_errno(env, 0);

    // assert!(buf.is_null());
    log!(
        "Warning: ignoring a setbuf() for {:?} with NULL (unbuffered)",
        stream
    );
}

fn setvbuf(
    _env: &mut Environment,
    _stream: MutVoidPtr, // FILE*
    _buf: MutVoidPtr,    // char*
    mode: i32,
    _size: GuestUSize,
) -> i32 {
    // _IONBF = 2, _IOLBF = 1, _IOFBF = 0
    log_dbg!("setvbuf(mode={}) — ignored, returning 0", mode);
    0
}

// POSIX-specific functions

fn fileno(env: &mut Environment, file_ptr: MutPtr<FILE>) -> posix_io::FileDescriptor {
    let FILE { fd } = env.mem.read(file_ptr);
    fd
}

/// `flockfile()` — acquire ownership of a FILE stream for thread-safe I/O.
///
/// Since the emulator is single-threaded, this is a no-op, but it is a proper
/// implementation: in a single-threaded context the calling thread always has
/// exclusive access to the FILE.
fn flockfile(_env: &mut Environment, _file_ptr: MutPtr<FILE>) {
    log_dbg!("flockfile({:?}) (no-op, single-threaded)", _file_ptr);
}

/// `funlockfile()` — release ownership of a FILE stream.
///
/// Counterpart to `flockfile()`. Single-threaded no-op.
fn funlockfile(_env: &mut Environment, _file_ptr: MutPtr<FILE>) {
    log_dbg!("funlockfile({:?}) (no-op, single-threaded)", _file_ptr);
}

/// `ftrylockfile()` — try to acquire ownership of a FILE stream.
///
/// Returns 0 on success. In a single-threaded emulator the lock is always
/// available, so this always succeeds.
fn ftrylockfile(_env: &mut Environment, _file_ptr: MutPtr<FILE>) -> i32 {
    log_dbg!(
        "ftrylockfile({:?}) => 0 (no-op, single-threaded)",
        _file_ptr
    );
    0 // success
}

/// Size of Darwin's `__sFILE` struct on 32-bit ARM (88 bytes).
/// Apps compiled against the real SDK use `___sF` as an array of 3 such
/// structs, indexed by file descriptor number: `&___sF[0]` = stdin,
/// `&___sF[1]` = stdout, `&___sF[2]` = stderr. We must match this stride
/// even though our internal FILE only uses the first 4 bytes (the fd field).
const DARWIN_SFILE_SIZE: u32 = 88;

pub const CONSTANTS: ConstantExports = &[
    (
        "___stdinp",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            let ptr = env.mem.alloc_and_write(FILE { fd: STDIN_FILENO });
            // Note: Host object would be created lazily
            env.mem.alloc_and_write(ptr).cast().cast_const()
        }),
    ),
    (
        "___stdoutp",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            let ptr = env.mem.alloc_and_write(FILE { fd: STDOUT_FILENO });
            // Note: Host object would be created lazily
            env.mem.alloc_and_write(ptr).cast().cast_const()
        }),
    ),
    (
        "___stderrp",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            let ptr = env.mem.alloc_and_write(FILE { fd: STDERR_FILENO });
            // Note: Host object would be created lazily
            env.mem.alloc_and_write(ptr).cast().cast_const()
        }),
    ),
    // BSD/Darwin `___sF` — an array of 3 `__sFILE` structs used by older
    // binaries (armv6 era) that reference stdin/stdout/stderr via this
    // symbol rather than the individual `___stdinp` / `___stdoutp` /
    // `___stderrp` pointers. Digital Chocolate engine games (StuntCar,
    // Pyramid Bloxx, etc.) link against this symbol.
    (
        "___sF",
        HostConstant::Custom(|env| -> ConstVoidPtr {
            // Allocate a contiguous block of 3 * 88 bytes (zero-filled).
            let total_size = DARWIN_SFILE_SIZE * 3;
            let base: MutPtr<u8> = env.mem.alloc(total_size).cast();
            // Zero the entire block first
            for i in 0..total_size {
                env.mem.write((base + i).cast::<u8>(), 0u8);
            }
            // Write the fd field at the start of each __sFILE slot.
            // Offset 0 of each struct is `_p` (char*) on real Darwin,
            // but since our FILE only needs the fd and apps using ___sF
            // typically pass the pointer to fprintf/fscanf which reads
            // our FILE.fd, we place fd at offset 0.
            let stdin_ptr: MutPtr<i32> = base.cast();
            env.mem.write(stdin_ptr, STDIN_FILENO);
            let stdout_ptr: MutPtr<i32> = Ptr::from_bits(base.to_bits() + DARWIN_SFILE_SIZE);
            env.mem.write(stdout_ptr, STDOUT_FILENO);
            let stderr_ptr: MutPtr<i32> = Ptr::from_bits(base.to_bits() + DARWIN_SFILE_SIZE * 2);
            env.mem.write(stderr_ptr, STDERR_FILENO);
            base.cast_const().cast()
        }),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    // Standard C functions
    export_c_func!(fopen(_, _)),
    export_c_func!(fdopen(_, _)),
    export_c_func!(freopen(_, _, _)),
    export_c_func!(fread(_, _, _, _)),
    export_c_func!(fgetc(_)),
    export_c_func!(getc(_)),
    export_c_func!(ungetc(_, _)),
    export_c_func!(fgets(_, _, _)),
    export_c_func!(fputs(_, _)),
    export_c_func!(fputc(_, _)),
    export_c_func!(putc(_, _)),
    export_c_func!(fwrite(_, _, _, _)),
    export_c_func!(fseek(_, _, _)),
    export_c_func!(fseeko(_, _, _)),
    export_c_func!(ftell(_)),
    export_c_func!(ftello(_)),
    export_c_func!(rewind(_)),
    export_c_func!(fsetpos(_, _)),
    export_c_func!(fgetpos(_, _)),
    export_c_func!(feof(_)),
    export_c_func!(clearerr(_)),
    export_c_func!(fflush(_)),
    export_c_func!(fclose(_)),
    export_c_func!(ferror(_)),
    export_c_func!(puts(_)),
    export_c_func!(putchar(_)),
    export_c_func!(remove(_)),
    export_c_func!(tmpfile()),
    export_c_func!(setbuf(_, _)),
    export_c_func!(setvbuf(_, _, _, _)),
    // POSIX-specific functions
    export_c_func!(fileno(_)),
    export_c_func!(flockfile(_)),
    export_c_func!(funlockfile(_)),
    export_c_func!(ftrylockfile(_)),
    // BSD/Darwin internal stdio functions.
    // ___srget is the slow-path single-character read called by the getc()
    // macro when the FILE's inline buffer is empty. It's semantically
    // identical to fgetc(). Apps compiled against the iOS SDK reference
    // this symbol directly because the SDK headers expand getc() to an
    // inline that calls ___srget on buffer miss.
    // The Mach-O symbol is "___srget" (C name "__srget" with _ prefix).
    (
        "___srget",
        &(fgetc as fn(&mut crate::Environment, MutPtr<FILE>) -> i32),
    ),
];
