/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFPreferences`.
//!
//! According to Apple's docs, it's not toll-free bridged to `NSUserDefaults`,
//! but we are still implementing one atop of another. Every domain, user and
//! host argument is treated as "the current app", since a sandboxed iOS app
//! can only reach its own preferences anyway.

use super::cf_array::CFArrayRef;
use super::cf_dictionary::CFDictionaryRef;
use super::cf_string::CFStringRef;
use super::{CFIndex, CFTypeRef};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::mem::MutPtr;
use crate::objc::{id, msg, msg_class, nil, release, retain};
use crate::Environment;

type CFPropertyListRef = CFTypeRef;

// MARK: - Helpers

/// Decide whether a CFPreferences `applicationID` argument refers to the
/// running app's preferences. Accepts `kCFPreferencesCurrentApplication`,
/// the app's own bundle identifier, and `NULL`. Any other identifier is
/// logged (debug only) and still treated as the current app, so the call
/// degrades to a normal `NSUserDefaults` lookup instead of asserting.
fn app_id_is_current(env: &mut Environment, app_id: CFStringRef) -> bool {
    if app_id == nil {
        return true;
    }
    let current_app = ns_string::get_static_str(env, kCFPreferencesCurrentApplication);
    if msg![env; app_id isEqualToString:current_app] {
        return true;
    }
    let app_id_str = ns_string::to_rust_string(env, app_id);
    let bundle_id = env.bundle.bundle_identifier();
    if app_id_str == bundle_id {
        return true;
    }
    log_dbg!(
        "CFPreferences called with applicationID {:?}, which is neither \
         kCFPreferencesCurrentApplication nor this app's bundle id ({:?}); \
         routing to NSUserDefaults anyway.",
        app_id_str,
        bundle_id,
    );
    false
}

fn standard_defaults(env: &mut Environment) -> id {
    msg_class![env; NSUserDefaults standardUserDefaults]
}

fn is_kind_of(env: &mut Environment, obj: id, class_name: &str) -> bool {
    let class = env.objc.get_known_class(class_name, &mut env.mem);
    msg![env; obj isKindOfClass:class]
}

/// The owned (+1) copy of a value, or `nil`. Shared by every "Copy" and
/// "Get" function. NULL keys are rejected instead of reaching the
/// dictionary code.
fn copy_app_value(
    env: &mut Environment,
    key: CFStringRef,
    app_id: CFStringRef,
) -> CFPropertyListRef {
    let _ = app_id_is_current(env, app_id);
    if key == nil {
        return nil;
    }
    let user_defaults = standard_defaults(env);
    let value: id = msg![env; user_defaults objectForKey:key];
    if value == nil {
        return nil;
    }
    msg![env; value copy]
}

/// A NULL value removes the key.
fn set_app_value(
    env: &mut Environment,
    key: CFStringRef,
    value: CFPropertyListRef,
    app_id: CFStringRef,
) {
    let _ = app_id_is_current(env, app_id);
    if key == nil {
        return;
    }
    let user_defaults = standard_defaults(env);
    if value.is_null() {
        () = msg![env; user_defaults removeObjectForKey:key];
    } else {
        () = msg![env; user_defaults setObject:value forKey:key];
    }
}

/// `(result, valid)` following CF's rules for booleans.
fn interpret_bool(env: &mut Environment, value: id) -> (bool, bool) {
    if is_kind_of(env, value, "NSString") {
        let s = ns_string::to_rust_string(env, value).into_owned();
        if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("yes") {
            (true, true)
        } else if s.eq_ignore_ascii_case("false") || s.eq_ignore_ascii_case("no") {
            (false, true)
        } else {
            (false, false)
        }
    } else if is_kind_of(env, value, "NSNumber") {
        let n: i64 = msg![env; value longLongValue];
        (n != 0, true)
    } else {
        (false, false)
    }
}

/// `(result, valid)` following CF's rules for integers.
fn interpret_int(env: &mut Environment, value: id) -> (CFIndex, bool) {
    if is_kind_of(env, value, "NSString") {
        // The whole string must be consumed; leading whitespace is skipped.
        let s = ns_string::to_rust_string(env, value).into_owned();
        match s.trim_start().parse::<i32>() {
            Ok(n) => (n, true),
            Err(_) => (0, false),
        }
    } else if is_kind_of(env, value, "NSNumber") {
        let n: i64 = msg![env; value longLongValue];
        match i32::try_from(n) {
            Ok(n) => (n, true),
            Err(_) => (n.clamp(i32::MIN as i64, i32::MAX as i64) as i32, false),
        }
    } else {
        (0, false)
    }
}

// MARK: - App-scoped functions

fn CFPreferencesCopyAppValue(
    env: &mut Environment,
    key: CFStringRef,
    app_id: CFStringRef,
) -> CFPropertyListRef {
    copy_app_value(env, key, app_id)
}

fn CFPreferencesGetAppBooleanValue(
    env: &mut Environment,
    key: CFStringRef,
    app_id: CFStringRef,
    key_exists_and_has_valid_format: MutPtr<u8>, // Boolean*
) -> bool {
    let value = copy_app_value(env, key, app_id);
    let (result, valid) = if value == nil {
        (false, false)
    } else {
        let r = interpret_bool(env, value);
        release(env, value);
        r
    };
    if !key_exists_and_has_valid_format.is_null() {
        env.mem.write(key_exists_and_has_valid_format, valid as u8);
    }
    result
}

fn CFPreferencesGetAppIntegerValue(
    env: &mut Environment,
    key: CFStringRef,
    app_id: CFStringRef,
    key_exists_and_has_valid_format: MutPtr<u8>, // Boolean*
) -> CFIndex {
    let value = copy_app_value(env, key, app_id);
    let (result, valid) = if value == nil {
        (0, false)
    } else {
        let r = interpret_int(env, value);
        release(env, value);
        r
    };
    if !key_exists_and_has_valid_format.is_null() {
        env.mem.write(key_exists_and_has_valid_format, valid as u8);
    }
    result
}

fn CFPreferencesSetAppValue(
    env: &mut Environment,
    key: CFStringRef,
    value: CFPropertyListRef,
    app_id: CFStringRef,
) {
    set_app_value(env, key, value, app_id);
}

fn CFPreferencesAppSynchronize(env: &mut Environment, app_id: CFStringRef) -> bool {
    let _ = app_id_is_current(env, app_id);
    let user_defaults = standard_defaults(env);
    msg![env; user_defaults synchronize]
}

/// Suites are not modelled: every lookup already goes through the single
/// merged `NSUserDefaults` view.
fn CFPreferencesAddSuitePreferencesToApp(
    _env: &mut Environment,
    _app_id: CFStringRef,
    _suite_id: CFStringRef,
) {
    log_dbg!("CFPreferencesAddSuitePreferencesToApp: no-op");
}

fn CFPreferencesRemoveSuitePreferencesFromApp(
    _env: &mut Environment,
    _app_id: CFStringRef,
    _suite_id: CFStringRef,
) {
    log_dbg!("CFPreferencesRemoveSuitePreferencesFromApp: no-op");
}

/// There are no managed (MDM) preferences, so no value is ever forced.
fn CFPreferencesAppValueIsForced(
    _env: &mut Environment,
    _key: CFStringRef,
    _app_id: CFStringRef,
) -> bool {
    false
}

// MARK: - Domain-scoped functions (user/host arguments are ignored)

fn CFPreferencesCopyValue(
    env: &mut Environment,
    key: CFStringRef,
    app_id: CFStringRef,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) -> CFPropertyListRef {
    copy_app_value(env, key, app_id)
}

/// `CFPreferencesCopyMultiple(keysToFetch, ...)` returns an owned dictionary
/// of the requested keys that exist. A NULL `keysToFetch` returns every key.
fn CFPreferencesCopyMultiple(
    env: &mut Environment,
    keys_to_fetch: CFArrayRef,
    app_id: CFStringRef,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) -> CFDictionaryRef {
    let _ = app_id_is_current(env, app_id);
    let user_defaults = standard_defaults(env);
    if keys_to_fetch == nil {
        let all: id = msg![env; user_defaults dictionaryRepresentation];
        if all == nil {
            return nil;
        }
        return msg![env; all mutableCopy];
    }
    let result: id = msg_class![env; NSMutableDictionary new];
    let count: u32 = msg![env; keys_to_fetch count];
    for i in 0..count {
        let key: id = msg![env; keys_to_fetch objectAtIndex:i];
        if key == nil {
            continue;
        }
        let value: id = msg![env; user_defaults objectForKey:key];
        if value != nil {
            () = msg![env; result setObject:value forKey:key];
        }
    }
    result
}

fn CFPreferencesSetValue(
    env: &mut Environment,
    key: CFStringRef,
    value: CFPropertyListRef,
    app_id: CFStringRef,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) {
    set_app_value(env, key, value, app_id);
}

/// Sets every pair in `keysToSet`, then removes every key in `keysToRemove`.
/// Either argument may be NULL.
fn CFPreferencesSetMultiple(
    env: &mut Environment,
    keys_to_set: CFDictionaryRef,
    keys_to_remove: CFArrayRef,
    app_id: CFStringRef,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) {
    let _ = app_id_is_current(env, app_id);
    let user_defaults = standard_defaults(env);
    if keys_to_set != nil {
        let keys: id = msg![env; keys_to_set allKeys];
        let count: u32 = msg![env; keys count];
        for i in 0..count {
            let key: id = msg![env; keys objectAtIndex:i];
            let value: id = msg![env; keys_to_set objectForKey:key];
            if key != nil && value != nil {
                () = msg![env; user_defaults setObject:value forKey:key];
            }
        }
    }
    if keys_to_remove != nil {
        let count: u32 = msg![env; keys_to_remove count];
        for i in 0..count {
            let key: id = msg![env; keys_to_remove objectAtIndex:i];
            if key != nil {
                () = msg![env; user_defaults removeObjectForKey:key];
            }
        }
    }
}

fn CFPreferencesSynchronize(
    env: &mut Environment,
    app_id: CFStringRef,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) -> bool {
    let _ = app_id_is_current(env, app_id);
    let user_defaults = standard_defaults(env);
    msg![env; user_defaults synchronize]
}

/// Returns an owned array of every key set. NULL when there are none.
/// Note that `dictionaryRepresentation` merges all domains, so registered
/// defaults and `AppleLanguages` are included.
fn CFPreferencesCopyKeyList(
    env: &mut Environment,
    app_id: CFStringRef,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) -> CFArrayRef {
    let _ = app_id_is_current(env, app_id);
    let user_defaults = standard_defaults(env);
    let dict: id = msg![env; user_defaults dictionaryRepresentation];
    if dict == nil {
        return nil;
    }
    let keys: id = msg![env; dict allKeys];
    if keys == nil {
        return nil;
    }
    let count: u32 = msg![env; keys count];
    if count == 0 {
        return nil;
    }
    msg![env; keys copy]
}

/// A sandboxed app sees only its own domain, so the list holds just the
/// bundle identifier. The returned array is owned by the caller.
fn CFPreferencesCopyApplicationList(
    env: &mut Environment,
    _user_name: CFStringRef,
    _host_name: CFStringRef,
) -> CFArrayRef {
    let bundle_id = env.bundle.bundle_identifier().to_string();
    let name = ns_string::from_rust_string(env, bundle_id);
    let list: id = msg_class![env; NSArray arrayWithObject:name];
    release(env, name);
    retain(env, list)
}

// MARK: - Constants

pub const kCFPreferencesCurrentApplication: &str = "kCFPreferencesCurrentApplication";
pub const kCFPreferencesAnyApplication: &str = "kCFPreferencesAnyApplication";
pub const kCFPreferencesAnyHost: &str = "kCFPreferencesAnyHost";
pub const kCFPreferencesCurrentHost: &str = "kCFPreferencesCurrentHost";
pub const kCFPreferencesAnyUser: &str = "kCFPreferencesAnyUser";
pub const kCFPreferencesCurrentUser: &str = "kCFPreferencesCurrentUser";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFPreferencesCurrentApplication",
        HostConstant::NSString(kCFPreferencesCurrentApplication),
    ),
    (
        "_kCFPreferencesAnyApplication",
        HostConstant::NSString(kCFPreferencesAnyApplication),
    ),
    (
        "_kCFPreferencesAnyHost",
        HostConstant::NSString(kCFPreferencesAnyHost),
    ),
    (
        "_kCFPreferencesCurrentHost",
        HostConstant::NSString(kCFPreferencesCurrentHost),
    ),
    (
        "_kCFPreferencesAnyUser",
        HostConstant::NSString(kCFPreferencesAnyUser),
    ),
    (
        "_kCFPreferencesCurrentUser",
        HostConstant::NSString(kCFPreferencesCurrentUser),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    // App-scoped
    export_c_func!(CFPreferencesCopyAppValue(_, _)),
    export_c_func!(CFPreferencesGetAppBooleanValue(_, _, _)),
    export_c_func!(CFPreferencesGetAppIntegerValue(_, _, _)),
    export_c_func!(CFPreferencesSetAppValue(_, _, _)),
    export_c_func!(CFPreferencesAddSuitePreferencesToApp(_, _)),
    export_c_func!(CFPreferencesRemoveSuitePreferencesFromApp(_, _)),
    export_c_func!(CFPreferencesAppSynchronize(_)),
    export_c_func!(CFPreferencesAppValueIsForced(_, _)),
    // Domain-scoped
    export_c_func!(CFPreferencesCopyValue(_, _, _, _)),
    export_c_func!(CFPreferencesCopyMultiple(_, _, _, _)),
    export_c_func!(CFPreferencesSetValue(_, _, _, _, _)),
    export_c_func!(CFPreferencesSetMultiple(_, _, _, _, _)),
    export_c_func!(CFPreferencesSynchronize(_, _, _)),
    export_c_func!(CFPreferencesCopyApplicationList(_, _)),
    export_c_func!(CFPreferencesCopyKeyList(_, _, _)),
];
