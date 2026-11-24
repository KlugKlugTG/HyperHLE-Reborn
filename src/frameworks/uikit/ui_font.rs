/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIFont`.

use super::ui_graphics::UIGraphicsGetCurrentContext;
use crate::font::{Font, TextAlignment, WrapMode};
use crate::frameworks::core_graphics::cg_bitmap_context::CGBitmapContextDrawer;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Default)]
pub(super) struct State {
    registered_fonts: HashMap<String, Font>,
    fonts: HashMap<FontKind, Font>,
    sans_regular_ja: Option<Font>,
    sans_bold_ja: Option<Font>,
    sans_regular_zh: Option<Font>,
    sans_bold_zh: Option<Font>,
    sans_regular_ar: Option<Font>,
    sans_bold_ar: Option<Font>,
}
impl State {
    fn get_font_by_kind(&mut self, font_kind: FontKind) -> &Font {
        self.fonts
            .entry(font_kind)
            .or_insert_with(|| match font_kind {
                FontKind::MonoRegular => Font::mono_regular(),
                FontKind::MonoBold => Font::mono_bold(),
                FontKind::MonoBoldItalic => Font::mono_bold_italic(),
                FontKind::MonoItalic => Font::mono_italic(),
                FontKind::SansRegular => Font::sans_regular(),
                FontKind::SansBold => Font::sans_bold(),
                FontKind::SansBoldItalic => Font::sans_bold_italic(),
                FontKind::SansItalic => Font::sans_italic(),
                FontKind::SerifRegular => Font::serif_regular(),
                FontKind::SerifBold => Font::serif_bold(),
                FontKind::SerifBoldItalic => Font::serif_bold_italic(),
                FontKind::SerifItalic => Font::serif_italic(),
            })
    }
}

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
enum FontKind {
    #[default]
    MonoRegular,
    MonoBold,
    MonoBoldItalic,
    MonoItalic,
    SansRegular,
    SansBold,
    SansBoldItalic,
    SansItalic,
    SerifRegular,
    SerifBold,
    SerifBoldItalic,
    SerifItalic,
}

#[derive(Default)]
struct UIFontHostObject {
    size: CGFloat,
    kind: FontKind,
    /// A resolved font face that is not represented by `kind`, such as a
    /// Core Graphics font or a legacy system-font alias.
    custom: Option<Font>,
}
impl HostObject for UIFontHostObject {}

#[derive(Copy, Clone, PartialEq, Eq)]
enum FontFace {
    BuiltIn(FontKind),
    SimplifiedChinese,
}

/// Resolve legacy iOS font names used by the Chinese-text compatibility fix.
/// These CJK/Unicode aliases are routed to touchHLE's Simplified Chinese
/// fallback; other names keep the existing built-in font substitutions. This
/// shared mapping keeps UIKit, Core Text, and Core Graphics on the same font
/// data and Unicode cmap.
fn resolve_legacy_chinese_font_face(name: &str) -> FontFace {
    match name {
        "ArialUnicodeMS" | "Arial Unicode MS" | "STHeitiSC-Light" | "STHeitiSC-Medium"
        | "STHeitiSC-Regular" | "STHeitiTC-Light" | "STHeitiTC-Medium" | "STHeiti-Light"
        | "STHeiti-Medium" => FontFace::SimplifiedChinese,
        _ => FontFace::BuiltIn(get_equivalent_font(name).unwrap_or(FontKind::SansRegular)),
    }
}

fn resolve_font_name(env: &mut Environment, name: &str) -> (FontKind, Option<Font>) {
    let registered = &env.framework_state.uikit.ui_font.registered_fonts;
    if let Some(font) = registered.get(name).cloned().or_else(|| {
        registered
            .values()
            .find(|font| {
                font.name(1).as_deref() == Some(name) || font.name(4).as_deref() == Some(name)
            })
            .cloned()
    }) {
        return (FontKind::SansRegular, Some(font));
    }
    match resolve_legacy_chinese_font_face(name) {
        FontFace::BuiltIn(kind) => (kind, None),
        FontFace::SimplifiedChinese => {
            let font = env
                .framework_state
                .uikit
                .ui_font
                .sans_regular_zh
                .get_or_insert_with(Font::sans_regular_zh)
                .clone();
            (FontKind::SansRegular, Some(font))
        }
    }
}

/// Create a UIFont using the shared resolver used by the Core Text and
/// Core Graphics font-name entry points.
pub fn new_uifont_with_name(env: &mut Environment, name: &str, size: CGFloat) -> id {
    let (kind, custom) = resolve_font_name(env, name);
    let host_object = UIFontHostObject { size, kind, custom };
    let class = env.objc.get_known_class("UIFont", &mut env.mem);
    let new = env
        .objc
        .alloc_object(class, Box::new(host_object), &mut env.mem);
    autorelease(env, new)
}

pub type UILineBreakMode = NSInteger;
pub const UILineBreakModeWordWrap: UILineBreakMode = 0;
pub const UILineBreakModeCharacterWrap: UILineBreakMode = 1;
#[allow(dead_code)]
pub const UILineBreakModeClip: UILineBreakMode = 2;
#[allow(dead_code)]
pub const UILineBreakModeHeadTruncation: UILineBreakMode = 3;
pub const UILineBreakModeTailTruncation: UILineBreakMode = 4;
#[allow(dead_code)]
pub const UILineBreakModeMiddleTruncation: UILineBreakMode = 5;

pub type UITextAlignment = NSInteger;
pub const UITextAlignmentLeft: UITextAlignment = 0;
pub const UITextAlignmentCenter: UITextAlignment = 1;
pub const UITextAlignmentRight: UITextAlignment = 2;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);
@implementation UIFont: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = UIFontHostObject {
        size: 17.0,
        kind: FontKind::SansRegular,
        custom: None,
    };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

+ (CGFloat)systemFontSize {
    14.0
}

+ (CGFloat)smallSystemFontSize {
    12.0
}

+ (CGFloat)labelFontSize {
    17.0
}

+ (CGFloat)buttonFontSize {
    18.0
}

+ (id)systemFontOfSize:(CGFloat)size {
    let host_object = UIFontHostObject {
        size,
        kind: FontKind::SansRegular,
        custom: None,
    };
    let new = env.objc.alloc_object(this, Box::new(host_object), &mut env.mem);
    autorelease(env, new)
}

+ (id)boldSystemFontOfSize:(CGFloat)size {
    let size = if size.is_finite() && size > 0.0 {
        size
    } else {
        14.0
    };
    let host_object = UIFontHostObject {
        size,
        kind: FontKind::SansBold,
        custom: None,
    };
    let new = env.objc.alloc_object(this, Box::new(host_object), &mut env.mem);
    autorelease(env, new)
}

// preferredFontForTextStyle: (iOS 7+) — sizes per Apple's documented
// defaults for the Large content size category. See:
// https://developer.apple.com/documentation/uikit/uifont/textstyle
+ (id)preferredFontForTextStyle:(id)style { // NSString*
    let style_name = if style != nil {
        to_rust_string(env, style).into_owned()
    } else {
        String::new()
    };
    let (size, bold) = match style_name.as_str() {
        "UICTFontTextStyleLargeTitle" | "UIFontTextStyleLargeTitle" => (34.0, false),
        "UICTFontTextStyleTitle0"     | "UIFontTextStyleTitle1"     => (28.0, false),
        "UICTFontTextStyleTitle1"     | "UIFontTextStyleTitle2"     => (22.0, false),
        "UICTFontTextStyleTitle2"     | "UIFontTextStyleTitle3"     => (20.0, false),
        "UICTFontTextStyleHeadline"   | "UIFontTextStyleHeadline"   => (17.0, true),
        "UICTFontTextStyleBody"       | "UIFontTextStyleBody"       => (17.0, false),
        "UICTFontTextStyleCallout"    | "UIFontTextStyleCallout"    => (16.0, false),
        "UICTFontTextStyleSubhead"    | "UIFontTextStyleSubheadline"=> (15.0, false),
        "UICTFontTextStyleFootnote"   | "UIFontTextStyleFootnote"   => (13.0, false),
        "UICTFontTextStyleCaption1"   | "UIFontTextStyleCaption1"   => (12.0, false),
        "UICTFontTextStyleCaption2"   | "UIFontTextStyleCaption2"   => (11.0, false),
        // Apple's docs: an unknown style falls back to Body size.
        _ => (17.0, false),
    };
    if bold {
        msg_class![env; UIFont boldSystemFontOfSize:(size as CGFloat)]
    } else {
        msg_class![env; UIFont systemFontOfSize:(size as CGFloat)]
    }
}

// familyNames / fontNamesForFamilyName: — these enumerate every font family
// the emulator can serve. The three families below cover Apple's documented
// iPhone OS 2.x default set (Helvetica, Times New Roman, Courier New) which
// is what touchHLE actually has fallback glyph tables for.
+ (id)familyNames {
    let mut names: Vec<String> = ["Courier New", "Helvetica", "Times New Roman"].into_iter().map(str::to_owned).collect();
    names.extend(env.framework_state.uikit.ui_font.registered_fonts.values().filter_map(|font| font.name(1)));
    names.sort();
    names.dedup();
    let arr: id = msg_class![env; NSMutableArray new];
    for name in names {
        let ns = from_rust_string(env, name);
        let _: () = msg![env; arr addObject:ns];
        crate::objc::release(env, ns);
    }
    autorelease(env, arr)
}

+ (id)fontNamesForFamilyName:(id)family_name { // NSString*
    if family_name == nil { return nil; }
    let s = to_rust_string(env, family_name).into_owned();
    let names: &[&str] = match s.as_str() {
        "Courier New" => &[
            "CourierNewPSMT",
            "CourierNewPS-BoldMT",
            "CourierNewPS-ItalicMT",
            "CourierNewPS-BoldItalicMT",
        ],
        "Helvetica" => &[
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-Oblique",
            "Helvetica-BoldOblique",
        ],
        "Times New Roman" => &[
            "TimesNewRomanPSMT",
            "TimesNewRomanPS-BoldMT",
            "TimesNewRomanPS-ItalicMT",
            "TimesNewRomanPS-BoldItalicMT",
        ],
        // Apple returns an empty array for unknown families; matching that.
        _ => &[],
    };
    let arr: id = msg_class![env; NSMutableArray new];
    for &name in names {
        let ns = get_static_str(env, name);
        let _: () = msg![env; arr addObject:ns];
    }
    let mut registered: Vec<String> = env.framework_state.uikit.ui_font.registered_fonts.values()
        .filter(|font| font.name(1).as_deref() == Some(s.as_str()))
        .filter_map(|font| font.name(6)).collect();
    registered.sort();
    registered.dedup();
    for name in registered {
        let ns = from_rust_string(env, name);
        () = msg![env; arr addObject:ns];
        crate::objc::release(env, ns);
    }
    autorelease(env, arr)
}

+ (id)italicSystemFontOfSize:(CGFloat)size {
    let host_object = UIFontHostObject {
        size,
        kind: FontKind::SansItalic,
        custom: None,
    };
    let new = env.objc.alloc_object(this, Box::new(host_object), &mut env.mem);
    autorelease(env, new)
}

+ (id)fontWithName:(id)fontName size:(CGFloat)fontSize {
    if fontName == nil {
        return new_uifont_with_name(env, "Helvetica", fontSize);
    }
    let font_name = to_rust_string(env, fontName).to_string();
    new_uifont_with_name(env, &font_name, fontSize)
}

- (id)initWithCoder:(id)coder {
    let key_name = get_static_str(env, "UIFontName");
    let mut font_name: id = nil;
    if msg![env; coder containsValueForKey:key_name] {
        font_name = msg![env; coder decodeObjectForKey:key_name];
    }

    let key_size = get_static_str(env, "UIFontPointSize");
    let mut font_size: f32 = 17.0;
    if msg![env; coder containsValueForKey:key_size] {
        font_size = msg![env; coder decodeFloatForKey:key_size];
    }

    let (kind, custom) = if font_name != nil {
        let name = to_rust_string(env, font_name).to_string();
        resolve_font_name(env, &name)
    } else {
        (FontKind::SansRegular, None)
    };
    let host = env.objc.borrow_mut::<UIFontHostObject>(this);
    host.size = font_size as CGFloat;
    host.kind = kind;
    host.custom = custom;

    this
}

// MARK: - Typography metrics
//
// These are derived from the underlying font's ascent and the documented
// proportions for the iOS system fonts (Helvetica / Helvetica Neue). For
// fonts where we don't have native metrics they are approximations rather
// than measured values.

- (CGFloat)capHeight {
    let (font, size) = host_font(env, this);
    // Cap height is typically ~73% of the ascent for Helvetica-family fonts.
    (font.ascent(size) * 0.73).round()
}

- (CGFloat)xHeight {
    let (font, size) = host_font(env, this);
    // X-height is typically ~54% of the ascent for Helvetica-family fonts.
    (font.ascent(size) * 0.54).round()
}

- (CGFloat)underlinePosition {
    // Apple specifies a negative offset from the baseline. For Helvetica at
    // 17pt the documented value is approximately -1.7; we generalize that
    // as -ceil(pointSize / 10), clamped so the absolute value is at least 1.
    let size = env.objc.borrow::<UIFontHostObject>(this).size;
    -(size * 0.1).round().max(1.0)
}

- (CGFloat)underlineThickness {
    // Roughly 1/14 of the point size, minimum 1pt — matches what iOS
    // reports for Helvetica.
    let size = env.objc.borrow::<UIFontHostObject>(this).size;
    (size / 14.0).round().max(1.0)
}

- (CGFloat)italicAngle {
    let kind = env.objc.borrow::<UIFontHostObject>(this).kind;
    match kind {
        FontKind::SansItalic
        | FontKind::SansBoldItalic
        | FontKind::SerifItalic
        | FontKind::SerifBoldItalic
        | FontKind::MonoItalic
        | FontKind::MonoBoldItalic => -12.0, // degrees, matches Helvetica-Oblique
        _ => 0.0,
    }
}

// NSCopying — UIFont is immutable, so `-copy` returns `self` retained, the
// same shortcut Apple documents.
- (id)copyWithZone:(NSZonePtr)_zone {
    crate::objc::retain(env, this)
}

- (id)description {
    let (size, kind) = {
        let host_object = env.objc.borrow::<UIFontHostObject>(this);
        (host_object.size, host_object.kind)
    };
    let name = font_kind_to_name(kind);
    let s = format!(
        "<UICTFont: {:?}; font-family: \"{}\"; font-weight: normal; \
         font-style: normal; font-size: {} pt>",
        this, name, size
    );
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

- (bool)isEqual:(id)other {
    if this == other { return true; }
    if other == nil  { return false; }
    // Check that `other` is actually a UIFont before borrowing.
    let ui_font_class = env.objc.get_known_class("UIFont", &mut env.mem);
    let other_class: id = msg![env; other class];
    let is_font: bool = msg![env; other_class isSubclassOfClass:ui_font_class];
    if !is_font { return false; }
    let (a_kind, a_size, a_custom) = {
        let a = env.objc.borrow::<UIFontHostObject>(this);
        (a.kind, a.size, a.custom.clone())
    };
    let (b_kind, b_size, b_custom) = {
        let b = env.objc.borrow::<UIFontHostObject>(other);
        (b.kind, b.size, b.custom.clone())
    };
    let same_face = match (a_custom, b_custom) {
        (Some(a), Some(b)) => a.raw_data() == b.raw_data(),
        (None, None) => a_kind == b_kind,
        _ => false,
    };
    same_face && a_size == b_size
}

- (CGFloat)pointSize {
    let host_object = env.objc.borrow::<UIFontHostObject>(this);
    host_object.size
}

- (CGFloat)ascender {
    let (font, size) = host_font(env, this);
    font.ascent(size)
}

- (CGFloat)descender {
    let (font, size) = host_font(env, this);
    font.descent(size)
}

- (CGFloat)leading {
    let (font, size) = host_font(env, this);
    // This _mostly_ lines up with what is reported on actual devices. It
    // seems there's variance between what apple and rusttype report for
    // leading/descent values, which is probably to be expected.
    //
    // As for what the 1.575 is doing here, I don't know. It's probably not
    // the right value, it's just (close) to a linear regression of size/leading
    // for Liberation Sans, and it mostly makes it work.
    (font.ascent(size) - font.descent(size) + font.line_gap(size) + 1.575).round()
}

- (CGFloat)lineHeight {
    let ascender: CGFloat = msg![env; this ascender];
    let descender: CGFloat = msg![env; this descender];
    let leading: CGFloat = msg![env; this leading];
    ascender + leading - descender
}

- (id)fontWithSize:(CGFloat)size {
    let (kind, custom) = {
        let host_object = env.objc.borrow::<UIFontHostObject>(this);
        (host_object.kind, host_object.custom.clone())
    };
    let host_object = UIFontHostObject { size, kind, custom };
    let class_ptr = env.objc.get_known_class("UIFont", &mut env.mem);
    let new_font = env.objc.alloc_object(class_ptr, Box::new(host_object), &mut env.mem);
    autorelease(env, new_font)
}

- (id)fontName {
    let host = env.objc.borrow::<UIFontHostObject>(this);
    let name = host.custom.as_ref().and_then(|font| font.name(6))
        .unwrap_or_else(|| font_kind_to_name(host.kind).to_string());
    let ns = from_rust_string(env, name);
    autorelease(env, ns)
}

- (id)familyName {
    let host = env.objc.borrow::<UIFontHostObject>(this);
    let family = host.custom.as_ref().and_then(|font| font.name(1))
        .unwrap_or_else(|| font_kind_to_family(host.kind).to_string());
    let ns = from_rust_string(env, family);
    autorelease(env, ns)
}

@end

};

/// Returns `true` if `obj` is a UIFont instance (backed by [UIFontHostObject]).
pub fn is_uifont(env: &mut Environment, obj: crate::objc::id) -> bool {
    if obj.is_null() {
        return false;
    }
    let class = env.objc.get_known_class("UIFont", &mut env.mem);
    let obj_class = crate::objc::ObjC::read_isa(obj, &env.mem);
    obj_class == class || env.objc.class_is_subclass_of(obj_class, class)
}

/// The font a UIFont draws with and its point size: the app's own font if it
/// has one, otherwise the built-in font of its kind.
fn host_font(env: &mut Environment, this: id) -> (Font, CGFloat) {
    let host_object = env.objc.borrow::<UIFontHostObject>(this);
    let font = match &host_object.custom {
        Some(font) => font.clone(),
        None => env
            .framework_state
            .uikit
            .ui_font
            .get_font_by_kind(host_object.kind)
            .clone(),
    };
    (font, host_object.size)
}

/// Returns the [Font] backing a UIFont object.
/// Returns `None` if `obj` is not a UIFont.
pub fn font_from_uifont(env: &mut Environment, obj: crate::objc::id) -> Option<Font> {
    if !is_uifont(env, obj) {
        return None;
    }
    Some(host_font(env, obj).0)
}

/// The app-supplied font backing a UIFont, if it has one (see
/// [new_uifont_with_custom_font]).
pub fn custom_font_of_uifont(env: &mut Environment, obj: crate::objc::id) -> Option<Font> {
    if !is_uifont(env, obj) {
        return None;
    }
    env.objc.borrow::<UIFontHostObject>(obj).custom.clone()
}

/// Create an (autoreleased) UIFont that draws with `font`, a font the app
/// supplied itself rather than one of touchHLE's built-in fonts.
pub fn new_uifont_with_custom_font(env: &mut Environment, font: Font, size: CGFloat) -> id {
    let host_object = UIFontHostObject {
        size,
        kind: FontKind::SansRegular,
        custom: Some(font),
    };
    let class = env.objc.get_known_class("UIFont", &mut env.mem);
    let new = env
        .objc
        .alloc_object(class, Box::new(host_object), &mut env.mem);
    autorelease(env, new)
}

/// Process-scoped registration retains the font data independently of its
/// CGFont reference, matching CTFontManagerRegisterGraphicsFont.
pub(crate) fn register_graphics_font(env: &mut Environment, font: Font) -> bool {
    let Some(name) = font.name(6) else {
        return false;
    };
    let fonts = &mut env.framework_state.uikit.ui_font.registered_fonts;
    if fonts.contains_key(&name) {
        return false;
    }
    fonts.insert(name, font);
    true
}

pub(crate) fn unregister_graphics_font(env: &mut Environment, font: &Font) -> bool {
    font.name(6).is_some_and(|name| {
        env.framework_state
            .uikit
            .ui_font
            .registered_fonts
            .remove(&name)
            .is_some()
    })
}

fn convert_line_break_mode(ui_mode: UILineBreakMode) -> WrapMode {
    match ui_mode {
        UILineBreakModeWordWrap => WrapMode::Word,
        UILineBreakModeCharacterWrap => WrapMode::Char,
        UILineBreakModeTailTruncation => WrapMode::Word,
        _ => WrapMode::Word,
    }
}

/// Returns `true` if the codepoint belongs to one of the CJK (Chinese,
/// Japanese, Korean) Unicode blocks that the bundled Latin/serif fonts can't
/// render and which require the Noto Sans CJK fallback.
fn is_cjk_char(c: u32) -> bool {
    (0x2E80..=0x2FFF).contains(&c)
        || (0x3000..=0x30FF).contains(&c)
        || (0x3100..=0x312F).contains(&c)
        || (0x3130..=0x318F).contains(&c)
        || (0x3190..=0x31FF).contains(&c)
        || (0x3200..=0x33FF).contains(&c)
        || (0xFF00..=0xFFEF).contains(&c)
        || (0x4E00..=0x9FFF).contains(&c)
        || (0x3400..=0x4DBF).contains(&c)
        || (0xA000..=0xA4CF).contains(&c)
        || (0xAC00..=0xD7AF).contains(&c)
        || (0xF900..=0xFAFF).contains(&c)
        || (0xFE30..=0xFE4F).contains(&c)
        || (0x1B000..=0x1B16F).contains(&c)
        || (0x20000..=0x2FA1F).contains(&c)
        || (0x30000..=0x323AF).contains(&c)
}

/// Returns `true` if the codepoint belongs to one of the Arabic Unicode blocks.
/// The Latin/serif/CJK fonts have no Arabic glyphs, so such text needs the
/// Noto Sans Arabic fallback.
fn is_arabic_char(c: u32) -> bool {
    (0x0600..=0x06FF).contains(&c) || // Arabic
    (0x0750..=0x077F).contains(&c) || // Arabic Supplement
    (0x08A0..=0x08FF).contains(&c) || // Arabic Extended-A
    (0xFB50..=0xFDFF).contains(&c) || // Arabic Presentation Forms-A
    (0xFE70..=0xFEFF).contains(&c) // Arabic Presentation Forms-B
}

fn font_maps_text(font: &Font, text: &str) -> bool {
    text.chars()
        .all(|c| c.is_control() || font.glyph_id_for_unicode(c).0 != 0)
}

/// Select a touchHLE font with coverage for the supplied Unicode text.
#[rustfmt::skip]
fn get_font(env: &mut Environment, kind: FontKind, text: &str) -> Font {
    let mut needs_cjk = false;
    let mut needs_arabic = false;
    for c in text.chars() {
        let c = c as u32;
        if is_cjk_char(c) {
            needs_cjk = true;
        } else if is_arabic_char(c) {
            needs_arabic = true;
        }
    }

    // CJK takes priority over Arabic when both are present, matching the order
    // in which fallbacks were historically added; the common case is text
    // containing only one of the two scripts.
    let is_bold = matches!(
        kind,
        FontKind::MonoBold | FontKind::MonoBoldItalic |
        FontKind::SansBold | FontKind::SansBoldItalic |
        FontKind::SerifBold | FontKind::SerifBoldItalic
    );

    if needs_cjk {
        let state = &mut env.framework_state.uikit.ui_font;
        if is_bold {
            return state.sans_bold_zh.get_or_insert_with(Font::sans_bold_zh).clone();
        }
        return state.sans_regular_zh.get_or_insert_with(Font::sans_regular_zh).clone();
    }
    if needs_arabic {
        let state = &mut env.framework_state.uikit.ui_font;
        if is_bold {
            return state.sans_bold_ar.get_or_insert_with(Font::sans_bold_ar).clone();
        }
        return state.sans_regular_ar.get_or_insert_with(Font::sans_regular_ar).clone();
    }

    env.framework_state.uikit.ui_font.get_font_by_kind(kind).clone()
}

/// Select the font used for a particular string, applying touchHLE's built-in
/// Unicode fallbacks when the requested font cannot map the text.
pub fn font_for_text(env: &mut Environment, obj: id, text: &str) -> Option<Font> {
    if !is_uifont(env, obj) {
        return None;
    }
    let (kind, custom) = {
        let host = env.objc.borrow::<UIFontHostObject>(obj);
        (host.kind, host.custom.clone())
    };
    if let Some(font) = custom {
        if font_maps_text(&font, text) {
            return Some(font);
        }
    }
    Some(get_font(env, kind, text))
}

pub fn size_with_font(
    env: &mut Environment,
    font: id,
    text: &str,
    constrained: Option<(CGSize, UILineBreakMode)>,
) -> CGSize {
    let size = {
        let host_object = env.objc.borrow::<UIFontHostObject>(font);
        host_object.size
    };
    let font = font_for_text(env, font, text).unwrap_or_else(Font::sans_regular);
    let wrap = constrained.map(|(size, ui_mode)| (size.width, convert_line_break_mode(ui_mode)));

    let (width, height) = font.calculate_text_size(size, text, wrap);
    CGSize { width, height }
}

pub fn break_lines_with_font<'a>(
    env: &mut Environment,
    font: id,
    text: &'a str,
    constrained: Option<(CGSize, UILineBreakMode)>,
) -> Vec<(f32, &'a str)> {
    let size = {
        let host_object = env.objc.borrow::<UIFontHostObject>(font);
        host_object.size
    };
    let font = font_for_text(env, font, text).unwrap_or_else(Font::sans_regular);
    let wrap = constrained.map(|(size, ui_mode)| (size.width, convert_line_break_mode(ui_mode)));

    font.break_lines(size, text, wrap)
}

#[inline(always)]
pub fn draw_font_glyph(
    drawer: &mut CGBitmapContextDrawer,
    raster_glyph: crate::font::RasterGlyph,
    fill_color: (f32, f32, f32, f32),
    clip_x: Option<Range<f32>>,
    clip_y: Option<Range<f32>>,
) {
    let mut glyph_rect = {
        let (x, y) = raster_glyph.origin();
        let (width, height) = raster_glyph.dimensions();
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: width as f32,
                height: height as f32,
            },
        }
    };
    if let Some(clip_x) = clip_x {
        if glyph_rect.origin.x >= clip_x.end {
            return;
        }
        if glyph_rect.origin.x + glyph_rect.size.width > clip_x.end {
            glyph_rect.size.width = clip_x.end - glyph_rect.origin.x;
        }
    }
    if let Some(clip_y) = clip_y {
        if glyph_rect.origin.y >= clip_y.end {
            return;
        }
        if glyph_rect.origin.y + glyph_rect.size.height > clip_y.end {
            glyph_rect.size.height = clip_y.end - glyph_rect.origin.y;
        }
    }

    for ((x, y), (tex_x, tex_y)) in drawer.iter_transformed_pixels(glyph_rect) {
        let coverage = raster_glyph.pixel_at((
            (tex_x * glyph_rect.size.width - 0.5).round() as i32,
            (tex_y * glyph_rect.size.height - 0.5).round() as i32,
        ));
        let (r, g, b, a) = fill_color;
        let (r, g, b, a) = (r * coverage, g * coverage, b * coverage, a * coverage);
        drawer.put_pixel((x, y), (r, g, b, a), true);
    }
}

pub fn draw_at_point(
    env: &mut Environment,
    font: id,
    text: &str,
    point: CGPoint,
    width_and_line_break_mode: Option<(CGFloat, UILineBreakMode)>,
) -> CGSize {
    let context = UIGraphicsGetCurrentContext(env);
    let size = {
        let host_object = env.objc.borrow::<UIFontHostObject>(font);
        host_object.size
    };
    let font = font_for_text(env, font, text).unwrap_or_else(Font::sans_regular);
    let width_and_line_break_mode =
        width_and_line_break_mode.map(|(width, ui_mode)| (width, convert_line_break_mode(ui_mode)));
    let clip_x = width_and_line_break_mode.map(|(width, _)| point.x..(point.x + width));
    let (width, height) = font.calculate_text_size(size, text, width_and_line_break_mode);
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();
    font.draw(
        size,
        text,
        (point.x, point.y),
        width_and_line_break_mode,
        TextAlignment::Left,
        |raster_glyph| draw_font_glyph(&mut drawer, raster_glyph, fill_color, clip_x.clone(), None),
    );
    CGSize { width, height }
}

pub fn draw_in_rect(
    env: &mut Environment,
    font: id,
    text: &str,
    rect: CGRect,
    line_break_mode: UILineBreakMode,
    alignment: UITextAlignment,
) -> CGSize {
    let context = UIGraphicsGetCurrentContext(env);
    let text_size = size_with_font(env, font, text, Some((rect.size, line_break_mode)));

    let size = {
        let host_object = env.objc.borrow::<UIFontHostObject>(font);
        host_object.size
    };
    let font = font_for_text(env, font, text).unwrap_or_else(Font::sans_regular);
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();
    let (origin_x_offset, alignment) = match alignment {
        UITextAlignmentLeft => (0.0, TextAlignment::Left),
        UITextAlignmentCenter => (rect.size.width / 2.0, TextAlignment::Center),
        UITextAlignmentRight => (rect.size.width, TextAlignment::Right),
        _ => (0.0, TextAlignment::Left),
    };
    font.draw(
        size,
        text,
        (rect.origin.x + origin_x_offset, rect.origin.y),
        Some((rect.size.width, convert_line_break_mode(line_break_mode))),
        alignment,
        |raster_glyph| {
            draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                Some(rect.origin.x..(rect.origin.x + rect.size.width)),
                Some(rect.origin.y..(rect.origin.y + rect.size.height)),
            )
        },
    );
    text_size
}

#[rustfmt::skip]
fn get_equivalent_font(system_font: &str) -> Option<FontKind> {
    match system_font {
        "Courier"                          => Some(FontKind::MonoRegular),
        "Courier-Bold"                     => Some(FontKind::MonoBold),
        "Courier-Oblique"                  => Some(FontKind::MonoItalic),
        "Courier-BoldOblique"              => Some(FontKind::MonoBoldItalic),
        "CourierNewPSMT"                   => Some(FontKind::MonoRegular),
        "CourierNewPS-BoldMT"              => Some(FontKind::MonoBold),
        "CourierNewPS-ItalicMT"            => Some(FontKind::MonoItalic),
        "CourierNewPS-BoldItalicMT"        => Some(FontKind::MonoBoldItalic),
        "ArialMT"                          => Some(FontKind::SansRegular),
        "Arial-BoldMT"                     => Some(FontKind::SansBold),
        "Arial-ItalicMT"                   => Some(FontKind::SansItalic),
        "Arial-BoldItalicMT"               => Some(FontKind::SansBoldItalic),
        "ArialRoundedMTBold"               => Some(FontKind::SansBold),
        "Helvetica"                        => Some(FontKind::SansRegular),
        "Helvetica-Bold"                   => Some(FontKind::SansBold),
        "Helvetica-Oblique"                => Some(FontKind::SansItalic),
        "Helvetica-BoldOblique"            => Some(FontKind::SansBoldItalic),
        "Helvetica-Light"                  => Some(FontKind::SansRegular),
        "Helvetica-LightOblique"           => Some(FontKind::SansItalic),
        "Helvetica-Narrow"                 => Some(FontKind::SansRegular),
        "Helvetica-Narrow-Bold"            => Some(FontKind::SansBold),
        "Helvetica-Narrow-Oblique"         => Some(FontKind::SansItalic),
        "Helvetica-Narrow-BoldOblique"     => Some(FontKind::SansBoldItalic),
        "HelveticaNeue"                    => Some(FontKind::SansRegular),
        "HelveticaNeue-Bold"               => Some(FontKind::SansBold),
        "HelveticaNeue-Italic"             => Some(FontKind::SansItalic),
        "HelveticaNeue-BoldItalic"         => Some(FontKind::SansBoldItalic),
        "HelveticaNeue-Light"              => Some(FontKind::SansRegular),
        "HelveticaNeue-LightItalic"        => Some(FontKind::SansItalic),
        "HelveticaNeue-Medium"             => Some(FontKind::SansBold),
        "HelveticaNeue-UltraLight"         => Some(FontKind::SansRegular),
        "HelveticaNeue-UltraLightItalic"   => Some(FontKind::SansItalic),
        "HelveticaNeue-CondensedBold"      => Some(FontKind::SansBold),
        "HelveticaNeue-CondensedBlack"     => Some(FontKind::SansBold),
        "HelveticaNeue-Thin"               => Some(FontKind::SansRegular),
        "HelveticaNeue-ThinItalic"         => Some(FontKind::SansItalic),
        "Verdana"                          => Some(FontKind::SansRegular),
        "Verdana-Bold"                     => Some(FontKind::SansBold),
        "Verdana-Italic"                   => Some(FontKind::SansItalic),
        "Verdana-BoldItalic"               => Some(FontKind::SansBoldItalic),
        "TrebuchetMS"                      => Some(FontKind::SansRegular),
        "TrebuchetMS-Bold"                 => Some(FontKind::SansBold),
        "TrebuchetMS-Italic"               => Some(FontKind::SansItalic),
        "TrebuchetMS-BoldItalic"           => Some(FontKind::SansBoldItalic),
        "Futura-Medium"                    => Some(FontKind::SansRegular),
        "Futura-MediumItalic"              => Some(FontKind::SansItalic),
        "Futura-CondensedMedium"           => Some(FontKind::SansRegular),
        "Futura-CondensedExtraBold"        => Some(FontKind::SansBold),
        "GillSans"                         => Some(FontKind::SansRegular),
        "GillSans-Bold"                    => Some(FontKind::SansBold),
        "GillSans-Italic"                  => Some(FontKind::SansItalic),
        "GillSans-BoldItalic"              => Some(FontKind::SansBoldItalic),
        "GillSans-Light"                   => Some(FontKind::SansRegular),
        "GillSans-LightItalic"             => Some(FontKind::SansItalic),
        "Optima-Regular"                   => Some(FontKind::SansRegular),
        "Optima-Bold"                      => Some(FontKind::SansBold),
        "Optima-Italic"                    => Some(FontKind::SansItalic),
        "Optima-BoldItalic"                => Some(FontKind::SansBoldItalic),
        "Optima-ExtraBlack"                => Some(FontKind::SansBold),
        "TimesNewRomanPSMT"                => Some(FontKind::SerifRegular),
        "TimesNewRomanPS-BoldMT"           => Some(FontKind::SerifBold),
        "TimesNewRomanPS-ItalicMT"         => Some(FontKind::SerifItalic),
        "TimesNewRomanPS-BoldItalicMT"     => Some(FontKind::SerifBoldItalic),
        "Georgia"                          => Some(FontKind::SerifRegular),
        "Georgia-Bold"                     => Some(FontKind::SerifBold),
        "Georgia-Italic"                   => Some(FontKind::SerifItalic),
        "Georgia-BoldItalic"               => Some(FontKind::SerifBoldItalic),
        "Palatino-Roman"                   => Some(FontKind::SerifRegular),
        "Palatino-Bold"                    => Some(FontKind::SerifBold),
        "Palatino-Italic"                  => Some(FontKind::SerifItalic),
        "Palatino-BoldItalic"              => Some(FontKind::SerifBoldItalic),
        "Baskerville"                      => Some(FontKind::SerifRegular),
        "Baskerville-Bold"                 => Some(FontKind::SerifBold),
        "Baskerville-Italic"               => Some(FontKind::SerifItalic),
        "Baskerville-BoldItalic"           => Some(FontKind::SerifBoldItalic),
        "Baskerville-SemiBold"             => Some(FontKind::SerifBold),
        "Baskerville-SemiBoldItalic"       => Some(FontKind::SerifBoldItalic),
        "Didot"                            => Some(FontKind::SerifRegular),
        "Didot-Bold"                       => Some(FontKind::SerifBold),
        "Didot-Italic"                     => Some(FontKind::SerifItalic),
        "Cochin"                           => Some(FontKind::SerifRegular),
        "Cochin-Bold"                      => Some(FontKind::SerifBold),
        "Cochin-Italic"                    => Some(FontKind::SerifItalic),
        "Cochin-BoldItalic"                => Some(FontKind::SerifBoldItalic),
        "AmericanTypewriter"               => Some(FontKind::MonoRegular),
        "AmericanTypewriter-Bold"          => Some(FontKind::MonoBold),
        "AmericanTypewriter-Condensed"     => Some(FontKind::MonoRegular),
        "AmericanTypewriter-CondensedBold" => Some(FontKind::MonoBold),
        "AmericanTypewriter-CondensedLight"=> Some(FontKind::MonoRegular),
        "AmericanTypewriter-Light"         => Some(FontKind::MonoRegular),
        "MarkerFelt-Thin"                  => Some(FontKind::SansRegular),
        "MarkerFelt-Wide"                  => Some(FontKind::SansBold),
        "ChalkboardSE-Regular"             => Some(FontKind::SansRegular),
        "ChalkboardSE-Bold"                => Some(FontKind::SansBold),
        "ChalkboardSE-Light"               => Some(FontKind::SansRegular),
        "Chalkduster"                      => Some(FontKind::SansRegular),
        "BradleyHandITCTT-Bold"            => Some(FontKind::SansBold),
        "EuphemiaUCAS"                     => Some(FontKind::SansRegular),
        "EuphemiaUCAS-Bold"                => Some(FontKind::SansBold),
        "EuphemiaUCAS-Italic"              => Some(FontKind::SansItalic),
        "DBLCDTempBlack"                   => Some(FontKind::MonoBold),
        "Thonburi"                         => Some(FontKind::SansRegular),
        "Thonburi-Bold"                    => Some(FontKind::SansBold),
        "soopafre.ttf"                     => Some(FontKind::SansRegular),

        _ => None,
    }
}

fn font_kind_to_name(kind: FontKind) -> &'static str {
    match kind {
        FontKind::MonoRegular => "CourierNewPSMT",
        FontKind::MonoBold => "CourierNewPS-BoldMT",
        FontKind::MonoBoldItalic => "CourierNewPS-BoldItalicMT",
        FontKind::MonoItalic => "CourierNewPS-ItalicMT",
        FontKind::SansRegular => "Helvetica",
        FontKind::SansBold => "Helvetica-Bold",
        FontKind::SansBoldItalic => "Helvetica-BoldOblique",
        FontKind::SansItalic => "Helvetica-Oblique",
        FontKind::SerifRegular => "TimesNewRomanPSMT",
        FontKind::SerifBold => "TimesNewRomanPS-BoldMT",
        FontKind::SerifBoldItalic => "TimesNewRomanPS-BoldItalicMT",
        FontKind::SerifItalic => "TimesNewRomanPS-ItalicMT",
    }
}

fn font_kind_to_family(kind: FontKind) -> &'static str {
    match kind {
        FontKind::MonoRegular
        | FontKind::MonoBold
        | FontKind::MonoBoldItalic
        | FontKind::MonoItalic => "Courier New",
        FontKind::SansRegular
        | FontKind::SansBold
        | FontKind::SansBoldItalic
        | FontKind::SansItalic => "Helvetica",
        FontKind::SerifRegular
        | FontKind::SerifBold
        | FontKind::SerifBoldItalic
        | FontKind::SerifItalic => "Times New Roman",
    }
}
