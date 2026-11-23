/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSLock family`.
//!
//! TODO: There is probably an opportunity to refactor common methods.
//! (Need to find a good way to do so! Common super class wouldn't
//! work as it breaks expected inheritance chain.)

use crate::environment::MutexType::PTHREAD_MUTEX_RECURSIVE;
use crate::environment::{Environment, MutexId, PTHREAD_MUTEX_DEFAULT};
use crate::msg;
use crate::objc::{id, nil, objc_classes, ClassExports, HostObject};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

// NSInteger is i32 on 32-bit ARM.
type NSInteger = i32;

#[derive(Default)]
struct NSLockHostObject {
    mutex_id: MutexId,
    name: id,
}
impl HostObject for NSLockHostObject {}

// ---------------------------------------------------------------------------
// NSConditionLock
// ---------------------------------------------------------------------------

struct NSConditionLockState {
    locked: bool,
    condition: NSInteger,
}

struct NSConditionLockHostObject {
    /// Arc lets lock methods clone the reference, drop the ObjC borrow,
    /// and then block on the condvar without holding a borrow on env.objc.
    inner: Arc<(Mutex<NSConditionLockState>, Condvar)>,
}
impl HostObject for NSConditionLockHostObject {}

// ---------------------------------------------------------------------------

/// Try first, including for an expired deadline, as Foundation does.
/// Waiters use the guest scheduler so the owning thread can release the lock.
/// https://github.com/swiftlang/swift-corelibs-foundation/blob/main/Sources/Foundation/NSLock.swift
fn lock_before_date(env: &mut Environment, lock: id, date: id) -> bool {
    let mutex_id = env.objc.borrow::<NSLockHostObject>(lock).mutex_id;
    if env
        .mutex_state
        .mutex_can_be_locked_by(mutex_id, env.current_thread)
    {
        return env.lock_mutex(mutex_id).is_ok();
    }
    let remaining: f64 = msg![env; date timeIntervalSinceNow];
    if !remaining.is_finite() || remaining <= 0.0 {
        return false;
    }
    let Ok(remaining) = Duration::try_from_secs_f64(remaining) else {
        return false;
    };
    let Some(due) = env.guest_clock.now().checked_add(remaining) else {
        return false;
    };
    env.lock_mutex_until(mutex_id, due)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSLock: NSObject

+ (id)alloc {
    log_dbg!("[NSLock alloc]");
    let mutex_id = env.mutex_state.init_mutex(PTHREAD_MUTEX_DEFAULT);
    let host_object = NSLockHostObject { mutex_id, name: nil };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

// NSLocking protocol implementation
- (())lock {
    log_dbg!("[(NSLock *){:?} lock]", this);
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    env.lock_mutex(host_object.mutex_id).unwrap();
}
- (())unlock {
    log_dbg!("[(NSLock *){:?} unlock]", this);
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    if !env.mutex_state.mutex_is_locked(host_object.mutex_id) {
        echo!("*** -[NSLock unlock]: lock (<NSLock: {:?}> '{:?}') unlocked when not locked", this, host_object.name);
    }
    if let Err(e) = env.unlock_mutex(host_object.mutex_id) {
        // Error 16 = EBUSY, Error 1 = EPERM. On real iOS these are
        // non-fatal (the unlock simply fails silently for NSLock).
        // Apps like EnbornX trigger this during shutdown sequences
        // when locks are released out of order.
        log_dbg!(
            "*** -[NSLock unlock]: unlock_mutex returned error {} for lock {:?}, ignoring",
            e, this
        );
    }
}

- (bool)tryLock {
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    let mutex_id = host_object.mutex_id;
    env.mutex_state.mutex_can_be_locked_by(mutex_id, env.current_thread)
        && env.lock_mutex(mutex_id).is_ok()
}

// https://developer.apple.com/documentation/foundation/nslock/lock(before:)
- (bool)lockBeforeDate:(id)date {
    lock_before_date(env, this, date)
}

- (())setName:(id)name { // NSString *
    // @property(copy), name has to be copied
    env.objc.borrow_mut::<NSLockHostObject>(this).name = msg![env; name copy];
}
- (id)name {
    env.objc.borrow::<NSLockHostObject>(this).name
}

- (())dealloc {
    log_dbg!("[(NSLock *){:?} dealloc]", this);
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    let _ = env.mutex_state.destroy_mutex(host_object.mutex_id);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation NSRecursiveLock: NSObject

+ (id)alloc {
    log_dbg!("[NSRecursiveLock alloc]");
    let mutex_id = env.mutex_state.init_mutex(PTHREAD_MUTEX_RECURSIVE);
    let host_object = NSLockHostObject { mutex_id, name: nil };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

// NSLocking protocol implementation
- (())lock {
    log_dbg!("[(NSRecursiveLock *){:?} lock]", this);
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    env.lock_mutex(host_object.mutex_id).unwrap();
}
- (())unlock {
    log_dbg!("[(NSRecursiveLock *){:?} unlock]", this);
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    if !env.mutex_state.mutex_is_locked(host_object.mutex_id) {
        echo!("*** -[NSRecursiveLock unlock]: lock (<NSRecursiveLock: {:?}> '{:?}') unlocked when not locked", this, host_object.name);
    }
    if let Err(e) = env.unlock_mutex(host_object.mutex_id) {
        log_dbg!(
            "*** -[NSRecursiveLock unlock]: unlock_mutex returned error {} for lock {:?}, ignoring",
            e, this
        );
    }
}

- (bool)tryLock {
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    let mutex_id = host_object.mutex_id;
    env.mutex_state.mutex_can_be_locked_by(mutex_id, env.current_thread)
        && env.lock_mutex(mutex_id).is_ok()
}

// https://developer.apple.com/documentation/foundation/nslock/lock(before:)
- (bool)lockBeforeDate:(id)date {
    lock_before_date(env, this, date)
}

- (())setName:(id)name { // NSString *
    // @property(copy), name has to be copied
    env.objc.borrow_mut::<NSLockHostObject>(this).name = msg![env; name copy];
}
- (id)name {
    env.objc.borrow::<NSLockHostObject>(this).name
}

- (())dealloc {
    log_dbg!("[(NSRecursiveLock *){:?} dealloc]", this);
    let host_object = env.objc.borrow::<NSLockHostObject>(this);
    let _ = env.mutex_state.destroy_mutex(host_object.mutex_id);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end


};
