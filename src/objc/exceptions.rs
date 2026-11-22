/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Objective-C exceptions use the guest C++ SjLj runtime, including its
//! landing pads, cleanup handlers and thread-local exception chain.
//! https://github.com/apple-oss-distributions/objc4/blob/main/runtime/objc-exception.mm

use super::{id, nil, release, retain, Class, ObjC};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{Dyld, HostFunction};
use crate::mem::{ConstPtr, ConstVoidPtr, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct State {
    vtable: Option<ConstPtr<u32>>,
    types: HashMap<String, ConstVoidPtr>,
    destructor: Option<GuestFunction>,
}

fn false_result(_env: &mut Environment) -> bool {
    false
}

fn vtable(dyld: &mut Dyld, objc: &mut ObjC, mem: &mut Mem) -> ConstPtr<u32> {
    if let Some(table) = objc.exceptions.vtable {
        return table;
    }
    let no_op = dyld.create_guest_function(
        mem,
        "_objc_exception_noop",
        &(false_result as fn(&mut Environment) -> bool),
    );
    let matcher: HostFunction = &(do_catch as fn(&mut Environment, _, _, _, _) -> _);
    let matcher = dyld.create_guest_function(mem, "_objc_exception_do_catch", matcher);
    // Itanium type_info address point follows offset-to-top and RTTI.
    // GNU libstdc++ slots: destructors, is_pointer, is_function, do_catch,
    // do_upcast. Apple's ObjC type_info uses the same layout.
    let table: MutPtr<u32> = mem.calloc(40).cast();
    for slot in 2..10 {
        mem.write(
            table + slot,
            if slot == 6 {
                matcher.addr_with_thumb_bit()
            } else {
                no_op.addr_with_thumb_bit()
            },
        );
    }
    objc.exceptions.vtable = Some(table.cast_const());
    table.cast_const()
}

pub(crate) fn link_symbol(
    dyld: &mut Dyld,
    objc: &mut ObjC,
    mem: &mut Mem,
    symbol: &str,
) -> Option<ConstVoidPtr> {
    if symbol == "_objc_ehtype_vtable" {
        return Some(vtable(dyld, objc, mem).cast());
    }
    let name = if symbol == "_OBJC_EHTYPE_id" {
        "id"
    } else {
        symbol.strip_prefix("_OBJC_EHTYPE_$_")?
    };
    if let Some(&ty) = objc.exceptions.types.get(name) {
        return Some(ty);
    }
    let table = vtable(dyld, objc, mem);
    let name_ptr = mem.alloc_and_write_cstr(name.as_bytes());
    let class = objc.classes.get(name).copied().unwrap_or(nil);
    let ty: MutPtr<u32> = mem.alloc(12).cast();
    mem.write(ty, (table + 2).to_bits());
    mem.write(ty + 1, name_ptr.to_bits());
    mem.write(ty + 2, class.to_bits());
    if name == "id" {
        mem.write(table.cast_mut() + 1, ty.to_bits());
    }
    objc.exceptions
        .types
        .insert(name.into(), ty.cast().cast_const());
    Some(ty.cast().cast_const())
}

fn do_catch(
    env: &mut Environment,
    catch_type: ConstPtr<u32>,
    throw_type: ConstPtr<u32>,
    thrown: MutPtr<MutVoidPtr>,
    _outer: u32,
) -> bool {
    let Some(table) = env.objc.exceptions.vtable else {
        return false;
    };
    if env.mem.read(throw_type) != (table + 2).to_bits() {
        return false;
    }
    let payload = env.mem.read(thrown);
    let exception: id = env.mem.read(payload.cast());
    // The adjusted pointer returned by __cxa_begin_catch is the ObjC id.
    env.mem.write(thrown, exception.cast());
    let name: ConstPtr<u8> = Ptr::from_bits(env.mem.read(catch_type + 1));
    let name = env.mem.cstr_at_utf8(name).unwrap_or("");
    if name == "id" {
        return true;
    }
    let class: Class = Ptr::from_bits(env.mem.read(catch_type + 2));
    // A host descriptor can have been linked before its guest class loaded.
    let class = if class == nil {
        env.objc.classes.get(name).copied().unwrap_or(nil)
    } else {
        class
    };
    class != nil
        && exception != nil
        && env
            .objc
            .class_is_subclass_of(ObjC::read_isa(exception, &env.mem), class)
}

fn guest_runtime(env: &Environment, symbol: &str) -> GuestFunction {
    GuestFunction::from_addr_with_thumb_bit(
        env.dyld.guest_cxxabi_export(symbol).unwrap_or_else(|| {
            panic!("Objective-C exception requires guest runtime symbol {symbol}")
        }),
    )
}

fn destroy_exception(env: &mut Environment, payload: ConstPtr<id>) {
    let exception = env.mem.read(payload);
    release(env, exception);
}

pub fn objc_exception_throw(env: &mut Environment, exception: id) {
    let allocator = guest_runtime(env, "___cxa_allocate_exception");
    let payload: MutPtr<u32> = allocator.call_from_host(env, (16u32,));
    assert!(
        !payload.is_null(),
        "Objective-C exception allocation failed"
    );
    retain(env, exception);
    let table = vtable(&mut env.dyld, &mut env.objc, &mut env.mem);
    let class = ObjC::read_isa(exception, &env.mem);
    let name = env.objc.get_class_name(class).to_owned();
    let ty = link_symbol(
        &mut env.dyld,
        &mut env.objc,
        &mut env.mem,
        &format!("_OBJC_EHTYPE_$_{name}"),
    )
    .unwrap();
    let name_ptr: ConstPtr<u8> = Ptr::from_bits(env.mem.read(ty.cast::<u32>() + 1));
    // objc_exception: id object; objc_typeinfo { vtable, name, Class }.
    env.mem.write(payload, exception.to_bits());
    env.mem.write(payload + 1, (table + 2).to_bits());
    env.mem.write(payload + 2, name_ptr.to_bits());
    env.mem.write(payload + 3, class.to_bits());
    let destructor = if let Some(destructor) = env.objc.exceptions.destructor {
        destructor
    } else {
        let destructor = env.dyld.create_guest_function(
            &mut env.mem,
            "_objc_exception_destructor",
            &(destroy_exception as fn(&mut Environment, _)),
        );
        env.objc.exceptions.destructor = Some(destructor);
        destructor
    };
    let target = guest_runtime(env, "___cxa_throw");
    env.cpu.regs_mut()[0] = payload.to_bits();
    env.cpu.regs_mut()[1] = (payload + 1).to_bits();
    env.cpu.regs_mut()[2] = destructor.addr_with_thumb_bit();
    // Tail-call: a synthesized host-to-guest frame must not survive unwind.
    env.cpu.branch(target);
    env.note_guest_control_flow_redirect();
}

pub fn objc_begin_catch(env: &mut Environment, exception: MutVoidPtr) -> id {
    guest_runtime(env, "___cxa_begin_catch").call_from_host(env, (exception,))
}

pub fn objc_end_catch(env: &mut Environment) {
    let _: () = guest_runtime(env, "___cxa_end_catch").call_from_host(env, ());
}

pub fn objc_exception_rethrow(env: &mut Environment) {
    let target = guest_runtime(env, "___cxa_rethrow");
    env.cpu.branch(target);
    env.note_guest_control_flow_redirect();
}

pub fn __objc_personality_v0(
    env: &mut Environment,
    version: i32,
    actions: i32,
    exception_class: u64,
    exception: MutVoidPtr,
    context: MutVoidPtr,
) -> i32 {
    // Apple forwards to the C++ personality. The guest runtime performs
    // SjLj search and cleanup, including the original compiler's LSDA.
    guest_runtime(env, "___gxx_personality_sj0")
        .call_from_host(env, (version, actions, exception_class, exception, context))
}
