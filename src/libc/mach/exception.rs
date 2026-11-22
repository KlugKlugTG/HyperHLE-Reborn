/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Mach exception server entry points (`exc.h`).
//!
//! These MIG-generated functions are called by crash reporters and old
//! exception handlers to continue/forward a Mach exception. HyperHLE never
//! delivers Mach exceptions (the guest runs as ordinary host threads), so
//! there is nothing to forward. Returning `KERN_FAILURE` tells the caller
//! that the exception could not be handled, which is the safe answer for an
//! unsupported exception path.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{MutPtr, MutVoidPtr};
use crate::Environment;

const KERN_FAILURE: i32 = 5;

fn exception_raise(
    _env: &mut Environment,
    _exception_port: u32,
    _thread: u32,
    _task: u32,
    _exception: u32,
    _code: MutVoidPtr,
    _code_count: u32,
) -> i32 {
    KERN_FAILURE
}

fn exception_raise_state(
    _env: &mut Environment,
    _exception_port: u32,
    _flavor: i32,
    _old_state: MutVoidPtr,
    _old_state_count: u32,
    _new_state: MutVoidPtr,
    _new_state_count: MutPtr<u32>,
) -> i32 {
    KERN_FAILURE
}

fn exception_raise_state_identity(
    _env: &mut Environment,
    _exception_port: u32,
    _thread: u32,
    _task: u32,
    _flavor: i32,
    _old_state: MutVoidPtr,
    _old_state_count: u32,
    _new_state: MutVoidPtr,
    _new_state_count: MutPtr<u32>,
) -> i32 {
    KERN_FAILURE
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(exception_raise(_, _, _, _, _, _)),
    export_c_func!(exception_raise_state(_, _, _, _, _, _)),
    export_c_func!(exception_raise_state_identity(_, _, _, _, _, _, _, _)),
];
