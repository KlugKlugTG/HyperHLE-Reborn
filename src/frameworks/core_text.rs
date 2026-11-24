/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CoreText.framework/CoreText`
//!
//! CoreText provides font discovery, descriptors, glyph metrics and drawing,
//! and attributed-string layout. Complex shaping and multiple attribute runs
//! are not yet supported by the simple layout engine below.
//!
//! Per Apple's `CTFontDescriptor.h`, `CTFont.h`, `CTFontTraits.h` and
//! `CTStringAttributes.h` headers these are `CFStringRef` constants
//! with a canonical string value. For attributed-string attribute
//! keys most of them have the same value as their `NSAttributedString`
//! counterpart (e.g. `kCTFontAttributeName` == the `CFStringRef` for
//! the C string "NSFont", which is the same as the AppKit / UIKit
//! `NSFontAttributeName`), which is what makes CoreText / Foundation
//! attributed strings toll-free bridgeable. For touchHLE's purposes
//! the exact textual content only matters for identity comparisons;
//! we mirror the spelling Apple's public headers document.
//!
//! References:
//! - <https://developer.apple.com/documentation/coretext/font_descriptor_attribute_keys>
//! - <https://developer.apple.com/documentation/coretext/core_text_string_attributes>
//! - `CTFontDescriptor.h`, `CTFont.h`, `CTFontTraits.h`,
//!   `CTStringAttributes.h` (Apple SDK).

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant, HostDylib};
use crate::font::{Font, TextAlignment, WrapMode};
use crate::frameworks::core_foundation::cf_attributed_string::CFAttributedStringRef;
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::{
    cf_array::CFArrayRef, cf_type::CFTypeRef, CFIndex, CFRange,
};
use crate::frameworks::core_graphics::cg_affine_transform::{
    CGAffineTransform, CGAffineTransformIdentity,
};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextDrawer, CGBitmapContextGetHeight,
};
use crate::frameworks::core_graphics::cg_context::{
    CGContextConcatCTM, CGContextGetCTM, CGContextRestoreGState, CGContextSaveGState,
    CGContextSetFont, CGContextSetFontSize, CGContextSetTextMatrix, CGContextShowGlyphsAtPoint,
};
use crate::frameworks::core_graphics::cg_font::{
    cg_font_with_font, data_provider_font, CGFontCreateWithFontName, CGFontRef, CGGlyph,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::{unichar, NSRange, NSUInteger};
use crate::frameworks::uikit::ui_font::{
    custom_font_of_uifont, draw_font_glyph, font_for_text, font_from_uifont, is_uifont,
    new_uifont_with_custom_font, new_uifont_with_name,
};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, Ptr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
};
use crate::Environment;

/// Opaque CoreText font reference.
pub type CTFontRef = crate::objc::id;

/// Opaque CoreText font descriptor reference.
pub type CTFontDescriptorRef = crate::objc::id;

/// Opaque CoreText typesetter reference.
pub type CTTypesetterRef = crate::objc::id;

/// Opaque CoreText line reference.
pub type CTLineRef = crate::objc::id;

/// `CTLineTruncationType`, from `CTLine.h`.
pub type CTLineTruncationType = u32;
pub const kCTLineTruncationStart: CTLineTruncationType = 0;
pub const kCTLineTruncationMiddle: CTLineTruncationType = 1;
pub const kCTLineTruncationEnd: CTLineTruncationType = 2;

/// Opaque CoreText run reference.
pub type CTRunRef = crate::objc::id;

/// Opaque CoreText framesetter reference.
pub type CTFramesetterRef = crate::objc::id;

/// Opaque CoreText frame reference.
pub type CTFrameRef = crate::objc::id;

#[derive(Clone, Default)]
struct CTFontDescriptorHostObject {
    attrs: id,
    /// A descriptor copied from a font retains its face even if an
    /// application subsequently unregisters the font's PostScript name.
    face: Option<Font>,
}
impl HostObject for CTFontDescriptorHostObject {}

#[derive(Clone, Default)]
struct CTFontHostObject {
    font: id,
    matrix: Option<CGAffineTransform>,
    attributes: id,
}
impl HostObject for CTFontHostObject {}

#[derive(Clone, Default)]
struct CTTypesetterHostObject {
    attr_string: id,
}
impl HostObject for CTTypesetterHostObject {}

#[derive(Clone, Default)]
struct CTLineHostObject {
    text: id,
    font: id,
    attributes: id,
    string_range: CFRange,
    /// Buffers handed out by `CTRunGetGlyphsPtr` / `CTRunGetPositionsPtr`,
    /// which must stay valid for as long as the run exists.
    glyphs_ptr: MutPtr<CGGlyph>,
    positions_ptr: MutPtr<CGPoint>,
}
impl HostObject for CTLineHostObject {}

#[derive(Clone, Default)]
struct CTFramesetterHostObject {
    attr_string: id,
}
impl HostObject for CTFramesetterHostObject {}

#[derive(Clone, Default)]
struct CTFrameHostObject {
    /// The single CTLine we produce for the (whole) attributed string. Our
    /// layout engine doesn't break a framesetter's string across multiple
    /// lines/columns, so a frame holds exactly one line. This matches what
    /// the apps in the corpus need (they mostly measure / draw short labels).
    line: id,
    /// The string range covered by the frame, mirrored back from the
    /// attributed string for `CTFrameGetStringRange`/`GetVisibleStringRange`.
    range: CFRange,
}
impl HostObject for CTFrameHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CTFontDescriptor: NSObject
- (())dealloc {
    let attrs = env.objc.borrow::<CTFontDescriptorHostObject>(this).attrs;
    release(env, attrs);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTFont: NSObject
- (())dealloc {
    let host = env.objc.borrow::<CTFontHostObject>(this);
    let (font, attributes) = (host.font, host.attributes);
    release(env, font);
    release(env, attributes);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTTypesetter: NSObject
- (())dealloc {
    let string = env.objc.borrow::<CTTypesetterHostObject>(this).attr_string;
    release(env, string);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTLine: NSObject
- (())dealloc {
    let (text, font, attributes, glyphs_ptr, positions_ptr) = {
        let host = env.objc.borrow::<CTLineHostObject>(this);
        (host.text, host.font, host.attributes, host.glyphs_ptr, host.positions_ptr)
    };
    release(env, text);
    release(env, font);
    release(env, attributes);
    env.mem.free(glyphs_ptr.cast());
    env.mem.free(positions_ptr.cast());
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTFramesetter: NSObject
- (())dealloc {
    let string = env.objc.borrow::<CTFramesetterHostObject>(this).attr_string;
    release(env, string);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _touchHLE_CTFrame: NSObject
- (())dealloc {
    let line = env.objc.borrow::<CTFrameHostObject>(this).line;
    release(env, line);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

fn alloc_descriptor(env: &mut Environment, attrs: id) -> CTFontDescriptorRef {
    let class = env
        .objc
        .get_known_class("_touchHLE_CTFontDescriptor", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFontDescriptorHostObject { attrs, face: None }),
        &mut env.mem,
    )
}

fn alloc_font(env: &mut Environment, font: id) -> CTFontRef {
    let class = env.objc.get_known_class("_touchHLE_CTFont", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFontHostObject {
            font,
            matrix: None,
            attributes: nil,
        }),
        &mut env.mem,
    )
}

fn alloc_typesetter(env: &mut Environment, attr_string: id) -> CTTypesetterRef {
    let class = env
        .objc
        .get_known_class("_touchHLE_CTTypesetter", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTTypesetterHostObject { attr_string }),
        &mut env.mem,
    )
}

fn alloc_line(env: &mut Environment, text: id, font: id, attributes: id) -> CTLineRef {
    let length: NSUInteger = msg![env; text length];
    let class = env.objc.get_known_class("_touchHLE_CTLine", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTLineHostObject {
            text,
            font,
            attributes,
            string_range: CFRange {
                location: 0,
                length: length as CFIndex,
            },
            glyphs_ptr: Ptr::null(),
            positions_ptr: Ptr::null(),
        }),
        &mut env.mem,
    )
}

fn alloc_framesetter(env: &mut Environment, attr_string: id) -> CTFramesetterRef {
    let class = env
        .objc
        .get_known_class("_touchHLE_CTFramesetter", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFramesetterHostObject { attr_string }),
        &mut env.mem,
    )
}

fn alloc_frame(env: &mut Environment, line: id, range: CFRange) -> CTFrameRef {
    let class = env.objc.get_known_class("_touchHLE_CTFrame", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFrameHostObject { line, range }),
        &mut env.mem,
    )
}

fn descriptor_attrs(env: &mut Environment, descriptor: CTFontDescriptorRef) -> id {
    env.objc
        .borrow::<CTFontDescriptorHostObject>(descriptor)
        .attrs
}

fn font_from_descriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    size: CGFloat,
) -> id {
    if descriptor == nil {
        return msg_class![env; UIFont systemFontOfSize:(if size > 0.0 { size } else { 12.0 })];
    }
    let attrs = descriptor_attrs(env, descriptor);
    if attrs == nil {
        return msg_class![env; UIFont systemFontOfSize:size];
    }
    let font_name_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontNameAttribute");
    let family_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontFamilyAttribute");
    let size_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontSizeAttribute");
    let mut name: id = nil;
    let by_name: id = msg![env; attrs objectForKey:font_name_key];
    if by_name != nil {
        name = by_name;
    } else {
        let family: id = msg![env; attrs objectForKey:family_key];
        if family != nil {
            let family_str = to_rust_string(env, family).into_owned();
            name = match family_str.as_str() {
                "Courier New" => get_static_str(env, "CourierNewPSMT"),
                "Helvetica" => get_static_str(env, "Helvetica"),
                "Times New Roman" => get_static_str(env, "TimesNewRomanPSMT"),
                _ => family,
            };
        }
    }
    let font_size: CGFloat = if size > 0.0 {
        size
    } else {
        let size_num: id = msg![env; attrs objectForKey:size_key];
        if size_num != nil {
            msg![env; size_num floatValue]
        } else {
            12.0
        }
    };
    if let Some(face) = env
        .objc
        .borrow::<CTFontDescriptorHostObject>(descriptor)
        .face
        .clone()
    {
        return new_uifont_with_custom_font(env, face, font_size);
    }
    if name == nil {
        msg_class![env; UIFont systemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont fontWithName:name size:font_size]
    }
}

fn attributed_string_parts(env: &mut Environment, attr_string: CFAttributedStringRef) -> (id, id) {
    if attr_string == nil {
        return (nil, nil);
    }
    let string: id = msg![env; attr_string string];
    if string == nil {
        return (nil, nil);
    }
    let length: NSUInteger = msg![env; string length];
    let attrs = if length == 0 {
        nil
    } else {
        let range_ptr: MutPtr<NSRange> = MutPtr::null();
        msg![env; attr_string attributesAtIndex:(0usize) effectiveRange:range_ptr]
    };
    (string, attrs)
}

fn string_and_font_from_attr_string(
    env: &mut Environment,
    attr_string: CFAttributedStringRef,
) -> (id, id, id) {
    let (string, attrs) = attributed_string_parts(env, attr_string);
    let font = if attrs == nil {
        msg_class![env; UIFont systemFontOfSize:12.0]
    } else {
        let key = get_static_str(env, "NSFont");
        let maybe_font = msg![env; attrs objectForKey:key];
        let ui_font = if maybe_font == nil || is_uifont(env, maybe_font) {
            maybe_font
        } else {
            env.objc
                .get_host_object(maybe_font)
                .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
                .map(|host| host.font)
                .unwrap_or(nil)
        };
        if ui_font == nil {
            msg_class![env; UIFont systemFontOfSize:12.0]
        } else {
            ui_font
        }
    };
    (string, font, attrs)
}

fn CTFontDescriptorCreateWithAttributes(
    env: &mut Environment,
    attributes: id,
) -> CTFontDescriptorRef {
    let attrs = if attributes == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        attributes
    };
    let attrs: id = msg![env; attrs copy];
    alloc_descriptor(env, attrs)
}

/// `CTFontDescriptorRef CTFontDescriptorCreateWithNameAndSize(
///     CFStringRef name, CGFloat size)`
///
/// Convenience constructor for a descriptor carrying the two most common
/// attributes: the PostScript/family name and the point size.
/// <https://developer.apple.com/documentation/coretext/1511145-ctfontdescriptorcreatewithnamea>
fn CTFontDescriptorCreateWithNameAndSize(
    env: &mut Environment,
    name: CFStringRef,
    size: CGFloat,
) -> CTFontDescriptorRef {
    if name.is_null() {
        return nil;
    }
    let attributes: id = msg_class![env; NSMutableDictionary dictionary];
    let name_key = get_static_str(env, "NSFontNameAttribute");
    let size_key = get_static_str(env, "NSFontSizeAttribute");
    let size_f = size as f64;
    let size_number: id = msg_class![env; NSNumber numberWithDouble:size_f];
    () = msg![env; attributes setObject:name forKey:name_key];
    () = msg![env; attributes setObject:size_number forKey:size_key];
    CTFontDescriptorCreateWithAttributes(env, attributes)
}

/// Registered fonts participate in descriptor matching alongside system
/// fonts. Mandatory keys must match exactly; optional keys choose the face.
/// https://developer.apple.com/documentation/coretext/ctfontdescriptorcreatematchingfontdescriptors(_:_:)
fn CTFontDescriptorCreateMatchingFontDescriptors(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    attributes: CFTypeRef, // CFSetRef of mandatory attribute keys
) -> CFArrayRef {
    if descriptor == nil {
        return nil;
    }
    let requested = descriptor_attrs(env, descriptor);
    let name_key = get_static_str(env, "NSFontNameAttribute");
    let family_key = get_static_str(env, "NSFontFamilyAttribute");
    let requested_name: id = msg![env; requested objectForKey:name_key];
    let requested_family: id = msg![env; requested objectForKey:family_key];
    let mandatory: id = msg![env; attributes allObjects];
    let mandatory_count: NSUInteger = msg![env; mandatory count];
    let families: id = msg_class![env; UIFont familyNames];
    let family_count: NSUInteger = msg![env; families count];
    let results: id = msg_class![env; NSMutableArray new];
    for family_index in 0..family_count {
        let family: id = msg![env; families objectAtIndex:family_index];
        if requested_family != nil && !msg![env; family isEqual:requested_family] {
            continue;
        }
        let names: id = msg_class![env; UIFont fontNamesForFamilyName:family];
        let name_count: NSUInteger = msg![env; names count];
        for name_index in 0..name_count {
            let name: id = msg![env; names objectAtIndex:name_index];
            if requested_name != nil && !msg![env; name isEqual:requested_name] {
                continue;
            }
            let candidate_attrs: id = if requested == nil {
                msg_class![env; NSMutableDictionary new]
            } else {
                msg![env; requested mutableCopy]
            };
            () = msg![env; candidate_attrs setObject:name forKey:name_key];
            () = msg![env; candidate_attrs setObject:family forKey:family_key];
            let candidate = CTFontDescriptorCreateWithAttributes(env, candidate_attrs);
            release(env, candidate_attrs);
            let font = CTFontCreateWithFontDescriptor(env, candidate, 0.0, Ptr::null());
            release(env, candidate);
            let normalized = CTFontCopyFontDescriptor(env, font);
            release(env, font);
            let mut matches = true;
            for index in 0..mandatory_count {
                let key: id = msg![env; mandatory objectAtIndex:index];
                let expected: id = msg![env; requested objectForKey:key];
                let actual = CTFontDescriptorCopyAttribute(env, normalized, key);
                let same =
                    expected == actual || (expected != nil && msg![env; expected isEqual:actual]);
                release(env, actual);
                if !same {
                    matches = false;
                    break;
                }
            }
            if matches {
                () = msg![env; results addObject:normalized];
            }
            release(env, normalized);
        }
    }
    let count: NSUInteger = msg![env; results count];
    if count == 0 && mandatory_count == 0 {
        let font = CTFontCreateWithFontDescriptor(env, descriptor, 0.0, Ptr::null());
        let normalized = CTFontCopyFontDescriptor(env, font);
        release(env, font);
        if normalized != nil {
            () = msg![env; results addObject:normalized];
            release(env, normalized);
        }
    }
    let immutable: id = msg![env; results copy];
    release(env, results);
    immutable
}

/// `CTFontDescriptorCreateMatchingFontDescriptor` — the single best match
/// for a descriptor, using the same matching and registration rules.
fn CTFontDescriptorCreateMatchingFontDescriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    mandatory_attributes: CFTypeRef, // CFSetRef of attribute keys
) -> CTFontDescriptorRef {
    if descriptor == nil {
        return nil;
    }
    let matches =
        CTFontDescriptorCreateMatchingFontDescriptors(env, descriptor, mandatory_attributes);
    let count: NSUInteger = msg![env; matches count];
    let matched: id = if count == 0 {
        nil
    } else {
        msg![env; matches objectAtIndex:(0 as NSUInteger)]
    };
    retain(env, matched);
    release(env, matches);
    matched
}

/// The file in `/System/Library/Fonts/Cache` (see `fs.rs`) that stands in for
/// a font with this name or family and style. touchHLE ships the metric
/// compatible Liberation fonts, plus Arial and a bold Helvetica.
fn system_font_file(name: &str, style: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    let style = style.to_ascii_lowercase();
    let bold = name.contains("bold") || style.contains("bold");
    let italic = ["italic", "oblique"]
        .iter()
        .any(|word| name.contains(word) || style.contains(word));
    let family = if name.contains("courier") || name.contains("mono") {
        "LiberationMono"
    } else if ["times", "georgia", "serif"]
        .iter()
        .any(|word| name.contains(word))
        && !name.contains("sans")
    {
        "LiberationSerif"
    } else if name.contains("helvetica") && bold && !italic {
        return "helvetica-bold.ttf";
    } else if name.contains("arial") && !bold && !italic {
        return "Arial.ttf";
    } else {
        "LiberationSans"
    };
    match (family, bold, italic) {
        ("LiberationMono", false, false) => "LiberationMono-Regular.ttf",
        ("LiberationMono", true, false) => "LiberationMono-Bold.ttf",
        ("LiberationMono", false, true) => "LiberationMono-Italic.ttf",
        ("LiberationMono", true, true) => "LiberationMono-BoldItalic.ttf",
        ("LiberationSerif", false, false) => "LiberationSerif-Regular.ttf",
        ("LiberationSerif", true, false) => "LiberationSerif-Bold.ttf",
        ("LiberationSerif", false, true) => "LiberationSerif-Italic.ttf",
        ("LiberationSerif", true, true) => "LiberationSerif-BoldItalic.ttf",
        (_, false, false) => "LiberationSans-Regular.ttf",
        (_, true, false) => "LiberationSans-Bold.ttf",
        (_, false, true) => "LiberationSans-Italic.ttf",
        (_, true, true) => "LiberationSans-BoldItalic.ttf",
    }
}

/// `CFTypeRef CTFontDescriptorCopyAttribute(CTFontDescriptorRef descriptor,
///     CFStringRef attribute)`. Attributes the descriptor was created with are
/// returned as is. The font file URL is filled in from the bundled fonts:
/// engines such as Unity load the font file themselves through it, and draw
/// no text at all without one.
fn CTFontDescriptorCopyAttribute(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    attribute: id,
) -> CFTypeRef {
    if descriptor == nil || attribute == nil {
        return nil;
    }
    let attrs = descriptor_attrs(env, descriptor);
    if attrs != nil {
        let value: id = msg![env; attrs objectForKey:attribute];
        if value != nil {
            return retain(env, value);
        }
    }
    let url_key = get_static_str(env, "NSCTFontFileURLAttribute");
    let is_url_key: bool = msg![env; attribute isEqualToString:url_key];
    if !is_url_key {
        return nil;
    }
    let mut attr_string = |key: &'static str| {
        if attrs == nil {
            return None;
        }
        let key = get_static_str(env, key);
        let value: id = msg![env; attrs objectForKey:key];
        (value != nil).then(|| to_rust_string(env, value).into_owned())
    };
    let name = attr_string("NSFontNameAttribute")
        .or_else(|| attr_string("NSFontFamilyAttribute"))
        .unwrap_or_default();
    let style = attr_string("NSFontFaceAttribute").unwrap_or_default();
    let path = format!(
        "/System/Library/Fonts/Cache/{}",
        system_font_file(&name, &style)
    );
    let path = from_rust_string(env, path);
    let url: id = msg_class![env; NSURL fileURLWithPath:path];
    release(env, path);
    retain(env, url)
}

fn CTFontCreateWithFontDescriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    size: CGFloat,
    matrix: ConstVoidPtr,
) -> CTFontRef {
    let font = font_from_descriptor(env, descriptor, size);
    retain(env, font);
    let font = alloc_font(env, font);
    let attrs = if descriptor == nil {
        nil
    } else {
        descriptor_attrs(env, descriptor)
    };
    set_font_matrix(env, font, matrix, attrs);
    font
}

fn CTFontCreateWithName(
    env: &mut Environment,
    name: id,
    size: CGFloat,
    matrix: ConstVoidPtr,
) -> CTFontRef {
    if name.is_null() {
        return nil;
    }
    let size = if size == 0.0 { 12.0 } else { size };
    let name = to_rust_string(env, name).into_owned();
    let font = new_uifont_with_name(env, &name, size);
    retain(env, font);
    let font = alloc_font(env, font);
    set_font_matrix(env, font, matrix, nil);
    font
}

/// Resolve a `CTFontRef` to the `UIFont` backing it. Also accepts a bare
/// `UIFont`, which some apps pass where a `CTFontRef` is expected.
fn ui_font_for_ct_font(env: &mut Environment, font: CTFontRef) -> id {
    if font == nil {
        return nil;
    }
    if is_uifont(env, font) {
        return font;
    }
    env.objc
        .get_host_object(font)
        .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
        .map(|host| host.font)
        .unwrap_or(nil)
}

/// `CTFontRef CTFontCreateCopyWithAttributes(CTFontRef font, CGFloat size,
///     const CGAffineTransform *matrix, CTFontDescriptorRef attributes)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509393-ctfontcreatecopywithattributes>
///
/// > Returns a new font with additional attributes based on the original
/// > font. [...] `size`: The point size for the font reference. If 0.0 is
/// > specified, the original font's size is preserved.
///
/// The original font's name is kept unless `attributes` carries a font name
/// or family of its own, in which case those win (descriptor attributes
/// override the original font's, as on iOS). An unspecified matrix preserves
/// the original font's transform.
fn CTFontCreateCopyWithAttributes(
    env: &mut Environment,
    font: CTFontRef,
    size: CGFloat,
    matrix: ConstPtr<CGAffineTransform>,
    attributes: CTFontDescriptorRef,
) -> CTFontRef {
    let original = ui_font_for_ct_font(env, font);
    if original == nil {
        return nil;
    }
    let size: CGFloat = if size > 0.0 {
        size
    } else {
        msg![env; original pointSize]
    };

    let descriptor = CTFontCopyFontDescriptor(env, font);
    let overrides = if attributes == nil {
        nil
    } else {
        descriptor_attrs(env, attributes)
    };
    let merged = CTFontDescriptorCreateCopyWithAttributes(env, descriptor, overrides);
    release(env, descriptor);
    let saved_matrix = if matrix.is_null() {
        let value = CTFontGetMatrix(env, font);
        env.mem.alloc_and_write(value)
    } else {
        Ptr::null()
    };
    let matrix = if matrix.is_null() {
        saved_matrix.cast_const()
    } else {
        matrix
    };
    let copy = CTFontCreateWithFontDescriptor(env, merged, size, matrix.cast());
    env.mem.free(saved_matrix.cast());
    release(env, merged);
    copy
}

fn CTFontGetSize(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() {
        return 0.0;
    }
    let ui_font = ui_font_for_ct_font(env, font);
    if ui_font == nil {
        return 0.0;
    }
    msg![env; ui_font pointSize]
}

fn CTFontCopyPostScriptName(env: &mut Environment, font: CTFontRef) -> id {
    if font.is_null() {
        return nil;
    }
    let ui_font = ui_font_for_ct_font(env, font);
    if ui_font == nil {
        return nil;
    }
    let name: id = msg![env; ui_font fontName];
    if name == nil {
        nil
    } else {
        retain(env, name)
    }
}

/// Font-name accessors are Copy operations and return owned strings.
/// https://developer.apple.com/documentation/coretext/ctfontcopyfamilyname(_:)
fn CTFontCopyFamilyName(env: &mut Environment, font: CTFontRef) -> id {
    let font = ui_font_for_ct_font(env, font);
    if font == nil {
        return nil;
    }
    let name: id = msg![env; font familyName];
    if name == nil {
        nil
    } else {
        retain(env, name)
    }
}

fn CTFontCopyFullName(env: &mut Environment, font: CTFontRef) -> id {
    let font = ui_font_for_ct_font(env, font);
    if font == nil {
        return nil;
    }
    if let Some(name) = custom_font_of_uifont(env, font).and_then(|f| f.name(4)) {
        return from_rust_string(env, name);
    }
    let name: id = msg![env; font fontName];
    if name == nil {
        nil
    } else {
        retain(env, name)
    }
}

fn CTFontGetSymbolicTraits(env: &mut Environment, font: CTFontRef) -> u32 {
    let ui = ui_font_for_ct_font(env, font);
    font_from_uifont(env, ui).map_or(0, |face| face.symbolic_traits())
}

fn CTFontGetMatrix(env: &mut Environment, font: CTFontRef) -> CGAffineTransform {
    if font == nil || is_uifont(env, font) {
        return CGAffineTransformIdentity;
    }
    env.objc
        .borrow::<CTFontHostObject>(font)
        .matrix
        .unwrap_or(CGAffineTransformIdentity)
}

fn matrix_from_attributes(env: &mut Environment, attrs: id) -> Option<CGAffineTransform> {
    if attrs == nil {
        return None;
    }
    let key = get_static_str(env, "NSCTFontMatrixAttribute");
    let data: id = msg![env; attrs objectForKey:key];
    if data == nil {
        return None;
    }
    let bytes = crate::frameworks::foundation::ns_data::to_rust_slice(env, data);
    if bytes.len() < 24 {
        return None;
    }
    let values: Vec<f32> = bytes[..24]
        .chunks_exact(4)
        .map(|b| f32::from_ne_bytes(b.try_into().unwrap()))
        .collect();
    Some(CGAffineTransform {
        a: values[0],
        b: values[1],
        c: values[2],
        d: values[3],
        tx: 0.0,
        ty: 0.0,
    })
}

fn set_font_matrix(env: &mut Environment, font: id, matrix: ConstVoidPtr, attrs: id) {
    let value = if matrix.is_null() {
        matrix_from_attributes(env, attrs)
    } else {
        Some(env.mem.read(matrix.cast()))
    };
    retain(env, attrs);
    let old = env.objc.borrow::<CTFontHostObject>(font).attributes;
    release(env, old);
    let host = env.objc.borrow_mut::<CTFontHostObject>(font);
    host.matrix = value;
    host.attributes = attrs;
}

/// The normalized descriptor keeps the face, name, size, and matrix needed
/// to recreate a font. Copying/augmenting it must not substitute a system face.
/// https://developer.apple.com/documentation/coretext/ctfontcopyfontdescriptor(_:)
fn CTFontCopyFontDescriptor(env: &mut Environment, font: CTFontRef) -> CTFontDescriptorRef {
    let ui = ui_font_for_ct_font(env, font);
    if ui == nil {
        return nil;
    }
    let original_attrs = if is_uifont(env, font) {
        nil
    } else {
        env.objc.borrow::<CTFontHostObject>(font).attributes
    };
    let attrs: id = if original_attrs == nil {
        msg_class![env; NSMutableDictionary new]
    } else {
        msg![env; original_attrs mutableCopy]
    };
    let name_key = get_static_str(env, "NSFontNameAttribute");
    let family_key = get_static_str(env, "NSFontFamilyAttribute");
    let size_key = get_static_str(env, "NSFontSizeAttribute");
    let name: id = msg![env; ui fontName];
    let family: id = msg![env; ui familyName];
    let size: CGFloat = msg![env; ui pointSize];
    let number: id = msg_class![env; NSNumber numberWithFloat:size];
    () = msg![env; attrs setObject:name forKey:name_key];
    () = msg![env; attrs setObject:family forKey:family_key];
    () = msg![env; attrs setObject:number forKey:size_key];
    let traits_key = get_static_str(env, "NSCTFontTraitsAttribute");
    let symbolic_key = get_static_str(env, "NSCTFontSymbolicTrait");
    let symbolic = CTFontGetSymbolicTraits(env, font);
    let symbolic: id = msg_class![env; NSNumber numberWithUnsignedInt:symbolic];
    let old_traits: id = msg![env; attrs objectForKey:traits_key];
    let traits: id = if old_traits == nil {
        msg_class![env; NSMutableDictionary new]
    } else {
        msg![env; old_traits mutableCopy]
    };
    () = msg![env; traits setObject:symbolic forKey:symbolic_key];
    let immutable_traits: id = msg![env; traits copy];
    () = msg![env; attrs setObject:immutable_traits forKey:traits_key];
    release(env, immutable_traits);
    release(env, traits);
    let matrix = CTFontGetMatrix(env, font);
    let bytes = env.mem.alloc_and_write(matrix);
    let data = crate::frameworks::core_foundation::cf_data::CFDataCreate(
        env,
        crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault,
        bytes.cast_const().cast(),
        24,
    );
    env.mem.free(bytes);
    let matrix_key = get_static_str(env, "NSCTFontMatrixAttribute");
    () = msg![env; attrs setObject:data forKey:matrix_key];
    release(env, data);
    let immutable: id = msg![env; attrs copy];
    release(env, attrs);
    let face = custom_font_of_uifont(env, ui);
    let descriptor = alloc_descriptor(env, immutable);
    env.objc
        .borrow_mut::<CTFontDescriptorHostObject>(descriptor)
        .face = face;
    descriptor
}

/// https://developer.apple.com/documentation/coretext/ctfontdescriptorcreatecopywithattributes(_:_:)
fn CTFontDescriptorCreateCopyWithAttributes(
    env: &mut Environment,
    original: CTFontDescriptorRef,
    overrides: id,
) -> CTFontDescriptorRef {
    if original == nil {
        return nil;
    }
    let attrs = descriptor_attrs(env, original);
    let merged: id = if attrs == nil {
        msg_class![env; NSMutableDictionary new]
    } else {
        msg![env; attrs mutableCopy]
    };
    if overrides != nil {
        () = msg![env; merged addEntriesFromDictionary:overrides];
    }
    // CTFontDescriptor.h specifies merging these two attributes instead of
    // replacing them. Variation values are keyed by axis, feature settings
    // are an array of dictionaries (later settings override earlier ones).
    if overrides != nil {
        for (key, dictionary) in [
            ("NSCTFontVariationAttribute", true),
            ("NSCTFontFeatureSettingsAttribute", false),
        ] {
            let key = get_static_str(env, key);
            let old: id = msg![env; attrs objectForKey:key];
            let new: id = msg![env; overrides objectForKey:key];
            if old == nil || new == nil {
                continue;
            }
            let combined: id = msg![env; old mutableCopy];
            if dictionary {
                () = msg![env; combined addEntriesFromDictionary:new];
            } else {
                () = msg![env; combined addObjectsFromArray:new];
            }
            let immutable: id = msg![env; combined copy];
            () = msg![env; merged setObject:immutable forKey:key];
            release(env, immutable);
            release(env, combined);
        }
    }
    let mut face = env
        .objc
        .borrow::<CTFontDescriptorHostObject>(original)
        .face
        .clone();
    if overrides != nil {
        for key in ["NSFontNameAttribute", "NSFontFamilyAttribute"] {
            let key = get_static_str(env, key);
            let new: id = msg![env; overrides objectForKey:key];
            let old: id = msg![env; attrs objectForKey:key];
            let equal: bool = msg![env; new isEqual:old];
            if new != nil && !equal {
                face = None;
                let family_key = get_static_str(env, "NSFontFamilyAttribute");
                if key == family_key {
                    let name_key = get_static_str(env, "NSFontNameAttribute");
                    let new_name: id = msg![env; overrides objectForKey:name_key];
                    if new_name == nil {
                        () = msg![env; merged removeObjectForKey:name_key];
                    }
                }
            }
        }
    }
    let attrs: id = msg![env; merged copy];
    release(env, merged);
    let descriptor = alloc_descriptor(env, attrs);
    env.objc
        .borrow_mut::<CTFontDescriptorHostObject>(descriptor)
        .face = face;
    descriptor
}

/// Draw the supplied glyph IDs at user-space positions using their actual
/// font. As documented by Apple, the font/size/matrix updates are not restored.
/// https://developer.apple.com/documentation/coretext/ctfontdrawglyphs(_:_:_:_:_:)
fn CTFontDrawGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    glyphs: ConstPtr<CGGlyph>,
    positions: ConstPtr<CGPoint>,
    count: GuestUSize,
    context: id,
) {
    if font == nil || context == nil || glyphs.is_null() || positions.is_null() || count == 0 {
        return;
    }
    let ui = ui_font_for_ct_font(env, font);
    let Some(face) = font_from_uifont(env, ui) else {
        return;
    };
    let size: CGFloat = msg![env; ui pointSize];
    let graphics = cg_font_with_font(env, face);
    CGContextSetFont(env, context, graphics);
    release(env, graphics);
    CGContextSetFontSize(env, context, size);
    if !is_uifont(env, font) {
        if let Some(matrix) = env.objc.borrow::<CTFontHostObject>(font).matrix {
            CGContextSetTextMatrix(env, context, matrix);
        }
    }
    for i in 0..count {
        let point = env.mem.read(positions + i);
        CGContextShowGlyphsAtPoint(env, context, point.x, point.y, glyphs + i, 1);
    }
}

fn CTFontGetAscent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    let ui = ui_font_for_ct_font(env, font);
    let Some(face) = font_from_uifont(env, ui) else {
        return 0.0;
    };
    face.design_v_metrics().0 * CTFontGetSize(env, font) / f32::from(face.units_per_em())
}

fn CTFontGetDescent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    let ui = ui_font_for_ct_font(env, font);
    let Some(face) = font_from_uifont(env, ui) else {
        return 0.0;
    };
    -face.design_v_metrics().1 * CTFontGetSize(env, font) / f32::from(face.units_per_em())
}

fn CTFontGetLeading(env: &mut Environment, font: CTFontRef) -> CGFloat {
    let ui = ui_font_for_ct_font(env, font);
    let Some(face) = font_from_uifont(env, ui) else {
        return 0.0;
    };
    face.design_v_metrics().2 * CTFontGetSize(env, font) / f32::from(face.units_per_em())
}

/// `uint32_t CTGetCoreTextVersion(void)`
///
/// Encodes the CoreText feature level using the `kCTVersionNumber*`
/// constants from `CTBase.h`. iOS 4.3's CoreText corresponds to the
/// 10.6-level API (0x00030000), which is what apps compare against when
/// deciding whether e.g. framesetter APIs are present.
/// <https://developer.apple.com/documentation/coretext/1499498-ctgetcoretextversion>
fn CTGetCoreTextVersion(_env: &mut Environment) -> u32 {
    0x0003_0000 // kCTVersionNumber10_6
}

fn CTTypesetterCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTTypesetterRef {
    if string.is_null() {
        return nil;
    }
    retain(env, string);
    alloc_typesetter(env, string)
}

fn CTTypesetterCreateLine(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    range: CFRange,
) -> CTLineRef {
    if typesetter == nil || range.location < 0 || range.length < 0 {
        return nil;
    }
    let attr_string = env
        .objc
        .borrow::<CTTypesetterHostObject>(typesetter)
        .attr_string;
    let (text, _, _) = string_and_font_from_attr_string(env, attr_string);
    if text == nil {
        return nil;
    }
    let total: NSUInteger = msg![env; text length];
    let start = range.location as NSUInteger;
    if start > total {
        return nil;
    }
    let length = if range.length == 0 {
        total - start
    } else {
        range.length as NSUInteger
    };
    if length > total - start {
        return nil;
    }
    let (_, attributes) = if start < total {
        let range_ptr: MutPtr<NSRange> = Ptr::null();
        (
            text,
            msg![env; attr_string attributesAtIndex:start effectiveRange:range_ptr],
        )
    } else {
        (text, nil)
    };
    let font_key = get_static_str(env, "NSFont");
    let specified: id = msg![env; attributes objectForKey:font_key];
    let font = ui_font_for_ct_font(env, specified);
    let font = if font == nil {
        msg_class![env; UIFont systemFontOfSize:12.0]
    } else {
        font
    };
    let substring: id = msg![env; text substringWithRange:(NSRange { location: start, length })];
    retain(env, substring);
    retain(env, font);
    retain(env, attributes);
    let line = alloc_line(env, substring, font, attributes);
    env.objc.borrow_mut::<CTLineHostObject>(line).string_range = CFRange {
        location: range.location,
        length: length as CFIndex,
    };
    line
}

fn CTLineCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTLineRef {
    if string.is_null() {
        return nil;
    }
    let (text, font, attributes) = string_and_font_from_attr_string(env, string);
    retain(env, text);
    retain(env, font);
    retain(env, attributes);
    alloc_line(env, text, font, attributes)
}

/// `CFIndex CTTypesetterSuggestClusterBreak(
///     CTTypesetterRef typesetter, CFIndex startIndex, double width)`
///
/// Returns how many characters, counted from `startIndex`, fit into `width`
/// without splitting a cluster. Like real Core Text, we always let at least
/// one cluster through so callers that loop on the result make progress.
/// Metrics come from the same font measurement `CTLineGetTypographicBounds`
/// uses.
/// <https://developer.apple.com/documentation/coretext/1509556-cttypesettersuggestclusterbreak>
fn text_break(
    font: &Font,
    size: f32,
    text: &str,
    start: CFIndex,
    width: f64,
    words: bool,
) -> CFIndex {
    use unicode_segmentation::UnicodeSegmentation;
    if start < 0 || width.is_nan() || width <= 0.0 {
        return 0;
    }
    let mut units = 0;
    let mut byte_start = text.len();
    for (byte, ch) in text.char_indices() {
        if units == start {
            byte_start = byte;
            break;
        }
        units += ch.len_utf16() as CFIndex;
        if units > start {
            return 0;
        } // do not split a UTF-16 surrogate pair
    }
    let text = &text[byte_start..];
    let opportunities: std::collections::HashMap<usize, unicode_linebreak::BreakOpportunity> =
        unicode_linebreak::linebreaks(text).collect();
    let mut fitted = 0;
    let mut allowed = 0;
    for (offset, cluster) in text.grapheme_indices(true) {
        let end = offset + cluster.len();
        if cluster.contains(['\n', '\r', '\u{0085}', '\u{2028}', '\u{2029}']) {
            return text[..end].encode_utf16().count() as CFIndex;
        }
        let glyphs: Vec<GlyphId> = text[..end]
            .chars()
            .map(|ch| font.glyph_id_for_unicode(ch))
            .collect();
        let positions = font.glyph_positions_at_size(size, &glyphs);
        let measured = positions
            .last()
            .zip(glyphs.last())
            .map_or(0.0, |(x, g)| x + font.glyph_advance_at_size(*g, size));
        if f64::from(measured) > width && fitted != 0 {
            let end = if words && allowed != 0 {
                allowed
            } else {
                fitted
            };
            return text[..end].encode_utf16().count() as CFIndex;
        }
        fitted = end;
        if opportunities.contains_key(&end) {
            allowed = end;
        }
    }
    text[..fitted].encode_utf16().count() as CFIndex
}

fn suggest_break(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    start: CFIndex,
    width: f64,
    offset: f64,
    words: bool,
) -> CFIndex {
    if typesetter == nil {
        return 0;
    }
    let attr_string = env
        .objc
        .borrow::<CTTypesetterHostObject>(typesetter)
        .attr_string;
    let (text, ui, _) = string_and_font_from_attr_string(env, attr_string);
    if text == nil || ui == nil {
        return 0;
    }
    let text = to_rust_string(env, text).into_owned();
    let size: CGFloat = msg![env; ui pointSize];
    let font = font_for_text(env, ui, &text).unwrap_or_else(Font::sans_regular);
    text_break(&font, size, &text, start, width - offset, words)
}

/// https://developer.apple.com/documentation/coretext/cttypesettersuggestlinebreak(_:_:_:)
fn CTTypesetterSuggestLineBreak(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    start: CFIndex,
    width: f64,
) -> CFIndex {
    suggest_break(env, typesetter, start, width, 0.0, true)
}
fn CTTypesetterSuggestLineBreakWithOffset(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    start: CFIndex,
    width: f64,
    offset: f64,
) -> CFIndex {
    suggest_break(env, typesetter, start, width, offset, true)
}
fn CTTypesetterSuggestClusterBreak(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    start: CFIndex,
    width: f64,
) -> CFIndex {
    suggest_break(env, typesetter, start, width, 0.0, false)
}
fn CTTypesetterSuggestClusterBreakWithOffset(
    env: &mut Environment,
    typesetter: CTTypesetterRef,
    start: CFIndex,
    width: f64,
    offset: f64,
) -> CFIndex {
    suggest_break(env, typesetter, start, width, offset, false)
}

/// Longest prefix of `text` whose measured width does not exceed `budget`.
fn fitting_prefix<'a>(measure: &impl Fn(&str) -> f32, text: &'a str, budget: f32) -> &'a str {
    let mut end = 0;
    for (offset, ch) in text.char_indices() {
        let candidate_end = offset + ch.len_utf8();
        if measure(&text[..candidate_end]) > budget {
            break;
        }
        end = candidate_end;
    }
    &text[..end]
}

/// Longest suffix of `text` whose measured width does not exceed `budget`.
fn fitting_suffix<'a>(measure: &impl Fn(&str) -> f32, text: &'a str, budget: f32) -> &'a str {
    let mut start = text.len();
    for (offset, _ch) in text.char_indices().rev() {
        if measure(&text[offset..]) > budget {
            break;
        }
        start = offset;
    }
    &text[start..]
}

/// `CTLineRef CTLineCreateTruncatedLine(
///     CTLineRef line, double width, CTLineTruncationType truncationType,
///     CTLineRef truncationToken)`
///
/// The truncated line keeps the original line's font and attributes and
/// carries the token text at the start, middle or end as requested. Per Apple,
/// a `NULL` token means no token is displayed (the line is simply clipped).
/// <https://developer.apple.com/documentation/coretext/1509487-ctlinecreatetruncatedline>
fn CTLineCreateTruncatedLine(
    env: &mut Environment,
    line: CTLineRef,
    width: f64,
    truncation_type: CTLineTruncationType,
    truncation_token: CTLineRef,
) -> CTLineRef {
    if line.is_null() || !(width > 0.0) {
        return nil;
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    if text == nil || font == nil {
        return nil;
    }
    let attributes = env.objc.borrow::<CTLineHostObject>(line).attributes;
    let text_str = to_rust_string(env, text).into_owned();
    let font_size: CGFloat = msg![env; font pointSize];
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let measure = |s: &str| font_obj.text_advance_at_size(font_size, s);
    let new_text = if measure(&text_str) as f64 <= width {
        text_str
    } else {
        let token_text = if truncation_token.is_null() {
            String::new()
        } else {
            let token = line_text(env, truncation_token);
            if token == nil {
                String::new()
            } else {
                to_rust_string(env, token).into_owned()
            }
        };
        let budget = (width as f32 - measure(&token_text)).max(0.0);
        match truncation_type {
            kCTLineTruncationStart => {
                let tail = fitting_suffix(&measure, &text_str, budget);
                format!("{token_text}{tail}")
            }
            kCTLineTruncationMiddle => {
                let head = fitting_prefix(&measure, &text_str, budget / 2.0);
                let rest = &text_str[head.len()..];
                let tail = fitting_suffix(&measure, rest, budget / 2.0);
                format!("{head}{token_text}{tail}")
            }
            _ => {
                let head = fitting_prefix(&measure, &text_str, budget);
                format!("{head}{token_text}")
            }
        }
    };
    let new_text = from_rust_string(env, new_text);
    retain(env, font);
    retain(env, attributes);
    alloc_line(env, new_text, font, attributes)
}

fn line_text(env: &mut Environment, line: CTLineRef) -> id {
    env.objc.borrow::<CTLineHostObject>(line).text
}

fn line_font(env: &mut Environment, line: CTLineRef) -> id {
    env.objc.borrow::<CTLineHostObject>(line).font
}

fn CTLineGetTypographicBounds(
    env: &mut Environment,
    line: CTLineRef,
    ascent: MutPtr<CGFloat>,
    descent: MutPtr<CGFloat>,
    leading: MutPtr<CGFloat>,
) -> f64 {
    if line.is_null() {
        return 0.0;
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let (ascent_val, descender_val, leading_val) = font_obj.design_v_metrics();
    let factor = size / f32::from(font_obj.units_per_em());
    let width = font_obj.text_advance_at_size(size, &text_str);
    if !ascent.is_null() {
        env.mem.write(ascent, ascent_val * factor);
    }
    if !descent.is_null() {
        env.mem.write(descent, -descender_val * factor);
    }
    if !leading.is_null() {
        env.mem.write(leading, leading_val * factor);
    }
    f64::from(width)
}

fn CTLineGetImageBounds(env: &mut Environment, line: CTLineRef, _context: id) -> CGRect {
    if line.is_null() {
        return CGRect::default();
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let ascent_val: CGFloat = msg![env; font ascender];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let (w, h) = font_obj.calculate_text_size(size, &text_str, None);
    CGRect {
        origin: CGPoint {
            x: 0.0,
            y: -ascent_val,
        },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

fn CTLineDraw(env: &mut Environment, line: CTLineRef, context: id) {
    if line.is_null() || context.is_null() {
        return;
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let needs_y_flip = CGContextGetCTM(env, context).d > 0.0;
    if needs_y_flip {
        let height = CGBitmapContextGetHeight(env, context) as CGFloat;
        CGContextSaveGState(env, context);
        CGContextConcatCTM(
            env,
            context,
            CGAffineTransform {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: -1.0,
                tx: 0.0,
                ty: height,
            },
        );
    }
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();
    font_obj.draw(
        size,
        &text_str,
        (0.0, 0.0),
        None,
        TextAlignment::Left,
        |glyph| draw_font_glyph(&mut drawer, glyph, fill_color, None, None),
    );
    drop(drawer);
    if needs_y_flip {
        CGContextRestoreGState(env, context);
    }
}

/// `CFIndex CTLineGetGlyphCount(CTLineRef line)`
///
/// Returns the total glyph count for the line. We map the line's text to
/// glyphs through the backing font's cmap (one glyph per UTF-16 code unit,
/// Supplementary Unicode scalars occupy two UTF-16 string indices but map
/// to one glyph. Complex shaping is not yet handled by this layout engine.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509609-ctlinegetglyphcount>
fn CTLineGetGlyphCount(env: &mut Environment, line: CTLineRef) -> i32 {
    if line.is_null() {
        return 0;
    }
    let text = line_text(env, line);
    if text == nil {
        return 0;
    }
    to_rust_string(env, text).chars().count() as CFIndex
}

/// `CFArrayRef CTLineGetGlyphRuns(CTLineRef line)`
///
/// Returns the array of glyph runs that make up the line. touchHLE lays text
/// out as a single run, so we return a one-element array wrapping the line
/// itself (our `_touchHLE_CTLine` host object also stands in for a run — the
/// run-level getters used by the apps in the corpus, e.g. glyph count, work
/// the same way). Returning a real (empty-safe) `CFArray` rather than NULL
/// prevents callers from dereferencing a NULL array.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509596-ctlinegetglyphruns>
fn CTLineGetGlyphRuns(env: &mut Environment, line: CTLineRef) -> id {
    if line.is_null() {
        return nil;
    }
    let arr: id = msg_class![env; NSMutableArray arrayWithCapacity:1u32];
    () = msg![env; arr addObject:line];
    let immutable: id = msg![env; arr copy];
    autorelease(env, immutable)
}

/// `CFIndex CTRunGetGlyphCount(CTRunRef run)`
fn CTRunGetGlyphCount(env: &mut Environment, run: CTRunRef) -> CFIndex {
    CTLineGetGlyphCount(env, run)
}

/// `CFDictionaryRef CTRunGetAttributes(CTRunRef run)`
fn CTRunGetAttributes(env: &mut Environment, run: CTRunRef) -> id {
    if run.is_null() {
        return nil;
    }
    let attributes = env.objc.borrow::<CTLineHostObject>(run).attributes;
    if attributes == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        attributes
    }
}

fn glyph_range_for_run(range: CFRange, glyph_count: CFIndex) -> Option<(u32, u32)> {
    let total = u32::try_from(glyph_count).ok()?;
    let start = u32::try_from(range.location).ok()?;
    if start > total {
        return None;
    }
    let length = if range.length == 0 {
        total - start
    } else {
        u32::try_from(range.length).ok()?
    };
    Some((start, start.saturating_add(length).min(total)))
}

fn glyph_for_code_unit(font: &Font, code_unit: unichar) -> CGGlyph {
    char::from_u32(code_unit as u32).map_or(0, |character| font.glyph_id_for_unicode(character).0)
}

/// `void CTRunGetGlyphs(CTRunRef run, CFRange range, CGGlyph buffer[])`
fn CTRunGetGlyphs(env: &mut Environment, run: CTRunRef, range: CFRange, buffer: MutPtr<CGGlyph>) {
    if run.is_null() || buffer.is_null() {
        return;
    }
    let (text, ui_font) = {
        let host = env.objc.borrow::<CTLineHostObject>(run);
        (host.text, host.font)
    };
    if text == nil {
        return;
    }
    let count = CTLineGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, count) else {
        return;
    };
    let text_str = to_rust_string(env, text).into_owned();
    let font = font_for_text(env, ui_font, &text_str).unwrap_or_else(Font::sans_regular);
    for (output_index, character) in text_str
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .enumerate()
    {
        env.mem.write(
            buffer + output_index as u32,
            font.glyph_id_for_unicode(character).0,
        );
    }
}

/// `const CGGlyph *CTRunGetGlyphsPtr(CTRunRef run)`
///
/// CoreText may return NULL here, telling the caller to copy the glyphs out
/// with `CTRunGetGlyphs` instead, but apps often skip that fallback: Real
/// Racing 3 only imports this function and read every glyph as 0 (.notdef),
/// so its text came out as a row of boxes. Hand out a buffer that lives as
/// long as the run, like CoreText's own storage.
fn CTRunGetGlyphsPtr(env: &mut Environment, run: CTRunRef) -> ConstPtr<CGGlyph> {
    if run.is_null() {
        return Ptr::null();
    }
    let existing = env.objc.borrow::<CTLineHostObject>(run).glyphs_ptr;
    if !existing.is_null() {
        return existing.cast_const();
    }
    let count = CTRunGetGlyphCount(env, run).max(0) as u32;
    if count == 0 {
        return Ptr::null();
    }
    let buffer: MutPtr<CGGlyph> = env.mem.calloc(count * 2).cast();
    CTRunGetGlyphs(
        env,
        run,
        CFRange {
            location: 0,
            length: 0,
        },
        buffer,
    );
    env.objc.borrow_mut::<CTLineHostObject>(run).glyphs_ptr = buffer;
    buffer.cast_const()
}

/// `const CGPoint *CTRunGetPositionsPtr(CTRunRef run)`
///
/// See [CTRunGetGlyphsPtr].
fn CTRunGetPositionsPtr(env: &mut Environment, run: CTRunRef) -> ConstPtr<CGPoint> {
    if run.is_null() {
        return Ptr::null();
    }
    let existing = env.objc.borrow::<CTLineHostObject>(run).positions_ptr;
    if !existing.is_null() {
        return existing.cast_const();
    }
    let count = CTRunGetGlyphCount(env, run).max(0) as u32;
    if count == 0 {
        return Ptr::null();
    }
    let size = count * crate::mem::guest_size_of::<CGPoint>();
    let buffer: MutPtr<CGPoint> = env.mem.calloc(size).cast();
    CTRunGetPositions(
        env,
        run,
        CFRange {
            location: 0,
            length: 0,
        },
        buffer,
    );
    env.objc.borrow_mut::<CTLineHostObject>(run).positions_ptr = buffer;
    buffer.cast_const()
}

/// `void CTRunGetPositions(CTRunRef run, CFRange range, CGPoint buffer[])`
fn CTRunGetPositions(env: &mut Environment, run: CTRunRef, range: CFRange, buffer: MutPtr<CGPoint>) {
    if run.is_null() || buffer.is_null() {
        return;
    }
    let (text, ui_font) = {
        let host = env.objc.borrow::<CTLineHostObject>(run);
        (host.text, host.font)
    };
    if text == nil {
        return;
    }
    let count = CTLineGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, count) else {
        return;
    };
    let text_str = to_rust_string(env, text).into_owned();
    let font = font_for_text(env, ui_font, &text_str).unwrap_or_else(Font::sans_regular);
    let size: CGFloat = msg![env; ui_font pointSize];
    let glyphs: Vec<GlyphId> = text_str
        .chars()
        .take(end as usize)
        .map(|character| font.glyph_id_for_unicode(character))
        .collect();
    let positions = font.glyph_positions_at_size(size, &glyphs);
    for text_index in start..end {
        env.mem.write(
            buffer + (text_index - start),
            CGPoint {
                x: positions[text_index as usize],
                y: 0.0,
            },
        );
    }
}

/// Run positions, advances and source indices must describe the same glyphs.
fn CTRunGetAdvances(env: &mut Environment, run: CTRunRef, range: CFRange, buffer: MutPtr<CGSize>) {
    if run == nil || buffer.is_null() {
        return;
    }
    let (text, ui) = {
        let host = env.objc.borrow::<CTLineHostObject>(run);
        (host.text, host.font)
    };
    let count = CTRunGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, count) else {
        return;
    };
    let text_str = to_rust_string(env, text).into_owned();
    let font = font_for_text(env, ui, &text_str).unwrap_or_else(Font::sans_regular);
    let size: CGFloat = msg![env; ui pointSize];
    let glyphs: Vec<GlyphId> = text_str
        .chars()
        .map(|character| font.glyph_id_for_unicode(character))
        .collect();
    let positions = font.glyph_positions_at_size(size, &glyphs);
    for index in start..end {
        let i = index as usize;
        let advance = positions.get(i + 1).map_or_else(
            || font.glyph_advance_at_size(glyphs[i], size),
            |next| next - positions[i],
        );
        env.mem.write(
            buffer + (index - start),
            CGSize {
                width: advance,
                height: 0.0,
            },
        );
    }
}

fn CTRunGetStringIndices(
    env: &mut Environment,
    run: CTRunRef,
    range: CFRange,
    buffer: MutPtr<CFIndex>,
) {
    if run == nil || buffer.is_null() {
        return;
    }
    let count = CTRunGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, count) else {
        return;
    };
    let offset = env
        .objc
        .borrow::<CTLineHostObject>(run)
        .string_range
        .location;
    let text = env.objc.borrow::<CTLineHostObject>(run).text;
    let string = to_rust_string(env, text).into_owned();
    let mut units = offset;
    for (index, ch) in string.chars().enumerate().take(end as usize) {
        if index >= start as usize {
            env.mem.write(buffer + (index as u32 - start), units);
        }
        units += ch.len_utf16() as CFIndex;
    }
}

/// `double CTFontGetAdvancesForGlyphs(CTFontRef font, CTFontOrientation orientation,
///     const CGGlyph glyphs[], CGSize advances[], CFIndex count)`
///
/// Returns the total advance width for the given glyphs and fills the
/// caller's array with per-glyph advances (width only; height 0, matching
/// horizontal orientation). The `orientation` parameter (0 = default /
/// horizontal, 1 = vertical) is accepted but only horizontal metrics are
/// produced — vertical layout is not used by apps in the corpus.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctfontgetadvancesforglyphs(_:_:_:_:_:)>
fn CTFontGetAdvancesForGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    _orientation: u32,
    glyphs: ConstPtr<CGGlyph>,
    advances: MutPtr<CGSize>,
    count: i32,
) -> f64 {
    if font.is_null() || glyphs.is_null() || count <= 0 {
        return 0.0;
    }
    let ui_font = ui_font_for_ct_font(env, font);
    let font_obj = font_from_uifont(env, ui_font).unwrap_or_else(Font::sans_regular);
    let size = CTFontGetSize(env, font);
    let mut total = 0.0f64;
    for i in 0..count as u32 {
        let glyph_id: CGGlyph = env.mem.read(glyphs + i);
        let advance = CTFontGetMatrix(env, font).apply_to_size(CGSize {
            width: font_obj.glyph_advance_at_size(GlyphId(glyph_id), size),
            height: 0.0,
        });
        let advance_pt = advance.width as f64;
        if !advances.is_null() {
            env.mem.write(advances + i, advance);
        }
        total += advance_pt;
    }
    total
}

/// `CGRect CTFontGetBoundingRectsForGlyphs(CTFontRef font, CTFontOrientation orientation,
///     const CGGlyph glyphs[], CGRect boundingRects[], CFIndex count)`
fn CTFontGetBoundingRectsForGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    _orientation: u32,
    glyphs: ConstPtr<CGGlyph>,
    bounding_rects: MutPtr<CGRect>,
    count: CFIndex,
) -> CGRect {
    let empty = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 0.0, height: 0.0 },
    };
    if font.is_null() || glyphs.is_null() || count <= 0 {
        return empty;
    }
    let ui_font = ui_font_for_ct_font(env, font);
    let font_obj = font_from_uifont(env, ui_font).unwrap_or_else(Font::sans_regular);
    let size = CTFontGetSize(env, font);
    let mut union: Option<CGRect> = None;
    for index in 0..count as u32 {
        let glyph = env.mem.read(glyphs + index);
        let (x, y, width, height) = font_obj.glyph_bbox_at_size(GlyphId(glyph), size);
        let rect = CTFontGetMatrix(env, font).apply_to_rect(CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        });
        if !bounding_rects.is_null() {
            env.mem.write(bounding_rects + index, rect);
        }
        union = Some(if let Some(current) = union {
            let min_x = current.origin.x.min(rect.origin.x);
            let min_y = current.origin.y.min(rect.origin.y);
            let max_x = (current.origin.x + current.size.width).max(rect.origin.x + rect.size.width);
            let max_y = (current.origin.y + current.size.height).max(rect.origin.y + rect.size.height);
            CGRect {
                origin: CGPoint { x: min_x, y: min_y },
                size: CGSize { width: max_x - min_x, height: max_y - min_y },
            }
        } else {
            rect
        });
    }
    union.unwrap_or(empty)
}

/// `bool CTFontGetGlyphsForCharacters(CTFontRef font, const UniChar characters[],
///     CGGlyph glyphs[], CFIndex count)`
///
/// Provides basic Unicode-to-glyph mapping for a font, writing one `CGGlyph`
/// per input `UniChar` via the backing font's cmap. Returns `true` only if
/// every character mapped to a non-zero (non-`.notdef`) glyph, exactly as
/// Apple documents.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctfontgetglyphsforcharacters(_:_:_:_:)>
fn CTFontGetGlyphsForCharacters(
    env: &mut Environment,
    font: CTFontRef,
    characters: ConstPtr<unichar>,
    glyphs: MutPtr<CGGlyph>,
    count: i32,
) -> bool {
    if font.is_null() || characters.is_null() || glyphs.is_null() || count <= 0 {
        return false;
    }
    // Resolve the backing UIFont, then its rasterizable Font.
    let ui_font = ui_font_for_ct_font(env, font);
    let Some(font_obj) = font_from_uifont(env, ui_font) else {
        return false;
    };
    let mut all_mapped = true;
    let mut i = 0;
    while i < count as u32 {
        let c: unichar = env.mem.read(characters + i);
        let paired = (0xd800..=0xdbff).contains(&c) && i + 1 < count as u32 && {
            let next: unichar = env.mem.read(characters + i + 1);
            (0xdc00..=0xdfff).contains(&next)
        };
        let glyph_id = if paired {
            let next: unichar = env.mem.read(characters + i + 1);
            let scalar = 0x10000 + ((u32::from(c) - 0xd800) << 10) + (u32::from(next) - 0xdc00);
            font_obj.glyph_id_for_unicode(char::from_u32(scalar).unwrap())
        } else {
            GlyphId(glyph_for_code_unit(&font_obj, c))
        };
        let g = glyph_id.0;
        if g == 0 {
            all_mapped = false;
        }
        env.mem.write(glyphs + i, g);
        if paired {
            i += 1;
            env.mem.write(glyphs + i, 0);
        }
        i += 1;
    }
    all_mapped
}

/// `CGFontRef CTFontCopyGraphicsFont(CTFontRef font, CTFontDescriptorRef *attributes)`
///
/// Returns a `CGFont` for the given `CTFont`. We bridge through the backing
/// `UIFont` and create a `CGFont` from its name, so subsequent CGFont glyph
/// queries operate on the same typeface. The caller owns the returned font
/// (the "Copy" rule), so it is returned retained.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509342-ctfontcopygraphicsfont>
fn CTFontCopyGraphicsFont(
    env: &mut Environment,
    font: CTFontRef,
    attributes: MutPtr<CTFontDescriptorRef>,
) -> CGFontRef {
    if font.is_null() {
        if !attributes.is_null() {
            env.mem.write(attributes, nil);
        }
        return Ptr::null();
    }
    if !attributes.is_null() {
        let descriptor = CTFontCopyFontDescriptor(env, font);
        env.mem.write(attributes, descriptor);
    }
    let ui_font = ui_font_for_ct_font(env, font);
    if let Some(custom) = custom_font_of_uifont(env, ui_font) {
        return cg_font_with_font(env, custom);
    }
    // Derive a font name from the UIFont and build a CGFont from it. This
    // matches what CoreText does (a CTFont and its CGFont share a typeface).
    let name: id = if ui_font != nil {
        msg![env; ui_font fontName]
    } else {
        nil
    };
    let cg_font = if name != nil {
        CGFontCreateWithFontName(env, name)
    } else {
        Ptr::null()
    };
    cg_font
}

/// `CTFramesetterRef CTFramesetterCreateWithAttributedString(CFAttributedStringRef string)`
///
/// Creates a framesetter that manages laying the attributed string into
/// frames. We retain the attributed string and wrap it in a host object so
/// frame creation and size suggestion can re-derive the text and font.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509611-ctframesettercreatewithattribute>
fn CTFramesetterCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTFramesetterRef {
    if string.is_null() {
        return nil;
    }
    retain(env, string);
    alloc_framesetter(env, string)
}

/// `CGSize CTFramesetterSuggestFrameSizeWithConstraints(CTFramesetterRef framesetter,
///     CFRange stringRange, CFDictionaryRef frameAttributes, CGSize constraints,
///     CFRange *fitRange)`
///
/// Determines the frame size needed for a string range. We measure the text
/// with the backing font, wrapping at the constraint width when one is given
/// (a width <= 0 or non-finite means "unconstrained"), and report the fit
/// range as the whole string. This produces correct sizing for the
/// single-/multi-line labels the apps in the corpus lay out.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctframesettersuggestframesizewithconstraints(_:_:_:_:_:)>
fn CTFramesetterSuggestFrameSizeWithConstraints(
    env: &mut Environment,
    framesetter: CTFramesetterRef,
    string_range: CFRange,
    _frame_attributes: id,
    constraints: CGSize,
    fit_range: MutPtr<CFRange>,
) -> CGSize {
    if framesetter.is_null() {
        if !fit_range.is_null() {
            env.mem.write(
                fit_range,
                CFRange {
                    location: 0,
                    length: 0,
                },
            );
        }
        return CGSize {
            width: 0.0,
            height: 0.0,
        };
    }
    let attr_string = env
        .objc
        .borrow::<CTFramesetterHostObject>(framesetter)
        .attr_string;
    let (text, font, _) = string_and_font_from_attr_string(env, attr_string);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);

    let wrap = if constraints.width.is_finite() && constraints.width > 0.0 {
        Some((constraints.width, WrapMode::Word))
    } else {
        None
    };
    let (w, h) = font_obj.calculate_text_size(size, &text_str, wrap);

    let full_len = {
        let len: NSUInteger = msg![env; text length];
        len as i32
    };
    if !fit_range.is_null() {
        // Report the entire requested range as fitting. A length of 0 in the
        // requested range means "to the end of the string" per Apple's docs.
        let length = if string_range.length == 0 {
            (full_len - string_range.location).max(0)
        } else {
            string_range.length
        };
        env.mem.write(
            fit_range,
            CFRange {
                location: string_range.location,
                length,
            },
        );
    }
    CGSize {
        width: w,
        height: h,
    }
}

/// `CTFrameRef CTFramesetterCreateFrame(CTFramesetterRef framesetter,
///     CFRange stringRange, CGPathRef path, CFDictionaryRef frameAttributes)`
///
/// Creates a frame laying the framesetter's attributed string into the
/// supplied path's bounding region. touchHLE represents the laid-out frame as
/// a single `CTLine` over the whole text; `CTFrameGetLines` then returns it.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509702-ctframesettercreateframe>
fn CTFramesetterCreateFrame(
    env: &mut Environment,
    framesetter: CTFramesetterRef,
    string_range: CFRange,
    _path: id,
    _frame_attributes: id,
) -> CTFrameRef {
    if framesetter.is_null() {
        return nil;
    }
    let attr_string = env
        .objc
        .borrow::<CTFramesetterHostObject>(framesetter)
        .attr_string;
    let (text, font, attributes) = string_and_font_from_attr_string(env, attr_string);
    retain(env, text);
    retain(env, font);
    retain(env, attributes);
    let line = alloc_line(env, text, font, attributes);
    // alloc_line took the retains above; the frame owns the line.
    let full_len = {
        let len: NSUInteger = msg![env; text length];
        len as i32
    };
    let range = CFRange {
        location: string_range.location,
        length: if string_range.length == 0 {
            (full_len - string_range.location).max(0)
        } else {
            string_range.length
        },
    };
    alloc_frame(env, line, range)
}

/// `CFArrayRef CTFrameGetLines(CTFrameRef frame)`
///
/// Returns the array of `CTLine`s that make up the frame. touchHLE lays the
/// frame out as a single line, so this returns a one-element array.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509601-ctframegetlines>
fn CTFrameGetLines(env: &mut Environment, frame: CTFrameRef) -> id {
    if frame.is_null() {
        return nil;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    let arr: id = msg_class![env; NSMutableArray arrayWithCapacity:1u32];
    if line != nil {
        () = msg![env; arr addObject:line];
    }
    let immutable: id = msg![env; arr copy];
    autorelease(env, immutable)
}

/// `void CTFrameGetLineOrigins(CTFrameRef frame, CFRange range, CGPoint origins[])`
///
/// Writes the origin of each line into `origins`. Our frame is a single line
/// whose origin sits at the top-left of the frame's coordinate space.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509559-ctframegetlineorigins>
fn CTFrameGetLineOrigins(
    env: &mut Environment,
    frame: CTFrameRef,
    range: CFRange,
    origins: MutPtr<CGPoint>,
) {
    if frame.is_null() || origins.is_null() {
        return;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    let line_count: i32 = if line != nil { 1 } else { 0 };
    let count = if range.length == 0 {
        line_count
    } else {
        range.length.min(line_count)
    };
    for i in 0..count as u32 {
        env.mem.write(origins + i, CGPoint { x: 0.0, y: 0.0 });
    }
}

/// `CFRange CTFrameGetStringRange(CTFrameRef frame)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509593-ctframegetstringrange>
fn CTFrameGetStringRange(env: &mut Environment, frame: CTFrameRef) -> CFRange {
    if frame.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTFrameHostObject>(frame).range
}

/// `CFRange CTFrameGetVisibleStringRange(CTFrameRef frame)`
///
/// We lay the entire string into the (single-line) frame, so the visible
/// range equals the full string range.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509563-ctframegetvisiblestringrange>
fn CTFrameGetVisibleStringRange(env: &mut Environment, frame: CTFrameRef) -> CFRange {
    if frame.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTFrameHostObject>(frame).range
}

/// `void CTFrameDraw(CTFrameRef frame, CGContextRef context)`
///
/// Draws the frame's lines into the context. We draw our single line at the
/// origin, matching `CTLineDraw`.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509601-ctframedraw>
fn CTFrameDraw(env: &mut Environment, frame: CTFrameRef, context: id) {
    if frame.is_null() || context.is_null() {
        return;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    if line != nil {
        CTLineDraw(env, line, context);
    }
}

/// `CTFontRef CTFontCreateWithGraphicsFont(CGFontRef graphicsFont,
///     CGFloat size, const CGAffineTransform *matrix,
///     CTFontDescriptorRef attributes)`
///
/// Creates a CTFont from a CGFont. We wrap a system UIFont at the requested
/// size in a real `_touchHLE_CTFont` host object so subsequent metric queries
/// (`CTFontGetAscent`/`Descent`/`Leading`/`Size`) and line drawing work.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509694-ctfontcreatewithgraphicsfont>
fn CTFontCreateWithGraphicsFont(
    env: &mut Environment,
    graphics_font: id, // CGFontRef
    size: CGFloat,
    matrix: ConstVoidPtr, // const CGAffineTransform*
    attributes: id,       // CTFontDescriptorRef
) -> CTFontRef {
    let font_size = if size > 0.0 { size } else { 12.0 };
    // Keep the typeface the CGFont was made from. Apps load their own font
    // files this way (Real Racing 3 draws all its text and its icon glyphs
    // with bundled fonts); substituting the system font turned icons into
    // plain letters. A CGFont made from a font name is a UIFont here.
    let ui_font: id = if let Some(font) = data_provider_font(env, graphics_font) {
        new_uifont_with_custom_font(env, font, font_size)
    } else if is_uifont(env, graphics_font) {
        msg![env; graphics_font fontWithSize:font_size]
    } else {
        msg_class![env; UIFont systemFontOfSize:font_size]
    };
    retain(env, ui_font);
    let font = alloc_font(env, ui_font);
    let attrs = if attributes == nil {
        nil
    } else {
        descriptor_attrs(env, attributes)
    };
    set_font_matrix(env, font, matrix, attrs);
    font
}

/// `bool CTFontManagerRegisterGraphicsFont(CGFontRef font, CFErrorRef *error)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1499468-ctfontmanagerregistergraphicsfon>
fn font_registration_error(env: &mut Environment, error: MutPtr<id>, code: i32) {
    if error.is_null() {
        return;
    }
    let domain = get_static_str(env, "com.apple.CoreText.CTFontManagerErrorDomain");
    let value: id = msg_class![env; NSError errorWithDomain:domain code:code userInfo:nil];
    retain(env, value);
    env.mem.write(error, value);
}

fn CTFontManagerRegisterGraphicsFont(
    env: &mut Environment,
    font: CGFontRef,
    error: MutPtr<id>,
) -> bool {
    let Some(face) = data_provider_font(env, font) else {
        font_registration_error(env, error, 104);
        return false;
    };
    if face.name(6).is_none_or(|name| name.is_empty()) {
        font_registration_error(env, error, 104);
        return false;
    }
    if !crate::frameworks::uikit::ui_font::register_graphics_font(env, face) {
        font_registration_error(env, error, 105);
        return false;
    }
    true
}

fn CTFontManagerUnregisterGraphicsFont(
    env: &mut Environment,
    font: CGFontRef,
    error: MutPtr<id>,
) -> bool {
    let Some(face) = data_provider_font(env, font) else {
        font_registration_error(env, error, 104);
        return false;
    };
    if !crate::frameworks::uikit::ui_font::unregister_graphics_font(env, &face) {
        font_registration_error(env, error, 201);
        return false;
    }
    true
}

/// Opaque paragraph style reference.
pub type CTParagraphStyleRef = crate::objc::id;

/// `CTParagraphStyleRef CTParagraphStyleCreate(
///     const CTParagraphStyleSetting *settings, size_t settingCount)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1524171-ctparagraphstylecreate>
fn CTParagraphStyleCreate(
    env: &mut Environment,
    _settings: ConstVoidPtr,
    _setting_count: u32,
) -> CTParagraphStyleRef {
    msg_class![env; NSObject new]
}

/// `CTParagraphStyleRef CTParagraphStyleCreateCopy(CTParagraphStyleRef)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1525098-ctparagraphstylecreatecopy>
fn CTParagraphStyleCreateCopy(
    env: &mut Environment,
    paragraph_style: CTParagraphStyleRef,
) -> CTParagraphStyleRef {
    if paragraph_style.is_null() {
        return nil;
    }
    retain(env, paragraph_style);
    paragraph_style
}

/// `bool CTParagraphStyleGetValueForSpecifier(CTParagraphStyleRef, CTParagraphStyleSpecifier,
///     size_t valueBufferSize, void *valueBuffer)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1525353-ctparagraphstylegetvalueforspeci>
fn CTParagraphStyleGetValueForSpecifier(
    env: &mut Environment,
    _paragraph_style: CTParagraphStyleRef,
    _spec: u32,
    value_buffer_size: u32,
    value_buffer: MutPtr<u8>,
) -> bool {
    if !value_buffer.is_null() && value_buffer_size > 0 {
        let slice = env.mem.bytes_at_mut(value_buffer, value_buffer_size);
        slice.fill(0);
    }
    false
}

// MARK: - Line / run metrics used by text-field and label layout

fn line_string_range(env: &mut Environment, line: CTLineRef) -> CFRange {
    if line.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTLineHostObject>(line).string_range
}

/// `CFRange CTLineGetStringRange(CTLineRef line)`
/// <https://developer.apple.com/documentation/coretext/1509536-ctlinegetstringrange>
fn CTLineGetStringRange(env: &mut Environment, line: CTLineRef) -> CFRange {
    line_string_range(env, line)
}

fn line_measure(env: &mut Environment, line: CTLineRef, text: &str) -> f32 {
    let font = line_font(env, line);
    if font == nil {
        return 0.0;
    }
    let size: CGFloat = msg![env; font pointSize];
    let font_obj = font_for_text(env, font, text).unwrap_or_else(Font::sans_regular);
    font_obj.text_advance_at_size(size, text)
}

/// `CFIndex CTLineGetStringIndexForPosition(CTLineRef line, CGPoint position)`
///
/// CoreText returns the UTF-16 index of the character whose advance contains
/// `position.x`. Our layout is a single left-aligned run, so walking the
/// line's characters with the same font metrics used for drawing gives the
/// expected hit-testing result for text fields and links.
/// <https://developer.apple.com/documentation/coretext/1509569-ctlinegetstringindexforposition>
fn CTLineGetStringIndexForPosition(
    env: &mut Environment,
    line: CTLineRef,
    position: CGPoint,
) -> CFIndex {
    if line.is_null() {
        return 0;
    }
    let text = line_text(env, line);
    if text == nil {
        return 0;
    }
    let text_str = to_rust_string(env, text).into_owned();
    let mut utf16_index = line_string_range(env, line).location;
    let mut prefix = String::new();
    for ch in text_str.chars() {
        let mut candidate = prefix.clone();
        candidate.push(ch);
        let width = line_measure(env, line, &candidate);
        if position.x < width {
            return utf16_index;
        }
        prefix = candidate;
        utf16_index += ch.len_utf16() as CFIndex;
    }
    utf16_index
}

/// `CGFloat CTLineGetOffsetForStringIndex(CTLineRef line, CFIndex charIndex,
///     CGFloat *secondaryOffset)`
/// <https://developer.apple.com/documentation/coretext/1509561-ctlinegetoffsetforstringindex>
fn CTLineGetOffsetForStringIndex(
    env: &mut Environment,
    line: CTLineRef,
    char_index: CFIndex,
    secondary_offset: MutPtr<CGFloat>,
) -> CGFloat {
    let location = line_string_range(env, line).location;
    if line.is_null() || char_index <= location {
        if !secondary_offset.is_null() {
            env.mem.write(secondary_offset, 0.0);
        }
        return 0.0;
    }
    let text = line_text(env, line);
    if text == nil {
        return 0.0;
    }
    let text_str = to_rust_string(env, text).into_owned();
    let mut utf16_seen = 0usize;
    let mut prefix = String::new();
    for ch in text_str.chars() {
        if utf16_seen + ch.len_utf16() > (char_index - location) as usize {
            break;
        }
        prefix.push(ch);
        utf16_seen += ch.len_utf16();
    }
    let offset = line_measure(env, line, &prefix);
    if !secondary_offset.is_null() {
        env.mem.write(secondary_offset, offset);
    }
    offset
}

/// `double CTLineGetPenOffsetForFlush(CTLineRef line, CGFloat flushFactor,
///     double flushWidth)` — offsets a line inside a flush-width box.
/// <https://developer.apple.com/documentation/coretext/1509565-ctlinegetpenoffsetforflush>
fn CTLineGetPenOffsetForFlush(
    env: &mut Environment,
    line: CTLineRef,
    flush_factor: CGFloat,
    flush_width: f64,
) -> f64 {
    if line.is_null() {
        return 0.0;
    }
    let width = CTLineGetTypographicBounds(
        env,
        line,
        MutPtr::null(),
        MutPtr::null(),
        MutPtr::null(),
    ) as f64;
    (flush_width - width) * flush_factor as f64
}

/// `CTRunStatus CTRunGetStatus(CTRunRef run)` — our single run never carries
/// the non-monotonic or right-to-left status bits.
/// <https://developer.apple.com/documentation/coretext/1509621-ctrungetstatus>
fn CTRunGetStatus(_env: &mut Environment, _run: CTRunRef) -> u32 {
    0
}

/// `CFRange CTRunGetStringRange(CTRunRef run)`
/// <https://developer.apple.com/documentation/coretext/1509573-ctrungetstringrange>
fn CTRunGetStringRange(env: &mut Environment, run: CTRunRef) -> CFRange {
    line_string_range(env, run)
}

/// `double CTRunGetTypographicBounds(CTRunRef run, CFRange range,
///     CGFloat *ascent, CGFloat *descent, CGFloat *leading)`
/// <https://developer.apple.com/documentation/coretext/1509614-ctrungettypographicbounds>
fn CTRunGetTypographicBounds(
    env: &mut Environment,
    run: CTRunRef,
    range: CFRange,
    ascent: MutPtr<CGFloat>,
    descent: MutPtr<CGFloat>,
    leading: MutPtr<CGFloat>,
) -> f64 {
    if run == nil {
        return 0.0;
    }
    let total = CTRunGetGlyphCount(env, run);
    let Some((start, end)) = glyph_range_for_run(range, total) else {
        return 0.0;
    };
    CTLineGetTypographicBounds(env, run, ascent, descent, leading);
    let text = line_text(env, run);
    let string = to_rust_string(env, text).into_owned();
    let ui = line_font(env, run);
    let font = font_for_text(env, ui, &string).unwrap_or_else(Font::sans_regular);
    let size: CGFloat = msg![env; ui pointSize];
    let glyphs: Vec<_> = string
        .chars()
        .map(|ch| font.glyph_id_for_unicode(ch))
        .collect();
    let positions = font.glyph_positions_at_size(size, &glyphs);
    if start == end {
        return 0.0;
    }
    let right = positions.get(end as usize).copied().unwrap_or_else(|| {
        positions.last().unwrap() + font.glyph_advance_at_size(*glyphs.last().unwrap(), size)
    });
    f64::from(right - positions[start as usize])
}

/// `CGFloat CTFontGetUnderlineThickness(CTFontRef font)`
/// CoreText derives this from the font's underline metrics; for the system
/// fonts used by iPhone OS apps a size-proportional approximation is enough
/// for underline drawing code.
/// <https://developer.apple.com/documentation/coretext/1509575-ctfontgetunderlinethickness>
fn CTFontGetUnderlineThickness(env: &mut Environment, font: CTFontRef) -> CGFloat {
    if font.is_null() {
        return 0.0;
    }
    let ui_font = match env
        .objc
        .get_host_object(font)
        .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
    {
        Some(host) => host.font,
        None => return 0.0,
    };
    if ui_font == nil {
        return 0.0;
    }
    let size: CGFloat = msg![env; ui_font pointSize];
    (size * 0.08).max(0.5)
}

/// `CFStringRef CTFontCopyName(CTFontRef font, CFStringRef nameKey)`
/// <https://developer.apple.com/documentation/coretext/1509609-ctfontcopyname>
fn CTFontCopyName(env: &mut Environment, font: CTFontRef, name_key: CFStringRef) -> id {
    if font.is_null() {
        return nil;
    }
    let ui_font = match env
        .objc
        .get_host_object(font)
        .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
    {
        Some(host) => host.font,
        None => return nil,
    };
    if ui_font == nil {
        return nil;
    }
    let key = if name_key == nil {
        String::new()
    } else {
        to_rust_string(env, name_key).into_owned()
    };
    let name: id = if key.contains("Family") {
        msg![env; ui_font familyName]
    } else {
        msg![env; ui_font fontName]
    };
    if name == nil {
        nil
    } else {
        retain(env, name)
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CTTypesetterSuggestLineBreak(_, _, _)),
    export_c_func!(CTTypesetterSuggestLineBreakWithOffset(_, _, _, _)),
    export_c_func!(CTTypesetterSuggestClusterBreakWithOffset(_, _, _, _)),
    export_c_func!(CTRunGetAdvances(_, _, _)),
    export_c_func!(CTRunGetStringIndices(_, _, _)),
    export_c_func!(CTFontCopyFamilyName(_)),
    export_c_func!(CTFontCopyFullName(_)),
    export_c_func!(CTFontCopyFontDescriptor(_)),
    export_c_func!(CTFontDescriptorCreateCopyWithAttributes(_, _)),
    export_c_func!(CTFontGetSymbolicTraits(_)),
    export_c_func!(CTFontGetMatrix(_)),
    export_c_func!(CTFontDrawGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontManagerUnregisterGraphicsFont(_, _)),
    export_c_func!(CTGetCoreTextVersion()),
    export_c_func!(CTFontCreateWithGraphicsFont(_, _, _, _)),
    export_c_func!(CTFontCreateWithName(_, _, _)),
    export_c_func!(CTFontCreateWithFontDescriptor(_, _, _)),
    export_c_func!(CTFontCreateCopyWithAttributes(_, _, _, _)),
    export_c_func!(CTFontDescriptorCreateWithAttributes(_)),
    export_c_func!(CTFontDescriptorCreateWithNameAndSize(_, _)),
    export_c_func!(CTFontDescriptorCreateMatchingFontDescriptors(_, _)),
    export_c_func!(CTFontDescriptorCreateMatchingFontDescriptor(_, _)),
    export_c_func!(CTFontDescriptorCopyAttribute(_, _)),
    export_c_func!(CTFontManagerRegisterGraphicsFont(_, _)),
    export_c_func!(CTFontGetAscent(_)),
    export_c_func!(CTFontGetDescent(_)),
    export_c_func!(CTFontGetLeading(_)),
    export_c_func!(CTFontGetSize(_)),
    export_c_func!(CTFontCopyPostScriptName(_)),
    export_c_func!(CTFontCopyName(_, _)),
    export_c_func!(CTFontGetUnderlineThickness(_)),
    export_c_func!(CTFontGetAdvancesForGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontGetBoundingRectsForGlyphs(_, _, _, _, _)),
    export_c_func!(CTRunGetGlyphCount(_)),
    export_c_func!(CTRunGetAttributes(_)),
    export_c_func!(CTRunGetGlyphs(_, _, _)),
    export_c_func!(CTRunGetGlyphsPtr(_)),
    export_c_func!(CTRunGetPositionsPtr(_)),
    export_c_func!(CTRunGetPositions(_, _, _)),
    export_c_func!(CTParagraphStyleCreate(_, _)),
    export_c_func!(CTParagraphStyleCreateCopy(_)),
    export_c_func!(CTParagraphStyleGetValueForSpecifier(_, _, _, _)),
    export_c_func!(CTTypesetterCreateWithAttributedString(_)),
    export_c_func!(CTTypesetterCreateLine(_, _)),
    export_c_func!(CTTypesetterSuggestClusterBreak(_, _, _)),
    export_c_func!(CTLineCreateWithAttributedString(_)),
    export_c_func!(CTLineCreateTruncatedLine(_, _, _, _)),
    export_c_func!(CTLineGetTypographicBounds(_, _, _, _)),
    export_c_func!(CTLineGetImageBounds(_, _)),
    export_c_func!(CTLineDraw(_, _)),
    export_c_func!(CTLineGetGlyphCount(_)),
    export_c_func!(CTLineGetGlyphRuns(_)),
    export_c_func!(CTLineGetStringRange(_)),
    export_c_func!(CTLineGetStringIndexForPosition(_, _)),
    export_c_func!(CTLineGetOffsetForStringIndex(_, _, _)),
    export_c_func!(CTLineGetPenOffsetForFlush(_, _, _)),
    export_c_func!(CTRunGetStatus(_)),
    export_c_func!(CTRunGetStringRange(_)),
    export_c_func!(CTRunGetTypographicBounds(_, _, _, _, _)),
    export_c_func!(CTFontGetGlyphsForCharacters(_, _, _, _)),
    export_c_func!(CTFontCopyGraphicsFont(_, _)),
    export_c_func!(CTFramesetterCreateWithAttributedString(_)),
    export_c_func!(CTFramesetterSuggestFrameSizeWithConstraints(_, _, _, _, _)),
    export_c_func!(CTFramesetterCreateFrame(_, _, _, _)),
    export_c_func!(CTFrameGetLines(_)),
    export_c_func!(CTFrameGetLineOrigins(_, _, _)),
    export_c_func!(CTFrameGetStringRange(_)),
    export_c_func!(CTFrameGetVisibleStringRange(_)),
    export_c_func!(CTFrameDraw(_, _)),
];

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCTFontManagerErrorDomain",
        HostConstant::NSString("com.apple.CoreText.CTFontManagerErrorDomain"),
    ),
    (
        "_kCTFontVariationAttribute",
        HostConstant::NSString("NSCTFontVariationAttribute"),
    ),
    // CTFontDescriptor.h
    (
        "_kCTFontNameAttribute",
        HostConstant::NSString("NSFontNameAttribute"),
    ),
    (
        "_kCTFontFamilyNameAttribute",
        HostConstant::NSString("NSFontFamilyAttribute"),
    ),
    (
        "_kCTFontStyleNameAttribute",
        HostConstant::NSString("NSFontFaceAttribute"),
    ),
    (
        "_kCTFontTraitsAttribute",
        HostConstant::NSString("NSCTFontTraitsAttribute"),
    ),
    (
        "_kCTFontURLAttribute",
        HostConstant::NSString("NSCTFontFileURLAttribute"),
    ),
    (
        "_kCTFontDisplayNameAttribute",
        HostConstant::NSString("NSFontVisibleNameAttribute"),
    ),
    (
        "_kCTFontSizeAttribute",
        HostConstant::NSString("NSFontSizeAttribute"),
    ),
    (
        "_kCTFontMatrixAttribute",
        HostConstant::NSString("NSCTFontMatrixAttribute"),
    ),
    (
        "_kCTFontCascadeListAttribute",
        HostConstant::NSString("NSCTFontCascadeListAttribute"),
    ),
    (
        "_kCTFontCharacterSetAttribute",
        HostConstant::NSString("NSCTFontCharacterSetAttribute"),
    ),
    (
        "_kCTFontLanguagesAttribute",
        HostConstant::NSString("NSCTFontLanguagesAttribute"),
    ),
    (
        "_kCTFontBaselineAdjustAttribute",
        HostConstant::NSString("NSCTFontBaselineAdjustAttribute"),
    ),
    (
        "_kCTFontMacintoshEncodingsAttribute",
        HostConstant::NSString("NSCTFontMacintoshEncodingsAttribute"),
    ),
    (
        "_kCTFontFeaturesAttribute",
        HostConstant::NSString("NSCTFontFeaturesAttribute"),
    ),
    (
        "_kCTFontFeatureSettingsAttribute",
        HostConstant::NSString("NSCTFontFeatureSettingsAttribute"),
    ),
    (
        "_kCTFontFixedAdvanceAttribute",
        HostConstant::NSString("NSCTFontFixedAdvanceAttribute"),
    ),
    (
        "_kCTFontOrientationAttribute",
        HostConstant::NSString("NSCTFontOrientationAttribute"),
    ),
    (
        "_kCTFontFormatAttribute",
        HostConstant::NSString("NSCTFontFormatAttribute"),
    ),
    (
        "_kCTFontRegistrationScopeAttribute",
        HostConstant::NSString("NSCTFontRegistrationScopeAttribute"),
    ),
    (
        "_kCTFontPriorityAttribute",
        HostConstant::NSString("NSCTFontPriorityAttribute"),
    ),
    (
        "_kCTFontEnabledAttribute",
        HostConstant::NSString("NSCTFontEnabledAttribute"),
    ),
    (
        "_kCTFontDownloadableAttribute",
        HostConstant::NSString("NSCTFontDownloadableAttribute"),
    ),
    (
        "_kCTFontDownloadedAttribute",
        HostConstant::NSString("NSCTFontDownloadedAttribute"),
    ),
    // CTFontTraits.h
    (
        "_kCTFontSymbolicTrait",
        HostConstant::NSString("NSCTFontSymbolicTrait"),
    ),
    (
        "_kCTFontWeightTrait",
        HostConstant::NSString("NSCTFontWeightTrait"),
    ),
    (
        "_kCTFontWidthTrait",
        HostConstant::NSString("NSCTFontWidthTrait"),
    ),
    (
        "_kCTFontSlantTrait",
        HostConstant::NSString("NSCTFontSlantTrait"),
    ),
    // CTStringAttributes.h — attribute keys for `CFAttributedStringRef`
    // (and, toll-free bridged, `NSAttributedString`) used by
    // `CTFramesetterCreateWithAttributedString` and friends.
    // Canonical string values come from Apple's public
    // `CTStringAttributes.h` header; many deliberately share their
    // value with the corresponding `NSAttributedString` attribute name
    // so the same dictionary can be used by both CoreText and
    // UIKit/AppKit.
    (
        "_kCTFontAttributeName",
        // Same value as `NSFontAttributeName`.
        HostConstant::NSString("NSFont"),
    ),
    (
        "_kCTForegroundColorAttributeName",
        HostConstant::NSString("CTForegroundColor"),
    ),
    (
        "_kCTForegroundColorFromContextAttributeName",
        HostConstant::NSString("CTForegroundColorFromContext"),
    ),
    (
        "_kCTBackgroundColorAttributeName",
        HostConstant::NSString("kCTBackgroundColorAttributeName"),
    ),
    (
        "_kCTKernAttributeName",
        // Same value as `NSKernAttributeName`.
        HostConstant::NSString("NSKern"),
    ),
    (
        "_kCTLigatureAttributeName",
        // Same value as `NSLigatureAttributeName`.
        HostConstant::NSString("NSLigature"),
    ),
    (
        "_kCTParagraphStyleAttributeName",
        // Same value as `NSParagraphStyleAttributeName`.
        HostConstant::NSString("NSParagraphStyle"),
    ),
    (
        "_kCTStrokeWidthAttributeName",
        // Same value as `NSStrokeWidthAttributeName`.
        HostConstant::NSString("NSStrokeWidth"),
    ),
    (
        "_kCTStrokeColorAttributeName",
        // Same value as `NSStrokeColorAttributeName`.
        HostConstant::NSString("NSStrokeColor"),
    ),
    (
        "_kCTUnderlineStyleAttributeName",
        HostConstant::NSString("CTUnderlineStyle"),
    ),
    (
        "_kCTUnderlineColorAttributeName",
        HostConstant::NSString("CTUnderlineColor"),
    ),
    (
        "_kCTSuperscriptAttributeName",
        // Same value as `NSSuperscriptAttributeName`.
        HostConstant::NSString("NSSuperScript"),
    ),
    (
        "_kCTVerticalFormsAttributeName",
        HostConstant::NSString("CTVerticalForms"),
    ),
    (
        "_kCTGlyphInfoAttributeName",
        HostConstant::NSString("CTGlyphInfo"),
    ),
    (
        "_kCTCharacterShapeAttributeName",
        // Same value as `NSCharacterShapeAttributeName`.
        HostConstant::NSString("NSCharacterShape"),
    ),
    (
        "_kCTLanguageAttributeName",
        HostConstant::NSString("CTLanguage"),
    ),
    (
        "_kCTRunDelegateAttributeName",
        HostConstant::NSString("CTRunDelegate"),
    ),
    (
        "_kCTBaselineClassAttributeName",
        HostConstant::NSString("CTBaselineClass"),
    ),
    (
        "_kCTBaselineInfoAttributeName",
        HostConstant::NSString("CTBaselineInfo"),
    ),
    (
        "_kCTBaselineReferenceInfoAttributeName",
        HostConstant::NSString("CTBaselineReferenceInfo"),
    ),
    (
        "_kCTBaselineOffsetAttributeName",
        // Same value as `NSBaselineOffsetAttributeName`.
        HostConstant::NSString("NSBaselineOffset"),
    ),
    (
        "_kCTWritingDirectionAttributeName",
        // Same value as `NSWritingDirectionAttributeName`.
        HostConstant::NSString("NSWritingDirection"),
    ),
    (
        "_kCTTrackingAttributeName",
        HostConstant::NSString("CTTracking"),
    ),
];

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreText.framework/CoreText",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};

#[cfg(test)]
mod tests {
    use super::{system_font_file, text_break};
    use crate::font::Font;

    #[test]
    fn breaks_use_utf16_indices_and_do_not_measure_the_preceding_prefix() {
        let font = Font::mono_regular();
        assert_eq!(
            text_break(&font, 20.0, "wide prefix x😀yz", 13, f64::INFINITY, false),
            4
        );
        assert_eq!(
            text_break(&font, 20.0, "prefix x😀yz", 8, f64::INFINITY, true),
            4
        );
        assert_eq!(text_break(&font, 20.0, "x😀yz", 2, f64::INFINITY, false), 0);
    }

    #[test]
    fn breaks_preserve_graphemes_and_stop_at_hard_line_breaks() {
        let font = Font::mono_regular();
        let one_cluster = f64::from(font.text_advance_at_size(20.0, "A\u{301}")) + 0.01;
        assert_eq!(
            text_break(&font, 20.0, "A\u{301}bc", 0, one_cluster, false),
            2
        );
        assert_eq!(
            text_break(&font, 20.0, "x😀\r\ny", 1, f64::INFINITY, true),
            4
        );
        assert_eq!(
            text_break(&font, 20.0, "xy\u{2028}z", 0, f64::INFINITY, true),
            3
        );
    }

    #[test]
    fn word_break_uses_the_last_line_break_opportunity() {
        let font = Font::mono_regular();
        let budget = f64::from(font.text_advance_at_size(20.0, "one tw")) + 0.01;
        assert_eq!(text_break(&font, 20.0, "one two three", 0, budget, true), 4);
        assert_eq!(
            text_break(&font, 20.0, "one two three", 0, budget, false),
            6
        );
    }

    #[test]
    fn font_names_map_to_bundled_files() {
        assert_eq!(system_font_file("Arial", ""), "Arial.ttf");
        assert_eq!(system_font_file("ArialMT", "Bold"), "LiberationSans-Bold.ttf");
        assert_eq!(system_font_file("Helvetica-Bold", ""), "helvetica-bold.ttf");
        assert_eq!(system_font_file("Helvetica", ""), "LiberationSans-Regular.ttf");
        assert_eq!(
            system_font_file("TimesNewRomanPS-ItalicMT", ""),
            "LiberationSerif-Italic.ttf"
        );
        assert_eq!(system_font_file("Courier New", "Bold Italic"), "LiberationMono-BoldItalic.ttf");
        assert_eq!(system_font_file("", ""), "LiberationSans-Regular.ttf");
    }
}
