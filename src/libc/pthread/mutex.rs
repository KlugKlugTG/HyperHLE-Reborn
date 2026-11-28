/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Guest mutex interface.
//!
//! See [crate::environment::mutex] for the internal implementation.
#![allow(rustdoc::broken_intra_doc_links)] // https://github.com/rust-lang/rust/issues/83049

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::{EBUSY, EINVAL};
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::{Environment, MutexId, MutexType, PTHREAD_MUTEX_DEFAULT};

/// Apple's implementation is a 4-byte magic number followed by an 8-byte opaque
/// region. We only have to match the size theirs has.
#[repr(C, packed)]
pub struct pthread_mutexattr_t {
    /// Magic number (must be [MAGIC_MUTEXATTR])
    magic: u32,
    type_: i32,
    /// This should eventually be a bitfield with the other attributes.
    _unused: u32,
}
unsafe impl SafeRead for pthread_mutexattr_t {}

/// Apple's implementation is a 4-byte magic number followed by a 56-byte opaque
/// region. We will store the actual data on the host, determined by a mutex
/// identifier.
#[repr(C, packed)]
pub struct pthread_mutex_t {
    /// Magic number (must be [MAGIC_MUTEX])
    magic: u32,
    /// Unique mutex identifier, used in matching the mutex to it's host object.
    pub mutex_id: MutexId,
}
unsafe impl SafeRead for pthread_mutex_t {}

/// Arbitrarily-chosen magic number for `pthread_mutexattr_t` (not Apple's).
const MAGIC_MUTEXATTR: u32 = u32::from_be_bytes(*b"MuAt");
/// Arbitrarily-chosen magic number for `pthread_mutex_t` (not Apple's).
const MAGIC_MUTEX: u32 = u32::from_be_bytes(*b"MUTX");
/// Signatures written by the `PTHREAD_*_MUTEX_INITIALIZER` macros. These are
/// part of the ABI: a statically-initialized guest `pthread_mutex_t` carries
/// one of these in its first word until the first lock, at which point real
/// libpthread lazily finishes setup. We do the same, registering a host mutex
/// of the matching type. See Apple's libpthread `src/pthread_mutex.c`.
const MAGIC_MUTEX_STATIC: u32 = 0x32AAABA7; // PTHREAD_MUTEX_INITIALIZER (normal)
const MAGIC_MUTEX_STATIC_ERRORCHECK: u32 = 0x32AAABA1; // PTHREAD_ERRORCHECK_MUTEX_INITIALIZER
const MAGIC_MUTEX_STATIC_RECURSIVE: u32 = 0x32AAABA2; // PTHREAD_RECURSIVE_MUTEX_INITIALIZER
const MAGIC_MUTEX_STATIC_FIRSTFIT: u32 = 0x32AAABA3; // first-fit; behaves as normal for us

#[allow(dead_code)]
const PTHREAD_PROCESS_SHARED: i32 = 1;
const PTHREAD_PROCESS_PRIVATE: i32 = 2;

fn pthread_mutexattr_init(env: &mut Environment, attr: MutPtr<pthread_mutexattr_t>) -> i32 {
    env.mem.write(
        attr,
        pthread_mutexattr_t {
            magic: MAGIC_MUTEXATTR,
            type_: PTHREAD_MUTEX_DEFAULT as i32,
            _unused: 0,
        },
    );
    0 // success
}
fn pthread_mutexattr_setpshared(
    env: &mut Environment,
    attr: MutPtr<pthread_mutexattr_t>,
    pshared: i32,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    // PTHREAD_PROCESS_PRIVATE is a default one
    // TODO: set this attribute in `attr` instead
    assert_eq!(pshared, PTHREAD_PROCESS_PRIVATE);
    0 // success
}
fn pthread_mutexattr_settype(
    env: &mut Environment,
    attr: MutPtr<pthread_mutexattr_t>,
    type_: i32,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    let mut attr_copy = env.mem.read(attr);
    attr_copy.type_ = type_;
    env.mem.write(attr, attr_copy);
    0 // success
}
fn pthread_mutexattr_gettype(
    env: &mut Environment,
    attr: ConstPtr<pthread_mutexattr_t>,
    type_out: MutPtr<i32>,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    if type_out.is_null() {
        return EINVAL;
    }
    let attr_copy = env.mem.read(attr);
    env.mem.write(type_out, attr_copy.type_);
    0 // success
}
/// `pthread_mutexattr_setprotocol(attr, protocol)` — Apple's
/// `<pthread/pthread.h>` declares this for POSIX priority-inheritance
/// support. Permitted protocols are:
///   - `PTHREAD_PRIO_NONE     = 0` (no priority changes — Apple default)
///   - `PTHREAD_PRIO_INHERIT  = 1` (priority inheritance)
///   - `PTHREAD_PRIO_PROTECT  = 2` (priority protection / ceiling)
///
/// touchHLE is single-process and runs on the host scheduler; priority
/// boosts are not user-observable. We therefore validate the protocol
/// argument like Apple does and treat the setting as a no-op, returning
/// `0` on success and `EINVAL` for unknown protocols. See
/// <https://opensource.apple.com/source/libpthread/libpthread-301.30.1/src/pthread_mutex.c.auto.html>.
const PTHREAD_PRIO_NONE: i32 = 0;
const PTHREAD_PRIO_INHERIT: i32 = 1;
const PTHREAD_PRIO_PROTECT: i32 = 2;

fn pthread_mutexattr_setprotocol(
    env: &mut Environment,
    attr: MutPtr<pthread_mutexattr_t>,
    protocol: i32,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    match protocol {
        PTHREAD_PRIO_NONE | PTHREAD_PRIO_INHERIT | PTHREAD_PRIO_PROTECT => 0,
        _ => EINVAL,
    }
}

fn pthread_mutexattr_getprotocol(
    env: &mut Environment,
    attr: ConstPtr<pthread_mutexattr_t>,
    out: MutPtr<i32>,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    if out.is_null() {
        return EINVAL;
    }
    // We don't store the protocol; always report Apple's default.
    env.mem.write(out, PTHREAD_PRIO_NONE);
    0
}

/// `pthread_mutexattr_setprioceiling(attr, ceiling)` — companion to
/// the protocol API. Apple validates the value against the host's
/// priority range; with no scheduler under us this is best-effort.
fn pthread_mutexattr_setprioceiling(
    env: &mut Environment,
    attr: MutPtr<pthread_mutexattr_t>,
    _ceiling: i32,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    0
}

fn pthread_mutexattr_getprioceiling(
    env: &mut Environment,
    attr: ConstPtr<pthread_mutexattr_t>,
    out: MutPtr<i32>,
) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    if out.is_null() {
        return EINVAL;
    }
    env.mem.write(out, 0);
    0
}

fn pthread_mutexattr_destroy(env: &mut Environment, attr: MutPtr<pthread_mutexattr_t>) -> i32 {
    check_magic!(env, attr, MAGIC_MUTEXATTR);
    env.mem.write(
        attr,
        pthread_mutexattr_t {
            magic: 0,
            type_: PTHREAD_MUTEX_DEFAULT as i32,
            _unused: 0,
        },
    );
    0 // success
}

pub fn pthread_mutex_init(
    env: &mut Environment,
    mutex: MutPtr<pthread_mutex_t>,
    attr: ConstPtr<pthread_mutexattr_t>,
) -> i32 {
    let type_ = if !attr.is_null() {
        check_magic!(env, attr, MAGIC_MUTEXATTR);
        let pthread_mutexattr_t { type_, .. } = env.mem.read(attr);
        type_.try_into().unwrap()
    } else {
        PTHREAD_MUTEX_DEFAULT
    };
    let mutex_id = env.mutex_state.init_mutex(type_);
    log_dbg!(
        "Mutex #{} created from pthread_mutex_init ({:#x})",
        mutex_id,
        mutex.to_bits()
    );
    env.mem.write(
        mutex,
        pthread_mutex_t {
            magic: MAGIC_MUTEX,
            mutex_id,
        },
    );

    0 // success
}

/// Darwin recognizes static mutex initializers on first use. Zero-filled or
/// clobbered storage has an invalid signature and must be left untouched.
/// https://github.com/apple-oss-distributions/Libc/blob/Libc-594.1.4/pthreads/pthread_mutex.c
fn mutex_id(env: &mut Environment, mutex: MutPtr<pthread_mutex_t>) -> Option<MutexId> {
    if mutex.is_null() {
        return None;
    }
    let data = env.mem.read(mutex);
    let type_ = match data.magic {
        MAGIC_MUTEX => {
            return env
                .mutex_state
                .contains_mutex(data.mutex_id)
                .then_some(data.mutex_id)
        }
        MAGIC_MUTEX_STATIC | MAGIC_MUTEX_STATIC_FIRSTFIT => MutexType::PTHREAD_MUTEX_NORMAL,
        MAGIC_MUTEX_STATIC_ERRORCHECK => MutexType::PTHREAD_MUTEX_ERRORCHECK,
        MAGIC_MUTEX_STATIC_RECURSIVE => MutexType::PTHREAD_MUTEX_RECURSIVE,
        _ => return None,
    };
    let mutex_id = env.mutex_state.init_mutex(type_);
    env.mem.write(
        mutex,
        pthread_mutex_t {
            magic: MAGIC_MUTEX,
            mutex_id,
        },
    );
    Some(mutex_id)
}

pub fn pthread_mutex_lock(env: &mut Environment, mutex: MutPtr<pthread_mutex_t>) -> i32 {
    let Some(id) = mutex_id(env, mutex) else {
        return EINVAL;
    };
    env.lock_mutex(id).err().unwrap_or(0)
}

pub fn pthread_mutex_trylock(env: &mut Environment, mutex: MutPtr<pthread_mutex_t>) -> i32 {
    let Some(id) = mutex_id(env, mutex) else {
        return EINVAL;
    };
    if !env
        .mutex_state
        .mutex_can_be_locked_by(id, env.current_thread)
    {
        return EBUSY;
    }
    env.lock_mutex(id).err().unwrap_or(0)
}

pub fn pthread_mutex_unlock(env: &mut Environment, mutex: MutPtr<pthread_mutex_t>) -> i32 {
    let Some(id) = mutex_id(env, mutex) else {
        return EINVAL;
    };
    env.unlock_mutex(id).err().unwrap_or(0)
}

fn pthread_mutex_destroy(env: &mut Environment, mutex: MutPtr<pthread_mutex_t>) -> i32 {
    let Some(id) = mutex_id(env, mutex) else {
        return EINVAL;
    };
    if let Err(error) = env.mutex_state.destroy_mutex(id) {
        return error;
    }
    env.mem.write(
        mutex,
        pthread_mutex_t {
            magic: 0,
            mutex_id: 0,
        },
    );
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(pthread_mutexattr_init(_)),
    export_c_func!(pthread_mutexattr_setpshared(_, _)),
    export_c_func!(pthread_mutexattr_settype(_, _)),
    export_c_func!(pthread_mutexattr_gettype(_, _)),
    export_c_func!(pthread_mutexattr_destroy(_)),
    export_c_func!(pthread_mutexattr_setprotocol(_, _)),
    export_c_func!(pthread_mutexattr_getprotocol(_, _)),
    export_c_func!(pthread_mutexattr_setprioceiling(_, _)),
    export_c_func!(pthread_mutexattr_getprioceiling(_, _)),
    export_c_func!(pthread_mutex_init(_, _)),
    export_c_func!(pthread_mutex_lock(_)),
    export_c_func!(pthread_mutex_trylock(_)),
    export_c_func!(pthread_mutex_unlock(_)),
    export_c_func!(pthread_mutex_destroy(_)),
];
