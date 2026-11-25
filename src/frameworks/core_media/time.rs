/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Exact time conversion and comparison. Contracts and rounding rules:
//! https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.8.sdk/System/Library/Frameworks/CoreMedia.framework/Versions/A/Headers/CMTime.h

use super::{write_cm_time, CMTime};
use crate::abi::GuestArg;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::ns_string;
use crate::mem::{MutPtr, MutVoidPtr, SafeRead};
use crate::objc::id;
use crate::Environment;
use std::cmp::Ordering;

fn category(time: CMTime) -> u8 {
    let flags = time.flags();
    if flags & 1 == 0 {
        4
    } else if flags & 4 != 0 {
        3
    } else if flags & 16 != 0 {
        2
    } else if flags & 8 != 0 {
        0
    } else if time.timescale() <= 0 {
        4
    } else {
        1
    }
}

pub(super) fn compare(lhs: CMTime, rhs: CMTime) -> i32 {
    let a = category(lhs);
    let b = category(rhs);
    let order = a.cmp(&b).then_with(|| {
        if a != 1 {
            return Ordering::Equal;
        }
        lhs.epoch().cmp(&rhs.epoch()).then_with(|| {
            (lhs.value() as i128 * rhs.timescale() as i128)
                .cmp(&(rhs.value() as i128 * lhs.timescale() as i128))
        })
    });
    match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

pub(super) fn rounded_divide(value: i128, divisor: i128, method: u32, larger_scale: bool) -> i128 {
    let quotient = value / divisor;
    let remainder = value % divisor;
    if remainder == 0 {
        return quotient;
    }
    let sign = value.signum();
    match method {
        1 => {
            quotient
                + if remainder.abs() * 2 >= divisor {
                    sign
                } else {
                    0
                }
        }
        2 => quotient,
        3 => quotient + sign,
        4 => {
            if larger_scale {
                quotient + sign
            } else if quotient == 0 && sign < 0 {
                -1
            } else {
                quotient
            }
        }
        5 => quotient + i128::from(sign > 0),
        6 => quotient - i128::from(sign < 0),
        _ => quotient,
    }
}

pub(super) fn convert(time: CMTime, timescale: i32, method: u32) -> CMTime {
    if time.flags() & 1 == 0 || time.flags() & 0x1c != 0 {
        return time;
    }
    if time.timescale() <= 0 || timescale <= 0 || !(1..=6).contains(&method) {
        return CMTime::default();
    }
    let numerator = time.value() as i128 * timescale as i128;
    let denominator = time.timescale() as i128;
    let value = rounded_divide(numerator, denominator, method, timescale > time.timescale());
    let rounded = if numerator % denominator == 0 { 0 } else { 2 };
    match i64::try_from(value) {
        Ok(value) => CMTime::from_parts(value, timescale, time.flags() | rounded, time.epoch()),
        Err(_) => CMTime::from_parts(0, 0, if value < 0 { 9 } else { 5 }, 0),
    }
}

pub(super) fn arithmetic(lhs: CMTime, rhs: CMTime, subtract: bool) -> CMTime {
    if lhs.flags() & 1 == 0 || rhs.flags() & 1 == 0 {
        return CMTime::default();
    }
    let left_infinity = if lhs.flags() & 4 != 0 {
        1
    } else if lhs.flags() & 8 != 0 {
        -1
    } else {
        0
    };
    let right_infinity = (if rhs.flags() & 4 != 0 {
        1
    } else if rhs.flags() & 8 != 0 {
        -1
    } else {
        0
    }) * if subtract { -1 } else { 1 };
    if left_infinity != 0 || right_infinity != 0 {
        if left_infinity != 0 && right_infinity != 0 && left_infinity != right_infinity {
            return CMTime::default();
        }
        let infinity = if left_infinity != 0 {
            left_infinity
        } else {
            right_infinity
        };
        return CMTime::from_parts(0, 0, if infinity < 0 { 9 } else { 5 }, 0);
    }
    if (lhs.flags() | rhs.flags()) & 16 != 0 {
        return CMTime::from_parts(0, 0, 17, 0);
    }
    if lhs.timescale() <= 0
        || rhs.timescale() <= 0
        || (lhs.epoch() != 0 && rhs.epoch() != 0 && lhs.epoch() != rhs.epoch())
    {
        return CMTime::default();
    }
    let epoch = if lhs.epoch() == rhs.epoch() {
        0
    } else if lhs.epoch() == 0 {
        rhs.epoch()
    } else {
        lhs.epoch()
    };
    let left_scale = lhs.timescale() as i64;
    let right_scale = rhs.timescale() as i64;
    let mut a = left_scale;
    let mut b = right_scale;
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    let common = left_scale / a * right_scale;
    let numerator = lhs.value() as i128 * (common / left_scale) as i128
        + rhs.value() as i128 * (common / right_scale) as i128 * if subtract { -1 } else { 1 };
    let mut scale = common.min(i32::MAX as i64);
    loop {
        let scaled = numerator * scale as i128;
        let value = rounded_divide(scaled, common as i128, 1, false);
        if let Ok(value) = i64::try_from(value) {
            let rounded = if scaled % common as i128 == 0 { 0 } else { 2 };
            return CMTime::from_parts(
                value,
                scale as i32,
                1 | ((lhs.flags() | rhs.flags()) & 2) | rounded,
                epoch,
            );
        }
        if scale == 1 {
            return CMTime::from_parts(0, 0, if value < 0 { 9 } else { 5 }, 0);
        }
        scale = (scale / 2).max(1);
    }
}

fn CMTimeConvertScale(
    env: &mut Environment,
    out: MutPtr<u8>,
    time: CMTime,
    timescale: i32,
    method: u32,
) -> MutPtr<u8> {
    write_cm_time(env, out, convert(time, timescale, method));
    out
}

fn CMTimeMakeWithEpoch(
    env: &mut Environment,
    out: MutPtr<u8>,
    value: i64,
    timescale: i32,
    epoch: i64,
) -> MutPtr<u8> {
    let time = if timescale > 0 {
        CMTime::from_parts(value, timescale, 1, epoch)
    } else {
        CMTime::default()
    };
    write_cm_time(env, out, time);
    out
}

fn CMTimeCopyDescription(env: &mut Environment, _allocator: MutVoidPtr, time: CMTime) -> id {
    let description = match category(time) {
        0 => "{-infinity}".to_owned(),
        2 => "{indefinite}".to_owned(),
        3 => "{+infinity}".to_owned(),
        4 => "{invalid}".to_owned(),
        _ => format!(
            "{{{}/{} = {:.3}}}",
            time.value(),
            time.timescale(),
            time.as_seconds()
        ),
    };
    ns_string::from_rust_string(env, description)
}

#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
struct CMTimeRange {
    start: CMTime,
    duration: CMTime,
}
unsafe impl SafeRead for CMTimeRange {}
impl GuestArg for CMTimeRange {
    const REG_COUNT: usize = 12;
    fn from_regs(regs: &[u32]) -> Self {
        Self {
            start: CMTime::from_regs(&regs[..6]),
            duration: CMTime::from_regs(&regs[6..12]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        self.start.to_regs(&mut regs[..6]);
        self.duration.to_regs(&mut regs[6..12]);
    }
}

fn CMTimeRangeContainsTime(_env: &mut Environment, range: CMTimeRange, time: CMTime) -> bool {
    let start = range.start;
    let duration = range.duration;
    if category(start) != 1
        || category(duration) != 1
        || category(time) != 1
        || duration.value() < 0
        || duration.epoch() != 0
        || start.epoch() != time.epoch()
    {
        return false;
    }
    // Compare time-start with duration without rounding or overflow.
    let elapsed = time.value() as i128 * start.timescale() as i128
        - start.value() as i128 * time.timescale() as i128;
    let scale = start.timescale() as i128 * time.timescale() as i128;
    elapsed >= 0 && elapsed * (duration.timescale() as i128) < duration.value() as i128 * scale
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CMTimeConvertScale(_, _, _, _)),
    export_c_func!(CMTimeMakeWithEpoch(_, _, _, _)),
    export_c_func!(CMTimeCopyDescription(_, _)),
    export_c_func!(CMTimeRangeContainsTime(_, _)),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compare_large_values_epochs_and_special_times() {
        let a = CMTime::from_parts(i64::MAX - 1, 10, 1, 0);
        let b = CMTime::from_parts(i64::MAX, 10, 1, 0);
        assert_eq!(compare(a, b), -1);
        assert_eq!(
            compare(
                CMTime::from_parts(-100, 1, 1, 2),
                CMTime::from_parts(100, 1, 1, 1)
            ),
            1
        );
        let values = [
            CMTime::from_parts(0, 0, 9, 0),
            CMTime::from_seconds(0),
            CMTime::from_parts(0, 0, 17, 0),
            CMTime::from_parts(0, 0, 5, 0),
            CMTime::default(),
        ];
        for pair in values.windows(2) {
            assert_eq!(compare(pair[0], pair[1]), -1);
        }
    }
    #[test]
    fn signed_conversion_preserves_epoch_and_rounding() {
        let time = CMTime::from_parts(-3, 2, 1, 7);
        let expected = [-2, -1, -2, -1, -1, -2];
        for (index, value) in expected.into_iter().enumerate() {
            let result = convert(time, 1, index as u32 + 1);
            assert_eq!(result.value(), value);
            assert_eq!(result.epoch(), 7);
            assert_eq!(result.flags(), 3);
        }
        assert_eq!(convert(CMTime::from_parts(-1, 10, 1, 0), 1, 4).value(), -1);
    }
}
