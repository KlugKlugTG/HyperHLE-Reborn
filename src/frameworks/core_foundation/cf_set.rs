/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFSet` and `CFMutableSet`.
//!
//! These are toll-free bridged to `NSSet` / `NSMutableSet` in Apple's
//! implementation. Here they are the same type.
//!
//! Every set is backed by an `NSMutableSet`, so members must be Objective-C
//! objects, and the callbacks passed at creation are ignored (see
//! `CFSetCreateMutable`).
//!
//! References:
//! - Apple [CFSet Reference](https://developer.apple.com/documentation/corefoundation/cfset-rul)
//! - Apple [CFMutableSet Reference](https://developer.apple.com/documentation/corefoundation/cfmutableset)

use super::cf_allocator::CFAllocatorRef;
use super::{CFIndex, CFTypeRef};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{id, msg, msg_class, nil, release, retain};
use crate::Environment;

pub type CFSetRef = CFTypeRef;
pub type CFMutableSetRef = CFTypeRef;

/// Apple's `CFSetCallBacks`, declared in `CFSet.h` as `{version, retain,
/// release, copyDescription, equal, hash}`. We accept any pointer (or NULL):
/// the underlying NSSet handles retain/release through Objective-C semantics.
#[repr(C, packed)]
#[allow(dead_code)]
pub struct CFSetCallBacks {
    pub version: CFIndex,
    pub retain: ConstVoidPtr,
    pub release: ConstVoidPtr,
    pub copy_description: ConstVoidPtr,
    pub equal: ConstVoidPtr,
    pub hash: ConstVoidPtr,
}
unsafe impl crate::mem::SafeRead for CFSetCallBacks {}

// MARK: - Type ID

// Placeholder type ID ('CFSe'), like the other fake IDs in this codebase.
// Delete this and its FUNCTIONS entry if cf_type.rs already exports
// CFSetGetTypeID.
fn CFSetGetTypeID(_env: &mut Environment) -> u32 {
    0x43465365
}

// MARK: - Constructors

fn CFSetCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    values: ConstPtr<ConstVoidPtr>,
    num_values: CFIndex,
    callbacks: ConstPtr<CFSetCallBacks>,
) -> CFSetRef {
    // A negative count would otherwise wrap to ~4 billion iterations.
    let Ok(count) = u32::try_from(num_values) else {
        return nil;
    };
    if count > 0 && values.is_null() {
        return nil;
    }
    // Returns a mutable set; apps never see the difference unless they
    // mutate a set that real CF would have made immutable.
    let set = CFSetCreateMutable(env, allocator, num_values, callbacks);
    for i in 0..count {
        let value: ConstVoidPtr = env.mem.read(values + i);
        if value.is_null() {
            continue; // NULL can't be an NSSet member
        }
        let value_id: id = value.cast().cast_mut();
        () = msg![env; set addObject:value_id];
    }
    set
}

fn CFSetCreateCopy(env: &mut Environment, _allocator: CFAllocatorRef, set: CFSetRef) -> CFSetRef {
    if set == nil {
        return nil;
    }
    let new: id = msg_class![env; NSSet alloc];
    msg![env; new initWithSet:set]
}

/// The capacity is a soft hint and `callbacks` is ignored: NSMutableSet
/// already retains, releases, hashes and compares its members.
pub fn CFSetCreateMutable(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    _capacity: CFIndex,
    _callbacks: ConstPtr<CFSetCallBacks>,
) -> CFMutableSetRef {
    let new: id = msg_class![env; NSMutableSet alloc];
    msg![env; new init]
}

fn CFSetCreateMutableCopy(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    _capacity: CFIndex,
    set: CFSetRef,
) -> CFMutableSetRef {
    let new: id = msg_class![env; NSMutableSet alloc];
    if set == nil {
        msg![env; new init]
    } else {
        msg![env; new initWithSet:set]
    }
}

// MARK: - Inspectors

fn CFSetGetCount(env: &mut Environment, set: CFSetRef) -> CFIndex {
    if set == nil {
        return 0;
    }
    let count: u32 = msg![env; set count];
    count as CFIndex
}

fn CFSetContainsValue(env: &mut Environment, set: CFSetRef, value: ConstVoidPtr) -> bool {
    if set == nil || value.is_null() {
        return false;
    }
    let value_id: id = value.cast().cast_mut();
    msg![env; set containsObject:value_id]
}

fn CFSetGetValue(env: &mut Environment, set: CFSetRef, value: ConstVoidPtr) -> ConstVoidPtr {
    if set == nil || value.is_null() {
        return ConstPtr::null();
    }
    let value_id: id = value.cast().cast_mut();
    let member: id = msg![env; set member:value_id];
    member.cast().cast_const()
}

/// If an equal member exists, stores it in `*value` (when non-NULL) and
/// returns true.
fn CFSetGetValueIfPresent(
    env: &mut Environment,
    set: CFSetRef,
    candidate: ConstVoidPtr,
    value: MutPtr<ConstVoidPtr>,
) -> bool {
    if set == nil || candidate.is_null() {
        return false;
    }
    let candidate_id: id = candidate.cast().cast_mut();
    let member: id = msg![env; set member:candidate_id];
    if member == nil {
        return false;
    }
    if !value.is_null() {
        let member_ptr: ConstVoidPtr = member.cast().cast_const();
        env.mem.write(value, member_ptr);
    }
    true
}

/// A set holds each value at most once, so the count is 0 or 1.
fn CFSetGetCountOfValue(env: &mut Environment, set: CFSetRef, value: ConstVoidPtr) -> CFIndex {
    if CFSetContainsValue(env, set, value) {
        1
    } else {
        0
    }
}

fn CFSetGetValues(env: &mut Environment, set: CFSetRef, values: MutPtr<ConstVoidPtr>) {
    if set == nil || values.is_null() {
        return;
    }
    let arr: id = msg![env; set allObjects];
    let count: u32 = msg![env; arr count];
    for i in 0..count {
        let obj: id = msg![env; arr objectAtIndex:i];
        env.mem.write(values + i, obj.cast().cast_const());
    }
}

/// Calls `applier(value, context)` for every member. Iterates a snapshot,
/// so the callback may safely remove members from the set.
fn CFSetApplyFunction(
    env: &mut Environment,
    set: CFSetRef,
    applier: GuestFunction,
    context: MutVoidPtr,
) {
    if set == nil || applier.to_ptr().is_null() {
        return;
    }
    let objects: id = msg![env; set allObjects];
    let count: u32 = msg![env; objects count];
    for i in 0..count {
        let obj: id = msg![env; objects objectAtIndex:i];
        let value: ConstVoidPtr = obj.cast().cast_const();
        let _: () = applier.call_from_host(env, (value, context));
    }
}

// MARK: - Mutation

fn CFSetAddValue(env: &mut Environment, set: CFMutableSetRef, value: ConstVoidPtr) {
    if set == nil || value.is_null() {
        return;
    }
    let value_id: id = value.cast().cast_mut();
    () = msg![env; set addObject:value_id];
}

/// Swaps an existing equal member for `value`. `value` is kept alive across
/// the removal: if it is the same object as the member and the set holds the
/// only reference, removing it would free it before it could be re-added.
fn replace_member(env: &mut Environment, set: CFMutableSetRef, value_id: id) {
    retain(env, value_id);
    () = msg![env; set removeObject:value_id];
    () = msg![env; set addObject:value_id];
    release(env, value_id);
}

fn CFSetReplaceValue(env: &mut Environment, set: CFMutableSetRef, value: ConstVoidPtr) {
    if set == nil || value.is_null() {
        return;
    }
    let value_id: id = value.cast().cast_mut();
    let contains: bool = msg![env; set containsObject:value_id];
    if contains {
        replace_member(env, set, value_id);
    }
}

/// Adds `value` if absent, replaces the equal member if present.
fn CFSetSetValue(env: &mut Environment, set: CFMutableSetRef, value: ConstVoidPtr) {
    if set == nil || value.is_null() {
        return;
    }
    let value_id: id = value.cast().cast_mut();
    let contains: bool = msg![env; set containsObject:value_id];
    if contains {
        replace_member(env, set, value_id);
    } else {
        () = msg![env; set addObject:value_id];
    }
}

fn CFSetRemoveValue(env: &mut Environment, set: CFMutableSetRef, value: ConstVoidPtr) {
    if set == nil || value.is_null() {
        return;
    }
    let value_id: id = value.cast().cast_mut();
    () = msg![env; set removeObject:value_id];
}

pub fn CFSetRemoveAllValues(env: &mut Environment, set: CFMutableSetRef) {
    if set == nil {
        return;
    }
    () = msg![env; set removeAllObjects];
}

// MARK: - Callback constants

/// Both constants are zero-initialised `CFSetCallBacks`: apps that read the
/// struct or take its address get a valid pointer with the documented layout.
/// `CFSetCreateMutable` ignores the callbacks anyway.
fn zeroed_callbacks(env: &mut Environment) -> ConstVoidPtr {
    let cb = CFSetCallBacks {
        version: 0,
        retain: Ptr::null(),
        release: Ptr::null(),
        copy_description: Ptr::null(),
        equal: Ptr::null(),
        hash: Ptr::null(),
    };
    env.mem.alloc_and_write(cb).cast_void().cast_const()
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFTypeSetCallBacks",
        HostConstant::Custom(|env| zeroed_callbacks(env)),
    ),
    (
        "_kCFCopyStringSetCallBacks",
        HostConstant::Custom(|env| zeroed_callbacks(env)),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFSetGetTypeID()),
    export_c_func!(CFSetCreate(_, _, _, _)),
    export_c_func!(CFSetCreateCopy(_, _)),
    export_c_func!(CFSetCreateMutable(_, _, _)),
    export_c_func!(CFSetCreateMutableCopy(_, _, _)),
    export_c_func!(CFSetGetCount(_)),
    export_c_func!(CFSetGetCountOfValue(_, _)),
    export_c_func!(CFSetContainsValue(_, _)),
    export_c_func!(CFSetGetValue(_, _)),
    export_c_func!(CFSetGetValueIfPresent(_, _, _)),
    export_c_func!(CFSetGetValues(_, _)),
    export_c_func!(CFSetApplyFunction(_, _, _)),
    export_c_func!(CFSetAddValue(_, _)),
    export_c_func!(CFSetReplaceValue(_, _)),
    export_c_func!(CFSetSetValue(_, _)),
    export_c_func!(CFSetRemoveValue(_, _)),
    export_c_func!(CFSetRemoveAllValues(_)),
];
