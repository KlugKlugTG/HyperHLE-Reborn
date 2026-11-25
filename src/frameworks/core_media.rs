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

mod time;
pub(crate) use time::FUNCTIONS as TIME_FUNCTIONS;

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
    env.mem
        .alloc_and_write(CMTime::from_seconds(0))
        .cast()
        .cast_const()
}

fn cm_time_invalid(env: &mut Environment) -> ConstVoidPtr {
    env.mem
        .alloc_and_write(CMTime::default())
        .cast()
        .cast_const()
}

fn cm_time_indefinite(env: &mut Environment) -> ConstVoidPtr {
    env.mem
        .alloc_and_write(CMTime::from_parts(0, 0, 0x11, 0))
        .cast()
        .cast_const()
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
    let time = if timescale > 0 {
        CMTime::from_parts(value, timescale, 1, 0)
    } else {
        CMTime::default()
    };
    write_cm_time(env, out, time);
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


fn cm_time_arithmetic(
    env: &mut Environment,
    out: MutPtr<u8>,
    lhs: CMTime,
    rhs: CMTime,
    subtract: bool,
) -> MutPtr<u8> {
    write_cm_time(env, out, time::arithmetic(lhs, rhs, subtract));
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
    time::compare(lhs, rhs)
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
    cm_time_arithmetic(env, out + 24u32, end, start, true);
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
    export_c_func!(CMSampleBufferSetDataBufferFromAudioBufferList(
        _,
        _,
        _,
        _,
        _
    )),
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
