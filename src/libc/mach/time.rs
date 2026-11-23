/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach_time.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{MutPtr, SafeRead};
use crate::Environment;
use std::time::Duration;

#[repr(C, packed)]
struct struct_mach_timebase_info {
    numerator: u32,
    denominator: u32,
}
unsafe impl SafeRead for struct_mach_timebase_info {}

#[allow(non_camel_case_types)]
type kern_return_t = i32;
const KERN_SUCCESS: kern_return_t = 0;

fn mach_timebase_info(
    env: &mut Environment,
    info: MutPtr<struct_mach_timebase_info>,
) -> kern_return_t {
    env.mem.write(
        info,
        struct_mach_timebase_info {
            numerator: 1,
            denominator: 1,
        },
    );
    KERN_SUCCESS
}

/// The result of this function, multiplied by the constant from
/// [mach_timebase_info], should be the absolute time in nanoseconds.
/// The absolute time is a monotonic clock with an arbitrary starting point.
fn mach_absolute_time(env: &mut Environment) -> u64 {
    let now = env.guest_clock.now();
    now.duration_since(env.startup_time)
        .as_nanos()
        .try_into()
        .unwrap()
}

/// Sleeps the current thread until the given deadline, in absolute time
/// units (see [mach_absolute_time]). With the 1:1 timebase written by
/// [mach_timebase_info], the deadline is in nanoseconds.
///
/// This must be a real blocking implementation: some apps call it in a
/// tight loop to pace their frame rate or poll loop. A return-0 stub makes
/// them busy-spin, hammering the host CPU and starving other guest
/// threads, and was observed right before a SIGSEGV in Bioshock.
// https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/mach_time.h
// This returns kern_return_t: leaving r0 untouched exposes the deadline's
// low word as a spurious error to engines that check the wait result.
fn mach_wait_until(env: &mut Environment, deadline: u64) -> kern_return_t {
    let now = env.guest_clock.now();
    let now_nanos: u64 = now
        .duration_since(env.startup_time)
        .as_nanos()
        .try_into()
        .unwrap();
    if deadline > now_nanos {
        env.sleep_guest(Duration::from_nanos(deadline - now_nanos));
    }
    KERN_SUCCESS
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mach_timebase_info(_)),
    export_c_func!(mach_absolute_time()),
    export_c_func!(mach_wait_until(_)),
];
