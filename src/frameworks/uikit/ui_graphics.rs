/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGraphics.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_context::{
    CGContextClearRect, CGContextClipToRect, CGContextFillRect, CGContextRef, CGContextRelease,
    CGContextRetain,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::mem::{MutVoidPtr, Ptr};
use crate::objc::{id, msg_class, nil};
use crate::Environment;

#[derive(Default)]
pub(super) struct State {
    pub(super) context_stack: Vec<CGContextRef>,
}

/// `kCGBlendModeClear`. It is the only blend mode that is honoured here: the
/// rect helpers clear instead of fill. Every other mode draws normally.
const kCGBlendModeClear: i32 = 16;

/// Largest offscreen bitmap we will allocate (256 MiB). A garbage size from
/// the guest must not exhaust host memory.
const MAX_IMAGE_BYTES: f64 = 256.0 * 1024.0 * 1024.0;

// MARK: - Context stack

pub fn UIGraphicsPushContext(env: &mut Environment, context: CGContextRef) {
    CGContextRetain(env, context);
    env.framework_state
        .uikit
        .ui_graphics
        .context_stack
        .push(context);
}

pub fn UIGraphicsPopContext(env: &mut Environment) {
    if let Some(context) = env.framework_state.uikit.ui_graphics.context_stack.pop() {
        CGContextRelease(env, context);
    } else {
        log!("Warning: UIGraphicsPopContext called on empty stack");
    }
}

pub fn UIGraphicsGetCurrentContext(env: &mut Environment) -> CGContextRef {
    env.framework_state
        .uikit
        .ui_graphics
        .context_stack
        .last()
        .copied()
        .unwrap_or(nil)
}

// MARK: - Image context

/// Begin an offscreen bitmap context sized `size` at `scale`.
/// A `scale` of 0.0 means the main screen scale, which is 1.0 here.
/// `opaque` is ignored: the bitmap always has an alpha channel.
fn UIGraphicsBeginImageContextWithOptions(
    env: &mut Environment,
    size: CGSize,
    _opaque: bool,
    scale: CGFloat,
) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let w = (size.width as f64 * scale as f64).ceil();
    let h = (size.height as f64 * scale as f64).ceil();

    // `!(x >= 1.0)` also rejects NaN. The byte cap keeps `width * 4` and
    // `bytes_per_row * height` far below u32::MAX, so no overflow is possible.
    if !(w >= 1.0 && h >= 1.0) || w * h * 4.0 > MAX_IMAGE_BYTES {
        log!(
            "Warning: UIGraphicsBeginImageContextWithOptions: unusable size ({}, {}), skipping",
            w,
            h
        );
        return;
    }
    let (width, height) = (w as u32, h as u32);
    let bytes_per_row = width * 4;

    // NULL data lets CGBitmapContextCreate allocate the buffer and own it, so
    // it is freed together with the context. Passing a buffer of our own
    // would leak it, because CG only frees buffers it allocated itself.
    let data: MutVoidPtr = Ptr::null();

    // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big = 0x00020008
    let context = CGBitmapContextCreate(
        env,
        data,
        width as _,
        height as _,
        8,
        bytes_per_row as _,
        nil, // device RGB
        0x0002_0008,
    );

    if context.is_null() {
        log!("Warning: UIGraphicsBeginImageContextWithOptions: failed to create context");
        return;
    }

    UIGraphicsPushContext(env, context);
    // CGBitmapContextCreate returns a +1 ref; the push retained it, so drop
    // the creation ref. The stack now holds the only reference.
    CGContextRelease(env, context);
}

fn UIGraphicsBeginImageContext(env: &mut Environment, size: CGSize) {
    UIGraphicsBeginImageContextWithOptions(env, size, false, 1.0);
}

/// Snapshot the current image context as an autoreleased UIImage. This does
/// not pop the context: `UIGraphicsEndImageContext` does that.
fn UIGraphicsGetImageFromCurrentImageContext(env: &mut Environment) -> id {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() {
        log!("Warning: UIGraphicsGetImageFromCurrentImageContext: no current context");
        return nil;
    }
    let cg_image = CGBitmapContextCreateImage(env, context);
    if cg_image.is_null() {
        log!("Warning: UIGraphicsGetImageFromCurrentImageContext: failed to create CGImage");
        return nil;
    }
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    crate::frameworks::core_graphics::cg_image::CGImageRelease(env, cg_image);
    ui_image
}

fn UIGraphicsEndImageContext(env: &mut Environment) {
    if env
        .framework_state
        .uikit
        .ui_graphics
        .context_stack
        .is_empty()
    {
        log!("Warning: UIGraphicsEndImageContext called on empty stack");
        return;
    }
    UIGraphicsPopContext(env);
}

// MARK: - PDF context (stubs: no PDF is ever produced)

fn UIGraphicsBeginPDFContextToFile(
    _env: &mut Environment,
    _path: id, // NSString*
    _bounds: CGRect,
    _document_info: id, // NSDictionary*
) -> bool {
    log_dbg!("UIGraphicsBeginPDFContextToFile: unsupported, returning false");
    false
}

fn UIGraphicsBeginPDFContextToData(
    _env: &mut Environment,
    _data: id, // NSMutableData*
    _bounds: CGRect,
    _document_info: id, // NSDictionary*
) {
    log_dbg!("UIGraphicsBeginPDFContextToData: unsupported");
}

fn UIGraphicsEndPDFContext(_env: &mut Environment) {
    log_dbg!("UIGraphicsEndPDFContext: unsupported");
}

fn UIGraphicsBeginPDFPage(_env: &mut Environment) {
    log_dbg!("UIGraphicsBeginPDFPage: unsupported");
}

fn UIGraphicsBeginPDFPageWithInfo(
    _env: &mut Environment,
    _bounds: CGRect,
    _page_info: id, // NSDictionary*
) {
    log_dbg!("UIGraphicsBeginPDFPageWithInfo: unsupported");
}

fn UIGraphicsGetPDFContextBounds(_env: &mut Environment) -> CGRect {
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    }
}

// Annotation calls: with no PDF being produced there is nothing to annotate.

fn UIGraphicsSetPDFContextURLForRect(
    _env: &mut Environment,
    _url: id, // NSURL*
    _rect: CGRect,
) {
    log_dbg!("UIGraphicsSetPDFContextURLForRect: unsupported");
}

fn UIGraphicsAddPDFContextDestinationAtPoint(
    _env: &mut Environment,
    _name: id, // NSString*
    _point: CGPoint,
) {
    log_dbg!("UIGraphicsAddPDFContextDestinationAtPoint: unsupported");
}

fn UIGraphicsSetPDFContextDestinationForRect(
    _env: &mut Environment,
    _name: id, // NSString*
    _rect: CGRect,
) {
    log_dbg!("UIGraphicsSetPDFContextDestinationForRect: unsupported");
}

// MARK: - Rect helpers

/// Fills, or clears for `kCGBlendModeClear`, using the current context.
fn paint_rect(env: &mut Environment, rect: CGRect, blend_mode: i32) {
    let ctx = UIGraphicsGetCurrentContext(env);
    if ctx.is_null() {
        return;
    }
    if blend_mode == kCGBlendModeClear {
        CGContextClearRect(env, ctx, rect);
    } else {
        CGContextFillRect(env, ctx, rect);
    }
}

/// The 1-point-wide bands that make up a frame drawn inside `rect`. The side
/// bands stop short of the corners so no pixel is painted twice (which would
/// show with a translucent colour). Empty for a rect with no area.
fn frame_bands(rect: CGRect) -> Vec<CGRect> {
    let (x, y) = (rect.origin.x, rect.origin.y);
    let (w, h) = (rect.size.width, rect.size.height);
    // `!(w > 0.0)` also rejects NaN.
    if !(w > 0.0 && h > 0.0) {
        return Vec::new();
    }
    let r = |x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat| CGRect {
        origin: CGPoint { x, y },
        size: CGSize {
            width: w,
            height: h,
        },
    };
    // Too thin to hold a hollow frame: paint the whole rect.
    if w <= 2.0 || h <= 2.0 {
        return vec![r(x, y, w, h)];
    }
    vec![
        r(x, y, w, 1.0),
        r(x, y + h - 1.0, w, 1.0),
        r(x, y + 1.0, 1.0, h - 2.0),
        r(x + w - 1.0, y + 1.0, 1.0, h - 2.0),
    ]
}

fn UIRectFill(env: &mut Environment, rect: CGRect) {
    paint_rect(env, rect, 0);
}

fn UIRectFillUsingBlendMode(
    env: &mut Environment,
    rect: CGRect,
    blend_mode: i32, // CGBlendMode
) {
    paint_rect(env, rect, blend_mode);
}

/// Draws a 1-point frame inside `rect` in the current fill colour.
fn UIRectFrame(env: &mut Environment, rect: CGRect) {
    for band in frame_bands(rect) {
        paint_rect(env, band, 0);
    }
}

fn UIRectFrameUsingBlendMode(
    env: &mut Environment,
    rect: CGRect,
    blend_mode: i32, // CGBlendMode
) {
    for band in frame_bands(rect) {
        paint_rect(env, band, blend_mode);
    }
}

fn UIRectClip(env: &mut Environment, rect: CGRect) {
    let ctx = UIGraphicsGetCurrentContext(env);
    if ctx.is_null() {
        return;
    }
    CGContextClipToRect(env, ctx, rect);
}

pub const FUNCTIONS: FunctionExports = &[
    // Context stack
    export_c_func!(UIGraphicsPushContext(_)),
    export_c_func!(UIGraphicsPopContext()),
    export_c_func!(UIGraphicsGetCurrentContext()),
    // Image context
    export_c_func!(UIGraphicsBeginImageContext(_)),
    export_c_func!(UIGraphicsBeginImageContextWithOptions(_, _, _)),
    export_c_func!(UIGraphicsGetImageFromCurrentImageContext()),
    export_c_func!(UIGraphicsEndImageContext()),
    // PDF context stubs
    export_c_func!(UIGraphicsBeginPDFContextToFile(_, _, _)),
    export_c_func!(UIGraphicsBeginPDFContextToData(_, _, _)),
    export_c_func!(UIGraphicsEndPDFContext()),
    export_c_func!(UIGraphicsBeginPDFPage()),
    export_c_func!(UIGraphicsBeginPDFPageWithInfo(_, _)),
    export_c_func!(UIGraphicsGetPDFContextBounds()),
    export_c_func!(UIGraphicsSetPDFContextURLForRect(_, _)),
    export_c_func!(UIGraphicsAddPDFContextDestinationAtPoint(_, _)),
    export_c_func!(UIGraphicsSetPDFContextDestinationForRect(_, _)),
    // Rect helpers
    export_c_func!(UIRectFill(_)),
    export_c_func!(UIRectFillUsingBlendMode(_, _)),
    export_c_func!(UIRectFrame(_)),
    export_c_func!(UIRectFrameUsingBlendMode(_, _)),
    export_c_func!(UIRectClip(_)),
];
