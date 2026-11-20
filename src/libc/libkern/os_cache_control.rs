/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Instruction cache maintenance from `libkern/OSCacheControl.h`.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{GuestUSize, MutVoidPtr};
use crate::Environment;

/// Prepare instructions written to guest memory for execution. Host memory is
/// coherent, but translated blocks and their links must be invalidated too.
/// https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/sys_icache_invalidate.3.html
fn sys_icache_invalidate(env: &mut Environment, start: MutVoidPtr, len: GuestUSize) {
    log_dbg!("sys_icache_invalidate({start:?}, {len:#x})");
    if len != 0 {
        env.cpu.invalidate_cache_range(start.to_bits(), len);
    }
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(sys_icache_invalidate(_, _))];
