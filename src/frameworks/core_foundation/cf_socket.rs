/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `CFSocket`
//!
//! There are no real sockets behind this: sockets can only be created by
//! wrapping an existing descriptor (`CFSocketCreateWithNative`), and callbacks
//! are never delivered because no run loop source is ever produced.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFRelease, CFRetain, CFTypeRef};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{nil, objc_classes, ClassExports, HostObject};
use crate::Environment;
use std::sync::atomic::{AtomicU16, Ordering};

pub type CFSocketRef = CFTypeRef;
pub type CFSocketNativeHandle = i32;
pub type CFSocketCallBackType = u32;
pub type CFSocketError = i32;

// CFSocketCallBackType flags
const kCFSocketNoCallBack: CFSocketCallBackType = 0;
const kCFSocketReadCallBack: CFSocketCallBackType = 1;
const kCFSocketAcceptCallBack: CFSocketCallBackType = 2;
const kCFSocketDataCallBack: CFSocketCallBackType = 3;
const kCFSocketConnectCallBack: CFSocketCallBackType = 4;
const kCFSocketWriteCallBack: CFSocketCallBackType = 8;

// Socket flags (CFSocketSetSocketFlags)
pub const kCFSocketAutomaticallyReenableReadCallBack: u32 = 1;
pub const kCFSocketAutomaticallyReenableAcceptCallBack: u32 = 2;
pub const kCFSocketAutomaticallyReenableDataCallBack: u32 = 3;
pub const kCFSocketAutomaticallyReenableWriteCallBack: u32 = 8;
pub const kCFSocketLeaveErrors: u32 = 64;
pub const kCFSocketCloseOnInvalidate: u32 = 128;

// CFSocketError values
const kCFSocketSuccess: CFSocketError = 0;
const kCFSocketError: CFSocketError = -1;
const kCFSocketTimeout: CFSocketError = -2;

const kCFSocketDefaultNameRegistryPortNumber: u16 = 2454;
static NAME_REGISTRY_PORT: AtomicU16 = AtomicU16::new(kCFSocketDefaultNameRegistryPortNumber);

// CFRunLoopSource — opaque stub ref reused from CFTypeRef
pub type CFRunLoopSourceRef = CFTypeRef;

/// Mirrors `CFSocketContext`: five 4-byte fields on 32-bit ARM (20 bytes).
/// Function pointers are kept as raw guest addresses (0 = none).
#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
pub struct CFSocketContext {
    version: CFIndex,
    info: u32,
    retain: u32,
    release: u32,
    copy_description: u32,
}
unsafe impl SafeRead for CFSocketContext {}

#[derive(Default)]
struct CFSocketHostObject {
    native_handle: CFSocketNativeHandle,
    valid: bool,
    flags: u32,
    context: CFSocketContext,
}
impl HostObject for CFSocketHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CFSocket: NSObject
- (())dealloc {
    // A socket that was never invalidated still owns a reference to `info`.
    invalidate_inner(env, this);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

// MARK: - Helpers

fn allocator_ok(env: &mut Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault
        || allocator.is_null()
        || env.mem.read(allocator).is_system_default()
}

fn guest_fn(addr: u32) -> Option<GuestFunction> {
    (addr != 0).then(|| GuestFunction::from_addr_with_thumb_bit(addr))
}

fn alloc_socket(
    env: &mut Environment,
    fd: CFSocketNativeHandle,
    context: CFSocketContext,
) -> CFSocketRef {
    let class = env.objc.get_known_class("_touchHLE_CFSocket", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CFSocketHostObject {
            native_handle: fd,
            valid: true,
            flags: 0,
            context,
        }),
        &mut env.mem,
    )
}

/// Marks the socket invalid and releases its context `info` exactly once.
fn invalidate_inner(env: &mut Environment, s: CFSocketRef) {
    let (info, release_cb) = {
        let h = env.objc.borrow_mut::<CFSocketHostObject>(s);
        if !h.valid {
            return;
        }
        h.valid = false;
        h.native_handle = -1;
        let ctx = h.context;
        h.context = CFSocketContext::default();
        (ctx.info, ctx.release)
    };
    if let Some(f) = guest_fn(release_cb) {
        let _: () = f.call_from_host(env, (ConstVoidPtr::from_bits(info),));
    }
}

// MARK: - Type ID

// Placeholder type ID ('CFSk'). Delete this and its FUNCTIONS entry if
// cf_type.rs already exports CFSocketGetTypeID.
fn CFSocketGetTypeID(_env: &mut Environment) -> u32 {
    0x4346536b
}

// MARK: - Lifecycle

pub fn CFSocketRetain(env: &mut Environment, s: CFSocketRef) -> CFSocketRef {
    if !s.is_null() {
        CFRetain(env, s)
    } else {
        s
    }
}

pub fn CFSocketRelease(env: &mut Environment, s: CFSocketRef) {
    if !s.is_null() {
        CFRelease(env, s);
    }
}

// MARK: - Constructors

/// There is no way to create a real socket here, so this fails like CF does
/// when the system call fails: it returns NULL.
fn CFSocketCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    protocol_family: i32,
    socket_type: i32,
    protocol: i32,
    callback_types: CFSocketCallBackType,
    _callout: MutVoidPtr,
    _context: ConstPtr<CFSocketContext>,
) -> CFSocketRef {
    if !allocator_ok(env, allocator) {
        return nil;
    }
    log_dbg!(
        "CFSocketCreate: family={} type={} protocol={} callbacks={} — unsupported, returning NULL",
        protocol_family,
        socket_type,
        protocol,
        callback_types
    );
    nil
}

fn CFSocketCreateWithNative(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    sock: CFSocketNativeHandle,
    callback_types: CFSocketCallBackType,
    _callout: MutVoidPtr,
    context: ConstPtr<CFSocketContext>,
) -> CFSocketRef {
    // A negative descriptor (e.g. -1 from a failed socket()) is never valid.
    if !allocator_ok(env, allocator) || sock < 0 {
        return nil;
    }
    log_dbg!(
        "CFSocketCreateWithNative: fd={} callbacks={}",
        sock,
        callback_types
    );
    let mut ctx = if context.is_null() {
        CFSocketContext::default()
    } else {
        env.mem.read(context)
    };
    // As in CF, `retain` (if present) is applied to `info` once, up front.
    let (retain_cb, info) = (ctx.retain, ctx.info);
    if let Some(f) = guest_fn(retain_cb) {
        let new_info: ConstVoidPtr = f.call_from_host(env, (ConstVoidPtr::from_bits(info),));
        ctx.info = new_info.to_bits();
    }
    alloc_socket(env, sock, ctx)
}

fn CFSocketCreateWithSocketSignature(
    _env: &mut Environment,
    _allocator: CFAllocatorRef,
    _signature: ConstVoidPtr, // const CFSocketSignature*
    _callback_types: CFSocketCallBackType,
    _callout: MutVoidPtr,
    _context: ConstPtr<CFSocketContext>,
) -> CFSocketRef {
    log_dbg!("CFSocketCreateWithSocketSignature: unsupported, returning NULL");
    nil
}

fn CFSocketCreateConnectedToSocketSignature(
    _env: &mut Environment,
    _allocator: CFAllocatorRef,
    _signature: ConstVoidPtr,
    _callback_types: CFSocketCallBackType,
    _callout: MutVoidPtr,
    _context: ConstPtr<CFSocketContext>,
    _timeout: f64,
) -> CFSocketRef {
    log_dbg!("CFSocketCreateConnectedToSocketSignature: unsupported, returning NULL");
    nil
}

// MARK: - Accessors

/// -1 for NULL or invalidated sockets, as in CF.
fn CFSocketGetNative(env: &mut Environment, s: CFSocketRef) -> CFSocketNativeHandle {
    if s.is_null() {
        return -1;
    }
    let h = env.objc.borrow::<CFSocketHostObject>(s);
    if h.valid {
        h.native_handle
    } else {
        -1
    }
}

fn CFSocketGetContext(env: &mut Environment, s: CFSocketRef, context: MutPtr<CFSocketContext>) {
    if s.is_null() || context.is_null() {
        return;
    }
    let ctx = env.objc.borrow::<CFSocketHostObject>(s).context;
    env.mem.write(context, ctx);
}

fn CFSocketIsValid(env: &mut Environment, s: CFSocketRef) -> bool {
    if s.is_null() {
        return false;
    }
    env.objc.borrow::<CFSocketHostObject>(s).valid
}

fn CFSocketInvalidate(env: &mut Environment, s: CFSocketRef) {
    if s.is_null() {
        return;
    }
    invalidate_inner(env, s);
}

// MARK: - Callbacks / flags

// Callbacks are never delivered (see CFSocketCreateRunLoopSource), so
// enabling and disabling them has nothing to change.
fn CFSocketEnableCallBacks(
    _env: &mut Environment,
    _s: CFSocketRef,
    _callback_types: CFSocketCallBackType,
) {
    log_dbg!("CFSocketEnableCallBacks: no-op");
}

fn CFSocketDisableCallBacks(
    _env: &mut Environment,
    _s: CFSocketRef,
    _callback_types: CFSocketCallBackType,
) {
    log_dbg!("CFSocketDisableCallBacks: no-op");
}

fn CFSocketSetSocketFlags(env: &mut Environment, s: CFSocketRef, flags: u32) {
    if s.is_null() {
        return;
    }
    env.objc.borrow_mut::<CFSocketHostObject>(s).flags = flags;
}

fn CFSocketGetSocketFlags(env: &mut Environment, s: CFSocketRef) -> u32 {
    if s.is_null() {
        return 0;
    }
    env.objc.borrow::<CFSocketHostObject>(s).flags
}

// MARK: - Address / data (no real socket behind these)

fn CFSocketCopyAddress(_env: &mut Environment, _s: CFSocketRef) -> CFTypeRef {
    nil // CFDataRef
}

fn CFSocketCopyPeerAddress(_env: &mut Environment, _s: CFSocketRef) -> CFTypeRef {
    nil
}

fn CFSocketSetAddress(
    _env: &mut Environment,
    _s: CFSocketRef,
    _address: CFTypeRef, // CFDataRef
) -> CFSocketError {
    log_dbg!("CFSocketSetAddress: unsupported");
    kCFSocketError
}

fn CFSocketConnectToAddress(
    _env: &mut Environment,
    _s: CFSocketRef,
    _address: CFTypeRef, // CFDataRef
    _timeout: f64,
) -> CFSocketError {
    log_dbg!("CFSocketConnectToAddress: unsupported");
    kCFSocketError
}

fn CFSocketSendData(
    _env: &mut Environment,
    _s: CFSocketRef,
    _address: CFTypeRef, // CFDataRef
    _data: CFTypeRef,    // CFDataRef
    _timeout: f64,
) -> CFSocketError {
    log_dbg!("CFSocketSendData: unsupported");
    kCFSocketError
}

// MARK: - Run loop source

fn CFSocketCreateRunLoopSource(
    _env: &mut Environment,
    _allocator: CFAllocatorRef,
    _s: CFSocketRef,
    _order: i32,
) -> CFRunLoopSourceRef {
    log_dbg!("CFSocketCreateRunLoopSource: unsupported, returning NULL");
    nil
}

// MARK: - Name registry (there is no name server, so every call fails)

fn CFSocketRegisterValue(
    _env: &mut Environment,
    _name_server_signature: ConstVoidPtr, // const CFSocketSignature*
    _timeout: f64,
    _name: CFTypeRef,  // CFStringRef
    _value: CFTypeRef, // CFPropertyListRef
) -> CFSocketError {
    log_dbg!("CFSocketRegisterValue: no name server");
    kCFSocketError
}

fn CFSocketCopyRegisteredValue(
    env: &mut Environment,
    _name_server_signature: ConstVoidPtr,
    _timeout: f64,
    _name: CFTypeRef,
    value: MutPtr<CFTypeRef>,
    name_server_address: MutPtr<CFTypeRef>,
) -> CFSocketError {
    if !value.is_null() {
        env.mem.write(value, nil);
    }
    if !name_server_address.is_null() {
        env.mem.write(name_server_address, nil);
    }
    log_dbg!("CFSocketCopyRegisteredValue: no name server");
    kCFSocketError
}

fn CFSocketRegisterSocketSignature(
    _env: &mut Environment,
    _name_server_signature: ConstVoidPtr,
    _timeout: f64,
    _name: CFTypeRef,
    _signature: ConstVoidPtr, // const CFSocketSignature*
) -> CFSocketError {
    log_dbg!("CFSocketRegisterSocketSignature: no name server");
    kCFSocketError
}

fn CFSocketCopyRegisteredSocketSignature(
    env: &mut Environment,
    _name_server_signature: ConstVoidPtr,
    _timeout: f64,
    _name: CFTypeRef,
    _signature: MutVoidPtr, // CFSocketSignature*, left untouched on failure
    name_server_address: MutPtr<CFTypeRef>,
) -> CFSocketError {
    if !name_server_address.is_null() {
        env.mem.write(name_server_address, nil);
    }
    log_dbg!("CFSocketCopyRegisteredSocketSignature: no name server");
    kCFSocketError
}

fn CFSocketUnregister(
    _env: &mut Environment,
    _name_server_signature: ConstVoidPtr,
    _timeout: f64,
    _name: CFTypeRef,
) -> CFSocketError {
    log_dbg!("CFSocketUnregister: no name server");
    kCFSocketError
}

/// Process-wide, not per `Environment`.
fn CFSocketSetDefaultNameRegistryPortNumber(_env: &mut Environment, port: u16) {
    NAME_REGISTRY_PORT.store(port, Ordering::Relaxed);
}

fn CFSocketGetDefaultNameRegistryPortNumber(_env: &mut Environment) -> u16 {
    NAME_REGISTRY_PORT.load(Ordering::Relaxed)
}

pub const CONSTANTS: ConstantExports = &[
    ("_kCFSocketCommandKey", HostConstant::NSString("Command")),
    ("_kCFSocketNameKey", HostConstant::NSString("Name")),
    ("_kCFSocketValueKey", HostConstant::NSString("Value")),
    ("_kCFSocketResultKey", HostConstant::NSString("Result")),
    ("_kCFSocketErrorKey", HostConstant::NSString("Error")),
    ("_kCFSocketRegisterCommand", HostConstant::NSString("Register")),
    ("_kCFSocketRetrieveCommand", HostConstant::NSString("Retrieve")),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFSocketGetTypeID()),
    export_c_func!(CFSocketRetain(_)),
    export_c_func!(CFSocketRelease(_)),
    export_c_func!(CFSocketCreate(_, _, _, _, _, _, _)),
    export_c_func!(CFSocketCreateWithNative(_, _, _, _, _)),
    export_c_func!(CFSocketCreateWithSocketSignature(_, _, _, _, _)),
    export_c_func!(CFSocketCreateConnectedToSocketSignature(_, _, _, _, _, _)),
    export_c_func!(CFSocketGetNative(_)),
    export_c_func!(CFSocketGetContext(_, _)),
    export_c_func!(CFSocketIsValid(_)),
    export_c_func!(CFSocketInvalidate(_)),
    export_c_func!(CFSocketEnableCallBacks(_, _)),
    export_c_func!(CFSocketDisableCallBacks(_, _)),
    export_c_func!(CFSocketSetSocketFlags(_, _)),
    export_c_func!(CFSocketGetSocketFlags(_)),
    export_c_func!(CFSocketCopyAddress(_)),
    export_c_func!(CFSocketCopyPeerAddress(_)),
    export_c_func!(CFSocketSetAddress(_, _)),
    export_c_func!(CFSocketConnectToAddress(_, _, _)),
    export_c_func!(CFSocketSendData(_, _, _, _)),
    export_c_func!(CFSocketCreateRunLoopSource(_, _, _)),
    export_c_func!(CFSocketRegisterValue(_, _, _, _)),
    export_c_func!(CFSocketCopyRegisteredValue(_, _, _, _, _)),
    export_c_func!(CFSocketRegisterSocketSignature(_, _, _, _)),
    export_c_func!(CFSocketCopyRegisteredSocketSignature(_, _, _, _, _)),
    export_c_func!(CFSocketUnregister(_, _, _)),
    export_c_func!(CFSocketSetDefaultNameRegistryPortNumber(_)),
    export_c_func!(CFSocketGetDefaultNameRegistryPortNumber()),
];
