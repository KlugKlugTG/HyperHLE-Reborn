/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGFunction.h`

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::core_graphics::CGFloat;
use crate::mem::{ConstPtr, GuestUSize, MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{objc_classes, nil, ClassExports, HostObject};
use crate::Environment;

pub type CGFunctionRef = CFTypeRef;

/// `CGFunctionCallbacks` — { u32 version; evaluate; releaseInfo }.
/// Function pointers are kept as raw guest addresses (0 = none).
#[derive(Copy, Clone)]
#[repr(C, packed)]
struct CGFunctionCallbacks {
    version: u32,
    evaluate: u32,
    release_info: u32,
}
unsafe impl SafeRead for CGFunctionCallbacks {}

pub struct CGFunctionHostObject {
    info: MutVoidPtr,
    domain_dimension: GuestUSize,
    range_dimension: GuestUSize,
    /// 2 floats per input dimension (min, max); empty if no domain given.
    domain: Vec<CGFloat>,
    /// 2 floats per output dimension; empty if no range given.
    range: Vec<CGFloat>,
    evaluate: u32,
    release_info: u32,
}
impl HostObject for CGFunctionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGFunction is a CFType in Apple's implementation; here it needs a class
// whose name isn't visible to apps.
@implementation _touchHLE_CGFunction: NSObject

- (())dealloc {
    let (info, release_info) = {
        let h = env.objc.borrow::<CGFunctionHostObject>(this);
        (h.info, h.release_info)
    };
    if release_info != 0 && !info.is_null() {
        let f = GuestFunction::from_addr_with_thumb_bit(release_info);
        let _: () = f.call_from_host(env, (info,));
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

fn read_floats(env: &mut Environment, ptr: ConstPtr<CGFloat>, count: usize) -> Vec<CGFloat> {
    if ptr.is_null() {
        return Vec::new();
    }
    (0..count as GuestUSize).map(|i| env.mem.read(ptr + i)).collect()
}

/// `CGFunctionRef CGFunctionCreate(void *info, size_t domainDimension,
///   const CGFloat *domain, size_t rangeDimension, const CGFloat *range,
///   const CGFunctionCallbacks *callbacks)`
fn CGFunctionCreate(
    env: &mut Environment,
    info: MutVoidPtr,
    domain_dimension: GuestUSize,
    domain: ConstPtr<CGFloat>,
    range_dimension: GuestUSize,
    range: ConstPtr<CGFloat>,
    callbacks: ConstPtr<CGFunctionCallbacks>,
) -> CGFunctionRef {
    if callbacks.is_null() {
        return nil;
    }
    let cb = env.mem.read(callbacks);
    // Copy packed fields to locals.
    let evaluate = cb.evaluate;
    let release_info = cb.release_info;
    if evaluate == 0 {
        return nil;
    }

    let domain = read_floats(env, domain, domain_dimension as usize * 2);
    let range = read_floats(env, range, range_dimension as usize * 2);

    let host_object = Box::new(CGFunctionHostObject {
        info,
        domain_dimension,
        range_dimension,
        domain,
        range,
        evaluate,
        release_info,
    });
    let class = env.objc.get_known_class("_touchHLE_CGFunction", &mut env.mem);
    env.objc.alloc_object(class, host_object, &mut env.mem)
}

pub fn CGFunctionRetain(env: &mut Environment, f: CGFunctionRef) -> CGFunctionRef {
    if !f.is_null() {
        CFRetain(env, f)
    } else {
        f
    }
}

pub fn CGFunctionRelease(env: &mut Environment, f: CGFunctionRef) {
    if !f.is_null() {
        CFRelease(env, f);
    }
}

/// For use by shading/gradient code: evaluate the function on the host side.
/// Inputs are clamped to the domain and outputs to the range when specified.
pub fn evaluate(env: &mut Environment, f: CGFunctionRef, input: &[CGFloat]) -> Vec<CGFloat> {
    let (info, evaluate_addr, domain_dim, range_dim, domain, range) = {
        let h = env.objc.borrow::<CGFunctionHostObject>(f);
        (
            h.info,
            h.evaluate,
            h.domain_dimension as usize,
            h.range_dimension as usize,
            h.domain.clone(),
            h.range.clone(),
        )
    };

    let mut clamped: Vec<CGFloat> = input.iter().copied().take(domain_dim).collect();
    clamped.resize(domain_dim, 0.0);
    for (i, v) in clamped.iter_mut().enumerate() {
        if let (Some(&lo), Some(&hi)) = (domain.get(i * 2), domain.get(i * 2 + 1)) {
            *v = v.clamp(lo, hi);
        }
    }

    let in_ptr: MutPtr<CGFloat> = env.mem.alloc((domain_dim.max(1) * 4) as GuestUSize).cast();
    let out_ptr: MutPtr<CGFloat> = env.mem.alloc((range_dim.max(1) * 4) as GuestUSize).cast();
    for (i, v) in clamped.iter().enumerate() {
        env.mem.write(in_ptr + i as GuestUSize, *v);
    }

    let func = GuestFunction::from_addr_with_thumb_bit(evaluate_addr);
    let _: () = func.call_from_host(env, (info, in_ptr.cast_const(), out_ptr));

    let mut out: Vec<CGFloat> = (0..range_dim as GuestUSize)
        .map(|i| env.mem.read(out_ptr + i))
        .collect();
    for (i, v) in out.iter_mut().enumerate() {
        if let (Some(&lo), Some(&hi)) = (range.get(i * 2), range.get(i * 2 + 1)) {
            *v = v.clamp(lo, hi);
        }
    }

    env.mem.free(in_ptr.cast());
    env.mem.free(out_ptr.cast());
    out
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGFunctionCreate(_, _, _, _, _, _)),
    export_c_func!(CGFunctionRetain(_)),
    export_c_func!(CGFunctionRelease(_)),
];
