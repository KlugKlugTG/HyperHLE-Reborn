/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSKeyedUnarchiver` and deserialization of its object graph format.
//!
//! Resources:
//! - You can get a good intuitive grasp of how the format works just by staring
//!   at a pretty-print of a simple nib file from something that can parse
//!   plists, e.g. `plutil -p` or `println!("{:#?}", plist::Value::...);`.
//! - Apple's [Archives and Serializations Programming Guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Archiving/Articles/archives.html)

use super::ns_error::{NSCocoaErrorDomain, NSPropertyListReadCorruptError};
use super::ns_string::{from_rust_string, get_static_str, to_rust_string};
use super::ns_value::NSNumberHostObject;
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::frameworks::uikit::ui_geometry::{
    CGPointFromString, CGRectFromString, CGSizeFromString,
};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;
use plist::{Dictionary, Uid, Value};
use std::io::Cursor;

pub const NSKeyedArchiveRootObjectKey: &str = "root";

pub const CONSTANTS: ConstantExports = &[(
    "_NSKeyedArchiveRootObjectKey",
    HostConstant::NSString(NSKeyedArchiveRootObjectKey),
)];

#[derive(Default)]
struct NSKeyedUnarchiverHostObject {
    plist: Dictionary,
    current_key: Option<Uid>,
    /// linear map of Uid => id
    already_unarchived: Vec<Option<id>>,
    /// Something responding to NSKeyedUnarchiverDelegate
    delegate: id,
    /// Stores the buffers decoded by `decodeBytesForKey:returnedLength:`
    /// Instead of reusing the same buffer, we allocate different ones that get
    /// freed on dealloc. A similar behavior has been observed in real iOS.
    temporary_buffers: Vec<MutVoidPtr>,
}
impl HostObject for NSKeyedUnarchiverHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSKeyedUnarchiver: NSCoder

+ (id)allocWithZone:(NSZonePtr)_zone { // struct _NSZone*
    let unarchiver = Box::new(NSKeyedUnarchiverHostObject {
        plist: Dictionary::new(),
        current_key: None,
        already_unarchived: Vec::new(),
        delegate: nil,
        temporary_buffers: Vec::new(),
    });
    env.objc.alloc_object(this, unarchiver, &mut env.mem)
}

// Chrome (and other apps) re-map classes when unarchiving nibs:
// [NSKeyedUnarchiver setClass:ActualClass forClassName:@"NibName"].
// The nib decoder already resolves swapped classes via UIClassSwapper,
// so acknowledging the mapping without storing it is enough.
+ (())setClass:(id)_class forClassName:(id)_codename {
    log_dbg!(
        "NSKeyedUnarchiver setClass:{:?} forClassName:{:?} (mapping is a no-op)",
        _class,
        _codename
    );
}

+ (id)unarchiveObjectWithFile:(id)path { // NSString *
    let data: id = msg_class![env; NSData dataWithContentsOfFile:path];
    if data == nil {
        return nil;
    }
    msg![env; this unarchiveObjectWithData:data]
}

+ (id)unarchiveObjectWithData:(id)data { // NSData *
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initForReadingWithData:data];
    let root_key = get_static_str(env, NSKeyedArchiveRootObjectKey);
    let result: id = msg![env; new decodeObjectForKey:root_key];
    autorelease(env, result)
}

+ (id)unarchiveTopLevelObjectWithData:(id)data
                                error:(MutPtr<MutVoidPtr>)error {
    if !error.is_null() {
        env.mem.write(error, nil.cast());
    }
    let result: id = msg![env; this unarchiveObjectWithData:data];
    if result == nil && !error.is_null() {
        {
            let err = plist_read_error(env);
            env.mem.write(error, err.cast());
        }
    }
    result
}

+ (id)unarchivedObjectOfClass:(id)_class
                     fromData:(id)data
                        error:(MutPtr<MutVoidPtr>)error {
    // We never enforce class conformance (see setRequiresSecureCoding:),
    // so this is identical to unarchiveTopLevelObjectWithData:error:.
    msg![env; this unarchiveTopLevelObjectWithData:data error:error]
}

// iOS 9+ variant of initForReadingWithData:; same semantics, plus an
// NSError out-param that is filled in on failure.
- (id)initForReadingFromData:(id)data error:(MutPtr<MutVoidPtr>)error {
    if !error.is_null() {
        env.mem.write(error, nil.cast());
    }
    let unarchiver: id = msg![env; this initForReadingWithData:data];
    if unarchiver == nil && !error.is_null() {
        {
            let err = plist_read_error(env);
            env.mem.write(error, err.cast());
        }
    }
    unarchiver
}
- (id)initForReadingWithData:(id)data { // NSData *
    if data == nil {
        release(env, this);
        return nil;
    }

    let length: NSUInteger = msg![env; data length];
    let bytes: ConstVoidPtr = msg![env; data bytes];

    // 1. Честная проверка на пустые данные или null-указатель
    if length == 0 || bytes.is_null() {
        log!("Warning: [NSKeyedUnarchiver initForReadingWithData:] called with empty data. Returning nil.");
        release(env, this);
        return nil;
    }

    let slice = env.mem.bytes_at(bytes.cast(), length);

    // 2. Безопасный парсинг plist вместо жесткого .unwrap()
    let plist = match Value::from_reader(Cursor::new(slice)) {
        Ok(p) => p,
        Err(e) => {
            log!("Warning: [NSKeyedUnarchiver initForReadingWithData:] failed to parse plist: {:?}", e);
            release(env, this);
            return nil;
        }
    };

    let plist = match plist.into_dictionary() {
        Some(d) => d,
        None => {
            log!("Warning: [NSKeyedUnarchiver initForReadingWithData:] root is not a dictionary.");
            release(env, this);
            return nil;
        }
    };

    // 3. Безопасная проверка версии и типа архива
    if plist.get("$version").and_then(|v| v.as_unsigned_integer()) != Some(100000) {
        log!("Warning: [NSKeyedUnarchiver initForReadingWithData:] unsupported archiver version.");
        release(env, this);
        return nil;
    }

    if plist.get("$archiver").and_then(|v| v.as_string()) != Some("NSKeyedArchiver") {
        log!("Warning: [NSKeyedUnarchiver initForReadingWithData:] unsupported archiver type.");
        release(env, this);
        return nil;
    }

    let key_count = plist.get("$objects").and_then(|v| v.as_array()).map_or(0, |a| a.len());

    // 4. Re-initialising an unarchiver instance is unusual but must not
    // panic the host: reset any previous state instead of asserting.
    {
        // Take the state out first so the host-object borrow is not held
        // across the `release()` calls (same pattern as `dealloc`).
        let (already_unarchived, temporary_buffers) = {
            let host_obj = env.objc.borrow_mut::<NSKeyedUnarchiverHostObject>(this);
            (
                std::mem::take(&mut host_obj.already_unarchived),
                std::mem::take(&mut host_obj.temporary_buffers),
            )
        };
        for &object in already_unarchived.iter().flatten() {
            release(env, object);
        }
        for &buffer in temporary_buffers.iter() {
            env.mem.free(buffer);
        }
        env.objc
            .borrow_mut::<NSKeyedUnarchiverHostObject>(this)
            .current_key = None;
    }

    let host_obj = env.objc.borrow_mut::<NSKeyedUnarchiverHostObject>(this);
    host_obj.already_unarchived = vec![None; key_count];
    host_obj.plist = plist;

    this
}

- (())dealloc {
    let host_obj = borrow_host_obj(env, this);
    let already_unarchived = std::mem::take(&mut host_obj.already_unarchived);
    let temporary_buffers = std::mem::take(&mut host_obj.temporary_buffers);

    for &object in already_unarchived.iter().flatten() {
        release(env, object);
    }

    for &buffer in temporary_buffers.iter() {
        env.mem.free(buffer);
    }

    env.objc.dealloc_object(this, &mut env.mem)
}

// The delegate is consulted in `finishDecoding` (will/did finish) and after
// each object has been decoded (`unarchiver:didDecodeObject:`, see
// `unarchive_key` below). All of the delegate methods are optional, so every
// call site checks that the delegate responds to the selector first.
// weak/non-retaining
- (())setDelegate:(id)delegate { // id<NSKeyedUnarchiverDelegate>
    let host_object = env.objc.borrow_mut::<NSKeyedUnarchiverHostObject>(this);
    host_object.delegate = delegate;
}
- (id)delegate {
    env.objc.borrow::<NSKeyedUnarchiverHostObject>(this).delegate
}

- (bool)allowsKeyedCoding {
    true
}

// These methods drive most of the decoding. They get called in two cases:
// - By the code that initiates the unarchival, e.g. UINib, to retrieve
//   top-level objects.
// - By the object currently being unarchived, i.e. something that had
//   `initWithCoder:` called on it, to retrieve objects from its scope.
// They are all from the NSCoder abstract class and they return default values
// if the key is unknown.

- (bool)decodeBoolForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return false; };
    if let Some(b) = value.as_boolean() { return b; }
    if let Some(i) = value.as_signed_integer() { return i != 0; }
    if let Some(u) = value.as_unsigned_integer() { return u != 0; }
    log!("Warning: decodeBoolForKey: non-boolean value {:?}; returning false.", value);
    false
}

- (f64)decodeDoubleForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return 0.0; };
    if let Some(r) = value.as_real() { return r; }
    if let Some(i) = value.as_signed_integer() { return i as f64; }
    if let Some(u) = value.as_unsigned_integer() { return u as f64; }
    log!("Warning: decodeDoubleForKey: non-numeric value {:?}; returning 0.0.", value);
    0.0
}

- (f32)decodeFloatForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return 0.0; };
    if let Some(r) = value.as_real() { return r as f32; }
    if let Some(i) = value.as_signed_integer() { return i as f32; }
    if let Some(u) = value.as_unsigned_integer() { return u as f32; }
    log!("Warning: decodeFloatForKey: non-numeric value {:?}; returning 0.0.", value);
    0.0
}

- (NSInteger)decodeIntegerForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return 0; };
    let Some(i) = value.as_signed_integer() else {
        log!("Warning: decodeIntegerForKey: non-integer value {:?}; returning 0.", value);
        return 0;
    };
    // Clamp to NSInteger range instead of panicking.
    if i > NSInteger::MAX as i64 { return NSInteger::MAX; }
    if i < NSInteger::MIN as i64 { return NSInteger::MIN; }
    i as NSInteger
}

- (i32)decodeIntForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return 0; };
    let Some(i) = value.as_signed_integer() else {
        log!("Warning: decodeIntForKey: non-integer value {:?}; returning 0.", value);
        return 0;
    };
    if i > i32::MAX as i64 { return i32::MAX; }
    if i < i32::MIN as i64 { return i32::MIN; }
    i as i32
}

- (i32)decodeInt32ForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return 0; };
    let Some(i) = value.as_signed_integer() else {
        log!("Warning: decodeInt32ForKey: non-integer value {:?}; returning 0.", value);
        return 0;
    };
    if i > i32::MAX as i64 { return i32::MAX; }
    if i < i32::MIN as i64 { return i32::MIN; }
    i as i32
}

- (i64)decodeInt64ForKey:(id)key { // NSString *
    let Some(value) = get_value_to_decode_for_key(env, this, key) else { return 0; };
    let Some(i) = value.as_signed_integer() else {
        log!("Warning: decodeInt64ForKey: non-integer value {:?}; returning 0.", value);
        return 0;
    };
    i
}

- (id)decodeObjectForKey:(id)key { // NSString*
    let Some(next_uid) = get_value_to_decode_for_key(env, this, key) else {
        return nil;
    };
    let Some(next_uid) = next_uid.as_uid().copied() else {
        log!("Warning: decodeObjectForKey: value {:?} is not a UID; returning nil.", next_uid);
        return nil;
    };
    let object = unarchive_key(env, this, next_uid);

    // on behalf of the caller
    retain(env, object);
    autorelease(env, object)
}

- (ConstPtr<u8>)decodeBytesForKey:(id)key returnedLength:(MutPtr<NSUInteger>)length {
    if key == nil {
        env.mem.write(length, 0);
        return ConstPtr::null();
    }
    let Some(data) = get_value_to_decode_for_key(env, this, key)
        .and_then(|value| value.as_data())
        .map(|data| data.to_vec()) else {
            env.mem.write(length, 0);
            return ConstPtr::null();
    };
    let Ok(len) = data.len().try_into() else {
        // A >4 GiB entry cannot exist in guest memory; allocating a
        // GuestUSize::MAX-sized buffer would only waste memory. Fail cleanly.
        log!("Warning: decodeBytesForKey: data of length {} exceeds u32; returning NULL.", data.len());
        env.mem.write(length, 0);
        return ConstPtr::null();
    };
    let guest_bytes: MutVoidPtr = env.mem.alloc(len);
    env.objc.borrow_mut::<NSKeyedUnarchiverHostObject>(this)
        .temporary_buffers
        .push(guest_bytes);
    let copy_len = std::cmp::min(len as usize, data.len());
    env.mem
        .bytes_at_mut(guest_bytes.cast(), copy_len as GuestUSize)
        .copy_from_slice(&data[..copy_len]);
    env.mem.write(length, len);
    guest_bytes.cast().cast_const()
}

- (bool)containsValueForKey:(id)key { // NSString*
    if key == nil { return false; }
    get_value_to_decode_for_key(env, this, key).is_some()
}

// The secure-coding variants skip class-conformance checks (we never
// enforce them, see setRequiresSecureCoding:) and forward to
// decodeObjectForKey:.
- (id)decodeObjectOfClass:(id)_class forKey:(id)key { // Class, NSString*
    msg![env; this decodeObjectForKey:key]
}
- (id)decodeObjectOfClasses:(id)_classes forKey:(id)key { // NSSet*, NSString*
    msg![env; this decodeObjectForKey:key]
}

// These treat the root object like any other keyed value.
- (id)decodeTopLevelObjectForKey:(id)key { // NSString*
    msg![env; this decodeObjectForKey:key]
}
- (id)decodePropertyListForKey:(id)key { // NSString*
    msg![env; this decodeObjectForKey:key]
}
- (id)decodeTopLevelObjectAndReturnError:(MutPtr<MutVoidPtr>)error {
    if !error.is_null() {
        env.mem.write(error, nil.cast());
    }
    let root_key = get_static_str(env, NSKeyedArchiveRootObjectKey);
    msg![env; this decodeObjectForKey:root_key]
}

// These come from a category in UIKit's UIGeometry.h
- (CGPoint)decodeCGPointForKey:(id)key { // NSString*
    let string: id = msg![env; this decodeObjectForKey:key];
    CGPointFromString(env, string)
}
- (CGSize)decodeCGSizeForKey:(id)key { // NSString*
    let string: id = msg![env; this decodeObjectForKey:key];
    CGSizeFromString(env, string)
}
- (CGRect)decodeCGRectForKey:(id)key { // NSString*
    let string: id = msg![env; this decodeObjectForKey:key];
    CGRectFromString(env, string)
}

// `- (void)finishDecoding`
// <https://developer.apple.com/documentation/foundation/nskeyedunarchiver/1418233-finishdecoding>
//
// Instructs the archiver to construct the final object graph.  Older apps
// (iOS < 9) call this directly; it is also called implicitly by
// `+unarchiveObjectWithData:` in Apple's implementation.  Our decode is
// already eager (objects are materialised as they are requested), so there
// is nothing to flush here — we simply notify the delegate if one has been
// set and return.
- (())finishDecoding {
    // Apple calls the will-finish delegate method before the final object
    // graph fixups and the did-finish method after them; our decode is
    // already eager, so ordering is all that remains to emulate.
    notify_delegate(env, this, "unarchiverWillFinish:");
    notify_delegate(env, this, "unarchiverDidFinish:");
}

// `- (void)setRequiresSecureCoding:(BOOL)flag`
// <https://developer.apple.com/documentation/foundation/nskeyedunarchiver/1413855-requiressecurecoding>
//
// Secure coding is a feature that prevents substitution attacks when
// deserialising objects.  touchHLE does not implement Class-level
// conformance checks, so we just store the flag and accept both values
// without enforcing anything.
- (())setRequiresSecureCoding:(bool)_flag {
    // No-op: we do not enforce secure coding checks.
}

- (bool)requiresSecureCoding {
    false
}

@end

};

fn borrow_host_obj(env: &mut Environment, unarchiver: id) -> &mut NSKeyedUnarchiverHostObject {
    env.objc.borrow_mut(unarchiver)
}

/// Builds a generic "archive is corrupt" NSError for the error out-params of
/// the modern `unarchive…` entry points.
fn plist_read_error(env: &mut Environment) -> id {
    let domain = get_static_str(env, NSCocoaErrorDomain);
    msg_class![env; NSError errorWithDomain:domain
                                       code:NSPropertyListReadCorruptError
                                   userInfo:nil]
}

/// Sends an optional, no-result delegate method (`sel_name`) to the
/// unarchiver's delegate, if the delegate implements it.
fn notify_delegate(env: &mut Environment, unarchiver: id, sel_name: &str) {
    let delegate = env
        .objc
        .borrow::<NSKeyedUnarchiverHostObject>(unarchiver)
        .delegate;
    if delegate == nil {
        return;
    }
    let Some(sel) = env.objc.lookup_selector(sel_name) else {
        return;
    };
    if !env
        .objc
        .class_has_method(crate::objc::ObjC::read_isa(delegate, &env.mem), sel)
    {
        return;
    }
    let _: () = crate::objc::msg_send_no_type_checking(env, (delegate, sel, unarchiver));
}

/// Notifies the delegate that an object has been decoded via the optional
/// `unarchiver:didDecodeObject:` method. Returns the delegate's replacement
/// object if one was supplied, or `object` otherwise.
fn notify_did_decode_object(env: &mut Environment, unarchiver: id, object: id) -> id {
    let delegate = env
        .objc
        .borrow::<NSKeyedUnarchiverHostObject>(unarchiver)
        .delegate;
    if delegate == nil {
        return object;
    }
    let Some(sel) = env.objc.lookup_selector("unarchiver:didDecodeObject:") else {
        return object;
    };
    if !env
        .objc
        .class_has_method(crate::objc::ObjC::read_isa(delegate, &env.mem), sel)
    {
        return object;
    }
    let replacement: id =
        crate::objc::msg_send_no_type_checking(env, (delegate, sel, unarchiver, object));
    if replacement == nil {
        object
    } else {
        replacement
    }
}

fn get_value_to_decode_for_key(env: &mut Environment, unarchiver: id, key: id) -> Option<&Value> {
    if key == nil {
        return None;
    }
    let key = to_rust_string(env, key); // TODO: avoid copying string
    let host_obj = borrow_host_obj(env, unarchiver);
    let scope_value = match host_obj.current_key {
        Some(current_uid) => {
            let objects = host_obj.plist.get("$objects").and_then(|v| v.as_array())?;
            objects.get(current_uid.get() as usize)?
        }
        None => host_obj.plist.get("$top")?,
    };
    let scope = scope_value.as_dictionary()?;
    scope.get(&key)
}

fn number_from_plist_value(value: &Value) -> Option<NSNumberHostObject> {
    match value {
        Value::Boolean(value) => Some(NSNumberHostObject::Bool(*value)),
        Value::Integer(value) => {
            value
                .as_signed()
                .map(NSNumberHostObject::LongLong)
                .or_else(|| {
                    value
                        .as_unsigned()
                        .map(NSNumberHostObject::UnsignedLongLong)
                })
        }
        Value::Real(value) => Some(NSNumberHostObject::Double(*value)),
        _ => None,
    }
}

pub(super) fn decode_current_number(
    env: &mut Environment,
    unarchiver: id,
) -> Option<NSNumberHostObject> {
    let host_obj = borrow_host_obj(env, unarchiver);
    let current_key = host_obj.current_key?;
    let objects = host_obj.plist.get("$objects")?.as_array()?;
    let item = objects.get(current_key.get() as usize)?.as_dictionary()?;

    if let Some(value) = item.get("NS.number") {
        return number_from_plist_value(value);
    }
    if let Some(value) = item.get("NS.boolval") {
        return value
            .as_boolean()
            .map(NSNumberHostObject::Bool)
            .or_else(|| {
                value
                    .as_signed_integer()
                    .map(|value| NSNumberHostObject::Bool(value != 0))
            })
            .or_else(|| {
                value
                    .as_unsigned_integer()
                    .map(|value| NSNumberHostObject::Bool(value != 0))
            });
    }
    if let Some(value) = item.get("NS.intval") {
        return value
            .as_signed_integer()
            .map(NSNumberHostObject::LongLong)
            .or_else(|| {
                value
                    .as_unsigned_integer()
                    .map(NSNumberHostObject::UnsignedLongLong)
            });
    }
    if let Some(value) = item.get("NS.dblval") {
        return value
            .as_real()
            .map(NSNumberHostObject::Double)
            .or_else(|| {
                value
                    .as_signed_integer()
                    .map(|value| NSNumberHostObject::Double(value as f64))
            })
            .or_else(|| {
                value
                    .as_unsigned_integer()
                    .map(|value| NSNumberHostObject::Double(value as f64))
            });
    }

    item.get("NS.numbervalue")
        .or_else(|| item.get("$0"))
        .and_then(number_from_plist_value)
}

/// The core of the implementation: unarchive something by its uid.
///
/// This is recursive in practice: the `initWithCoder:` messages sent by this
/// function will be received by objects which will then send
/// `decodeXXXWithKey:` messages back to the unarchiver, which will then call
/// this function (and so on).
///
/// The object returned is retained only by the archiver. Remember to retain and
/// possibly autorelease it as appropriate.
fn unarchive_key(env: &mut Environment, unarchiver: id, key: Uid) -> id {
    let host_obj = borrow_host_obj(env, unarchiver);
    let key_idx = key.get() as usize;
    if key_idx >= host_obj.already_unarchived.len() {
        log!(
            "Warning: unarchive_key: uid {} out of range (max {}); returning nil.",
            key.get(),
            host_obj.already_unarchived.len()
        );
        return nil;
    }
    if let Some(existing) = host_obj.already_unarchived[key_idx] {
        return existing;
    }

    let Some(objects) = host_obj.plist.get("$objects").and_then(|v| v.as_array()) else {
        log!("Warning: unarchive_key: $objects missing or not an array; returning nil.");
        return nil;
    };

    let Some(item) = objects.get(key_idx) else {
        log!(
            "Warning: unarchive_key: uid {} out of $objects range; returning nil.",
            key.get()
        );
        return nil;
    };
    let new_object = match item {
        // The most general kind of item: a dictionary that contains the info
        // needed to invoke `initWithCoder:` on a class implementing NSCoding.
        Value::Dictionary(dict) => {
            let Some(class_key) = dict.get("$class").and_then(|v| v.as_uid()).copied() else {
                log!(
                    "Warning: unarchive_key: missing $class for uid {}; returning nil.",
                    key.get()
                );
                return nil;
            };
            let class_key_idx = class_key.get() as usize;
            let class;
            if class_key_idx >= host_obj.already_unarchived.len() {
                log!(
                    "Warning: unarchive_key: class uid {} out of range; returning nil.",
                    class_key.get()
                );
                return nil;
            }
            if let Some(existing) = host_obj.already_unarchived[class_key_idx] {
                class = existing;
            } else {
                let Some(class_dict) = objects.get(class_key_idx) else {
                    log!("Warning: unarchive_key: class uid {} out of $objects range; returning nil.", class_key.get());
                    return nil;
                };
                let Some(class_dict) = class_dict.as_dictionary() else {
                    log!("Warning: unarchive_key: class entry at uid {} is not a dict; returning nil.", class_key.get());
                    return nil;
                };

                let Some(class_name) = class_dict.get("$classname").and_then(|v| v.as_string())
                else {
                    log!("Warning: unarchive_key: missing $classname for class uid {}; returning nil.", class_key.get());
                    return nil;
                };

                class = {
                    // get_known_class needs &mut ObjC, so we can't call it
                    // while holding a reference to the class name, since it
                    // is ultimately owned by ObjC via the host object
                    let class_name = class_name.to_string();
                    if class_name.is_empty() {
                        log!(
                            "Warning: unarchive_key: empty $classname for class uid {}.",
                            class_key.get()
                        );
                    }
                    env.objc.get_known_class(&class_name, &mut env.mem)
                };
                let host_obj = borrow_host_obj(env, unarchiver); // reborrow

                host_obj.already_unarchived[class_key_idx] = Some(class);
            };

            let host_obj = borrow_host_obj(env, unarchiver); // reborrow
            let old_current_key = host_obj.current_key;
            host_obj.current_key = Some(key);

            let new_object: id = msg![env; class alloc];
            let new_object: id = msg![env; new_object initWithCoder:unarchiver];

            let host_obj = borrow_host_obj(env, unarchiver); // reborrow
            host_obj.current_key = old_current_key;

            new_object
        }
        Value::String(s) => {
            let s = s.to_string();
            from_rust_string(env, s)
        }
        Value::Integer(int) => {
            let int = *int;
            // Similar logic to deserialize_plist()
            let number: id = msg_class![env; NSNumber alloc];
            // TODO: is this the correct order of preference? does it matter?
            if let Some(int64) = int.as_signed() {
                let longlong: i64 = int64;
                msg![env; number initWithLongLong:longlong]
            } else if let Some(uint64) = int.as_unsigned() {
                let ulonglong: u64 = uint64;
                msg![env; number initWithUnsignedLongLong:ulonglong]
            } else {
                // plist crate docs say this is unreachable, but if we ever
                // hit it just return a zero NSNumber rather than panicking.
                log!("Warning: unarchive_key: integer with no signed/unsigned representation; returning 0.");
                msg![env; number initWithInteger:(0 as NSInteger)]
            }
        }
        Value::Real(r) => {
            let r = *r;
            let number: id = msg_class![env; NSNumber alloc];
            msg![env; number initWithDouble:r]
        }
        Value::Boolean(b) => {
            let b = *b;
            let number: id = msg_class![env; NSNumber alloc];
            msg![env; number initWithBool:b]
        }
        Value::Data(data) => {
            let data = data.clone();
            let ns_data: id = msg_class![env; NSData alloc];
            let Ok(len) = data.len().try_into() else {
                // A >4 GiB entry cannot exist in guest memory; return nil
                // rather than attempting a GuestUSize::MAX-sized allocation.
                log!(
                    "Warning: unarchive_key: data at uid {} too large for guest memory; returning nil.",
                    key.get()
                );
                return nil;
            };
            let bytes_ptr: MutVoidPtr = env.mem.alloc(len);
            let copy_len = std::cmp::min(len as usize, data.len());
            env.mem
                .bytes_at_mut(bytes_ptr.cast(), copy_len as GuestUSize)
                .copy_from_slice(&data[..copy_len]);
            let ns_len: NSUInteger = copy_len as NSUInteger;
            let bytes_const: ConstVoidPtr = bytes_ptr.cast_const();
            let result: id = msg![env; ns_data initWithBytes:bytes_const length:ns_len];
            env.mem.free(bytes_ptr);
            result
        }
        // (Value::Dictionary is handled above)
        Value::Date(_) | Value::Array(_) | Value::Uid(_) => {
            log!(
                "Warning: unarchive_key: unhandled plist variant for uid {}; returning nil.",
                key.get()
            );
            nil
        }
        _ => {
            log!(
                "Warning: unarchive_key: unknown plist variant for uid {}; returning nil.",
                key.get()
            );
            nil
        }
    };

    let new_object = notify_did_decode_object(env, unarchiver, new_object);

    let host_obj = borrow_host_obj(env, unarchiver); // reborrow
    host_obj.already_unarchived[key_idx] = Some(new_object);
    new_object
}

/// Shortcut for use by `[_touchHLE_NSArray initWithCoder:]`.
///
/// The objects are to be considered retained by the `Vec`.
pub fn decode_current_array(env: &mut Environment, unarchiver: id) -> Vec<id> {
    let keys = keys_for_key(env, unarchiver, "NS.objects");

    keys.into_iter()
        .map(|key| {
            let new_object = unarchive_key(env, unarchiver, key);
            // object is retained by the Vec
            retain(env, new_object)
        })
        .collect()
}

/// Shortcut for use by `[_touchHLE_NSMutableDictionary initWithCoder:]`.
///
/// Similar to `decode_current_array`, but for dictionaries.
/// The keys and objects are not retained!
pub fn decode_current_dict(env: &mut Environment, unarchiver: id) -> Vec<(id, id)> {
    let keys = keys_for_key(env, unarchiver, "NS.keys");
    let vals = keys_for_key(env, unarchiver, "NS.objects");
    log_dbg!("decode_current_dict: keys {:?}, vals {:?}", keys, vals);

    let keys: Vec<id> = keys
        .into_iter()
        .map(|key| unarchive_key(env, unarchiver, key))
        .collect();
    let vals: Vec<id> = vals
        .into_iter()
        .map(|val| unarchive_key(env, unarchiver, val))
        .collect();

    keys.into_iter().zip(vals).collect()
}

/// Shortcut for use by `[NSDate initWithCoder:]`.
pub fn decode_current_date(env: &mut Environment, unarchiver: id) -> id {
    let key = get_static_str(env, "NS.time");
    // A corrupt or missing NS.time entry must not panic the host; return nil
    // so [NSDate initWithCoder:] fails gracefully.
    let Some(timestamp) =
        get_value_to_decode_for_key(env, unarchiver, key).and_then(|value| value.as_real())
    else {
        log!("Warning: decode_current_date: NS.time missing or not a real; returning nil.");
        return nil;
    };

    let date: id = msg_class![env; NSDate alloc];
    msg![env; date initWithTimeIntervalSinceReferenceDate:timestamp]
}

/// Shortcut for use by `[NSData initWithCoder:]`.
pub fn decode_current_data(env: &mut Environment, unarchiver: id, is_mutable: bool) -> id {
    let key = get_static_str(env, "NS.data");
    // TODO: avoid copying (twice!)
    // Missing or non-data entries occur in corrupt archives; return nil
    // instead of panicking.
    let Some(bytes) = get_value_to_decode_for_key(env, unarchiver, key)
        .and_then(|value| value.as_data())
        .map(|data| data.to_vec())
    else {
        log!("Warning: decode_current_data: NS.data missing or not data; returning nil.");
        return nil;
    };
    let Ok(len) = bytes.len().try_into() else {
        // A >4 GiB entry cannot exist in guest memory.
        log!(
            "Warning: decode_current_data: data of length {} exceeds u32; returning nil.",
            bytes.len()
        );
        return nil;
    };
    let guest_bytes: MutVoidPtr = env.mem.alloc(len);
    env.mem
        .bytes_at_mut(guest_bytes.cast(), len)
        .copy_from_slice(bytes.as_slice());

    let data: id = if is_mutable {
        let new: id = msg_class![env; NSMutableData alloc];
        msg![env; new initWithBytesNoCopy:guest_bytes length:len freeWhenDone:true]
    } else {
        let new: id = msg_class![env; NSData alloc];
        msg![env; new initWithBytesNoCopy:guest_bytes length:len freeWhenDone:true]
    };
    data
}

/// Decode an NSString-style payload while unarchiving.
pub fn decode_current_string(env: &mut Environment, unarchiver: id) -> id {
    let bytes = {
        let key = get_static_str(env, "NS.bytes");
        get_value_to_decode_for_key(env, unarchiver, key)
            .and_then(|value| value.as_data())
            .map(|data| data.to_vec())
    };
    let string = {
        let key = get_static_str(env, "NS.string");
        get_value_to_decode_for_key(env, unarchiver, key)
            .and_then(|value| value.as_string())
            .map(str::to_owned)
    };
    let Some(string) = decode_string_fields(bytes.as_deref(), string.as_deref()) else {
        log!(
            "Warning: decode_current_string: NS.bytes and NS.string missing or invalid; \
             returning empty string."
        );
        return from_rust_string(env, String::new());
    };
    from_rust_string(env, string)
}

fn decode_string_fields(bytes: Option<&[u8]>, string: Option<&str>) -> Option<String> {
    bytes
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .or_else(|| string.map(str::to_owned))
}

#[cfg(test)]
mod keyed_string_decode_tests {
    use super::decode_string_fields;
    use plist::Value;

    #[test]
    fn decodes_mutable_string_from_ns_string_field() {
        // BioShock's table-cell nibs store mutable text under NS.string:
        // https://github.com/shawnw67/Podroid/commit/1a31fab1.
        let archived_string = Value::String("ALL 3 Player".to_owned());
        assert_eq!(
            decode_string_fields(None, archived_string.as_string()),
            Some("ALL 3 Player".to_owned())
        );
    }

    #[test]
    fn uses_ns_string_when_ns_bytes_has_the_wrong_type() {
        let wrong_bytes = Value::String("not data".to_owned());
        let archived_string = Value::String("Player".to_owned());
        assert_eq!(
            decode_string_fields(wrong_bytes.as_data(), archived_string.as_string()),
            Some("Player".to_owned())
        );
    }

    #[test]
    fn prefers_ns_bytes_when_both_string_fields_exist() {
        let archived_bytes = Value::Data(b"Bytes".to_vec());
        let archived_string = Value::String("String".to_owned());
        assert_eq!(
            decode_string_fields(archived_bytes.as_data(), archived_string.as_string()),
            Some("Bytes".to_owned())
        );
    }

    #[test]
    fn rejects_missing_or_wrongly_typed_string_fields() {
        let wrong_bytes = Value::String("not data".to_owned());
        let wrong_string = Value::Data(b"not a string".to_vec());
        assert_eq!(decode_string_fields(None, None), None);
        assert_eq!(
            decode_string_fields(wrong_bytes.as_data(), wrong_string.as_string()),
            None
        );
    }
}

fn keys_for_key(env: &mut Environment, unarchiver: id, key: &str) -> Vec<Uid> {
    let host_obj = borrow_host_obj(env, unarchiver);
    let Some(objects) = host_obj.plist.get("$objects").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let Some(current_key) = host_obj.current_key else {
        return Vec::new();
    };
    let idx = current_key.get() as usize;
    if idx >= objects.len() {
        return Vec::new();
    }
    let item = &objects[idx];
    let Some(dict) = item.as_dictionary() else {
        return Vec::new();
    };
    let Some(arr) = dict.get(key).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|value| value.as_uid().copied())
        .collect()
}
