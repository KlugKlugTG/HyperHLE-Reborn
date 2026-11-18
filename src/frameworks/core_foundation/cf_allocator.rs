/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFAllocator`.
//!
//! The built-in allocators (default, system default, malloc, malloc zone)
//! all use the guest heap. `kCFAllocatorNull` fails every allocation and
//! never frees. Allocators made with `CFAllocatorCreate` call the guest's
//! own callbacks, but only through the `CFAllocator*` functions below.
//! Other CF files that are handed a custom allocator still allocate from
//! the system heap.

use super::CFIndex;
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{ConstantExports, FunctionExports, HostConstant};
use crate::environment::Environment;
use crate::export_c_func;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead};

// Allocator kinds. Plain integers, not a Rust enum: the struct is read
// straight out of guest memory, and an enum with an invalid discriminant
// would be undefined behaviour.
const KIND_SYSTEM_DEFAULT: u32 = 1;
const KIND_MALLOC: u32 = 2;
const KIND_MALLOC_ZONE: u32 = 3;
const KIND_NULL: u32 = 4;
const KIND_CUSTOM: u32 = 5;

/// Mirrors `CFAllocatorContext`: nine 4-byte fields on 32-bit ARM
/// (36 bytes). Function pointers are kept as raw guest addresses.
#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
pub struct CFAllocatorContext {
    version: i32,
    info: u32,
    retain: u32,
    release: u32,
    copy_description: u32,
    allocate: u32,
    reallocate: u32,
    deallocate: u32,
    preferred_size: u32,
}
unsafe impl SafeRead for CFAllocatorContext {}

// CFAllocator is an opaque struct. It isn't an Obj-C object here, so it
// lives in plain guest memory.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct CFAllocatorHostObject {
    kind: u32,
    context: CFAllocatorContext,
}
unsafe impl SafeRead for CFAllocatorHostObject {}

impl CFAllocatorHostObject {
    fn builtin(kind: u32) -> Self {
        CFAllocatorHostObject {
            kind,
            context: CFAllocatorContext::default(),
        }
    }

    /// Other CF files use this as their "is this allocator acceptable?"
    /// check, so it stays lenient: true for every valid kind (including
    /// Null and custom). Garbage memory is rejected.
    pub fn is_system_default(&self) -> bool {
        let kind = self.kind;
        (KIND_SYSTEM_DEFAULT..=KIND_CUSTOM).contains(&kind)
    }
}

pub type CFAllocatorRef = ConstPtr<CFAllocatorHostObject>;

pub const kCFAllocatorDefault: CFAllocatorRef = Ptr::null();

fn builtin_allocator_constant(env: &mut Environment, kind: u32) -> ConstVoidPtr {
    // The constant is a pointer variable, so the symbol address must point
    // at a cell that holds the pointer to the allocator object.
    let object = env.mem.alloc_and_write(CFAllocatorHostObject::builtin(kind));
    env.mem.alloc_and_write(object).cast().cast_const()
}

pub const CONSTANTS: ConstantExports = &[
    ("_kCFAllocatorDefault", HostConstant::NullPtr),
    (
        "_kCFAllocatorSystemDefault",
        HostConstant::Custom(|env| builtin_allocator_constant(env, KIND_SYSTEM_DEFAULT)),
    ),
    (
        "_kCFAllocatorMalloc",
        HostConstant::Custom(|env| builtin_allocator_constant(env, KIND_MALLOC)),
    ),
    (
        "_kCFAllocatorMallocZone",
        HostConstant::Custom(|env| builtin_allocator_constant(env, KIND_MALLOC_ZONE)),
    ),
    (
        "_kCFAllocatorNull",
        HostConstant::Custom(|env| builtin_allocator_constant(env, KIND_NULL)),
    ),
    // Only meaningful as the first argument of CFAllocatorCreate, which
    // ignores that argument.
    ("_kCFAllocatorUseContext", HostConstant::NullPtr),
];

// MARK: - Helpers

enum Backend {
    System,
    /// `kCFAllocatorNull`: allocation fails, deallocation does nothing.
    Fail,
    Custom(CFAllocatorContext),
}

fn backend(env: &mut Environment, allocator: CFAllocatorRef) -> Backend {
    if allocator.is_null() {
        return Backend::System;
    }
    let object = env.mem.read(allocator);
    let kind = object.kind;
    match kind {
        KIND_NULL => Backend::Fail,
        KIND_CUSTOM => Backend::Custom(object.context),
        _ => Backend::System,
    }
}

fn guest_fn(addr: u32) -> Option<GuestFunction> {
    (addr != 0).then(|| GuestFunction::from_addr_with_thumb_bit(addr))
}

/// `CFIndex` is signed, so a size with the top bit set is negative.
fn size_is_negative(size: GuestUSize) -> bool {
    size > i32::MAX as GuestUSize
}

fn custom_allocate(
    env: &mut Environment,
    ctx: CFAllocatorContext,
    size: GuestUSize,
    hint: u32,
) -> MutVoidPtr {
    let (allocate, info) = (ctx.allocate, ctx.info);
    let Some(f) = guest_fn(allocate) else {
        return Ptr::null();
    };
    f.call_from_host(env, (size as CFIndex, hint, MutVoidPtr::from_bits(info)))
}

fn custom_deallocate(env: &mut Environment, ctx: CFAllocatorContext, ptr: MutVoidPtr) {
    let (deallocate, info) = (ctx.deallocate, ctx.info);
    if let Some(f) = guest_fn(deallocate) {
        let _: () = f.call_from_host(env, (ptr, MutVoidPtr::from_bits(info)));
    }
}

// MARK: - Type / default allocator

// Placeholder type ID ('CFAl'). Delete this and its FUNCTIONS entry if
// cf_type.rs already exports CFAllocatorGetTypeID.
fn CFAllocatorGetTypeID(_env: &mut Environment) -> u32 {
    0x4346416c
}

/// The default allocator is not tracked per thread: a NULL allocator means
/// the system allocator everywhere in touchHLE.
fn CFAllocatorSetDefault(_env: &mut Environment, allocator: CFAllocatorRef) {
    log_dbg!(
        "CFAllocatorSetDefault({:?}): ignored, default allocator is not tracked",
        allocator
    );
}

fn CFAllocatorGetDefault(_env: &mut Environment) -> CFAllocatorRef {
    kCFAllocatorDefault
}

/// `CFAllocatorRef CFAllocatorCreate(CFAllocatorRef allocator,
///                                   CFAllocatorContext *context)`
fn CFAllocatorCreate(
    env: &mut Environment,
    _allocator: CFAllocatorRef, // the allocator object always lives in guest memory
    context: ConstPtr<CFAllocatorContext>,
) -> CFAllocatorRef {
    if context.is_null() {
        return Ptr::null();
    }
    let mut ctx = env.mem.read(context);
    let version = ctx.version;
    if version != 0 {
        log_dbg!("CFAllocatorCreate: unsupported context version {}", version);
        return Ptr::null();
    }
    // As in CF, `retain` (if present) is applied to `info` once, up front.
    let (retain, info) = (ctx.retain, ctx.info);
    if let Some(f) = guest_fn(retain) {
        let new_info: ConstVoidPtr = f.call_from_host(env, (ConstVoidPtr::from_bits(info),));
        ctx.info = new_info.to_bits();
    }
    env.mem
        .alloc_and_write(CFAllocatorHostObject {
            kind: KIND_CUSTOM,
            context: ctx,
        })
        .cast_const()
}

// MARK: - Allocate / reallocate / deallocate

/// Allocating 0 bytes returns NULL, as in CF. Negative sizes fail.
fn CFAllocatorAllocate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    size: GuestUSize,
    hint: u32,
) -> MutVoidPtr {
    if size == 0 || size_is_negative(size) {
        return Ptr::null();
    }
    match backend(env, allocator) {
        Backend::Fail => Ptr::null(),
        Backend::System => env.mem.alloc(size),
        Backend::Custom(ctx) => custom_allocate(env, ctx, size, hint),
    }
}

/// `realloc` semantics: NULL `ptr` allocates, zero `new_size` frees and
/// returns NULL.
fn CFAllocatorReallocate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    ptr: MutVoidPtr,
    new_size: GuestUSize,
    hint: u32,
) -> MutVoidPtr {
    if size_is_negative(new_size) {
        return Ptr::null();
    }
    match backend(env, allocator) {
        Backend::Fail => Ptr::null(),
        Backend::System => {
            if new_size == 0 {
                if !ptr.is_null() {
                    env.mem.free(ptr);
                }
                return Ptr::null();
            }
            if ptr.is_null() {
                return env.mem.alloc(new_size);
            }
            env.mem.realloc(ptr, new_size)
        }
        Backend::Custom(ctx) => {
            if ptr.is_null() {
                return if new_size == 0 {
                    Ptr::null()
                } else {
                    custom_allocate(env, ctx, new_size, hint)
                };
            }
            if new_size == 0 {
                custom_deallocate(env, ctx, ptr);
                return Ptr::null();
            }
            let (reallocate, info) = (ctx.reallocate, ctx.info);
            let Some(f) = guest_fn(reallocate) else {
                return Ptr::null();
            };
            f.call_from_host(
                env,
                (ptr, new_size as CFIndex, hint, MutVoidPtr::from_bits(info)),
            )
        }
    }
}

/// A NULL pointer is a no-op, as per the CF docs.
fn CFAllocatorDeallocate(env: &mut Environment, allocator: CFAllocatorRef, ptr: MutVoidPtr) {
    if ptr.is_null() {
        return;
    }
    match backend(env, allocator) {
        Backend::Fail => {}
        Backend::System => env.mem.free(ptr),
        Backend::Custom(ctx) => custom_deallocate(env, ctx, ptr),
    }
}

/// The size the allocator would really allocate. Never less than `size`.
fn CFAllocatorGetPreferredSizeForSize(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    size: CFIndex,
    hint: u32,
) -> CFIndex {
    if size < 0 {
        return 0;
    }
    match backend(env, allocator) {
        Backend::Custom(ctx) => {
            let (preferred_size, info) = (ctx.preferred_size, ctx.info);
            let Some(f) = guest_fn(preferred_size) else {
                return size;
            };
            let preferred: CFIndex =
                f.call_from_host(env, (size, hint, MutVoidPtr::from_bits(info)));
            preferred.max(size)
        }
        _ => size,
    }
}

/// Custom allocators return the context they were created with. Built-in
/// ones return an all-zero context.
fn CFAllocatorGetContext(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    context: MutPtr<CFAllocatorContext>,
) {
    if context.is_null() {
        return;
    }
    let ctx = match backend(env, allocator) {
        Backend::Custom(ctx) => ctx,
        _ => CFAllocatorContext::default(),
    };
    env.mem.write(context, ctx);
}

/// `CFGetAllocator(cf)` — every object in touchHLE uses the system default.
fn CFGetAllocator(env: &mut Environment, _cf: ConstVoidPtr) -> CFAllocatorRef {
    CFAllocatorGetDefault(env)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFAllocatorGetTypeID()),
    export_c_func!(CFAllocatorSetDefault(_)),
    export_c_func!(CFAllocatorGetDefault()),
    export_c_func!(CFAllocatorCreate(_, _)),
    export_c_func!(CFAllocatorAllocate(_, _, _)),
    export_c_func!(CFAllocatorReallocate(_, _, _, _)),
    export_c_func!(CFAllocatorDeallocate(_, _)),
    export_c_func!(CFAllocatorGetPreferredSizeForSize(_, _, _)),
    export_c_func!(CFAllocatorGetContext(_, _)),
    export_c_func!(CFGetAllocator(_)),
];
