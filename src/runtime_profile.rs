/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Opt-in wall-time accounting for guest execution, HLE calls and presentation.
//! Tracks a separate scope stack for every guest coroutine: a suspended HLE
//! call must not be charged for work executed on another guest thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Counter {
    calls: u64,
    elapsed: Duration,
}

struct Profile {
    stacks: HashMap<usize, Vec<&'static str>>,
    current: usize,
    counters: HashMap<&'static str, Counter>,
    last: Instant,
    reported: Instant,
}

impl Profile {
    fn new(now: Instant) -> Self {
        Self {
            stacks: HashMap::new(),
            current: 0,
            counters: HashMap::new(),
            last: now,
            reported: now,
        }
    }

    fn charge(&mut self, now: Instant) {
        let category = self
            .stacks
            .get(&self.current)
            .and_then(|stack| stack.last())
            .copied()
            .unwrap_or("scheduler");
        self.counters.entry(category).or_default().elapsed += now.duration_since(self.last);
        self.last = now;
    }

    fn push(&mut self, thread: usize, name: &'static str, now: Instant) {
        self.charge(now);
        self.current = thread;
        self.stacks.entry(thread).or_default().push(name);
        self.counters.entry(name).or_default().calls += 1;
    }

    fn pop(&mut self, thread: usize, now: Instant) {
        self.charge(now);
        self.stacks.get_mut(&thread).unwrap().pop();
    }
}

thread_local! {
    static PROFILE: RefCell<Option<Profile>> = const { RefCell::new(None) };
}

fn enabled() -> bool {
    crate::env_flag_cached!("TOUCHHLE_PROFILE_RUNTIME")
}

fn with_profile(f: impl FnOnce(&mut Profile, Instant)) {
    let now = Instant::now();
    PROFILE.with(|profile| {
        let mut profile = profile.borrow_mut();
        f(profile.get_or_insert_with(|| Profile::new(now)), now);
    });
}

pub(crate) struct Scope {
    thread: usize,
    enabled: bool,
}

impl Drop for Scope {
    fn drop(&mut self) {
        if self.enabled {
            with_profile(|profile, now| profile.pop(self.thread, now));
        }
    }
}

pub(crate) fn scope(thread: usize, name: &'static str) -> Scope {
    let enabled = enabled();
    if enabled {
        with_profile(|profile, now| profile.push(thread, name, now));
    }
    Scope { thread, enabled }
}

pub(crate) fn switch_thread(thread: usize) {
    if enabled() {
        with_profile(|profile, now| {
            profile.charge(now);
            profile.current = thread;
        });
    }
}

pub(crate) fn report() {
    if !enabled() {
        return;
    }
    with_profile(|profile, now| {
        let interval = now.duration_since(profile.reported);
        if interval < Duration::from_secs(2) {
            return;
        }
        profile.charge(now);
        let mut counters: Vec<_> = profile.counters.drain().collect();
        counters.sort_by_key(|(_, counter)| std::cmp::Reverse(counter.elapsed));
        log!(
            "RUNTIME-PROFILE interval_ms={:.3}",
            interval.as_secs_f64() * 1000.0
        );
        for (name, counter) in counters {
            log!(
                "RUNTIME-PROFILE category={} calls={} self_ms={:.3}",
                name,
                counter.calls,
                counter.elapsed.as_secs_f64() * 1000.0
            );
        }
        profile.reported = now;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspended_coroutine_does_not_own_other_threads_work() {
        let now = Instant::now();
        let mut profile = Profile::new(now);
        profile.push(0, "waiter", now);
        profile.charge(now + Duration::from_millis(1));
        profile.current = 1;
        profile.push(1, "worker", now + Duration::from_millis(1));
        profile.pop(1, now + Duration::from_millis(11));
        profile.current = 0;
        profile.pop(0, now + Duration::from_millis(12));
        assert_eq!(
            profile.counters["worker"].elapsed,
            Duration::from_millis(10)
        );
        assert_eq!(profile.counters["waiter"].elapsed, Duration::from_millis(2));
    }
}
