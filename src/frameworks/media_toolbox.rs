/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, you can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! MediaToolbox framework registration and audio-processing tap support.
//!
//! MediaToolbox's public audio tap API is used by AVFoundation to insert a
//! processing callback into an audio mix. The framework handle is registered
//! here so apps that link it directly resolve through the normal dyld path.
//! The opaque tap API is exposed only when its callback ABI is available to
//! the guest; no fake function pointers are installed for unknown symbols.

use crate::abi::{CallFromHost, GuestArg, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::audio_toolbox::audio_unit::AudioBufferList;
use crate::frameworks::carbon_core::OSStatus;
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Default)]
pub struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}
unsafe impl SafeRead for CMTime {}
crate::abi::impl_GuestRet_for_large_struct!(CMTime);

impl CMTime {
    /// Build a `CMTime` from its four raw fields. Used by the CoreMedia
    /// arithmetic helpers, which need to preserve the flags/epoch of the
    /// operands.
    pub fn from_parts(value: i64, timescale: i32, flags: u32, epoch: i64) -> Self {
        Self {
            value,
            timescale,
            flags,
            epoch,
        }
    }

    pub fn value(self) -> i64 {
        self.value
    }

    pub fn timescale(self) -> i32 {
        self.timescale
    }

    pub fn flags(self) -> u32 {
        self.flags
    }

    pub fn epoch(self) -> i64 {
        self.epoch
    }

    pub const fn from_seconds(value: i64) -> Self {
        Self {
            value,
            timescale: 1,
            flags: 1,
            epoch: 0,
        }
    }

    pub fn from_seconds_f64(value: f64) -> Self {
        Self::from_seconds_with_timescale(value, 1_000_000)
    }

    pub fn from_seconds_with_timescale(value: f64, preferred_timescale: i32) -> Self {
        if value.is_nan() || preferred_timescale <= 0 {
            return Self::default();
        }
        if value.is_infinite() {
            return Self::from_parts(0, 0, if value < 0.0 { 9 } else { 5 }, 0);
        }
        let mut timescale = preferred_timescale;
        loop {
            let scaled = (value * f64::from(timescale)).round();
            if scaled.is_finite()
                && scaled >= i64::MIN as f64
                && scaled < 9_223_372_036_854_775_808.0
            {
                return Self {
                    value: scaled as i64,
                    timescale,
                    flags: if (scaled / f64::from(timescale)) == value {
                        1
                    } else {
                        3
                    },
                    epoch: 0,
                };
            }
            if timescale == 1 {
                return Self::from_parts(0, 0, if value < 0.0 { 9 } else { 5 }, 0);
            }
            timescale = (timescale / 2).max(1);
        }
    }

    pub fn as_seconds(self) -> f64 {
        let value = self.value;
        let timescale = self.timescale;
        let flags = self.flags;
        if flags & 0x1 == 0 || flags & 0x10 != 0 {
            f64::NAN
        } else if flags & 0x4 != 0 {
            f64::INFINITY
        } else if flags & 0x8 != 0 {
            f64::NEG_INFINITY
        } else if timescale == 0 {
            f64::NAN
        } else {
            value as f64 / timescale as f64
        }
    }
}

impl GuestArg for CMTime {
    const REG_COUNT: usize = 6;

    fn from_regs(regs: &[u32]) -> Self {
        Self {
            value: <i64 as GuestArg>::from_regs(&regs[0..2]),
            timescale: <i32 as GuestArg>::from_regs(&regs[2..3]),
            flags: <u32 as GuestArg>::from_regs(&regs[3..4]),
            epoch: <i64 as GuestArg>::from_regs(&regs[4..6]),
        }
    }

    fn to_regs(self, regs: &mut [u32]) {
        let value = self.value;
        let timescale = self.timescale;
        let flags = self.flags;
        let epoch = self.epoch;
        <i64 as GuestArg>::to_regs(value, &mut regs[0..2]);
        <i32 as GuestArg>::to_regs(timescale, &mut regs[2..3]);
        <u32 as GuestArg>::to_regs(flags, &mut regs[3..4]);
        <i64 as GuestArg>::to_regs(epoch, &mut regs[4..6]);
    }
}

#[cfg(test)]
mod cmtime_tests {
    use super::{CMTime, GuestArg};

    #[test]
    fn cm_time_seconds_and_flags() {
        assert_eq!(CMTime::from_seconds(7).as_seconds(), 7.0);
        assert_eq!(CMTime::from_seconds_f64(32.75).as_seconds(), 32.75);
        assert_eq!(
            CMTime::from_seconds_with_timescale(2.5, 600).as_seconds(),
            2.5
        );
        assert!(CMTime::from_seconds_with_timescale(2.0, 0)
            .as_seconds()
            .is_nan());
        assert!(CMTime::from_seconds_f64(f64::NAN).as_seconds().is_nan());
        assert!(CMTime::default().as_seconds().is_nan());
        assert!(CMTime {
            value: 0,
            timescale: 0,
            flags: 0x11,
            epoch: 0,
        }
        .as_seconds()
        .is_nan());
        assert_eq!(
            CMTime {
                value: 0,
                timescale: 0,
                flags: 0x5,
                epoch: 0,
            }
            .as_seconds(),
            f64::INFINITY
        );
        assert_eq!(
            CMTime {
                value: 0,
                timescale: 0,
                flags: 0x9,
                epoch: 0,
            }
            .as_seconds(),
            f64::NEG_INFINITY
        );
    }

    #[test]
    fn cm_time_guest_argument_round_trip() {
        let time = CMTime::from_seconds(7);
        let mut registers = [0; 6];
        time.to_regs(&mut registers);
        assert_eq!(CMTime::from_regs(&registers).as_seconds(), 7.0);
    }
}

#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct CMTimeRange {
    start: CMTime,
    duration: CMTime,
}
unsafe impl SafeRead for CMTimeRange {}
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;
use std::collections::HashMap;

pub type MTAudioProcessingTapRef = MutPtr<OpaqueMTAudioProcessingTap>;

#[repr(C, packed)]
pub struct OpaqueMTAudioProcessingTap {
    _marker: u8,
}
unsafe impl SafeRead for OpaqueMTAudioProcessingTap {}

#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct MTAudioProcessingTapCallbacks {
    pub version: i32,
    pub client_info: MutVoidPtr,
    pub init: GuestFunction,
    pub finalize: GuestFunction,
    pub prepare: GuestFunction,
    pub unprepare: GuestFunction,
    pub process: GuestFunction,
}
unsafe impl SafeRead for MTAudioProcessingTapCallbacks {}

struct TapState {
    callbacks: MTAudioProcessingTapCallbacks,
    storage: MutVoidPtr,
    prepared: bool,
}

#[derive(Default)]
pub struct State {
    taps: HashMap<u32, TapState>,
}

const PRE_EFFECTS: u32 = 1;
const POST_EFFECTS: u32 = 2;
const PARAM_ERR: OSStatus = -50;

fn state(env: &mut Environment) -> &mut State {
    &mut env.framework_state.media_toolbox
}

fn tap_id(tap: MTAudioProcessingTapRef) -> u32 {
    tap.to_bits()
}

fn callback_is_null(callback: GuestFunction) -> bool {
    callback.addr_with_thumb_bit() == 0
}

fn create_tap(
    env: &mut Environment,
    callbacks: MutPtr<MTAudioProcessingTapCallbacks>,
    flags: u32,
    out: MutPtr<MTAudioProcessingTapRef>,
) -> OSStatus {
    if callbacks.is_null() || out.is_null() || flags != PRE_EFFECTS && flags != POST_EFFECTS {
        return PARAM_ERR;
    }
    let callbacks = env.mem.read(callbacks);
    if callbacks.version != 0 || callback_is_null(callbacks.process) {
        return PARAM_ERR;
    }
    let tap = env.mem.alloc(1u32).cast::<OpaqueMTAudioProcessingTap>();
    let id = tap_id(tap);
    let init = callbacks.init;
    let client_info = callbacks.client_info;
    state(env).taps.insert(
        id,
        TapState {
            callbacks,
            storage: MutVoidPtr::null(),
            prepared: false,
        },
    );
    env.mem.write(out, tap);
    if !callback_is_null(init) {
        let storage = env.mem.alloc(0u32).cast_void();
        let _: () = init.call_from_host(env, (tap, client_info, storage));
        state(env).taps.get_mut(&id).unwrap().storage = storage;
    }
    0
}

fn get_storage(env: &mut Environment, tap: MTAudioProcessingTapRef) -> MutVoidPtr {
    state(env)
        .taps
        .get(&tap_id(tap))
        .map(|t| t.storage)
        .unwrap_or(MutVoidPtr::null())
}

fn prepare_tap(
    env: &mut Environment,
    tap: MTAudioProcessingTapRef,
    max_frames: i32,
    format: ConstVoidPtr,
) {
    let id = tap_id(tap);
    let callback = state(env)
        .taps
        .get(&id)
        .map(|entry| entry.callbacks.prepare);
    if let Some(callback) = callback {
        if !callback_is_null(callback) {
            let _: () = <GuestFunction as CallFromHost<
                (),
                (MTAudioProcessingTapRef, i32, ConstVoidPtr),
            >>::call_from_host(&callback, env, (tap, max_frames, format));
        }
    }
    if let Some(entry) = state(env).taps.get_mut(&id) {
        entry.prepared = true;
    }
}

fn unprepare_tap(env: &mut Environment, tap: MTAudioProcessingTapRef) {
    let id = tap_id(tap);
    let callback = state(env)
        .taps
        .get(&id)
        .map(|entry| (entry.callbacks.unprepare, entry.prepared));
    if let Some((callback, prepared)) = callback {
        if prepared && !callback_is_null(callback) {
            let _: () =
                <GuestFunction as CallFromHost<(), (MTAudioProcessingTapRef,)>>::call_from_host(
                    &callback,
                    env,
                    (tap,),
                );
        }
    }
    if let Some(entry) = state(env).taps.get_mut(&id) {
        entry.prepared = false;
    }
}

fn destroy_tap(env: &mut Environment, tap: MTAudioProcessingTapRef) {
    let Some(entry) = state(env).taps.remove(&tap_id(tap)) else {
        return;
    };
    let unprepare = entry.callbacks.unprepare;
    let finalize = entry.callbacks.finalize;
    if entry.prepared && !callback_is_null(unprepare) {
        let _: () = <GuestFunction as CallFromHost<(), (MTAudioProcessingTapRef,)>>::call_from_host(
            &unprepare,
            env,
            (tap,),
        );
    }
    if !callback_is_null(finalize) {
        let _: () = <GuestFunction as CallFromHost<(), (MTAudioProcessingTapRef,)>>::call_from_host(
            &finalize,
            env,
            (tap,),
        );
    }
}

fn get_source_audio(
    env: &mut Environment,
    tap: MTAudioProcessingTapRef,
    number_frames: i32,
    buffer_list: MutPtr<AudioBufferList<1>>,
    flags_out: MutPtr<u32>,
    time_range_out: MutPtr<CMTimeRange>,
    frames_out: MutPtr<i32>,
) -> OSStatus {
    let Some(entry) = state(env).taps.get(&tap_id(tap)) else {
        return PARAM_ERR;
    };
    if callback_is_null(entry.callbacks.process) || buffer_list.is_null() {
        return PARAM_ERR;
    }
    if !flags_out.is_null() {
        env.mem.write(flags_out, 0);
    }
    if !time_range_out.is_null() {
        env.mem.write(time_range_out, CMTimeRange::default());
    }
    if !frames_out.is_null() {
        env.mem.write(frames_out, number_frames);
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(MTAudioProcessingTapGetTypeID()),
    export_c_func!(MTAudioProcessingTapCreate(_, _, _, _)),
    export_c_func!(MTAudioProcessingTapGetStorage(_)),
    export_c_func!(MTAudioProcessingTapGetSourceAudio(_, _, _, _, _, _)),
];

fn MTAudioProcessingTapGetTypeID(_env: &mut Environment) -> u32 {
    0x54415001
}
fn MTAudioProcessingTapCreate(
    env: &mut Environment,
    allocator: ConstVoidPtr,
    callbacks: MutPtr<MTAudioProcessingTapCallbacks>,
    flags: u32,
    out: MutPtr<MTAudioProcessingTapRef>,
) -> OSStatus {
    let _ = allocator;
    create_tap(env, callbacks, flags, out)
}
fn MTAudioProcessingTapGetStorage(
    env: &mut Environment,
    tap: MTAudioProcessingTapRef,
) -> MutVoidPtr {
    get_storage(env, tap)
}
fn MTAudioProcessingTapGetSourceAudio(
    env: &mut Environment,
    tap: MTAudioProcessingTapRef,
    frames: i32,
    buffers: MutPtr<AudioBufferList<1>>,
    flags: MutPtr<u32>,
    range: MutPtr<CMTimeRange>,
    out: MutPtr<i32>,
) -> OSStatus {
    get_source_audio(env, tap, frames, buffers, flags, range, out)
}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/MediaToolbox.framework/MediaToolbox",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};
