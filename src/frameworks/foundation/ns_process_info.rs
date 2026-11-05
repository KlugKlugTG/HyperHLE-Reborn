/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSProcessInfo`.
//!
//! Apple references used:
//! - <https://developer.apple.com/documentation/foundation/nsprocessinfo>
//! - <https://developer.apple.com/documentation/foundation/nsoperatingsystemversion>
//! - `Foundation/NSProcessInfo.h` declares `NSOperatingSystemVersion` as a
//!   struct of three `NSInteger` fields (`majorVersion`, `minorVersion`,
//!   `patchVersion`).
//!
//! On 32-bit iOS `NSInteger` is `int32_t` (4 bytes), so the struct is 12
//! bytes total and is returned by value via the AAPCS struct-return ABI
//! (i.e. via the stret variant of `objc_msgSend`).

use super::NSTimeInterval;
use crate::abi::{impl_GuestRet_for_large_struct, GuestArg};
use crate::frameworks::foundation::ns_string;
use crate::libc::mach::host::physical_memory;
use crate::libc::sysctl::GUEST_PROCESSOR_COUNT;
use crate::mem::SafeRead;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports,
};
use crate::Environment;
use std::collections::HashMap;

/// `NSOperatingSystemVersion` from `Foundation/NSProcessInfo.h`.
///
/// ```c
/// typedef struct {
///     NSInteger majorVersion;
///     NSInteger minorVersion;
///     NSInteger patchVersion;
/// } NSOperatingSystemVersion;
/// ```
///
/// On 32-bit iOS, `NSInteger` is a 32-bit signed integer.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C, packed)]
pub struct NSOperatingSystemVersion {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
}
unsafe impl SafeRead for NSOperatingSystemVersion {}
impl_GuestRet_for_large_struct!(NSOperatingSystemVersion);
impl GuestArg for NSOperatingSystemVersion {
    const REG_COUNT: usize = 3;
    fn from_regs(regs: &[u32]) -> Self {
        NSOperatingSystemVersion {
            major: GuestArg::from_regs(&regs[0..1]),
            minor: GuestArg::from_regs(&regs[1..2]),
            patch: GuestArg::from_regs(&regs[2..3]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        self.major.to_regs(&mut regs[0..1]);
        self.minor.to_regs(&mut regs[1..2]);
        self.patch.to_regs(&mut regs[2..3]);
    }
}

#[derive(Default)]
pub struct State {
    /// `NSProcessInfo*`
    process_info: Option<id>,
    /// Outstanding activity assertions from
    /// `-beginActivityWithOptions:reason:`, keyed by the token object that
    /// was handed to the guest. Per Apple's documentation, the token is an
    /// opaque object that must be passed back to `-endActivity:`.
    active_activities: HashMap<id, ActivityInfo>,
}

/// Bookkeeping for one outstanding activity assertion.
struct ActivityInfo {
    options: u64,
    reason: String,
}

fn assert_process_info_singleton(env: &mut Environment, this: id) {
    assert_eq!(
        this,
        env.framework_state
            .foundation
            .ns_process_info
            .process_info
            .unwrap()
    );
}

/// Fake OS version used when the app queries the host system version.
///
/// touchHLE targets early iPhone OS apps; we report iOS 12.0.0 so that any
/// iOS 8+ feature gate (which is the floor for many third-party SDKs that
/// query `operatingSystemVersion`) passes without triggering the
/// "unsupported version" path inside the app.
fn os_version(env: &Environment) -> (i32, i32, i32) {
    env.options
        .as_ref()
        .ios_version
        .unwrap_or(crate::options::LATEST_IOS_VERSION)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSProcessInfo: NSObject

// =========================================================================
// MARK: - Singleton
// =========================================================================

+ (id)processInfo {
    if let Some(existing) = env.framework_state.foundation.ns_process_info.process_info {
        existing
    } else {
        let process_info: id = msg![env; this new];
        env.framework_state.foundation.ns_process_info.process_info = Some(process_info);
        process_info
    }
}

// =========================================================================
// MARK: - Process identity
// =========================================================================

- (id)processName {
    assert_process_info_singleton(env, this);
    let main_bundle: id = msg_class![env; NSBundle mainBundle];
    let name_key: id = ns_string::get_static_str(env, "CFBundleName");
    msg![env; main_bundle objectForInfoDictionaryKey:name_key]
}

- (())setProcessName:(id)_name {
    assert_process_info_singleton(env, this);
    log!("TODO: [NSProcessInfo setProcessName:] — ignored");
}

- (i32)processIdentifier {
    assert_process_info_singleton(env, this);
    1234
}

- (id)globallyUniqueString {
    assert_process_info_singleton(env, this);
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let uptime_ns = env.guest_clock.now()
        .duration_since(env.startup_time)
        .as_nanos() as u64;
    let s = format!("1234-{}-{}", uptime_ns, n);
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

// =========================================================================
// MARK: - Host / environment
// =========================================================================

- (id)hostName {
    assert_process_info_singleton(env, this);
    ns_string::get_static_str(env, "touchHLE-host.local")
}

- (id)arguments {
    assert_process_info_singleton(env, this);
    msg_class![env; NSArray array]
}

- (id)environment {
    assert_process_info_singleton(env, this);
    // Snapshot the emulated process environment, not the host's variables.
    let vars: Vec<(String, String)> = env
        .env_vars
        .iter()
        .map(|(key, value)| {
            (
                String::from_utf8_lossy(key).into_owned(),
                env.mem.cstr_at_utf8(*value).unwrap().to_string(),
            )
        })
        .collect();
    let dictionary: id = msg_class![env; NSMutableDictionary new];
    for (key, value) in vars {
        let key = ns_string::from_rust_string(env, key);
        let value = ns_string::from_rust_string(env, value);
        () = msg![env; dictionary setObject:value forKey:key];
        release(env, key);
        release(env, value);
    }
    let snapshot: id = msg![env; dictionary copy];
    release(env, dictionary);
    autorelease(env, snapshot)
}

// =========================================================================
// MARK: - Hardware / memory
// =========================================================================

- (u64)physicalMemory {
    assert_process_info_singleton(env, this);
    physical_memory(env)
}

- (u32)processorCount {
    assert_process_info_singleton(env, this);
    GUEST_PROCESSOR_COUNT as u32
}

- (u32)activeProcessorCount {
    assert_process_info_singleton(env, this);
    GUEST_PROCESSOR_COUNT as u32
}

// =========================================================================
// MARK: - Uptime
// =========================================================================

- (NSTimeInterval)systemUptime {
    assert_process_info_singleton(env, this);
    env.guest_clock.now().duration_since(env.startup_time).as_secs_f64()
}

// =========================================================================
// MARK: - OS version
// =========================================================================

// `- (NSOperatingSystemVersion)operatingSystemVersion` (iOS 8+).
//
// Documented at
// <https://developer.apple.com/documentation/foundation/nsprocessinfo/1410031-operatingsystemversion>.
//
// The selector returns `NSOperatingSystemVersion` *by value*. Because the
// struct is larger than the AAPCS register-return limit (4 bytes on
// 32-bit ARM), the compiler emits `objc_msgSend_stret`, passing a hidden
// pointer in `r0` for the struct destination. Returning `id` here (as the
// old stub did) caused the receiver to be mis-mapped to that destination
// pointer, which produced the assertion failure observed in Bloons TD 5.
- (NSOperatingSystemVersion)operatingSystemVersion {
    assert_process_info_singleton(env, this);
    let (major, minor, patch) = os_version(env);
    NSOperatingSystemVersion { major, minor, patch }
}

- (id)operatingSystemVersionString {
    assert_process_info_singleton(env, this);
    // Per Apple's documentation this returns a human-readable version string.
    // Apps (e.g. Puzzle Agent) use it together with operatingSystemName for
    // platform diagnostics.
    let (major, minor, patch) = os_version(env);
    let s = format!("Version {major}.{minor}.{patch} (Build 16A366)");
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

// `- (BOOL)isOperatingSystemAtLeastVersion:(NSOperatingSystemVersion)version`
// (iOS 8+).
//
// Documented at
// <https://developer.apple.com/documentation/foundation/nsprocessinfo/1417380-isoperatingsystematleastversion>.
//
// Takes a single `NSOperatingSystemVersion` struct by value. On 32-bit
// ARM AAPCS the 12-byte struct is split across `r2`, `r3` and one stack
// slot (after the implicit `self`/`_cmd`). The previous declaration used
// three separate `u64` parameters with the wrong selector
// (`...:minor:patch:`), so it was unreachable from real apps.
- (bool)isOperatingSystemAtLeastVersion:(NSOperatingSystemVersion)version {
    assert_process_info_singleton(env, this);
    let (current_major, current_minor, current_patch) = os_version(env);
    let NSOperatingSystemVersion { major, minor, patch } = version;
    if major != current_major {
        return major < current_major;
    }
    if minor != current_minor {
        return minor < current_minor;
    }
    patch <= current_patch
}

// =========================================================================
// MARK: - Thermal state (iOS 11+)
// =========================================================================

// NSProcessInfoThermalStateNominal = 0
- (i64)thermalState {
    assert_process_info_singleton(env, this);
    0
}

// =========================================================================
// MARK: - Low Power Mode (iOS 9+)
// =========================================================================

- (bool)isLowPowerModeEnabled {
    assert_process_info_singleton(env, this);
    false
}

// =========================================================================
// MARK: - Activity assertions (iOS 7+)
// =========================================================================

- (id)beginActivityWithOptions:(u64)options reason:(id)reason {
    assert_process_info_singleton(env, this);

    // Per the NSProcessInfo documentation the return value is an opaque
    // activity token that must be passed back to -endActivity:. Returning
    // `this` (as the old stub did) is wrong: the app could then pass the
    // process-info object itself back to -endActivity:, and two concurrent
    // activities would be indistinguishable. Give out unique NSObject
    // tokens instead, like -NSNotificationCenter's block-observation
    // tokens, and remember them so -endActivity: can match them.
    let token: id = msg_class![env; NSObject new];
    retain(env, token);

    let reason_str = if reason == nil {
        String::new()
    } else {
        ns_string::to_rust_string(env, reason).into_owned()
    };
    log_dbg!(
        "[NSProcessInfo beginActivityWithOptions:{:#x} reason:{:?}] -> token {:?}",
        options,
        reason_str,
        token
    );

    let state = &mut env.framework_state.foundation.ns_process_info;
    state
        .active_activities
        .insert(token, ActivityInfo { options, reason: reason_str });

    token
}

- (())endActivity:(id)activity {
    assert_process_info_singleton(env, this);
    if activity == nil {
        // Passing nil is a caller bug; Apple's documentation does not
        // define the behaviour, so just ignore it.
        return;
    }
    let state = &mut env.framework_state.foundation.ns_process_info;
    match state.active_activities.remove(&activity) {
        Some(info) => {
            log_dbg!(
                "[(NSProcessInfo*){:?} endActivity:{:?}] ended activity \"{}\" (options {:#x})",
                this,
                activity,
                info.reason,
                info.options
            );
            release(env, activity);
        }
        None => {
            log!(
                "Warning: [NSProcessInfo endActivity:{:?}] called with an unknown activity token; ignoring.",
                activity
            );
        }
    }
}

- (())performActivityWithOptions:(u64)options
                          reason:(id)reason
                      usingBlock:(id)block {
    assert_process_info_singleton(env, this);
    // Apple's contract: apply the activity for the duration of the block.
    // (The `wait` behaviour only matters across threads, which HLE does not
    // model — the block is always run synchronously here.)
    let token: id = msg![env; this beginActivityWithOptions:options reason:reason];
    let _: () = msg![env; block invoke];
    let _: () = msg![env; this endActivity:token];
}

// =========================================================================
// MARK: - Sudden termination (macOS; silently ignored on iOS)
// =========================================================================

- (())disableSuddenTermination {
    assert_process_info_singleton(env, this);
}

- (())enableSuddenTermination {
    assert_process_info_singleton(env, this);
}

- (())disableAutomaticTermination:(id)_reason {
    assert_process_info_singleton(env, this);
}

- (())enableAutomaticTermination:(id)_reason {
    assert_process_info_singleton(env, this);
}

- (bool)automaticTerminationSupportEnabled {
    assert_process_info_singleton(env, this);
    false
}

- (())setAutomaticTerminationSupportEnabled:(bool)_enabled {
    assert_process_info_singleton(env, this);
}

- (id)operatingSystemName {
    assert_process_info_singleton(env, this); // TODO
    // This is the value documented by Foundation for the Darwin/Mach family.
    ns_string::get_static_str(env, "NSMACHOperatingSystem")
}

@end

};
