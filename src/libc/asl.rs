/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `asl.h` — Apple System Log stubs.
//!
//! touchHLE does not forward logs to a syslog-style facility; the guest's
//! messages are silently accepted. Just enough surface is provided to keep
//! apps like Google Mobile iOS 2.0 (which calls `asl_open` at startup and
//! `asl_log` opportunistically) from crashing or tripping a dyld lookup
//! warning.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, MutVoidPtr, Ptr};
use crate::Environment;

/// Sentinel non-NULL "client" handle returned by `asl_open`.
const ASL_CLIENT_STUB: MutVoidPtr = Ptr::from_bits(1);

fn asl_open(
    _env: &mut Environment,
    _ident: ConstPtr<u8>,
    _facility: ConstPtr<u8>,
    _opts: u32,
) -> MutVoidPtr {
    ASL_CLIENT_STUB
}

fn asl_close(_env: &mut Environment, _client: MutVoidPtr) {}

fn asl_new(_env: &mut Environment, _msg_type: u32) -> MutVoidPtr {
    ASL_CLIENT_STUB
}

fn asl_free(_env: &mut Environment, _msg: MutVoidPtr) {}

fn asl_set(
    _env: &mut Environment,
    _msg: MutVoidPtr,
    _key: ConstPtr<u8>,
    _value: ConstPtr<u8>,
) -> i32 {
    0
}

fn asl_set_query(
    _env: &mut Environment,
    _msg: MutVoidPtr,
    _key: ConstPtr<u8>,
    _value: ConstPtr<u8>,
    _op: u32,
) -> i32 {
    0
}

fn asl_log(
    _env: &mut Environment,
    _client: MutVoidPtr,
    _msg: MutVoidPtr,
    _level: i32,
    _format: ConstPtr<u8>,
) -> i32 {
    // Ignore; touchHLE already logs app warnings at a higher level.
    0
}

fn asl_vlog(
    _env: &mut Environment,
    _client: MutVoidPtr,
    _msg: MutVoidPtr,
    _level: i32,
    _format: ConstPtr<u8>,
    _ap: MutVoidPtr,
) -> i32 {
    0
}

fn asl_add_log_file(_env: &mut Environment, _client: MutVoidPtr, _fd: i32) -> i32 {
    0
}

fn asl_remove_log_file(_env: &mut Environment, _client: MutVoidPtr, _fd: i32) -> i32 {
    0
}

fn asl_set_filter(_env: &mut Environment, _client: MutVoidPtr, _mask: i32) -> i32 {
    0
}

/// Legacy convenience wrapper (iOS 7+, deprecated in 10.0): games such as
/// Asphalt 9 log through this instead of the `asl_*` handle-based API.
/// It is `printf`-variadic in C, so only the fixed prefix is registered;
/// the message is forwarded to the host log rather than silently dropped.
fn asl_log_message(
    _env: &mut Environment,
    level: i32,
    format: ConstPtr<u8>,
) -> i32 {
    if let Ok(message) = _env.mem.cstr_at_utf8(format) {
        log_dbg!(
            "asl_log_message(level={}): {}",
            level,
            message
        );
    }
    0
}

/// `char *asl_get(aslmsg msg, const char *key)` — returns the value stored
/// under `key`. HyperHLE does not keep an ASL message dictionary, so every
/// lookup reports "not present" instead of dereferencing a guest pointer.
fn asl_get(_env: &mut Environment, _msg: MutVoidPtr, _key: ConstPtr<u8>) -> MutVoidPtr {
    MutVoidPtr::null()
}

/// `char *asl_key(aslmsg msg, uint32_t n)` — enumerates keys. No dictionary
/// is stored, so there is no key at any index.
fn asl_key(_env: &mut Environment, _msg: MutVoidPtr, _index: u32) -> MutVoidPtr {
    MutVoidPtr::null()
}

/// `aslresponse asl_search(aslclient client, const char *query)` — returns
/// NULL when no matching messages exist, which is always the case here.
fn asl_search(_env: &mut Environment, _client: MutVoidPtr, _query: ConstPtr<u8>) -> MutVoidPtr {
    MutVoidPtr::null()
}

/// `aslmsg aslresponse_next(aslresponse response)` — empty response.
fn aslresponse_next(_env: &mut Environment, _response: MutVoidPtr) -> MutVoidPtr {
    MutVoidPtr::null()
}

/// `void aslresponse_free(aslresponse response)`.
fn aslresponse_free(_env: &mut Environment, _response: MutVoidPtr) {}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(asl_open(_, _, _)),
    export_c_func!(asl_close(_)),
    export_c_func!(asl_new(_)),
    export_c_func!(asl_free(_)),
    export_c_func!(asl_set(_, _, _)),
    export_c_func!(asl_set_query(_, _, _, _)),
    // asl_log / asl_vlog are variadic in C; touchHLE ignores the payload,
    // so we only register the fixed arguments.
    export_c_func!(asl_log(_, _, _, _)),
    export_c_func!(asl_vlog(_, _, _, _, _)),
    export_c_func!(asl_add_log_file(_, _)),
    export_c_func!(asl_remove_log_file(_, _)),
    export_c_func!(asl_set_filter(_, _)),
    export_c_func!(asl_log_message(_, _)),
    export_c_func!(asl_get(_, _)),
    export_c_func!(asl_key(_, _)),
    export_c_func!(asl_search(_, _)),
    export_c_func!(aslresponse_next(_)),
    export_c_func!(aslresponse_free(_)),
];
