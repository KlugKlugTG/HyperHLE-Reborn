/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFNumber`.
//!
//! This is toll-free bridged to `NSNumber` in Apple's implementation.
//! Here it is the same type.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFComparisonResult, CFIndex, CFTypeRef};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_value::is_conversion_lossless;
use crate::mem::{ConstPtr, ConstVoidPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class};
use crate::Environment;

pub type CFNumberType = CFIndex;
pub const kCFNumberSInt8Type: CFNumberType = 1;
pub const kCFNumberSInt16Type: CFNumberType = 2;
pub const kCFNumberSInt32Type: CFNumberType = 3;
pub const kCFNumberSInt64Type: CFNumberType = 4;
pub const kCFNumberFloat32Type: CFNumberType = 5;
pub const kCFNumberFloat64Type: CFNumberType = 6;
pub const kCFNumberCharType: CFNumberType = 7;
pub const kCFNumberShortType: CFNumberType = 8;
pub const kCFNumberIntType: CFNumberType = 9;
pub const kCFNumberLongLongType: CFNumberType = 11;
pub const kCFNumberFloatType: CFNumberType = 12;
pub const kCFNumberDoubleType: CFNumberType = 13;
// Apple CFNumber.h: these are platform-width types. On the 32-bit iOS ABI
// that touchHLE emulates, NSInteger / CFIndex / long are 32-bit signed
// integers and CGFloat is a 32-bit float.
pub const kCFNumberLongType: CFNumberType = 10;
pub const kCFNumberCFIndexType: CFNumberType = 14;
pub const kCFNumberNSIntegerType: CFNumberType = 15;
pub const kCFNumberCGFloatType: CFNumberType = 16;

type CFNumberRef = CFTypeRef;
// Note: on iOS SDK side this type is defined as a pointer to an opaque struct
type CFBooleanRef = CFNumberRef;

fn CFNumberCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    type_: CFNumberType,
    value_ptr: ConstVoidPtr,
) -> CFNumberRef {
    // TODO: unique some common numbers to improve performance
    if allocator != kCFAllocatorDefault && !env.mem.read(allocator).is_system_default() {
        log!(
            "Warning: CFNumberCreate: custom allocator {:?} is not supported; \
             falling back to the system default allocator.",
            allocator
        );
    }
    log_dbg!("CFNumberCreate type {}", type_);
    let num = msg_class![env; NSNumber alloc];
    match type_ {
        kCFNumberSInt32Type | kCFNumberIntType => {
            let val: i32 = env.mem.read(value_ptr.cast());
            msg![env; num initWithInt:val]
        }
        // 32-bit-ABI integer types (NSInteger / CFIndex / long): all read as i32.
        kCFNumberLongType | kCFNumberCFIndexType | kCFNumberNSIntegerType => {
            let val: i32 = env.mem.read(value_ptr.cast());
            msg![env; num initWithInt:val]
        }
        kCFNumberSInt8Type | kCFNumberCharType => {
            let val: i8 = env.mem.read(value_ptr.cast());
            msg![env; num initWithChar:val]
        }
        kCFNumberSInt16Type | kCFNumberShortType => {
            let val: i16 = env.mem.read(value_ptr.cast());
            msg![env; num initWithShort:val]
        }
        kCFNumberFloat32Type | kCFNumberFloatType => {
            let val: f32 = env.mem.read(value_ptr.cast());
            msg![env; num initWithFloat:val]
        }
        // CGFloat is a 32-bit float on the emulated 32-bit ABI.
        kCFNumberCGFloatType => {
            let val: f32 = env.mem.read(value_ptr.cast());
            msg![env; num initWithFloat:val]
        }
        kCFNumberFloat64Type | kCFNumberDoubleType => {
            let val: f64 = env.mem.read(value_ptr.cast());
            msg![env; num initWithDouble:val]
        }
        kCFNumberSInt64Type | kCFNumberLongLongType => {
            let val: i64 = env.mem.read(value_ptr.cast());
            msg![env; num initWithLongLong:val]
        }
        _ => {
            log!(
                "Warning: CFNumberCreate: unsupported CFNumberType {}; defaulting to 0.",
                type_
            );
            msg![env; num initWithInt:(0_i32)]
        }
    }
}

fn CFNumberGetValue(
    env: &mut Environment,
    num: CFNumberRef,
    type_: CFNumberType,
    value_ptr: MutVoidPtr,
) -> bool {
    match type_ {
        kCFNumberSInt32Type | kCFNumberIntType => {
            let val: i32 = msg![env; num intValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        // 32-bit-ABI integer types (NSInteger / CFIndex / long): all i32-wide.
        kCFNumberLongType | kCFNumberCFIndexType | kCFNumberNSIntegerType => {
            let val: i32 = msg![env; num intValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        kCFNumberSInt8Type | kCFNumberCharType => {
            let val: i8 = msg![env; num charValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        kCFNumberSInt16Type | kCFNumberShortType => {
            let val: i16 = msg![env; num shortValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        kCFNumberFloat32Type | kCFNumberFloatType => {
            let val: f32 = msg![env; num floatValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        // CGFloat is a 32-bit float on the emulated 32-bit ABI.
        kCFNumberCGFloatType => {
            let val: f32 = msg![env; num floatValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        // ИСПРАВЛЕНИЕ: Добавлена полноценная обработка 64-битных целых чисел (Type 4 и 11)
        kCFNumberSInt64Type | kCFNumberLongLongType => {
            let val: i64 = msg![env; num longLongValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        // ИСПРАВЛЕНИЕ: Добавлена полноценная обработка 64-битных чисел с плавающей точкой (Type 6 и 13)
        kCFNumberFloat64Type | kCFNumberDoubleType => {
            let val: f64 = msg![env; num doubleValue];
            env.mem.write(value_ptr.cast(), val);
            is_conversion_lossless(env, num, type_)
        }
        _ => {
            log!(
                "Warning: CFNumberGetValue: unsupported CFNumberType {}; returning false.",
                type_
            );
            false
        }
    }
}

fn CFNumberCompare(
    env: &mut Environment,
    num1: CFNumberRef,
    num2: CFNumberRef,
    context: MutVoidPtr,
) -> CFComparisonResult {
    if !context.is_null() {
        log!("Warning: CFNumberCompare: context must be NULL; ignoring.");
    }
    msg![env; num1 compare:num2]
}

// CFNumber and NSNumber share storage. Report the canonical CF type of the
// bridged number's objCType, respecting guest NSNumber subclass overrides.
// CF does not promise to preserve the type alias used at creation time.
// https://developer.apple.com/documentation/corefoundation/cfnumbergettype(_:)
// https://github.com/apple-oss-distributions/CF/blob/main/CFNumber.c
fn CFNumberGetType(env: &mut Environment, number: CFNumberRef) -> CFNumberType {
    let encoding: ConstPtr<u8> = msg![env; number objCType];
    match env.mem.read(encoding) {
        b'c' | b'C' | b'B' => kCFNumberSInt8Type,
        b's' | b'S' => kCFNumberSInt16Type,
        b'i' | b'I' | b'l' | b'L' => kCFNumberSInt32Type,
        b'q' | b'Q' => kCFNumberSInt64Type,
        b'f' => kCFNumberFloat32Type,
        b'd' => kCFNumberFloat64Type,
        _ => 0,
    }
}

fn CFNumberGetByteSize(env: &mut Environment, number: CFNumberRef) -> CFIndex {
    match CFNumberGetType(env, number) {
        kCFNumberSInt8Type => 1,
        kCFNumberSInt16Type => 2,
        kCFNumberSInt32Type | kCFNumberFloat32Type => 4,
        kCFNumberSInt64Type | kCFNumberFloat64Type => 8,
        _ => 0,
    }
}

fn CFNumberIsFloatType(env: &mut Environment, number: CFNumberRef) -> bool {
    matches!(
        CFNumberGetType(env, number),
        kCFNumberFloat32Type | kCFNumberFloat64Type
    )
}

fn CFBooleanGetValue(env: &mut Environment, boolean: CFBooleanRef) -> bool {
    msg![env; boolean boolValue]
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFNull",
        HostConstant::Custom(|env| {
            // CFNull is a singleton CFType. Apps usually only compare against
            // it for identity (`x == kCFNull`) or pass it through dictionaries
            // representing JSON `null` values. NSNull already exists in our
            // runtime and is a singleton, so we reuse it: any later reference
            // to `kCFNull` will resolve to the same NSNull pointer-to-pointer.
            let null: id = msg_class![env; NSNull null];
            env.mem.alloc_and_write(null).cast_void().cast_const()
        }),
    ),
    (
        "_kCFBooleanFalse",
        HostConstant::Custom(|env| {
            let num = msg_class![env; NSNumber alloc];
            let num: id = msg![env; num initWithBool:false];
            // Apparently, it's a pointer to pointer
            env.mem.alloc_and_write(num).cast_void().cast_const()
        }),
    ),
    (
        "_kCFBooleanTrue",
        HostConstant::Custom(|env| {
            let num = msg_class![env; NSNumber alloc];
            let num: id = msg![env; num initWithBool:true];
            // Apparently, it's a pointer to pointer
            env.mem.alloc_and_write(num).cast_void().cast_const()
        }),
    ),
    // Apple Developer Documentation:
    // > kCFNumberPositiveInfinity   "Positive infinity (kCFNumberFloat64Type)."
    // > kCFNumberNegativeInfinity   "Negative infinity (kCFNumberFloat64Type)."
    // > kCFNumberNaN                "Not a number (kCFNumberFloat64Type)."
    //
    // These are documented `const CFNumberRef` singletons. The real CF
    // implementation creates them once at framework load time, marks them
    // as immortal, and lets `CFNumberGetType()` report
    // `kCFNumberFloat64Type` for each one. Here we build them with the
    // existing NSNumber bridge so they participate in toll-free bridging
    // with `CFNumber*` calls and behave correctly under `intValue` /
    // `doubleValue` / `compare:` (NaN sorts as `NSOrderedDescending`
    // against any non-NaN, infinities sort by sign — exactly the
    // semantics our `NSNumberHostObject::Double` already provides).
    (
        "_kCFNumberPositiveInfinity",
        HostConstant::Custom(|env| {
            let num = msg_class![env; NSNumber alloc];
            let num: id = msg![env; num initWithDouble:(f64::INFINITY)];
            env.mem.alloc_and_write(num).cast_void().cast_const()
        }),
    ),
    (
        "_kCFNumberNegativeInfinity",
        HostConstant::Custom(|env| {
            let num = msg_class![env; NSNumber alloc];
            let num: id = msg![env; num initWithDouble:(f64::NEG_INFINITY)];
            env.mem.alloc_and_write(num).cast_void().cast_const()
        }),
    ),
    (
        "_kCFNumberNaN",
        HostConstant::Custom(|env| {
            let num = msg_class![env; NSNumber alloc];
            let num: id = msg![env; num initWithDouble:(f64::NAN)];
            env.mem.alloc_and_write(num).cast_void().cast_const()
        }),
    ),
    // -----------------------------------------------------------------
    // CFNumberFormatter property key constants (CFStringRef).
    // These are the string keys passed to CFNumberFormatterSetProperty /
    // CFNumberFormatterCopyProperty. Apple documents their literal values
    // as their own names (e.g. "kCFNumberFormatterCurrencyCode").
    // Reference: Apple CF source & ICU backing.
    // -----------------------------------------------------------------
    (
        "_kCFNumberFormatterCurrencyCode",
        HostConstant::NSString("kCFNumberFormatterCurrencyCode"),
    ),
    (
        "_kCFNumberFormatterCurrencyDecimalSeparator",
        HostConstant::NSString("kCFNumberFormatterCurrencyDecimalSeparator"),
    ),
    (
        "_kCFNumberFormatterCurrencyGroupingSeparator",
        HostConstant::NSString("kCFNumberFormatterCurrencyGroupingSeparator"),
    ),
    (
        "_kCFNumberFormatterCurrencySymbol",
        HostConstant::NSString("kCFNumberFormatterCurrencySymbol"),
    ),
    (
        "_kCFNumberFormatterDecimalSeparator",
        HostConstant::NSString("kCFNumberFormatterDecimalSeparator"),
    ),
    (
        "_kCFNumberFormatterGroupingSeparator",
        HostConstant::NSString("kCFNumberFormatterGroupingSeparator"),
    ),
    (
        "_kCFNumberFormatterGroupingSize",
        HostConstant::NSString("kCFNumberFormatterGroupingSize"),
    ),
    (
        "_kCFNumberFormatterMaxFractionDigits",
        HostConstant::NSString("kCFNumberFormatterMaxFractionDigits"),
    ),
    (
        "_kCFNumberFormatterMinFractionDigits",
        HostConstant::NSString("kCFNumberFormatterMinFractionDigits"),
    ),
    (
        "_kCFNumberFormatterMinIntegerDigits",
        HostConstant::NSString("kCFNumberFormatterMinIntegerDigits"),
    ),
    (
        "_kCFNumberFormatterMinusSign",
        HostConstant::NSString("kCFNumberFormatterMinusSign"),
    ),
    (
        "_kCFNumberFormatterNegativePrefix",
        HostConstant::NSString("kCFNumberFormatterNegativePrefix"),
    ),
    (
        "_kCFNumberFormatterNegativeSuffix",
        HostConstant::NSString("kCFNumberFormatterNegativeSuffix"),
    ),
    (
        "_kCFNumberFormatterPositivePrefix",
        HostConstant::NSString("kCFNumberFormatterPositivePrefix"),
    ),
    (
        "_kCFNumberFormatterPositiveSuffix",
        HostConstant::NSString("kCFNumberFormatterPositiveSuffix"),
    ),
    (
        "_kCFNumberFormatterRoundingIncrement",
        HostConstant::NSString("kCFNumberFormatterRoundingIncrement"),
    ),
    (
        "_kCFNumberFormatterSecondaryGroupingSize",
        HostConstant::NSString("kCFNumberFormatterSecondaryGroupingSize"),
    ),
    (
        "_kCFNumberFormatterUseGroupingSeparator",
        HostConstant::NSString("kCFNumberFormatterUseGroupingSeparator"),
    ),
    (
        "_kCFNumberFormatterZeroSymbol",
        HostConstant::NSString("kCFNumberFormatterZeroSymbol"),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFNumberCreate(_, _, _)),
    export_c_func!(CFNumberGetValue(_, _, _)),
    export_c_func!(CFNumberCompare(_, _, _)),
    export_c_func!(CFNumberGetType(_)),
    export_c_func!(CFNumberGetByteSize(_)),
    export_c_func!(CFNumberIsFloatType(_)),
    export_c_func!(CFBooleanGetValue(_)),
];
