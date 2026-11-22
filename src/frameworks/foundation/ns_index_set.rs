/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSIndexSet`.
//!
//! An immutable collection of unique, sorted, non-negative integer indexes.
//! We back it with a sorted, de-duplicated `Vec<NSUInteger>`, which keeps the
//! semantics exact for the accessor methods apps commonly use.
//!
//! Apple reference:
//! <https://developer.apple.com/documentation/foundation/nsindexset>

use super::ns_string::from_rust_string;
use super::{NSInteger, NSRange, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, retain, ClassExports, HostObject, NSZonePtr,
};

/// `NSNotFound` is `NSIntegerMax` (per Apple's `NSObjCRuntime.h`).
const NS_NOT_FOUND: NSUInteger = NSInteger::MAX as NSUInteger;

#[derive(Default)]
struct NSIndexSetHostObject {
    /// Sorted, de-duplicated list of indexes contained in the set.
    indexes: Vec<NSUInteger>,
}
impl HostObject for NSIndexSetHostObject {}

fn set_range(host: &mut NSIndexSetHostObject, location: NSUInteger, length: NSUInteger) {
    host.indexes.clear();
    if length == 0 {
        return;
    }
    // Guard against overflow when location + length would exceed the range.
    let end = (location as u64) + (length as u64);
    let mut i = location as u64;
    while i < end {
        host.indexes.push(i as NSUInteger);
        i += 1;
    }
}

fn add_range(host: &mut NSIndexSetHostObject, location: NSUInteger, length: NSUInteger) {
    if length == 0 {
        return;
    }
    let end = (location as u64).saturating_add(length as u64);
    let mut i = location as u64;
    while i < end {
        let index = i as NSUInteger;
        match host.indexes.binary_search(&index) {
            Ok(_) => {}
            Err(pos) => host.indexes.insert(pos, index),
        }
        i = match i.checked_add(1) {
            Some(next) => next,
            None => break,
        };
    }
}

fn remove_range(host: &mut NSIndexSetHostObject, location: NSUInteger, length: NSUInteger) {
    if length == 0 {
        return;
    }
    let end = (location as u64).saturating_add(length as u64);
    host.indexes
        .retain(|&index| (index as u64) < (location as u64) || (index as u64) >= end);
}

fn indexes_of(env: &mut crate::Environment, other: id) -> Vec<NSUInteger> {
    if other == nil {
        Vec::new()
    } else {
        env.objc
            .borrow::<NSIndexSetHostObject>(other)
            .indexes
            .clone()
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSIndexSet: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<NSIndexSetHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: - Constructors

+ (id)indexSet {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

+ (id)indexSetWithIndex:(NSUInteger)index {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithIndex:index];
    autorelease(env, new)
}

+ (id)indexSetWithIndexesInRange:(NSRange)range {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    {
        let host = env.objc.borrow_mut::<NSIndexSetHostObject>(new);
        set_range(host, range.location, range.length);
    }
    autorelease(env, new)
}

// MARK: - Init

- (id)init {
    env.objc.borrow_mut::<NSIndexSetHostObject>(this).indexes = Vec::new();
    this
}

- (id)initWithIndex:(NSUInteger)index {
    env.objc.borrow_mut::<NSIndexSetHostObject>(this).indexes = vec![index];
    this
}

- (id)initWithIndexesInRange:(NSRange)range {
    let host = env.objc.borrow_mut::<NSIndexSetHostObject>(this);
    set_range(host, range.location, range.length);
    this
}

- (id)initWithIndexSet:(id)other { // NSIndexSet*
    let indexes = if other == nil {
        Vec::new()
    } else {
        env.objc.borrow::<NSIndexSetHostObject>(other).indexes.clone()
    };
    env.objc.borrow_mut::<NSIndexSetHostObject>(this).indexes = indexes;
    this
}

// MARK: - Dealloc

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Count / membership

- (NSUInteger)count {
    env.objc.borrow::<NSIndexSetHostObject>(this).indexes.len() as NSUInteger
}

- (bool)containsIndex:(NSUInteger)index {
    env.objc.borrow::<NSIndexSetHostObject>(this).indexes.contains(&index)
}

- (NSUInteger)countOfIndexesInRange:(NSRange)range {
    let end = (range.location as u64) + (range.length as u64);
    env.objc
        .borrow::<NSIndexSetHostObject>(this)
        .indexes
        .iter()
        .filter(|&&i| (i as u64) >= (range.location as u64) && (i as u64) < end)
        .count() as NSUInteger
}

// MARK: - Boundaries

- (NSUInteger)firstIndex {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    *host.indexes.first().unwrap_or(&NS_NOT_FOUND)
}

- (NSUInteger)lastIndex {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    *host.indexes.last().unwrap_or(&NS_NOT_FOUND)
}

- (NSUInteger)indexGreaterThanIndex:(NSUInteger)index {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    host.indexes
        .iter()
        .copied()
        .find(|&i| i > index)
        .unwrap_or(NS_NOT_FOUND)
}

- (NSUInteger)indexLessThanIndex:(NSUInteger)index {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    host.indexes
        .iter()
        .copied()
        .rev()
        .find(|&i| i < index)
        .unwrap_or(NS_NOT_FOUND)
}

- (NSUInteger)indexGreaterThanOrEqualToIndex:(NSUInteger)index {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    host.indexes
        .iter()
        .copied()
        .find(|&i| i >= index)
        .unwrap_or(NS_NOT_FOUND)
}

- (NSUInteger)indexLessThanOrEqualToIndex:(NSUInteger)index {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    host.indexes
        .iter()
        .copied()
        .rev()
        .find(|&i| i <= index)
        .unwrap_or(NS_NOT_FOUND)
}

// MARK: - Equality

- (bool)isEqualToIndexSet:(id)other { // NSIndexSet*
    if this == other { return true; }
    if other == nil  { return false; }
    let a = env.objc.borrow::<NSIndexSetHostObject>(this).indexes.clone();
    let b = env.objc.borrow::<NSIndexSetHostObject>(other);
    a == b.indexes
}

- (bool)isEqual:(id)other {
    if this == other { return true; }
    if other == nil  { return false; }
    let cls: id = msg_class![env; NSIndexSet class];
    let is_kind: bool = msg![env; other isKindOfClass:cls];
    if !is_kind {
        return false;
    }
    let a = env.objc.borrow::<NSIndexSetHostObject>(this).indexes.clone();
    let b = env.objc.borrow::<NSIndexSetHostObject>(other);
    a == b.indexes
}

- (NSUInteger)hash {
    env.objc.borrow::<NSIndexSetHostObject>(this).indexes.len() as NSUInteger
}

// MARK: - NSCopying

- (id)copyWithZone:(NSZonePtr)_zone {
    // NSIndexSet is immutable — return self retained.
    retain(env, this)
}

- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    let new: id = msg_class![env; NSMutableIndexSet alloc];
    let idxs = env.objc.borrow::<NSIndexSetHostObject>(this).indexes.clone();
    env.objc.borrow_mut::<NSIndexSetHostObject>(new).indexes = idxs;
    autorelease(env, new)
}

// MARK: - Description

- (id)description {
    let host = env.objc.borrow::<NSIndexSetHostObject>(this);
    let parts: Vec<String> = host.indexes.iter().map(|i| i.to_string()).collect();
    let s = format!("<NSIndexSet: {} index(es) [{}]>", parts.len(), parts.join(", "));
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

// NSMutableIndexSet adds the mutation half of Apple's index-set API.
// <https://developer.apple.com/documentation/foundation/nsmutableindexset>
@implementation NSMutableIndexSet: NSIndexSet

- (id)copyWithZone:(NSZonePtr)_zone {
    let new: id = msg_class![env; NSIndexSet alloc];
    let idxs = env.objc.borrow::<NSIndexSetHostObject>(this).indexes.clone();
    env.objc.borrow_mut::<NSIndexSetHostObject>(new).indexes = idxs;
    new
}

- (())addIndex:(NSUInteger)index {
    add_range(
        env.objc.borrow_mut::<NSIndexSetHostObject>(this),
        index,
        1,
    );
}

- (())addIndexes:(id)other { // NSIndexSet*
    for index in indexes_of(env, other) {
        add_range(
            env.objc.borrow_mut::<NSIndexSetHostObject>(this),
            index,
            1,
        );
    }
}

- (())addIndexesInRange:(NSRange)range {
    add_range(
        env.objc.borrow_mut::<NSIndexSetHostObject>(this),
        range.location,
        range.length,
    );
}

- (())removeIndex:(NSUInteger)index {
    remove_range(
        env.objc.borrow_mut::<NSIndexSetHostObject>(this),
        index,
        1,
    );
}

- (())removeIndexes:(id)other { // NSIndexSet*
    for index in indexes_of(env, other) {
        remove_range(
            env.objc.borrow_mut::<NSIndexSetHostObject>(this),
            index,
            1,
        );
    }
}

- (())removeIndexesInRange:(NSRange)range {
    remove_range(
        env.objc.borrow_mut::<NSIndexSetHostObject>(this),
        range.location,
        range.length,
    );
}

- (())removeAllIndexes {
    env.objc
        .borrow_mut::<NSIndexSetHostObject>(this)
        .indexes
        .clear();
}

// Shifts every index greater than or equal to `index` by `delta`. Apple's
// documentation does not allow the result to become negative: indexes that
// would move below zero are removed instead of wrapping.
// https://developer.apple.com/documentation/foundation/nsmutableindexset/1415640-shiftindexesstartingatindex
- (())shiftIndexesStartingAtIndex:(NSUInteger)index by:(NSInteger)delta {
    let host = env.objc.borrow_mut::<NSIndexSetHostObject>(this);
    let mut shifted = Vec::with_capacity(host.indexes.len());
    for &old in &host.indexes {
        let new = if old >= index {
            (old as i64) + (delta as i64)
        } else {
            old as i64
        };
        if new >= 0 {
            shifted.push(new as NSUInteger);
        }
    }
    host.indexes = shifted;
}

// Removes every index not present in `other`.
- (())intersectIndexes:(id)other { // NSIndexSet*
    let keep = indexes_of(env, other);
    let host = env.objc.borrow_mut::<NSIndexSetHostObject>(this);
    host.indexes.retain(|index| keep.binary_search(index).is_ok());
}

@end

};
