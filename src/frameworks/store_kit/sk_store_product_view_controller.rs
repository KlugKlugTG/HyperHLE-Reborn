/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `SKStoreProductViewController` — the in-app App Store sheet.
//!
//! Emulating the App Store UI is out of scope: the sheet can't be presented
//! and there is no store connection to load a product from. The class still
//! has to exist, because launcher-era apps and the ad/cross-promotion SDKs
//! they bundle probe for it (`[SKStoreProductViewController class]`,
//! `+canMakePayments`-style availability checks) and crash or misbehave when
//! the class is missing.
//!
//! `loadProductWithParameters:completionBlock:` therefore reports the same
//! failure a device would report with the store unavailable: the block is
//! invoked with `success = NO` and an `SKErrorDomain` error.

use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::foundation::ns_string;
use crate::mem::{ConstVoidPtr, Ptr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, objc_retainBlock, release, retain,
    ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
struct SKStoreProductViewControllerHostObject {
    /// `SKStoreProductViewControllerDelegate` — weak, like the real API.
    delegate: id,
}
impl HostObject for SKStoreProductViewControllerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKStoreProductViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<SKStoreProductViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// Both of these are how apps and SDKs create the sheet: the plain
// `-initWithNibName:bundle:` from UIViewController, or `-init`.
- (id)initWithNibName:(id)_nib_name bundle:(id)_nib_bundle {
    let this: id = msg![env; this init];
    this
}

- (())setDelegate:(id)delegate {
    let old_delegate = {
        let host = env
            .objc
            .borrow_mut::<SKStoreProductViewControllerHostObject>(this);
        let old = host.delegate;
        host.delegate = delegate;
        old
    };
    if delegate != nil {
        retain(env, delegate);
    }
    if old_delegate != nil {
        release(env, old_delegate);
    }
    log_dbg!("SKStoreProductViewController setDelegate:{:?}", delegate);
}

- (id)delegate {
    env.objc
        .borrow::<SKStoreProductViewControllerHostObject>(this)
        .delegate
}

// `-loadProductWithParameters:completionBlock:` never reaches the store, so the
// completion handler is called with a failure, exactly once, before this method
// returns (the real implementation is asynchronous, but the block is documented
// to run on the main thread and callers only branch on success).
- (())loadProductWithParameters:(id)parameters completionBlock:(id)block {
    let count = if parameters == nil {
        0
    } else {
        let count: u32 = msg![env; parameters count];
        count
    };
    log!(
        "SKStoreProductViewController loadProductWithParameters: ({} key(s)) \
         — no App Store in the emulator, reporting failure",
        count
    );
    if block == nil {
        return;
    }
    let block = objc_retainBlock(env, block);
    let invoke_ptr: u32 = env.mem.read(block.cast::<u32>() + 3u32);
    if invoke_ptr != 0 {
        let domain: id = ns_string::get_static_str(env, "SKErrorDomain");
        let error: id = msg_class![env; NSError errorWithDomain: domain code: 0i32 userInfo: nil];
        let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_ptr);
        let block_arg: ConstVoidPtr = Ptr::from_bits(block.to_bits()).cast_const();
        let _: () = invoke.call_from_host(env, (block_arg, false, error));
    } else {
        log!("SKStoreProductViewController: completion block has no invoke pointer");
    }
    release(env, block);
}

- (())dealloc {
    let delegate = env
        .objc
        .borrow::<SKStoreProductViewControllerHostObject>(this)
        .delegate;
    if delegate != nil {
        release(env, delegate);
    }
    env.objc.dealloc_object(this, &mut env.mem);
}

@end

};
