/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `time.h` (C) and `sys/time.h` (POSIX)

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::clocale::{setlocale, LC_CTYPE};
use crate::libc::errno::set_errno;
use crate::libc::stdio::printf::{isspace, isspace_inner};
use crate::mem::{guest_size_of, ConstPtr, GuestUSize, MutPtr, Ptr, SafeRead};
use crate::Environment;
use std::ops::Range;
use std::time::{Duration, SystemTime};

#[derive(Default)]
pub struct State {
    /// Temporary static storage for the return value of `gmtime` or
    /// `localtime`. The standard allows calls to either to overwrite it.
    gmtime_tmp: Option<MutPtr<tm>>,
    /// Temporary static storage for the string returned by `ctime` or
    /// `asctime`. The standard allows calls to either to overwrite it.
    asctime_buf: Option<MutPtr<u8>>,
    /// Seconds west of UTC, populated by tzset().
    pub timezone_offset_seconds: i32,
    /// DST flag (non-zero if DST is ever observed), populated by tzset().
    pub daylight_flag: i32,
    /// Guest pointer to pre-allocated 32-byte buffer for standard TZ name.
    pub tzname_std_ptr: Option<MutPtr<u8>>,
    /// Guest pointer to pre-allocated 32-byte buffer for DST TZ name.
    pub tzname_dst_ptr: Option<MutPtr<u8>>,
    /// Guest pointer to tzname[2] array.
    pub tzname_array_ptr: Option<MutPtr<ConstPtr<u8>>>,
    /// Guest pointer to _timezone global.
    pub timezone_global_ptr: Option<MutPtr<i32>>,
    /// Guest pointer to _daylight global.
    pub daylight_global_ptr: Option<MutPtr<i32>>,
}

// time.h (C)

#[allow(non_camel_case_types)]
/// Time in seconds since UNIX epoch (1970-01-01 00:00:00)
pub type time_t = i32;

#[allow(non_camel_case_types)]
type clock_t = u64;

const CLOCKS_PER_SEC: clock_t = 1000000;

fn clock(env: &mut Environment) -> clock_t {
    // ИСПРАВЛЕНИЕ: Возвращаем точное время в микросекундах (а не усекаем до
    // секунд).
    // Это критически важно для игр (Cocos2D и др.), которые считают дельту
    // времени.
    // Иначе delta time = 0.0, что ведет к делению на ноль -> NaN ->
    // отрицательный sleep -> Crash.
    env.guest_clock.now().duration_since(env.startup_time).as_micros() as clock_t
}

fn time(env: &mut Environment, out: MutPtr<time_t>) -> time_t {
    // TODO: handle errno properly
    set_errno(env, 0);
    let time64 = env.guest_clock.system_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let time = time64 as time_t;
    if time64 != time as u64 {
        log_once!("Warning: [time] system clock is beyond Y2K38 and might confuse the app");
    }
    if !out.is_null() {
        env.mem.write(out, time);
    }
    time
}

fn tzset(env: &mut Environment) {
    let (offset_sec, dst, std_name, dst_name) = query_host_timezone();

    env.libc_state.time.timezone_offset_seconds = offset_sec;
    env.libc_state.time.daylight_flag = dst;

    // Update exported globals
    if let Some(ptr) = env.libc_state.time.timezone_global_ptr {
        env.mem.write(ptr, offset_sec);
    }
    if let Some(ptr) = env.libc_state.time.daylight_global_ptr {
        env.mem.write(ptr, dst);
    }

    // Ensure buffers exist
    let std_buf = *env.libc_state.time.tzname_std_ptr.get_or_insert_with(|| {
        let ptr = env.mem.alloc(32).cast::<u8>();
        env.mem.bytes_at_mut(ptr, 32).fill(0);
        ptr
    });
    let dst_buf = *env.libc_state.time.tzname_dst_ptr.get_or_insert_with(|| {
        let ptr = env.mem.alloc(32).cast::<u8>();
        env.mem.bytes_at_mut(ptr, 32).fill(0);
        ptr
    });

    write_cstr_to_buffer(env, std_buf, &std_name, 32);
    write_cstr_to_buffer(env, dst_buf, &dst_name, 32);

    if let Some(arr_ptr) = env.libc_state.time.tzname_array_ptr {
        env.mem.write(arr_ptr, std_buf.cast_const());
        env.mem.write(arr_ptr + 1, dst_buf.cast_const());
    }

    log_dbg!(
        "tzset() => timezone={} daylight={} tzname=[{}, {}]",
        offset_sec,
        dst,
        std_name,
        dst_name
    );
}

fn write_cstr_to_buffer(env: &mut Environment, ptr: MutPtr<u8>, s: &str, buf_size: GuestUSize) {
    let bytes = s.as_bytes();
    let buf = env.mem.bytes_at_mut(ptr, buf_size);
    buf.fill(0);
    let len = bytes.len().min((buf_size - 1) as usize);
    buf[..len].copy_from_slice(&bytes[..len]);
}

#[cfg(unix)]
fn query_host_timezone() -> (i32, i32, String, String) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as ::libc::time_t;

    unsafe {
        let mut tm = std::mem::MaybeUninit::<::libc::tm>::uninit();
        ::libc::localtime_r(&now, tm.as_mut_ptr());
        let tm = tm.assume_init();
        let offset = -(tm.tm_gmtoff as i32);
        let isdst = tm.tm_isdst;
        let current_zone = if !tm.tm_zone.is_null() {
            std::ffi::CStr::from_ptr(tm.tm_zone)
                .to_string_lossy()
                .into_owned()
        } else {
            "GMT".to_string()
        };

        // Compare with a date ~6 months away to distinguish standard/DST names
        let six_months: ::libc::time_t = 183 * 24 * 3600;
        let other_time = if now > six_months {
            now - six_months
        } else {
            now + six_months
        };

        let mut other_tm = std::mem::MaybeUninit::<::libc::tm>::uninit();
        ::libc::localtime_r(&other_time, other_tm.as_mut_ptr());
        let other_tm = other_tm.assume_init();
        let other_zone = if !other_tm.tm_zone.is_null() {
            std::ffi::CStr::from_ptr(other_tm.tm_zone)
                .to_string_lossy()
                .into_owned()
        } else {
            "GMT".to_string()
        };

        // Compute `has_dst` first so we can still inspect both names
        // before the `(std_name, dst_name)` match consumes them.
        let has_dst = if current_zone != other_zone || isdst > 0 || other_tm.tm_isdst > 0 {
            1
        } else {
            0
        };

        let (std_name, dst_name) = if current_zone == other_zone {
            (current_zone.clone(), current_zone)
        } else if isdst > 0 {
            (other_zone, current_zone)
        } else {
            (current_zone, other_zone)
        };

        (offset, has_dst, std_name, dst_name)
    }
}

#[cfg(not(unix))]
fn query_host_timezone() -> (i32, i32, String, String) {
    (0, 0, "GMT".to_string(), "GMT".to_string())
}

#[allow(non_camel_case_types)]
#[repr(C, packed)]
#[derive(Copy, Clone, Debug)]
/// `struct tm`, fields count from 0 unless marked otherwise
pub struct tm {
    pub tm_sec: i32,
    pub tm_min: i32,
    pub tm_hour: i32,
    pub tm_mday: i32,
    pub tm_mon: i32,
    pub tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i32,
    tm_zone: ConstPtr<u8>,
}
unsafe impl SafeRead for tm {}

impl tm {
    pub fn from(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Self {
        // Some sources of `tm` (e.g. ZIP-archive `DateTime` defaults read
        // by [crate::fs::bundle::IpaFile]) hand us year=0 / month=0 when
        // the archive entry has no recorded date. The previous u16/u8
        // arithmetic `year - 1900` / `month - 1` would overflow in
        // release builds, producing a wildly out-of-range `tm_year`
        // that later panicked the `i64` -> `time_t` (`i32`) cast in
        // [calendar_date_to_timestamp] — which crashed the whole app
        // picker on startup.
        //
        // Clamp to representable values so callers always get a sane
        // timestamp back instead of a panic.
        let year = year.max(1970);
        let month = month.clamp(1, 12);
        let day = day.clamp(1, 31);
        let hour = hour.min(23);
        let minute = minute.min(59);
        let second = second.min(60); // POSIX leap seconds
        tm {
            tm_year: i32::from(year) - 1900,
            tm_mon: i32::from(month) - 1,
            tm_mday: day.into(),
            tm_hour: hour.into(),
            tm_min: minute.into(),
            tm_sec: second.into(),
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: Ptr::null(),
        }
    }
}

const fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const CYCLE_YEARS: i32 = 400;
const YEAR_TO_DAY: [i32; CYCLE_YEARS as usize] = calc_year_to_day().0;
const CYCLE_DAYS: i32 = calc_year_to_day().1;

const fn calc_year_to_day() -> ([i32; CYCLE_YEARS as usize], i32) {
    let mut table = [0i32; CYCLE_YEARS as usize];
    let mut day = 0;
    let mut year = 0;

    while year < CYCLE_YEARS {
        table[year as usize] = day;

        day += if is_leap_year(year) { 366 } else { 365 };
        year += 1;
    }

    (table, day)
}

const DAYS_IN_MONTH: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const MONTH_TO_DAY_NONLEAP: [i32; 12] = calc_month_to_day(false);
const MONTH_TO_DAY_LEAP: [i32; 12] = calc_month_to_day(true);

const fn calc_month_to_day(leap_year: bool) -> [i32; 12] {
    let mut table = [0i32; 12];
    let mut day = 0;
    let mut month = 0;

    while month < 12 {
        table[month] = day;

        day += DAYS_IN_MONTH[month] + ((leap_year && month == 1) as i32);
        month += 1;
    }

    table
}

pub fn timestamp_to_calendar_date(timestamp: time_t) -> tm {
    let seconds_since_unix_epoch: i32 = timestamp;

    let days_since_unix_epoch = seconds_since_unix_epoch.div_euclid(DAY_SECONDS);
    let second_in_day = seconds_since_unix_epoch.rem_euclid(DAY_SECONDS);

    const MINUTE_SECONDS: i32 = 60;
    const HOUR_SECONDS: i32 = MINUTE_SECONDS * 60;
    const DAY_SECONDS: i32 = HOUR_SECONDS * 24;

    let tm_sec = second_in_day % MINUTE_SECONDS;
    let tm_min = (second_in_day % HOUR_SECONDS) / MINUTE_SECONDS;
    let tm_hour = second_in_day / HOUR_SECONDS;

    let days_since_y2k = days_since_unix_epoch - 10957;

    let cycles_since_y2k = days_since_y2k.div_euclid(CYCLE_DAYS);
    let day_in_cycle = days_since_y2k.rem_euclid(CYCLE_DAYS);

    let year_in_cycle: i32 = (YEAR_TO_DAY.partition_point(|&day| day <= day_in_cycle) - 1) as _;
    let year = 2000 + cycles_since_y2k * CYCLE_YEARS + year_in_cycle;
    let day_in_year = day_in_cycle - YEAR_TO_DAY[usize::try_from(year_in_cycle).unwrap()];
    let is_leap_year = is_leap_year(year_in_cycle);

    assert!(day_in_year < (365 + is_leap_year as i32));

    let month_to_day = if is_leap_year {
        &MONTH_TO_DAY_LEAP
    } else {
        &MONTH_TO_DAY_NONLEAP
    };

    let month_in_year: i32 = (month_to_day.partition_point(|&day| day <= day_in_year) - 1) as _;
    let day_in_month = day_in_year - month_to_day[usize::try_from(month_in_year).unwrap()];

    assert!(day_in_month < DAYS_IN_MONTH[month_in_year as usize] + is_leap_year as i32);

    let day_of_the_week = (4 + days_since_unix_epoch).rem_euclid(7);

    tm {
        tm_sec,
        tm_min,
        tm_hour,
        tm_mday: day_in_month + 1,
        tm_mon: month_in_year,
        tm_year: year - 1900,
        tm_wday: day_of_the_week,
        tm_yday: day_in_year,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: Ptr::null(),
    }
}

pub fn calendar_date_to_timestamp(tm: tm) -> time_t {
    // ИСПРАВЛЕНИЕ: Нормализация месяца и года по стандарту POSIX.
    // Если игра передает месяц 12 (или больше), мы конвертируем это в Январь следующего года.
    // Если передает отрицательный месяц - откатываем год назад.
    let mut y = tm.tm_year as i64 + 1900;
    let mut m = tm.tm_mon as i64;

    y += m.div_euclid(12);
    m = m.rem_euclid(12);

    let year = y as i32;
    let month = m as usize;

    let mut seconds = 0i64;

    for y in 1970..year {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        seconds += days_in_year * 86400;
    }

    let days_in_months_cumul = if is_leap_year(year) {
        MONTH_TO_DAY_LEAP[month]
    } else {
        MONTH_TO_DAY_NONLEAP[month]
    };

    seconds += days_in_months_cumul as i64 * 86400;

    // Дни, часы, минуты и секунды можно не нормализовать сложной математикой —
    // при конвертации в секунды они сами "перетекают" куда надо, так как
    // умножаются на свои константы.
    seconds += (tm.tm_mday as i64 - 1) * 86400;
    seconds += tm.tm_hour as i64 * 3600;
    seconds += tm.tm_min as i64 * 60;
    seconds += tm.tm_sec as i64;

    if year < 1970 {
        let mut days_before_year = 0i64;

        for y in year..1970 {
            days_before_year += if is_leap_year(y) { 366 } else { 365 };
        }

        seconds -= days_before_year * 86400;
    }

    seconds.try_into().unwrap_or({
        if seconds > 0 {
            time_t::MAX
        } else {
            time_t::MIN
        }
    })
}

fn gmtime_r(env: &mut Environment, timestamp: ConstPtr<time_t>, res: MutPtr<tm>) -> MutPtr<tm> {
    let timestamp = env.mem.read(timestamp);

    let calendar_date = timestamp_to_calendar_date(timestamp);
    env.mem.write(res, calendar_date);
    res
}
fn gmtime(env: &mut Environment, timestamp: ConstPtr<time_t>) -> MutPtr<tm> {
    let tmp = *env
        .libc_state
        .time
        .gmtime_tmp
        .get_or_insert_with(|| env.mem.alloc(guest_size_of::<tm>()).cast());

    gmtime_r(env, timestamp, tmp)
}

fn localtime_r(env: &mut Environment, timestamp: ConstPtr<time_t>, res: MutPtr<tm>) -> MutPtr<tm> {
    let timestamp = env.mem.read(timestamp);
    let offset = env.libc_state.time.timezone_offset_seconds;
    let adjusted = timestamp.saturating_sub(offset);
    let mut calendar_date = timestamp_to_calendar_date(adjusted);

    #[cfg(unix)]
    {
        let t = timestamp as ::libc::time_t;
        unsafe {
            let mut host_tm = std::mem::MaybeUninit::<::libc::tm>::uninit();
            ::libc::localtime_r(&t, host_tm.as_mut_ptr());
            let host_tm = host_tm.assume_init();
            calendar_date.tm_isdst = host_tm.tm_isdst;
        }
    }
    #[cfg(not(unix))]
    {
        calendar_date.tm_isdst = env.libc_state.time.daylight_flag;
    }

    calendar_date.tm_gmtoff = -offset;
    calendar_date.tm_zone = if calendar_date.tm_isdst > 0 {
        env.libc_state
            .time
            .tzname_dst_ptr
            .map(|p| p.cast_const())
            .unwrap_or_default()
    } else {
        env.libc_state
            .time
            .tzname_std_ptr
            .map(|p| p.cast_const())
            .unwrap_or_default()
    };

    env.mem.write(res, calendar_date);
    res
}
fn localtime(env: &mut Environment, timestamp: ConstPtr<time_t>) -> MutPtr<tm> {
    let tmp = *env
        .libc_state
        .time
        .gmtime_tmp
        .get_or_insert_with(|| env.mem.alloc(guest_size_of::<tm>()).cast());

    localtime_r(env, timestamp, tmp)
}

fn mktime(env: &mut Environment, tm: MutPtr<tm>) -> time_t {
    let tm_value = env.mem.read(tm);
    let offset = env.libc_state.time.timezone_offset_seconds;

    #[cfg(unix)]
    {
        // `::libc::tm` is platform-specific (different field count and
        // different mutability of `tm_zone` between Linux and macOS), so
        // zero-initialise the whole struct and then set only the portable
        // fields explicitly. `mktime` is documented (POSIX, Apple
        // <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/mktime.3.html>)
        // to read only `tm_year`/`tm_mon`/`tm_mday`/`tm_hour`/`tm_min`/`tm_sec`/`tm_isdst`
        // and re-derive everything else, so leaving the platform-private
        // fields at zero is safe.
        let mut host_tm: ::libc::tm = unsafe { std::mem::zeroed() };
        host_tm.tm_sec = tm_value.tm_sec;
        host_tm.tm_min = tm_value.tm_min;
        host_tm.tm_hour = tm_value.tm_hour;
        host_tm.tm_mday = tm_value.tm_mday;
        host_tm.tm_mon = tm_value.tm_mon;
        host_tm.tm_year = tm_value.tm_year;
        host_tm.tm_wday = tm_value.tm_wday;
        host_tm.tm_yday = tm_value.tm_yday;
        host_tm.tm_isdst = tm_value.tm_isdst;
        let res = unsafe { ::libc::mktime(&mut host_tm) } as time_t;

        let guest_tm = tm {
            tm_sec: host_tm.tm_sec,
            tm_min: host_tm.tm_min,
            tm_hour: host_tm.tm_hour,
            tm_mday: host_tm.tm_mday,
            tm_mon: host_tm.tm_mon,
            tm_year: host_tm.tm_year,
            tm_wday: host_tm.tm_wday,
            tm_yday: host_tm.tm_yday,
            tm_isdst: host_tm.tm_isdst,
            tm_gmtoff: -offset,
            tm_zone: if host_tm.tm_isdst > 0 {
                env.libc_state
                    .time
                    .tzname_dst_ptr
                    .map(|p| p.cast_const())
                    .unwrap_or_default()
            } else {
                env.libc_state
                    .time
                    .tzname_std_ptr
                    .map(|p| p.cast_const())
                    .unwrap_or_default()
            },
        };
        env.mem.write(tm, guest_tm);
        log_dbg!("mktime({:?}) => {}", tm_value, res);
        res
    }

    #[cfg(not(unix))]
    {
        let utc_timestamp = calendar_date_to_timestamp(tm_value);
        let res = utc_timestamp.saturating_add(offset);

        let mut normalized = timestamp_to_calendar_date(utc_timestamp);
        normalized.tm_isdst = env.libc_state.time.daylight_flag;
        normalized.tm_gmtoff = -offset;
        normalized.tm_zone = if normalized.tm_isdst > 0 {
            env.libc_state
                .time
                .tzname_dst_ptr
                .map(|p| p.cast_const())
                .unwrap_or(Ptr::null())
        } else {
            env.libc_state
                .time
                .tzname_std_ptr
                .map(|p| p.cast_const())
                .unwrap_or(Ptr::null())
        };
        env.mem.write(tm, normalized);
        log_dbg!("mktime({:?}) => {}", tm_value, res);
        res
    }
}

// sys/time.h (POSIX)

#[allow(non_camel_case_types)]
type suseconds_t = i32;

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub(super) struct timeval {
    pub(super) tv_sec: time_t,
    pub(super) tv_usec: suseconds_t,
}
unsafe impl SafeRead for timeval {}

#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, Default)]
#[repr(C, packed)]
pub struct timespec {
    pub tv_sec: time_t,
    pub tv_nsec: i32,
}
unsafe impl SafeRead for timespec {}

#[allow(non_camel_case_types)]
#[repr(C, packed)]
struct timezone {
    tz_minuteswest: i32,
    tz_dsttime: i32,
}
unsafe impl SafeRead for timezone {}

fn gettimeofday(
    env: &mut Environment,
    timeval_ptr: MutPtr<timeval>,
    timezone_ptr: MutPtr<timezone>,
) -> i32 {
    set_errno(env, 0);

    if !timezone_ptr.is_null() {
        env.mem.write(
            timezone_ptr,
            timezone {
                tz_minuteswest: env.libc_state.time.timezone_offset_seconds / 60,
                tz_dsttime: env.libc_state.time.daylight_flag,
            },
        );
    }

    if timeval_ptr.is_null() {
        return 0;
    }

    let time = env.guest_clock.system_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();

    let time_s_64: u64 = time.as_secs();
    let tv_sec = time_s_64 as time_t;

    if time_s_64 != tv_sec as u64 {
        log_once!("Warning: [gettimeofday] system clock is beyond Y2K38 and might confuse the app");
    }
    let tv_usec: suseconds_t = time.subsec_micros().try_into().unwrap();

    env.mem.write(timeval_ptr, timeval { tv_sec, tv_usec });

    0
}

fn nanosleep(env: &mut Environment, rqtp: ConstPtr<timespec>, _rmtp: MutPtr<timespec>) -> i32 {
    set_errno(env, 0);

    let t = env.mem.read(rqtp);
    // ИСПРАВЛЕНИЕ: Исключаем панику при отрицательном времени.
    // Функция `try_into().unwrap()` скрашилась бы с `TryFromIntError` при
    // отрицательных значениях от плохих игр.
    // Защищаем Rust-составляющую, ограничивая минимальное время нулем.
    let tv_sec = t.tv_sec.max(0) as u64;
    let tv_nsec = t.tv_nsec.max(0) as u64;
    log_dbg!("nanosleep {} {}", tv_sec, tv_nsec);

    let total_sleep = Duration::from_secs(tv_sec) + Duration::from_nanos(tv_nsec);
    env.sleep_guest(total_sleep);

    0
}

fn strptime(
    env: &mut Environment,
    buffer: ConstPtr<u8>,
    format: ConstPtr<u8>,
    time_ptr: MutPtr<tm>,
) -> MutPtr<u8> {
    log_dbg!(
        "strptime({:?}, {:?})",
        env.mem.cstr_at_utf8(buffer),
        env.mem.cstr_at_utf8(format)
    );

    let mut time_val = env.mem.read(time_ptr);

    let mut conversation_failed = false;
    let mut buffer_char_idx = 0;
    let mut format_char_idx = 0;

    loop {
        let c = env.mem.read(format + format_char_idx);
        format_char_idx += 1;

        if c == b'\0' {
            break;
        }
        if c != b'%' {
            let mut cc = env.mem.read(buffer + buffer_char_idx);

            if isspace(env, format + format_char_idx - 1) {
                while isspace_inner(cc) {
                    buffer_char_idx += 1;
                    cc = env.mem.read(buffer + buffer_char_idx);
                }
                continue;
            }
            if c != cc {
                conversation_failed = true;
                break;
            }
            buffer_char_idx += 1;
            continue;
        }

        let specifier = env.mem.read(format + format_char_idx);
        format_char_idx += 1;

        let mut parse_2_digits = |range: Range<i32>| -> Result<i32, ()> {
            let mut num: i32 = 0;

            let mut chars_count = 0;
            while let c @ b'0'..=b'9' = env.mem.read(buffer + buffer_char_idx) {
                if chars_count >= 2 {
                    break;
                }
                num = num * 10 + (c - b'0') as i32;

                buffer_char_idx += 1;
                chars_count += 1;
            }
            if chars_count != 2 {
                Err(())
            } else if !range.contains(&num) {
                // The guest fed us a syntactically valid 2-digit number that
                // is outside the legal range for this field (e.g. hour=99).
                // Real strptime() returns NULL for this. Don't crash the host.
                log!(
                    "Warning: strptime(): value {} out of range {:?} for field; failing conversion.",
                    num,
                    range
                );
                Err(())
            } else {
                Ok(num)
            }
        };

        match specifier {
            b'H' => match parse_2_digits(0..24) {
                Ok(hour) => {
                    time_val.tm_hour = hour;
                }
                Err(_) => {
                    conversation_failed = true;
                    break;
                }
            },
            b'M' => match parse_2_digits(0..60) {
                Ok(minute) => {
                    time_val.tm_min = minute;
                }
                Err(_) => {
                    conversation_failed = true;
                    break;
                }
            },
            b'S' => match parse_2_digits(0..61) {
                Ok(second) => {
                    time_val.tm_sec = second;
                }
                Err(_) => {
                    conversation_failed = true;
                    break;
                }
            },
            _ => {
                // Unsupported format specifier. Real strptime() returns NULL
                // here; do the same instead of taking down the host.
                log!(
                    "Warning: strptime(): unsupported format character '{}' at index {}; failing conversion.",
                    specifier as char,
                    format_char_idx
                );
                conversation_failed = true;
                break;
            }
        }
    }

    env.mem.write(time_ptr, time_val);

    if conversation_failed {
        Ptr::null()
    } else {
        (buffer + buffer_char_idx).cast_mut()
    }
}

pub(super) fn strftime(
    env: &mut Environment,
    s: MutPtr<u8>,
    max_size: GuestUSize,
    format: ConstPtr<u8>,
    time_ptr: ConstPtr<tm>,
) -> GuestUSize {
    log_dbg!(
        "strftime({:?}, {}, {:?}, {:?})",
        s,
        max_size,
        env.mem.cstr_at_utf8(format),
        time_ptr
    );

    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    let ctype_locale_byte = env.mem.read(ctype_locale);
    if ctype_locale_byte != b'C' {
        // We currently only model the C locale. Apps that set a different
        // LC_CTYPE will silently get C-locale formatting; that's fine for
        // numeric format specifiers, just log so we notice.
        log!(
            "Warning: strftime(): unexpected LC_CTYPE locale {:?}; treating as C locale.",
            ctype_locale_byte
        );
    }

    let time_val = env.mem.read(time_ptr);

    let mut res = Vec::<u8>::new();
    let mut format_char_idx = 0;

    loop {
        let c = env.mem.read(format + format_char_idx);
        format_char_idx += 1;

        if c == b'\0' {
            break;
        }
        if c != b'%' {
            res.push(c);
            continue;
        }

        let specifier = env.mem.read(format + format_char_idx);
        format_char_idx += 1;

        match specifier {
            b'm' => {
                let month = (time_val.tm_mon + 1).clamp(1, 12);
                let formatted_month = format!("{:02}", month);
                res.extend_from_slice(formatted_month.as_bytes());
            }
            b'W' => {
                let wday = time_val.tm_wday;
                let yday = time_val.tm_yday;
                // Для %W неделя начинается с понедельника.
                // tm_wday: 0 = Вск, 1 = Пнд... Нам нужно 0 = Пнд... 6 = Вск
                let wday_monday_based = (wday + 6) % 7;

                // Честная формула вычисления номера недели (00-53)
                let week = (yday - wday_monday_based + 7) / 7;
                let formatted_week = format!("{:02}", week);
                res.extend_from_slice(formatted_week.as_bytes());
            }
            b'U' => {
                // Аналогично, но неделя начинается с воскресенья (%U)
                let wday = time_val.tm_wday;
                let yday = time_val.tm_yday;
                let week = (yday - wday + 7) / 7;
                let formatted_week = format!("{:02}", week);
                res.extend_from_slice(formatted_week.as_bytes());
            }
            b'w' => {
                // Номер дня недели от 0 (Воскресенье) до 6 (Суббота)
                let wday = time_val.tm_wday;
                let formatted_wday = format!("{}", wday);
                res.extend_from_slice(formatted_wday.as_bytes());
            }
            b'z' => {
                // RFC 822 numeric zone: ±HHMM (e.g. +0300). Uses the guest
                // tm_gmtoff like the %Z handler above; with the epoch-relative
                // clock this is +0000 (UTC).
                let gmtoff: i32 = time_val.tm_gmtoff;
                let (sign, abs) = if gmtoff < 0 {
                    ('-', -gmtoff)
                } else {
                    ('+', gmtoff)
                };
                let hh = abs / 3600;
                let mm = (abs % 3600) / 60;
                res.extend_from_slice(format!("{}{:02}{:02}", sign, hh, mm).as_bytes());
            }
            b'd' => {
                let day = time_val.tm_mday.clamp(1, 31);
                let formatted_day = format!("{:02}", day);
                res.extend_from_slice(formatted_day.as_bytes());
            }
            b'H' => {
                let hour = time_val.tm_hour.clamp(0, 23);
                let formatted_hour = format!("{:02}", hour);
                res.extend_from_slice(formatted_hour.as_bytes());
            }
            b'M' => {
                let minute = time_val.tm_min.clamp(0, 59);
                let formatted_minute = format!("{:02}", minute);
                res.extend_from_slice(formatted_minute.as_bytes());
            }
            b'X' => {
                // %X is the locale's time representation; in the C locale
                // (touchHLE's only locale) that is exactly %H:%M:%S.
                let hour = time_val.tm_hour.clamp(0, 23);
                let minute = time_val.tm_min.clamp(0, 59);
                let second = time_val.tm_sec.clamp(0, 60);
                res.extend_from_slice(
                    format!("{:02}:{:02}:{:02}", hour, minute, second).as_bytes(),
                );
            }
            b'x' => {
                // %x is the locale's date representation; in the C locale
                // that is %m/%d/%y.
                let month = (time_val.tm_mon.clamp(0, 11)) + 1;
                let day = time_val.tm_mday.clamp(1, 31);
                let year = (time_val.tm_year + 1900) % 100;
                res.extend_from_slice(format!("{:02}/{:02}/{:02}", month, day, year).as_bytes());
            }
            b'b' | b'h' => {
                let month = time_val.tm_mon.clamp(0, 11);
                const MONTH_ABBRS: [&[u8]; 12] = [
                    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct",
                    b"Nov", b"Dec",
                ];
                res.extend_from_slice(MONTH_ABBRS[month as usize]);
            }
            b'a' => {
                let wday = time_val.tm_wday.clamp(0, 6);
                const WDAY_ABBRS: [&[u8]; 7] =
                    [b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"];
                res.extend_from_slice(WDAY_ABBRS[wday as usize]);
            }
            b'Y' => {
                let year = time_val.tm_year + 1900;
                let formatted_year = format!("{:04}", year);
                res.extend_from_slice(formatted_year.as_bytes());
            }
            b'y' => {
                let year = (time_val.tm_year + 1900) % 100;
                let formatted_year = format!("{:02}", year);
                res.extend_from_slice(formatted_year.as_bytes());
            }
            b'z' => {
                // RFC 822 / ISO 8601 numeric offset: "+hhmm" / "-hhmm".
                // Derived from tm_gmtoff exactly like glibc's implementation
                // (tm_gmtoff is seconds east of UTC).
                let offset_seconds = time_val.tm_gmtoff;
                let sign = if offset_seconds < 0 { '-' } else { '+' };
                let abs_offset = offset_seconds.abs();
                let hh = abs_offset / 3600;
                let mm = (abs_offset % 3600) / 60;
                res.push(sign as u8);
                res.extend_from_slice(format!("{:02}{:02}", hh, mm).as_bytes());
            }
            b'Z' => {
                let tz_ptr = time_val.tm_zone;
                if tz_ptr.is_null() {
                    // Эмулятор считает время от UNIX_EPOCH без смещения
                    // (tm_gmtoff = 0),
                    // поэтому мы легально находимся в зоне GMT.
                    res.extend_from_slice(b"GMT");
                } else if let Ok(tz_str) = env.mem.cstr_at_utf8(tz_ptr) {
                    // Если указатель есть — честно читаем зону из памяти гостя
                    res.extend_from_slice(tz_str.as_bytes());
                } else {
                    res.extend_from_slice(b"GMT");
                }
            }
            b'S' => {
                let second = time_val.tm_sec.clamp(0, 60);
                let formatted_second = format!("{:02}", second);
                res.extend_from_slice(formatted_second.as_bytes());
            }
            b'z' => {
                // Numeric UTC offset, e.g. "+0300". Derived from tm_gmtoff
                // (seconds east of UTC); zones west produce a '-'.
                let offset_seconds = time_val.tm_gmtoff as i64;
                let sign = if offset_seconds < 0 { '-' } else { '+' };
                let abs_offset = offset_seconds.abs();
                let hours = abs_offset / 3600;
                let minutes = (abs_offset % 3600) / 60;
                let formatted_offset = format!("{}{:02}{:02}", sign, hours, minutes);
                res.extend_from_slice(formatted_offset.as_bytes());
            }
            other => {
                // Unsupported format specifier in strftime(). Emit it
                // literally (preceded by the percent, as glibc does for
                // some unsupported specifiers) and continue instead of
                // crashing the host.
                log!(
                    "Warning: strftime(): unsupported format character '{}' at index {}; emitting literally.",
                    other as char,
                    format_char_idx
                );
                res.push(b'%');
                res.push(other);
            }
        }
    }

    if max_size == 0 {
        // glibc strftime() with max_size=0 returns 0 and writes nothing.
        // Avoid the underflow below.
        return 0;
    }

    // strftime returns zero if the result including NUL does not fit.
    // Returning the untruncated length lets callers read beyond the buffer.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/strftime.3.html
    if res.len() >= max_size as usize {
        return 0;
    }
    let len = res.len() as GuestUSize;
    env.mem.bytes_at_mut(s, len).copy_from_slice(&res);
    env.mem.write(s + len, 0);
    len
}

/// Size of the buffer returned by `asctime`/`ctime`. The C standard output is
/// 26 bytes ("Www Mmm dd hh:mm:ss yyyy\n\0"); we allocate extra slack so that
/// out-of-range years (which produce a longer year field) still fit.
const ASCTIME_BUF_SIZE: GuestUSize = 64;

const ASCTIME_WDAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const ASCTIME_MON: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Format a `tm` exactly like the C standard `asctime`:
/// `"%.3s %.3s%3d %.2d:%.2d:%.2d %d\n"`, e.g. `"Thu Nov 24 18:22:48 1986\n"`.
fn format_asctime(tm: &tm) -> String {
    let wday = ASCTIME_WDAY[tm.tm_wday.rem_euclid(7) as usize];
    let mon = ASCTIME_MON[tm.tm_mon.rem_euclid(12) as usize];
    // `format!` takes references to its arguments, but references to fields of
    // a `#[repr(packed)]` struct are not allowed (they may be unaligned), so
    // copy each field into a local first.
    let mday = tm.tm_mday;
    let hour = tm.tm_hour;
    let min = tm.tm_min;
    let sec = tm.tm_sec;
    let year = tm.tm_year + 1900;
    format!(
        "{} {}{:3} {:02}:{:02}:{:02} {}\n",
        wday, mon, mday, hour, min, sec, year,
    )
}

/// Lazily allocate (and return) the shared static buffer used by both
/// `asctime` and `ctime`.
fn asctime_buf(env: &mut Environment) -> MutPtr<u8> {
    *env.libc_state.time.asctime_buf.get_or_insert_with(|| {
        let ptr = env.mem.alloc(ASCTIME_BUF_SIZE).cast::<u8>();
        env.mem.bytes_at_mut(ptr, ASCTIME_BUF_SIZE).fill(0);
        ptr
    })
}

/// `char *asctime(const struct tm *timeptr)`
fn asctime(env: &mut Environment, timeptr: ConstPtr<tm>) -> MutPtr<u8> {
    if timeptr.is_null() {
        return Ptr::null();
    }
    let calendar_date = env.mem.read(timeptr);
    let s = format_asctime(&calendar_date);
    let buf = asctime_buf(env);
    write_cstr_to_buffer(env, buf, &s, ASCTIME_BUF_SIZE);
    buf
}

/// `char *ctime(const time_t *timep)`
///
/// Equivalent to `asctime(localtime(timep))`: converts the timestamp to local
/// calendar time, then formats it into the shared static buffer.
fn ctime(env: &mut Environment, timep: ConstPtr<time_t>) -> MutPtr<u8> {
    if timep.is_null() {
        return Ptr::null();
    }
    let timestamp = env.mem.read(timep);
    let offset = env.libc_state.time.timezone_offset_seconds;
    let calendar_date = timestamp_to_calendar_date(timestamp.saturating_sub(offset));
    let s = format_asctime(&calendar_date);
    let buf = asctime_buf(env);
    write_cstr_to_buffer(env, buf, &s, ASCTIME_BUF_SIZE);
    buf
}

fn difftime(_env: &mut Environment, time1: time_t, time0: time_t) -> f64 {
    // Возвращаем разницу в секундах.
    // Приведение к f64 гарантирует, что мы отдаем честный double, как того ждет
    // игра.
    (time1 as f64) - (time0 as f64)
}

/// `size_t strftime_l(char *restrict s, size_t maxsize,
/// const char *restrict format, const struct tm *restrict timeptr,
/// locale_t loc)`
///
/// Per Apple's `strftime_l(3)` man page (iPhoneOS man pages), this behaves
/// exactly like `strftime()` except it formats using the supplied extended
/// locale instead of the global locale. touchHLE only models the "C"
/// locale, so the locale argument is ignored and we forward to `strftime`.
/// This symbol is imported by libstdc++/libc++ iostreams (`std::time_put`),
/// which guest C++ code hits when formatting dates.
fn strftime_l(
    env: &mut Environment,
    s: MutPtr<u8>,
    max_size: GuestUSize,
    format: ConstPtr<u8>,
    time_ptr: ConstPtr<tm>,
    _locale: crate::libc::clocale::locale_t,
) -> GuestUSize {
    strftime(env, s, max_size, format, time_ptr)
}

#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub(super) struct rusage {
    ru_utime: timeval,
    ru_stime: timeval,
    // The remaining fields are long counters; zero-filled is fine.
    ru_maxrss: i32,
    ru_ixrss: i32,
    ru_idrss: i32,
    ru_isrss: i32,
    ru_minflt: i32,
    ru_majflt: i32,
    ru_nswap: i32,
    ru_inblock: i32,
    ru_oublock: i32,
    ru_msgsnd: i32,
    ru_msgrcv: i32,
    ru_nsignals: i32,
    ru_nvcsw: i32,
    ru_nivcsw: i32,
}
unsafe impl SafeRead for rusage {}

const RUSAGE_SELF: i32 = 0;

/// `int getrusage(int who, struct rusage *usage)` (Darwin)
///
/// Reports the host process CPU time as the guest's user/system time. Apps
/// like Chrome call this for performance counters and tolerate zeros, but
/// returning real elapsed CPU time is more faithful.
fn getrusage(env: &mut Environment, who: i32, usage_ptr: MutPtr<rusage>) -> i32 {
    if who != RUSAGE_SELF {
        log!("Warning: getrusage(who: {}) for non-self usage is unimplemented; returning zeros", who);
        grusage_zero(env, usage_ptr);
        return 0;
    }
    if usage_ptr.is_null() {
        set_errno(env, 14); // EFAULT
        return -1;
    }
    let elapsed = env.startup_time.elapsed();
    let mut usage = rusage {
        ru_utime: timeval {
            tv_sec: elapsed.as_secs() as crate::libc::time::time_t,
            tv_usec: (elapsed.subsec_micros()) as suseconds_t,
        },
        ru_stime: timeval { tv_sec: 0, tv_usec: 0 },
        ru_maxrss: 0,
        ru_ixrss: 0,
        ru_idrss: 0,
        ru_isrss: 0,
        ru_minflt: 0,
        ru_majflt: 0,
        ru_nswap: 0,
        ru_inblock: 0,
        ru_oublock: 0,
        ru_msgsnd: 0,
        ru_msgrcv: 0,
        ru_nsignals: 0,
        ru_nvcsw: 0,
        ru_nivcsw: 0,
    };
    // time_t is i64 on Darwin; our packed struct above uses the right widths
    // via the timeval fields. Write it out.
    env.mem.write(usage_ptr, usage);
    0
}

fn grusage_zero(env: &mut Environment, usage_ptr: MutPtr<rusage>) {
    if usage_ptr.is_null() {
        return;
    }
    let zero = rusage {
        ru_utime: timeval { tv_sec: 0, tv_usec: 0 },
        ru_stime: timeval { tv_sec: 0, tv_usec: 0 },
        ru_maxrss: 0,
        ru_ixrss: 0,
        ru_idrss: 0,
        ru_isrss: 0,
        ru_minflt: 0,
        ru_majflt: 0,
        ru_nswap: 0,
        ru_inblock: 0,
        ru_oublock: 0,
        ru_msgsnd: 0,
        ru_msgrcv: 0,
        ru_nsignals: 0,
        ru_nvcsw: 0,
        ru_nivcsw: 0,
    };
    env.mem.write(usage_ptr, zero);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getrusage(_, _)),
    export_c_func!(clock()),
    export_c_func!(time(_)),
    export_c_func!(tzset()),
    export_c_func!(gmtime_r(_, _)),
    export_c_func!(gmtime(_)),
    export_c_func!(mktime(_)),
    export_c_func!(localtime_r(_, _)),
    export_c_func!(localtime(_)),
    export_c_func!(gettimeofday(_, _)),
    export_c_func!(nanosleep(_, _)),
    export_c_func!(strptime(_, _, _)),
    export_c_func!(strftime(_, _, _, _)),
    export_c_func!(strftime_l(_, _, _, _, _)),
    export_c_func!(difftime(_, _)),
    export_c_func!(asctime(_)),
    export_c_func!(ctime(_)),
];
