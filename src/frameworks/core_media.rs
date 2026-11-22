/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `CoreMedia.framework/CoreMedia`.
//!
//! On iOS, CoreMedia provides time-related types (`CMTime`, `CMTimeRange`),
//! sample buffer plumbing (`CMSampleBufferRef`), and format descriptions used
//! mostly by AVFoundation. Apps that link against CoreMedia (directly or
//! transitively, e.g. via AVFoundation cutscene playback) put the path
//! `/System/Library/Frameworks/CoreMedia.framework/CoreMedia` in their Mach-O
//! load commands.
//!
//! Without a [crate::dyld::HostDylib] entry for that path, touchHLE prints a
//! `Warning: app binary depends on unimplemented or missing dylib
//! "/System/Library/Frameworks/CoreMedia.framework/CoreMedia"` at startup,
//! which can spook users into reporting otherwise-fine apps as broken (e.g.
//! HyperHLE appdb report #22, GhostToasters).
//!
//! This stub exists so that the dependency is recognized and the warning is
//! suppressed. The few CoreMedia functions touchHLE currently implements
//! (`CMSampleBufferGetImageBuffer`, `CMSampleBufferDataIsReady`, …) are
//! registered with [crate::frameworks::core_video] for historical reasons;
//! `dyld` searches all framework `function_exports` regardless of which
//! dylib they were declared under, so the binding still resolves correctly
//! whether the app links CoreMedia or CoreVideo.

use crate::abi::{GuestArg, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::media_toolbox::CMTime;
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::Environment;

/// `CMTime` is a 24-byte struct (`int64_t value`, `int32_t timescale`,
/// `uint32_t flags`, `int64_t epoch`). `kCMTimeZero` is the all-zero
/// constant — `{value:0, timescale:0, flags:0, epoch:0}`. Apps reach the
/// symbol by Mach-O lookup and either compare against it for equality
/// or copy it as a starting point. Allocate the layout once per access
/// site; CFTime's other "well-known" constants (kCMTimeInvalid,
/// kCMTimePositiveInfinity, …) fit the same template, so we expose
/// each as a separate slot.
fn cm_time_zero(env: &mut Environment) -> ConstVoidPtr {
    let p: MutPtr<u8> = env.mem.alloc(24).cast();
    for i in 0..24 {
        env.mem.write(p + i, 0);
    }
    p.cast().cast_const()
}

/// `kCMTimeInvalid` has flags=0 (kCMTimeFlags_Valid bit clear). Same
/// 24-byte layout as kCMTimeZero, every byte zero.
fn cm_time_invalid(env: &mut Environment) -> ConstVoidPtr {
    cm_time_zero(env)
}

/// `kCMTimeIndefinite`: value=0, timescale=0, flags=kCMTimeFlags_Valid|
/// kCMTimeFlags_Indefinite (=0x3), epoch=0. Apps usually only check for
/// (flags & kCMTimeFlags_Valid) and ignore the rest, so a flags-only
/// difference from kCMTimeZero is enough.
fn cm_time_indefinite(env: &mut Environment) -> ConstVoidPtr {
    let p: MutPtr<u8> = env.mem.alloc(24).cast();
    for i in 0..24 {
        env.mem.write(p + i, 0);
    }
    // Bytes 12..16 are the `flags` field (after value:i64, timescale:i32).
    env.mem.write(p + 12u32, 0x03);
    p.cast().cast_const()
}

/// `kCMTimePositiveInfinity` and `kCMTimeNegativeInfinity` use flags
/// `kCMTimeFlags_Valid|kCMTimeFlags_PositiveInfinity` (0x5) and
/// `kCMTimeFlags_Valid|kCMTimeFlags_NegativeInfinity` (0x9) respectively.
fn cm_time_positive_infinity(env: &mut Environment) -> ConstVoidPtr {
    let p: MutPtr<u8> = env.mem.alloc(24).cast();
    for i in 0..24 {
        env.mem.write(p + i, 0);
    }
    env.mem.write(p + 12u32, 0x05);
    p.cast().cast_const()
}

fn cm_time_negative_infinity(env: &mut Environment) -> ConstVoidPtr {
    let p: MutPtr<u8> = env.mem.alloc(24).cast();
    for i in 0..24 {
        env.mem.write(p + i, 0);
    }
    env.mem.write(p + 12u32, 0x09);
    p.cast().cast_const()
}

/// `kCMTimingInfoInvalid` is a 56-byte `CMSampleTimingInfo` struct
/// (3 × `CMTime` = 72 bytes — actually 72, since each CMTime is 24).
/// Apple `CMSampleBuffer.h` defines all three CMTime fields as
/// `kCMTimeInvalid`, i.e. every byte zero. Apps memcpy this whole
/// struct as a starting point or compare individual fields to
/// `kCMTimeInvalid`. The published constant is an `extern const`.
/// <https://developer.apple.com/documentation/coremedia/kcmtiminginfo_invalid>
fn cm_timing_info_invalid(env: &mut Environment) -> ConstVoidPtr {
    let p: MutPtr<u8> = env.mem.alloc(72).cast();
    for i in 0..72 {
        env.mem.write(p + i, 0);
    }
    p.cast().cast_const()
}

pub const CONSTANTS: ConstantExports = &[
    ("_kCMTimeZero", HostConstant::Custom(cm_time_zero)),
    ("_kCMTimeInvalid", HostConstant::Custom(cm_time_invalid)),
    (
        "_kCMTimeIndefinite",
        HostConstant::Custom(cm_time_indefinite),
    ),
    (
        "_kCMTimePositiveInfinity",
        HostConstant::Custom(cm_time_positive_infinity),
    ),
    (
        "_kCMTimeNegativeInfinity",
        HostConstant::Custom(cm_time_negative_infinity),
    ),
    (
        "_kCMTimingInfoInvalid",
        HostConstant::Custom(cm_timing_info_invalid),
    ),
    // -----------------------------------------------------------------
    // `kCMSampleAttachmentKey_*` — CFStringRef keys used with
    // `CMSampleBufferGetSampleAttachmentsArray()`. Apple ships them as
    // `extern const CFStringRef`; their literal values are the
    // canonical CMSampleBuffer.h tags. Apps reach them through string
    // identity / `CFEqual()` comparisons.
    // <https://developer.apple.com/documentation/coremedia/kcmsampleattachmentkey_notsync>
    // -----------------------------------------------------------------
    (
        "_kCMSampleAttachmentKey_NotSync",
        HostConstant::NSString("NotSync"),
    ),
    (
        "_kCMSampleAttachmentKey_PartialSync",
        HostConstant::NSString("PartialSync"),
    ),
    (
        "_kCMSampleAttachmentKey_HasRedundantCoding",
        HostConstant::NSString("HasRedundantCoding"),
    ),
    (
        "_kCMSampleAttachmentKey_IsDependedOnByOthers",
        HostConstant::NSString("IsDependedOnByOthers"),
    ),
    (
        "_kCMSampleAttachmentKey_DependsOnOthers",
        HostConstant::NSString("DependsOnOthers"),
    ),
    (
        "_kCMSampleAttachmentKey_EarlierDisplayTimesAllowed",
        HostConstant::NSString("EarlierDisplayTimesAllowed"),
    ),
    (
        "_kCMSampleAttachmentKey_DisplayImmediately",
        HostConstant::NSString("DisplayImmediately"),
    ),
    (
        "_kCMSampleAttachmentKey_DoNotDisplay",
        HostConstant::NSString("DoNotDisplay"),
    ),
];

/// `CMTimeMake(int64_t value, int32_t timescale)` returns a `CMTime` with
/// `kCMTimeFlags_Valid` set, `epoch == 0` and the supplied value /
/// timescale. On 32-bit ARM iOS the 24-byte struct is returned via a
/// hidden first sret pointer (the AAPCS "indirect result location"
/// register r0); the caller supplies a pointer to writable storage,
/// and the C-visible arguments shift one slot to the right. Apple
/// inlined this helper in `<CoreMedia/CMTime.h>` before iOS 7.1 and
/// began exporting an out-of-line `_CMTimeMake` symbol shortly after,
/// which is the symbol the touchHLE-targeted apps reference here.
/// <https://developer.apple.com/documentation/coremedia/cmtimemake(_:_:)>
fn CMTimeMake(env: &mut Environment, out: MutPtr<u8>, value: i64, timescale: i32) -> MutPtr<u8> {
    // Apple ARM 32-bit ABI lays out `CMTime` as:
    //   offset  0: int64_t  value      (8 bytes, naturally aligned)
    //   offset  8: int32_t  timescale  (4 bytes)
    //   offset 12: uint32_t flags      (4 bytes; bit 0 = kCMTimeFlags_Valid)
    //   offset 16: int64_t  epoch      (8 bytes)  → total = 24 bytes
    // <https://developer.apple.com/documentation/coremedia/cmtime>
    let value_bytes = value.to_le_bytes();
    for (i, b) in value_bytes.iter().enumerate() {
        env.mem.write(out + (i as u32), *b);
    }
    let timescale_bytes = timescale.to_le_bytes();
    for (i, b) in timescale_bytes.iter().enumerate() {
        env.mem.write(out + 8u32 + (i as u32), *b);
    }
    // `flags = kCMTimeFlags_Valid` (0x1).
    env.mem.write(out + 12u32, 0x01);
    env.mem.write(out + 13u32, 0x00);
    env.mem.write(out + 14u32, 0x00);
    env.mem.write(out + 15u32, 0x00);
    // `epoch = 0` — zero out bytes 16..24.
    for i in 16..24u32 {
        env.mem.write(out + i, 0);
    }
    out
}

fn CMTimeMakeWithSeconds(
    env: &mut Environment,
    out: MutPtr<u8>,
    seconds: f64,
    preferred_timescale: i32,
) -> MutPtr<u8> {
    let time = CMTime::from_seconds_with_timescale(seconds, preferred_timescale);
    let mut words = [0; 6];
    time.to_regs(&mut words);
    for (word_index, word) in words.into_iter().enumerate() {
        for (byte_index, byte) in word.to_le_bytes().into_iter().enumerate() {
            env.mem
                .write(out + (word_index * 4 + byte_index) as u32, byte);
        }
    }
    out
}

fn CMTimeGetSeconds(_env: &mut Environment, time: CMTime) -> f64 {
    time.as_seconds()
}

const CM_TIME_FLAGS_VALID: u32 = 0x1;

fn cm_time_is_valid(time: CMTime) -> bool {
    time.timescale() > 0 && (time.flags() & CM_TIME_FLAGS_VALID) != 0
}

fn write_cm_time(env: &mut Environment, out: MutPtr<u8>, time: CMTime) {
    let mut words = [0u32; 6];
    time.to_regs(&mut words);
    for (word_index, word) in words.into_iter().enumerate() {
        for (byte_index, byte) in word.to_le_bytes().into_iter().enumerate() {
            env.mem
                .write(out + (word_index * 4 + byte_index) as u32, byte);
        }
    }
}

fn write_invalid_cm_time(env: &mut Environment, out: MutPtr<u8>) {
    write_cm_time(env, out, CMTime::default());
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.abs().max(1)
}

/// Adds or subtracts two `CMTime` values exactly in their least common
/// timescale. Apple's `CMTimeAdd`/`CMTimeSubtract` return an invalid time
/// when either operand is invalid or the result would overflow.
/// <https://developer.apple.com/documentation/coremedia/cmtimeadd(_:_:)>
fn cm_time_arithmetic(
    env: &mut Environment,
    out: MutPtr<u8>,
    lhs: CMTime,
    rhs: CMTime,
    subtract: bool,
) -> MutPtr<u8> {
    if !cm_time_is_valid(lhs) || !cm_time_is_valid(rhs) {
        write_invalid_cm_time(env, out);
        return out;
    }
    let l_scale = lhs.timescale() as i64;
    let r_scale = rhs.timescale() as i64;
    let common = l_scale / gcd(l_scale, r_scale) * r_scale;
    if common <= 0 || common > i32::MAX as i64 {
        write_invalid_cm_time(env, out);
        return out;
    }
    let l_value = lhs.value() as i128 * (common / l_scale) as i128;
    let r_value = rhs.value() as i128 * (common / r_scale) as i128;
    let value = if subtract {
        l_value - r_value
    } else {
        l_value + r_value
    };
    let value = match i64::try_from(value) {
        Ok(value) => value,
        Err(_) => {
            write_invalid_cm_time(env, out);
            return out;
        }
    };
    let result = CMTime::from_parts(value, common as i32, CM_TIME_FLAGS_VALID, 0);
    write_cm_time(env, out, result);
    out
}

fn CMTimeAdd(env: &mut Environment, out: MutPtr<u8>, lhs: CMTime, rhs: CMTime) -> MutPtr<u8> {
    cm_time_arithmetic(env, out, lhs, rhs, false)
}

fn CMTimeSubtract(env: &mut Environment, out: MutPtr<u8>, lhs: CMTime, rhs: CMTime) -> MutPtr<u8> {
    cm_time_arithmetic(env, out, lhs, rhs, true)
}

/// `int32_t CMTimeCompare(CMTime time1, CMTime time2)`
/// <https://developer.apple.com/documentation/coremedia/cmtimesetisfinite>
fn CMTimeCompare(_env: &mut Environment, lhs: CMTime, rhs: CMTime) -> i32 {
    let a = lhs.as_seconds();
    let b = rhs.as_seconds();
    // Apple documents an ordering only for valid, finite times. Treat the
    // NaN/indefinite cases as equal so callers that sort or de-duplicate
    // timestamps do not get an undefined comparison.
    if a.is_nan() || b.is_nan() {
        return 0;
    }
    if a < b {
        -1
    } else if a > b {
        1
    } else {
        0
    }
}

/// `CMTimeRange CMTimeRangeMake(CMTime start, CMTime duration)` — the
/// 48-byte struct is returned through the caller's hidden sret pointer.
/// <https://developer.apple.com/documentation/coremedia/cmtimerangemake(_:_:)>
fn CMTimeRangeMake(
    env: &mut Environment,
    out: MutPtr<u8>,
    start: CMTime,
    duration: CMTime,
) -> MutPtr<u8> {
    write_cm_time(env, out, start);
    write_cm_time(env, out + 24u32, duration);
    out
}

/// `CMTimeRange CMTimeRangeFromTimeToTime(CMTime start, CMTime end)`.
/// <https://developer.apple.com/documentation/coremedia/cmtimerangefromtimetotime(_:_:)>
fn CMTimeRangeFromTimeToTime(
    env: &mut Environment,
    out: MutPtr<u8>,
    start: CMTime,
    end: CMTime,
) -> MutPtr<u8> {
    write_cm_time(env, out, start);
    let mut duration_out = [0u8; 24];
    // Reuse the arithmetic path by writing into a stack-like guest buffer.
    // `MutPtr` has no stack storage, so allocate a tiny scratch block.
    let scratch: MutPtr<u8> = env.mem.alloc(24).cast();
    cm_time_arithmetic(env, scratch, end, start, true);
    for i in 0..24u32 {
        duration_out[i as usize] = env.mem.read(scratch + i);
    }
    env.mem.free(scratch.cast());
    for (i, byte) in duration_out.into_iter().enumerate() {
        env.mem.write(out + 24u32 + i as u32, byte);
    }
    out
}

/// Sample-buffer creation is not backed by a real media pipeline, but apps
/// (and AVFoundation's own shims) only need a stable non-NULL handle that the
/// other CMSampleBuffer stubs accept. Allocate a small guest block per call.
fn CMSampleBufferCreate(
    env: &mut Environment,
    _allocator: MutVoidPtr,
    _data_buffer: MutVoidPtr,
    _data_ready: u32,
    _make_data_ready_callback: GuestFunction,
    _make_data_ready_refcon: MutVoidPtr,
    _format_description: MutVoidPtr,
    _num_samples: i32,
    _num_sample_timing_entries: i32,
    _sample_timing_array: ConstVoidPtr,
    _num_sample_size_entries: i32,
    _sample_size_array: ConstVoidPtr,
    sample_buffer_out: MutPtr<u32>,
) -> i32 {
    if !sample_buffer_out.is_null() {
        let handle: MutPtr<u8> = env.mem.alloc(16).cast();
        env.mem.write(sample_buffer_out, handle.to_bits());
    }
    0
}

fn CMSampleBufferSetDataReady(_env: &mut Environment, _sample_buffer: MutVoidPtr) -> i32 {
    0
}

fn CMSampleBufferSetDataBufferFromAudioBufferList(
    _env: &mut Environment,
    _sample_buffer: MutVoidPtr,
    _block_buffer_structure_allocator: MutVoidPtr,
    _block_buffer_block_allocator: MutVoidPtr,
    _flags: u32,
    _buffer_list: ConstVoidPtr,
) -> i32 {
    0
}

/// `OSStatus CMSampleBufferGetPresentationTimeStamp(CMTime *out,
///     CMSampleBufferRef sbuf)` — stubs report a valid zero timestamp, which
/// keeps AVFoundation's timeline code on the "no media yet" path.
fn CMSampleBufferGetPresentationTimeStamp(
    env: &mut Environment,
    out: MutPtr<u8>,
    _sample_buffer: MutVoidPtr,
) -> i32 {
    write_cm_time(env, out, CMTime::from_seconds(0));
    0
}

fn CMAudioFormatDescriptionCreate(
    env: &mut Environment,
    _allocator: MutVoidPtr,
    _asbd: ConstVoidPtr,
    _layout_size: u32,
    _layout: ConstVoidPtr,
    _magic_cookie_size: u32,
    _magic_cookie: ConstVoidPtr,
    _extensions: MutVoidPtr,
    format_description_out: MutPtr<u32>,
) -> i32 {
    if !format_description_out.is_null() {
        let handle: MutPtr<u8> = env.mem.alloc(16).cast();
        env.mem.write(format_description_out, handle.to_bits());
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CMTimeMake(_, _, _)),
    export_c_func!(CMTimeAdd(_, _, _)),
    export_c_func!(CMTimeSubtract(_, _, _)),
    export_c_func!(CMTimeCompare(_, _)),
    export_c_func!(CMTimeRangeMake(_, _, _)),
    export_c_func!(CMTimeRangeFromTimeToTime(_, _, _)),
    export_c_func!(CMSampleBufferCreate(_, _, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(CMSampleBufferSetDataReady(_)),
    export_c_func!(CMSampleBufferSetDataBufferFromAudioBufferList(_, _, _, _, _)),
    export_c_func!(CMSampleBufferGetPresentationTimeStamp(_, _)),
    export_c_func!(CMAudioFormatDescriptionCreate(_, _, _, _, _, _, _, _)),
    export_c_func!(CMTimeMakeWithSeconds(_, _, _)),
    export_c_func!(CMTimeGetSeconds(_)),
    export_c_func!(CMFormatDescriptionGetMediaType(_)),
];

/// `CMMediaType CMFormatDescriptionGetMediaType(CMFormatDescriptionRef desc)`
/// returns the media type FourCC ('vide', 'soun', 'muxed', ...) of a format
/// description (see Apple's CMFormatDescription.h). touchHLE never creates
/// real format descriptions (there is no CMSampleBuffer pipeline), but
/// ad-SDK video players call this while parsing decoded metadata. Any valid
/// pointer is assumed to describe a video track, which matches every guest
/// call site we have seen (they compare against `kCMMediaType_Video`).
/// NULL returns 0; the documentation defines no result for NULL and all
/// callers guard anyway.
const kCMMediaType_Video: u32 = 0x7669_6465; // 'vide'

fn CMFormatDescriptionGetMediaType(env: &mut Environment, desc: MutPtr<u8>) -> u32 {
    let _ = env;
    if desc.to_bits() == 0 {
        0
    } else {
        kCMMediaType_Video
    }
}
