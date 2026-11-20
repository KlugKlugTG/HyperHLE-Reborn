//! `<pwd.h>` — POSIX password database access.
//!
//! Apple reference: `getpwuid_r(3)` man page
//! <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/getpwuid_r.3.html>
//!
//! ```c
//! struct passwd {
//!     char    *pw_name;       /* user name */
//!     char    *pw_passwd;     /* encrypted password */
//!     uid_t    pw_uid;        /* user uid */
//!     gid_t    pw_gid;        /* user gid */
//!     char    *pw_gecos;      /* user real name */
//!     char    *pw_dir;        /* home directory */
//!     char    *pw_shell;      /* shell program */
//! };
//!
//! int getpwuid_r(uid_t uid, struct passwd *pwd, char *buf, size_t buflen,
//!     struct passwd **result);
//! ```
//!
//! The emulator presents a single virtual user ("mobile", the historical
//! unprivileged iOS user) whose home directory matches the paths the
//! guest filesystem already exposes (`/var/mobile`). Only the fields
//! guest engines actually consume (notably `pw_dir`, which Unity's
//! `rvmStartup` uses to locate the home folder) are meaningful; the rest
//! are plausible fixed values, mirroring a stock iOS install.

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::posix_io::stat::{gid_t, uid_t};
use crate::mem::{GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::Environment;

/// Layout of `struct passwd` on 32-bit iOS: seven 4-byte fields.
const PASSWD_STRUCT_SIZE: GuestUSize = 28;

struct PasswdStrings {
    name: &'static str,
    passwd: &'static str,
    gecos: &'static str,
    dir: &'static str,
    shell: &'static str,
}

/// The virtual unprivileged iOS user. `pw_dir` must match the home
/// directory prefix used elsewhere in the guest filesystem.
const MOBILE_USER: PasswdStrings = PasswdStrings {
    name: "mobile",
    passwd: "*",
    gecos: "Mobile User",
    dir: "/var/mobile",
    shell: "/bin/sh",
};

/// `struct passwd *getpwuid(uid_t uid)`
///
/// Non-reentrant form used by older libc++/Foundation code. The returned
/// structure and its strings are valid for the rest of the process lifetime.
fn getpwuid(env: &mut Environment, uid: uid_t) -> MutPtr<passwd_t> {
    if uid != 501 {
        return Ptr::null();
    }
    let strings = &MOBILE_USER;
    let alloc = |env: &mut Environment, text: &str| env.mem.alloc_and_write_cstr(text.as_bytes());
    let out = passwd_t {
        pw_name: alloc(env, strings.name),
        pw_passwd: alloc(env, strings.passwd),
        pw_uid: uid,
        pw_gid: 501,
        pw_gecos: alloc(env, strings.gecos),
        pw_dir: alloc(env, strings.dir),
        pw_shell: alloc(env, strings.shell),
    };
    let ptr: MutPtr<passwd_t> = env.mem.alloc(PASSWD_STRUCT_SIZE).cast();
    env.mem.write(ptr, out);
    ptr
}

/// `int getpwuid_r(uid_t uid, struct passwd *pwd, char *buf, size_t buflen,
///    struct passwd **result);`
///
/// Fills `pwd` from caller-provided `buf` storage (POSIX thread-safe
/// form). Returns 0 and sets `*result = pwd` on success; returns 0 with
/// `*result = NULL` when no entry matches; returns an error number on
/// failure (we use ERANGE for an undersized buffer, like Darwin).
fn getpwuid_r(
    env: &mut Environment,
    uid: uid_t,
    pwd: MutPtr<passwd_t>,
    buf: MutPtr<u8>,
    buflen: GuestUSize,
    result: MutPtr<MutPtr<passwd_t>>,
) -> i32 {
    const ERANGE: i32 = 34;

    if pwd.is_null() || result.is_null() {
        return crate::libc::errno::EINVAL;
    }
    // The emulator exposes exactly one user; anything else is "not found".
    if uid != 501 {
        env.mem.write(result, Ptr::null());
        return 0;
    }

    let strings = &MOBILE_USER;
    let mut needed: GuestUSize = 0;
    let pieces = [
        strings.name,
        strings.passwd,
        strings.gecos,
        strings.dir,
        strings.shell,
    ];
    for piece in pieces {
        needed += piece.len() as GuestUSize + 1;
    }
    if buflen < needed {
        log!(
            "getpwuid_r: buffer of {} bytes too small ({} needed), returning ERANGE",
            buflen,
            needed
        );
        return ERANGE;
    }

    // Copy strings into `buf`, recording 4-byte-aligned pointers.
    let mut cursor = buf;
    let mut string_ptr = |env: &mut Environment, s: &str| -> MutPtr<u8> {
        let ptr = cursor;
        for (i, b) in s.bytes().enumerate() {
            env.mem.write(ptr + i as GuestUSize, b);
        }
        env.mem.write(ptr + s.len() as GuestUSize, b'\0');
        cursor = ptr + (s.len() as GuestUSize) + 1;
        // keep next string 4-byte aligned
        let rem = cursor.to_bits() % 4;
        if rem != 0 {
            cursor = cursor + (4 - rem);
        }
        ptr
    };

    let pw_name = string_ptr(env, strings.name);
    let pw_passwd = string_ptr(env, strings.passwd);
    let pw_gecos = string_ptr(env, strings.gecos);
    let pw_dir = string_ptr(env, strings.dir);
    let pw_shell = string_ptr(env, strings.shell);

    let out = passwd_t {
        pw_name,
        pw_passwd,
        pw_uid: uid,
        pw_gid: 501,
        pw_gecos,
        pw_dir,
        pw_shell,
    };
    env.mem.write(pwd, out);
    env.mem.write(result, pwd.cast());
    log_dbg!(
        "getpwuid_r({}) => user '{}' home '{}'",
        uid,
        strings.name,
        strings.dir
    );
    0
}

#[repr(C)]
#[derive(Clone, Copy)]
struct passwd_t {
    pw_name: MutPtr<u8>,
    pw_passwd: MutPtr<u8>,
    pw_uid: uid_t,
    pw_gid: gid_t,
    pw_gecos: MutPtr<u8>,
    pw_dir: MutPtr<u8>,
    pw_shell: MutPtr<u8>,
}

unsafe impl SafeRead for passwd_t {}

// Static assertion that the struct matches the 32-bit ABI layout.
const _: () = assert!(std::mem::size_of::<passwd_t>() == PASSWD_STRUCT_SIZE as usize);

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getpwuid(_)),
    export_c_func!(getpwuid_r(_, _, _, _, _)),
];
