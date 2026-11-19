/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach_host.h` and some other related functions

#![allow(non_camel_case_types)]

use crate::dyld::FunctionExports;
use crate::libc::mach::core_types::natural_t;
use crate::libc::mach::port::mach_port_t;
use crate::libc::mach::thread_info::{
    kern_return_t, mach_msg_type_number_t, KERN_INVALID_ARGUMENT, KERN_SUCCESS,
};
use crate::mem::{guest_size_of, GuestUSize, MutPtr, SafeRead, PAGE_SIZE};
use crate::{export_c_func, Environment};

type host_t = mach_port_t;
type host_name_port_t = host_t;
type host_flavor_t = natural_t;
type host_info_t = MutPtr<natural_t>;
type vm_size_t = natural_t;
type clock_id_t = natural_t;
type clock_serv_t = mach_port_t;

// The value doesn't matter that much, only the fact that it's unique
// per host so we could assert against it in our code.
const MACH_HOST_SELF: host_name_port_t = 0x100c442e;

// Clock service port sentinels — arbitrary unique values, one per clock ID.
const CLOCK_PORT_REALTIME: clock_serv_t = 0x200c0001;
const CLOCK_PORT_MONOTONIC: clock_serv_t = 0x200c0002;
const CLOCK_PORT_CALENDAR: clock_serv_t = 0x200c0003;

// clock_id_t values (from <mach/clock_types.h>)
const SYSTEM_CLOCK: clock_id_t = 0; // monotonic uptime
const CALENDAR_CLOCK: clock_id_t = 1; // wall-clock (UTC)
const REALTIME_CLOCK: clock_id_t = 2; // alias for SYSTEM_CLOCK on Darwin

// Used in host_statistics function (returned in vm_statistics).
// These are reported as a fraction of the device's physical memory so that
// `host_statistics(HOST_VM_INFO)` agrees with `NSProcessInfo.physicalMemory`
// and the `hw.memsize` sysctl. The previous code hardcoded counts taken from
// an iPod touch 4 (≈50 MiB total), which under-reported RAM on every device
// and confused memory-budgeting engines such as Unreal Engine 3.
//
// We model a freshly-launched device: roughly a quarter of RAM still free,
// the rest accounted as wired/active. The exact split doesn't matter much —
// only that free_count is non-trivial and the total matches physical memory.

/// Physical memory of the emulated device, in bytes. Mirrors what a real
/// device reports through `NSProcessInfo.physicalMemory` and `hw.memsize`.
pub fn physical_memory(env: &Environment) -> u64 {
    // Device information is available in headless sessions too. Environment
    // initialization records the selected profile in the options.
    env.options
        .as_ref()
        .device_family
        .unwrap_or(crate::window::DeviceFamily::iPhone3GS)
        .physical_memory()
}

const HOST_VM_INFO: host_flavor_t = 2;
const HOST_VM_INFO64: host_flavor_t = 4;
// host_sched_info { min_timeout, min_quantum } — HOST_SCHED_INFO is 3.
const HOST_SCHED_INFO: host_flavor_t = 3;

/// `struct host_sched_info` (<mach/host_info.h>): the scheduler reports
/// the minimum timeout and quantum, both equal to the initial quantum in
/// milliseconds.
fn host_statistics_sched_info(
    env: &mut Environment,
    host_info_out: host_info_t,
    host_info_out_count: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    let out_size_available = env.mem.read(host_info_out_count);
    if (out_size_available as u32) < 2 {
        log!(
            "host_statistics: caller buffer too small for HOST_SCHED_INFO: \
             available={}",
            out_size_available
        );
        return KERN_INVALID_ARGUMENT;
    }
    let initial_quantum_ms: natural_t = 2;
    for (i, value) in [initial_quantum_ms, initial_quantum_ms].iter().enumerate() {
        let field_size = guest_size_of::<natural_t>() as GuestUSize;
        env.mem.write(
            (host_info_out + (i as GuestUSize * field_size)).cast(),
            *value,
        );
    }
    env.mem.write(host_info_out_count, 2);
    KERN_SUCCESS
}

#[repr(C, packed)]
struct vm_statistics {
    free_count: natural_t,
    active_count: natural_t,
    inactive_count: natural_t,
    wire_count: natural_t,
    zero_fill_count: natural_t,
    reactivations: natural_t,
    pageins: natural_t,
    pageouts: natural_t,
    faults: natural_t,
    cow_faults: natural_t,
    lookups: natural_t,
    hits: natural_t,
    purgeable_count: natural_t,
    purges: natural_t,
    speculative_count: natural_t,
}
unsafe impl SafeRead for vm_statistics {}

fn mach_host_self(_env: &mut Environment) -> host_name_port_t {
    MACH_HOST_SELF
}

fn host_page_size(
    env: &mut Environment,
    host: host_t,
    out_page_size: MutPtr<vm_size_t>,
) -> kern_return_t {
    // Apple's host_page_size accepts MACH_HOST_SELF only; other ports fail
    // with KERN_INVALID_ARGUMENT per XNU host.c. Real apps (e.g. Unreal Engine)
    // pass different host-port values discovered at runtime, so use a soft
    // error path instead of assert! which takes down the host.
    if host != MACH_HOST_SELF {
        log!(
            "host_page_size: unexpected host port {:#010x} (expected MACH_HOST_SELF {:#010x}); \
             returning KERN_INVALID_ARGUMENT",
            host,
            MACH_HOST_SELF
        );
        return KERN_INVALID_ARGUMENT;
    }
    env.mem.write(out_page_size, PAGE_SIZE);
    KERN_SUCCESS
}

fn host_statistics(
    env: &mut Environment,
    host: host_t,
    flavor: host_flavor_t,
    host_info_out: host_info_t,
    host_info_out_count: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    if host != MACH_HOST_SELF {
        log!(
            "host_statistics: unexpected host port {:#010x} (expected MACH_HOST_SELF {:#010x}); \
             returning KERN_INVALID_ARGUMENT",
            host,
            MACH_HOST_SELF
        );
        return KERN_INVALID_ARGUMENT;
    }
    if flavor == HOST_SCHED_INFO {
        return host_statistics_sched_info(env, host_info_out, host_info_out_count);
    }
    if flavor == HOST_SCHED_INFO {
        // Real iOS returns the scheduler's initial quantum (in ms) for both
        // fields; report a sane 2 ms rather than failing the call.
        let available = env.mem.read(host_info_out_count);
        if (available as u32) < 2 {
            log!(
                "host_statistics: caller buffer too small for sched info: \
                 available={}",
                available
            );
            return KERN_INVALID_ARGUMENT;
        }
        env.mem.write(host_info_out.cast(), 2);
        env.mem.write(
            (host_info_out + guest_size_of::<natural_t>() as u32).cast(),
            2,
        );
        env.mem.write(host_info_out_count, 2);
        return KERN_SUCCESS;
    }
    if flavor != HOST_VM_INFO {
        log!(
            "host_statistics: unsupported flavor {}; returning KERN_INVALID_ARGUMENT",
            flavor
        );
        return KERN_INVALID_ARGUMENT;
    }
    let out_size_available = env.mem.read(host_info_out_count);
    let out_size_expected = guest_size_of::<vm_statistics>() / guest_size_of::<natural_t>();
    if (out_size_available as u32) < (out_size_expected as u32) {
        log!(
            "host_statistics: caller buffer too small: available={}, expected={}; \
             returning KERN_INVALID_ARGUMENT",
            out_size_available,
            out_size_expected
        );
        return KERN_INVALID_ARGUMENT;
    }
    let physical = physical_memory(env);
    // `vm_statistics` reports page counts, not bytes. Convert and split the
    // total across the buckets so they sum exactly to the physical page count
    // (model a freshly-booted device with ~1/4 of RAM free).
    let total_pages = (physical / PAGE_SIZE as u64) as natural_t;
    let free_count = total_pages / 4;
    let inactive_count = total_pages / 8;
    let wire_count = total_pages / 4;
    let active_count = total_pages - free_count - inactive_count - wire_count;
    env.mem.write(
        host_info_out.cast(),
        vm_statistics {
            free_count,
            active_count,
            inactive_count,
            wire_count,
            zero_fill_count: 0,
            reactivations: 0,
            pageins: 0,
            pageouts: 0,
            faults: 0,
            cow_faults: 0,
            lookups: 0,
            hits: 0,
            purgeable_count: 0,
            purges: 0,
            speculative_count: 0,
        },
    );
    // Per Apple XNU host_statistics semantics, update *count to the number of
    // natural_t-sized fields actually written, so callers that passed a
    // larger-than-needed buffer (e.g. Unreal Engine passing byte-size) still
    // receive a valid, stable count back.
    env.mem.write(
        host_info_out_count,
        out_size_expected as mach_msg_type_number_t,
    );
    KERN_SUCCESS
}

// =========================================================================
// MARK: - Clock services
// =========================================================================

/// `kern_return_t host_get_clock_service(host_t host, clock_id_t clock_id,
///                                       clock_serv_t *clock_serv)`
///
/// Returns a send right to the named clock service. Apps use the returned
/// port with `clock_get_time` / `clock_get_attributes`. We hand back a
/// stable sentinel port value per clock ID so the caller can hold and pass
/// it around freely.
fn host_get_clock_service(
    env: &mut Environment,
    host: host_t,
    clock_id: clock_id_t,
    clock_serv: MutPtr<clock_serv_t>,
) -> kern_return_t {
    // Soft-fail on unexpected host ports (see host_page_size /
    // host_statistics): per XNU host.c the kernel returns
    // KERN_INVALID_ARGUMENT rather than aborting.
    if host != MACH_HOST_SELF {
        log!(
            "host_get_clock_service: unexpected host port {:#010x} (expected MACH_HOST_SELF \
             {:#010x}); returning KERN_INVALID_ARGUMENT",
            host,
            MACH_HOST_SELF
        );
        return KERN_INVALID_ARGUMENT;
    }
    let port = match clock_id {
        SYSTEM_CLOCK => CLOCK_PORT_MONOTONIC,
        CALENDAR_CLOCK => CLOCK_PORT_CALENDAR,
        REALTIME_CLOCK => CLOCK_PORT_REALTIME,
        other => {
            log!(
                "host_get_clock_service: unknown clock_id {}, returning monotonic port",
                other
            );
            CLOCK_PORT_MONOTONIC
        }
    };
    env.mem.write(clock_serv, port);
    log_dbg!(
        "host_get_clock_service(clock_id={}) => port {:#010x}",
        clock_id,
        port
    );
    KERN_SUCCESS
}

/// `kern_return_t clock_get_time(clock_serv_t clock_serv,
///                               mach_timespec_t *cur_time)`
///
/// Fills `*cur_time` with the current time for the given clock service port.
/// `mach_timespec_t` is `{ tv_sec: u32, tv_nsec: u32 }`.
#[repr(C, packed)]
struct mach_timespec_t {
    tv_sec: u32,
    tv_nsec: u32,
}
unsafe impl SafeRead for mach_timespec_t {}

fn clock_get_time(
    env: &mut Environment,
    clock_serv: clock_serv_t,
    cur_time: MutPtr<mach_timespec_t>,
) -> kern_return_t {
    let (secs, nanos): (u64, u32) = match clock_serv {
        CLOCK_PORT_MONOTONIC | CLOCK_PORT_REALTIME => {
            // Monotonic: seconds and nanoseconds since process startup.
            let d = env.guest_clock.now().duration_since(env.startup_time);
            (d.as_secs(), d.subsec_nanos())
        }
        CLOCK_PORT_CALENDAR => {
            // Calendar: seconds since Unix epoch via SystemTime.
            match env.guest_clock.system_time().duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs(), d.subsec_nanos()),
                Err(_) => (0, 0),
            }
        }
        other => {
            if other == 0 {
                log_dbg!("clock_get_time: called with MACH_PORT_NULL (0)");
            } else {
                log_dbg!("clock_get_time: unknown clock_serv port {:#010x}", other);
            }
            // Per Mach documentation, an invalid clock service port should
            // return KERN_INVALID_ARGUMENT. We still zero the output to avoid
            // leaving garbage in guest memory for callers that ignore errors.
            env.mem.write(
                cur_time,
                mach_timespec_t {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
            );
            return KERN_INVALID_ARGUMENT;
        }
    };
    env.mem.write(
        cur_time,
        mach_timespec_t {
            tv_sec: secs as u32,
            tv_nsec: nanos,
        },
    );
    log_dbg!(
        "clock_get_time(port={:#010x}) => {}.{:09}",
        clock_serv,
        secs,
        nanos
    );
    KERN_SUCCESS
}

/// `kern_return_t clock_get_attributes(clock_serv_t clock_serv,
///                                     clock_flavor_t flavor,
///                                     clock_attr_t attr,
///                                     mach_msg_type_number_t *attr_count)`
///
/// Stub — most apps only call this to query the clock resolution, which we
/// report as 1 ms (1 000 000 ns), a safe conservative value.
fn clock_get_attributes(
    env: &mut Environment,
    clock_serv: clock_serv_t,
    flavor: natural_t,
    attr: MutPtr<natural_t>,
    attr_count: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    log_dbg!(
        "clock_get_attributes(port={:#010x}, flavor={}) — returning 1ms resolution",
        clock_serv,
        flavor
    );
    // CLOCK_GET_TIME_RES (flavor 1): resolution in nanoseconds.
    // Write a single natural_t value of 1 000 000 (1 ms).
    if !attr.is_null() {
        env.mem.write(attr, 1_000_000u32);
    }
    if !attr_count.is_null() {
        env.mem.write(attr_count, 1);
    }
    KERN_SUCCESS
}

/// `kern_return_t host_statistics64(host_t host, host_flavor_t flavor,
///                                  host_info_t host_info_out,
///                                  mach_msg_type_number_t
///                                      *host_info_out_count)`
///
/// The 64-bit variant of `host_statistics`, used by memory-monitors and by
/// Mono's `GCGetTotalMemory`-style probes via
/// `sysctl(CTL_VM, VM_METER)`-style reporting paths. Fill the same
/// freshly-booted device model as `host_statistics`.
fn host_statistics64(
    env: &mut Environment,
    host: host_t,
    flavor: host_flavor_t,
    host_info_out: host_info_t,
    host_info_out_count: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    if host != MACH_HOST_SELF {
        log!(
            "host_statistics64: unexpected host port {:#010x} (expected MACH_HOST_SELF {:#010x}); \
             returning KERN_INVALID_ARGUMENT",
            host,
            MACH_HOST_SELF
        );
        return KERN_INVALID_ARGUMENT;
    }
    if flavor != HOST_VM_INFO64 {
        log!(
            "host_statistics64: unsupported flavor {}; returning KERN_INVALID_ARGUMENT",
            flavor
        );
        return KERN_INVALID_ARGUMENT;
    }
    let available = env.mem.read(host_info_out_count);
    // Callers occasionally pass a byte-size count by mistake (as with
    // host_statistics); writing `available` words could then overflow the
    // buffer, so cap the write at a generous upper bound for the struct
    // (the real iOS 6 count is well under this).
    let fields_to_write = available.min(64);
    let physical = physical_memory(env);
    let total_pages = (physical / PAGE_SIZE as u64) as natural_t;
    let free_count = total_pages / 4;
    let inactive_count = total_pages / 8;
    let wire_count = total_pages / 4;
    let active_count = total_pages - free_count - inactive_count - wire_count;
    // vm_statistics64 begins with the same four natural_t counts as
    // vm_statistics (free/active/inactive/wire); everything after them is
    // 64-bit statistics that may safely read as zero on a quiet device.
    let field_size = guest_size_of::<natural_t>() as GuestUSize;
    for i in 0..fields_to_write {
        let value: natural_t = match i {
            0 => free_count,
            1 => active_count,
            2 => inactive_count,
            3 => wire_count,
            _ => 0,
        };
        env.mem.write(
            (host_info_out + (i as GuestUSize * field_size)).cast(),
            value,
        );
    }
    env.mem.write(host_info_out_count, fields_to_write);
    KERN_SUCCESS
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mach_host_self()),
    export_c_func!(host_page_size(_, _)),
    export_c_func!(host_statistics(_, _, _, _)),
    export_c_func!(host_statistics64(_, _, _, _)),
    export_c_func!(host_get_clock_service(_, _, _)),
    export_c_func!(clock_get_time(_, _)),
    export_c_func!(clock_get_attributes(_, _, _, _)),
];
