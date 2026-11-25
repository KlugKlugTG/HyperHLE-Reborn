/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Core Foundation binary heaps with guest comparison and ownership callbacks.
//! https://github.com/swiftlang/swift-corelibs-foundation/blob/main/Sources/CoreFoundation/CFBinaryHeap.c

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::objc::{id, objc_classes, ClassExports, HostObject};
use crate::Environment;

#[derive(Default)]
pub struct CFBinaryHeapState;

#[derive(Clone, Copy, Default)]
#[repr(C, packed)]
struct Callbacks {
    version: i32,
    retain: GuestFunction,
    release: GuestFunction,
    description: GuestFunction,
    compare: GuestFunction,
}
unsafe impl SafeRead for Callbacks {}

#[derive(Clone, Copy, Default)]
#[repr(C, packed)]
struct CompareContext {
    version: i32,
    info: MutVoidPtr,
    retain: GuestFunction,
    release: GuestFunction,
    description: GuestFunction,
}
unsafe impl SafeRead for CompareContext {}

#[derive(Default)]
struct Heap {
    allocator: MutVoidPtr,
    callbacks: Callbacks,
    context: CompareContext,
    values: Vec<ConstVoidPtr>,
}
impl HostObject for Heap {}

fn compare(env: &mut Environment, heap: id, a: ConstVoidPtr, b: ConstVoidPtr) -> i32 {
    let state = env.objc.borrow::<Heap>(heap);
    let callback = state.callbacks.compare;
    let context = state.context.info;
    if callback == GuestFunction::null_ptr() {
        match a.to_bits().cmp(&b.to_bits()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    } else {
        callback.call_from_host(env, (a, b, context))
    }
}

fn CFBinaryHeapCreate(
    env: &mut Environment,
    allocator: MutVoidPtr,
    capacity: i32,
    callbacks: ConstPtr<Callbacks>,
    context: ConstPtr<CompareContext>,
) -> id {
    if capacity < 0 {
        return Ptr::null();
    }
    let callbacks = if callbacks.is_null() {
        Callbacks::default()
    } else {
        env.mem.read(callbacks)
    };
    let mut context = if context.is_null() {
        CompareContext::default()
    } else {
        env.mem.read(context)
    };
    if callbacks.version != 0 || context.version != 0 {
        return Ptr::null();
    }
    let retain_context = context.retain;
    if retain_context != GuestFunction::null_ptr() {
        context.info = retain_context.call_from_host(env, (context.info,));
    }
    let class = env
        .objc
        .get_known_class("_touchHLE_CFBinaryHeap", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(Heap {
            allocator,
            callbacks,
            context,
            values: Vec::with_capacity(capacity as usize),
        }),
        &mut env.mem,
    )
}

fn CFBinaryHeapAddValue(env: &mut Environment, heap: id, mut value: ConstVoidPtr) {
    let state = env.objc.borrow::<Heap>(heap);
    let retain = state.callbacks.retain;
    let allocator = state.allocator;
    if retain != GuestFunction::null_ptr() {
        value = retain.call_from_host(env, (allocator, value));
    }
    let values = &mut env.objc.borrow_mut::<Heap>(heap).values;
    values.push(value);
    let mut index = values.len() - 1;
    while index > 0 {
        let parent = (index - 1) / 2;
        let parent_value = env.objc.borrow::<Heap>(heap).values[parent];
        if compare(env, heap, parent_value, value) <= 0 {
            break;
        }
        env.objc.borrow_mut::<Heap>(heap).values.swap(index, parent);
        index = parent;
    }
}

fn CFBinaryHeapGetCount(env: &mut Environment, heap: id) -> i32 {
    env.objc.borrow::<Heap>(heap).values.len() as i32
}

fn CFBinaryHeapGetMinimum(env: &mut Environment, heap: id) -> ConstVoidPtr {
    env.objc
        .borrow::<Heap>(heap)
        .values
        .first()
        .copied()
        .unwrap_or(Ptr::null())
}

fn remove_minimum(values: &mut Vec<ConstVoidPtr>) {
    if !values.is_empty() {
        values.swap_remove(0);
    }
}

fn CFBinaryHeapRemoveMinimumValue(env: &mut Environment, heap: id) {
    let state = env.objc.borrow_mut::<Heap>(heap);
    let Some(&removed) = state.values.first() else {
        return;
    };
    let release = state.callbacks.release;
    let allocator = state.allocator;
    remove_minimum(&mut state.values);
    let mut index = 0;
    loop {
        let values = &env.objc.borrow::<Heap>(heap).values;
        let left = index * 2 + 1;
        if left >= values.len() {
            break;
        }
        let right = left + 1;
        let left_value = values[left];
        let right_value = values.get(right).copied();
        let parent_value = values[index];
        let child = if let Some(right_value) = right_value {
            if compare(env, heap, left_value, right_value) > 0 {
                right
            } else {
                left
            }
        } else {
            left
        };
        let child_value = env.objc.borrow::<Heap>(heap).values[child];
        if compare(env, heap, parent_value, child_value) <= 0 {
            break;
        }
        env.objc.borrow_mut::<Heap>(heap).values.swap(index, child);
        index = child;
    }
    if release != GuestFunction::null_ptr() {
        let _: () = release.call_from_host(env, (allocator, removed));
    }
}

fn CFBinaryHeapRemoveMinimum(env: &mut Environment, heap: id) {
    CFBinaryHeapRemoveMinimumValue(env, heap);
}

fn CFBinaryHeapRemoveAllValues(env: &mut Environment, heap: id) {
    let state = env.objc.borrow_mut::<Heap>(heap);
    let values = std::mem::take(&mut state.values);
    let release = state.callbacks.release;
    let allocator = state.allocator;
    if release != GuestFunction::null_ptr() {
        for value in values {
            let _: () = release.call_from_host(env, (allocator, value));
        }
    }
}

fn CFBinaryHeapGetCountOfValue(env: &mut Environment, heap: id, value: ConstVoidPtr) -> i32 {
    let values = env.objc.borrow::<Heap>(heap).values.clone();
    values
        .into_iter()
        .filter(|&candidate| candidate == value || compare(env, heap, candidate, value) == 0)
        .count() as i32
}

fn CFBinaryHeapContainsValue(env: &mut Environment, heap: id, value: ConstVoidPtr) -> bool {
    CFBinaryHeapGetCountOfValue(env, heap, value) != 0
}

fn CFBinaryHeapGetValues(env: &mut Environment, heap: id, out: MutPtr<ConstVoidPtr>) {
    let mut values = env.objc.borrow::<Heap>(heap).values.clone();
    values.sort_by(|&a, &b| compare(env, heap, a, b).cmp(&0));
    for (index, value) in values.into_iter().enumerate() {
        env.mem.write(out + index as u32, value);
    }
}

fn CFBinaryHeapGetMinimumIfPresent(
    env: &mut Environment,
    heap: id,
    out: MutPtr<ConstVoidPtr>,
) -> bool {
    if CFBinaryHeapGetCount(env, heap) == 0 {
        return false;
    }
    let value = CFBinaryHeapGetMinimum(env, heap);
    env.mem.write(out, value);
    true
}

fn CFBinaryHeapCreateCopy(
    env: &mut Environment,
    allocator: MutVoidPtr,
    capacity: i32,
    heap: id,
) -> id {
    let state = env.objc.borrow::<Heap>(heap);
    let callbacks = state.callbacks;
    let context = state.context;
    let values = state.values.clone();
    let callbacks_ptr = env.mem.alloc_and_write(callbacks);
    let context_ptr = env.mem.alloc_and_write(context);
    let copy = CFBinaryHeapCreate(
        env,
        allocator,
        capacity,
        callbacks_ptr.cast_const(),
        context_ptr.cast_const(),
    );
    env.mem.free(callbacks_ptr.cast());
    env.mem.free(context_ptr.cast());
    if !copy.is_null() {
        for value in values {
            CFBinaryHeapAddValue(env, copy, value);
        }
    }
    copy
}

fn CFBinaryHeapApplyFunction(
    env: &mut Environment,
    heap: id,
    callback: GuestFunction,
    context: MutVoidPtr,
) {
    let values = env.objc.borrow::<Heap>(heap).values.clone();
    for value in values {
        let _: () = callback.call_from_host(env, (value, context));
    }
}

fn CFBinaryHeapGetTypeID(env: &mut Environment) -> i32 {
    env.objc
        .get_known_class("_touchHLE_CFBinaryHeap", &mut env.mem)
        .to_bits() as i32
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation _touchHLE_CFBinaryHeap: NSObject
- (())dealloc {
    CFBinaryHeapRemoveAllValues(env, this);
    let context = env.objc.borrow::<Heap>(this).context;
    let release_context = context.release;
    if release_context != GuestFunction::null_ptr() {
        let _: () = release_context.call_from_host(env, (context.info,));
    }
    env.objc.dealloc_object(this, &mut env.mem);
}
@end
};

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFBinaryHeapCreate(_, _, _, _)),
    export_c_func!(CFBinaryHeapCreateCopy(_, _, _)),
    export_c_func!(CFBinaryHeapAddValue(_, _)),
    export_c_func!(CFBinaryHeapGetCount(_)),
    export_c_func!(CFBinaryHeapGetCountOfValue(_, _)),
    export_c_func!(CFBinaryHeapContainsValue(_, _)),
    export_c_func!(CFBinaryHeapGetValues(_, _)),
    export_c_func!(CFBinaryHeapGetMinimum(_)),
    export_c_func!(CFBinaryHeapGetMinimumIfPresent(_, _)),
    export_c_func!(CFBinaryHeapRemoveMinimum(_)),
    export_c_func!(CFBinaryHeapRemoveMinimumValue(_)),
    export_c_func!(CFBinaryHeapRemoveAllValues(_)),
    export_c_func!(CFBinaryHeapApplyFunction(_, _, _)),
    export_c_func!(CFBinaryHeapGetTypeID()),
];

#[cfg(test)]
mod tests {
    use super::remove_minimum;
    use crate::mem::ConstVoidPtr;

    #[test]
    fn remove_minimum_value_removes_first_value() {
        let first = ConstVoidPtr::from_bits(0x1000);
        let second = ConstVoidPtr::from_bits(0x2000);
        let mut heap = vec![first, second];
        remove_minimum(&mut heap);
        assert_eq!(heap, vec![second]);
        remove_minimum(&mut heap);
        assert!(heap.is_empty());
        remove_minimum(&mut heap);
        assert!(heap.is_empty());
    }
}
