/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */
//! Read-once, cached environment variable lookups for hot paths.
//!
//! `std::env::var_os`/`std::env::var` take a global lock, linearly scan the
//! process environ array and allocate, on *every* call. That's far too
//! expensive to do per frame or per touch event. The various
//! `TOUCHHLE_*` debug/compat toggles are set by the user before launch, so
//! reading them once and caching the result for the lifetime of the process
//! preserves their meaning while making repeated checks almost free.
//!
//! Caching for the lifetime of the process is wrong for the flags that the
//! emulator's own per-app options set: the app picker runs the same
//! UIKit/OpenGL ES code as the app and can read a flag before the app's
//! options have been applied (`UIScreen.bounds` reading
//! `TOUCHHLE_LANDSCAPE_UISCREEN_BOUNDS` is a real example), which then
//! disabled `--landscape-uiscreen-bounds` for the app launched afterwards.
//! [invalidate_caches] bumps a global generation; the next lookup of *every*
//! cached flag re-reads the environment. Call it once the options for an app
//! have been applied, and after a compatibility profile adjusts the
//! environment.

use std::sync::atomic::{AtomicU64, Ordering};

/// Incremented by [invalidate_caches]. Cached lookups compare the generation
/// they were made in with the current one and refresh when it changed.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The current cache generation, for the caching macros below.
#[doc(hidden)]
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// Make every `env_flag_cached!` call site forget its cached value, so the
/// next lookup observes the current environment. Cheap (one atomic increment)
/// and safe to call at any point; call sites that never see a generation
/// change stay as cheap as they were.
pub fn invalidate_caches() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Check for the *presence* of an environment variable, once per generation,
/// and return the cached result on subsequent calls.
#[macro_export]
macro_rules! env_flag_cached {
    ($name:literal) => {{
        // Generation and value are packed into one atomic as
        // `generation << 1 | value`; `u64::MAX` means "nothing cached yet".
        static CACHED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
        let generation = $crate::env_flags::generation();
        let mut slot = CACHED.load(std::sync::atomic::Ordering::Relaxed);
        loop {
            if slot != u64::MAX && (slot >> 1) == generation {
                break (slot & 1) != 0;
            }
            let value = std::env::var_os($name).is_some();
            let new_slot = (generation << 1) | (value as u64);
            match CACHED.compare_exchange_weak(
                slot,
                new_slot,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => break value,
                // Lost a race with another thread; retry against its value.
                Err(current) => slot = current,
            }
        }
    }};
}

/// Read the string value of an environment variable, once, and return the
/// cached result (`Some(&str)` if set and valid UTF-8) on subsequent calls.
///
/// Note: unlike [env_flag_cached], this is not refreshed by
/// [invalidate_caches]. No per-app option sets a string-valued `TOUCHHLE_*`
/// variable today; the ones that are set that way (`--force-gles1-context`,
/// `--no-fix-texture-min-filter`) are read through their own caches.
#[macro_export]
macro_rules! env_var_cached {
    ($name:literal) => {{
        static CACHED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        CACHED.get_or_init(|| std::env::var($name).ok()).as_deref()
    }};
}

#[cfg(test)]
mod tests {
    /// The app picker can read a flag before the app's options are applied;
    /// `invalidate_caches` must make the next lookup observe the new value.
    #[test]
    fn invalidate_caches_refreshes_flags() {
        // The macro takes a literal, so the name cannot come from a constant.
        std::env::remove_var("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION");
        crate::env_flags::invalidate_caches();
        assert!(!crate::env_flag_cached!("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION"));

        std::env::set_var("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION", "1");
        // Still cached from the previous generation.
        assert!(!crate::env_flag_cached!("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION"));
        crate::env_flags::invalidate_caches();
        assert!(crate::env_flag_cached!("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION"));

        std::env::remove_var("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION");
        crate::env_flags::invalidate_caches();
        assert!(!crate::env_flag_cached!("TOUCHHLE_TEST_FLAG_FOR_CACHE_INVALIDATION"));
    }
}
