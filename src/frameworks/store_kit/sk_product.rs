/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `SKProduct`, `SKProductsRequest` and `SKProductsResponse`.
//!
//! With IAP emulation enabled, a products request answers locally: every
//! requested identifier becomes a purchasable product (title/description
//! copied from the identifier, price 0.00), so games that hide their buy
//! buttons until the product list arrives show them and the queue completes
//! the purchase (see `sk_payment_queue.rs`).
//!
//! With emulation disabled (Cheat Engine inactive — the default), the
//! original pre-emulation stubs apply: `initWithProductIdentifiers:` and
//! `start` fail, so a products request never begins and no response is
//! ever delivered.

use crate::frameworks::foundation::ns_string;
use crate::frameworks::store_kit::emulation_enabled;
use crate::objc::{autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr};
use crate::Environment;

// MARK: - SKProduct

#[derive(Default)]
struct SKProductHostObject {
    /// All three are retained NSStrings (or nil).
    product_identifier: id,
    localized_title: id,
    localized_description: id,
    /// Emulated price. IAP emulation makes everything free.
    price: f64,
}
impl HostObject for SKProductHostObject {}

fn make_product(env: &mut Environment, product_identifier: id) -> id {
    let product: id = msg_class![env; SKProduct alloc];
    {
        let host = env.objc.borrow_mut::<SKProductHostObject>(product);
        host.product_identifier = product_identifier;
        host.localized_title = product_identifier;
        host.localized_description = product_identifier;
        host.price = 0.0;
    }
    retain(env, product_identifier);
    // One more retain backs the title/description aliases.
    retain(env, product_identifier);
    // And one for the localized_description field.
    retain(env, product_identifier);
    autorelease(env, product);
    product
}

// MARK: - SKProductsResponse

#[derive(Default)]
struct SKProductsResponseHostObject {
    /// Retained NSArray of SKProduct.
    products: id,
    /// Retained NSArray of NSString.
    invalid_product_identifiers: id,
}
impl HostObject for SKProductsResponseHostObject {}

// MARK: - SKProductsRequest

#[derive(Default)]
struct SKProductsRequestHostObject {
    /// Retained NSSet of NSString product identifiers.
    product_identifiers: id,
    /// SKProductsRequestDelegate — weak reference.
    delegate: id,
    /// Pending response NSTimer, if `start` was called. Retained.
    timer: id,
}
impl HostObject for SKProductsRequestHostObject {}

fn stop_response_timer(env: &mut Environment, this: id) {
    let timer = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<SKProductsRequestHostObject>(this)
            .timer,
        nil,
    );
    if timer != nil {
        () = msg![env; timer invalidate];
        release(env, timer);
    }
}

fn deliver_products_response(env: &mut Environment, this: id) {
    let product_identifiers = {
        let host = env.objc.borrow::<SKProductsRequestHostObject>(this);
        host.product_identifiers
    };
    let products: id = msg_class![env; NSMutableArray array];
    let mut requested = 0usize;
    if product_identifiers != nil {
        let array: id = msg![env; product_identifiers allObjects];
        let count: crate::frameworks::foundation::NSUInteger = msg![env; array count];
        for i in 0..count {
            let identifier: id = msg![env; array objectAtIndex:i];
            // Remember what the game asked for: the IAP emulation can later
            // hand these over as purchased transactions even when the shop's
            // remote product list never arrives.
            let identifier_text = super::sk_payment_queue::identifier_string(env, identifier);
            crate::frameworks::store_kit::remember_offered_identifier(&identifier_text);
            let product = make_product(env, identifier);
            () = msg![env; products addObject:product];
            requested += 1;
        }
    }
    let invalid: id = msg_class![env; NSMutableArray array];
    let response: id = msg_class![env; SKProductsResponse alloc];
    {
        let host = env.objc.borrow_mut::<SKProductsResponseHostObject>(response);
        host.products = products;
        host.invalid_product_identifiers = invalid;
    }
    retain(env, products);
    retain(env, invalid);
    autorelease(env, response);

    let delegate = env.objc.borrow::<SKProductsRequestHostObject>(this).delegate;
    log!(
        "SKProductsRequest: IAP emulation answered locally with {} product(s) (all free).",
        requested
    );
    if delegate == nil {
        return;
    }
    retain(env, delegate);
    autorelease(env, delegate);
    let sel = env.objc.register_host_selector(
        "productsRequest:didReceiveResponse:".to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        let _: () = msg![env; delegate productsRequest:this didReceiveResponse:response];
    } else {
        log!(
            "Warning: SKProductsRequestDelegate does not respond to productsRequest:didReceiveResponse:; response dropped."
        );
    }
    // SKRequestDelegate: real StoreKit finishes the request after the
    // response. Some games build their identifier-to-amount table only in
    // requestDidFinish:, and without this callback they would credit 0.
    let finish_sel = env
        .objc
        .register_host_selector("requestDidFinish:".to_string(), &mut env.mem);
    let responds: bool = msg![env; delegate respondsToSelector:finish_sel];
    if responds {
        let _: () = msg![env; delegate requestDidFinish:this];
    }
}


pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKProduct: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc
        .alloc_object(this, Box::<SKProductHostObject>::default(), &mut env.mem)
}

- (id)productIdentifier {
    let identifier = env
        .objc
        .borrow::<SKProductHostObject>(this)
        .product_identifier;
    if identifier == nil {
        ns_string::get_static_str(env, "")
    } else {
        retain(env, identifier);
        identifier
    }
}

- (id)localizedTitle {
    let title = env.objc.borrow::<SKProductHostObject>(this).localized_title;
    if title == nil {
        ns_string::get_static_str(env, "")
    } else {
        retain(env, title);
        title
    }
}

- (id)title {
    msg![env; this localizedTitle]
}

- (id)localizedDescription {
    let description = env
        .objc
        .borrow::<SKProductHostObject>(this)
        .localized_description;
    if description == nil {
        ns_string::get_static_str(env, "")
    } else {
        retain(env, description);
        description
    }
}

- (id)description {
    msg![env; this localizedDescription]
}

- (id)price {
    let price = env.objc.borrow::<SKProductHostObject>(this).price;
    msg_class![env; NSNumber numberWithDouble:price]
}

- (id)priceLocale {
    msg_class![env; NSLocale currentLocale]
}

- (bool)downloadable {
    false
}

// Era-appropriate aliases: iOS 3–6 games check `isDownloadable` (and some
// very old builds `contentDownloadable`) before enabling their buy buttons;
// an unimplemented selector returns 0 here, but we silence the warning spam.
- (bool)isDownloadable {
    false
}

- (bool)contentDownloadable {
    false
}

// iOS 6+ hosted-content fields. Real StoreKit reports them per product; we
// have no downloadable content, so an empty list and an empty version match
// the "Not Available" defaults games fall back to.
- (id)downloadContentLengths {
    msg_class![env; NSArray array]
}

- (id)downloadContentVersion {
    ns_string::get_static_str(env, "")
}

- (())dealloc {
    let (product_identifier, title, description) = {
        let host = env.objc.borrow::<SKProductHostObject>(this);
        (
            host.product_identifier,
            host.localized_title,
            host.localized_description,
        )
    };
    release(env, product_identifier);
    release(env, title);
    release(env, description);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation SKProductsResponse: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<SKProductsResponseHostObject>::default(),
        &mut env.mem,
    )
}

- (id)products {
    let products = env
        .objc
        .borrow::<SKProductsResponseHostObject>(this)
        .products;
    if products == nil {
        msg_class![env; NSArray array]
    } else {
        retain(env, products);
        products
    }
}

- (id)invalidProductIdentifiers {
    let invalid = env
        .objc
        .borrow::<SKProductsResponseHostObject>(this)
        .invalid_product_identifiers;
    if invalid == nil {
        msg_class![env; NSArray array]
    } else {
        retain(env, invalid);
        invalid
    }
}

- (())dealloc {
    let (products, invalid) = {
        let host = env.objc.borrow::<SKProductsResponseHostObject>(this);
        (host.products, host.invalid_product_identifiers)
    };
    release(env, products);
    release(env, invalid);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation SKProductsRequest: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<SKProductsRequestHostObject>::default(),
        &mut env.mem,
    )
}

- (id)initWithProductIdentifiers:(id)product_identifiers { // NSSet*
    if !emulation_enabled() {
        // Cheat Engine inactive: pre-emulation stub — requests can never
        // start, so games keep their stock no-store behavior.
        log!("SKProductsRequest initWithProductIdentifiers: stubbed (IAP emulation off)");
        return nil;
    }
    {
        let host = env.objc.borrow_mut::<SKProductsRequestHostObject>(this);
        host.product_identifiers = product_identifiers;
    }
    if product_identifiers != nil {
        retain(env, product_identifiers);
    }
    this
}

- (())setDelegate:(id)delegate {
    let host = env.objc.borrow_mut::<SKProductsRequestHostObject>(this);
    host.delegate = delegate;
}

- (id)delegate {
    env.objc
        .borrow::<SKProductsRequestHostObject>(this)
        .delegate
}

- (bool)start {
    if !emulation_enabled() {
        // Defensive twin of the init stub above (e.g. a request created
        // before the toggle changed).
        log!("SKProductsRequest start: stubbed (IAP emulation off)");
        return false;
    }
    stop_response_timer(env, this);
    let Some(selector) = env.objc.lookup_selector("_touchHLE_iapProductsTimer:") else {
        log!("Warning: SKProductsRequest timer selector missing; cannot answer.");
        return false;
    };
    let timer: id = msg_class![env; NSTimer scheduledTimerWithTimeInterval:0.05 target:this selector:selector userInfo:nil repeats:false];
    retain(env, timer);
    env.objc
        .borrow_mut::<SKProductsRequestHostObject>(this)
        .timer = timer;
    true
}

- (())cancel {
    stop_response_timer(env, this);
}

- (())_touchHLE_iapProductsTimer:(id)_timer {
    stop_response_timer(env, this);
    // The overlay may have disabled emulation while this timer was pending.
    if emulation_enabled() {
        deliver_products_response(env, this);
    }
}

- (())dealloc {
    stop_response_timer(env, this);
    let product_identifiers = env
        .objc
        .borrow::<SKProductsRequestHostObject>(this)
        .product_identifiers;
    release(env, product_identifiers);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
