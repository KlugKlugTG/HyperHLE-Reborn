/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Logging and terminal output macros.

use std::fs::File;
use std::sync::{LazyLock, Mutex};

/// Get a handle to the log file. This is only for use by logging macros!
///
/// All the logging macros print to stderr or (on Android) logcat, but this
/// is not convenient for users who aren't accustomed to command-line tools or
/// who don't have access to ADB, so we also write to a log file.
pub fn get_log_file() -> &'static Mutex<File> {
    static LOG_FILE: LazyLock<Mutex<File>> = LazyLock::new(|| {
        let file =
            File::create(crate::paths::user_data_base_path().join(crate::paths::LOG_FILE)).unwrap();
        #[cfg(unix)]
        crate::crash_handler::set_log_fd(std::os::fd::AsRawFd::as_raw_fd(&file));
        Mutex::new(file)
    });

    &LOG_FILE
}

/// The module prefix for log lines: the crate's historical `touchHLE` name
/// (used by `module_path!()`) swapped for the project's current brand.
macro_rules! display_module_path {
    () => {
        // The crate is still named touchHLE internally, but the project is
        // HyperHLE; swap the prefix out in the log output.
        module_path!().replacen("touchHLE", "HyperHLE", 1)
    };
}

/// Prints a log message unconditionally. Use this for errors or warnings.
///
/// The message is prefixed with the module path, so it is clear where it comes
/// from.
macro_rules! log {
    ($($arg:tt)+) => {
        echo!("{}: {}", display_module_path!(), format_args!($($arg)+))
    }
}

/// Same as [log], but silently fails on panic instead of
/// panicking.
macro_rules! log_no_panic {
    ($($arg:tt)+) => {
        echo_no_panic!("{}: {}", display_module_path!(), format_args!($($arg)+));
    }
}

/// Like [log], but prints the message only if debugging is enabled for the
/// module where it is used. This can be used for verbose things only needed
/// when debugging.
macro_rules! log_dbg {
    ($($arg:tt)+) => {
        if $crate::log::ENABLED_MODULES.contains(&module_path!()) {
            log!($($arg)*);
        }
    }
}

/// Like [log], but messages only log once and cannot have formatting.
/// To be used for log messages that are known to spam the log file (like those
/// logged every frame).
macro_rules! log_once {
    ($msg:literal) => {{
        static LOG_ONCE: std::sync::Once = std::sync::Once::new();
        LOG_ONCE.call_once(|| {
            log!("{} [this log will only be shown once]", $msg);
        });
    }};
}

/// Print a message (with implicit newline). This should be used for all
/// HyperHLE output that isn't coming from the app itself.
///
/// Prefer use [log] or [log_dbg] for errors and warnings during emulation.
macro_rules! echo {
    ($($arg:tt)+) => {
        {
            let mut formatted_str = format!($($arg)+);

            #[cfg(target_os = "android")]
            {
                sdl2::log::log(&formatted_str);
            }
            #[cfg(not(target_os = "android"))]
            eprintln!("{}", formatted_str);

            if let Ok(mut log_file) = $crate::log::get_log_file().lock() {
                // Append the newline into the same buffer so the line reaches
                // the file in a single write syscall (halves per-line I/O; this
                // matters on slow storage such as Android's app sandbox).
                formatted_str.push('\n');
                let _ = std::io::Write::write_all(&mut *log_file, formatted_str.as_bytes());
            }
        }
    };
    () => {
        {
            #[cfg(target_os = "android")]
            {
                sdl2::log::log("");
            }
            #[cfg(not(target_os = "android"))]
            eprintln!("");

            if let Ok(mut log_file) = $crate::log::get_log_file().lock() {
                let _ = std::io::Write::write_all(&mut *log_file, b"\n");
                let _ = std::io::Write::flush(&mut *log_file);
            }
        }
    }
}

/// Like [echo], but only writes to the in-file log: nothing is sent to
/// logcat/stderr. Intended for opt-in tracing of extremely hot paths (e.g.
/// per-call `--verbose-gles` output), where the logcat round-trip alone is a
/// measurable frame-time cost on Android.
macro_rules! echo_file_only {
    ($($arg:tt)+) => {
        {
            let formatted_str = format!($($arg)+);

            if let Ok(mut log_file) = $crate::log::get_log_file().lock() {
                let _ = std::io::Write::write_all(&mut *log_file, formatted_str.as_bytes());
                let _ = std::io::Write::write_all(&mut *log_file, b"\n");
            }
        }
    };
}

/// Like [log], but only writes to the in-file log (never logcat/stderr). To
/// be used for per-call traces that can emit thousands of lines per second.
macro_rules! log_file_only {
    ($($arg:tt)+) => {
        echo_file_only!("{}: {}", display_module_path!(), format_args!($($arg)+))
    };
}

/// Same as [echo], but silently fails on panic instead of
/// panicking.
macro_rules! echo_no_panic {
    ($($arg:tt)*) => {
        {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                echo!($($arg)*);
            }));
        }
    }
}

/// Put modules to enable [log_dbg] for here, e.g. "touchHLE::mem" to see when
/// memory is allocated and freed. Keep the default empty to avoid hot-path log
/// traffic in normal builds.
#[cfg(not(target_os = "ios"))]
pub const ENABLED_MODULES: &[&str] = &["touchHLE::frameworks::media_player::movie_player"];
// NOTE: keep this EMPTY for builds that run on an iOS device. Every
// `log_dbg!` does two blocking writes (stderr, which the iOS host redirects to
// a file, and HyperHLE_log.txt). On the main thread that starves UIKit of the
// run-loop time it needs to dispatch touches.
#[cfg(target_os = "ios")]
pub const ENABLED_MODULES: &[&str] = &[];
