/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::errno::{set_errno, EINVAL, EIO, ENOMEM};
use crate::libc::posix_io;
use crate::libc::posix_io::{off_t, open_direct, FileDescriptor, SEEK_SET};
use crate::mem::{ConstPtr, GuestUSize, Mem, MutVoidPtr, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK};
use std::collections::HashMap;

#[allow(dead_code)]
const MAP_FILE: i32 = 0x0000;
const MAP_FIXED: i32 = 0x0010;
const MAP_ANON: i32 = 0x1000;

const MAP_FAILED: MutVoidPtr = MutVoidPtr::from_bits(0xFFFFFFFF);

#[derive(Default)]
pub struct State {
    /// Keeping track of `mmap` allocations
    allocations: HashMap<MutVoidPtr, GuestUSize>,
}

/// For files, our implementation of mmap is really simple:
/// it's just load entirety of file in memory!
fn mmap(
    env: &mut Environment,
    addr: MutVoidPtr,
    len: GuestUSize,
    prot: i32,
    flags: i32,
    fd: FileDescriptor,
    offset: off_t,
) -> MutVoidPtr {
    // TODO: handle errno properly
    set_errno(env, 0);
    log_dbg!(
        "mmap({:?}, {}, {}, {}, {}, {})",
        addr,
        len,
        prot,
        flags,
        fd,
        offset
    );

    if (flags & MAP_FIXED) != 0 {
        return mmap_fixed(env, addr, len, flags, fd, offset);
    }

    // TODO: use vm_allocate() instead
    let ptr = env.mem.calloc(len);
    if ptr.is_null() {
        set_errno(env, ENOMEM);
        return MAP_FAILED;
    }

    if (flags & MAP_ANON) != 0 {
        // Real mmap always returns page-aligned addresses and callers (e.g.
        // Boehm GC) rely on that. Fail gracefully instead of panicking the host.
        if ptr.to_bits() & PAGE_SIZE_ALIGN_MASK != 0 {
            log!(
                "Warning: mmap MAP_ANON: allocator returned unaligned address {:?} for {} bytes; returning MAP_FAILED",
                ptr,
                len
            );
            env.mem.free(ptr);
            set_errno(env, ENOMEM);
            return MAP_FAILED;
        }

        // Убираем жесткие assert_eq!(fd, -1) и assert_eq!(offset, 0).
        // В реальной iOS/Darwin при наличии флага MAP_ANON аргументы fd и
        // offset
        // просто игнорируются ОС. Движки вроде Adobe AIR передают сюда мусор.
        if fd != -1 || offset != 0 {
            log_dbg!("Warning: mmap MAP_ANON called with fd={} and offset={}. Ignoring them as per OS behavior.", fd, offset);
        }

        if !addr.is_null() {
            // POSIX `mmap` documents the `addr` argument as a hint that
            // implementations may ignore. We always allocate from the
            // guest heap, so the actual placement is the heap allocator's
            // choice. Apps that genuinely require fixed-address mappings
            // would set MAP_FIXED, which we'd then need to honour
            // separately. Demoted to debug to keep Mono/Unity startup
            // logs readable; the host-vs-hint mismatch is not an error.
            log_dbg!(
                "mmap MAP_ANON ignoring hint for address {:?}, actual is {:?}",
                addr,
                ptr
            );
        }
    } else {
        // File-backed mmap: read file content into the allocated buffer.
        if !addr.is_null() {
            log_dbg!(
                "mmap file-backed ignoring hint for address {:?}, actual is {:?}",
                addr,
                ptr
            );
        }
        // Seek to the requested offset. If the seek fails (e.g. bad fd),
        // return MAP_FAILED (-1 as pointer) instead of crashing.
        let new_offset = posix_io::lseek(env, fd, offset, SEEK_SET);
        if new_offset != offset {
            log!(
                "Warning: mmap: lseek to offset {} failed (returned {}); returning MAP_FAILED",
                offset,
                new_offset
            );
            env.mem.free(ptr);
            set_errno(env, EIO);
            return MAP_FAILED;
        }

        let read = posix_io::read(env, fd, ptr, len);
        if (read as u32) < len {
            log!(
                "Warning: mmap: read only {} of {} bytes from fd {}; padding remainder with zeros",
                read,
                len,
                fd
            );
            // Remainder is already zeroed (calloc)
        }
    }

    track_allocation(&mut env.libc_state.mmap.allocations, ptr, len);

    ptr
}

/// Track an mmap allocation, replacing any stale entry left behind after the
/// guest heap recycled an address. `Mem::alloc` only returns addresses that are
/// currently free, so an existing entry for that address cannot still describe
/// a live allocation and must not turn guest behavior into a host panic.
fn track_allocation(
    allocations: &mut HashMap<MutVoidPtr, GuestUSize>,
    ptr: MutVoidPtr,
    len: GuestUSize,
) {
    if let Some(previous_len) = allocations.insert(ptr, len) {
        log!(
            "Warning: mmap: allocator reused {:?} while a stale mapping record remained (old length {}, new length {}); replacing it",
            ptr,
            previous_len,
            len
        );
    }
}

/// `mmap` with `MAP_FIXED`: the mapping must be placed at exactly `addr`,
/// replacing whatever was mapped there before.
///
/// The Boehm GC used by Mono (and so by every Unity game) relies on this to
/// release memory: `GC_unmap` maps fresh `PROT_NONE` pages over free heap
/// blocks with `MAP_FIXED` and aborts with `mmap(...PROT_NONE...) failed` if
/// the result isn't the address it asked for. `GC_remap` does the same with
/// read/write pages in older versions. Guest memory has no page protections,
/// so replacing a mapping just means giving it the contents of the new one:
/// zeroes for an anonymous mapping, the file's data for a file mapping.
fn mmap_fixed(
    env: &mut Environment,
    addr: MutVoidPtr,
    len: GuestUSize,
    flags: i32,
    fd: FileDescriptor,
    offset: off_t,
) -> MutVoidPtr {
    let (len, is_new_mapping) = match map_fixed_zeroed(
        &mut env.mem,
        &mut env.libc_state.mmap.allocations,
        addr,
        len,
    ) {
        Ok(result) => result,
        Err(errno) => {
            set_errno(env, errno);
            return MAP_FAILED;
        }
    };

    if (flags & MAP_ANON) == 0 {
        let new_offset = posix_io::lseek(env, fd, offset, SEEK_SET);
        if new_offset != offset {
            log!(
                "Warning: mmap MAP_FIXED: lseek to offset {} failed (returned {}); returning MAP_FAILED",
                offset,
                new_offset
            );
            if is_new_mapping {
                env.libc_state.mmap.allocations.remove(&addr);
                env.mem.free(addr);
            }
            set_errno(env, EIO);
            return MAP_FAILED;
        }
        posix_io::read(env, fd, addr, len);
    }

    set_errno(env, 0);
    addr
}

/// The memory side of [mmap_fixed]: make the `len` bytes at `addr` a fresh,
/// zero-filled mapping. On success, returns the page-rounded length and
/// whether the range was previously unused (and is now tracked in
/// `allocations`), otherwise an errno value.
fn map_fixed_zeroed(
    mem: &mut Mem,
    allocations: &mut HashMap<MutVoidPtr, GuestUSize>,
    addr: MutVoidPtr,
    len: GuestUSize,
) -> Result<(GuestUSize, bool), i32> {
    let base = addr.to_bits();
    let len = len
        .checked_next_multiple_of(PAGE_SIZE)
        .filter(|&len| len != 0 && base.checked_add(len - 1).is_some())
        .ok_or(EINVAL)?;
    if base & PAGE_SIZE_ALIGN_MASK != 0 || base < mem.null_segment_size() {
        return Err(EINVAL);
    }
    if mem.get_bytes_fallible_mut(addr.cast_const(), len).is_none() {
        return Err(ENOMEM);
    }

    let is_new_mapping = mem.is_range_free(base, len);
    if is_new_mapping {
        // A brand-new mapping: claim the range so the allocator won't hand
        // it out, and let munmap() release it again.
        mem.reserve(base, len);
        allocations.insert(addr, len);
    } else {
        // Replacing (part of) memory that is already in use, which is what
        // the GC does to its own heap. The range keeps belonging to whatever
        // allocation it is part of.
        log_dbg!(
            "mmap MAP_FIXED over in-use memory {:?} ({:#x} bytes)",
            addr,
            len
        );
    }
    mem.bytes_at_mut(addr.cast(), len).fill(0);
    Ok((len, is_new_mapping))
}

fn munmap(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);
    log_dbg!("munmap({:?}, {})", addr, len);

    if len == 0 {
        // Darwin returns EINVAL for `munmap(addr, 0)`, but several apps
        // (e.g. the UE3-based BioShock port) call it this way in a loop to
        // release cached buffers and treat any failure as fatal for their
        // allocator bookkeeping. Be permissive: if the whole mapping at
        // `addr` is known, release it in full (its recorded length is what
        // the caller means); otherwise treat the call as a successful no-op.
        // This matches Linux's munmap(NULL-adjacent) tolerance and keeps the
        // guest allocator's state consistent.
        if let Some(&expected_len) = env.libc_state.mmap.allocations.get(&addr) {
            log_dbg!(
                "munmap({:?}, 0): zero length, unmapping whole {}-byte mapping",
                addr,
                expected_len
            );
            env.mem.free(addr);
            env.libc_state.mmap.allocations.remove(&addr);
        } else {
            log_dbg!("munmap({:?}, 0): unknown mapping, treating as no-op", addr);
        }
        set_errno(env, 0);
        return 0;
    }

    if let Some(&expected_len) = env.libc_state.mmap.allocations.get(&addr) {
        if expected_len != len {
            log_dbg!(
                "munmap({:?}, {}): length mismatch (expected {}), proceeding anyway",
                addr,
                len,
                expected_len
            );
        }
        env.mem.free(addr);
        env.libc_state.mmap.allocations.remove(&addr);
        0 // success
    } else {
        // Darwin's `munmap` only rejects invalid arguments: unmapping a range
        // that holds no mapping is a successful no-op, not an error.
        // <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/munmap.2.html>
        //
        // Reporting `EINVAL` here made BioShock abort during startup: its
        // allocator releases memory in ranges that do not line up with one of
        // our whole `mmap` allocations (and with ranges that `mmap` with
        // `MAP_FIXED` replaced rather than created), and it treats any
        // `munmap` failure as fatal.
        //
        // Guest memory has no page protections, so keeping the range mapped is
        // harmless: it stays valid and either gets released together with the
        // allocation that owns it, or costs host memory until exit.
        log_dbg!(
            "munmap({:?}, {}): no tracked mapping at this address, treating as no-op",
            addr,
            len
        );
        set_errno(env, 0);
        0
    }
}

// Darwin `madvise` advice values (sys/mman.h).
const MADV_NORMAL: i32 = 0;
const MADV_RANDOM: i32 = 1;
const MADV_SEQUENTIAL: i32 = 2;
const MADV_WILLNEED: i32 = 3;
const MADV_DONTNEED: i32 = 4;
const MADV_FREE: i32 = 5;
const MADV_ZERO_WIRED_PAGES: i32 = 6;
const MADV_FREE_REUSABLE: i32 = 7;
const MADV_FREE_REUSE: i32 = 8;
const MADV_CAN_REUSE: i32 = 9;

/// Guest memory is a plain host allocation with no paging, so every advice
/// is a hint we can safely ignore. Only the argument validation that a real
/// kernel performs is emulated: unknown advice values and unaligned
/// addresses fail with `EINVAL`.
fn madvise(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize, advice: i32) -> i32 {
    log_dbg!("madvise({:?}, {}, {})", addr, len, advice);
    match advice {
        MADV_NORMAL
        | MADV_RANDOM
        | MADV_SEQUENTIAL
        | MADV_WILLNEED
        | MADV_DONTNEED
        | MADV_FREE
        | MADV_ZERO_WIRED_PAGES
        | MADV_FREE_REUSABLE
        | MADV_FREE_REUSE
        | MADV_CAN_REUSE => {}
        _ => {
            set_errno(env, EINVAL);
            return -1;
        }
    }
    if addr.to_bits() & PAGE_SIZE_ALIGN_MASK != 0 {
        set_errno(env, EINVAL);
        return -1;
    }
    set_errno(env, 0);
    0
}

fn shm_open(env: &mut Environment, name: ConstPtr<u8>, oflag: i32, mode: u32) -> i32 {
    set_errno(env, 0);

    let name_str = env.mem.cstr_at_utf8(name).unwrap_or("<invalid>");
    log_dbg!("shm_open({:?}, {:#x}, {:#x})", name_str, oflag, mode);

    // Используем open_direct! Параметр mode для эмулятора здесь не нужен,
    // поэтому просто передаем env, name и oflag.
    open_direct(env, name, oflag)
}

/// `int shm_unlink(const char *name)`
///
/// POSIX shared-memory unlink: removes the named shared-memory object.
/// Regions opened via `shm_open` are backed by our fs layer, where `unlink`
/// already implements remove-on-last-close semantics. Missing regions return
/// -1/ENOENT per POSIX.
fn shm_unlink(env: &mut Environment, name: ConstPtr<u8>) -> i32 {
    crate::libc::unistd::unlink(env, name)
}

fn mprotect(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize, prot: i32) -> i32 {
    // POSIX `int mprotect(void *addr, size_t len, int prot)`: returns 0
    // on success, -1 on failure with errno set.
    //
    // touchHLE doesn't enforce per-page memory protections — the entire
    // guest address space is treated as RW (and code pages as RX through
    // the JIT). However, returning -1 + ENOTSUP for every mprotect call
    // is wrong: it makes Mono/Boehm GC and Unity's runtime think the
    // protection change failed during JIT/GC initialization, which can
    // leave them in a broken state. Real Darwin kernels never fail
    // mprotect for the address ranges that mmap'd allocations sit in,
    // so the correct behavior is "succeed silently as a no-op".
    //
    // Reference: POSIX mprotect(2) — return value is 0 on success, -1 on
    // error; the only documented errors apply to invalid/non-mapped
    // address ranges, which our guest mmap allocator handles by always
    // returning a valid range.
    log_dbg!(
        "mprotect({:?}, {}, {:#x}) -> 0 (no-op; touchHLE does not enforce per-page protections)",
        addr,
        len,
        prot
    );
    set_errno(env, 0);
    0
}

/// `int mlock(const void *addr, size_t len)` — lock a region of memory
/// so it stays resident in physical RAM. touchHLE keeps the entire
/// guest address space resident in the host process at all times, so
/// there is nothing to pin: the memory is already non-pageable from the
/// guest's perspective. Real Darwin returns 0 on success, so we report
/// success as a no-op (returning -1 here would make guest code that
/// relies on mlock — e.g. crypto/keychain libraries protecting secrets,
/// or audio engines pinning buffers — treat initialization as failed).
///
/// Reference: POSIX/Darwin mlock(2) — returns 0 on success, -1 with
/// errno on failure.
fn mlock(env: &mut Environment, addr: ConstPtr<u8>, len: GuestUSize) -> i32 {
    log_dbg!(
        "mlock({:?}, {}) -> 0 (no-op; guest memory is always resident)",
        addr,
        len
    );
    set_errno(env, 0);
    0
}

/// `int munlock(const void *addr, size_t len)` — unlock a region
/// previously locked with `mlock`. Mirrors [mlock]: a no-op that
/// succeeds.
///
/// Reference: POSIX/Darwin munlock(2) — returns 0 on success.
fn munlock(env: &mut Environment, addr: ConstPtr<u8>, len: GuestUSize) -> i32 {
    log_dbg!(
        "munlock({:?}, {}) -> 0 (no-op; guest memory is always resident)",
        addr,
        len
    );
    set_errno(env, 0);
    0
}

/// `int msync(void *addr, size_t len, int flags)` — synchronize a mapped
/// region with the file that backs it. touchHLE's `mmap` copies file
/// content into a guest allocation and never writes it back, and guest
/// memory is not paged, so there is nothing to flush: report success as
/// a no-op (Darwin returns 0 on success; returning -1 would make guests
/// that treat msync failures as fatal — e.g. engines persisting save
/// data through mmap — abort their write path).
///
/// Reference: POSIX msync(2) — returns 0 on success, -1 with errno on
/// failure.
fn msync(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize, flags: i32) -> i32 {
    log_dbg!("msync({:?}, {}, {:#x}) -> 0 (no-op)", addr, len, flags);
    set_errno(env, 0);
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mmap(_, _, _, _, _, _)),
    export_c_func!(munmap(_, _)),
    export_c_func!(msync(_, _, _)),
    export_c_func!(madvise(_, _, _)),
    export_c_func!(shm_open(_, _, _)),
    export_c_func!(shm_unlink(_)),
    export_c_func!(mprotect(_, _, _)),
    export_c_func!(mlock(_, _)),
    export_c_func!(munlock(_, _)),
];

#[cfg(test)]
mod map_fixed_tests {
    use super::*;
    use crate::mem::Ptr;

    fn new_test_mem() -> Mem {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        mem
    }

    #[test]
    fn recycled_mmap_address_replaces_stale_bookkeeping() {
        let ptr = MutVoidPtr::from_bits(0x2000_0000);
        let mut allocations = HashMap::new();
        allocations.insert(ptr, PAGE_SIZE);

        track_allocation(&mut allocations, ptr, PAGE_SIZE * 2);

        assert_eq!(allocations.len(), 1);
        assert_eq!(allocations.get(&ptr), Some(&(PAGE_SIZE * 2)));
    }

    /// What Boehm GC's `GC_unmap` does: map `PROT_NONE` pages over part of
    /// a heap block it got from an earlier `mmap`, and abort unless the
    /// result is the address it asked for.
    #[test]
    fn replaces_pages_inside_an_existing_mapping_in_place() {
        let mut mem = new_test_mem();
        let mut allocations = HashMap::new();
        let heap: MutVoidPtr = mem.alloc(PAGE_SIZE * 4);
        allocations.insert(heap, PAGE_SIZE * 4);
        mem.bytes_at_mut(heap.cast(), PAGE_SIZE * 4).fill(0xAB);

        let middle: MutVoidPtr = Ptr::from_bits(heap.to_bits() + PAGE_SIZE);
        assert_eq!(
            map_fixed_zeroed(&mut mem, &mut allocations, middle, PAGE_SIZE * 2),
            Ok((PAGE_SIZE * 2, false))
        );

        let bytes = mem.bytes_at(heap.cast(), PAGE_SIZE * 4);
        let page = PAGE_SIZE as usize;
        assert!(bytes[..page].iter().all(|&b| b == 0xAB));
        assert!(bytes[page..page * 3].iter().all(|&b| b == 0));
        assert!(bytes[page * 3..].iter().all(|&b| b == 0xAB));
        // The heap block is still a single mapping that munmap() can free.
        assert_eq!(allocations.len(), 1);
        assert!(mem.is_known_allocation(heap.to_bits()));
    }

    #[test]
    fn claims_unused_address_space() {
        let mut mem = new_test_mem();
        let mut allocations = HashMap::new();
        let addr: MutVoidPtr = Ptr::from_bits(0x2000_0000);
        assert!(mem.is_range_free(addr.to_bits(), PAGE_SIZE));

        // Lengths are rounded up to whole pages.
        assert_eq!(
            map_fixed_zeroed(&mut mem, &mut allocations, addr, 100),
            Ok((PAGE_SIZE, true))
        );
        assert!(!mem.is_range_free(addr.to_bits(), PAGE_SIZE));
        assert_eq!(allocations.get(&addr), Some(&PAGE_SIZE));
        assert!(mem.bytes_at(addr.cast(), PAGE_SIZE).iter().all(|&b| b == 0));

        mem.free(addr);
        assert!(mem.is_range_free(addr.to_bits(), PAGE_SIZE));
    }

    #[test]
    fn rejects_bad_arguments() {
        let mut mem = new_test_mem();
        let mut allocations = HashMap::new();
        let unaligned: MutVoidPtr = Ptr::from_bits(0x2000_0010);
        assert_eq!(
            map_fixed_zeroed(&mut mem, &mut allocations, unaligned, PAGE_SIZE),
            Err(EINVAL)
        );
        let aligned: MutVoidPtr = Ptr::from_bits(0x2000_0000);
        assert_eq!(
            map_fixed_zeroed(&mut mem, &mut allocations, aligned, 0),
            Err(EINVAL)
        );
        assert_eq!(
            map_fixed_zeroed(&mut mem, &mut allocations, Ptr::null(), PAGE_SIZE),
            Err(EINVAL)
        );
        let near_top: MutVoidPtr = Ptr::from_bits(0xFFFF_F000);
        assert_eq!(
            map_fixed_zeroed(&mut mem, &mut allocations, near_top, PAGE_SIZE * 2),
            Err(EINVAL)
        );
        assert!(allocations.is_empty());
    }
}
