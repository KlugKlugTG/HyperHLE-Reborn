/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `CFData` and `CFMutableData`.
//!
//! These are toll-free bridged to `NSData` and `NSMutableData` in Apple's
//! implementation.
//! Here they are the same types.
//!
//! Guest-supplied lengths, ranges and pointers are validated: bad input makes
//! a function return `nil`/0/not-found or do nothing, never panic the host.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{kCFNotFound, CFIndex, CFOptionFlags, CFRange, CFRelease, CFRetain, CFTypeRef};
use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::frameworks::foundation::{NSRange, NSUInteger};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

pub type CFDataRef = CFTypeRef;
pub type CFMutableDataRef = CFTypeRef;

pub type CFDataSearchFlags = CFOptionFlags;
#[allow(dead_code)]
pub const kCFDataSearchBackwards: CFDataSearchFlags = 1 << 0;
#[allow(dead_code)]
pub const kCFDataSearchAnchored: CFDataSearchFlags = 1 << 1;

/// Upper bound on any single data buffer we will create or grow (256 MiB).
/// Stops a garbage length from exhausting host memory.
const MAX_DATA_LEN: NSUInteger = 256 * 1024 * 1024;
/// Largest capacity hint we honour up front (16 MiB).
const MAX_CAPACITY_HINT: NSUInteger = 16 * 1024 * 1024;

// MARK: - Helpers

fn validate_allocator(env: &mut Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault
        || allocator.is_null()
        || env.mem.read(allocator).is_system_default()
}

/// Convert a `CFIndex` length to `NSUInteger`, rejecting negatives and
/// anything over `MAX_DATA_LEN`.
fn checked_len(length: CFIndex) -> Option<NSUInteger> {
    let len = NSUInteger::try_from(length).ok()?;
    (len <= MAX_DATA_LEN).then_some(len)
}

/// Convert a `CFRange` to an `NSRange` that lies entirely inside `data`.
fn ns_range_checked(env: &mut Environment, data: CFDataRef, range: CFRange) -> Option<NSRange> {
    let location = NSUInteger::try_from(range.location).ok()?;
    let length = NSUInteger::try_from(range.length).ok()?;
    let total: NSUInteger = msg![env; data length];
    if location.checked_add(length)? > total {
        return None;
    }
    Some(NSRange { location, length })
}

// MARK: - CFData (immutable)

pub fn CFDataCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) -> CFDataRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    let Some(length) = checked_len(length) else {
        return nil;
    };
    if bytes.is_null() && length != 0 {
        return nil;
    }
    let bytes: ConstVoidPtr = bytes.cast();
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

pub fn CFDataCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    data: CFDataRef,
) -> CFDataRef {
    if !validate_allocator(env, allocator) || data.is_null() {
        return nil;
    }
    let bytes: ConstVoidPtr = msg![env; data bytes];
    let length: NSUInteger = msg![env; data length];
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

fn CFDataCreateWithBytesNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
    _bytes_deallocator: CFAllocatorRef, // ignored — we copy anyway
) -> CFDataRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    // We don't support custom deallocators; just copy the bytes.
    CFDataCreate(env, kCFAllocatorDefault, bytes, length)
}

pub fn CFDataGetLength(env: &mut Environment, data: CFDataRef) -> CFIndex {
    if data.is_null() {
        return 0;
    }
    let len: NSUInteger = msg![env; data length];
    len.try_into().unwrap_or(CFIndex::MAX)
}

pub fn CFDataGetBytePtr(env: &mut Environment, data: CFDataRef) -> ConstPtr<u8> {
    if data.is_null() {
        return ConstPtr::null();
    }
    let ptr: ConstVoidPtr = msg![env; data bytes];
    ptr.cast()
}

fn CFDataGetBytes(env: &mut Environment, data: CFDataRef, range: CFRange, buffer: MutPtr<u8>) {
    if data.is_null() || buffer.is_null() {
        return;
    }
    let Some(range) = ns_range_checked(env, data, range) else {
        return;
    };
    () = msg![env; data getBytes:buffer range:range]
}

fn CFDataGetMutableBytePtr(env: &mut Environment, data: CFMutableDataRef) -> MutPtr<u8> {
    if data.is_null() {
        return MutPtr::null();
    }
    let ptr: MutVoidPtr = msg![env; data mutableBytes];
    ptr.cast()
}

// MARK: - CFData retain / release / equality

pub fn CFDataRetain(env: &mut Environment, data: CFDataRef) -> CFDataRef {
    if !data.is_null() {
        CFRetain(env, data)
    } else {
        data
    }
}

pub fn CFDataRelease(env: &mut Environment, data: CFDataRef) {
    if !data.is_null() {
        CFRelease(env, data);
    }
}

fn CFDataIsEqual(env: &mut Environment, a: CFDataRef, b: CFDataRef) -> bool {
    if a == b {
        return true;
    }
    if a.is_null() || b.is_null() {
        return false;
    }
    let len_a: NSUInteger = msg![env; a length];
    let len_b: NSUInteger = msg![env; b length];
    if len_a != len_b {
        return false;
    }
    msg![env; a isEqualToData:b]
}

// MARK: - Searching

/// `CFRange CFDataFind(CFDataRef, CFDataRef dataToFind, CFRange searchRange,
///                     CFDataSearchFlags options)`
fn CFDataFind(
    env: &mut Environment,
    data: CFDataRef,
    data_to_find: CFDataRef,
    search_range: CFRange,
    compare_options: CFDataSearchFlags,
) -> CFRange {
    let not_found = CFRange {
        location: kCFNotFound,
        length: 0,
    };
    if data.is_null() || data_to_find.is_null() {
        return not_found;
    }
    let (Ok(start), Ok(range_len)) = (
        usize::try_from(search_range.location),
        usize::try_from(search_range.length),
    ) else {
        return not_found;
    };
    let total: NSUInteger = msg![env; data length];
    let needle_len: NSUInteger = msg![env; data_to_find length];
    let (total, needle_len) = (total as usize, needle_len as usize);
    let Some(end) = start.checked_add(range_len) else {
        return not_found;
    };
    // Empty needle, range outside the data, or needle longer than the range.
    if needle_len == 0 || end > total || needle_len > range_len {
        return not_found;
    }

    // Copy both buffers to the host so no guest borrow is held while searching.
    let hay_ptr = CFDataGetBytePtr(env, data);
    let needle_ptr = CFDataGetBytePtr(env, data_to_find);
    let haystack = env
        .mem
        .bytes_at(hay_ptr + start as GuestUSize, range_len as GuestUSize)
        .to_vec();
    let needle = env
        .mem
        .bytes_at(needle_ptr, needle_len as GuestUSize)
        .to_vec();

    let backwards = compare_options & kCFDataSearchBackwards != 0;
    let anchored = compare_options & kCFDataSearchAnchored != 0;
    let last_offset = range_len - needle_len;

    let found: Option<usize> = match (backwards, anchored) {
        // Anchored: the match must touch the start (or end, if backwards)
        // of the search range.
        (false, true) => (haystack[..needle_len] == needle[..]).then_some(0),
        (true, true) => (haystack[last_offset..] == needle[..]).then_some(last_offset),
        (false, false) => haystack.windows(needle_len).position(|w| w == &needle[..]),
        (true, false) => haystack.windows(needle_len).rposition(|w| w == &needle[..]),
    };

    match found {
        Some(off) => CFRange {
            location: (start + off) as CFIndex,
            length: needle_len as CFIndex,
        },
        None => not_found,
    }
}

// MARK: - CFMutableData

fn CFDataCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
) -> CFMutableDataRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    let Ok(cap) = NSUInteger::try_from(capacity) else {
        return nil; // negative capacity
    };
    let new: id = msg_class![env; NSMutableData alloc];
    // Capacity is only a hint here; never pre-allocate more than a sane amount.
    if cap > 0 && cap <= MAX_CAPACITY_HINT {
        msg![env; new initWithCapacity:cap]
    } else {
        msg![env; new init]
    }
}

fn CFDataCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    _capacity: CFIndex,
    data: CFDataRef,
) -> CFMutableDataRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    let new: id = msg_class![env; NSMutableData alloc];
    if data.is_null() {
        // Real CF would crash here; return an empty mutable data instead.
        return msg![env; new init];
    }
    msg![env; new initWithData:data]
}

fn CFDataSetLength(env: &mut Environment, data: CFMutableDataRef, length: CFIndex) {
    if data.is_null() {
        return;
    }
    let Some(len) = checked_len(length) else {
        return;
    };
    () = msg![env; data setLength:len]
}

fn CFDataIncreaseLength(env: &mut Environment, data: CFMutableDataRef, extra_length: CFIndex) {
    if data.is_null() {
        return;
    }
    let Ok(extra) = NSUInteger::try_from(extra_length) else {
        return;
    };
    let current: NSUInteger = msg![env; data length];
    let Some(new_len) = current.checked_add(extra).filter(|l| *l <= MAX_DATA_LEN) else {
        return;
    };
    () = msg![env; data setLength:new_len]
}

fn CFDataAppendBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) {
    if data.is_null() {
        return;
    }
    let Some(len) = checked_len(length) else {
        return;
    };
    if len == 0 || bytes.is_null() {
        return; // nothing to append, or a NULL source we must not read
    }
    let current: NSUInteger = msg![env; data length];
    if current.checked_add(len).map_or(true, |l| l > MAX_DATA_LEN) {
        return;
    }
    () = msg![env; data appendBytes:(bytes.cast::<std::ffi::c_void>()) length:len]
}

fn CFDataDeleteBytes(env: &mut Environment, data: CFMutableDataRef, range: CFRange) {
    if data.is_null() {
        return;
    }
    let Some(range) = ns_range_checked(env, data, range) else {
        return;
    };
    () = msg![env; data replaceBytesInRange:range withBytes:(nil) length:0u32]
}

fn CFDataReplaceBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    range: CFRange,
    new_bytes: ConstPtr<u8>,
    new_length: CFIndex,
) {
    if data.is_null() {
        return;
    }
    let Some(range) = ns_range_checked(env, data, range) else {
        return;
    };
    let Some(new_len) = checked_len(new_length) else {
        return;
    };
    if new_bytes.is_null() && new_len != 0 {
        return;
    }
    // Refuse a replacement that would push the buffer past the size cap.
    let current: NSUInteger = msg![env; data length];
    let resulting = (current - range.length).saturating_add(new_len);
    if resulting > MAX_DATA_LEN {
        return;
    }
    () = msg![env; data replaceBytesInRange:range
                                  withBytes:(new_bytes.cast::<std::ffi::c_void>())
                                     length:new_len]
}

pub const FUNCTIONS: FunctionExports = &[
    // Immutable
    export_c_func!(CFDataCreate(_, _, _)),
    export_c_func!(CFDataCreateCopy(_, _)),
    export_c_func!(CFDataCreateWithBytesNoCopy(_, _, _, _)),
    export_c_func!(CFDataGetLength(_)),
    export_c_func!(CFDataGetBytePtr(_)),
    export_c_func!(CFDataGetMutableBytePtr(_)),
    export_c_func!(CFDataGetBytes(_, _, _)),
    export_c_func!(CFDataRetain(_)),
    export_c_func!(CFDataRelease(_)),
    export_c_func!(CFDataIsEqual(_, _)),
    export_c_func!(CFDataFind(_, _, _, _)),
    // Mutable
    export_c_func!(CFDataCreateMutable(_, _)),
    export_c_func!(CFDataCreateMutableCopy(_, _, _)),
    export_c_func!(CFDataSetLength(_, _)),
    export_c_func!(CFDataIncreaseLength(_, _)),
    export_c_func!(CFDataAppendBytes(_, _, _)),
    export_c_func!(CFDataDeleteBytes(_, _)),
    export_c_func!(CFDataReplaceBytes(_, _, _, _)),
];
