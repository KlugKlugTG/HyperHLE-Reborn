/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! StoreKit

mod sk_payment_queue;
mod sk_product;
mod sk_store_product_view_controller;

use crate::dyld::{ConstantExports, HostConstant};

/// Whether guest in-app purchases are emulated ("Lucky Patcher"-style free
/// buys). Off by default; `TOUCHHLE_IAP_EMULATION` in the environment forces
/// it on at startup, and the Cheat Engine overlay's `IAP` button latches it
/// per session. While on, SystemConfiguration also reports a reachable
/// Wi-Fi-style route for games that gate StoreKit on a connectivity preflight.
/// This does not enable actual network access or emulate remote game servers.
static IAP_EMULATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `SKPaymentTransactionStatePurchased` — the transaction succeeded.
pub const SK_PAYMENT_TRANSACTION_STATE_PURCHASED: crate::frameworks::foundation::NSInteger = 1;
/// `SKPaymentTransactionStateFailed` — the transaction did not go through.
pub const SK_PAYMENT_TRANSACTION_STATE_FAILED: crate::frameworks::foundation::NSInteger = 2;
/// `SKPaymentTransactionStateRestored` — a previously purchased product was
/// restored.
pub const SK_PAYMENT_TRANSACTION_STATE_RESTORED: crate::frameworks::foundation::NSInteger = 3;

/// Current IAP emulation switch.
pub fn emulation_enabled() -> bool {
    IAP_EMULATION.load(std::sync::atomic::Ordering::Relaxed)
        || crate::env_flag_cached!("TOUCHHLE_IAP_EMULATION")
}

/// Change the IAP emulation switch (Cheat Engine overlay `IAP` button).
pub fn set_emulation_enabled(enabled: bool) {
    IAP_EMULATION.store(enabled, std::sync::atomic::Ordering::Relaxed);
    log!(
        "StoreKit IAP emulation {} (in-app purchases auto-succeed while on)",
        if enabled { "ON" } else { "OFF" }
    );
}

/// Constants used by the StoreKit framework.
///
/// These are NSString-typed `extern const` symbols. Apps that link against
/// StoreKit on iOS 6+ pull them in via Mach-O symbol lookup (e.g. for use
/// as `SKStoreProductViewController` parameter dictionary keys); without
/// these stubs the linker leaves the slots NULL, so any guest-side
/// dereference (CFString equality check, `[dict objectForKey:nil]`, etc.)
/// crashes with a NULL-page read.
pub const CONSTANTS: ConstantExports = &[
    (
        "_SKStoreProductParameterITunesItemIdentifier",
        HostConstant::NSString("itemIdentifier"),
    ),
    (
        "_SKStoreProductParameterAffiliateToken",
        HostConstant::NSString("affiliateToken"),
    ),
    (
        "_SKStoreProductParameterCampaignToken",
        HostConstant::NSString("campaignToken"),
    ),
    (
        "_SKStoreProductParameterProviderToken",
        HostConstant::NSString("providerToken"),
    ),
    (
        "_SKStoreProductParameterAdvertisingPartnerToken",
        HostConstant::NSString("advertisingPartnerToken"),
    ),
    ("_SKErrorDomain", HostConstant::NSString("SKErrorDomain")),
];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/StoreKit.framework/StoreKit",
    aliases: &[],
    class_exports: &[
        sk_payment_queue::CLASSES,
        sk_product::CLASSES,
        sk_store_product_view_controller::CLASSES,
    ],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    pub payment_queue: sk_payment_queue::State,
}
