/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Internal mutex interface.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Instant;

use super::{Environment, ThreadBlock, ThreadId};
use crate::libc::errno::{EBUSY, EDEADLK, EPERM};

/// Stores and manages mutexes. Note that all the methods for locking and
/// unlocking mutexes are on [Environment] instead, because they interact with
/// threads.
#[derive(Default)]
pub struct MutexState {
    // TODO?: Maybe this should be a Vec instead? It would be bad if there were
    // many mutexes over the lifetime of an application, but it would perform
    // better. Maybe it could also be a fixed size allocator? (although that
    // seems a little overkill)
    mutexes: HashMap<MutexId, Mutex>,
    // Hopefully there will never be more than 2^64 mutexes in an application's
    // lifetime :P
    mutex_count: u64,
}

/// Unique identifier for mutexes, used for mutexes held by host objects and
/// guest pthread mutexes.
pub type MutexId = u64;

struct Mutex {
    type_: MutexType,
    waiting_count: u32,
    /// The `NonZeroU32` is the number of locks on this thread (if it's a
    /// recursive mutex).
    locked: Option<(ThreadId, NonZeroU32)>,
    /// Counts of "phantom" no-op locks per thread, created when a NORMAL
    /// mutex self-lock that would deadlock on real iOS is instead allowed to
    /// succeed (see `Environment::lock_mutex`). Each phantom lock must be
    /// paired with a matching unlock that releases nothing, so the guest's
    /// lock/unlock bookkeeping stays consistent with the real lock held by
    /// the original owner.
    phantom_locks: HashMap<ThreadId, u32>,
}

#[repr(i32)]
#[derive(Debug, PartialEq, Copy, Clone)]
#[allow(non_camel_case_types)]
pub enum MutexType {
    PTHREAD_MUTEX_NORMAL = 0,
    PTHREAD_MUTEX_ERRORCHECK = 1,
    PTHREAD_MUTEX_RECURSIVE = 2,
}

impl TryFrom<i32> for MutexType {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(MutexType::PTHREAD_MUTEX_NORMAL),
            1 => Ok(MutexType::PTHREAD_MUTEX_ERRORCHECK),
            2 => Ok(MutexType::PTHREAD_MUTEX_RECURSIVE),
            _ => Err("Value is not a valid mutex type!"),
        }
    }
}
pub const PTHREAD_MUTEX_DEFAULT: MutexType = MutexType::PTHREAD_MUTEX_NORMAL;

impl MutexState {
    /// Initializes a mutex and returns a handle to it. Similar to
    /// `pthread_mutex_init`, but for host code.
    pub fn init_mutex(&mut self, mutex_type: MutexType) -> MutexId {
        let mutex_id = self.mutex_count;
        self.mutex_count = self.mutex_count.checked_add(1).unwrap();
        self.mutexes.insert(
            mutex_id,
            Mutex {
                type_: mutex_type,
                waiting_count: 0,
                locked: None,
                phantom_locks: HashMap::new(),
            },
        );
        log_dbg!("Created mutex #{}, type {:?}", mutex_id, mutex_type);
        mutex_id
    }

    /// Destroys a mutex and returns an error on failure (as errno). Similar to
    /// `pthread_mutex_destroy`, but for host code. Note that the mutex is not
    /// destroyed on an Err return.
    pub fn destroy_mutex(&mut self, mutex_id: MutexId) -> Result<(), i32> {
        let mutex = self.mutexes.get_mut(&mutex_id).unwrap();
        if mutex.locked.is_some() {
            log_dbg!("Attempted to destroy currently locked mutex, returning EBUSY!");
            return Err(EBUSY);
        } else if mutex.waiting_count != 0 {
            log_dbg!("Attempted to destroy mutex with waiting locks, returning EBUSY!");
            return Err(EBUSY);
        }
        // TODO?: If we switch to a vec-based system, we should reuse destroyed
        // ids if they are at the top of the stack.
        self.mutexes.remove(&mutex_id);
        Ok(())
    }

    pub fn mutex_is_locked(&self, mutex_id: MutexId) -> bool {
        self.mutexes
            .get(&mutex_id)
            .is_some_and(|mutex| mutex.locked.is_some())
    }

    pub fn mutex_is_recursive(&self, mutex_id: MutexId) -> bool {
        self.mutexes
            .get(&mutex_id)
            .is_some_and(|mutex| mutex.type_ == MutexType::PTHREAD_MUTEX_RECURSIVE)
    }

    pub fn mutex_is_locked_by(&self, mutex_id: MutexId, thread: ThreadId) -> bool {
        self.mutexes.get(&mutex_id).is_some_and(|mutex| {
            if let Some((lock_thread, _)) = mutex.locked {
                lock_thread == thread
            } else {
                false
            }
        })
    }

    /// Whether `thread` can take `mutex_id` without blocking: it is unlocked,
    /// or it is a recursive mutex `thread` already holds. The latter happens
    /// when a thread waits on a condition variable while holding a recursive
    /// mutex more than once: the wait unlocks it only once, so the thread
    /// still holds it when it wakes, and must not then wait for itself to
    /// release it (Gangstar: Miami Vindication's loader threads).
    pub fn mutex_can_be_locked_by(&self, mutex_id: MutexId, thread: ThreadId) -> bool {
        !self.mutex_is_locked(mutex_id)
            || (self.mutex_is_recursive(mutex_id) && self.mutex_is_locked_by(mutex_id, thread))
    }

    pub(super) fn cancel_mutex_wait(&mut self, mutex_id: MutexId) {
        let mutex = self.mutexes.get_mut(&mutex_id).unwrap();
        mutex.waiting_count = mutex.waiting_count.checked_sub(1).unwrap();
    }
}

impl Environment {
    /// Try immediately, then wait cooperatively for ownership or a guest
    /// deadline. A timeout removes the waiter without acquiring the mutex.
    pub fn lock_mutex_until(&mut self, mutex_id: MutexId, due: Instant) -> bool {
        if self.mutex_state.mutex_can_be_locked_by(mutex_id, self.current_thread) {
            return self.lock_mutex(mutex_id).is_ok();
        }
        if due <= self.guest_clock.now() {
            return false;
        }
        self.mutex_state.mutexes.get_mut(&mutex_id).unwrap().waiting_count += 1;
        self.threads[self.current_thread].timed_mutex_acquired = false;
        self.yield_thread(ThreadBlock::TimedMutex(mutex_id, due));
        self.threads[self.current_thread].timed_mutex_acquired
    }

    /// Relock mutex that was just unblocked. This should probably only be used
    /// by the thread scheduler.
    pub fn relock_unblocked_mutex_for_thread(&mut self, thread_id: ThreadId, mutex_id: MutexId) {
        log_dbg!(
            "Relocking unblocked mutex {} for thread {}, waiting count {}",
            mutex_id,
            self.current_thread,
            self.mutex_state
                .mutexes
                .get_mut(&mutex_id)
                .unwrap()
                .waiting_count
        );
        let mutex: &mut _ = self.mutex_state.mutexes.get_mut(&mutex_id).unwrap();
        mutex.locked = match mutex.locked {
            None => Some((thread_id, NonZeroU32::new(1).unwrap())),
            // See `mutex_can_be_locked_by`.
            Some((locking_thread, lock_count))
                if locking_thread == thread_id
                    && mutex.type_ == MutexType::PTHREAD_MUTEX_RECURSIVE =>
            {
                Some((locking_thread, lock_count.checked_add(1).unwrap()))
            }
            Some(_) => panic!("Mutex #{mutex_id} is locked by another thread"),
        };
        if self
            .mutex_state
            .mutexes
            .get_mut(&mutex_id)
            .unwrap()
            .waiting_count
            > 0
        {
            self.mutex_state
                .mutexes
                .get_mut(&mutex_id)
                .unwrap()
                .waiting_count -= 1;
        }
    }

    /// Locks a mutex and returns the lock count or an error (as errno). Similar
    /// to `pthread_mutex_lock`, but for host code.
    pub fn lock_mutex(&mut self, mutex_id: MutexId) -> Result<u32, i32> {
        let current_thread = self.current_thread;
        let mutex: &mut _ = self.mutex_state.mutexes.get_mut(&mutex_id).unwrap();

        let Some((locking_thread, lock_count)) = mutex.locked else {
            log_dbg!("Locked mutex #{} for thread {}.", mutex_id, current_thread);
            mutex.locked = Some((current_thread, NonZeroU32::new(1).unwrap()));
            return Ok(1);
        };

        if locking_thread == current_thread {
            match mutex.type_ {
                MutexType::PTHREAD_MUTEX_NORMAL => {
                    // POSIX says behaviour is undefined here; on real iOS the
                    // guest would deadlock forever. We can't deadlock a single
                    // host thread safely, and returning an error is actively
                    // harmful: guests rarely check pthread_mutex_lock's return
                    // value, and the failed lock sends them into corrupted
                    // lock/unlock states that end in guest aborts (observed
                    // with N.O.V.A. 3, which spun a mutex-unlock loop for
                    // seconds after receiving EDEADLK here and then aborted).
                    // We grant a "phantom" lock: it succeeds without changing
                    // real ownership, and is consumed by a matching unlock
                    // (see `unlock_mutex`), so the guest's lock/unlock pair
                    // bookkeeping stays balanced without touching the lock
                    // actually held by the other thread.
                    static SELF_LOCK_LOGGED: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let bit = 1u64 << (mutex_id % 64);
                    let logged =
                        SELF_LOCK_LOGGED.fetch_or(bit, std::sync::atomic::Ordering::Relaxed);
                    if logged & bit == 0 {
                        log!(
                            "Warning: pthread_mutex_lock: non-error-checking mutex #{mutex_id} would deadlock on thread {current_thread}; granting phantom lock instead (real iOS would deadlock here).",
                        );
                    }
                    *mutex
                        .phantom_locks
                        .entry(current_thread)
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                    return Ok(1);
                }
                MutexType::PTHREAD_MUTEX_ERRORCHECK => {
                    log_dbg!("Attempted to lock error-checking mutex #{} for thread {}, already locked by same thread! Returning EDEADLK.", mutex_id, current_thread);
                    return Err(EDEADLK);
                }
                MutexType::PTHREAD_MUTEX_RECURSIVE => {
                    log_dbg!(
                        "Increasing lock level on recursive mutex #{}, currently locked by thread {}.",
                        mutex_id,
                        locking_thread,
                    );
                    mutex.locked = Some((locking_thread, lock_count.checked_add(1).unwrap()));
                    return Ok(lock_count.get() + 1);
                }
            }
        }

        // Add to the waiting count, so that the mutex isn't destroyed. This is
        // subtracted in relock_unblocked_mutex.
        mutex.waiting_count += 1;

        // Mutex is already locked, block thread until it isn't.
        self.block_on_mutex(mutex_id);
        // Lock count is always 1 after a thread-blocking lock.
        Ok(1)
    }

    /// Unlocks a mutex and returns the lock count or an error (as errno).
    /// Similar to `pthread_mutex_unlock`, but for host code.
    pub fn unlock_mutex(&mut self, mutex_id: MutexId) -> Result<u32, i32> {
        let current_thread = self.current_thread;
        let mutex: &mut _ = self.mutex_state.mutexes.get_mut(&mutex_id).unwrap();

        // If this thread holds a phantom lock (granted when a NORMAL mutex
        // self-lock that would deadlock was allowed to succeed instead), an
        // unlock just consumes the phantom and releases nothing: the real
        // lock belongs to whichever thread owns it.
        if let Some(phantom_count) = mutex.phantom_locks.get_mut(&current_thread) {
            if *phantom_count > 0 {
                *phantom_count -= 1;
                if *phantom_count == 0 {
                    mutex.phantom_locks.remove(&current_thread);
                }
                log_dbg!(
                    "Consumed phantom lock on mutex #{} for thread {}.",
                    mutex_id,
                    current_thread
                );
                return Ok(0);
            }
        }

        let Some((locking_thread, lock_count)) = mutex.locked else {
            match mutex.type_ {
                MutexType::PTHREAD_MUTEX_NORMAL => {
                    // Убираем panic!, так как реальные iOS-игры часто пытаются
                    // разблокировать уже разблокированные мьютексы.
                    log_dbg!(
                        "Warning: Attempted to unlock non-error-checking mutex #{mutex_id} for thread {current_thread}, already unlocked! Ignoring and returning EPERM.",
                    );
                    return Err(EPERM);
                }
                MutexType::PTHREAD_MUTEX_ERRORCHECK | MutexType::PTHREAD_MUTEX_RECURSIVE => {
                    log_dbg!(
                        "Attempted to unlock error-checking or recursive mutex #{} for thread {}, already unlocked! Returning EPERM.",
                        mutex_id, current_thread,
                    );
                    return Err(EPERM);
                }
            }
        };

        if locking_thread != current_thread {
            match mutex.type_ {
                MutexType::PTHREAD_MUTEX_NORMAL => {
                    // This case is undefined,
                    // but tests on macOS/iOS shows it is allowed!
                    // Logging is capped: games like N.O.V.A. 3 unlock this mutex
                    // from another thread every frame, flooding the log.
                    static CROSS_UNLOCK_LOGGED: std::sync::atomic::AtomicU32 =
                        std::sync::atomic::AtomicU32::new(0);
                    let n = CROSS_UNLOCK_LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if n < 8 || n == 1000 {
                        log!(
                            "Warning: Allowing to unlock non-error-checking mutex #{mutex_id} for thread {current_thread}, locked by different thread {locking_thread}! (occurrence {})",
                            n + 1
                        );
                    }
                }
                MutexType::PTHREAD_MUTEX_ERRORCHECK | MutexType::PTHREAD_MUTEX_RECURSIVE => {
                    log_dbg!(
                        "Attempted to unlock error-checking or recursive mutex #{} for thread {}, locked by different thread {}! Returning EPERM.",
                        mutex_id, current_thread, locking_thread,
                    );
                    return Err(EPERM);
                }
            }
        }

        if lock_count.get() == 1 {
            log_dbg!(
                "Unlocked mutex #{} for thread {}.",
                mutex_id,
                current_thread
            );
            mutex.locked = None;
            Ok(0)
        } else {
            assert!(mutex.type_ == MutexType::PTHREAD_MUTEX_RECURSIVE);
            log_dbg!(
                "Decreasing lock level on recursive mutex #{} (count {}), currently locked by thread {}.",
                mutex_id,
                lock_count,
                locking_thread
            );
            mutex.locked = Some((
                locking_thread,
                NonZeroU32::new(lock_count.get() - 1).unwrap(),
            ));
            Ok(lock_count.get() - 1)
        }
    }
}
