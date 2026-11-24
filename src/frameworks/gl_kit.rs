/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! GLKit.
//!
//! Apple documents `GLKView` as "a default implementation for views that
//! draw their content using OpenGL ES" and `GLKViewController` as "a view
//! controller that manages an OpenGL ES rendering loop":
//! <https://developer.apple.com/documentation/glkit/glkview>
//! <https://developer.apple.com/documentation/glkit/glkviewcontroller>
//!
//! GLKit owns the drawable buffers, sets the current context and viewport,
//! invokes update/draw callbacks, and presents through EAGL. Its animation
//! loop uses the same CADisplayLink/run-loop clock as other iOS frameworks.
//! Public contracts and callback precedence are also described in Apple's
//! SDK headers:
//! <https://github.com/xybp888/iOS-SDKs/tree/master/iPhoneOS9.3.sdk/System/Library/Frameworks/GLKit.framework/Headers>

use crate::abi::{impl_GuestRet_for_large_struct, GuestArg};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_graphics::{CGFloat, CGRect};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{NSInteger, NSTimeInterval};
use crate::frameworks::opengles::{glBindFramebuffer, glViewport, with_ctx_and_mem};
use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::gles::gles11_raw as gl;
use crate::mem::{ConstVoidPtr, MutPtr, SafeRead};
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, Class, ClassExports, HostObject, NSZonePtr, ObjC,
};
use crate::Environment;

// GLKMatrix4Identity — 4x4 identity matrix of 32-bit floats (16 × 4 = 64
// bytes). Layout matches Apple's `_GLKMatrix4 { float m[16]; }`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub(super) struct GLKMatrix4 {
    m: [f32; 16],
}
unsafe impl SafeRead for GLKMatrix4 {}
impl_GuestRet_for_large_struct!(GLKMatrix4);
impl GuestArg for GLKMatrix4 {
    const REG_COUNT: usize = 16;

    fn from_regs(regs: &[u32]) -> Self {
        GLKMatrix4 {
            m: std::array::from_fn(|index| f32::from_bits(regs[index])),
        }
    }

    fn to_regs(self, regs: &mut [u32]) {
        for (destination, value) in regs.iter_mut().zip(self.m) {
            *destination = value.to_bits();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub(super) struct GLKVector2 {
    pub(super) x: f32,
    pub(super) y: f32,
}

impl GuestArg for GLKVector2 {
    const REG_COUNT: usize = 2;

    fn from_regs(regs: &[u32]) -> Self {
        Self {
            x: f32::from_bits(regs[0]),
            y: f32::from_bits(regs[1]),
        }
    }

    fn to_regs(self, regs: &mut [u32]) {
        regs[0] = self.x.to_bits();
        regs[1] = self.y.to_bits();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub(super) struct GLKVector3 {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) z: f32,
}

impl GuestArg for GLKVector3 {
    const REG_COUNT: usize = 3;

    fn from_regs(regs: &[u32]) -> Self {
        Self {
            x: f32::from_bits(regs[0]),
            y: f32::from_bits(regs[1]),
            z: f32::from_bits(regs[2]),
        }
    }

    fn to_regs(self, regs: &mut [u32]) {
        regs[0] = self.x.to_bits();
        regs[1] = self.y.to_bits();
        regs[2] = self.z.to_bits();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub(super) struct GLKVector4 {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) z: f32,
    pub(super) w: f32,
}

impl GuestArg for GLKVector4 {
    const REG_COUNT: usize = 4;

    fn from_regs(regs: &[u32]) -> Self {
        Self {
            x: f32::from_bits(regs[0]),
            y: f32::from_bits(regs[1]),
            z: f32::from_bits(regs[2]),
            w: f32::from_bits(regs[3]),
        }
    }

    fn to_regs(self, regs: &mut [u32]) {
        regs[0] = self.x.to_bits();
        regs[1] = self.y.to_bits();
        regs[2] = self.z.to_bits();
        regs[3] = self.w.to_bits();
    }
}

fn matrix4_identity() -> GLKMatrix4 {
    GLKMatrix4 {
        m: [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ],
    }
}

fn glk_matrix4_identity(env: &mut Environment) -> ConstVoidPtr {
    env.mem
        .alloc_and_write(matrix4_identity())
        .cast_void()
        .cast_const()
}

fn invert_matrix4(matrix: GLKMatrix4) -> (GLKMatrix4, bool) {
    let mut augmented = [[0.0_f64; 8]; 4];
    for row in 0..4 {
        for column in 0..4 {
            augmented[row][column] = matrix.m[column * 4 + row] as f64;
        }
        augmented[row][row + 4] = 1.0;
    }

    for column in 0..4 {
        let mut pivot_row = column;
        for row in column + 1..4 {
            if augmented[row][column].abs() > augmented[pivot_row][column].abs() {
                pivot_row = row;
            }
        }
        let pivot = augmented[pivot_row][column];
        if !pivot.is_finite() || pivot == 0.0 {
            return (matrix4_identity(), false);
        }
        augmented.swap(column, pivot_row);
        for value in &mut augmented[column] {
            *value /= pivot;
        }
        for row in 0..4 {
            if row == column {
                continue;
            }
            let factor = augmented[row][column];
            for index in 0..8 {
                augmented[row][index] -= factor * augmented[column][index];
            }
        }
    }

    let mut inverse = GLKMatrix4 { m: [0.0; 16] };
    for row in 0..4 {
        for column in 0..4 {
            let value = augmented[row][column + 4] as f32;
            if !value.is_finite() {
                return (matrix4_identity(), false);
            }
            inverse.m[column * 4 + row] = value;
        }
    }
    (inverse, true)
}

#[allow(non_snake_case)]
pub fn GLKMatrix4Invert(
    env: &mut Environment,
    matrix: GLKMatrix4,
    is_invertible: MutPtr<u8>,
) -> GLKMatrix4 {
    static TRACE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (inverse, invertible) = invert_matrix4(matrix);
    if crate::env_flag_cached!("TOUCHHLE_TRACE_GLK")
        && TRACE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8
    {
        log!(
            "GLKMatrix4Invert input={:?} invertible={} output={:?}",
            matrix.m,
            invertible,
            inverse.m
        );
    }
    if !is_invertible.is_null() {
        env.mem.write(is_invertible, u8::from(invertible));
    }
    inverse
}

pub const CONSTANTS: ConstantExports = &[(
    "_GLKMatrix4Identity",
    HostConstant::Custom(glk_matrix4_identity),
)];

pub const FUNCTIONS: FunctionExports = &[export_c_func!(GLKMatrix4Invert(_, _))];

/// `GLKViewDrawableColorFormat` etc. are plain integer enums in GLKit's
/// headers (`GLKView.h`); `GLKViewDrawableColorFormatRGBA8888` (the
/// documented default for `drawableColorFormat`) is 0, as are the
/// "FormatNone" defaults of the depth/stencil/multisample properties.
type GLKViewDrawableFormat = NSInteger;

#[derive(Clone, Copy, Default)]
struct Drawable {
    framebuffer: u32,
    color: u32,
    multisample_framebuffer: u32,
    multisample_color: u32,
    depth_stencil: u32,
    width: i32,
    height: i32,
}

fn delete_drawable(env: &mut Environment, view: id) {
    let drawable = std::mem::take(&mut env.objc.borrow_mut::<GLKViewHostObject>(view).drawable);
    if drawable.framebuffer == 0 {
        return;
    }
    let context = env.objc.borrow::<GLKViewHostObject>(view).context;
    let previous: id = msg_class![env; EAGLContext currentContext];
    retain(env, previous);
    let _: bool = msg_class![env; EAGLContext setCurrentContext:context];
    with_ctx_and_mem(env, |gles, _| unsafe {
        let framebuffers = [drawable.framebuffer, drawable.multisample_framebuffer];
        let buffers = [
            drawable.color,
            drawable.multisample_color,
            drawable.depth_stencil,
        ];
        gles.DeleteFramebuffersOES(2, framebuffers.as_ptr());
        gles.DeleteRenderbuffersOES(3, buffers.as_ptr());
    });
    crate::frameworks::opengles::forget_renderbuffer_drawables(
        env,
        &[
            drawable.color,
            drawable.multisample_color,
            drawable.depth_stencil,
        ],
    );
    let _: bool = msg_class![env; EAGLContext setCurrentContext:previous];
    release(env, previous);
}

fn bind_drawable(env: &mut Environment, view: id) -> bool {
    let context = env.objc.borrow::<GLKViewHostObject>(view).context;
    if context == nil {
        return false;
    }
    let _: bool = msg_class![env; EAGLContext setCurrentContext:context];
    let bounds: CGRect = msg![env; view bounds];
    let scale: CGFloat = msg![env; view contentScaleFactor];
    let width = (bounds.size.width * scale).round() as i32;
    let height = (bounds.size.height * scale).round() as i32;
    if width <= 0 || height <= 0 {
        delete_drawable(env, view);
        return false;
    }
    let old = env.objc.borrow::<GLKViewHostObject>(view).drawable;
    if old.width != width || old.height != height {
        delete_drawable(env, view);
    }
    if env
        .objc
        .borrow::<GLKViewHostObject>(view)
        .drawable
        .framebuffer
        == 0
    {
        let host = env.objc.borrow::<GLKViewHostObject>(view);
        let (color, depth, stencil, multisample) = (
            host.drawable_color_format,
            host.drawable_depth_format,
            host.drawable_stencil_format,
            host.drawable_multisample,
        );
        let layer: id = msg![env; view layer];
        let format = get_static_str(
            env,
            match color {
                1 => "RGB565",
                2 => "SRGBA8",
                _ => "RGBA8",
            },
        );
        let key = get_static_str(env, "ColorFormat");
        let properties: id = msg_class![env; NSDictionary dictionaryWithObject:format forKey:key];
        () = msg![env; layer setDrawableProperties:properties];
        let mut drawable = Drawable::default();
        with_ctx_and_mem(env, |gles, _| unsafe {
            gles.GenFramebuffersOES(1, &mut drawable.framebuffer);
            gles.GenRenderbuffersOES(1, &mut drawable.color);
            gles.BindRenderbufferOES(gl::RENDERBUFFER_OES, drawable.color);
        });
        if drawable.framebuffer == 0 {
            return false;
        }
        env.objc.borrow_mut::<GLKViewHostObject>(view).drawable = drawable;
        let allocated: bool =
            msg![env; context renderbufferStorage:gl::RENDERBUFFER_OES fromDrawable:layer];
        if !allocated {
            delete_drawable(env, view);
            return false;
        }
        let factor = env.options.scale_hack.get() as i32;
        with_ctx_and_mem(env, |gles, _| unsafe {
            let mut pixel_width = 0;
            let mut pixel_height = 0;
            gles.GetRenderbufferParameterivOES(
                gl::RENDERBUFFER_OES,
                gl::RENDERBUFFER_WIDTH_OES,
                &mut pixel_width,
            );
            gles.GetRenderbufferParameterivOES(
                gl::RENDERBUFFER_OES,
                gl::RENDERBUFFER_HEIGHT_OES,
                &mut pixel_height,
            );
            drawable.width = pixel_width / factor;
            drawable.height = pixel_height / factor;
            gles.BindFramebufferOES(gl::FRAMEBUFFER_OES, drawable.framebuffer);
            gles.FramebufferRenderbufferOES(
                gl::FRAMEBUFFER_OES,
                gl::COLOR_ATTACHMENT0_OES,
                gl::RENDERBUFFER_OES,
                drawable.color,
            );
            if multisample != 0 {
                gles.GenFramebuffersOES(1, &mut drawable.multisample_framebuffer);
                gles.GenRenderbuffersOES(1, &mut drawable.multisample_color);
                gles.BindFramebufferOES(gl::FRAMEBUFFER_OES, drawable.multisample_framebuffer);
                gles.BindRenderbufferOES(gl::RENDERBUFFER_OES, drawable.multisample_color);
                let color_format = match color {
                    1 => 0x8d62,
                    2 => 0x8c43,
                    _ => gl::RGBA8_OES,
                };
                gles.RenderbufferStorageMultisampleAPPLE(
                    gl::RENDERBUFFER_OES,
                    4,
                    color_format,
                    pixel_width,
                    pixel_height,
                );
                gles.FramebufferRenderbufferOES(
                    gl::FRAMEBUFFER_OES,
                    gl::COLOR_ATTACHMENT0_OES,
                    gl::RENDERBUFFER_OES,
                    drawable.multisample_color,
                );
            }
            if depth != 0 || stencil != 0 {
                // Packed depth/stencil avoids unsupported separate attachments
                // on ES 2 drivers. GLKit may promote the requested depth size.
                let format = if stencil != 0 && depth != 0 {
                    0x88f0
                } else if stencil != 0 {
                    0x8d48
                } else if depth == 2 {
                    0x81a6
                } else {
                    gl::DEPTH_COMPONENT16_OES
                };
                gles.GenRenderbuffersOES(1, &mut drawable.depth_stencil);
                gles.BindRenderbufferOES(gl::RENDERBUFFER_OES, drawable.depth_stencil);
                if multisample != 0 {
                    gles.RenderbufferStorageMultisampleAPPLE(
                        gl::RENDERBUFFER_OES,
                        4,
                        format,
                        pixel_width,
                        pixel_height,
                    );
                } else {
                    gles.RenderbufferStorageOES(
                        gl::RENDERBUFFER_OES,
                        format,
                        pixel_width,
                        pixel_height,
                    );
                }
                if depth != 0 {
                    gles.FramebufferRenderbufferOES(
                        gl::FRAMEBUFFER_OES,
                        gl::DEPTH_ATTACHMENT_OES,
                        gl::RENDERBUFFER_OES,
                        drawable.depth_stencil,
                    );
                }
                if stencil != 0 {
                    gles.FramebufferRenderbufferOES(
                        gl::FRAMEBUFFER_OES,
                        gl::STENCIL_ATTACHMENT_OES,
                        gl::RENDERBUFFER_OES,
                        drawable.depth_stencil,
                    );
                }
            }
            let status = gles.CheckFramebufferStatusOES(gl::FRAMEBUFFER_OES);
            assert_eq!(
                status,
                gl::FRAMEBUFFER_COMPLETE_OES,
                "GLKit drawable framebuffer is incomplete"
            );
        });
        env.objc.borrow_mut::<GLKViewHostObject>(view).drawable = drawable;
        log!(
            "GLKView {:?} allocated drawable {}x{} depth={} stencil={} msaa={}",
            view,
            drawable.width,
            drawable.height,
            depth,
            stencil,
            multisample
        );
    }
    let drawable = env.objc.borrow::<GLKViewHostObject>(view).drawable;
    let framebuffer = if drawable.multisample_framebuffer != 0 {
        drawable.multisample_framebuffer
    } else {
        drawable.framebuffer
    };
    glBindFramebuffer(env, gl::FRAMEBUFFER_OES, framebuffer);
    true
}

fn resolve_drawable(env: &mut Environment, drawable: Drawable) {
    if drawable.multisample_framebuffer != 0 {
        glBindFramebuffer(env, 0x8ca8, drawable.multisample_framebuffer);
        glBindFramebuffer(env, 0x8ca9, drawable.framebuffer);
        with_ctx_and_mem(env, |gles, _| unsafe {
            gles.ResolveMultisampleFramebufferAPPLE()
        });
    }
    glBindFramebuffer(env, gl::FRAMEBUFFER_OES, drawable.framebuffer);
}

/// Choose the closest integral display interval, as specified in GLKViewController.h.
fn frame_interval(preferred: NSInteger) -> NSInteger {
    let preferred = if preferred <= 0 { 30 } else { preferred };
    (1..=60)
        .min_by_key(|interval| (60 / interval - preferred).abs())
        .unwrap()
}

#[derive(Default)]
struct DisplayLinkMessenger {
    controller: id,
}
impl HostObject for DisplayLinkMessenger {}

fn start_display_link(env: &mut Environment, controller: id) {
    let host = env.objc.borrow::<GLKViewControllerHostObject>(controller);
    if host.display_link != nil || !host.visible || host.paused {
        return;
    }
    // The display link retains its target. A messenger with a weak controller
    // reference avoids a controller -> link -> controller retain cycle.
    let messenger: id = msg_class![env; _touchHLE_GLKDisplayLinkMessenger new];
    env.objc
        .borrow_mut::<DisplayLinkMessenger>(messenger)
        .controller = controller;
    let selector = env
        .objc
        .lookup_selector("_touchHLE_glkDisplayLink:")
        .unwrap();
    let link: id = msg_class![env; CADisplayLink displayLinkWithTarget:messenger selector:selector];
    release(env, messenger);
    retain(env, link);
    env.objc
        .borrow_mut::<GLKViewControllerHostObject>(controller)
        .display_link = link;
    let interval = frame_interval(
        env.objc
            .borrow::<GLKViewControllerHostObject>(controller)
            .preferred_frames_per_second,
    );
    () = msg![env; link setFrameInterval:interval];
    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let mode = get_static_str(env, "NSRunLoopCommonModes");
    () = msg![env; link addToRunLoop:run_loop forMode:mode];
    let now = crate::frameworks::core_animation::CACurrentMediaTime(env);
    let host = env
        .objc
        .borrow_mut::<GLKViewControllerHostObject>(controller);
    host.first_resume.get_or_insert(now);
    host.last_resume = Some(now);
    log_dbg!(
        "GLKViewController {:?} started display link {:?}",
        controller,
        link
    );
}

fn stop_display_link(env: &mut Environment, controller: id) {
    let link = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<GLKViewControllerHostObject>(controller)
            .display_link,
        nil,
    );
    if link != nil {
        () = msg![env; link invalidate];
        release(env, link);
    }
}

fn configure_view(env: &mut Environment, controller: id, view: id) {
    if view == nil {
        return;
    }
    let class: Class = msg_class![env; GLKView class];
    let is_glk: bool = msg![env; view isKindOfClass:class];
    if is_glk {
        () = msg![env; view setEnableSetNeedsDisplay:false];
        let delegate: id = msg![env; view delegate];
        if delegate == nil {
            () = msg![env; view setDelegate:controller];
        }
    }
}

fn detach_view(env: &mut Environment, controller: id) {
    let loaded: bool = msg![env; controller isViewLoaded];
    if !loaded {
        return;
    }
    let view: id = msg![env; controller view];
    let class: Class = msg_class![env; GLKView class];
    let is_glk: bool = msg![env; view isKindOfClass:class];
    if is_glk {
        let delegate: id = msg![env; view delegate];
        if delegate == controller {
            () = msg![env; view setDelegate:nil];
        }
    }
}

struct GLKViewHostObject {
    superclass: UIViewHostObject,
    /// `EAGLContext*` — retained (documented as a strong property).
    context: id,
    /// `id<GLKViewDelegate>` — NOT retained. Apple documents the property
    /// as `unowned(unsafe)`.
    delegate: id,
    drawable_color_format: GLKViewDrawableFormat,
    drawable_depth_format: GLKViewDrawableFormat,
    drawable_stencil_format: GLKViewDrawableFormat,
    drawable_multisample: GLKViewDrawableFormat,
    enable_set_needs_display: bool,
    drawable: Drawable,
    drawing: bool,
}
impl_HostObject_with_superclass!(GLKViewHostObject);
impl Default for GLKViewHostObject {
    fn default() -> Self {
        GLKViewHostObject {
            superclass: Default::default(),
            context: nil,
            delegate: nil,
            // Defaults documented in GLKView.h: RGBA8888 color, no depth,
            // no stencil, no multisampling.
            drawable_color_format: 0,
            drawable_depth_format: 0,
            drawable_stencil_format: 0,
            drawable_multisample: 0,
            // Apple docs: "the default value is YES".
            enable_set_needs_display: true,
            drawable: Drawable::default(),
            drawing: false,
        }
    }
}

struct GLKViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// `id<GLKViewControllerDelegate>` — NOT retained
    /// (`unowned(unsafe)` per Apple docs).
    delegate: id,
    preferred_frames_per_second: NSInteger,
    pause_on_will_resign_active: bool,
    resume_on_did_become_active: bool,
    paused: bool,
    frames_displayed: NSInteger,
    display_link: id,
    visible: bool,
    observing: bool,
    first_resume: Option<f64>,
    last_resume: Option<f64>,
    last_update: Option<f64>,
    last_draw: Option<f64>,
    update_delta: f64,
    draw_delta: f64,
}
impl_HostObject_with_superclass!(GLKViewControllerHostObject);
impl Default for GLKViewControllerHostObject {
    fn default() -> Self {
        GLKViewControllerHostObject {
            superclass: Default::default(),
            delegate: nil,
            // Apple docs: "The default value is 30" frames per second.
            preferred_frames_per_second: 30,
            // Apple docs: both pause/resume-on-app-state flags default
            // to YES.
            pause_on_will_resign_active: true,
            resume_on_did_become_active: true,
            paused: false,
            frames_displayed: 0,
            display_link: nil,
            visible: false,
            observing: false,
            first_resume: None,
            last_resume: None,
            last_update: None,
            last_draw: None,
            update_delta: 0.0,
            draw_delta: 0.0,
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GLKView: UIView

+ (Class)layerClass { msg_class![env; CAEAGLLayer class] }

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GLKViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithFrame:(CGRect)frame context:(id)context {
    let this: id = msg![env; this initWithFrame:frame];
    () = msg![env; this setContext:context];
    this
}

- (id)context {
    env.objc.borrow::<GLKViewHostObject>(this).context
}
- (())setContext:(id)context { // EAGLContext*
    let old = env.objc.borrow::<GLKViewHostObject>(this).context;
    if context == old { return; }
    delete_drawable(env, this);
    if context != nil {
        retain(env, context);
    }
    env.objc.borrow_mut::<GLKViewHostObject>(this).context = context;
    if old != nil {
        release(env, old);
    }
}

- (id)delegate {
    env.objc.borrow::<GLKViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    // Apple documents this property as unowned(unsafe): do not retain.
    env.objc.borrow_mut::<GLKViewHostObject>(this).delegate = delegate;
}

- (GLKViewDrawableFormat)drawableColorFormat {
    env.objc.borrow::<GLKViewHostObject>(this).drawable_color_format
}
- (())setDrawableColorFormat:(GLKViewDrawableFormat)format {
    if env.objc.borrow::<GLKViewHostObject>(this).drawable_color_format != format {
        delete_drawable(env, this);
        env.objc.borrow_mut::<GLKViewHostObject>(this).drawable_color_format = format;
    }
}
- (GLKViewDrawableFormat)drawableDepthFormat {
    env.objc.borrow::<GLKViewHostObject>(this).drawable_depth_format
}
- (())setDrawableDepthFormat:(GLKViewDrawableFormat)format {
    if env.objc.borrow::<GLKViewHostObject>(this).drawable_depth_format != format {
        delete_drawable(env, this);
        env.objc.borrow_mut::<GLKViewHostObject>(this).drawable_depth_format = format;
    }
}
- (GLKViewDrawableFormat)drawableStencilFormat {
    env.objc.borrow::<GLKViewHostObject>(this).drawable_stencil_format
}
- (())setDrawableStencilFormat:(GLKViewDrawableFormat)format {
    if env.objc.borrow::<GLKViewHostObject>(this).drawable_stencil_format != format {
        delete_drawable(env, this);
        env.objc.borrow_mut::<GLKViewHostObject>(this).drawable_stencil_format = format;
    }
}
- (GLKViewDrawableFormat)drawableMultisample {
    env.objc.borrow::<GLKViewHostObject>(this).drawable_multisample
}
- (())setDrawableMultisample:(GLKViewDrawableFormat)format {
    if env.objc.borrow::<GLKViewHostObject>(this).drawable_multisample != format {
        delete_drawable(env, this);
        env.objc.borrow_mut::<GLKViewHostObject>(this).drawable_multisample = format;
    }
}

- (NSInteger)drawableWidth {
    bind_drawable(env, this);
    env.objc.borrow::<GLKViewHostObject>(this).drawable.width
}
- (NSInteger)drawableHeight {
    bind_drawable(env, this);
    env.objc.borrow::<GLKViewHostObject>(this).drawable.height
}

- (bool)enableSetNeedsDisplay {
    env.objc.borrow::<GLKViewHostObject>(this).enable_set_needs_display
}
- (())setEnableSetNeedsDisplay:(bool)value {
    env.objc.borrow_mut::<GLKViewHostObject>(this).enable_set_needs_display = value;
}

- (())bindDrawable { bind_drawable(env, this); }
- (())deleteDrawable { delete_drawable(env, this); }

- (())setNeedsDisplay {
    if env.objc.borrow::<GLKViewHostObject>(this).enable_set_needs_display {
        let layer: id = msg![env; this layer];
        () = msg![env; layer setNeedsDisplay];
    }
}
- (())setNeedsDisplayInRect:(CGRect)_rect { () = msg![env; this setNeedsDisplay]; }
- (())displayLayer:(id)_layer {
    if env.objc.borrow::<GLKViewHostObject>(this).enable_set_needs_display {
        () = msg![env; this display];
    }
}

- (())display {
    if env.objc.borrow::<GLKViewHostObject>(this).drawing || !bind_drawable(env, this) { return; }
    retain(env, this);
    env.objc.borrow_mut::<GLKViewHostObject>(this).drawing = true;
    let bounds: CGRect = msg![env; this bounds];
    let drawable = env.objc.borrow::<GLKViewHostObject>(this).drawable;
    glViewport(env, 0, 0, drawable.width, drawable.height);
    // A subclass drawRect: takes precedence over the delegate (GLKView.h).
    let class = ObjC::read_isa(this, &env.mem);
    let base = env.objc.get_known_class("UIView", &mut env.mem);
    let selector = env.objc.lookup_selector("drawRect:").unwrap();
    let delegate = env.objc.borrow::<GLKViewHostObject>(this).delegate;
    if env.objc.class_overrides_method_of_superclass(class, selector, base) {
        () = msg![env; this drawRect:bounds];
    } else if delegate != nil {
        () = msg![env; delegate glkView:this drawInRect:bounds];
    }
    // Drawing code may bind another FBO or context. Present our color buffer.
    let context = env.objc.borrow::<GLKViewHostObject>(this).context;
    let _: bool = msg_class![env; EAGLContext setCurrentContext:context];
    let drawable = env.objc.borrow::<GLKViewHostObject>(this).drawable;
    if drawable.framebuffer != 0 {
        resolve_drawable(env, drawable);
        with_ctx_and_mem(env, |gles, _| unsafe { gles.BindRenderbufferOES(gl::RENDERBUFFER_OES, drawable.color) });
        let _: bool = msg![env; context presentRenderbuffer:gl::RENDERBUFFER_OES];
    }
    env.objc.borrow_mut::<GLKViewHostObject>(this).drawing = false;
    release(env, this);
}

- (id)snapshot {
    assert!(!env.objc.borrow::<GLKViewHostObject>(this).drawing, "GLKView snapshot during drawing");
    let previous: id = msg_class![env; EAGLContext currentContext];
    retain(env, previous);
    () = msg![env; this display];
    if !bind_drawable(env, this) {
        let _: bool = msg_class![env; EAGLContext setCurrentContext:previous];
        release(env, previous);
        return nil;
    }
    let drawable = env.objc.borrow::<GLKViewHostObject>(this).drawable;
    resolve_drawable(env, drawable);
    let factor = env.options.scale_hack.get() as i32;
    let (width, height) = (drawable.width * factor, drawable.height * factor);
    let mut pixels = vec![0; width as usize * height as usize * 4];
    let context = env.objc.borrow::<GLKViewHostObject>(this).context;
    let api: u32 = msg![env; context API];
    with_ctx_and_mem(env, |gles, _| unsafe {
        let mut alignment = 0;
        gles.GetIntegerv(gl::PACK_ALIGNMENT, &mut alignment);
        gles.PixelStorei(gl::PACK_ALIGNMENT, 1);
        // ReadPixels takes a byte offset when an ES 3 pixel-pack buffer is
        // bound. Reset row/skip state too so the snapshot fits this buffer.
        let mut pack_buffer = 0;
        let mut pack_layout = [0; 3];
        let layout_keys = [0x0d02, 0x0d03, 0x0d04];
        if api == 3 {
            gles.GetIntegerv(0x88ed, &mut pack_buffer);
            gles.BindBuffer(0x88eb, 0);
            for (key, value) in layout_keys.iter().zip(&mut pack_layout) {
                gles.GetIntegerv(*key, value);
                gles.PixelStorei(*key, 0);
            }
        }
        gles.ReadPixels(0, 0, width, height, gl::RGBA, gl::UNSIGNED_BYTE, pixels.as_mut_ptr().cast());
        if api == 3 {
            for (key, value) in layout_keys.iter().zip(pack_layout) { gles.PixelStorei(*key, value); }
            gles.BindBuffer(0x88eb, pack_buffer as u32);
        }
        gles.PixelStorei(gl::PACK_ALIGNMENT, alignment);
    });
    let stride = width as usize * 4;
    for y in 0..height as usize / 2 {
        for x in 0..stride { pixels.swap(y * stride + x, (height as usize - 1 - y) * stride + x); }
    }
    let cg_image = crate::frameworks::core_graphics::cg_image::from_image(env, crate::image::Image::from_pixels(width as u32, height as u32, pixels));
    let scale: CGFloat = msg![env; this contentScaleFactor];
    let image: id = msg_class![env; UIImage imageWithCGImage:cg_image scale:(scale * factor as CGFloat) orientation:0i32];
    crate::frameworks::core_graphics::cg_image::CGImageRelease(env, cg_image);
    let _: bool = msg_class![env; EAGLContext setCurrentContext:previous];
    release(env, previous);
    image
}

- (())dealloc {
    delete_drawable(env, this);
    let context = env.objc.borrow::<GLKViewHostObject>(this).context;
    release(env, context);
    () = msg_super![env; this dealloc];
}

@end

@implementation GLKViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GLKViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)delegate {
    env.objc.borrow::<GLKViewControllerHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    // Apple documents this property as unowned(unsafe): do not retain.
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).delegate = delegate;
}

- (NSInteger)preferredFramesPerSecond {
    env.objc.borrow::<GLKViewControllerHostObject>(this).preferred_frames_per_second
}
- (())setPreferredFramesPerSecond:(NSInteger)fps {
    // Apple docs: setting 0 (or a negative value) means "use the default",
    // which is 30 fps.
    let fps = if fps <= 0 { 30 } else { fps };
    env.objc
        .borrow_mut::<GLKViewControllerHostObject>(this)
        .preferred_frames_per_second = fps;
    let link = env.objc.borrow::<GLKViewControllerHostObject>(this).display_link;
    if link != nil { () = msg![env; link setFrameInterval:frame_interval(fps)]; }
}
- (NSInteger)framesPerSecond {
    60 / frame_interval(env.objc.borrow::<GLKViewControllerHostObject>(this).preferred_frames_per_second)
}

- (bool)isPaused {
    env.objc.borrow::<GLKViewControllerHostObject>(this).paused
}
- (())setPaused:(bool)paused {
    let host = env.objc.borrow_mut::<GLKViewControllerHostObject>(this);
    let was_paused = host.paused;
    host.paused = paused;
    if was_paused == paused {
        return;
    }
    if paused { stop_display_link(env, this); } else { start_display_link(env, this); }
    // Apple docs (GLKViewControllerDelegate): the delegate receives
    // glkViewController:willPause: when the pause state changes.
    let delegate = env.objc.borrow::<GLKViewControllerHostObject>(this).delegate;
    if delegate != nil {
        if let Some(sel) = env.objc.lookup_selector("glkViewController:willPause:") {
            let responds: bool = msg![env; delegate respondsToSelector:sel];
            if responds {
                () = msg![env; delegate glkViewController:this willPause:paused];
            }
        }
    }
}

- (bool)pauseOnWillResignActive {
    env.objc.borrow::<GLKViewControllerHostObject>(this).pause_on_will_resign_active
}
- (())setPauseOnWillResignActive:(bool)value {
    env.objc
        .borrow_mut::<GLKViewControllerHostObject>(this)
        .pause_on_will_resign_active = value;
}
- (bool)resumeOnDidBecomeActive {
    env.objc.borrow::<GLKViewControllerHostObject>(this).resume_on_did_become_active
}
- (())setResumeOnDidBecomeActive:(bool)value {
    env.objc
        .borrow_mut::<GLKViewControllerHostObject>(this)
        .resume_on_did_become_active = value;
}

- (NSInteger)framesDisplayed {
    env.objc.borrow::<GLKViewControllerHostObject>(this).frames_displayed
}
- (NSTimeInterval)timeSinceLastUpdate { env.objc.borrow::<GLKViewControllerHostObject>(this).update_delta }
- (NSTimeInterval)timeSinceLastDraw { env.objc.borrow::<GLKViewControllerHostObject>(this).draw_delta }
- (NSTimeInterval)timeSinceFirstResume {
    let now = crate::frameworks::core_animation::CACurrentMediaTime(env);
    env.objc.borrow::<GLKViewControllerHostObject>(this).first_resume.map_or(0.0, |start| now - start)
}
- (NSTimeInterval)timeSinceLastResume {
    let now = crate::frameworks::core_animation::CACurrentMediaTime(env);
    env.objc.borrow::<GLKViewControllerHostObject>(this).last_resume.map_or(0.0, |start| now - start)
}

- (())setView:(id)view {
    detach_view(env, this);
    () = msg_super![env; this setView:view];
    configure_view(env, this, view);
}
- (())loadView {
    () = msg_super![env; this loadView];
    let view: id = msg![env; this view];
    let class: Class = msg_class![env; GLKView class];
    let is_glk: bool = msg![env; view isKindOfClass:class];
    if !is_glk {
        let frame: CGRect = msg![env; view frame];
        let view: id = msg_class![env; GLKView alloc];
        let view: id = msg![env; view initWithFrame:frame];
        () = msg![env; this setView:view];
        release(env, view);
    }
}
- (())viewDidLoad {
    () = msg_super![env; this viewDidLoad];
    let view: id = msg![env; this view];
    configure_view(env, this, view);
    if !env.objc.borrow::<GLKViewControllerHostObject>(this).observing {
        env.objc.borrow_mut::<GLKViewControllerHostObject>(this).observing = true;
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let pause = env.objc.lookup_selector("_touchHLE_glkResignActive:").unwrap();
        let resume = env.objc.lookup_selector("_touchHLE_glkBecomeActive:").unwrap();
        let pause_name = get_static_str(env, "UIApplicationWillResignActiveNotification");
        let resume_name = get_static_str(env, "UIApplicationDidBecomeActiveNotification");
        () = msg![env; center addObserver:this selector:pause name:pause_name object:nil];
        () = msg![env; center addObserver:this selector:resume name:resume_name object:nil];
    }
}
- (())viewWillAppear:(bool)animated {
    () = msg_super![env; this viewWillAppear:animated];
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).visible = true;
    () = msg![env; this setPaused:false];
    start_display_link(env, this);
}
- (())viewWillDisappear:(bool)animated {
    () = msg_super![env; this viewWillDisappear:animated];
    env.objc.borrow_mut::<GLKViewControllerHostObject>(this).visible = false;
    () = msg![env; this setPaused:true];
}
- (())viewDidUnload {
    stop_display_link(env, this);
    detach_view(env, this);
    () = msg_super![env; this viewDidUnload];
}
- (())_touchHLE_glkResignActive:(id)_notification {
    if env.objc.borrow::<GLKViewControllerHostObject>(this).pause_on_will_resign_active {
        () = msg![env; this setPaused:true];
    }
    let view: id = msg![env; this view];
    () = msg![env; view deleteDrawable];
}
- (())_touchHLE_glkBecomeActive:(id)_notification {
    if env.objc.borrow::<GLKViewControllerHostObject>(this).resume_on_did_become_active {
        () = msg![env; this setPaused:false];
    }
}
- (())_touchHLE_glkUpdateAndDraw {
    let host = env.objc.borrow::<GLKViewControllerHostObject>(this);
    if host.paused || !host.visible { return; }
    retain(env, this);
    let now = crate::frameworks::core_animation::CACurrentMediaTime(env);
    let host = env.objc.borrow_mut::<GLKViewControllerHostObject>(this);
    host.update_delta = host.last_update.map_or(0.0, |last| now - last);
    host.last_update = Some(now);
    let delegate = host.delegate;
    let class = ObjC::read_isa(this, &env.mem);
    if env.objc.class_has_method_named(class, "update") {
        () = msg![env; this update];
    } else if delegate != nil {
        () = msg![env; delegate glkViewControllerUpdate:this];
    }
    // update may hide the controller or pause the loop.
    if !env.objc.borrow::<GLKViewControllerHostObject>(this).paused {
        let view: id = msg![env; this view];
        let now = crate::frameworks::core_animation::CACurrentMediaTime(env);
        let host = env.objc.borrow_mut::<GLKViewControllerHostObject>(this);
        host.draw_delta = host.last_draw.map_or(0.0, |last| now - last);
        host.last_draw = Some(now);
        () = msg![env; view display];
        env.objc.borrow_mut::<GLKViewControllerHostObject>(this).frames_displayed += 1;
        log_once!("GLKViewController delivered its first update/draw frame");
    }
    release(env, this);
}
- (())glkView:(id)_view drawInRect:(CGRect)_rect { }
- (())dealloc {
    stop_display_link(env, this);
    detach_view(env, this);
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    () = msg![env; center removeObserver:this];
    () = msg_super![env; this dealloc];
}

@end

@implementation _touchHLE_GLKDisplayLinkMessenger: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<DisplayLinkMessenger>::default(), &mut env.mem)
}
- (())_touchHLE_glkDisplayLink:(id)_link {
    let controller = env.objc.borrow::<DisplayLinkMessenger>(this).controller;
    () = msg![env; controller _touchHLE_glkUpdateAndDraw];
}
@end

};

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/GLKit.framework/GLKit",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};

#[cfg(test)]
mod tests {
    use super::{invert_matrix4, matrix4_identity, GLKMatrix4};

    #[test]
    fn glk_matrix4_invert_handles_identity_and_translation() {
        let matrix = GLKMatrix4 {
            m: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 4.0, -5.0, 6.0, 1.0,
            ],
        };
        let (inverse, invertible) = invert_matrix4(matrix);
        assert!(invertible);
        assert_eq!(
            inverse.m,
            [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -4.0, 5.0, -6.0, 1.0,]
        );
    }

    #[test]
    fn glk_matrix4_invert_reports_singular_matrices() {
        let matrix = GLKMatrix4 { m: [0.0; 16] };
        let (inverse, invertible) = invert_matrix4(matrix);
        assert!(!invertible);
        assert_eq!(inverse, matrix4_identity());
    }
}
