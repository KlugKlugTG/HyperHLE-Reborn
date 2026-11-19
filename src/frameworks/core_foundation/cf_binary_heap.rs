/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFBinaryHeap` — Core Foundation priority queue.
//!
//! A real binary min-heap. Ordering comes from the guest's
//! `CFBinaryHeapCallBacks.compare` (called via `GuestFunction`), with
//! retain/release callbacks honoured on add/remove/dealloc. A heap is an
//! Objective-C object with a host object, so `CFRetain`/`CFRelease` work.
//!
//! Guest `CFBinaryHeapCallBacks` layout (32-bit ARM):
//! `{ CFIndex version; retain; release; copyDescription; compare }`,
//! i.e. retain at +4, release at +8, compare at +16.
//!
//! Heap operations copy the value list out, run guest callbacks, then write
//! it back, so a callback that re-enters the *same* heap sees the old
//! contents. Real apps don't do that.

use super::{CFIndex, CFTypeRef};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{nil, objc_classes, ClassExports, HostObject};
use crate::Environment;

pub type CFBinaryHeapRef = CFTypeRef;

/// No longer used for heap storage (state lives in each heap's host object).
/// Kept only because `core_foundation::State` still has a field of this type;
/// delete both together if you like.
#[derive(Default)]
pub struct CFBinaryHeapState;

// MARK: - Host object

#[derive(Copy, Clone)]
struct Callbacks {
    retain: Option<GuestFunction>,
    release: Option<GuestFunction>,
    compare: Option<GuestFunction>,
}

struct CFBinaryHeapHostObject {
    allocator: MutVoidPtr,
    callbacks: Callbacks,
    context: MutVoidPtr,
    /// Binary min-heap order: `values[0]` is the minimum.
    values: Vec<ConstVoidPtr>,
}
impl HostObject for CFBinaryHeapHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CFBinaryHeap: NSObject

- (())dealloc {
    let (allocator, callbacks, values) = {
        let h = env.objc.borrow::<CFBinaryHeapHostObject>(this);
        (h.allocator, h.callbacks, h.values.clone())
    };
    for v in values {
        release_value(env, &callbacks, allocator, v);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

// MARK: - Pure heap algorithm (comparator returns <0, 0, >0 like CF)

fn sift_up<T: Copy>(v: &mut [T], mut i: usize, cmp: &mut impl FnMut(T, T) -> i32) {
    while i > 0 {
        let parent = (i - 1) / 2;
        if cmp(v[i], v[parent]) < 0 {
            v.swap(i, parent);
            i = parent;
        } else {
            break;
        }
    }
}

fn sift_down<T: Copy>(v: &mut [T], mut i: usize, cmp: &mut impl FnMut(T, T) -> i32) {
    let n = v.len();
    loop {
        let l = 2 * i + 1;
        let r = l + 1;
        let mut smallest = i;
        if l < n && cmp(v[l], v[smallest]) < 0 {
            smallest = l;
        }
        if r < n && cmp(v[r], v[smallest]) < 0 {
            smallest = r;
        }
        if smallest == i {
            break;
        }
        v.swap(i, smallest);
        i = smallest;
    }
}

fn heap_push<T: Copy>(v: &mut Vec<T>, x: T, cmp: &mut impl FnMut(T, T) -> i32) {
    v.push(x);
    let i = v.len() - 1;
    sift_up(v, i, cmp);
}

fn heap_pop_min<T: Copy>(v: &mut Vec<T>, cmp: &mut impl FnMut(T, T) -> i32) -> Option<T> {
    if v.is_empty() {
        return None;
    }
    let last = v.len() - 1;
    v.swap(0, last);
    let min = v.pop();
    if !v.is_empty() {
        sift_down(v, 0, cmp);
    }
    min
}

// MARK: - Guest callback helpers

fn read_guest_fn(env: &mut Environment, addr: u32) -> Option<GuestFunction> {
    let raw: u32 = env.mem.read(ConstPtr::<u32>::from_bits(addr));
    if raw == 0 {
        None
    } else {
        // The pointer value carries the Thumb bit in bit 0.
        Some(GuestFunction::from_addr_with_thumb_bit(raw))
    }
}

fn read_callbacks(env: &mut Environment, ptr: ConstVoidPtr) -> Callbacks {
    if ptr.is_null() {
        return Callbacks {
            retain: None,
            release: None,
            compare: None,
        };
    }
    let base = ptr.to_bits();
    Callbacks {
        retain: read_guest_fn(env, base + 4),
        release: read_guest_fn(env, base + 8),
        compare: read_guest_fn(env, base + 16),
    }
}

/// `compare(a, b, context)`. With no compare callback, falls back to
/// comparing the pointer values.
fn compare(
    env: &mut Environment,
    cbs: &Callbacks,
    context: MutVoidPtr,
    a: ConstVoidPtr,
    b: ConstVoidPtr,
) -> i32 {
    match cbs.compare {
        Some(f) => f.call_from_host(env, (a, b, context)),
        None => a.to_bits().cmp(&b.to_bits()) as i32,
    }
}

fn retain_value(
    env: &mut Environment,
    cbs: &Callbacks,
    allocator: MutVoidPtr,
    value: ConstVoidPtr,
) -> ConstVoidPtr {
    match cbs.retain {
        Some(f) => f.call_from_host(env, (allocator, value)),
        None => value,
    }
}

fn release_value(
    env: &mut Environment,
    cbs: &Callbacks,
    allocator: MutVoidPtr,
    value: ConstVoidPtr,
) {
    if let Some(f) = cbs.release {
        let _: () = f.call_from_host(env, (allocator, value));
    }
}

/// Values in ascending order, consuming `values`.
fn sorted_values(
    env: &mut Environment,
    cbs: Callbacks,
    context: MutVoidPtr,
    mut values: Vec<ConstVoidPtr>,
) -> Vec<ConstVoidPtr> {
    let mut out = Vec::with_capacity(values.len());
    loop {
        // `let` (not `while let`) so the closure's borrow of `env` ends here.
        let next = heap_pop_min(&mut values, &mut |a, b| compare(env, &cbs, context, a, b));
        match next {
            Some(v) => out.push(v),
            None => break,
        }
    }
    out
}

// MARK: - State access

struct State {
    allocator: MutVoidPtr,
    cbs: Callbacks,
    context: MutVoidPtr,
    values: Vec<ConstVoidPtr>,
}

fn read_state(env: &mut Environment, heap: CFBinaryHeapRef) -> Option<State> {
    if heap.is_null() {
        return None;
    }
    let h = env.objc.borrow::<CFBinaryHeapHostObject>(heap);
    Some(State {
        allocator: h.allocator,
        cbs: h.callbacks,
        context: h.context,
        values: h.values.clone(),
    })
}

fn write_values(env: &mut Environment, heap: CFBinaryHeapRef, values: Vec<ConstVoidPtr>) {
    env.objc
        .borrow_mut::<CFBinaryHeapHostObject>(heap)
        .values = values;
}

fn new_heap(
    env: &mut Environment,
    allocator: MutVoidPtr,
    callbacks: Callbacks,
    context: MutVoidPtr,
    values: Vec<ConstVoidPtr>,
) -> CFBinaryHeapRef {
    let host = Box::new(CFBinaryHeapHostObject {
        allocator,
        callbacks,
        context,
        values,
    });
    let class = env
        .objc
        .get_known_class("_touchHLE_CFBinaryHeap", &mut env.mem);
    // Retain count 1, as a Create function owes.
    env.objc.alloc_object(class, host, &mut env.mem)
}

// MARK: - C API: creation

fn CFBinaryHeapCreate(
    env: &mut Environment,
    allocator: MutVoidPtr,
    _capacity: CFIndex,
    call_backs: ConstVoidPtr,
    compare_context: MutVoidPtr,
) -> CFBinaryHeapRef {
    let cbs = read_callbacks(env, call_backs);
    new_heap(env, allocator, cbs, compare_context, Vec::new())
}

fn CFBinaryHeapCreateCopy(
    env: &mut Environment,
    allocator: MutVoidPtr,
    _capacity: CFIndex,
    heap: CFBinaryHeapRef,
) -> CFBinaryHeapRef {
    let Some(st) = read_state(env, heap) else {
        return nil;
    };
    // The values are already in heap order, so a straight copy is valid.
    // The copy owns its own references, so retain each one.
    let mut copied = Vec::with_capacity(st.values.len());
    for v in st.values {
        copied.push(retain_value(env, &st.cbs, st.allocator, v));
    }
    new_heap(env, allocator, st.cbs, st.context, copied)
}

// MARK: - C API: queries

fn CFBinaryHeapGetCount(env: &mut Environment, heap: CFBinaryHeapRef) -> CFIndex {
    if heap.is_null() {
        return 0;
    }
    env.objc
        .borrow::<CFBinaryHeapHostObject>(heap)
        .values
        .len() as CFIndex
}

fn CFBinaryHeapGetCountOfValue(
    env: &mut Environment,
    heap: CFBinaryHeapRef,
    value: ConstVoidPtr,
) -> CFIndex {
    let Some(st) = read_state(env, heap) else {
        return 0;
    };
    let mut n: CFIndex = 0;
    for v in st.values {
        if compare(env, &st.cbs, st.context, value, v) == 0 {
            n += 1;
        }
    }
    n
}

fn CFBinaryHeapContainsValue(
    env: &mut Environment,
    heap: CFBinaryHeapRef,
    value: ConstVoidPtr,
) -> bool {
    let Some(st) = read_state(env, heap) else {
        return false;
    };
    for v in st.values {
        if compare(env, &st.cbs, st.context, value, v) == 0 {
            return true;
        }
    }
    false
}

fn CFBinaryHeapGetMinimum(env: &mut Environment, heap: CFBinaryHeapRef) -> ConstVoidPtr {
    if heap.is_null() {
        return Ptr::null();
    }
    env.objc
        .borrow::<CFBinaryHeapHostObject>(heap)
        .values
        .first()
        .copied()
        .unwrap_or(Ptr::null())
}

fn CFBinaryHeapGetMinimumIfPresent(
    env: &mut Environment,
    heap: CFBinaryHeapRef,
    value: MutPtr<ConstVoidPtr>,
) -> bool {
    let min = if heap.is_null() {
        None
    } else {
        env.objc
            .borrow::<CFBinaryHeapHostObject>(heap)
            .values
            .first()
            .copied()
    };
    if !value.is_null() {
        env.mem.write(value, min.unwrap_or(Ptr::null()));
    }
    min.is_some()
}

/// Copies all values, in ascending order, into `values`, which must have room
/// for `CFBinaryHeapGetCount()` entries.
fn CFBinaryHeapGetValues(
    env: &mut Environment,
    heap: CFBinaryHeapRef,
    values: MutPtr<ConstVoidPtr>,
) {
    if values.is_null() {
        return;
    }
    let Some(st) = read_state(env, heap) else {
        return;
    };
    let sorted = sorted_values(env, st.cbs, st.context, st.values);
    for (i, v) in sorted.into_iter().enumerate() {
        env.mem.write(values + i as u32, v);
    }
}

/// Calls `applier(value, context)` for every value, in ascending order. (Real
/// CF doesn't promise an order.)
fn CFBinaryHeapApplyFunction(
    env: &mut Environment,
    heap: CFBinaryHeapRef,
    applier: GuestFunction,
    context: MutVoidPtr,
) {
    let Some(st) = read_state(env, heap) else {
        return;
    };
    let sorted = sorted_values(env, st.cbs, st.context, st.values);
    for v in sorted {
        let _: () = applier.call_from_host(env, (v, context));
    }
}

// MARK: - C API: mutation

fn CFBinaryHeapAddValue(env: &mut Environment, heap: CFBinaryHeapRef, value: ConstVoidPtr) {
    let Some(State {
        allocator,
        cbs,
        context,
        mut values,
    }) = read_state(env, heap)
    else {
        log!("Warning: CFBinaryHeapAddValue called with NULL heap");
        return;
    };
    let value = retain_value(env, &cbs, allocator, value);
    heap_push(&mut values, value, &mut |a, b| {
        compare(env, &cbs, context, a, b)
    });
    write_values(env, heap, values);
}

fn CFBinaryHeapRemoveMinimumValue(env: &mut Environment, heap: CFBinaryHeapRef) {
    let Some(State {
        allocator,
        cbs,
        context,
        mut values,
    }) = read_state(env, heap)
    else {
        return;
    };
    let popped = heap_pop_min(&mut values, &mut |a, b| {
        compare(env, &cbs, context, a, b)
    });
    write_values(env, heap, values);
    if let Some(v) = popped {
        release_value(env, &cbs, allocator, v);
    }
}

fn CFBinaryHeapRemoveAllValues(env: &mut Environment, heap: CFBinaryHeapRef) {
    let Some(st) = read_state(env, heap) else {
        return;
    };
    write_values(env, heap, Vec::new());
    for v in st.values {
        release_value(env, &st.cbs, st.allocator, v);
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFBinaryHeapCreate(_, _, _, _)),
    export_c_func!(CFBinaryHeapCreateCopy(_, _, _)),
    export_c_func!(CFBinaryHeapGetCount(_)),
    export_c_func!(CFBinaryHeapGetCountOfValue(_, _)),
    export_c_func!(CFBinaryHeapContainsValue(_, _)),
    export_c_func!(CFBinaryHeapGetMinimum(_)),
    export_c_func!(CFBinaryHeapGetMinimumIfPresent(_, _)),
    export_c_func!(CFBinaryHeapGetValues(_, _)),
    export_c_func!(CFBinaryHeapApplyFunction(_, _, _)),
    export_c_func!(CFBinaryHeapAddValue(_, _)),
    export_c_func!(CFBinaryHeapRemoveMinimumValue(_)),
    export_c_func!(CFBinaryHeapRemoveAllValues(_)),
];

#[cfg(test)]
mod tests {
    use super::{heap_pop_min, heap_push};

    fn drain(mut v: Vec<u32>) -> Vec<u32> {
        let mut out = Vec::new();
        while let Some(x) = heap_pop_min(&mut v, &mut |a: u32, b: u32| a.cmp(&b) as i32) {
            out.push(x);
        }
        out
    }

    #[test]
    fn pops_in_ascending_order() {
        let mut v = Vec::new();
        for x in [5u32, 1, 4, 2, 3, 9, 0, 7] {
            heap_push(&mut v, x, &mut |a: u32, b: u32| a.cmp(&b) as i32);
        }
        assert_eq!(v[0], 0);
        assert_eq!(drain(v), vec![0, 1, 2, 3, 4, 5, 7, 9]);
    }

    #[test]
    fn handles_duplicates_and_empty() {
        let mut v = Vec::new();
        for x in [2u32, 2, 1, 2] {
            heap_push(&mut v, x, &mut |a: u32, b: u32| a.cmp(&b) as i32);
        }
        assert_eq!(drain(v), vec![1, 2, 2, 2]);

        let mut empty: Vec<u32> = Vec::new();
        assert_eq!(
            heap_pop_min(&mut empty, &mut |a: u32, b: u32| a.cmp(&b) as i32),
            None
        );
    }
}
