/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `CFLocale`

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_array::CFArrayRef;
use super::cf_string::CFStringRef;
use super::{CFIndex, CFRelease, CFRetain, CFTypeRef};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

type CFLocaleIdentifier = CFStringRef;
/// `NSLocale` is toll-free bridged with `CFLocaleRef`
pub(super) type CFLocaleRef = CFTypeRef;
type CFLocaleKey = CFStringRef;

/// `CFLocaleLanguageDirection`
pub type CFLocaleLanguageDirection = CFIndex;
pub const kCFLocaleLanguageDirectionUnknown: CFLocaleLanguageDirection = 0;
pub const kCFLocaleLanguageDirectionLeftToRight: CFLocaleLanguageDirection = 1;
pub const kCFLocaleLanguageDirectionRightToLeft: CFLocaleLanguageDirection = 2;
pub const kCFLocaleLanguageDirectionTopToBottom: CFLocaleLanguageDirection = 3;
#[allow(dead_code)]
pub const kCFLocaleLanguageDirectionBottomToTop: CFLocaleLanguageDirection = 4;

// MARK: - Constants

pub const kCFLocaleIdentifier: &str = "kCFLocaleIdentifierKey";
pub const kCFLocaleLanguageCode: &str = "kCFLocaleLanguageCode";
pub const kCFLocaleCountryCode: &str = "kCFLocaleCountryCodeKey";
pub const kCFLocaleScriptCode: &str = "kCFLocaleScriptCode";
pub const kCFLocaleVariantCode: &str = "kCFLocaleVariantCode";
pub const kCFLocaleExemplarCharacterSet: &str = "kCFLocaleExemplarCharacterSet";
pub const kCFLocaleCalendarIdentifier: &str = "kCFLocaleCalendarIdentifier";
pub const kCFLocaleCalendar: &str = "kCFLocaleCalendar";
pub const kCFLocaleCollationIdentifier: &str = "kCFLocaleCollationIdentifier";
pub const kCFLocaleUsesMetricSystem: &str = "kCFLocaleUsesMetricSystem";
pub const kCFLocaleMeasurementSystem: &str = "kCFLocaleMeasurementSystem";
pub const kCFLocaleDecimalSeparator: &str = "kCFLocaleDecimalSeparator";
pub const kCFLocaleGroupingSeparator: &str = "kCFLocaleGroupingSeparator";
pub const kCFLocaleCurrencySymbol: &str = "kCFLocaleCurrencySymbol";
pub const kCFLocaleCurrencyCode: &str = "kCFLocaleCurrencyCode";
pub const kCFLocaleCollatorIdentifier: &str = "kCFLocaleCollatorIdentifier";
pub const kCFLocaleQuotationBeginDelimiterKey: &str = "kCFLocaleQuotationBeginDelimiterKey";
pub const kCFLocaleQuotationEndDelimiterKey: &str = "kCFLocaleQuotationEndDelimiterKey";
pub const kCFLocaleAlternateQuotationBeginDelimiterKey: &str =
    "kCFLocaleAlternateQuotationBeginDelimiterKey";
pub const kCFLocaleAlternateQuotationEndDelimiterKey: &str =
    "kCFLocaleAlternateQuotationEndDelimiterKey";

// Calendar identifier constants
pub const kCFGregorianCalendar: &str = "gregorian";
pub const kCFBuddhistCalendar: &str = "buddhist";
pub const kCFChineseCalendar: &str = "chinese";
pub const kCFHebrewCalendar: &str = "hebrew";
pub const kCFIslamicCalendar: &str = "islamic";
pub const kCFIslamicCivilCalendar: &str = "islamic-civil";
pub const kCFIslamicTabularCalendar: &str = "islamic-tbla";
pub const kCFIslamicUmmAlQuraCalendar: &str = "islamic-umalqura";
pub const kCFJapaneseCalendar: &str = "japanese";
pub const kCFRepublicOfChinaCalendar: &str = "roc";
pub const kCFPersianCalendar: &str = "persian";
pub const kCFIndianCalendar: &str = "indian";
pub const kCFISO8601Calendar: &str = "iso8601";
pub const kCFCopticCalendar: &str = "coptic";
pub const kCFEthiopicAmeteMihretCalendar: &str = "ethiopic";
pub const kCFEthiopicAmeteAlemCalendar: &str = "ethiopic-amete-alem";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFLocaleIdentifier",
        HostConstant::NSString(kCFLocaleIdentifier),
    ),
    (
        "_kCFLocaleLanguageCode",
        HostConstant::NSString(kCFLocaleLanguageCode),
    ),
    (
        "_kCFLocaleCountryCode",
        HostConstant::NSString(kCFLocaleCountryCode),
    ),
    (
        "_kCFLocaleScriptCode",
        HostConstant::NSString(kCFLocaleScriptCode),
    ),
    (
        "_kCFLocaleVariantCode",
        HostConstant::NSString(kCFLocaleVariantCode),
    ),
    (
        "_kCFLocaleExemplarCharacterSet",
        HostConstant::NSString(kCFLocaleExemplarCharacterSet),
    ),
    (
        "_kCFLocaleCalendarIdentifier",
        HostConstant::NSString(kCFLocaleCalendarIdentifier),
    ),
    (
        "_kCFLocaleCalendar",
        HostConstant::NSString(kCFLocaleCalendar),
    ),
    (
        "_kCFLocaleCollationIdentifier",
        HostConstant::NSString(kCFLocaleCollationIdentifier),
    ),
    (
        "_kCFLocaleUsesMetricSystem",
        HostConstant::NSString(kCFLocaleUsesMetricSystem),
    ),
    (
        "_kCFLocaleMeasurementSystem",
        HostConstant::NSString(kCFLocaleMeasurementSystem),
    ),
    (
        "_kCFLocaleDecimalSeparator",
        HostConstant::NSString(kCFLocaleDecimalSeparator),
    ),
    (
        "_kCFLocaleGroupingSeparator",
        HostConstant::NSString(kCFLocaleGroupingSeparator),
    ),
    (
        "_kCFLocaleCurrencySymbol",
        HostConstant::NSString(kCFLocaleCurrencySymbol),
    ),
    (
        "_kCFLocaleCurrencyCode",
        HostConstant::NSString(kCFLocaleCurrencyCode),
    ),
    (
        "_kCFLocaleCollatorIdentifier",
        HostConstant::NSString(kCFLocaleCollatorIdentifier),
    ),
    (
        "_kCFLocaleQuotationBeginDelimiterKey",
        HostConstant::NSString(kCFLocaleQuotationBeginDelimiterKey),
    ),
    (
        "_kCFLocaleQuotationEndDelimiterKey",
        HostConstant::NSString(kCFLocaleQuotationEndDelimiterKey),
    ),
    (
        "_kCFLocaleAlternateQuotationBeginDelimiterKey",
        HostConstant::NSString(kCFLocaleAlternateQuotationBeginDelimiterKey),
    ),
    (
        "_kCFLocaleAlternateQuotationEndDelimiterKey",
        HostConstant::NSString(kCFLocaleAlternateQuotationEndDelimiterKey),
    ),
    // Notification name (CFNotificationCenter.h). Chrome reads it to observe
    // locale changes; touchHLE never fires it, but the constant must resolve.
    (
        "_kCFLocaleCurrentLocaleDidChangeNotification",
        HostConstant::NSString("CFLocaleCurrentLocaleDidChangeNotification"),
    ),
    // Calendar identifiers
    (
        "_kCFGregorianCalendar",
        HostConstant::NSString(kCFGregorianCalendar),
    ),
    (
        "_kCFBuddhistCalendar",
        HostConstant::NSString(kCFBuddhistCalendar),
    ),
    (
        "_kCFChineseCalendar",
        HostConstant::NSString(kCFChineseCalendar),
    ),
    (
        "_kCFHebrewCalendar",
        HostConstant::NSString(kCFHebrewCalendar),
    ),
    (
        "_kCFIslamicCalendar",
        HostConstant::NSString(kCFIslamicCalendar),
    ),
    (
        "_kCFIslamicCivilCalendar",
        HostConstant::NSString(kCFIslamicCivilCalendar),
    ),
    (
        "_kCFIslamicTabularCalendar",
        HostConstant::NSString(kCFIslamicTabularCalendar),
    ),
    (
        "_kCFIslamicUmmAlQuraCalendar",
        HostConstant::NSString(kCFIslamicUmmAlQuraCalendar),
    ),
    (
        "_kCFJapaneseCalendar",
        HostConstant::NSString(kCFJapaneseCalendar),
    ),
    (
        "_kCFRepublicOfChinaCalendar",
        HostConstant::NSString(kCFRepublicOfChinaCalendar),
    ),
    (
        "_kCFPersianCalendar",
        HostConstant::NSString(kCFPersianCalendar),
    ),
    (
        "_kCFIndianCalendar",
        HostConstant::NSString(kCFIndianCalendar),
    ),
    (
        "_kCFISO8601Calendar",
        HostConstant::NSString(kCFISO8601Calendar),
    ),
    (
        "_kCFCopticCalendar",
        HostConstant::NSString(kCFCopticCalendar),
    ),
    (
        "_kCFEthiopicAmeteMihretCalendar",
        HostConstant::NSString(kCFEthiopicAmeteMihretCalendar),
    ),
    (
        "_kCFEthiopicAmeteAlemCalendar",
        HostConstant::NSString(kCFEthiopicAmeteAlemCalendar),
    ),
];

// MARK: - Helpers

/// Null-safe allocator check: NULL, `kCFAllocatorDefault` and the system
/// default allocator are accepted. (Reading through a NULL allocator pointer
/// would crash, so this must not be written as a bare `env.mem.read`.)
fn validate_allocator(env: &mut Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault
        || allocator.is_null()
        || env.mem.read(allocator).is_system_default()
}

/// Normalise a locale identifier: language lower-case, 4-letter script in
/// Title case, 2-letter / 3-digit region upper-case, subtags joined with
/// `sep`. A trailing `@keyword=value` section is kept as-is.
fn canonicalize_locale_id(s: &str, sep: char) -> String {
    let (base, suffix) = match s.find('@') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let mut out = String::new();
    for (i, part) in base
        .split(|c| c == '_' || c == '-')
        .filter(|p| !p.is_empty())
        .enumerate()
    {
        if i > 0 {
            out.push(sep);
        }
        let all_alpha = part.chars().all(|c| c.is_ascii_alphabetic());
        let all_digit = part.chars().all(|c| c.is_ascii_digit());
        if i == 0 {
            out.push_str(&part.to_ascii_lowercase());
        } else if part.len() == 4 && all_alpha {
            let mut chars = part.chars();
            out.push(chars.next().unwrap().to_ascii_uppercase());
            out.push_str(&chars.as_str().to_ascii_lowercase());
        } else if (part.len() == 2 && all_alpha) || (part.len() == 3 && all_digit) {
            out.push_str(&part.to_ascii_uppercase());
        } else {
            out.push_str(part);
        }
    }
    out.push_str(suffix);
    out
}

/// Primary language subtag of an identifier like "ar", "ar_EG" or "zh-Hans",
/// lower-cased. Empty if the input has no alphabetic language code.
fn primary_language(s: &str) -> String {
    let lang = s
        .split(|c| c == '_' || c == '-')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_alphabetic()) {
        lang
    } else {
        String::new()
    }
}

// MARK: - Retain / Release

pub fn CFLocaleRetain(env: &mut Environment, locale: CFLocaleRef) -> CFLocaleRef {
    if !locale.is_null() {
        CFRetain(env, locale)
    } else {
        locale
    }
}

pub fn CFLocaleRelease(env: &mut Environment, locale: CFLocaleRef) {
    if !locale.is_null() {
        CFRelease(env, locale);
    }
}

// MARK: - Type info

fn CFLocaleGetTypeID(_env: &mut Environment) -> u32 {
    // Stable non-zero value; apps only compare it against CFGetTypeID().
    0x4346_4C4F // 'CFLO'
}

// MARK: - Constructors

fn CFLocaleCopyCurrent(env: &mut Environment) -> CFLocaleRef {
    let locale: id = msg_class![env; NSLocale currentLocale];
    msg![env; locale copy]
}

fn CFLocaleCreateCopy(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    locale: CFLocaleRef,
) -> CFLocaleRef {
    if locale.is_null() {
        return nil;
    }
    msg![env; locale copy]
}

fn CFLocaleCopyPreferredLanguages(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale preferredLanguages];
    msg![env; arr copy]
}

fn CFLocaleCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    locale_identifier: CFLocaleIdentifier,
) -> CFLocaleRef {
    if !validate_allocator(env, allocator) || locale_identifier.is_null() {
        return nil;
    }
    let new: id = msg_class![env; NSLocale alloc];
    msg![env; new initWithLocaleIdentifier:locale_identifier]
}

fn CFLocaleGetSystem(env: &mut Environment) -> CFLocaleRef {
    msg_class![env; NSLocale systemLocale]
}

// MARK: - Identifier helpers

fn CFLocaleCreateCanonicalLocaleIdentifierFromString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    locale_identifier: CFStringRef,
) -> CFLocaleIdentifier {
    if !validate_allocator(env, allocator) || locale_identifier.is_null() {
        return nil;
    }
    let s = ns_string::to_rust_string(env, locale_identifier).into_owned();
    let canonical = canonicalize_locale_id(&s, '_');
    // from_rust_string returns +1, which is what a Create function owes.
    ns_string::from_rust_string(env, canonical)
}

fn CFLocaleCreateCanonicalLanguageIdentifierFromString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    locale_identifier: CFStringRef,
) -> CFLocaleIdentifier {
    if !validate_allocator(env, allocator) || locale_identifier.is_null() {
        return nil;
    }
    // Language identifiers use hyphens ("en-US", "zh-Hans").
    let s = ns_string::to_rust_string(env, locale_identifier).into_owned();
    let canonical = canonicalize_locale_id(&s, '-');
    ns_string::from_rust_string(env, canonical)
}

fn CFLocaleCreateLocaleIdentifierFromComponents(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    dictionary: CFTypeRef, // CFDictionaryRef
) -> CFLocaleIdentifier {
    if dictionary.is_null() {
        return nil;
    }
    // Delegate to NSLocale.
    let ns: id = msg_class![env; NSLocale localeIdentifierFromComponents:dictionary];
    if ns.is_null() {
        return nil;
    }
    msg![env; ns copy]
}

fn CFLocaleCreateComponentsFromLocaleIdentifier(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    locale_identifier: CFLocaleIdentifier,
) -> CFTypeRef {
    if locale_identifier.is_null() {
        return nil;
    }
    // CFDictionaryRef. NSLocale returns it autoreleased, but this is a Create
    // function, so the caller expects to own it.
    let dict: id = msg_class![env; NSLocale componentsFromLocaleIdentifier:locale_identifier];
    if dict.is_null() {
        return nil;
    }
    CFRetain(env, dict)
}

fn CFLocaleCreateLocaleIdentifierFromWindowsLocaleCode(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    _lcid: u32,
) -> CFLocaleIdentifier {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    // Stub — return "en_US" (+1).
    ns_string::from_rust_string(env, "en_US".to_string())
}

fn CFLocaleGetWindowsLocaleCodeFromLocaleIdentifier(
    _env: &mut Environment,
    _locale_identifier: CFLocaleIdentifier,
) -> u32 {
    // 0x0409 = en-US LCID.
    0x0409
}

fn CFLocaleCreateCanonicalLocaleIdentifierFromScriptManagerCodes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    _lcode: i32, // LangCode (i16 in the C API)
    _rcode: i32, // RegionCode (i16 in the C API)
) -> CFLocaleIdentifier {
    if !validate_allocator(env, allocator) {
        return nil;
    }
    log_dbg!("CFLocaleCreateCanonicalLocaleIdentifierFromScriptManagerCodes: stub, returning en_US");
    ns_string::from_rust_string(env, "en_US".to_string())
}

// MARK: - Language direction

fn CFLocaleGetLanguageCharacterDirection(
    env: &mut Environment,
    iso_lang_code: CFStringRef,
) -> CFLocaleLanguageDirection {
    if iso_lang_code.is_null() {
        return kCFLocaleLanguageDirectionUnknown;
    }
    let s = ns_string::to_rust_string(env, iso_lang_code).into_owned();
    let lang = primary_language(&s);
    match lang.as_str() {
        "" => kCFLocaleLanguageDirectionUnknown,
        // Languages written right-to-left.
        "ar" | "fa" | "he" | "iw" | "ur" | "yi" | "ji" | "ps" | "sd" | "ug" | "dv" | "ckb" => {
            kCFLocaleLanguageDirectionRightToLeft
        }
        _ => kCFLocaleLanguageDirectionLeftToRight,
    }
}

fn CFLocaleGetLanguageLineDirection(
    env: &mut Environment,
    iso_lang_code: CFStringRef,
) -> CFLocaleLanguageDirection {
    if iso_lang_code.is_null() {
        return kCFLocaleLanguageDirectionUnknown;
    }
    let s = ns_string::to_rust_string(env, iso_lang_code).into_owned();
    if primary_language(&s).is_empty() {
        kCFLocaleLanguageDirectionUnknown
    } else {
        // Lines stack top-to-bottom for essentially every language.
        kCFLocaleLanguageDirectionTopToBottom
    }
}

// MARK: - Value accessors

fn CFLocaleGetValue(env: &mut Environment, locale: CFLocaleRef, key: CFLocaleKey) -> CFTypeRef {
    msg![env; locale objectForKey:key]
}

fn CFLocaleCopyDisplayNameForPropertyValue(
    env: &mut Environment,
    display_locale: CFLocaleRef,
    key: CFLocaleKey,
    value: CFStringRef,
) -> CFStringRef {
    // Delegate to NSLocale displayNameForKey:value:
    let result: id = msg![env; display_locale displayNameForKey:key value:value];
    if result.is_null() {
        return nil;
    }
    msg![env; result copy]
}

fn CFLocaleGetIdentifier(env: &mut Environment, locale: CFLocaleRef) -> CFLocaleIdentifier {
    msg![env; locale localeIdentifier]
}

// MARK: - Available locales

fn CFLocaleCopyAvailableLocaleIdentifiers(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale availableLocaleIdentifiers];
    msg![env; arr copy]
}

fn CFLocaleCopyISOLanguageCodes(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale ISOLanguageCodes];
    msg![env; arr copy]
}

fn CFLocaleCopyISOCountryCodes(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale ISOCountryCodes];
    msg![env; arr copy]
}

fn CFLocaleCopyISOCurrencyCodes(env: &mut Environment) -> CFArrayRef {
    let arr: id = msg_class![env; NSLocale ISOCurrencyCodes];
    msg![env; arr copy]
}

fn CFLocaleCopyCommonISOCurrencyCodes(env: &mut Environment) -> CFArrayRef {
    // Built on the host so this does not depend on NSLocale implementing
    // +commonISOCurrencyCodes. `new` gives +1, as a Copy function requires.
    const CODES: [&str; 35] = [
        "AED", "ARS", "AUD", "BRL", "CAD", "CHF", "CLP", "CNY", "COP", "CZK", "DKK", "EUR", "GBP",
        "HKD", "HUF", "IDR", "ILS", "INR", "JPY", "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PLN",
        "RUB", "SAR", "SEK", "SGD", "THB", "TRY", "TWD", "USD", "ZAR",
    ];
    let arr: id = msg_class![env; NSMutableArray new];
    for code in CODES {
        let s = ns_string::get_static_str(env, code);
        let _: () = msg![env; arr addObject:s];
    }
    arr
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFLocaleRetain(_)),
    export_c_func!(CFLocaleRelease(_)),
    export_c_func!(CFLocaleGetTypeID()),
    export_c_func!(CFLocaleCopyCurrent()),
    export_c_func!(CFLocaleCreateCopy(_, _)),
    export_c_func!(CFLocaleCopyPreferredLanguages()),
    export_c_func!(CFLocaleCreate(_, _)),
    export_c_func!(CFLocaleGetSystem()),
    export_c_func!(CFLocaleCreateCanonicalLocaleIdentifierFromString(_, _)),
    export_c_func!(CFLocaleCreateCanonicalLanguageIdentifierFromString(_, _)),
    export_c_func!(CFLocaleCreateCanonicalLocaleIdentifierFromScriptManagerCodes(
        _, _, _
    )),
    export_c_func!(CFLocaleCreateLocaleIdentifierFromComponents(_, _)),
    export_c_func!(CFLocaleCreateComponentsFromLocaleIdentifier(_, _)),
    export_c_func!(CFLocaleCreateLocaleIdentifierFromWindowsLocaleCode(_, _)),
    export_c_func!(CFLocaleGetWindowsLocaleCodeFromLocaleIdentifier(_)),
    export_c_func!(CFLocaleGetLanguageCharacterDirection(_)),
    export_c_func!(CFLocaleGetLanguageLineDirection(_)),
    export_c_func!(CFLocaleGetValue(_, _)),
    export_c_func!(CFLocaleCopyDisplayNameForPropertyValue(_, _, _)),
    export_c_func!(CFLocaleGetIdentifier(_)),
    export_c_func!(CFLocaleCopyAvailableLocaleIdentifiers()),
    export_c_func!(CFLocaleCopyISOLanguageCodes()),
    export_c_func!(CFLocaleCopyISOCountryCodes()),
    export_c_func!(CFLocaleCopyISOCurrencyCodes()),
    export_c_func!(CFLocaleCopyCommonISOCurrencyCodes()),
];
