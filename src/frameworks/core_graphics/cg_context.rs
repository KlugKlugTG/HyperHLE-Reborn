/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGContext.h`

use super::cg_affine_transform::{CGAffineTransform, CGAffineTransformIdentity};
use super::cg_bitmap_context::{
    CGBitmapContextDrawer, CGBitmapContextGetHeight, CGBitmapContextGetWidth,
};
use super::cg_color::CGColorRef;
use super::cg_color_space::{
    kCGColorSpaceModelCMYK, kCGColorSpaceModelMonochrome, kCGColorSpaceModelRGB,
    CGColorSpaceGetModel, CGColorSpaceModel,
};
use super::cg_font::{CGFontHostObject, CGFontRef, CGFontRelease, CGFontRetain, CGGlyph};
use super::cg_geometry::CGPointZero;
use super::cg_image::CGImageRef;
use super::{cg_bitmap_context, cg_color, CGFloat, CGPoint, CGRect};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::uikit;
use crate::mem::{ConstPtr, GuestUSize};
use crate::objc::{objc_classes, ClassExports, HostObject};
use crate::Environment;

type CGInterpolationQuality = i32;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGContext seems to be a CFType-based type, but in our implementation those
// are just Objective-C types, so we need a class for it, but its name is not
// visible anywhere.
@implementation _touchHLE_CGContext: NSObject

- (())dealloc {
    let host_obj = env.objc.borrow::<CGContextHostObject>(this);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    if bitmap_data.data_is_owned {
        env.mem.free(bitmap_data.data);
    }
    let font = host_obj.font;
    CGFontRelease(env, font);

    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

#[derive(Default)]
pub(super) struct CGContextHostObject {
    pub(super) subclass: CGContextSubclass,
    pub(super) rgb_fill_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub(super) fill_color_space_model: CGColorSpaceModel,
    pub(super) rgb_stroke_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub(super) alpha: CGFloat,
    pub(super) line_width: CGFloat,
    pub(super) line_cap: i32,
    pub(super) line_join: i32,
    pub(super) miter_limit: CGFloat,
    pub(super) flatness: CGFloat,
    pub(super) blend_mode: i32,
    pub(super) interpolation_quality: CGInterpolationQuality,
    /// Current font (or null). Used by CGContextShowGlyphsAtPoint.
    pub(super) font: CGFontRef,
    /// Current font size in points.
    pub(super) font_size: CGFloat,
    pub(super) transform: CGAffineTransform,
    /// Text transform for glyph drawing (see `CGContextSetTextMatrix`).
    pub(super) text_transform: Option<CGAffineTransform>,
    /// (fill, stroke, alpha, line_width, line_cap, line_join, miter_limit,
    ///  flatness, blend_mode, transform)
    pub(super) state_stack: Vec<CGContextState>,
    // Path is not graphics state: geometry is transformed when appended.
    pub(super) path_elements: Vec<super::cg_path::PathElement>,
    /// Current rendering intent for the fill color space. Set by
    /// `CGContextSetRenderingIntent`; defaults to `kCGRenderingIntentDefault`
    /// per Apple's "Core Graphics – Color Spaces" documentation.
    pub(super) rendering_intent: i32,
    /// Current shadow state: (offset_x, offset_y, blur, color_rgba). When
    /// blur is zero, no shadow is drawn (matching Apple's CGContext docs).
    pub(super) shadow: CGShadowState,
}

/// State for shadow operations. Stored verbatim on the host object so that
/// `CGContextSaveGState`/`RestoreGState` can preserve it across drawing
/// blocks, mirroring real Quartz behaviour.
#[derive(Clone, Copy)]
pub struct CGShadowState {
    pub enabled: bool,
    pub offset_x: CGFloat,
    pub offset_y: CGFloat,
    pub blur: CGFloat,
    pub color: (CGFloat, CGFloat, CGFloat, CGFloat),
}

impl Default for CGShadowState {
    fn default() -> Self {
        // Apple defaults: black shadow, alpha 1/3, see
        // <https://developer.apple.com/documentation/coregraphics/1455324-cgcontextsetshadow>.
        CGShadowState {
            enabled: false,
            offset_x: 0.0,
            offset_y: 0.0,
            blur: 0.0,
            color: (0.0, 0.0, 0.0, 1.0 / 3.0),
        }
    }
}
impl HostObject for CGContextHostObject {}

#[derive(Clone)]
pub(super) struct CGContextState {
    pub fill_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub fill_color_space_model: CGColorSpaceModel,
    pub stroke_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub alpha: CGFloat,
    pub line_width: CGFloat,
    pub line_cap: i32,
    pub line_join: i32,
    pub miter_limit: CGFloat,
    pub flatness: CGFloat,
    pub blend_mode: i32,
    pub interpolation_quality: CGInterpolationQuality,
    pub transform: CGAffineTransform,
    pub font: CGFontRef,
    pub font_size: CGFloat,
    pub rendering_intent: i32,
    pub shadow: CGShadowState,
}

pub(super) enum CGContextSubclass {
    CGBitmapContext(cg_bitmap_context::CGBitmapContextData),
}
impl Default for CGContextSubclass {
    // Phantom-fallback for the objc `borrow` path; a default bitmap context
    // with zero dimensions is the cheapest stable state.
    fn default() -> Self {
        CGContextSubclass::CGBitmapContext(Default::default())
    }
}

pub type CGContextRef = CFTypeRef;

pub fn CGContextRelease(env: &mut Environment, c: CGContextRef) {
    if !c.is_null() {
        CFRelease(env, c);
    }
}
pub fn CGContextRetain(env: &mut Environment, c: CGContextRef) -> CGContextRef {
    if !c.is_null() {
        CFRetain(env, c)
    } else {
        c
    }
}

fn CGContextSetFillColorWithColor(env: &mut Environment, context: CGContextRef, color: CGColorRef) {
    let (r, g, b, a) = cg_color::to_rgba(&env.objc, color);
    CGContextSetRGBFillColor(env, context, r, g, b, a)
}

fn CGContextSetFillColor(
    env: &mut Environment,
    context: CGContextRef,
    components: ConstPtr<CGFloat>,
) {
    if context.is_null() || components.is_null() {
        return;
    }
    let model = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .fill_color_space_model;
    let color = match model {
        kCGColorSpaceModelMonochrome => {
            let gray = env.mem.read(components);
            let alpha = env.mem.read(components + 1);
            (gray, gray, gray, alpha)
        }
        kCGColorSpaceModelCMYK => {
            let cyan = env.mem.read(components);
            let magenta = env.mem.read(components + 1);
            let yellow = env.mem.read(components + 2);
            let black = env.mem.read(components + 3);
            let alpha = env.mem.read(components + 4);
            (
                (1.0 - cyan) * (1.0 - black),
                (1.0 - magenta) * (1.0 - black),
                (1.0 - yellow) * (1.0 - black),
                alpha,
            )
        }
        _ => (
            env.mem.read(components),
            env.mem.read(components + 1),
            env.mem.read(components + 2),
            env.mem.read(components + 3),
        ),
    };
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = color;
}

pub fn CGContextSetRGBFillColor(
    env: &mut Environment,
    context: CGContextRef,
    red: CGFloat,
    green: CGFloat,
    blue: CGFloat,
    alpha: CGFloat,
) {
    let color = (red, green, blue, alpha);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = color;
}

pub fn CGContextSetRGBStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    red: CGFloat,
    green: CGFloat,
    blue: CGFloat,
    alpha: CGFloat,
) {
    if context.is_null() {
        return;
    }
    // Пишем напрямую в поле структуры через borrow_mut
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_stroke_color = (red, green, blue, alpha);
}

// MARK: - Stroke colour helpers

fn CGContextSetStrokeColorWithColor(
    env: &mut Environment,
    context: CGContextRef,
    color: CGColorRef,
) {
    if context.is_null() {
        return;
    }
    let (r, g, b, a) = cg_color::to_rgba(&env.objc, color);
    CGContextSetRGBStrokeColor(env, context, r, g, b, a);
}

fn CGContextSetGrayStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    gray: CGFloat,
    alpha: CGFloat,
) {
    CGContextSetRGBStrokeColor(env, context, gray, gray, gray, alpha);
}

/// `void CGContextSetStrokeColor(CGContextRef c, const CGFloat components[])`
///
/// Colour-space-agnostic stroke colour setter. touchHLE contexts only track
/// RGBA, so we interpret the components as RGBA (matching the most common
/// device-RGB usage). `CGContextGetShouldColorSpace`-aware behaviour is not
/// modelled.
fn CGContextSetStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    components: ConstPtr<CGFloat>,
) {
    if context.is_null() || components.is_null() {
        return;
    }
    let r: CGFloat = env.mem.read(components + 0);
    let g: CGFloat = env.mem.read(components + 1);
    let b: CGFloat = env.mem.read(components + 2);
    let a: CGFloat = env.mem.read(components + 3);
    CGContextSetRGBStrokeColor(env, context, r, g, b, a);
}

// MARK: - Alpha

fn CGContextSetAlpha(env: &mut Environment, context: CGContextRef, alpha: CGFloat) {
    if context.is_null() {
        return;
    }
    env.objc.borrow_mut::<CGContextHostObject>(context).alpha = alpha.clamp(0.0, 1.0);
}

// MARK: - Line style

fn CGContextSetLineWidth(env: &mut Environment, context: CGContextRef, width: CGFloat) {
    if context.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .line_width = width;
}

fn CGContextSetLineCap(env: &mut Environment, context: CGContextRef, cap: i32) {
    if context.is_null() {
        return;
    }
    env.objc.borrow_mut::<CGContextHostObject>(context).line_cap = cap;
}

fn CGContextSetLineJoin(env: &mut Environment, context: CGContextRef, join: i32) {
    if context.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .line_join = join;
}

fn CGContextSetMiterLimit(env: &mut Environment, context: CGContextRef, limit: CGFloat) {
    if context.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .miter_limit = limit;
}

fn CGContextSetLineDash(
    _env: &mut Environment,
    _context: CGContextRef,
    _phase: CGFloat,
    _lengths: crate::mem::ConstPtr<CGFloat>,
    _count: usize,
) {
    // No dash rendering — stub.
}

fn CGContextSetFlatness(env: &mut Environment, context: CGContextRef, flatness: CGFloat) {
    if context.is_null() {
        return;
    }
    env.objc.borrow_mut::<CGContextHostObject>(context).flatness = flatness;
}

// MARK: - Blend mode / shadow

fn CGContextSetBlendMode(env: &mut Environment, context: CGContextRef, mode: i32) {
    if context.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .blend_mode = mode;
}

fn CGContextSetShadow(
    env: &mut Environment,
    context: CGContextRef,
    offset: super::CGSize,
    blur: CGFloat,
) {
    if context.is_null() {
        return;
    }
    // Per Apple's CGContext documentation:
    // "Sets the shadow drawing parameters... Specify a positive blur value
    // to draw the shadow with a blurred edge; a blur of 0.0 produces no
    // blur." The default shadow color when no explicit color is passed is
    // "black with 1/3 alpha".
    // https://developer.apple.com/documentation/coregraphics/1454559-cgcontextsetshadow
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.shadow = CGShadowState {
        enabled: true,
        offset_x: offset.width,
        offset_y: offset.height,
        blur,
        color: (0.0, 0.0, 0.0, 1.0 / 3.0),
    };
}

fn CGContextSetShadowWithColor(
    env: &mut Environment,
    context: CGContextRef,
    offset: super::CGSize,
    blur: CGFloat,
    color: CGColorRef,
) {
    if context.is_null() {
        return;
    }
    // Per Apple:
    // "If the color parameter is NULL, then shadowing is disabled."
    // https://developer.apple.com/documentation/coregraphics/1456225-cgcontextsetshadowwithcolor
    if color.is_null() {
        env.objc
            .borrow_mut::<CGContextHostObject>(context)
            .shadow
            .enabled = false;
        return;
    }
    let rgba = cg_color::to_rgba(&env.objc, color);
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.shadow = CGShadowState {
        enabled: true,
        offset_x: offset.width,
        offset_y: offset.height,
        blur,
        color: rgba,
    };
}

fn CGContextSetFillColorSpace(
    env: &mut Environment,
    context: CGContextRef,
    color_space: CFTypeRef,
) {
    if context.is_null() {
        return;
    }
    let model = if color_space.is_null() {
        kCGColorSpaceModelRGB
    } else {
        CGColorSpaceGetModel(env, color_space)
    };
    let model = match model {
        kCGColorSpaceModelMonochrome | kCGColorSpaceModelRGB | kCGColorSpaceModelCMYK => model,
        _ => kCGColorSpaceModelRGB,
    };
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.fill_color_space_model = model;
    host.rgb_fill_color = (0.0, 0.0, 0.0, 1.0);
}

fn CGContextSetStrokeColorSpace(
    env: &mut Environment,
    context: CGContextRef,
    color_space: CFTypeRef,
) {
    if context.is_null() {
        return;
    }
    // Same rationale as CGContextSetFillColorSpace.
    // https://developer.apple.com/documentation/coregraphics/1455379-cgcontextsetstrokecolorspace
    let _ = color_space;
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_stroke_color = (0.0, 0.0, 0.0, 1.0);
}

fn CGContextSetRenderingIntent(env: &mut Environment, context: CGContextRef, intent: i32) {
    if context.is_null() {
        return;
    }
    // Per Apple's "CGColorRenderingIntent" reference:
    // https://developer.apple.com/documentation/coregraphics/cgcolorrenderingintent
    // - kCGRenderingIntentDefault (0)
    // - kCGRenderingIntentAbsoluteColorimetric (1)
    // - kCGRenderingIntentRelativeColorimetric (2)
    // - kCGRenderingIntentPerceptual (3)
    // - kCGRenderingIntentSaturation (4)
    // We don't perform any CMS, so just record the chosen intent so that
    // round-tripping via CGContextGetState/RestoreState preserves it.
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rendering_intent = intent;
}

// MARK: - Stroking rects / ellipses

pub fn CGContextStrokeRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if context.is_null() {
        return;
    }
    let lw = env.objc.borrow::<CGContextHostObject>(context).line_width;
    CGContextStrokeRectWithWidth(env, context, rect, lw);
}

pub fn CGContextStrokeRectWithWidth(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    width: CGFloat,
) {
    if context.is_null() {
        return;
    }
    let (r, g, b, a) = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .rgb_stroke_color;
    // Draw four filled thin rects forming the border.
    let _hw = width / 2.0;
    let CGRect { origin, size } = rect;

    // Top, bottom, left, right bands.
    let top = CGRect {
        origin: CGPoint {
            x: origin.x,
            y: origin.y,
        },
        size: super::CGSize {
            width: size.width,
            height: width,
        },
    };
    let bottom = CGRect {
        origin: CGPoint {
            x: origin.x,
            y: origin.y + size.height - width,
        },
        size: super::CGSize {
            width: size.width,
            height: width,
        },
    };
    let left = CGRect {
        origin: CGPoint {
            x: origin.x,
            y: origin.y,
        },
        size: super::CGSize {
            width,
            height: size.height,
        },
    };
    let right = CGRect {
        origin: CGPoint {
            x: origin.x + size.width - width,
            y: origin.y,
        },
        size: super::CGSize {
            width,
            height: size.height,
        },
    };

    // Temporarily set fill to stroke colour.
    let saved_fill = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .rgb_fill_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    for band in [top, bottom, left, right] {
        cg_bitmap_context::fill_rect(env, context, band, false);
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = saved_fill;
}

fn CGContextStrokeEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    // Approximate with stroking the bounding rect for now.
    log_dbg!("CGContextStrokeEllipseInRect: approximated as stroke rect");
    CGContextStrokeRect(env, context, rect);
}

pub fn CGContextFillEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if context.is_null() {
        return;
    }
    log_dbg!("CGContextFillEllipseInRect: approximated as fill rect");
    cg_bitmap_context::fill_rect(env, context, rect, false);
}

// MARK: - Path construction and rasterisation

fn append_path_element(env: &mut Environment, context: CGContextRef, element: super::cg_path::PathElement) {
    use super::cg_path::PathElement::*;
    if context.is_null() { return; }
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    let t = host.transform;
    let element = match element {
        MoveTo(p) => MoveTo(t.apply_to_point(p)),
        LineTo(p) => LineTo(t.apply_to_point(p)),
        QuadCurveTo { control, to } => QuadCurveTo { control: t.apply_to_point(control), to: t.apply_to_point(to) },
        CurveTo { c1, c2, to } => CurveTo { c1: t.apply_to_point(c1), c2: t.apply_to_point(c2), to: t.apply_to_point(to) },
        Close => Close,
    };
    let finite = |p: CGPoint| p.x.is_finite() && p.y.is_finite();
    let valid = match &element {
        MoveTo(p) | LineTo(p) => finite(*p),
        QuadCurveTo { control, to } => finite(*control) && finite(*to),
        CurveTo { c1, c2, to } => finite(*c1) && finite(*c2) && finite(*to),
        Close => true,
    };
    if valid { host.path_elements.push(element); }
}

pub fn CGContextAddPath(env: &mut Environment, context: CGContextRef, path: super::cg_path::CGPathRef) {
    if context.is_null() || path.is_null() { return; }
    let elements = env.objc.borrow::<super::cg_path::CGPathHostObject>(path).elements.clone();
    for element in elements { append_path_element(env, context, element); }
}
fn CGContextBeginPath(env: &mut Environment, context: CGContextRef) {
    if !context.is_null() { env.objc.borrow_mut::<CGContextHostObject>(context).path_elements.clear(); }
}
fn CGContextMoveToPoint(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    append_path_element(env, context, super::cg_path::PathElement::MoveTo(CGPoint { x, y }));
}
fn CGContextAddLineToPoint(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    append_path_element(env, context, super::cg_path::PathElement::LineTo(CGPoint { x, y }));
}
/// `void CGContextStrokeLineSegments(CGContextRef c, const CGPoint points[],
///     size_t count)` — each consecutive pair of points is an independent
/// segment. Build them as one path and stroke once.
fn CGContextStrokeLineSegments(
    env: &mut Environment,
    context: CGContextRef,
    points: ConstPtr<CGPoint>,
    count: usize,
) {
    if context.is_null() || points.is_null() || count < 2 {
        return;
    }
    CGContextBeginPath(env, context);
    let mut i = 0usize;
    while i + 1 < count {
        let a = env.mem.read(points + i as u32);
        let b = env.mem.read(points + (i + 1) as u32);
        CGContextMoveToPoint(env, context, a.x, a.y);
        CGContextAddLineToPoint(env, context, b.x, b.y);
        i += 2;
    }
    CGContextStrokePath(env, context);
}

fn CGContextAddLines(env: &mut Environment, context: CGContextRef, points: ConstPtr<CGPoint>, count: usize) {
    for i in 0..count as u32 {
        let p = env.mem.read(points + i);
        if i == 0 { CGContextMoveToPoint(env, context, p.x, p.y); }
        else { CGContextAddLineToPoint(env, context, p.x, p.y); }
    }
}
fn CGContextClosePath(env: &mut Environment, context: CGContextRef) {
    append_path_element(env, context, super::cg_path::PathElement::Close);
}
fn CGContextAddRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let (x,y,w,h)=(rect.origin.x,rect.origin.y,rect.size.width,rect.size.height);
    CGContextMoveToPoint(env,context,x,y);
    CGContextAddLineToPoint(env,context,x+w,y);
    CGContextAddLineToPoint(env,context,x+w,y+h);
    CGContextAddLineToPoint(env,context,x,y+h);
    CGContextClosePath(env,context);
}
fn CGContextAddRects(env: &mut Environment, context: CGContextRef, rects: ConstPtr<CGRect>, count: usize) {
    for i in 0..count as u32 { let rect=env.mem.read(rects+i); CGContextAddRect(env,context,rect); }
}
fn CGContextAddCurveToPoint(env: &mut Environment, context: CGContextRef, cp1x: CGFloat, cp1y: CGFloat, cp2x: CGFloat, cp2y: CGFloat, x: CGFloat, y: CGFloat) {
    append_path_element(env,context,super::cg_path::PathElement::CurveTo {
        c1: CGPoint{x:cp1x,y:cp1y}, c2: CGPoint{x:cp2x,y:cp2y}, to: CGPoint{x,y}
    });
}
fn CGContextAddQuadCurveToPoint(env: &mut Environment, context: CGContextRef, cpx: CGFloat, cpy: CGFloat, x: CGFloat, y: CGFloat) {
    append_path_element(env,context,super::cg_path::PathElement::QuadCurveTo { control:CGPoint{x:cpx,y:cpy}, to:CGPoint{x,y} });
}
fn CGContextAddEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let (cx,cy)=(rect.origin.x+rect.size.width*0.5,rect.origin.y+rect.size.height*0.5);
    let (rx,ry)=(rect.size.width*0.5,rect.size.height*0.5);
    let k=0.5522848;
    CGContextMoveToPoint(env,context,cx+rx,cy);
    CGContextAddCurveToPoint(env,context,cx+rx,cy+k*ry,cx+k*rx,cy+ry,cx,cy+ry);
    CGContextAddCurveToPoint(env,context,cx-k*rx,cy+ry,cx-rx,cy+k*ry,cx-rx,cy);
    CGContextAddCurveToPoint(env,context,cx-rx,cy-k*ry,cx-k*rx,cy-ry,cx,cy-ry);
    CGContextAddCurveToPoint(env,context,cx+k*rx,cy-ry,cx+rx,cy-k*ry,cx+rx,cy);
    CGContextClosePath(env,context);
}
fn CGContextAddArc(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat, radius: CGFloat, start_angle: CGFloat, end_angle: CGFloat, clockwise: i32) {
    if context.is_null() || radius < 0.0 || ![x,y,radius,start_angle,end_angle].iter().all(|v|v.is_finite()) { return; }
    let tau=std::f32::consts::TAU;
    let raw=end_angle-start_angle;
    let sweep=if raw.abs()>=tau { if clockwise!=0 {-tau} else {tau} }
        else if clockwise!=0 { -(-raw).rem_euclid(tau) } else {raw.rem_euclid(tau)};
    let p=CGPoint{x:x+radius*start_angle.cos(),y:y+radius*start_angle.sin()};
    if env.objc.borrow::<CGContextHostObject>(context).path_elements.is_empty() { CGContextMoveToPoint(env,context,p.x,p.y); }
    else { CGContextAddLineToPoint(env,context,p.x,p.y); }
    let steps=(sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as u32;
    for i in 0..steps {
        let a=start_angle+sweep*i as f32/steps as f32;
        let b=start_angle+sweep*(i+1) as f32/steps as f32;
        let k=4.0/3.0*((b-a)/4.0).tan();
        CGContextAddCurveToPoint(env,context,
            x+radius*(a.cos()-k*a.sin()),y+radius*(a.sin()+k*a.cos()),
            x+radius*(b.cos()+k*b.sin()),y+radius*(b.sin()-k*b.cos()),
            x+radius*b.cos(),y+radius*b.sin());
    }
}
fn CGContextAddArcToPoint(env: &mut Environment, context: CGContextRef, x1: CGFloat, y1: CGFloat, x2: CGFloat, y2: CGFloat, radius: CGFloat) {
    if context.is_null() || radius < 0.0 { return; }
    let h=env.objc.borrow::<CGContextHostObject>(context);
    let contours=super::path_geometry::flatten(&h.path_elements);
    let Some(contour)=contours.last() else { CGContextMoveToPoint(env,context,x1,y1); return; };
    let p=if contour.closed {contour.points.first()} else {contour.points.last()};
    let Some(&p)=p else {return;};
    let p=h.transform.invert().apply_to_point(p);
    let (ux,uy)=(p.x-x1,p.y-y1); let (vx,vy)=(x2-x1,y2-y1);
    let (ul,vl)=((ux*ux+uy*uy).sqrt(),(vx*vx+vy*vy).sqrt());
    if ul==0.0 || vl==0.0 || radius==0.0 { CGContextAddLineToPoint(env,context,x1,y1); return; }
    let (ux,uy,vx,vy)=(ux/ul,uy/ul,vx/vl,vy/vl);
    let cross=ux*vy-uy*vx;
    let angle=(ux*vx+uy*vy).clamp(-1.0,1.0).acos();
    if cross.abs()<1e-6 { CGContextAddLineToPoint(env,context,x1,y1); return; }
    let d=radius/(angle*0.5).tan();
    let t1=CGPoint{x:x1+ux*d,y:y1+uy*d};
    let t2=CGPoint{x:x1+vx*d,y:y1+vy*d};
    let sign=cross.signum();
    let center=CGPoint{x:t1.x-uy*radius*sign,y:t1.y+ux*radius*sign};
    CGContextAddArc(env,context,center.x,center.y,radius,
        (t1.y-center.y).atan2(t1.x-center.x),(t2.y-center.y).atan2(t2.x-center.x),if cross>0.0 {1} else {0});
}

fn CGContextDrawPath(env: &mut Environment, context: CGContextRef, mode: i32) {
    if context.is_null() || !(0..=4).contains(&mode) { return; }
    let h=env.objc.borrow_mut::<CGContextHostObject>(context);
    let elements=std::mem::take(&mut h.path_elements);
    let fill=h.rgb_fill_color; let stroke=h.rgb_stroke_color; let alpha=h.alpha;
    let blend=h.blend_mode!=17; let cap=h.line_cap; let join=h.line_join; let miter=h.miter_limit;
    // Frobenius norm bounds the largest CTM stretch, including shears.
    let scale=(h.transform.a*h.transform.a+h.transform.b*h.transform.b
        +h.transform.c*h.transform.c+h.transform.d*h.transform.d).sqrt();
    let hairline = h.line_width == 0.0;
    let pen_width = if hairline { 1.0 } else { h.line_width.abs() };
    let width = if hairline { 1.0 } else { pen_width * scale };
    let inverse = if hairline { CGAffineTransformIdentity } else { h.transform.invert() };
    let contours=super::path_geometry::flatten(&elements);
    let stroke_contours: Vec<_> = contours.iter().map(|c| super::path_geometry::Contour {
        points: c.points.iter().map(|&p| inverse.apply_to_point(p)).collect(),
        closed: c.closed,
    }).collect();
    let all:Vec<CGPoint>=contours.iter().flat_map(|c|c.points.iter().copied()).filter(|p|p.x.is_finite()&&p.y.is_finite()).collect();
    if all.is_empty() {return;}
    let do_fill=matches!(mode,0|1|3|4); let do_stroke=matches!(mode,2|3|4);
    let pad=if do_stroke {width*0.5*miter.max(1.0)+1.0} else {1.0};
    let minx=all.iter().map(|p|p.x).fold(f32::INFINITY,f32::min)-pad;
    let miny=all.iter().map(|p|p.y).fold(f32::INFINITY,f32::min)-pad;
    let maxx=all.iter().map(|p|p.x).fold(f32::NEG_INFINITY,f32::max)+pad;
    let maxy=all.iter().map(|p|p.y).fold(f32::NEG_INFINITY,f32::max)+pad;
    let mut drawer=CGBitmapContextDrawer::new(&env.objc,&mut env.mem,context);
    for y in (miny.floor().max(0.0) as u32)..(maxy.ceil().min(drawer.height() as f32).max(0.0) as u32) {
        for x in (minx.floor().max(0.0) as u32)..(maxx.ceil().min(drawer.width() as f32).max(0.0) as u32) {
            let mut fc=0; let mut sc=0;
            for (ox,oy) in [(0.25,0.25),(0.75,0.25),(0.25,0.75),(0.75,0.75)] {
                let p=CGPoint{x:x as f32+ox,y:y as f32+oy};
                if do_fill && super::path_geometry::contains(&contours,p,matches!(mode,1|4)) {fc+=1;}
                if do_stroke && super::path_geometry::on_stroke(&stroke_contours,inverse.apply_to_point(p),pen_width,cap,join,miter) {sc+=1;}
            }
            if fc>0 {drawer.put_srgba_pixel((x as i32,y as i32),(fill.0,fill.1,fill.2,fill.3*alpha*fc as f32/4.0),blend);}
            if sc>0 {drawer.put_srgba_pixel((x as i32,y as i32),(stroke.0,stroke.1,stroke.2,stroke.3*alpha*sc as f32/4.0),blend);}
        }
    }
}
fn CGContextFillPath(env: &mut Environment, context: CGContextRef) { CGContextDrawPath(env,context,0); }
fn CGContextEOFillPath(env: &mut Environment, context: CGContextRef) { CGContextDrawPath(env,context,1); }
fn CGContextStrokePath(env: &mut Environment, context: CGContextRef) { CGContextDrawPath(env,context,2); }

// MARK: - Antialiasing / quality hints

fn CGContextSetShouldAntialias(_env: &mut Environment, _context: CGContextRef, _value: bool) {}
fn CGContextSetAllowsAntialiasing(_env: &mut Environment, _context: CGContextRef, _value: bool) {}
fn CGContextSetShouldSmoothFonts(_env: &mut Environment, _context: CGContextRef, _value: bool) {}

fn CGContextSetAllowsFontSmoothing(_env: &mut Environment, _context: CGContextRef, _value: bool) {}
fn CGContextSetShouldSubpixelPositionFonts(
    _env: &mut Environment,
    _context: CGContextRef,
    _value: bool,
) {
}
fn CGContextSetAllowsFontSubpixelQuantization(
    _env: &mut Environment,
    _context: CGContextRef,
    _value: bool,
) {
}

// MARK: - Flush / sync

fn CGContextFlush(_env: &mut Environment, _context: CGContextRef) {}
fn CGContextSynchronize(_env: &mut Environment, _context: CGContextRef) {}

// MARK: - Clipping

pub fn CGContextGetClipBoundingBox(env: &mut Environment, context: CGContextRef) -> CGRect {
    let w = CGBitmapContextGetWidth(env, context) as CGFloat;
    let h = CGBitmapContextGetHeight(env, context) as CGFloat;
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: super::CGSize {
            width: w,
            height: h,
        },
    }
}

fn CGContextResetClip(_env: &mut Environment, _context: CGContextRef) {
    log_dbg!("CGContextResetClip: stubbed");
}

fn CGContextClipToMask(
    _env: &mut Environment,
    _context: CGContextRef,
    _rect: CGRect,
    _mask: CGImageRef,
) {
    log!("CGContextClipToMask: stubbed");
}

fn CGContextSetGrayFillColor(
    env: &mut Environment,
    context: CGContextRef,
    gray: CGFloat,
    alpha: CGFloat,
) {
    let color = (gray, gray, gray, alpha);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = color;
}

pub fn CGContextFillRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    // ← opens function
    if context.is_null() {
        // ← opens if
        log!("Warning: CGContextFillRect called with null context, skipping");
        return;
    } // ← closes if
    cg_bitmap_context::fill_rect(env, context, rect, /* clear: */ false);
}

pub fn CGContextClearRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    cg_bitmap_context::fill_rect(env, context, rect, /* clear: */ true);
}

pub fn CGContextClipToRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if context.is_null() {
        return;
    }
    if rect.origin == CGPointZero
        && rect.size.height == CGBitmapContextGetHeight(env, context) as f32
        && rect.size.width == CGBitmapContextGetWidth(env, context) as f32
    {
        // The fast path: the rect already covers the whole context. As long as
        // the CTM is identity, this is a no-op. With a non-identity CTM the
        // rect actually represents a sub-region of the backing store, so
        // arbitrary clipping is required — we don't implement that yet, but
        // we no longer panic on it.
        let is_identity = env
            .objc
            .borrow_mut::<CGContextHostObject>(context)
            .transform
            .is_identity();
        if is_identity {
            return;
        }
        log_dbg!(
            "CGContextClipToRect({:?}): full-bounds rect with non-identity CTM; \
             clipping is not implemented, ignoring.",
            rect
        );
        return;
    }
    log_dbg!(
        "CGContextClipToRect({:?}) on context {:?}: arbitrary clipping is not implemented, \
         ignoring.",
        rect,
        context
    );
}

pub fn CGContextConcatCTM(
    env: &mut Environment,
    context: CGContextRef,
    transform: CGAffineTransform,
) {
    log_dbg!("CGContextConcatCTM({:?})", transform);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = transform.concat(host_obj.transform);
}
pub fn CGContextGetCTM(env: &mut Environment, context: CGContextRef) -> CGAffineTransform {
    let res = env.objc.borrow::<CGContextHostObject>(context).transform;
    log_dbg!("CGContextGetCTM() => {:?}", res);
    res
}
pub fn CGContextRotateCTM(env: &mut Environment, context: CGContextRef, angle: CGFloat) {
    log_dbg!("CGContextRotateCTM({:?})", angle);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.rotate(angle);
}
pub fn CGContextScaleCTM(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    log_dbg!("CGContextScaleCTM({:?})", (x, y));
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.scale(x, y);
}
pub fn CGContextTranslateCTM(
    env: &mut Environment,
    context: CGContextRef,
    tx: CGFloat,
    ty: CGFloat,
) {
    log_dbg!("CGContextTranslateCTM({:?})", (tx, ty));
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.translate(tx, ty);
}

pub fn CGContextDrawImage(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {
    // ← opens function
    if context.is_null() {
        // ← opens if
        log!("Warning: CGContextDrawImage called with null context, skipping");
        return;
    } // ← closes if
    cg_bitmap_context::draw_image(env, context, rect, image);
}

/// `void CGContextDrawTiledImage(CGContextRef c, CGRect rect, CGImageRef image)`
///
/// Apple documentation: draws an image repeatedly, tiling it across the entire
/// clipping region of the context. `rect` defines the origin (the tiling
/// phase) and the size of a single tile. We reproduce this faithfully by
/// computing the current clip bounding box and drawing the image into a grid
/// of tile-sized rectangles aligned to `rect.origin` until the whole clip
/// region is covered.
pub fn CGContextDrawTiledImage(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {
    if context.is_null() || image.is_null() {
        log!("Warning: CGContextDrawTiledImage called with null context/image, skipping");
        return;
    }

    let tile_w = rect.size.width;
    let tile_h = rect.size.height;
    if !(tile_w > 0.0) || !(tile_h > 0.0) {
        // A degenerate tile size would tile forever; nothing to draw.
        return;
    }

    // The region we must fill.
    let clip = CGContextGetClipBoundingBox(env, context);
    let clip_min_x = clip.origin.x;
    let clip_min_y = clip.origin.y;
    let clip_max_x = clip.origin.x + clip.size.width;
    let clip_max_y = clip.origin.y + clip.size.height;

    // Align the tile grid to `rect.origin` (the tiling phase): find the first
    // tile boundary at or before the clip's minimum corner on each axis.
    let start_x = rect.origin.x + ((clip_min_x - rect.origin.x) / tile_w).floor() * tile_w;
    let start_y = rect.origin.y + ((clip_min_y - rect.origin.y) / tile_h).floor() * tile_h;

    // Guard against pathological cases (e.g. a huge clip with a tiny tile)
    // producing an unbounded number of draw calls.
    const MAX_TILES: u64 = 1_000_000;
    let mut drawn: u64 = 0;

    let mut y = start_y;
    while y < clip_max_y {
        let mut x = start_x;
        while x < clip_max_x {
            let tile_rect = CGRect {
                origin: CGPoint { x, y },
                size: super::CGSize {
                    width: tile_w,
                    height: tile_h,
                },
            };
            cg_bitmap_context::draw_image(env, context, tile_rect, image);

            drawn += 1;
            if drawn >= MAX_TILES {
                log!(
                    "Warning: CGContextDrawTiledImage hit the {} tile cap; stopping early",
                    MAX_TILES
                );
                return;
            }

            x += tile_w;
        }
        y += tile_h;
    }
}

/// Solve |p - (c0 + t*(c1-c0))| = r0 + t*(r1-r0). The largest
/// admissible root is the last circle painted when the circles overlap.
fn radial_gradient_parameter(
    point: CGPoint,
    start: CGPoint,
    start_radius: CGFloat,
    end: CGPoint,
    end_radius: CGFloat,
    options: u32,
) -> Option<CGFloat> {
    let px = f64::from(point.x) - f64::from(start.x);
    let py = f64::from(point.y) - f64::from(start.y);
    let dx = f64::from(end.x) - f64::from(start.x);
    let dy = f64::from(end.y) - f64::from(start.y);
    let r = f64::from(start_radius);
    let dr = f64::from(end_radius) - r;
    let a = dx * dx + dy * dy - dr * dr;
    let b = -2.0 * (px * dx + py * dy + r * dr);
    let c = px * px + py * py - r * r;
    let roots = if a.abs() <= f64::EPSILON * (dx * dx + dy * dy + dr * dr).max(1.0) {
        if b == 0.0 { return None; }
        [-c / b, -c / b]
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 { return None; }
        // Stable quadratic formula avoids cancellation near either circle.
        let q = -0.5 * (b + discriminant.sqrt().copysign(b));
        if q == 0.0 { [0.0, 0.0] } else { [q / a, c / q] }
    };
    roots.into_iter()
        .filter(|t| t.is_finite() && r + t * dr >= 0.0)
        .filter(|t| (*t >= 0.0 || options & 1 != 0) && (*t <= 1.0 || options & 2 != 0))
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .map(|t| t.clamp(0.0, 1.0) as CGFloat)
}

/// Software radial shading in user space. Uses the existing bitmap context
/// blender; arbitrary path clipping and unsupported blend modes retain the
/// same limitations as the other software CGContext drawing operations.
#[allow(clippy::too_many_arguments)]
pub fn CGContextDrawRadialGradient(
    env: &mut Environment,
    context: CGContextRef,
    gradient: super::cg_gradient::CGGradientRef,
    start_center: CGPoint,
    start_radius: CGFloat,
    end_center: CGPoint,
    end_radius: CGFloat,
    options: u32,
) {
    if context.is_null() || gradient.is_null() { return; }
    if ![start_center.x, start_center.y, end_center.x, end_center.y,
        start_radius, end_radius].iter().all(|v| v.is_finite())
        || start_radius < 0.0 || end_radius < 0.0 {
        return;
    }
    let host = env.objc.borrow::<CGContextHostObject>(context);
    let transform = host.transform;
    let alpha = host.alpha;
    let blend = host.blend_mode != 17; // kCGBlendModeCopy
    let determinant = transform.a * transform.d - transform.b * transform.c;
    if !determinant.is_finite() || determinant == 0.0 { return; }
    let inverse = transform.invert();
    let sample = super::cg_gradient::color_sampler(env, gradient);
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    for y in 0..drawer.height() {
        for x in 0..drawer.width() {
            let point = inverse.apply_to_point(CGPoint { x: x as f32 + 0.5, y: y as f32 + 0.5 });
            if let Some(t) = radial_gradient_parameter(point, start_center, start_radius, end_center, end_radius, options) {
                let (r, g, b, a) = sample(t);
                drawer.put_srgba_pixel((x as i32, y as i32), (r, g, b, a * alpha), blend);
            }
        }
    }
}

pub fn CGContextDrawLinearGradient(
    _env: &mut Environment,
    _context: CGContextRef,
    _gradient: CFTypeRef, // CGGradientRef
    _start_point: CGPoint,
    _end_point: CGPoint,
    _options: u32,
) {
    // Stubbed: the previous implementation filled the clip bounding box with the
    // current fill color as an approximation, but this caused two problems for
    // Mirror's Edge iPad: (1) the tutorial overlay background is drawn with a
    // white fill color, producing an opaque white screen that hides all 3D
    // content, and (2) iterating every pixel of a 1024x768 software bitmap
    // context on the CPU caused a multi-second hang that Windows reported as
    // "Not Responding". True gradient rendering would require per-pixel color
    // interpolation using the CGGradientRef color stops; for now we skip the
    // draw entirely so overlays remain transparent and the game stays responsive.
    log_dbg!("CGContextDrawLinearGradient: stubbed (skipped)");
}

pub fn CGContextSaveGState(env: &mut Environment, context: CGContextRef) {
    if context.is_null() {
        return;
    }
    let h = env.objc.borrow::<CGContextHostObject>(context);
    let state = CGContextState {
        fill_color: h.rgb_fill_color,
        fill_color_space_model: h.fill_color_space_model,
        stroke_color: h.rgb_stroke_color,
        alpha: h.alpha,
        line_width: h.line_width,
        line_cap: h.line_cap,
        line_join: h.line_join,
        miter_limit: h.miter_limit,
        flatness: h.flatness,
        blend_mode: h.blend_mode,
        interpolation_quality: h.interpolation_quality,
        transform: h.transform,
        font: h.font,
        font_size: h.font_size,
        rendering_intent: h.rendering_intent,
        shadow: h.shadow,
    };
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .state_stack
        .push(state);
    // Retain the font we just stored in the saved state. It will be released
    // by CGContextRestoreGState (or whenever the matching state is popped).
    let font = env.objc.borrow::<CGContextHostObject>(context).font;
    CGFontRetain(env, font);
}

pub fn CGContextRestoreGState(env: &mut Environment, context: CGContextRef) {
    if context.is_null() {
        return;
    }
    // We need to release the _current_ font on the context before overwriting
    // it. There are 2 cases:
    // - font hasn't been set between save/restore: this release balances the
    //   font retain from save
    // - font has been set between save/restore: we need to release the
    //   font that was retained by CGContextSetFont
    let current_font = env.objc.borrow::<CGContextHostObject>(context).font;
    CGFontRelease(env, current_font);
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    if let Some(state) = host.state_stack.pop() {
        host.rgb_fill_color = state.fill_color;
        host.fill_color_space_model = state.fill_color_space_model;
        host.rgb_stroke_color = state.stroke_color;
        host.alpha = state.alpha;
        host.line_width = state.line_width;
        host.line_cap = state.line_cap;
        host.line_join = state.line_join;
        host.miter_limit = state.miter_limit;
        host.flatness = state.flatness;
        host.blend_mode = state.blend_mode;
        host.interpolation_quality = state.interpolation_quality;
        host.transform = state.transform;
        host.font = state.font;
        host.font_size = state.font_size;
        host.rendering_intent = state.rendering_intent;
        host.shadow = state.shadow;
    } else {
        log!("Warning: CGContextRestoreGState: stack underflow");
    }
}

fn CGContextSetInterpolationQuality(
    env: &mut Environment,
    context: CGContextRef,
    quality: CGInterpolationQuality,
) {
    if context.is_null() {
        return;
    }

    // Честно записываем качество в структуру контекста
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .interpolation_quality = quality;
}

fn CGContextGetTextPosition(_env: &mut Environment, _context: CGContextRef) -> CGPoint {
    CGPoint { x: 0.0, y: 0.0 }
}

fn CGContextSetTextPosition(
    _env: &mut Environment,
    _context: CGContextRef,
    _x: CGFloat,
    _y: CGFloat,
) {
}

fn CGContextSetTextDrawingMode(_env: &mut Environment, _context: CGContextRef, _mode: i32) {}

fn CGContextSetCharacterSpacing(_env: &mut Environment, _context: CGContextRef, _spacing: CGFloat) {
}

fn CGContextSetTextMatrix(
    env: &mut Environment,
    context: CGContextRef,
    transform: CGAffineTransform,
) {
    log_dbg!("CGContextSetTextMatrix({:?})", transform);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .text_transform = Some(transform);
}

pub fn CGContextGetTextMatrix(env: &mut Environment, context: CGContextRef) -> CGAffineTransform {
    if context.is_null() {
        return CGAffineTransformIdentity;
    }
    env.objc
        .borrow::<CGContextHostObject>(context)
        .text_transform
        .unwrap_or(CGAffineTransformIdentity)
}

fn CGContextSelectFont(
    _env: &mut Environment,
    _context: CGContextRef,
    _name: crate::mem::ConstPtr<u8>,
    _size: CGFloat,
    _encoding: i32,
) {
}

fn CGContextShowTextAtPoint(
    _env: &mut Environment,
    _context: CGContextRef,
    _x: CGFloat,
    _y: CGFloat,
    _string: crate::mem::ConstPtr<u8>,
    _length: u32,
) {
}

fn CGContextShowText(
    _env: &mut Environment,
    _context: CGContextRef,
    _string: crate::mem::ConstPtr<u8>,
    _length: u32,
) {
}

fn CGContextSetFontSize(env: &mut Environment, context: CGContextRef, size: CGFloat) {
    if context.is_null() {
        return;
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .font_size = size;
}

fn CGContextSetFont(env: &mut Environment, context: CGContextRef, font: CGFontRef) {
    if context.is_null() {
        return;
    }
    // On real iOS, UIFont and CGFont are toll-free bridged. In HyperHLE they are
    // separate types. Handle three cases:
    // 1. Already a _touchHLE_CGFont — use as-is.
    // 2. A live UIFont — wrap it in a _touchHLE_CGFont on the fly.
    // 3. A freed/nil-isa object (use-after-free) — substitute Liberation Sans
    //    so text is at least visible rather than silently dropped.
    let font = if font.is_null() {
        font
    } else if super::cg_font::is_data_provider_font(env, font) {
        // Case 1: already the right type.
        font
    } else if uikit::ui_font::is_uifont(env, font) {
        // Case 2: live UIFont — convert.
        if let Some(f) = uikit::ui_font::font_from_uifont(env, font) {
            let host_obj = Box::new(CGFontHostObject { font: f });
            let class = env.objc.get_known_class("_touchHLE_CGFont", &mut env.mem);
            env.objc.alloc_object(class, host_obj, &mut env.mem)
        } else {
            font
        }
    } else {
        // Case 3: unknown / freed object — use a bundled fallback font so
        // CGContextShowGlyphsAtPoint has something to render with.
        log_dbg!(
            "CGContextSetFont: unrecognised font {:?} (possibly freed UIFont); \
                  substituting Liberation Sans",
            font
        );
        let host_obj = Box::new(CGFontHostObject {
            font: crate::font::Font::sans_regular(),
        });
        let class = env.objc.get_known_class("_touchHLE_CGFont", &mut env.mem);
        env.objc.alloc_object(class, host_obj, &mut env.mem)
    };
    CGFontRetain(env, font);
    let old_font = env.objc.borrow_mut::<CGContextHostObject>(context).font;
    CGFontRelease(env, old_font);
    env.objc.borrow_mut::<CGContextHostObject>(context).font = font;
}

/// `void CGContextShowGlyphsAtPoint(CGContextRef c, CGFloat x, CGFloat y,
///                                  const CGGlyph *glyphs, size_t count)`
fn CGContextShowGlyphsAtPoint(
    env: &mut Environment,
    context: CGContextRef,
    x: CGFloat,
    y: CGFloat,
    glyphs: ConstPtr<CGGlyph>,
    count: GuestUSize,
) {
    if context.is_null() {
        return;
    }
    let font = env.objc.borrow::<CGContextHostObject>(context).font;
    if font.is_null() {
        log!("Warning: CGContextShowGlyphsAtPoint called with no font set");
        return;
    }
    if !super::cg_font::is_data_provider_font(env, font) {
        log!(
            "TODO: CGContextShowGlyphsAtPoint with non-data-provider font {:?}, skipping",
            font
        );
        return;
    }

    let mut glyph_ids = Vec::with_capacity(count as usize);
    for i in 0..count {
        let glyph_id: CGGlyph = env.mem.read(glyphs + i);
        glyph_ids.push(rusttype::GlyphId(glyph_id));
    }

    let host = env.objc.borrow::<CGContextHostObject>(context);
    let font_size = host.font_size;
    let text_transform = host.text_transform.unwrap_or(CGAffineTransformIdentity);

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();

    // Borrow the actual rusttype Font for rendering. We hold a separate
    // borrow on env.objc here because `drawer` has already taken a borrow
    // on env.mem and on the bitmap context's host object, but it does not
    // need the font host object.
    let rusttype_font = &env.objc.borrow::<CGFontHostObject>(font).font;
    rusttype_font.draw_glyphs(
        font_size,
        glyph_ids,
        (x, y),
        text_transform,
        |raster_glyph| {
            uikit::ui_font::draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                /* clip_x: */ None,
                /* clip_y: */ None,
            )
        },
    );
}

/// `void CGContextShowGlyphsAtPositions(CGContextRef c,
///     const CGGlyph *glyphs, const CGPoint *positions, size_t count)`
///
/// Draws glyphs at specified positions relative to the text position.
/// Each position gives the (x, y) offset for the corresponding glyph.
fn CGContextShowGlyphsAtPositions(
    env: &mut Environment,
    context: CGContextRef,
    glyphs: ConstPtr<CGGlyph>,
    positions: ConstPtr<CGPoint>,
    count: GuestUSize,
) {
    if context.is_null() {
        return;
    }
    let font = env.objc.borrow::<CGContextHostObject>(context).font;
    if font.is_null() {
        log!("Warning: CGContextShowGlyphsAtPositions called with no font set");
        return;
    }
    if !super::cg_font::is_data_provider_font(env, font) {
        log!(
            "TODO: CGContextShowGlyphsAtPositions with non-data-provider font {:?}, skipping",
            font
        );
        return;
    }

    let mut glyph_ids = Vec::with_capacity(count as usize);
    let mut pos_vec = Vec::with_capacity(count as usize);
    for i in 0..count {
        let glyph_id: CGGlyph = env.mem.read(glyphs + i);
        glyph_ids.push(rusttype::GlyphId(glyph_id));
        let pos: CGPoint = env.mem.read(positions + i);
        pos_vec.push((pos.x, pos.y));
    }

    let font_size = env.objc.borrow::<CGContextHostObject>(context).font_size;

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();

    let rusttype_font = &env.objc.borrow::<CGFontHostObject>(font).font;
    rusttype_font.draw_glyphs_at_positions(
        font_size,
        &glyph_ids,
        &pos_vec,
        (0.0, 0.0),
        |raster_glyph| {
            uikit::ui_font::draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                /* clip_x: */ None,
                /* clip_y: */ None,
            )
        },
    );
}

/// `void CGContextShowGlyphsWithAdvances(CGContextRef c,
///     const CGGlyph *glyphs, const CGSize *advances, size_t count)`
///
/// Draws glyphs with explicit advance widths/heights between each glyph.
fn CGContextShowGlyphsWithAdvances(
    env: &mut Environment,
    context: CGContextRef,
    glyphs: ConstPtr<CGGlyph>,
    advances: ConstPtr<super::CGSize>,
    count: GuestUSize,
) {
    if context.is_null() {
        return;
    }
    let font = env.objc.borrow::<CGContextHostObject>(context).font;
    if font.is_null() {
        log!("Warning: CGContextShowGlyphsWithAdvances called with no font set");
        return;
    }
    if !super::cg_font::is_data_provider_font(env, font) {
        log!(
            "TODO: CGContextShowGlyphsWithAdvances with non-data-provider font {:?}, skipping",
            font
        );
        return;
    }

    let mut glyph_ids = Vec::with_capacity(count as usize);
    let mut advance_vec = Vec::with_capacity(count as usize);
    for i in 0..count {
        let glyph_id: CGGlyph = env.mem.read(glyphs + i);
        glyph_ids.push(rusttype::GlyphId(glyph_id));
        let adv: super::CGSize = env.mem.read(advances + i);
        advance_vec.push((adv.width, adv.height));
    }

    let font_size = env.objc.borrow::<CGContextHostObject>(context).font_size;

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();

    let rusttype_font = &env.objc.borrow::<CGFontHostObject>(font).font;
    rusttype_font.draw_glyphs_with_advances(
        font_size,
        &glyph_ids,
        &advance_vec,
        (0.0, 0.0),
        |raster_glyph| {
            uikit::ui_font::draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                /* clip_x: */ None,
                /* clip_y: */ None,
            )
        },
    );
}

/// `void CGContextShowGlyphs(CGContextRef c, const CGGlyph *glyphs, size_t count)`
///
/// Draws glyphs at the current text position. Since we don't track text
/// position fully yet, we draw at (0, 0).
fn CGContextShowGlyphs(
    env: &mut Environment,
    context: CGContextRef,
    glyphs: ConstPtr<CGGlyph>,
    count: GuestUSize,
) {
    CGContextShowGlyphsAtPoint(env, context, 0.0, 0.0, glyphs, count);
}

/// `void CGContextSetAllowsFontSubpixelPositioning(CGContextRef c, bool allow)`
///
/// Controls whether subpixel font positioning is used.  On the emulated
/// screen there is no physical sub-pixel grid to exploit, so this is a
/// no-op.  Exporting the symbol eliminates the "unimplemented function"
/// warning produced by apps that call it unconditionally.
///
/// Reference: <https://developer.apple.com/documentation/coregraphics/1454839-cgcontextsetallowsfontsubpixelpo>
fn CGContextSetAllowsFontSubpixelPositioning(
    _env: &mut Environment,
    _context: CGContextRef,
    _allows: bool,
) {
}

/// `void CGContextSetShouldSubpixelQuantizeFonts(CGContextRef c, bool should)`
///
/// Controls whether font glyph metrics are quantised to sub-pixel
/// boundaries.  No-op for the same reasons as
/// `CGContextSetAllowsFontSubpixelPositioning`.
///
/// Reference: <https://developer.apple.com/documentation/coregraphics/1455671-cgcontextsetshouldsub pixelquanti>
fn CGContextSetShouldSubpixelQuantizeFonts(
    _env: &mut Environment,
    _context: CGContextRef,
    _should: bool,
) {
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGContextRetain(_)),
    export_c_func!(CGContextRelease(_)),
    export_c_func!(CGContextSetFillColorWithColor(_, _)),
    export_c_func!(CGContextSetFillColor(_, _)),
    export_c_func!(CGContextSetRGBFillColor(_, _, _, _, _)),
    export_c_func!(CGContextSetRGBStrokeColor(_, _, _, _, _)),
    export_c_func!(CGContextSetGrayFillColor(_, _, _)),
    export_c_func!(CGContextFillRect(_, _)),
    export_c_func!(CGContextClearRect(_, _)),
    export_c_func!(CGContextClipToRect(_, _)),
    export_c_func!(CGContextConcatCTM(_, _)),
    export_c_func!(CGContextGetCTM(_)),
    export_c_func!(CGContextRotateCTM(_, _)),
    export_c_func!(CGContextScaleCTM(_, _, _)),
    export_c_func!(CGContextTranslateCTM(_, _, _)),
    export_c_func!(CGContextDrawImage(_, _, _)),
    export_c_func!(CGContextDrawTiledImage(_, _, _)),
    export_c_func!(CGContextSaveGState(_)),
    export_c_func!(CGContextRestoreGState(_)),
    export_c_func!(CGContextSetInterpolationQuality(_, _)),
    export_c_func!(CGContextGetTextPosition(_)),
    export_c_func!(CGContextSetTextPosition(_, _, _)),
    export_c_func!(CGContextSetTextDrawingMode(_, _)),
    export_c_func!(CGContextSetCharacterSpacing(_, _)),
    export_c_func!(CGContextSetTextMatrix(_, _)),
    export_c_func!(CGContextGetTextMatrix(_)),
    export_c_func!(CGContextSelectFont(_, _, _, _)),
    export_c_func!(CGContextShowTextAtPoint(_, _, _, _, _)),
    export_c_func!(CGContextShowText(_, _, _)),
    export_c_func!(CGContextSetFontSize(_, _)),
    export_c_func!(CGContextSetFont(_, _)),
    export_c_func!(CGContextShowGlyphsAtPoint(_, _, _, _, _)),
    export_c_func!(CGContextShowGlyphsAtPositions(_, _, _, _)),
    export_c_func!(CGContextShowGlyphsWithAdvances(_, _, _, _)),
    export_c_func!(CGContextShowGlyphs(_, _, _)),
    // Add to FUNCTIONS:
    export_c_func!(CGContextSetStrokeColorWithColor(_, _)),
    export_c_func!(CGContextSetStrokeColor(_, _)),
    export_c_func!(CGContextSetGrayStrokeColor(_, _, _)),
    export_c_func!(CGContextSetAlpha(_, _)),
    export_c_func!(CGContextSetLineWidth(_, _)),
    export_c_func!(CGContextSetLineCap(_, _)),
    export_c_func!(CGContextSetLineJoin(_, _)),
    export_c_func!(CGContextSetMiterLimit(_, _)),
    export_c_func!(CGContextSetLineDash(_, _, _, _)),
    export_c_func!(CGContextSetFlatness(_, _)),
    export_c_func!(CGContextStrokeRect(_, _)),
    export_c_func!(CGContextStrokeRectWithWidth(_, _, _)),
    export_c_func!(CGContextStrokeEllipseInRect(_, _)),
    export_c_func!(CGContextFillEllipseInRect(_, _)),
    export_c_func!(CGContextAddRect(_, _)),
    export_c_func!(CGContextAddRects(_, _, _)),
    export_c_func!(CGContextAddEllipseInRect(_, _)),
    export_c_func!(CGContextAddArc(_, _, _, _, _, _, _)),
    export_c_func!(CGContextAddArcToPoint(_, _, _, _, _, _)),
    export_c_func!(CGContextAddLineToPoint(_, _, _)),
    export_c_func!(CGContextAddLines(_, _, _)),
    export_c_func!(CGContextStrokeLineSegments(_, _, _)),
    export_c_func!(CGContextMoveToPoint(_, _, _)),
    export_c_func!(CGContextAddCurveToPoint(_, _, _, _, _, _, _)),
    export_c_func!(CGContextAddQuadCurveToPoint(_, _, _, _, _)),
    export_c_func!(CGContextClosePath(_)),
    export_c_func!(CGContextBeginPath(_)),
    export_c_func!(CGContextAddPath(_, _)),
    export_c_func!(CGContextDrawPath(_, _)),
    export_c_func!(CGContextFillPath(_)),
    export_c_func!(CGContextEOFillPath(_)),
    export_c_func!(CGContextStrokePath(_)),
    export_c_func!(CGContextSetShouldAntialias(_, _)),
    export_c_func!(CGContextSetAllowsAntialiasing(_, _)),
    export_c_func!(CGContextSetShouldSmoothFonts(_, _)),
    export_c_func!(CGContextSetAllowsFontSmoothing(_, _)),
    export_c_func!(CGContextSetShouldSubpixelPositionFonts(_, _)),
    export_c_func!(CGContextSetAllowsFontSubpixelQuantization(_, _)),
    export_c_func!(CGContextFlush(_)),
    export_c_func!(CGContextSynchronize(_)),
    export_c_func!(CGContextGetClipBoundingBox(_)),
    export_c_func!(CGContextResetClip(_)),
    export_c_func!(CGContextClipToMask(_, _, _)),
    export_c_func!(CGContextSetBlendMode(_, _)),
    export_c_func!(CGContextSetShadow(_, _, _)),
    export_c_func!(CGContextSetShadowWithColor(_, _, _, _)),
    export_c_func!(CGContextSetFillColorSpace(_, _)),
    export_c_func!(CGContextSetStrokeColorSpace(_, _)),
    export_c_func!(CGContextSetRenderingIntent(_, _)),
    export_c_func!(CGContextDrawLinearGradient(_, _, _, _, _)),
    export_c_func!(CGContextDrawRadialGradient(_, _, _, _, _, _, _)),
    export_c_func!(CGContextSetAllowsFontSubpixelPositioning(_, _)),
    export_c_func!(CGContextSetShouldSubpixelQuantizeFonts(_, _)),
];

#[cfg(test)]
mod radial_gradient_tests {
    use super::*;

    fn concentric(x: f32, r0: f32, r1: f32, options: u32) -> Option<f32> {
        radial_gradient_parameter(CGPoint { x, y: 0.0 }, CGPointZero, r0, CGPointZero, r1, options)
    }

    #[test]
    fn concentric_and_reversed_radii() {
        assert_eq!(concentric(0.0, 0.0, 10.0, 0), Some(0.0));
        assert_eq!(concentric(5.0, 0.0, 10.0, 0), Some(0.5));
        assert_eq!(concentric(10.0, 0.0, 10.0, 0), Some(1.0));
        assert_eq!(concentric(2.5, 10.0, 0.0, 0), Some(0.75));
    }

    #[test]
    fn extension_flags_and_degenerate_circles() {
        assert_eq!(concentric(2.0, 4.0, 10.0, 0), None);
        assert_eq!(concentric(2.0, 4.0, 10.0, 1), Some(0.0));
        assert_eq!(concentric(12.0, 4.0, 10.0, 0), None);
        assert_eq!(concentric(12.0, 4.0, 10.0, 2), Some(1.0));
        assert_eq!(concentric(5.0, 5.0, 5.0, 3), None);
    }

    #[test]
    fn offset_centres_and_linear_case() {
        let end = CGPoint { x: 10.0, y: 0.0 };
        assert_eq!(radial_gradient_parameter(end, CGPointZero, 0.0, end, 10.0, 0), Some(0.5));
        assert_eq!(radial_gradient_parameter(CGPoint { x: 5.0, y: 9.0 }, CGPointZero, 2.0, end, 2.0, 0), None);
    }
}
