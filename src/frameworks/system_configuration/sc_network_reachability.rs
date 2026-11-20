/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#![allow(dead_code)]
//! SCNetworkReachability

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_allocator::CFAllocatorRef;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::mem::{guest_size_of, ConstPtr, GuestISize, Mem, MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{nil, objc_classes, ClassExports, HostObject};
use crate::Environment;

type SCNetworkReachabilityFlags = u32;
const kSCNetworkReachabilityFlagsTransientConnection: SCNetworkReachabilityFlags = 1 << 0;
const kSCNetworkReachabilityFlagsReachable: SCNetworkReachabilityFlags = 1 << 1;
const kSCNetworkReachabilityFlagsConnectionRequired: SCNetworkReachabilityFlags = 1 << 2;
const kSCNetworkReachabilityFlagsConnectionOnTraffic: SCNetworkReachabilityFlags = 1 << 3;
const kSCNetworkReachabilityFlagsInterventionRequired: SCNetworkReachabilityFlags = 1 << 4;
const kSCNetworkReachabilityFlagsConnectionOnDemand: SCNetworkReachabilityFlags = 1 << 5;
const kSCNetworkReachabilityFlagsIsLocalAddress: SCNetworkReachabilityFlags = 1 << 16;
const kSCNetworkReachabilityFlagsIsDirect: SCNetworkReachabilityFlags = 1 << 17;
const kSCNetworkReachabilityFlagsIsWWAN: SCNetworkReachabilityFlags = 1 << 18;

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);
    @implementation _touchHLE_SCNetworkReachability: NSObject
    - (())dealloc {
        let (context, retired) = {
            let host = env.objc.borrow_mut::<SCNetworkReachabilityHostObject>(this);
            host.callout = None;
            (host.context.take(), std::mem::take(&mut host.retired_contexts))
        };
        env.objc.dealloc_object(this, &mut env.mem);
        release_context(env, context);
        for context in retired {
            release_context(env, Some(context));
        }
    }
    @end
};

/// iOS/ARM32 ABI: CFIndex followed by info and three function pointers.
/// The caller may put this on its stack. Store a COPY, never that stack address.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
struct SCNetworkReachabilityContext {
    version: GuestISize,
    info: MutVoidPtr,
    retain: GuestFunction,
    release: GuestFunction,
    copy_description: GuestFunction,
}
unsafe impl SafeRead for SCNetworkReachabilityContext {}

impl SCNetworkReachabilityContext {
    fn retain_with(&mut self, mut call: impl FnMut(GuestFunction, MutVoidPtr) -> MutVoidPtr) {
        if self.retain.addr_with_thumb_bit() != 0 {
            self.info = call(self.retain, self.info);
        }
    }

    fn release_with(self, mut call: impl FnMut(GuestFunction, MutVoidPtr)) {
        if self.release.addr_with_thumb_bit() != 0 {
            call(self.release, self.info);
        }
    }
}

fn read_context(
    mem: &Mem,
    ptr: ConstPtr<SCNetworkReachabilityContext>,
) -> Result<Option<SCNetworkReachabilityContext>, &'static str> {
    if ptr.is_null() {
        return Ok(None);
    }
    let size = guest_size_of::<SCNetworkReachabilityContext>();
    let end = ptr.to_bits() as u64 + size as u64;
    // get_bytes_fallible can return a SHORT synthetic null-page slice.
    if ptr.to_bits() < mem.null_segment_size()
        || end > u32::MAX as u64 + 1
        || mem.get_bytes_fallible(ptr.cast(), size).map(|b| b.len()) != Some(size as usize)
    {
        return Err("unreadable context");
    }
    let context: SCNetworkReachabilityContext = mem.read(ptr);
    if context.version != 0 {
        return Err("unsupported context version");
    }
    Ok(Some(context))
}

fn retain_context(env: &mut Environment, context: &mut Option<SCNetworkReachabilityContext>) {
    if let Some(context) = context {
        // Retain may return a different pointer: use it for callout/release.
        context.retain_with(|callback, info| callback.call_from_host(env, (info,)));
    }
}

fn release_context(env: &mut Environment, context: Option<SCNetworkReachabilityContext>) {
    if let Some(context) = context {
        context.release_with(|callback, info| {
            let _: () = callback.call_from_host(env, (info,));
        });
    }
}

#[derive(Default)]
struct SCNetworkReachabilityHostObject {
    name: Option<String>,
    callout: Option<GuestFunction>,
    context: Option<SCNetworkReachabilityContext>,
    active_callbacks: usize,
    retired_contexts: Vec<SCNetworkReachabilityContext>,
    /// Set by ScheduleWithRunLoop, cleared by UnscheduleFromRunLoop. Only used
    /// to give Unschedule a truthful return value: there is no real run-loop
    /// source behind it.
    scheduled: bool,
}
impl HostObject for SCNetworkReachabilityHostObject {}

impl SCNetworkReachabilityHostObject {
    /// Defer release if a callback replaces/unregisters its own context.
    fn replace_callback(
        &mut self,
        callout: Option<GuestFunction>,
        context: Option<SCNetworkReachabilityContext>,
    ) -> Option<SCNetworkReachabilityContext> {
        self.callout = callout;
        let old = std::mem::replace(&mut self.context, context);
        if self.active_callbacks == 0 {
            return old;
        }
        if let Some(old) = old {
            self.retired_contexts.push(old);
        }
        None
    }

    fn finish_callback(&mut self) -> Vec<SCNetworkReachabilityContext> {
        self.active_callbacks -= 1;
        if self.active_callbacks == 0 {
            std::mem::take(&mut self.retired_contexts)
        } else {
            Vec::new()
        }
    }
}

type SCNetworkReachabilityRef = CFTypeRef;

/// Whether `target` is a live reachability object. NULL and objects of other
/// classes are rejected; the guest controls this value, so it must not reach
/// a typed borrow unchecked.
fn is_reachability(env: &Environment, target: SCNetworkReachabilityRef) -> bool {
    if target.is_null() {
        return false;
    }
    env.objc
        .get_host_object(target)
        .is_some_and(|host| host.as_any().is::<SCNetworkReachabilityHostObject>())
}

/// Common tail of the three `SCNetworkReachabilityCreate*` functions. Returns
/// a +1 object, as a Create function owes.
fn alloc_reachability(env: &mut Environment, name: Option<String>) -> SCNetworkReachabilityRef {
    let isa = env
        .objc
        .get_known_class("_touchHLE_SCNetworkReachability", &mut env.mem);
    env.objc.alloc_object(
        isa,
        Box::new(SCNetworkReachabilityHostObject {
            name,
            ..Default::default()
        }),
        &mut env.mem,
    )
}

pub fn SCNetworkReachabilityRetain(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
) -> SCNetworkReachabilityRef {
    if !target.is_null() {
        CFRetain(env, target)
    } else {
        target
    }
}

pub fn SCNetworkReachabilityRelease(env: &mut Environment, target: SCNetworkReachabilityRef) {
    if !target.is_null() {
        CFRelease(env, target);
    }
}

fn SCNetworkReachabilityGetTypeID(_env: &mut Environment) -> u32 {
    // Stable non-zero value; apps only compare it against CFGetTypeID().
    0x5343_5243 // 'SCRC'
}

fn SCNetworkReachabilityCreateWithName(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    name: ConstPtr<u8>,
) -> SCNetworkReachabilityRef {
    // Real SC returns NULL for a NULL node name.
    if name.is_null() {
        return nil;
    }
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or("").to_string();
    alloc_reachability(env, Some(name_str))
}

fn SCNetworkReachabilityCreateWithAddress(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    address: ConstPtr<u8>,
) -> SCNetworkReachabilityRef {
    if address.is_null() {
        return nil;
    }
    alloc_reachability(env, None)
}

fn SCNetworkReachabilityCreateWithAddressPair(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    local: ConstPtr<u8>,
    remote: ConstPtr<u8>,
) -> SCNetworkReachabilityRef {
    // Both addresses are optional, but at least one must be given.
    if local.is_null() && remote.is_null() {
        return nil;
    }
    alloc_reachability(env, None)
}

/// Report a Wi-Fi-style route for real networking or local StoreKit emulation.
/// This is only a reachability hint: it must never enable sockets or HTTP.
/// Unity and common Reachability wrappers check Reachable and ConnectionRequired
/// before attempting StoreKit calls, even if the purchase itself is local.
fn reachability_flags(network_access: bool, iap_emulation: bool) -> SCNetworkReachabilityFlags {
    if network_access || iap_emulation {
        kSCNetworkReachabilityFlagsReachable
    } else {
        0
    }
}

fn current_flags(env: &Environment) -> SCNetworkReachabilityFlags {
    reachability_flags(
        env.options.network_access,
        crate::frameworks::store_kit::emulation_enabled(),
    )
}

fn SCNetworkReachabilityGetFlags(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
    flags: MutPtr<SCNetworkReachabilityFlags>,
) -> bool {
    // Real SC fails (and leaves *flags alone) for an invalid target.
    if !is_reachability(env, target) {
        return false;
    }
    let flags_value = current_flags(env);
    if !flags.is_null() {
        env.mem.write(flags, flags_value);
    }
    true
}

fn SCNetworkReachabilitySetCallback(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
    callout: GuestFunction,
    context: ConstPtr<SCNetworkReachabilityContext>,
) -> bool {
    if !is_reachability(env, target) {
        return false;
    }
    let callout = (callout.addr_with_thumb_bit() != 0).then_some(callout);
    let mut context = if callout.is_none() {
        // NULL callout unregisters the callback; do not retain a new context.
        None
    } else {
        match read_context(&env.mem, context) {
            Ok(context) => context,
            Err(reason) => {
                log!(
                    "SCNetworkReachabilitySetCallback: {}; keeping previous callback",
                    reason
                );
                return false;
            }
        }
    };
    // Guest retain/release can re-enter the runtime. Keep the target alive
    // and never hold a host-object borrow across guest execution.
    CFRetain(env, target);
    retain_context(env, &mut context);
    let old = env
        .objc
        .borrow_mut::<SCNetworkReachabilityHostObject>(target)
        .replace_callback(callout, context);
    release_context(env, old);
    CFRelease(env, target);
    true
}

fn SCNetworkReachabilityScheduleWithRunLoop(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
    _run_loop: CFTypeRef,
    _run_loop_mode: CFTypeRef,
) -> bool {
    if !is_reachability(env, target) {
        return false;
    }
    // Keep the existing immediate-notification stub for now. A real run-loop
    // source/network-change implementation is separate from this ABI fix.
    CFRetain(env, target);
    let (callback, info) = {
        let host = env
            .objc
            .borrow_mut::<SCNetworkReachabilityHostObject>(target);
        host.scheduled = true;
        if host.callout.is_some() {
            host.active_callbacks += 1;
        }
        (
            host.callout,
            host.context.map_or(MutVoidPtr::null(), |c| c.info),
        )
    };
    if let Some(callback) = callback {
        // Match GetFlags: do not invent a cellular/direct connection here.
        let flags = current_flags(env);
        let _: () = callback.call_from_host(env, (target, flags, info));
        let retired = env
            .objc
            .borrow_mut::<SCNetworkReachabilityHostObject>(target)
            .finish_callback();
        for context in retired {
            release_context(env, Some(context));
        }
    }
    CFRelease(env, target);
    true
}

fn SCNetworkReachabilityUnscheduleFromRunLoop(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
    _run_loop: CFTypeRef,
    _run_loop_mode: CFTypeRef,
) -> bool {
    if !is_reachability(env, target) {
        return false;
    }
    // Nothing real to tear down (notifications are delivered synchronously at
    // schedule time), so just report whether it had been scheduled.
    std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<SCNetworkReachabilityHostObject>(target)
            .scheduled,
        false,
    )
}

fn SCNetworkReachabilitySetDispatchQueue(
    _env: &mut Environment,
    _target: SCNetworkReachabilityRef,
    queue: MutVoidPtr,
) -> bool {
    // Delivering callbacks on a dispatch queue isn't implemented; callers
    // should fall back to the run-loop path.
    log_dbg!(
        "SCNetworkReachabilitySetDispatchQueue({:?}): not supported, returning false",
        queue
    );
    false
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(SCNetworkReachabilityRetain(_)),
    export_c_func!(SCNetworkReachabilityRelease(_)),
    export_c_func!(SCNetworkReachabilityGetTypeID()),
    export_c_func!(SCNetworkReachabilityCreateWithName(_, _)),
    export_c_func!(SCNetworkReachabilityCreateWithAddress(_, _)),
    export_c_func!(SCNetworkReachabilityCreateWithAddressPair(_, _, _)),
    export_c_func!(SCNetworkReachabilityGetFlags(_, _)),
    export_c_func!(SCNetworkReachabilitySetCallback(_, _, _)),
    export_c_func!(SCNetworkReachabilityScheduleWithRunLoop(_, _, _)),
    export_c_func!(SCNetworkReachabilityUnscheduleFromRunLoop(_, _, _)),
    export_c_func!(SCNetworkReachabilitySetDispatchQueue(_, _)),
];

#[cfg(test)]
mod tests;
