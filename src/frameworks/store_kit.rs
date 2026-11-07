/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! StoreKit

mod sk_payment_queue;
mod sk_product;

use crate::dyld::{ConstantExports, HostConstant};

/// Whether guest in-app purchases are emulated ("Lucky Patcher"-style free
/// buys). Off by default; `TOUCHHLE_IAP_EMULATION` in the environment forces
/// it on at startup, and the Cheat Engine overlay's `IAP` button latches it
/// per session. While on, SystemConfiguration also reports a reachable
/// Wi-Fi-style route for games that gate StoreKit on a connectivity preflight.
/// This does not enable actual network access or emulate remote game servers.
static IAP_EMULATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Product identifiers the guest has asked StoreKit about.
///
/// The IAP emulation can turn these into *purchased* transactions, which is
/// what actually hands items over: games routinely ignore restored
/// transactions for consumables (credits, boosters), so a shop whose store
/// preflight never completes can still be paid out.
static OFFERED_IDENTIFIERS: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> =
    std::sync::OnceLock::new();

fn offered_identifiers() -> &'static std::sync::Mutex<Vec<String>> {
    OFFERED_IDENTIFIERS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Remember a product identifier requested via `SKProductsRequest`.
pub fn remember_offered_identifier(identifier: &str) {
    if identifier.is_empty() {
        return;
    }
    let Ok(mut list) = offered_identifiers().lock() else {
        return;
    };
    if !list.iter().any(|existing| existing == identifier) {
        list.push(identifier.to_string());
    }
}

/// Every product identifier the guest has asked about so far.
pub fn offered_identifier_snapshot() -> Vec<String> {
    offered_identifiers()
        .lock()
        .map(|list| list.clone())
        .unwrap_or_default()
}

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
    class_exports: &[sk_payment_queue::CLASSES, sk_product::CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    pub payment_queue: sk_payment_queue::State,
}
