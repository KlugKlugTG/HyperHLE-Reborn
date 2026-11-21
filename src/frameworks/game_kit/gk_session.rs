/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKSession` and related peer-to-peer classes.
//!
//! There is no Bluetooth or local network on the emulated device, so a
//! session can never become available and never sees a peer. The properties
//! read back what the app set. Anything that would need a peer fails with the
//! matching `GKSessionErrorDomain` error, and delegate callbacks are made
//! synchronously, before the method returns (real GameKit calls them later).

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::{NSInteger, NSTimeInterval};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

// MARK: - Constants

/// `GKSessionMode`
pub type GKSessionMode = i32;
pub const GKSessionModeServer: GKSessionMode = 0;
pub const GKSessionModeClient: GKSessionMode = 1;
pub const GKSessionModePeer: GKSessionMode = 2;

/// `GKPeerConnectionState`
pub type GKPeerConnectionState = i32;
pub const GKPeerStateAvailable: GKPeerConnectionState = 0;
pub const GKPeerStateUnavailable: GKPeerConnectionState = 1;
pub const GKPeerStateConnected: GKPeerConnectionState = 2;
pub const GKPeerStateDisconnected: GKPeerConnectionState = 3;
pub const GKPeerStateConnecting: GKPeerConnectionState = 4;

/// `GKSendDataMode`
pub type GKSendDataMode = i32;
pub const GKSendDataReliable: GKSendDataMode = 0;
pub const GKSendDataUnreliable: GKSendDataMode = 1;

/// `GKSessionError` codes, in `GKSessionErrorDomain`.
pub const GKSessionInvalidParameterError: NSInteger = 30500;
pub const GKSessionPeerNotFoundError: NSInteger = 30501;
pub const GKSessionDeclinedError: NSInteger = 30502;
pub const GKSessionTimedOutError: NSInteger = 30503;
pub const GKSessionCannotEnableError: NSInteger = 30504;
pub const GKSessionConnectionFailedError: NSInteger = 30505;
pub const GKSessionConnectionClosedError: NSInteger = 30506;
pub const GKSessionInProgressError: NSInteger = 30507;
pub const GKSessionSystemError: NSInteger = 30508;

/// `GKPeerPickerConnectionType` (bit flags)
pub const GKPeerPickerConnectionTypeOnline: u32 = 1 << 0;
pub const GKPeerPickerConnectionTypeNearby: u32 = 1 << 1;

pub const GKSessionErrorDomain: &str = "GKSessionErrorDomain";

pub const CONSTANTS: ConstantExports = &[(
    "_GKSessionErrorDomain",
    HostConstant::NSString(GKSessionErrorDomain),
)];

// MARK: - Host objects

#[derive(Default)]
struct GKSessionHostObject {
    /// `NSString *`, copied. May be nil.
    session_id: id,
    /// `NSString *`, copied.
    display_name: id,
    session_mode: GKSessionMode,
    /// Weak.
    delegate: id,
    available: bool,
    disconnect_timeout: NSTimeInterval,
    /// Weak. Stored only: no data ever arrives.
    data_receive_handler: id,
    /// A `void *` the app gave us. Never dereferenced.
    receive_context: id,
}
impl HostObject for GKSessionHostObject {}

struct GKPeerPickerControllerHostObject {
    /// Weak.
    delegate: id,
    connection_types_mask: u32,
    visible: bool,
}
impl HostObject for GKPeerPickerControllerHostObject {}

// MARK: - Helpers

/// Copy `val` into the field picked by `field`, releasing the old value.
/// `copy` returns +1, which the host object keeps; nil stays nil. No borrow
/// is held across `copy` or `release`.
fn set_copy<F>(env: &mut Environment, this: id, field: F, val: id)
where
    F: FnOnce(&mut GKSessionHostObject) -> &mut id,
{
    let copied: id = if val == nil { nil } else { msg![env; val copy] };
    let old = std::mem::replace(
        field(env.objc.borrow_mut::<GKSessionHostObject>(this)),
        copied,
    );
    release(env, old);
}

fn responds_to(env: &mut Environment, obj: id, selector: &str) -> bool {
    match env.objc.lookup_selector(selector) {
        Some(sel) => {
            let responds: bool = msg![env; obj respondsToSelector:sel];
            responds
        }
        None => false,
    }
}

/// An autoreleased `NSError` in `GKSessionErrorDomain`.
fn session_error(env: &mut Environment, code: NSInteger) -> id {
    let domain = get_static_str(env, GKSessionErrorDomain);
    msg_class![env; NSError errorWithDomain:domain code:code userInfo:nil]
}

/// Fills `*error_out` (an `NSError **`) if the caller passed one.
fn write_error(env: &mut Environment, error_out: MutPtr<id>, code: NSInteger) {
    if error_out.is_null() {
        return;
    }
    let error = session_error(env, code);
    env.mem.write(error_out, error);
}

/// The ID of the one peer that exists: this device.
fn local_peer_id(env: &mut Environment) -> id {
    get_static_str(env, "touchHLE-local-peer")
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// MARK: - GKSession

@implementation GKSession: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(GKSessionHostObject {
        session_id: nil,
        display_name: nil,
        session_mode: GKSessionModePeer,
        delegate: nil,
        available: false,
        // GameKit's default.
        disconnect_timeout: 20.0,
        data_receive_handler: nil,
        receive_context: nil,
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)initWithSessionID:(id)session_id // NSString*
            displayName:(id)display_name // NSString*
            sessionMode:(i32)session_mode {
    set_copy(env, this, |h| &mut h.session_id, session_id);
    // The real default is the device name.
    let name = if display_name == nil {
        get_static_str(env, "iPhone")
    } else {
        display_name
    };
    set_copy(env, this, |h| &mut h.display_name, name);
    env.objc.borrow_mut::<GKSessionHostObject>(this).session_mode = session_mode;
    this
}

- (())dealloc {
    let (session_id, display_name) = {
        let h = env.objc.borrow::<GKSessionHostObject>(this);
        (h.session_id, h.display_name)
    };
    release(env, session_id);
    release(env, display_name);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Properties

// Weak, as in GameKit.
- (id)delegate {
    env.objc.borrow::<GKSessionHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<GKSessionHostObject>(this).delegate = delegate;
}

- (id)sessionID {
    env.objc.borrow::<GKSessionHostObject>(this).session_id
}

- (id)displayName {
    env.objc.borrow::<GKSessionHostObject>(this).display_name
}

- (i32)sessionMode {
    env.objc.borrow::<GKSessionHostObject>(this).session_mode
}

// Our own peer ID; there is no one else.
- (id)peerID {
    local_peer_id(env)
}

- (NSTimeInterval)disconnectTimeout {
    env.objc.borrow::<GKSessionHostObject>(this).disconnect_timeout
}
- (())setDisconnectTimeout:(NSTimeInterval)timeout {
    env.objc.borrow_mut::<GKSessionHostObject>(this).disconnect_timeout = timeout;
}

// MARK: Availability

- (bool)isAvailable {
    env.objc.borrow::<GKSessionHostObject>(this).available
}

// Becoming available needs a radio. Report that to the delegate instead of
// leaving the app waiting for peers that cannot arrive.
- (())setAvailable:(bool)available {
    env.objc.borrow_mut::<GKSessionHostObject>(this).available = false;
    if !available {
        return;
    }
    log!("GKSession {:?} setAvailable:YES: no peer-to-peer networking, failing", this);
    let delegate = env.objc.borrow::<GKSessionHostObject>(this).delegate;
    if delegate != nil && responds_to(env, delegate, "session:didFailWithError:") {
        let error = session_error(env, GKSessionCannotEnableError);
        let _: () = msg![env; delegate session:this didFailWithError:error];
    }
}

// MARK: Data handler

// Both are stored only: nothing is ever received.
- (())setDataReceiveHandler:(id)handler
                withContext:(id)context {
    let h = env.objc.borrow_mut::<GKSessionHostObject>(this);
    h.data_receive_handler = handler;
    h.receive_context = context;
}

// MARK: Connections

- (())connectToPeer:(id)peer_id
        withTimeout:(f64)_timeout {
    log!("GKSession {:?} connectToPeer: no peers exist, failing", this);
    let delegate = env.objc.borrow::<GKSessionHostObject>(this).delegate;
    if delegate != nil
        && responds_to(env, delegate, "session:connectionWithPeerFailed:withError:")
    {
        let error = session_error(env, GKSessionPeerNotFoundError);
        let _: () = msg![env; delegate session:this
                    connectionWithPeerFailed:peer_id
                                   withError:error];
    }
}

- (())cancelConnectToPeer:(id)_peer_id {
}

- (bool)acceptConnectionFromPeer:(id)_peer_id
                           error:(MutPtr<id>)error {
    // No peer ever asks to connect, so there is nothing to accept.
    write_error(env, error, GKSessionPeerNotFoundError);
    false
}

- (())denyConnectionFromPeer:(id)_peer_id {
}

- (())disconnectFromAllPeers {
}

- (())disconnectPeerFromAllPeers:(id)_peer_id {
}

// MARK: Peers

// Always empty: no peer is ever found.
- (id)peersWithConnectionState:(i32)_state {
    msg_class![env; NSArray array]
}

// Only our own ID resolves to a name.
- (id)displayNameForPeer:(id)peer_id {
    if peer_id == nil {
        return nil;
    }
    let local = local_peer_id(env);
    let is_local: bool = msg![env; peer_id isEqual:local];
    if is_local {
        env.objc.borrow::<GKSessionHostObject>(this).display_name
    } else {
        nil
    }
}

// MARK: Sending

// No peer is connected, so there is no one to send to.
- (bool)sendData:(id)_data
         toPeers:(id)_peers
    withDataMode:(i32)_mode
           error:(MutPtr<id>)error {
    write_error(env, error, GKSessionPeerNotFoundError);
    false
}

// "All peers" is the empty set here, so this succeeds vacuously.
- (bool)sendDataToAllPeers:(id)_data
              withDataMode:(i32)_mode
                     error:(MutPtr<id>)_error {
    true
}

// MARK: Description

- (id)description {
    let (session_id, display_name, mode, available) = {
        let h = env.objc.borrow::<GKSessionHostObject>(this);
        (h.session_id, h.display_name, h.session_mode, h.available)
    };
    let session_id = if session_id == nil {
        "(null)".to_string()
    } else {
        to_rust_string(env, session_id).into_owned()
    };
    let display_name = if display_name == nil {
        "(null)".to_string()
    } else {
        to_rust_string(env, display_name).into_owned()
    };
    let s = format!(
        "<GKSession: {:?}; sessionID = {}; displayName = {}; mode = {}; available = {}>",
        this, session_id, display_name, mode, available
    );
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

// MARK: - GKPeerPickerController

@implementation GKPeerPickerController: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(GKPeerPickerControllerHostObject {
        delegate: nil,
        // Unsure of the real default; Nearby is the useful one here.
        connection_types_mask: GKPeerPickerConnectionTypeNearby,
        visible: false,
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

// Weak, as in GameKit.
- (id)delegate {
    env.objc.borrow::<GKPeerPickerControllerHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<GKPeerPickerControllerHostObject>(this).delegate = delegate;
}

- (u32)connectionTypesMask {
    env.objc.borrow::<GKPeerPickerControllerHostObject>(this).connection_types_mask
}
- (())setConnectionTypesMask:(u32)mask {
    env.objc.borrow_mut::<GKPeerPickerControllerHostObject>(this).connection_types_mask = mask;
}

- (bool)isVisible {
    env.objc.borrow::<GKPeerPickerControllerHostObject>(this).visible
}

// There is no picker UI and no peers to pick, so the user "cancels"
// immediately. The delegate may release the picker in its callback, so hold a
// reference across the call and touch nothing afterwards.
- (())show {
    log!("GKPeerPickerController {:?} show: no peers, reporting cancel", this);
    retain(env, this);
    env.objc.borrow_mut::<GKPeerPickerControllerHostObject>(this).visible = false;
    let delegate = env.objc.borrow::<GKPeerPickerControllerHostObject>(this).delegate;
    if delegate != nil && responds_to(env, delegate, "peerPickerControllerDidCancel:") {
        let _: () = msg![env; delegate peerPickerControllerDidCancel:this];
    }
    release(env, this);
}

- (())dismiss {
    env.objc.borrow_mut::<GKPeerPickerControllerHostObject>(this).visible = false;
}

@end

// GKSessionDelegate is a protocol in GameKit, not a class. This empty class
// is kept so nothing that already refers to it breaks.
@implementation GKSessionDelegate: NSObject
// TODO
@end

};
