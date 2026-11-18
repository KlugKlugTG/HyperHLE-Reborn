/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFNotificationCenter.h`
//!
//! On iOS the local notification center and `NSNotificationCenter`'s
//! default center are the same object, so this is implemented on top of
//! that: every center reference handed out is the `NSNotificationCenter`
//! singleton, and CF-style (callback-based) observers are stored alongside
//! the selector/block ones, so a notification posted through either API
//! reaches both flavours of observer. See
//! `crate::frameworks::foundation::ns_notification_center`.

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::{CFTypeRef, CFOptionFlags};
use crate::frameworks::foundation::ns_notification_center::{self, CfObserver};
use crate::frameworks::foundation::ns_string;
use crate::objc::{msg, msg_class, nil, release, id};
use crate::Environment;
use std::borrow::Cow;

pub type CFNotificationCenterRef = CFTypeRef;

pub(super) fn CFNotificationCenterGetLocalCenter(env: &mut Environment) -> CFNotificationCenterRef {
    // iOS: the local center is NSNotificationCenter's default center.
    msg_class![env; NSNotificationCenter defaultCenter]
}

/// `name == nil` means "any name", both when observing and when removing.
fn cf_name_to_option(env: &mut Environment, name: CFStringRef) -> Option<Cow<'static, str>> {
    if name == nil {
        None
    } else {
        Some(ns_string::to_rust_string(env, name))
    }
}

pub(super) fn CFNotificationCenterAddObserver(
    env: &mut Environment,
    center: CFNotificationCenterRef,
    observer: id, // const void *observer
    call_back: GuestFunction,
    name: CFStringRef,
    object: id, // const void *object
    _suspension_behavior: u32, // CFNotificationSuspensionBehavior
) {
    if center == nil {
        log!("Warning: CFNotificationCenterAddObserver with NULL center ignored");
        return;
    }
    if call_back.to_ptr().is_null() {
        log!("Warning: CFNotificationCenterAddObserver with NULL callback ignored");
        return;
    }
    // Per CF semantics neither the observer nor the object is retained
    // (and hence nothing is released on removal either).
    let name = cf_name_to_option(env, name);
    log_dbg!(
        "CFNotificationCenterAddObserver(center:{:?} observer:{:?} name:{:?} object:{:?})",
        center,
        observer,
        name,
        object
    );
    ns_notification_center::add_cf_observer(
        env,
        center,
        CfObserver {
            observer,
            callback: call_back,
            name,
            object,
        },
    );
}

pub(super) fn CFNotificationCenterRemoveObserver(
    env: &mut Environment,
    center: CFNotificationCenterRef,
    observer: id,
    name: CFStringRef,
    object: id, // const void *object
) {
    if center == nil {
        return;
    }
    let name = cf_name_to_option(env, name);
    log_dbg!(
        "CFNotificationCenterRemoveObserver(center:{:?} observer:{:?} name:{:?} object:{:?})",
        center,
        observer,
        name,
        object
    );
    ns_notification_center::remove_cf_observer(env, center, observer, name, object);
}

pub(super) fn CFNotificationCenterRemoveEveryObserver(
    env: &mut Environment,
    center: CFNotificationCenterRef,
    observer: id,
) {
    if center == nil {
        return;
    }
    log_dbg!(
        "CFNotificationCenterRemoveEveryObserver(center:{:?} observer:{:?})",
        center,
        observer
    );
    ns_notification_center::remove_every_cf_observer(env, center, observer);
}

pub(super) fn CFNotificationCenterPostNotification(
    env: &mut Environment,
    center: CFNotificationCenterRef,
    name: CFStringRef,
    object: id,  // const void *object
    user_info: id, // void *userInfo (CFDictionaryRef)
    _deliver_immediately: bool,
) {
    post_notification(env, center, name, object, user_info);
}

pub(super) fn CFNotificationCenterPostNotificationWithOptions(
    env: &mut Environment,
    center: CFNotificationCenterRef,
    name: CFStringRef,
    object: id,
    user_info: id,
    _options: CFOptionFlags,
    _deliver_time: f64, // CFTimeInterval
) {
    // touchHLE always delivers notifications synchronously, so the
    // posting options and the delivery time make no difference here.
    post_notification(env, center, name, object, user_info);
}

fn post_notification(
    env: &mut Environment,
    center: CFNotificationCenterRef,
    name: CFStringRef,
    object: id,
    user_info: id,
) {
    if center == nil {
        log!("Warning: CFNotificationCenterPostNotification with NULL center ignored");
        return;
    }
    if name == nil {
        log!("Warning: CFNotificationCenterPostNotification with NULL name ignored");
        return;
    }
    log_dbg!(
        "CFNotificationCenterPostNotification(center:{:?} name:{:?} object:{:?} userInfo:{:?})",
        center,
        ns_string::to_rust_string(env, name),
        object,
        user_info
    );
    // Wrap the payload in an NSNotification and post it through the
    // shared center, so selector-, block- and CF-callback observers all
    // get it.
    let notification: id = msg_class![env; NSNotification alloc];
    let notification: id = msg![env; notification initWithName:name
                                                        object:object
                                                      userInfo:user_info];
    let _: () = msg![env; center postNotification:notification];
    release(env, notification);
}

// `CFNotificationSuspensionBehavior` and the posting options are plain
// enums in the C header, not linkable symbols, so they are Rust constants only.
#[allow(dead_code)]
pub const CFNotificationSuspensionBehaviorDrop: u32 = 1;
#[allow(dead_code)]
pub const CFNotificationSuspensionBehaviorCoalesce: u32 = 2;
#[allow(dead_code)]
pub const CFNotificationSuspensionBehaviorHold: u32 = 3;
#[allow(dead_code)]
pub const CFNotificationSuspensionBehaviorDeliverImmediately: u32 = 4;
#[allow(dead_code)]
pub const kCFNotificationDeliverImmediately: CFOptionFlags = 1 << 0;
#[allow(dead_code)]
pub const kCFNotificationPostToAllSessions: CFOptionFlags = 1 << 1;

// touchHLE has a single notification center and no other processes, so the
// Darwin and distributed centers resolve to the same object as the local one.
// Observers registered on either flavour still fire for posts made through
// any of them.
fn CFNotificationCenterGetDarwinNotifyCenter(env: &mut Environment) -> CFNotificationCenterRef {
    CFNotificationCenterGetLocalCenter(env)
}

fn CFNotificationCenterGetDistributedCenter(env: &mut Environment) -> CFNotificationCenterRef {
    CFNotificationCenterGetLocalCenter(env)
}

// Placeholder type ID ('CFNC'), in the same spirit as the fake IDs used for
// CFString and CFStringTokenizer.
fn CFNotificationCenterGetTypeID(_env: &mut Environment) -> u32 {
    0x43464e43
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFNotificationCenterGetLocalCenter()),
    export_c_func!(CFNotificationCenterAddObserver(_, _, _, _, _, _)),
    export_c_func!(CFNotificationCenterRemoveObserver(_, _, _, _)),
    export_c_func!(CFNotificationCenterRemoveEveryObserver(_, _)),
    export_c_func!(CFNotificationCenterPostNotification(_, _, _, _, _)),
    export_c_func!(CFNotificationCenterPostNotificationWithOptions(_, _, _, _, _, _)),
    export_c_func!(CFNotificationCenterGetDarwinNotifyCenter()),
    export_c_func!(CFNotificationCenterGetDistributedCenter()),
    export_c_func!(CFNotificationCenterGetTypeID()),
];
