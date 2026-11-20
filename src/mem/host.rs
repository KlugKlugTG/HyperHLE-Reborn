/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Cross-platform memory management wrappers using the host's system calls.

/// Windows kernel I/O cannot invoke the user-mode fault handler that commits
/// guest pages. Touch each page before passing a buffer to ReadFile/Winsock.
/// Reading preserves its contents and is cheap for already committed pages.
/// https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile
pub(super) fn prepare_for_host_io(bytes: &[u8]) {
    #[cfg(windows)]
    if !bytes.is_empty() {
        let pointer = bytes.as_ptr();
        let page = crate::mem::PAGE_SIZE as usize;
        // SAFETY: All offsets are within the validated guest buffer. Access
        // faults inside the guest mapping commit its reserved backing pages.
        unsafe {
            std::ptr::read_volatile(pointer);
        }
        let first_boundary = page - (pointer as usize & (page - 1));
        for offset in (first_boundary..bytes.len()).step_by(page) {
            unsafe {
                std::ptr::read_volatile(pointer.add(offset));
            }
        }
    }
    #[cfg(not(windows))]
    let _ = bytes;
}

/// Protects a guest memory segment from host reads and writes.
///
/// # Safety
/// `address` must point to a mapped region at least `size` bytes long. Both
/// `address` and `size` must be aligned to the host page size.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) unsafe fn protect_guest_null_segment(
    address: *mut core::ffi::c_void,
    size: usize,
) -> std::io::Result<()> {
    let host_page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if host_page_size <= 0 {
        return Err(std::io::Error::last_os_error());
    }
    let host_page_size = host_page_size as usize;
    if address as usize % host_page_size != 0 || size == 0 || size % host_page_size != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("null segment must be aligned to the {host_page_size}-byte host page size"),
        ));
    }

    if unsafe { libc::mprotect(address, size, libc::PROT_NONE) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Cross-platform memory allocation using the host's system calls.
/// Returns an address aligned to the guest's 4KB page boundaries.
///
/// - The function returns a raw pointer to allocated memory.
///   The caller is responsible for managing that memory.
/// - The returned pointer must be freed using the corresponding
///   [`free_memory`] call.
#[cfg(windows)]
pub(super) unsafe fn allocate_memory(size: usize) -> std::io::Result<*mut core::ffi::c_void> {
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE,
    };

    let ptr = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            size,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };

    if ptr.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ptr)
}

#[cfg(unix)]
pub(super) unsafe fn allocate_memory(size: usize) -> std::io::Result<*mut core::ffi::c_void> {
    use libc::{
        mmap, mprotect, sysconf, _SC_PAGESIZE, MAP_ANONYMOUS, MAP_NORESERVE, MAP_PRIVATE,
        PROT_NONE, PROT_READ, PROT_WRITE,
    };

    const PAGE_SIZE: usize = crate::mem::PAGE_SIZE as usize;
    let host_page_size = unsafe { sysconf(_SC_PAGESIZE) as usize };

    assert!(
        host_page_size >= PAGE_SIZE,
        "Hosts with smaller than 4KiB pages are not supported."
    );

    let attempt = |prot: i32, flags: i32| -> std::io::Result<*mut core::ffi::c_void> {
        let ptr = unsafe { mmap(std::ptr::null_mut(), size, prot, flags, -1, 0) };
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(ptr)
        }
    };

    // The guest address space is one flat 4GiB mapping. Desktop kernels
    // overcommit, so asking for it read-write up front costs nothing until the
    // pages are touched. iOS does not overcommit anonymous memory, so on
    // devices with less RAM than the mapping this fails outright with ENOMEM
    // before the guest app ever starts. Fall back to progressively weaker
    // requests, ending with a pure PROT_NONE reservation that commits nothing
    // and is then widened with mprotect.
    let first_error = match attempt(PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS) {
        Ok(ptr) => return Ok(ptr),
        Err(error) => error,
    };

    log!(
        "Could not map {} MiB of guest address space read-write ({}); retrying.",
        size / (1024 * 1024),
        first_error
    );

    if let Ok(ptr) = attempt(
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
    ) {
        log!("Mapped guest address space with MAP_NORESERVE.");
        return Ok(ptr);
    }

    let ptr = attempt(PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE)
        .or_else(|_| attempt(PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS))
        // Report the original read-write failure: it is the informative one.
        .map_err(|_| first_error)?;

    if unsafe { mprotect(ptr, size, PROT_READ | PROT_WRITE) } != 0 {
        let error = std::io::Error::last_os_error();
        unsafe { libc::munmap(ptr, size) };
        return Err(error);
    }

    log!("Mapped guest address space as a reservation widened with mprotect.");
    Ok(ptr)
}

#[cfg(windows)]
pub(super) unsafe fn allocate_guest_memory(size: usize) -> std::io::Result<*mut core::ffi::c_void> {
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_RESERVE, PAGE_READWRITE};

    let ptr = unsafe { VirtualAlloc(std::ptr::null(), size, MEM_RESERVE, PAGE_READWRITE) };

    if ptr.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    lazy_commit::register(ptr as usize, size);
    Ok(ptr)
}

#[cfg(unix)]
pub(super) unsafe fn allocate_guest_memory(size: usize) -> std::io::Result<*mut core::ffi::c_void> {
    unsafe { allocate_memory(size) }
}

/// Cross-platform memory free using the host's system calls.
///
/// # Safety
/// - The address and size should match parameters and result of the
///   [`allocate_memory`] call.
#[cfg(windows)]
pub(super) unsafe fn free_memory(
    address: *mut core::ffi::c_void,
    _size: usize,
) -> std::io::Result<()> {
    use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};

    let res = unsafe { VirtualFree(address, 0, MEM_RELEASE) };

    if res == 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(unix)]
pub(super) unsafe fn free_memory(
    address: *mut core::ffi::c_void,
    size: usize,
) -> std::io::Result<()> {
    use libc::munmap;

    let res = unsafe { munmap(address, size) };

    if res == -1 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(windows)]
pub(super) unsafe fn free_guest_memory(
    address: *mut core::ffi::c_void,
    size: usize,
) -> std::io::Result<()> {
    lazy_commit::unregister(address as usize);
    unsafe { free_memory(address, size) }
}

#[cfg(unix)]
pub(super) unsafe fn free_guest_memory(
    address: *mut core::ffi::c_void,
    size: usize,
) -> std::io::Result<()> {
    unsafe { free_memory(address, size) }
}

#[cfg(windows)]
mod lazy_commit {
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
        EXCEPTION_POINTERS,
    };
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, PAGE_READWRITE};

    struct Region {
        base: usize,
        size: usize,
    }

    static REGIONS: Mutex<Vec<Region>> = Mutex::new(Vec::new());
    static HANDLER_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    pub(super) fn register(base: usize, size: usize) {
        {
            let mut regions = REGIONS.lock().unwrap();
            regions.push(Region { base, size });
        }
        HANDLER_ONCE.get_or_init(|| {
            let res = unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
            assert!(
                !res.is_null(),
                "AddVectoredExceptionHandler failed: {}",
                std::io::Error::last_os_error()
            );
        });
    }

    pub(super) fn unregister(base: usize) {
        let mut regions = REGIONS.lock().unwrap();
        regions.retain(|r| r.base != base);
    }

    unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
        let info = unsafe { &*info };
        let rec_ptr = info.ExceptionRecord;
        if rec_ptr.is_null() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let rec = unsafe { &*rec_ptr };
        if rec.ExceptionCode as u32 != EXCEPTION_ACCESS_VIOLATION as u32 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        if rec.NumberParameters < 2 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let fault_addr = rec.ExceptionInformation[1] as usize;

        let in_region = {
            let regions = match REGIONS.lock() {
                Ok(g) => g,
                Err(_) => return EXCEPTION_CONTINUE_SEARCH,
            };
            regions
                .iter()
                .any(|r| fault_addr >= r.base && fault_addr < r.base.wrapping_add(r.size))
        };
        if !in_region {
            return EXCEPTION_CONTINUE_SEARCH;
        }

        const PAGE_SIZE: usize = super::super::PAGE_SIZE as usize;
        let page_addr = fault_addr & !(PAGE_SIZE - 1);

        let committed =
            unsafe { VirtualAlloc(page_addr as *const _, PAGE_SIZE, MEM_COMMIT, PAGE_READWRITE) };
        if committed.is_null() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        EXCEPTION_CONTINUE_EXECUTION
    }
}
