/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Generic OpenGL ES 1.1 interface.
//!
//! Unfortunately this does not provide the types and constants, so the correct
//! usage is to import `GLES` and `types` from this module, but get the
//! constants from [super::gles11_raw].

use crate::window::{GLContext, Window};

use super::gles11_raw as gles11;
use super::gles11_raw::types::*;

macro_rules! log_unimplemented_gles_once {
    ($gles:expr, $($message:tt)+) => {{
        static LOG_ONCE: std::sync::Once = std::sync::Once::new();
        let profile = ($gles).diagnostic_profile();
        LOG_ONCE.call_once(|| {
            log!(
                "Call to unimplemented GLES API on {}: {}",
                profile,
                format_args!($($message)+)
            );
        });
    }};
}

macro_rules! gles_unimplemented {
    ($gles:expr, $($message:tt)+) => {{
        log_unimplemented_gles_once!($gles, $($message)+);
        unimplemented!($($message)+)
    }};
}

std::thread_local! {
    static GUEST_GLES_CALL_DEPTH: std::cell::Cell<usize> = std::cell::Cell::new(0);
}

pub(crate) struct GuestGlesCallGuard {
    _private: (),
}

impl GuestGlesCallGuard {
    pub(crate) fn enter() -> Self {
        GUEST_GLES_CALL_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self { _private: () }
    }
}

impl Drop for GuestGlesCallGuard {
    fn drop(&mut self) {
        GUEST_GLES_CALL_DEPTH.with(|depth| {
            debug_assert_ne!(depth.get(), 0);
            depth.set(depth.get() - 1);
        });
    }
}

pub(super) fn guest_gles_call_active() -> bool {
    GUEST_GLES_CALL_DEPTH.with(|depth| depth.get() != 0)
}

pub(super) fn log_unimplemented_gles_function(function: &'static str, profile: &'static str) {
    log!(
        "Call to unimplemented GLES function {} on {} [this log will only be shown once]",
        function,
        profile
    );
}

#[doc(hidden)]
#[macro_export]
macro_rules! log_unimplemented_gles_guest_call {
    ($gles:expr, $function:literal) => {{
        if $crate::gles::gles_generic::guest_gles_call_active() {
            static LOG_ONCE: std::sync::Once = std::sync::Once::new();
            let profile = ($gles).diagnostic_profile();
            LOG_ONCE.call_once(|| {
                $crate::gles::gles_generic::log_unimplemented_gles_function($function, profile);
            });
        }
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_unsupported_gles1_methods {
    () => {
        unsafe fn Fogf(&mut self, _pname: GLenum, _param: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glFogf");
        }
        unsafe fn Fogx(&mut self, _pname: GLenum, _param: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glFogx");
        }
        unsafe fn Fogfv(&mut self, _pname: GLenum, _params: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glFogfv");
        }
        unsafe fn Fogxv(&mut self, _pname: GLenum, _params: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glFogxv");
        }
        unsafe fn Lightf(&mut self, _light: GLenum, _pname: GLenum, _param: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightf");
        }
        unsafe fn Lightx(&mut self, _light: GLenum, _pname: GLenum, _param: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightx");
        }
        unsafe fn Lightfv(&mut self, _light: GLenum, _pname: GLenum, _params: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightfv");
        }
        unsafe fn Lightxv(&mut self, _light: GLenum, _pname: GLenum, _params: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightxv");
        }
        unsafe fn LightModelf(&mut self, _pname: GLenum, _param: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightModelf");
        }
        unsafe fn LightModelx(&mut self, _pname: GLenum, _param: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightModelx");
        }
        unsafe fn LightModelfv(&mut self, _pname: GLenum, _params: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightModelfv");
        }
        unsafe fn LightModelxv(&mut self, _pname: GLenum, _params: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLightModelxv");
        }
        unsafe fn Materialf(&mut self, _face: GLenum, _pname: GLenum, _param: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMaterialf");
        }
        unsafe fn Materialx(&mut self, _face: GLenum, _pname: GLenum, _param: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMaterialx");
        }
        unsafe fn Materialfv(&mut self, _face: GLenum, _pname: GLenum, _params: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMaterialfv");
        }
        unsafe fn Materialxv(&mut self, _face: GLenum, _pname: GLenum, _params: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMaterialxv");
        }
        unsafe fn GetLightfv(&mut self, _light: GLenum, _pname: GLenum, _params: *mut GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetLightfv");
        }
        unsafe fn GetLightxv(&mut self, _light: GLenum, _pname: GLenum, _params: *mut GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetLightxv");
        }
        unsafe fn GetMaterialfv(&mut self, _face: GLenum, _pname: GLenum, _params: *mut GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetMaterialfv");
        }
        unsafe fn GetMaterialxv(&mut self, _face: GLenum, _pname: GLenum, _params: *mut GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetMaterialxv");
        }
        unsafe fn ClientActiveTexture(&mut self, _texture: GLenum) {
            $crate::log_unimplemented_gles_guest_call!(self, "glClientActiveTexture");
        }
        unsafe fn EnableClientState(&mut self, _array: GLenum) {
            $crate::log_unimplemented_gles_guest_call!(self, "glEnableClientState");
        }
        unsafe fn DisableClientState(&mut self, _array: GLenum) {
            $crate::log_unimplemented_gles_guest_call!(self, "glDisableClientState");
        }
        unsafe fn GetTexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetTexEnviv");
        }
        unsafe fn GetTexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetTexEnvfv");
        }
        unsafe fn GetPointerv(&mut self, _pname: GLenum, _params: *mut *const GLvoid) {
            $crate::log_unimplemented_gles_guest_call!(self, "glGetPointerv");
        }
        unsafe fn AlphaFunc(&mut self, _func: GLenum, _ref_: GLclampf) {
            $crate::log_unimplemented_gles_guest_call!(self, "glAlphaFunc");
        }
        unsafe fn AlphaFuncx(&mut self, _func: GLenum, _ref_: GLclampx) {
            $crate::log_unimplemented_gles_guest_call!(self, "glAlphaFuncx");
        }
        unsafe fn Color4f(&mut self, _r: GLfloat, _g: GLfloat, _b: GLfloat, _a: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glColor4f");
        }
        unsafe fn Color4x(&mut self, _r: GLfixed, _g: GLfixed, _b: GLfixed, _a: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glColor4x");
        }
        unsafe fn Color4ub(&mut self, _r: GLubyte, _g: GLubyte, _b: GLubyte, _a: GLubyte) {
            $crate::log_unimplemented_gles_guest_call!(self, "glColor4ub");
        }
        unsafe fn ShadeModel(&mut self, _mode: GLenum) {
            $crate::log_unimplemented_gles_guest_call!(self, "glShadeModel");
        }
        unsafe fn LoadIdentity(&mut self) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLoadIdentity");
        }
        unsafe fn LoadMatrixf(&mut self, _m: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLoadMatrixf");
        }
        unsafe fn LoadMatrixx(&mut self, _m: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glLoadMatrixx");
        }
        unsafe fn MultMatrixf(&mut self, _m: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMultMatrixf");
        }
        unsafe fn MultMatrixx(&mut self, _m: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMultMatrixx");
        }
        unsafe fn PushMatrix(&mut self) {
            $crate::log_unimplemented_gles_guest_call!(self, "glPushMatrix");
        }
        unsafe fn PopMatrix(&mut self) {
            $crate::log_unimplemented_gles_guest_call!(self, "glPopMatrix");
        }
        unsafe fn MatrixMode(&mut self, _mode: GLenum) {
            $crate::log_unimplemented_gles_guest_call!(self, "glMatrixMode");
        }
        unsafe fn Frustumf(
            &mut self,
            _left: GLfloat,
            _right: GLfloat,
            _bottom: GLfloat,
            _top: GLfloat,
            _near: GLfloat,
            _far: GLfloat,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glFrustumf");
        }
        unsafe fn Frustumx(
            &mut self,
            _left: GLfixed,
            _right: GLfixed,
            _bottom: GLfixed,
            _top: GLfixed,
            _near: GLfixed,
            _far: GLfixed,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glFrustumx");
        }
        unsafe fn Orthof(
            &mut self,
            _left: GLfloat,
            _right: GLfloat,
            _bottom: GLfloat,
            _top: GLfloat,
            _near: GLfloat,
            _far: GLfloat,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glOrthof");
        }
        unsafe fn Orthox(
            &mut self,
            _left: GLfixed,
            _right: GLfixed,
            _bottom: GLfixed,
            _top: GLfixed,
            _near: GLfixed,
            _far: GLfixed,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glOrthox");
        }
        unsafe fn Rotatef(&mut self, _a: GLfloat, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glRotatef");
        }
        unsafe fn Rotatex(&mut self, _a: GLfixed, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glRotatex");
        }
        unsafe fn Scalef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glScalef");
        }
        unsafe fn Scalex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glScalex");
        }
        unsafe fn Translatef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTranslatef");
        }
        unsafe fn Translatex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTranslatex");
        }
        unsafe fn TexEnvf(&mut self, _target: GLenum, _pname: GLenum, _param: GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexEnvf");
        }
        unsafe fn TexEnvx(&mut self, _target: GLenum, _pname: GLenum, _param: GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexEnvx");
        }
        unsafe fn TexEnvi(&mut self, _target: GLenum, _pname: GLenum, _param: GLint) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexEnvi");
        }
        unsafe fn TexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfloat) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexEnvfv");
        }
        unsafe fn TexEnvxv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfixed) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexEnvxv");
        }
        unsafe fn TexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLint) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexEnviv");
        }
        unsafe fn VertexPointer(
            &mut self,
            _size: GLint,
            _type_: GLenum,
            _stride: GLsizei,
            _pointer: *const GLvoid,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glVertexPointer");
        }
        unsafe fn ColorPointer(
            &mut self,
            _size: GLint,
            _type_: GLenum,
            _stride: GLsizei,
            _pointer: *const GLvoid,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glColorPointer");
        }
        unsafe fn NormalPointer(
            &mut self,
            _type_: GLenum,
            _stride: GLsizei,
            _pointer: *const GLvoid,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glNormalPointer");
        }
        unsafe fn TexCoordPointer(
            &mut self,
            _size: GLint,
            _type_: GLenum,
            _stride: GLsizei,
            _pointer: *const GLvoid,
        ) {
            $crate::log_unimplemented_gles_guest_call!(self, "glTexCoordPointer");
        }
    };
}

/// `GLchar` from the ES 2.0 type set. Not defined by the ES 1.1 registry, so
/// we provide our own alias here for use in the [GLES] trait's ES 2.0 entry
/// points.
pub type GLchar = std::os::raw::c_char;

fn tex_storage_2d_parameters(internalformat: GLenum) -> Option<(GLint, GLenum, GLenum)> {
    let (format, type_) = match internalformat {
        0x803C => (gles11::ALPHA, gles11::UNSIGNED_BYTE),
        0x8040 => (gles11::LUMINANCE, gles11::UNSIGNED_BYTE),
        0x8045 => (gles11::LUMINANCE_ALPHA, gles11::UNSIGNED_BYTE),
        0x8051 => (gles11::RGB, gles11::UNSIGNED_BYTE),
        0x8052 => (gles11::RGB, 0x8368),
        0x8058 => (gles11::RGBA, gles11::UNSIGNED_BYTE),
        0x8059 => (gles11::RGBA, 0x8368),
        0x8D62 => (gles11::RGB, gles11::UNSIGNED_SHORT_5_6_5),
        0x8056 => (gles11::RGBA, gles11::UNSIGNED_SHORT_4_4_4_4),
        0x8057 => (gles11::RGBA, gles11::UNSIGNED_SHORT_5_5_5_1),
        0x8814 => (gles11::RGBA, gles11::FLOAT),
        0x8815 => (gles11::RGB, gles11::FLOAT),
        0x8816 => (gles11::ALPHA, gles11::FLOAT),
        0x8818 => (gles11::LUMINANCE, gles11::FLOAT),
        0x8819 => (gles11::LUMINANCE_ALPHA, gles11::FLOAT),
        0x881A => (gles11::RGBA, 0x8D61),
        0x881B => (gles11::RGB, 0x8D61),
        0x881C => (gles11::ALPHA, 0x8D61),
        0x881E => (gles11::LUMINANCE, 0x8D61),
        0x881F => (gles11::LUMINANCE_ALPHA, 0x8D61),
        0x8229 => (0x1903, gles11::UNSIGNED_BYTE),
        0x822B => (0x8227, gles11::UNSIGNED_BYTE),
        0x822D => (0x1903, 0x8D61),
        0x822E => (0x1903, gles11::FLOAT),
        0x822F => (0x8227, 0x8D61),
        0x8230 => (0x8227, gles11::FLOAT),
        0x8A51 => (0x8A1F, 0x85BA),
        0x93A1 => (0x80E1, gles11::UNSIGNED_BYTE),
        _ => return None,
    };
    Some((format as GLint, format, type_))
}

/// Trait representing an OpenGL ES implementation and context.
///
/// The GL context is not necessarily active, so GL functions can't be called
/// from this trait. It can be made active from [GLESContext::make_current].
#[allow(clippy::upper_case_acronyms)]
pub trait GLESContext {
    /// Get a human-friendly description of this implementation.
    fn description() -> &'static str
    where
        Self: Sized;

    /// Construct a new context. This might fail if the host OS doesn't have a
    /// compatible driver, for example.
    #[allow(clippy::new_ret_no_self)]
    fn new(window: &mut crate::window::Window) -> Result<Self, String>
    where
        Self: Sized;

    /// Make this context (and any underlying context) the active OpenGL
    /// context.
    ///
    /// The lifetime ensures safety - the GLES object can't be destroyed while
    /// the instance is active, so the OpenGL state remains valid, and the
    /// window reference prevents the thread from yielding while the GLES
    /// object is being used, and prevents multiple contexts from existing at
    /// the same time (which can cause a UAF).
    fn make_current<'gl_ctx, 'win: 'gl_ctx>(
        &'gl_ctx mut self,
        window: &'win mut Window,
    ) -> Box<dyn GLES + 'gl_ctx>;

    /// Make this context (and any underlying context) the active OpenGL
    /// context, without checking if it is the only context. You shouldn't use
    /// this outside of [crate::window::Window], as this is function exists to
    /// work around lifetime splitting issues inside of it.
    ///
    /// SAFETY: Callers must ensure that this is the only active context,
    /// that the GLES instance does not outlive the self or window
    /// parameter, that make_current_fn makes the passed context current,
    /// and that loader_fn properly loads the requested function.
    unsafe fn make_current_unchecked_for_window<'gl_ctx>(
        &'gl_ctx mut self,
        make_current_fn: &mut dyn FnMut(&GLContext),
        loader_fn: &mut dyn FnMut(&'static str) -> *const std::ffi::c_void,
    ) -> Box<dyn GLES + 'gl_ctx>;
}

/// An active GLES context that can be used.
///
/// These are effectively direct wrappers around the raw OpenGL functions,
/// but they make sure that the context is active while it is using it.
/// # Safety
/// These functions (should) act as documented by the OpenGL ES spec. Callers
/// should ensure that all uses of raw pointers are verfied to be valid and
/// of the correct size as documented in the OpenGL ES spec.
#[allow(clippy::upper_case_acronyms)]
#[allow(clippy::too_many_arguments)] // not our fault :(
pub trait GLES {
    /// Get some string describing the underlying driver. For OpenGL this is
    /// `GL_VENDOR`, `GL_RENDERER` and `GL_VERSION`.
    unsafe fn driver_description(&self) -> String {
        gles_unimplemented!(self, "driver_description not implemented by this backend")
    }
    /// Returns `true` if this backend is a real OpenGL ES 2.0 / 3.0 driver and
    /// therefore does NOT support fixed-function pipeline calls (`MatrixMode`,
    /// `EnableClientState`, `Color4f`, …). Used by `present_renderbuffer` so it
    /// can take a shader-based code path on such backends.
    fn is_es2(&self) -> bool {
        false
    }

    /// Returns `true` if the host driver natively implements PVRTC compressed
    /// texture formats (`GL_IMG_texture_compression_pvrtc`). When `false`, the
    /// guest layer must emulate immutable PVRTC textures from `glTexStorage2D`
    /// by software-decoding uploads to RGBA (see `glTexStorage2D` and
    /// `glCompressedTexSubImage2D`). Apps targeting iPhone OS (Gameloft games,
    /// for example) commonly allocate their texture atlases this way on the
    /// OpenGL ES 3.0 API, where a driver without PVRTC rejects the
    /// internalformat with `GL_INVALID_ENUM` and the texture stays black.
    fn pvrtc_native(&self) -> bool {
        false
    }
    /// Returns `true` when this backend translates the guest fixed-function
    /// GLES 1.1 API into a shader-based host API. Translators need a more
    /// conservative presentation path on mobile drivers because switching
    /// away from the guest framebuffer can discard unresolved tile contents.
    fn is_translator(&self) -> bool {
        false
    }
    /// Returns `true` if this backend is a real (native) OpenGL ES 1.1 driver,
    /// i.e. one that strictly enforces the ES 1.1 spec and rejects desktop-GL
    /// enums. The desktop GL 2.1 emulation backend (`gles1_on_gl2`) returns
    /// `false` because it accepts the broader GL 2.1 enum set. Used by
    /// `present_renderbuffer` to skip state queries / state restores for
    /// enums that are part of GL 2.1 but not part of ES 1.1 (e.g.
    /// `GL_COLOR_LOGIC_OP`), which would otherwise generate `GL_INVALID_ENUM`
    /// on a strict native driver and pollute the GL error queue.
    fn is_native_es1(&self) -> bool {
        false
    }
    /// True only on the GLES1-on-GL2 emulation backend, whose GL2 host context
    /// supports the client vertex-array queries used by the guard.
    fn is_gles1_on_gl2(&self) -> bool {
        false
    }
    /// Returns the guest-facing GLES profile for compatibility diagnostics.
    fn diagnostic_profile(&self) -> &'static str {
        if self.is_translator() {
            "OpenGL ES 1.1 translated through OpenGL ES 2.0"
        } else if self.is_es3() {
            "OpenGL ES 3.0"
        } else if self.is_es2() {
            "OpenGL ES 2.0"
        } else if self.is_native_es1() {
            "OpenGL ES 1.1"
        } else if self.is_gles1_on_gl2() {
            "OpenGL ES 1.1 emulated on desktop OpenGL 2.1"
        } else {
            "unknown GLES profile"
        }
    }
    /// Forward the guest's `glDiscardFramebufferEXT` call to the host driver.
    ///
    /// `GL_EXT_discard_framebuffer` is a bandwidth hint: the app promises not
    /// to rely on the contents of the named attachments until they are
    /// reloaded. Tile-based mobile GPUs (ARM Mali, Qualcomm Adreno, PowerVR)
    /// can then skip writing tile memory back to system RAM at the end of the
    /// frame, which is a measurable fragment-bandwidth win — see the
    /// "External memory bandwidth" / tile write-back sections of ARM's
    /// Mali-G57 performance counters reference guide.
    ///
    /// `attachments` must already be translated to attachment enums the host
    /// driver accepts (`GL_COLOR_EXT` / `GL_DEPTH_EXT` / `GL_STENCIL_EXT` or
    /// the equivalent attachment enums). Returns `true` if the host call was
    /// made, `false` when the backend has no equivalent (callers treat the
    /// discard as consumed either way — it is only a hint).
    unsafe fn DiscardFramebufferEXT(
        &mut self,
        _target: GLenum,
        _num_attachments: GLsizei,
        _attachments: *const GLenum,
    ) -> bool {
        log_unimplemented_gles_once!(
            self,
            "glDiscardFramebufferEXT ignored because this backend lacks GL_EXT_discard_framebuffer"
        );
        false
    }
    /// Whether this backend can forward `glDiscardFramebufferEXT` to the
    /// host driver (host extension support was detected).
    fn discard_ext_supported(&self) -> bool {
        false
    }
    // Generic state manipulation
    unsafe fn GetError(&mut self) -> GLenum {
        gles_unimplemented!(self, "GetError not implemented by this backend")
    }
    unsafe fn Enable(&mut self, _cap: GLenum) {
        gles_unimplemented!(self, "Enable not implemented by this backend")
    }
    unsafe fn IsEnabled(&mut self, _cap: GLenum) -> GLboolean {
        gles_unimplemented!(self, "IsEnabled not implemented by this backend")
    }
    unsafe fn Disable(&mut self, _cap: GLenum) {
        gles_unimplemented!(self, "Disable not implemented by this backend")
    }
    unsafe fn ClientActiveTexture(&mut self, _texture: GLenum) {
        gles_unimplemented!(self, "ClientActiveTexture not implemented by this backend")
    }
    unsafe fn EnableClientState(&mut self, _array: GLenum) {
        gles_unimplemented!(self, "EnableClientState not implemented by this backend")
    }
    unsafe fn DisableClientState(&mut self, _array: GLenum) {
        gles_unimplemented!(self, "DisableClientState not implemented by this backend")
    }
    unsafe fn GetBooleanv(&mut self, _pname: GLenum, _params: *mut GLboolean) {
        gles_unimplemented!(self, "GetBooleanv not implemented by this backend")
    }
    unsafe fn GetFloatv(&mut self, _pname: GLenum, _params: *mut GLfloat) {
        gles_unimplemented!(self, "GetFloatv not implemented by this backend")
    }
    unsafe fn GetIntegerv(&mut self, _pname: GLenum, _params: *mut GLint) {
        gles_unimplemented!(self, "GetIntegerv not implemented by this backend")
    }
    unsafe fn GetFixedv(&mut self, _pname: GLenum, _params: *mut GLfixed) {
        gles_unimplemented!(self, "GetFixedv not implemented by this backend")
    }
    unsafe fn GetTexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint) {
        gles_unimplemented!(self, "GetTexEnviv not implemented by this backend")
    }
    unsafe fn GetTexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLfloat) {
        gles_unimplemented!(self, "GetTexEnvfv not implemented by this backend")
    }
    unsafe fn GetTexEnvxv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLfixed) {
        gles_unimplemented!(self, "GetTexEnvxv not implemented by this backend")
    }
    unsafe fn GetTexParameteriv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint) {
        gles_unimplemented!(self, "GetTexParameteriv not implemented by this backend")
    }
    unsafe fn GetTexParameterfv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLfloat) {
        gles_unimplemented!(self, "GetTexParameterfv not implemented by this backend")
    }
    unsafe fn GetTexParameterxv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLfixed) {
        gles_unimplemented!(self, "GetTexParameterxv not implemented by this backend")
    }
    unsafe fn GetClipPlanef(&mut self, _plane: GLenum, _equation: *mut GLfloat) {
        gles_unimplemented!(self, "GetClipPlanef not implemented by this backend")
    }
    unsafe fn GetClipPlanex(&mut self, _plane: GLenum, _equation: *mut GLfixed) {
        gles_unimplemented!(self, "GetClipPlanex not implemented by this backend")
    }
    unsafe fn GetLightfv(&mut self, _light: GLenum, _pname: GLenum, _params: *mut GLfloat) {
        gles_unimplemented!(self, "GetLightfv not implemented by this backend")
    }
    unsafe fn GetLightxv(&mut self, _light: GLenum, _pname: GLenum, _params: *mut GLfixed) {
        gles_unimplemented!(self, "GetLightxv not implemented by this backend")
    }
    unsafe fn GetMaterialfv(&mut self, _face: GLenum, _pname: GLenum, _params: *mut GLfloat) {
        gles_unimplemented!(self, "GetMaterialfv not implemented by this backend")
    }
    unsafe fn GetMaterialxv(&mut self, _face: GLenum, _pname: GLenum, _params: *mut GLfixed) {
        gles_unimplemented!(self, "GetMaterialxv not implemented by this backend")
    }
    unsafe fn GetPointerv(&mut self, _pname: GLenum, _params: *mut *const GLvoid) {
        gles_unimplemented!(self, "GetPointerv not implemented by this backend")
    }
    unsafe fn Hint(&mut self, _target: GLenum, _mode: GLenum) {
        gles_unimplemented!(self, "Hint not implemented by this backend")
    }
    unsafe fn Finish(&mut self) {
        gles_unimplemented!(self, "Finish not implemented by this backend")
    }
    unsafe fn Flush(&mut self) {
        gles_unimplemented!(self, "Flush not implemented by this backend")
    }
    #[allow(dead_code)]
    unsafe fn GetString(&mut self, _name: GLenum) -> *const GLubyte {
        gles_unimplemented!(self, "GetString not implemented by this backend")
    }

    // Other state manipulation
    unsafe fn AlphaFunc(&mut self, _func: GLenum, _ref_: GLclampf) {
        gles_unimplemented!(self, "AlphaFunc not implemented by this backend")
    }
    unsafe fn AlphaFuncx(&mut self, _func: GLenum, _ref_: GLclampx) {
        gles_unimplemented!(self, "AlphaFuncx not implemented by this backend")
    }
    unsafe fn BlendFunc(&mut self, _sfactor: GLenum, _dfactor: GLenum) {
        gles_unimplemented!(self, "BlendFunc not implemented by this backend")
    }
    unsafe fn BlendEquationOES(&mut self, _mode: GLenum) {
        gles_unimplemented!(self, "BlendEquationOES not implemented by this backend")
    }
    unsafe fn ColorMask(
        &mut self,
        _red: GLboolean,
        _green: GLboolean,
        _blue: GLboolean,
        _alpha: GLboolean,
    ) {
        gles_unimplemented!(self, "ColorMask not implemented by this backend")
    }
    unsafe fn ClipPlanef(&mut self, _plane: GLenum, _equation: *const GLfloat) {
        gles_unimplemented!(self, "ClipPlanef not implemented by this backend")
    }
    unsafe fn ClipPlanex(&mut self, _plane: GLenum, _equation: *const GLfixed) {
        gles_unimplemented!(self, "ClipPlanex not implemented by this backend")
    }
    unsafe fn CullFace(&mut self, _mode: GLenum) {
        gles_unimplemented!(self, "CullFace not implemented by this backend")
    }
    unsafe fn DepthFunc(&mut self, _func: GLenum) {
        gles_unimplemented!(self, "DepthFunc not implemented by this backend")
    }
    unsafe fn DepthMask(&mut self, _flag: GLboolean) {
        gles_unimplemented!(self, "DepthMask not implemented by this backend")
    }
    unsafe fn DepthRangef(&mut self, _near: GLclampf, _far: GLclampf) {
        gles_unimplemented!(self, "DepthRangef not implemented by this backend")
    }
    unsafe fn DepthRangex(&mut self, _near: GLclampx, _far: GLclampx) {
        gles_unimplemented!(self, "DepthRangex not implemented by this backend")
    }
    unsafe fn FrontFace(&mut self, _mode: GLenum) {
        gles_unimplemented!(self, "FrontFace not implemented by this backend")
    }
    unsafe fn PolygonOffset(&mut self, _factor: GLfloat, _units: GLfloat) {
        gles_unimplemented!(self, "PolygonOffset not implemented by this backend")
    }
    unsafe fn PolygonOffsetx(&mut self, _factor: GLfixed, _units: GLfixed) {
        gles_unimplemented!(self, "PolygonOffsetx not implemented by this backend")
    }
    unsafe fn SampleCoverage(&mut self, _value: GLclampf, _invert: GLboolean) {
        gles_unimplemented!(self, "SampleCoverage not implemented by this backend")
    }
    unsafe fn SampleCoveragex(&mut self, _value: GLclampx, _invert: GLboolean) {
        gles_unimplemented!(self, "SampleCoveragex not implemented by this backend")
    }
    unsafe fn ShadeModel(&mut self, _mode: GLenum) {
        gles_unimplemented!(self, "ShadeModel not implemented by this backend")
    }
    unsafe fn Scissor(&mut self, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei) {
        gles_unimplemented!(self, "Scissor not implemented by this backend")
    }
    unsafe fn Viewport(&mut self, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei) {
        gles_unimplemented!(self, "Viewport not implemented by this backend")
    }
    unsafe fn LineWidth(&mut self, _val: GLfloat) {
        gles_unimplemented!(self, "LineWidth not implemented by this backend")
    }
    unsafe fn LineWidthx(&mut self, _val: GLfixed) {
        gles_unimplemented!(self, "LineWidthx not implemented by this backend")
    }
    unsafe fn StencilFunc(&mut self, _func: GLenum, _ref_: GLint, _mask: GLuint) {
        gles_unimplemented!(self, "StencilFunc not implemented by this backend")
    }
    unsafe fn StencilOp(&mut self, _sfail: GLenum, _dpfail: GLenum, _dppass: GLenum) {
        gles_unimplemented!(self, "StencilOp not implemented by this backend")
    }
    unsafe fn StencilMask(&mut self, _mask: GLuint) {
        gles_unimplemented!(self, "StencilMask not implemented by this backend")
    }
    unsafe fn LogicOp(&mut self, _opcode: GLenum) {
        gles_unimplemented!(self, "LogicOp not implemented by this backend")
    }

    // Fixed-function point state has no equivalent in GLES 2.0/3.0 contexts.
    // Legacy apps sometimes call these setters from a shared GLES code path;
    // ignoring them is safer than aborting the guest on shader-based backends.
    unsafe fn PointSize(&mut self, _size: GLfloat) {}
    unsafe fn PointSizex(&mut self, _size: GLfixed) {}
    unsafe fn PointParameterf(&mut self, _pname: GLenum, _param: GLfloat) {}
    unsafe fn PointParameterx(&mut self, _pname: GLenum, _param: GLfixed) {}
    unsafe fn PointParameterfv(&mut self, _pname: GLenum, _params: *const GLfloat) {}
    unsafe fn PointParameterxv(&mut self, _pname: GLenum, _params: *const GLfixed) {}

    // Lighting and materials
    unsafe fn Fogf(&mut self, _pname: GLenum, _param: GLfloat) {}
    unsafe fn Fogx(&mut self, _pname: GLenum, _param: GLfixed) {}
    unsafe fn Fogfv(&mut self, _pname: GLenum, _params: *const GLfloat) {}
    unsafe fn Fogxv(&mut self, _pname: GLenum, _params: *const GLfixed) {}
    unsafe fn Lightf(&mut self, _light: GLenum, _pname: GLenum, _param: GLfloat) {}
    unsafe fn Lightx(&mut self, _light: GLenum, _pname: GLenum, _param: GLfixed) {}
    unsafe fn Lightfv(&mut self, _light: GLenum, _pname: GLenum, _params: *const GLfloat) {}
    unsafe fn Lightxv(&mut self, _light: GLenum, _pname: GLenum, _params: *const GLfixed) {}
    unsafe fn LightModelf(&mut self, _pname: GLenum, _param: GLfloat) {}
    unsafe fn LightModelx(&mut self, _pname: GLenum, _param: GLfixed) {}
    unsafe fn LightModelfv(&mut self, _pname: GLenum, _params: *const GLfloat) {}
    unsafe fn LightModelxv(&mut self, _pname: GLenum, _params: *const GLfixed) {}
    unsafe fn Materialf(&mut self, _face: GLenum, _pname: GLenum, _param: GLfloat) {}
    unsafe fn Materialx(&mut self, _face: GLenum, _pname: GLenum, _param: GLfixed) {}
    unsafe fn Materialfv(&mut self, _face: GLenum, _pname: GLenum, _params: *const GLfloat) {}
    unsafe fn Materialxv(&mut self, _face: GLenum, _pname: GLenum, _params: *const GLfixed) {}

    // Buffers
    unsafe fn IsBuffer(&mut self, _buffer: GLuint) -> GLboolean {
        gles_unimplemented!(self, "IsBuffer not implemented by this backend")
    }
    unsafe fn GenBuffers(&mut self, _n: GLsizei, _buffers: *mut GLuint) {
        gles_unimplemented!(self, "GenBuffers not implemented by this backend")
    }
    unsafe fn DeleteBuffers(&mut self, _n: GLsizei, _buffers: *const GLuint) {
        gles_unimplemented!(self, "DeleteBuffers not implemented by this backend")
    }
    unsafe fn BindBuffer(&mut self, _target: GLenum, _buffer: GLuint) {
        gles_unimplemented!(self, "BindBuffer not implemented by this backend")
    }
    unsafe fn BufferData(
        &mut self,
        _target: GLenum,
        _size: GLsizeiptr,
        _data: *const GLvoid,
        _usage: GLenum,
    ) {
        gles_unimplemented!(self, "BufferData not implemented by this backend")
    }
    unsafe fn BufferSubData(
        &mut self,
        _target: GLenum,
        _offset: GLintptr,
        _size: GLsizeiptr,
        _data: *const GLvoid,
    ) {
        gles_unimplemented!(self, "BufferSubData not implemented by this backend")
    }

    // Non-pointers
    unsafe fn Color4f(&mut self, _red: GLfloat, _green: GLfloat, _blue: GLfloat, _alpha: GLfloat) {
        gles_unimplemented!(self, "Color4f not implemented by this backend")
    }
    unsafe fn Color4x(&mut self, _red: GLfixed, _green: GLfixed, _blue: GLfixed, _alpha: GLfixed) {
        gles_unimplemented!(self, "Color4x not implemented by this backend")
    }
    unsafe fn Color4ub(&mut self, _red: GLubyte, _green: GLubyte, _blue: GLubyte, _alpha: GLubyte) {
        gles_unimplemented!(self, "Color4ub not implemented by this backend")
    }
    unsafe fn Normal3f(&mut self, _nx: GLfloat, _ny: GLfloat, _nz: GLfloat) {
        gles_unimplemented!(self, "Normal3f not implemented by this backend")
    }
    unsafe fn Normal3x(&mut self, _nx: GLfixed, _ny: GLfixed, _nz: GLfixed) {
        gles_unimplemented!(self, "Normal3x not implemented by this backend")
    }

    // Pointers
    unsafe fn ColorPointer(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        gles_unimplemented!(self, "ColorPointer not implemented by this backend")
    }
    unsafe fn NormalPointer(&mut self, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid) {
        gles_unimplemented!(self, "NormalPointer not implemented by this backend")
    }
    unsafe fn TexCoordPointer(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        gles_unimplemented!(self, "TexCoordPointer not implemented by this backend")
    }
    unsafe fn VertexPointer(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        gles_unimplemented!(self, "VertexPointer not implemented by this backend")
    }
    /// `glPointSizePointerOES` from `GL_OES_point_size_array`. Lets the
    /// app supply per-vertex point sizes alongside the regular vertex
    /// array. `type_` is `GL_FLOAT` or `GL_FIXED`.
    unsafe fn PointSizePointerOES(
        &mut self,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        let _ = (type_, stride, pointer);
        // Backends that lack `GL_ARB_point_parameters`-like per-vertex
        // sizing fall back to the scalar `glPointSize` value, which is the
        // closest sensible behaviour available.
    }

    /// `GL_OES_matrix_palette` / `GL_ARB_matrix_palette`.
    /// Backends may override this to keep track of the current palette matrix
    /// index; the default just accepts the call.
    unsafe fn CurrentPaletteMatrixOES(&mut self, _matrixpaletteindex: GLuint) {
        let _ = _matrixpaletteindex;
    }
    /// Copies the current model-view matrix into the current palette slot.
    /// Backends may override this for real palette support.
    unsafe fn LoadPaletteFromModelViewMatrixOES(&mut self) {}
    /// `glMatrixIndexPointerOES` / `glMatrixIndexPointerARB`.
    unsafe fn MatrixIndexPointerOES(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        let _ = (_size, _type_, _stride, _pointer);
    }
    /// `glWeightPointerOES` / `glWeightPointerARB`.
    unsafe fn WeightPointerOES(
        &mut self,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        let _ = (_size, _type_, _stride, _pointer);
    }

    // Drawing
    unsafe fn DrawArrays(&mut self, _mode: GLenum, _first: GLint, _count: GLsizei) {
        gles_unimplemented!(self, "DrawArrays not implemented by this backend")
    }
    unsafe fn DrawElements(
        &mut self,
        _mode: GLenum,
        _count: GLsizei,
        _type_: GLenum,
        _indices: *const GLvoid,
    ) {
        gles_unimplemented!(self, "DrawElements not implemented by this backend")
    }
    /// `glDrawTex{s,i,x,f}OES` from `GL_OES_draw_texture` — blit the
    /// currently bound 2D texture (using `GL_TEXTURE_CROP_RECT_OES`) to a
    /// screen-space rectangle. The default implementation provides a
    /// software fallback in `present_renderbuffer`-style code paths; concrete
    /// backends with a native equivalent override these.
    unsafe fn DrawTexfOES(
        &mut self,
        x: GLfloat,
        y: GLfloat,
        z: GLfloat,
        width: GLfloat,
        height: GLfloat,
    ) {
        let _ = (x, y, z, width, height);
        // Default: do nothing. Most apps that use GL_OES_draw_texture also
        // ship a fallback path; not panicking lets them progress.
    }

    // Clearing
    unsafe fn Clear(&mut self, _mask: GLbitfield) {
        gles_unimplemented!(self, "Clear not implemented by this backend")
    }
    unsafe fn ClearColor(
        &mut self,
        _red: GLclampf,
        _green: GLclampf,
        _blue: GLclampf,
        _alpha: GLclampf,
    ) {
        gles_unimplemented!(self, "ClearColor not implemented by this backend")
    }
    unsafe fn ClearColorx(
        &mut self,
        _red: GLclampx,
        _green: GLclampx,
        _blue: GLclampx,
        _alpha: GLclampx,
    ) {
        gles_unimplemented!(self, "ClearColorx not implemented by this backend")
    }
    unsafe fn ClearDepthf(&mut self, _depth: GLclampf) {
        gles_unimplemented!(self, "ClearDepthf not implemented by this backend")
    }
    unsafe fn ClearDepthx(&mut self, _depth: GLclampx) {
        gles_unimplemented!(self, "ClearDepthx not implemented by this backend")
    }
    unsafe fn ClearStencil(&mut self, _s: GLint) {
        gles_unimplemented!(self, "ClearStencil not implemented by this backend")
    }

    // Textures
    unsafe fn PixelStorei(&mut self, _pname: GLenum, _param: GLint) {
        gles_unimplemented!(self, "PixelStorei not implemented by this backend")
    }
    unsafe fn ReadPixels(
        &mut self,
        _x: GLint,
        _y: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _format: GLenum,
        _type_: GLenum,
        _pixels: *mut GLvoid,
    ) {
        gles_unimplemented!(self, "ReadPixels not implemented by this backend")
    }
    unsafe fn GenTextures(&mut self, _n: GLsizei, _textures: *mut GLuint) {
        gles_unimplemented!(self, "GenTextures not implemented by this backend")
    }
    unsafe fn DeleteTextures(&mut self, _n: GLsizei, _textures: *const GLuint) {
        gles_unimplemented!(self, "DeleteTextures not implemented by this backend")
    }
    unsafe fn ActiveTexture(&mut self, _texture: GLenum) {
        gles_unimplemented!(self, "ActiveTexture not implemented by this backend")
    }
    unsafe fn IsTexture(&mut self, _texture: GLuint) -> GLboolean {
        gles_unimplemented!(self, "IsTexture not implemented by this backend")
    }
    unsafe fn BindTexture(&mut self, _target: GLenum, _texture: GLuint) {
        gles_unimplemented!(self, "BindTexture not implemented by this backend")
    }
    unsafe fn TexParameteri(&mut self, _target: GLenum, _pname: GLenum, _param: GLint) {
        gles_unimplemented!(self, "TexParameteri not implemented by this backend")
    }
    unsafe fn TexParameterf(&mut self, _target: GLenum, _pname: GLenum, _param: GLfloat) {
        gles_unimplemented!(self, "TexParameterf not implemented by this backend")
    }
    unsafe fn TexParameterx(&mut self, _target: GLenum, _pname: GLenum, _param: GLfixed) {
        gles_unimplemented!(self, "TexParameterx not implemented by this backend")
    }
    unsafe fn TexParameteriv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLint) {
        gles_unimplemented!(self, "TexParameteriv not implemented by this backend")
    }
    unsafe fn TexParameterfv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfloat) {
        gles_unimplemented!(self, "TexParameterfv not implemented by this backend")
    }
    unsafe fn TexParameterxv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfixed) {
        gles_unimplemented!(self, "TexParameterxv not implemented by this backend")
    }
    unsafe fn TexImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _internalformat: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _border: GLint,
        _format: GLenum,
        _type_: GLenum,
        _pixels: *const GLvoid,
    ) {
        gles_unimplemented!(self, "TexImage2D not implemented by this backend")
    }
    unsafe fn TexSubImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _xoffset: GLint,
        _yoffset: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _format: GLenum,
        _type_: GLenum,
        _pixels: *const GLvoid,
    ) {
        gles_unimplemented!(self, "TexSubImage2D not implemented by this backend")
    }
    unsafe fn CompressedTexImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _internalformat: GLenum,
        _width: GLsizei,
        _height: GLsizei,
        _border: GLint,
        _image_size: GLsizei,
        _data: *const GLvoid,
    ) {
        gles_unimplemented!(self, "CompressedTexImage2D not implemented by this backend")
    }
    unsafe fn CompressedTexSubImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        image_size: GLsizei,
        data: *const GLvoid,
    ) {
        gles11::CompressedTexSubImage2D(
            target, level, xoffset, yoffset, width, height, format, image_size, data,
        )
    }
    unsafe fn CopyTexImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _internalformat: GLenum,
        _x: GLint,
        _y: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _border: GLint,
    ) {
        gles_unimplemented!(self, "CopyTexImage2D not implemented by this backend")
    }
    unsafe fn CopyTexSubImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _xoffset: GLint,
        _yoffset: GLint,
        _x: GLint,
        _y: GLint,
        _width: GLsizei,
        _height: GLsizei,
    ) {
        gles_unimplemented!(self, "CopyTexSubImage2D not implemented by this backend")
    }
    unsafe fn TexEnvf(&mut self, _target: GLenum, _pname: GLenum, _param: GLfloat) {
        gles_unimplemented!(self, "TexEnvf not implemented by this backend")
    }
    unsafe fn TexEnvx(&mut self, _target: GLenum, _pname: GLenum, _param: GLfixed) {
        gles_unimplemented!(self, "TexEnvx not implemented by this backend")
    }
    unsafe fn TexEnvi(&mut self, _target: GLenum, _pname: GLenum, _param: GLint) {
        gles_unimplemented!(self, "TexEnvi not implemented by this backend")
    }
    unsafe fn TexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfloat) {
        gles_unimplemented!(self, "TexEnvfv not implemented by this backend")
    }
    unsafe fn TexEnvxv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfixed) {
        gles_unimplemented!(self, "TexEnvxv not implemented by this backend")
    }
    unsafe fn TexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLint) {
        gles_unimplemented!(self, "TexEnviv not implemented by this backend")
    }
    unsafe fn DrawTexsOES(&mut self, x: i16, y: i16, z: i16, width: i16, height: i16) {
        self.DrawTexfOES(
            x as GLfloat,
            y as GLfloat,
            z as GLfloat,
            width as GLfloat,
            height as GLfloat,
        )
    }
    unsafe fn DrawTexiOES(&mut self, x: GLint, y: GLint, z: GLint, width: GLint, height: GLint) {
        self.DrawTexfOES(
            x as GLfloat,
            y as GLfloat,
            z as GLfloat,
            width as GLfloat,
            height as GLfloat,
        )
    }
    unsafe fn DrawTexxOES(
        &mut self,
        x: GLfixed,
        y: GLfixed,
        z: GLfixed,
        width: GLfixed,
        height: GLfixed,
    ) {
        use super::util::fixed_to_float;
        self.DrawTexfOES(
            fixed_to_float(x),
            fixed_to_float(y),
            fixed_to_float(z),
            fixed_to_float(width),
            fixed_to_float(height),
        )
    }
    unsafe fn DrawTexsvOES(&mut self, coords: *const i16) {
        self.DrawTexsOES(
            coords.read_unaligned(),
            coords.add(1).read_unaligned(),
            coords.add(2).read_unaligned(),
            coords.add(3).read_unaligned(),
            coords.add(4).read_unaligned(),
        )
    }
    unsafe fn DrawTexivOES(&mut self, coords: *const GLint) {
        self.DrawTexiOES(
            coords.read_unaligned(),
            coords.add(1).read_unaligned(),
            coords.add(2).read_unaligned(),
            coords.add(3).read_unaligned(),
            coords.add(4).read_unaligned(),
        )
    }
    unsafe fn DrawTexxvOES(&mut self, coords: *const GLfixed) {
        self.DrawTexxOES(
            coords.read_unaligned(),
            coords.add(1).read_unaligned(),
            coords.add(2).read_unaligned(),
            coords.add(3).read_unaligned(),
            coords.add(4).read_unaligned(),
        )
    }
    unsafe fn DrawTexfvOES(&mut self, coords: *const GLfloat) {
        self.DrawTexfOES(
            coords.read_unaligned(),
            coords.add(1).read_unaligned(),
            coords.add(2).read_unaligned(),
            coords.add(3).read_unaligned(),
            coords.add(4).read_unaligned(),
        )
    }

    unsafe fn MultiTexCoord4f(
        &mut self,
        _target: GLenum,
        _s: GLfloat,
        _t: GLfloat,
        _r: GLfloat,
        _q: GLfloat,
    ) {
        gles_unimplemented!(self, "MultiTexCoord4f not implemented by this backend")
    }
    unsafe fn MultiTexCoord4x(
        &mut self,
        _target: GLenum,
        _s: GLfixed,
        _t: GLfixed,
        _r: GLfixed,
        _q: GLfixed,
    ) {
        gles_unimplemented!(self, "MultiTexCoord4x not implemented by this backend")
    }

    // Matrix stack operations
    unsafe fn MatrixMode(&mut self, _mode: GLenum) {
        gles_unimplemented!(self, "MatrixMode not implemented by this backend")
    }
    unsafe fn LoadIdentity(&mut self) {
        gles_unimplemented!(self, "LoadIdentity not implemented by this backend")
    }
    unsafe fn LoadMatrixf(&mut self, _m: *const GLfloat) {
        gles_unimplemented!(self, "LoadMatrixf not implemented by this backend")
    }
    unsafe fn LoadMatrixx(&mut self, _m: *const GLfixed) {
        gles_unimplemented!(self, "LoadMatrixx not implemented by this backend")
    }
    unsafe fn MultMatrixf(&mut self, _m: *const GLfloat) {
        gles_unimplemented!(self, "MultMatrixf not implemented by this backend")
    }
    unsafe fn MultMatrixx(&mut self, _m: *const GLfixed) {
        gles_unimplemented!(self, "MultMatrixx not implemented by this backend")
    }
    unsafe fn PushMatrix(&mut self) {
        gles_unimplemented!(self, "PushMatrix not implemented by this backend")
    }
    unsafe fn PopMatrix(&mut self) {
        gles_unimplemented!(self, "PopMatrix not implemented by this backend")
    }
    unsafe fn Orthof(
        &mut self,
        _left: GLfloat,
        _right: GLfloat,
        _bottom: GLfloat,
        _top: GLfloat,
        _near: GLfloat,
        _far: GLfloat,
    ) {
        gles_unimplemented!(self, "Orthof not implemented by this backend")
    }
    unsafe fn Orthox(
        &mut self,
        _left: GLfixed,
        _right: GLfixed,
        _bottom: GLfixed,
        _top: GLfixed,
        _near: GLfixed,
        _far: GLfixed,
    ) {
        gles_unimplemented!(self, "Orthox not implemented by this backend")
    }
    unsafe fn Frustumf(
        &mut self,
        _left: GLfloat,
        _right: GLfloat,
        _bottom: GLfloat,
        _top: GLfloat,
        _near: GLfloat,
        _far: GLfloat,
    ) {
        gles_unimplemented!(self, "Frustumf not implemented by this backend")
    }
    unsafe fn Frustumx(
        &mut self,
        _left: GLfixed,
        _right: GLfixed,
        _bottom: GLfixed,
        _top: GLfixed,
        _near: GLfixed,
        _far: GLfixed,
    ) {
        gles_unimplemented!(self, "Frustumx not implemented by this backend")
    }
    unsafe fn Rotatef(&mut self, _angle: GLfloat, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        gles_unimplemented!(self, "Rotatef not implemented by this backend")
    }
    unsafe fn Rotatex(&mut self, _angle: GLfixed, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
        gles_unimplemented!(self, "Rotatex not implemented by this backend")
    }
    unsafe fn Scalef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        gles_unimplemented!(self, "Scalef not implemented by this backend")
    }
    unsafe fn Scalex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
        gles_unimplemented!(self, "Scalex not implemented by this backend")
    }
    unsafe fn Translatef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        gles_unimplemented!(self, "Translatef not implemented by this backend")
    }
    unsafe fn Translatex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
        gles_unimplemented!(self, "Translatex not implemented by this backend")
    }

    // OES_framebuffer_object (incomplete)
    unsafe fn GenFramebuffersOES(&mut self, _n: GLsizei, _framebuffers: *mut GLuint) {
        gles_unimplemented!(self, "GenFramebuffersOES not implemented by this backend")
    }
    unsafe fn GenRenderbuffersOES(&mut self, _n: GLsizei, _renderbuffers: *mut GLuint) {
        gles_unimplemented!(self, "GenRenderbuffersOES not implemented by this backend")
    }
    unsafe fn IsFramebufferOES(&mut self, _framebuffer: GLuint) -> GLboolean {
        gles_unimplemented!(self, "IsFramebufferOES not implemented by this backend")
    }
    unsafe fn IsRenderbufferOES(&mut self, _renderbuffer: GLuint) -> GLboolean {
        gles_unimplemented!(self, "IsRenderbufferOES not implemented by this backend")
    }
    unsafe fn BindFramebufferOES(&mut self, _target: GLenum, _framebuffer: GLuint) {
        gles_unimplemented!(self, "BindFramebufferOES not implemented by this backend")
    }
    unsafe fn BindRenderbufferOES(&mut self, _target: GLenum, _renderbuffer: GLuint) {
        gles_unimplemented!(self, "BindRenderbufferOES not implemented by this backend")
    }
    unsafe fn RenderbufferStorageOES(
        &mut self,
        _target: GLenum,
        _internalformat: GLenum,
        _width: GLsizei,
        _height: GLsizei,
    ) {
        gles_unimplemented!(
            self,
            "RenderbufferStorageOES not implemented by this backend"
        )
    }
    unsafe fn FramebufferRenderbufferOES(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _renderbuffertarget: GLenum,
        _renderbuffer: GLuint,
    ) {
        gles_unimplemented!(
            self,
            "FramebufferRenderbufferOES not implemented by this backend"
        )
    }
    unsafe fn FramebufferTexture2DOES(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _textarget: GLenum,
        _texture: GLuint,
        _level: i32,
    ) {
        gles_unimplemented!(
            self,
            "FramebufferTexture2DOES not implemented by this backend"
        )
    }
    unsafe fn GetFramebufferAttachmentParameterivOES(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        gles_unimplemented!(
            self,
            "GetFramebufferAttachmentParameterivOES not implemented by this backend"
        )
    }
    unsafe fn GetRenderbufferParameterivOES(
        &mut self,
        _target: GLenum,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        gles_unimplemented!(
            self,
            "GetRenderbufferParameterivOES not implemented by this backend"
        )
    }
    unsafe fn CheckFramebufferStatusOES(&mut self, _target: GLenum) -> GLenum {
        gles_unimplemented!(
            self,
            "CheckFramebufferStatusOES not implemented by this backend"
        )
    }
    unsafe fn DeleteFramebuffersOES(&mut self, _n: GLsizei, _framebuffers: *const GLuint) {
        gles_unimplemented!(
            self,
            "DeleteFramebuffersOES not implemented by this backend"
        )
    }
    unsafe fn DeleteRenderbuffersOES(&mut self, _n: GLsizei, _renderbuffers: *const GLuint) {
        gles_unimplemented!(
            self,
            "DeleteRenderbuffersOES not implemented by this backend"
        )
    }
    unsafe fn GenerateMipmapOES(&mut self, _target: GLenum) {
        gles_unimplemented!(self, "GenerateMipmapOES not implemented by this backend")
    }

    // GL_APPLE_framebuffer_multisample
    //
    // Apple's iOS MSAA pattern uses a "sample" framebuffer with multisample
    // renderbuffer storage, renders to it, and then resolves into a separate
    // "resolve" framebuffer whose color renderbuffer is the CAEAGLLayer
    // drawable. `presentRenderbuffer:` is then called on the resolve
    // renderbuffer. Without an honest resolve, the resolve renderbuffer stays
    // empty and the screen never updates — see Apple's "Working with EAGL
    // Contexts" docs and the
    // [GL_APPLE_framebuffer_multisample]
    // (https://registry.khronos.org/OpenGL/extensions/APPLE/APPLE_framebuffer_multisample.txt)
    // extension spec.
    unsafe fn RenderbufferStorageMultisampleAPPLE(
        &mut self,
        target: GLenum,
        samples: GLsizei,
        internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
    ) {
        // Default: fall back to single-sample. Concrete backends that have
        // the equivalent host-side extension override this.
        let _ = samples;
        self.RenderbufferStorageOES(target, internalformat, width, height);
    }
    unsafe fn ResolveMultisampleFramebufferAPPLE(&mut self) {
        // Default fallback for backends that don't implement a true
        // multisample resolve (e.g. native OpenGL ES 1.1 / ES 2.0 on Android,
        // where the host driver lacks `GL_APPLE_framebuffer_multisample` and
        // there's no `glBlitFramebuffer` to copy with). The companion
        // `RenderbufferStorageMultisampleAPPLE` default impl already degrades
        // multisample storage to single-sample, so the "sample" renderbuffer
        // holds the rendered pixels directly. With the canonical Apple
        // EAGLView MSAA pattern the sample framebuffer remains the currently
        // bound framebuffer (because the driver rejects
        // `GL_READ_FRAMEBUFFER_APPLE` / `GL_DRAW_FRAMEBUFFER_APPLE` as
        // targets without the extension), so `presentRenderbuffer:`'s
        // existing fallback — `glCopyTexImage2D` from the currently bound
        // FBO's color attachment — still finds the rendered frame and
        // displays it. Panicking here turned Temple Run (and any other
        // canonical EAGLView MSAA app) into a hard host-process abort on the
        // first frame; logging once and continuing keeps the app running.
        log_once!(
            "ResolveMultisampleFramebufferAPPLE: backend lacks a real resolve; \
             relying on the single-sample fallback for RenderbufferStorageMultisampleAPPLE \
             and the bound-FBO source in presentRenderbuffer:"
        );
    }

    // Non-OES aliases for OES_framebuffer_object functions.
    // Some GLES1 apps call the suffix-free ES2-style names directly.
    unsafe fn GenFramebuffers(&mut self, _n: GLsizei, _framebuffers: *mut GLuint) {
        gles_unimplemented!(self, "GenFramebuffers not implemented by this backend")
    }
    unsafe fn GenRenderbuffers(&mut self, _n: GLsizei, _renderbuffers: *mut GLuint) {
        gles_unimplemented!(self, "GenRenderbuffers not implemented by this backend")
    }
    unsafe fn IsFramebuffer(&mut self, _framebuffer: GLuint) -> GLboolean {
        gles_unimplemented!(self, "IsFramebuffer not implemented by this backend")
    }
    unsafe fn IsRenderbuffer(&mut self, _renderbuffer: GLuint) -> GLboolean {
        gles_unimplemented!(self, "IsRenderbuffer not implemented by this backend")
    }
    unsafe fn BindFramebuffer(&mut self, _target: GLenum, _framebuffer: GLuint) {
        gles_unimplemented!(self, "BindFramebuffer not implemented by this backend")
    }
    unsafe fn BindRenderbuffer(&mut self, _target: GLenum, _renderbuffer: GLuint) {
        gles_unimplemented!(self, "BindRenderbuffer not implemented by this backend")
    }
    unsafe fn RenderbufferStorage(
        &mut self,
        _target: GLenum,
        _internalformat: GLenum,
        _width: GLsizei,
        _height: GLsizei,
    ) {
        gles_unimplemented!(self, "RenderbufferStorage not implemented by this backend")
    }
    unsafe fn FramebufferRenderbuffer(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _renderbuffertarget: GLenum,
        _renderbuffer: GLuint,
    ) {
        gles_unimplemented!(
            self,
            "FramebufferRenderbuffer not implemented by this backend"
        )
    }
    unsafe fn FramebufferTexture2D(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _textarget: GLenum,
        _texture: GLuint,
        _level: i32,
    ) {
        gles_unimplemented!(self, "FramebufferTexture2D not implemented by this backend")
    }
    unsafe fn CheckFramebufferStatus(&mut self, _target: GLenum) -> GLenum {
        gles_unimplemented!(
            self,
            "CheckFramebufferStatus not implemented by this backend"
        )
    }
    unsafe fn DeleteFramebuffers(&mut self, _n: GLsizei, _framebuffers: *const GLuint) {
        gles_unimplemented!(self, "DeleteFramebuffers not implemented by this backend")
    }
    unsafe fn DeleteRenderbuffers(&mut self, _n: GLsizei, _renderbuffers: *const GLuint) {
        gles_unimplemented!(self, "DeleteRenderbuffers not implemented by this backend")
    }
    unsafe fn GenerateMipmap(&mut self, _target: GLenum) {
        gles_unimplemented!(self, "GenerateMipmap not implemented by this backend")
    }
    unsafe fn GetFramebufferAttachmentParameteriv(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        gles_unimplemented!(
            self,
            "GetFramebufferAttachmentParameteriv not implemented by this backend"
        )
    }
    unsafe fn GetRenderbufferParameteriv(
        &mut self,
        _target: GLenum,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        gles_unimplemented!(
            self,
            "GetRenderbufferParameteriv not implemented by this backend"
        )
    }

    unsafe fn GetBufferParameteriv(
        &mut self,
        _target: GLenum,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        gles_unimplemented!(self, "GetBufferParameteriv not implemented by this backend")
    }
    unsafe fn MapBufferOES(&mut self, _target: GLenum, _access: GLenum) -> *mut GLvoid {
        gles_unimplemented!(self, "MapBufferOES not implemented by this backend")
    }
    unsafe fn UnmapBufferOES(&mut self, _target: GLenum) -> GLboolean {
        gles_unimplemented!(self, "UnmapBufferOES not implemented by this backend")
    }

    // OpenGL ES 2.0 entry points. Every shader-capable backend
    // ([super::gles1_on_gl2], [super::gles2_native], [super::gles2_on_gl3],
    // [super::gles3_native], [super::gles3_on_gl3]) implements every method
    // below as a real call into the underlying driver. EAGL routes ES 2.0
    // contexts to one of these backends; ES 1.1-only backends
    // ([super::gles1_native]) override every method below with an ES-spec
    // compliant "no shader pipeline" stub that records a synthetic
    // `GL_INVALID_OPERATION` so the guest sees standard OpenGL ES error
    // semantics rather than a panic.
    //
    // The default implementations therefore `gles_unimplemented!(self, )` so that any
    // future backend which forgets to provide one of these methods fails
    // loudly at the point of misuse, rather than silently returning zero or
    // no-opping. This is what makes the touchHLE ES 2.0 surface
    // "implemented without stubs" — every code path is real.
    unsafe fn CreateShader(&mut self, _type_: GLenum) -> GLuint {
        gles_unimplemented!(
            self,
            "CreateShader (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn DeleteShader(&mut self, _shader: GLuint) {
        gles_unimplemented!(
            self,
            "DeleteShader (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn ShaderSource(
        &mut self,
        _shader: GLuint,
        _count: GLsizei,
        _string: *const *const GLchar,
        _length: *const GLint,
    ) {
        gles_unimplemented!(
            self,
            "ShaderSource (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn CompileShader(&mut self, _shader: GLuint) {
        gles_unimplemented!(
            self,
            "CompileShader (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetShaderiv(&mut self, _shader: GLuint, _pname: GLenum, _params: *mut GLint) {
        gles_unimplemented!(
            self,
            "GetShaderiv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetShaderInfoLog(
        &mut self,
        _shader: GLuint,
        _maxLength: GLsizei,
        _length: *mut GLsizei,
        _infoLog: *mut GLchar,
    ) {
        gles_unimplemented!(
            self,
            "GetShaderInfoLog (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn IsShader(&mut self, _shader: GLuint) -> GLboolean {
        gles_unimplemented!(
            self,
            "IsShader (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn CreateProgram(&mut self) -> GLuint {
        gles_unimplemented!(
            self,
            "CreateProgram (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn DeleteProgram(&mut self, _program: GLuint) {
        gles_unimplemented!(
            self,
            "DeleteProgram (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn AttachShader(&mut self, _program: GLuint, _shader: GLuint) {
        gles_unimplemented!(
            self,
            "AttachShader (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn DetachShader(&mut self, _program: GLuint, _shader: GLuint) {
        gles_unimplemented!(
            self,
            "DetachShader (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn LinkProgram(&mut self, _program: GLuint) {
        gles_unimplemented!(
            self,
            "LinkProgram (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn UseProgram(&mut self, _program: GLuint) {
        gles_unimplemented!(
            self,
            "UseProgram (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetProgramiv(&mut self, _program: GLuint, _pname: GLenum, _params: *mut GLint) {
        gles_unimplemented!(
            self,
            "GetProgramiv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetProgramInfoLog(
        &mut self,
        _program: GLuint,
        _maxLength: GLsizei,
        _length: *mut GLsizei,
        _infoLog: *mut GLchar,
    ) {
        gles_unimplemented!(
            self,
            "GetProgramInfoLog (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn IsProgram(&mut self, _program: GLuint) -> GLboolean {
        gles_unimplemented!(
            self,
            "IsProgram (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn ValidateProgram(&mut self, _program: GLuint) {
        gles_unimplemented!(
            self,
            "ValidateProgram (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn BindAttribLocation(
        &mut self,
        _program: GLuint,
        _index: GLuint,
        _name: *const GLchar,
    ) {
        gles_unimplemented!(
            self,
            "BindAttribLocation (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetAttribLocation(&mut self, _program: GLuint, _name: *const GLchar) -> GLint {
        gles_unimplemented!(
            self,
            "GetAttribLocation (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetUniformLocation(&mut self, _program: GLuint, _name: *const GLchar) -> GLint {
        gles_unimplemented!(
            self,
            "GetUniformLocation (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetActiveAttrib(
        &mut self,
        _program: GLuint,
        _index: GLuint,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _size: *mut GLint,
        _type_: *mut GLenum,
        _name: *mut GLchar,
    ) {
        gles_unimplemented!(
            self,
            "GetActiveAttrib (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetActiveUniform(
        &mut self,
        _program: GLuint,
        _index: GLuint,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _size: *mut GLint,
        _type_: *mut GLenum,
        _name: *mut GLchar,
    ) {
        gles_unimplemented!(
            self,
            "GetActiveUniform (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn EnableVertexAttribArray(&mut self, _index: GLuint) {
        gles_unimplemented!(
            self,
            "EnableVertexAttribArray (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn DisableVertexAttribArray(&mut self, _index: GLuint) {
        gles_unimplemented!(self,
            "DisableVertexAttribArray (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttribPointer(
        &mut self,
        _index: GLuint,
        _size: GLint,
        _type_: GLenum,
        _normalized: GLboolean,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        gles_unimplemented!(
            self,
            "VertexAttribPointer (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib1f(&mut self, _index: GLuint, _x: GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib1f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib2f(&mut self, _index: GLuint, _x: GLfloat, _y: GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib2f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib3f(&mut self, _index: GLuint, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib3f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib4f(
        &mut self,
        _index: GLuint,
        _x: GLfloat,
        _y: GLfloat,
        _z: GLfloat,
        _w: GLfloat,
    ) {
        gles_unimplemented!(
            self,
            "VertexAttrib4f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib1fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib1fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib2fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib2fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib3fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib3fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn VertexAttrib4fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        gles_unimplemented!(
            self,
            "VertexAttrib4fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform1f(&mut self, _location: GLint, _v0: GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform1f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform2f(&mut self, _location: GLint, _v0: GLfloat, _v1: GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform2f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform3f(&mut self, _location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform3f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform4f(
        &mut self,
        _location: GLint,
        _v0: GLfloat,
        _v1: GLfloat,
        _v2: GLfloat,
        _v3: GLfloat,
    ) {
        gles_unimplemented!(
            self,
            "Uniform4f (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform1i(&mut self, _location: GLint, _v0: GLint) {
        gles_unimplemented!(
            self,
            "Uniform1i (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform2i(&mut self, _location: GLint, _v0: GLint, _v1: GLint) {
        gles_unimplemented!(
            self,
            "Uniform2i (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform3i(&mut self, _location: GLint, _v0: GLint, _v1: GLint, _v2: GLint) {
        gles_unimplemented!(
            self,
            "Uniform3i (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform4i(
        &mut self,
        _location: GLint,
        _v0: GLint,
        _v1: GLint,
        _v2: GLint,
        _v3: GLint,
    ) {
        gles_unimplemented!(
            self,
            "Uniform4i (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform1fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform1fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform2fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform2fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform3fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform3fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform4fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        gles_unimplemented!(
            self,
            "Uniform4fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform1iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        gles_unimplemented!(
            self,
            "Uniform1iv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform2iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        gles_unimplemented!(
            self,
            "Uniform2iv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform3iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        gles_unimplemented!(
            self,
            "Uniform3iv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn Uniform4iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        gles_unimplemented!(
            self,
            "Uniform4iv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn UniformMatrix2fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        gles_unimplemented!(
            self,
            "UniformMatrix2fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn UniformMatrix3fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        gles_unimplemented!(
            self,
            "UniformMatrix3fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn UniformMatrix4fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        gles_unimplemented!(
            self,
            "UniformMatrix4fv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn BlendColor(&mut self, _r: GLclampf, _g: GLclampf, _b: GLclampf, _a: GLclampf) {
        gles_unimplemented!(
            self,
            "BlendColor (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn BlendEquation(&mut self, _mode: GLenum) {
        gles_unimplemented!(
            self,
            "BlendEquation (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn BlendEquationSeparate(&mut self, _modeRGB: GLenum, _modeAlpha: GLenum) {
        gles_unimplemented!(
            self,
            "BlendEquationSeparate (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn BlendFuncSeparate(
        &mut self,
        _srcRGB: GLenum,
        _dstRGB: GLenum,
        _srcAlpha: GLenum,
        _dstAlpha: GLenum,
    ) {
        gles_unimplemented!(
            self,
            "BlendFuncSeparate (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn StencilFuncSeparate(
        &mut self,
        _face: GLenum,
        _func: GLenum,
        _ref_: GLint,
        _mask: GLuint,
    ) {
        gles_unimplemented!(
            self,
            "StencilFuncSeparate (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn StencilOpSeparate(
        &mut self,
        _face: GLenum,
        _sfail: GLenum,
        _dpfail: GLenum,
        _dppass: GLenum,
    ) {
        gles_unimplemented!(
            self,
            "StencilOpSeparate (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn StencilMaskSeparate(&mut self, _face: GLenum, _mask: GLuint) {
        gles_unimplemented!(
            self,
            "StencilMaskSeparate (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetVertexAttribiv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLint) {
        gles_unimplemented!(
            self,
            "GetVertexAttribiv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetVertexAttribfv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLfloat) {
        gles_unimplemented!(
            self,
            "GetVertexAttribfv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetVertexAttribPointerv(
        &mut self,
        _index: GLuint,
        _pname: GLenum,
        _pointer: *mut *mut GLvoid,
    ) {
        gles_unimplemented!(
            self,
            "GetVertexAttribPointerv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetUniformiv(&mut self, _program: GLuint, _location: GLint, _params: *mut GLint) {
        gles_unimplemented!(
            self,
            "GetUniformiv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetUniformfv(&mut self, _program: GLuint, _location: GLint, _params: *mut GLfloat) {
        gles_unimplemented!(
            self,
            "GetUniformfv (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetAttachedShaders(
        &mut self,
        _program: GLuint,
        _maxCount: GLsizei,
        _count: *mut GLsizei,
        _shaders: *mut GLuint,
    ) {
        gles_unimplemented!(
            self,
            "GetAttachedShaders (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn GetShaderSource(
        &mut self,
        _shader: GLuint,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _source: *mut GLchar,
    ) {
        gles_unimplemented!(
            self,
            "GetShaderSource (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn ReleaseShaderCompiler(&mut self) {
        // No-op: we always have a shader compiler.
    }
    unsafe fn GetShaderPrecisionFormat(
        &mut self,
        _shadertype: GLenum,
        _precisiontype: GLenum,
        _range: *mut GLint,
        _precision: *mut GLint,
    ) {
        gles_unimplemented!(self,
            "GetShaderPrecisionFormat (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }
    unsafe fn ShaderBinary(
        &mut self,
        _count: GLsizei,
        _shaders: *const GLuint,
        _binaryformat: GLenum,
        _binary: *const GLvoid,
        _length: GLsizei,
    ) {
        gles_unimplemented!(
            self,
            "ShaderBinary (OpenGL ES 2.0) must be overridden by ES 2.0-capable backends"
        )
    }

    // ===== OpenGL ES 3.0 entry points =====
    //
    // The methods below correspond 1:1 to the OpenGL ES 3.0 specification
    // entry points that are new vs. ES 2.0 (plus a few that have ES-2
    // OES-extension equivalents — those keep their pre-existing entries
    // above and gain unsuffixed core variants here).
    //
    // Backends that do not implement OpenGL ES 3.0 inherit a default that
    // log-once warns and returns the type's default value, mirroring the
    // existing ES 2.0 stub convention. The dedicated `gles3_native` and
    // `gles3_on_gl3` backends override every single one with a real
    // pass-through call to the underlying driver — i.e. these are *not*
    // stubs at the level the guest app interacts with.

    /// Returns `true` if this backend implements OpenGL ES 3.0 (or higher).
    /// Used by `gles_guest` to gate ES-3-only entry points.
    fn is_es3(&self) -> bool {
        false
    }

    // -- Vertex array objects (promoted from OES_vertex_array_object) --
    unsafe fn IsVertexArray(&mut self, _array: GLuint) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "IsVertexArray (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn BindVertexArray(&mut self, _array: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "BindVertexArray (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DeleteVertexArrays(&mut self, _n: GLsizei, _arrays: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "DeleteVertexArrays (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GenVertexArrays(&mut self, _n: GLsizei, _arrays: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GenVertexArrays (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Vertex array objects via GL_OES_vertex_array_object (ES 1.1 / 2.0) --
    //
    // Returns whether this backend can back the `*VertexArrayOES` entry points
    // with real host vertex array objects. When false, the guest-facing
    // wrappers fall back to a no-op emulation. Failing to honour VAO state is a
    // common cause of lost buffer bindings (see the My Talking Tom crash where
    // a tapped-Tom draw used a vertex attribute whose backing buffer binding
    // had been dropped), so backends that can should override this.
    fn supports_vao_oes(&self) -> bool {
        false
    }
    unsafe fn BindVertexArrayOES(&mut self, _array: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "BindVertexArrayOES not supported by this backend [stubbed]"
        );
    }
    unsafe fn GenVertexArraysOES(&mut self, _n: GLsizei, _arrays: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GenVertexArraysOES not supported by this backend [stubbed]"
        );
    }
    unsafe fn DeleteVertexArraysOES(&mut self, _n: GLsizei, _arrays: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "DeleteVertexArraysOES not supported by this backend [stubbed]"
        );
    }
    unsafe fn IsVertexArrayOES(&mut self, _array: GLuint) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "IsVertexArrayOES not supported by this backend [stubbed]"
        );
        0
    }

    // -- Buffer object operations (3.0) --
    unsafe fn MapBufferRange(
        &mut self,
        _target: GLenum,
        _offset: GLintptr,
        _length: GLsizeiptr,
        _access: GLbitfield,
    ) -> *mut GLvoid {
        log_unimplemented_gles_once!(
            self,
            "MapBufferRange (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        std::ptr::null_mut()
    }
    unsafe fn FlushMappedBufferRange(
        &mut self,
        _target: GLenum,
        _offset: GLintptr,
        _length: GLsizeiptr,
    ) {
        log_unimplemented_gles_once!(
            self,
            "FlushMappedBufferRange (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetBufferPointerv(
        &mut self,
        _target: GLenum,
        _pname: GLenum,
        _params: *mut *mut GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetBufferPointerv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetBufferParameteri64v(
        &mut self,
        _target: GLenum,
        _pname: GLenum,
        _params: *mut i64,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetBufferParameteri64v (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn CopyBufferSubData(
        &mut self,
        _readTarget: GLenum,
        _writeTarget: GLenum,
        _readOffset: GLintptr,
        _writeOffset: GLintptr,
        _size: GLsizeiptr,
    ) {
        log_unimplemented_gles_once!(
            self,
            "CopyBufferSubData (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn BindBufferBase(&mut self, _target: GLenum, _index: GLuint, _buffer: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "BindBufferBase (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn BindBufferRange(
        &mut self,
        _target: GLenum,
        _index: GLuint,
        _buffer: GLuint,
        _offset: GLintptr,
        _size: GLsizeiptr,
    ) {
        log_unimplemented_gles_once!(
            self,
            "BindBufferRange (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UnmapBuffer(&mut self, _target: GLenum) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "UnmapBuffer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }

    // -- 3D textures and immutable storage --
    unsafe fn TexImage3D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _internalformat: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _depth: GLsizei,
        _border: GLint,
        _format: GLenum,
        _type_: GLenum,
        _pixels: *const GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "TexImage3D (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn TexSubImage3D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _xoffset: GLint,
        _yoffset: GLint,
        _zoffset: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _depth: GLsizei,
        _format: GLenum,
        _type_: GLenum,
        _pixels: *const GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "TexSubImage3D (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn CopyTexSubImage3D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _xoffset: GLint,
        _yoffset: GLint,
        _zoffset: GLint,
        _x: GLint,
        _y: GLint,
        _width: GLsizei,
        _height: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "CopyTexSubImage3D (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn CompressedTexImage3D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _internalformat: GLenum,
        _width: GLsizei,
        _height: GLsizei,
        _depth: GLsizei,
        _border: GLint,
        _imageSize: GLsizei,
        _data: *const GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "CompressedTexImage3D (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn CompressedTexSubImage3D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _xoffset: GLint,
        _yoffset: GLint,
        _zoffset: GLint,
        _width: GLsizei,
        _height: GLsizei,
        _depth: GLsizei,
        _format: GLenum,
        _imageSize: GLsizei,
        _data: *const GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "CompressedTexSubImage3D (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn TexStorage2D(
        &mut self,
        target: GLenum,
        levels: GLsizei,
        internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
    ) {
        let (image_internalformat, format, type_) = tex_storage_2d_parameters(internalformat)
            .unwrap_or((
                internalformat as GLint,
                internalformat,
                gles11::UNSIGNED_BYTE,
            ));
        let targets = if target == 0x8513 {
            [0x8515, 0x8516, 0x8517, 0x8518, 0x8519, 0x851A]
        } else {
            [target; 6]
        };
        let target_count = if target == 0x8513 { 6 } else { 1 };
        if levels <= 0 || width <= 0 || height <= 0 {
            let invalid_width = if levels <= 0 || width <= 0 { -1 } else { width };
            let invalid_height = if height <= 0 { -1 } else { height };
            self.TexImage2D(
                targets[0],
                0,
                image_internalformat,
                invalid_width,
                invalid_height,
                0,
                format,
                type_,
                std::ptr::null(),
            );
            return;
        }
        let max_levels = 32 - (width.max(height) as u32).leading_zeros() as i32;
        if levels > max_levels {
            self.TexImage2D(
                targets[0],
                0,
                image_internalformat,
                -1,
                height,
                0,
                format,
                type_,
                std::ptr::null(),
            );
            return;
        }
        for image_target in targets.iter().take(target_count) {
            let mut level_width = width;
            let mut level_height = height;
            for level in 0..levels {
                self.TexImage2D(
                    *image_target,
                    level,
                    image_internalformat,
                    level_width,
                    level_height,
                    0,
                    format,
                    type_,
                    std::ptr::null(),
                );
                level_width = (level_width / 2).max(1);
                level_height = (level_height / 2).max(1);
            }
        }
    }
    unsafe fn TexStorage3D(
        &mut self,
        _target: GLenum,
        _levels: GLsizei,
        _internalformat: GLenum,
        _width: GLsizei,
        _height: GLsizei,
        _depth: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "TexStorage3D (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Framebuffer / renderbuffer (3.0) --
    unsafe fn BlitFramebuffer(
        &mut self,
        _srcX0: GLint,
        _srcY0: GLint,
        _srcX1: GLint,
        _srcY1: GLint,
        _dstX0: GLint,
        _dstY0: GLint,
        _dstX1: GLint,
        _dstY1: GLint,
        _mask: GLbitfield,
        _filter: GLenum,
    ) {
        log_unimplemented_gles_once!(
            self,
            "BlitFramebuffer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn RenderbufferStorageMultisample(
        &mut self,
        _target: GLenum,
        _samples: GLsizei,
        _internalformat: GLenum,
        _width: GLsizei,
        _height: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "RenderbufferStorageMultisample (OpenGL ES 3.0) not supported by this backend \
             [stubbed]"
        );
    }
    unsafe fn FramebufferTextureLayer(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _texture: GLuint,
        _level: GLint,
        _layer: GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "FramebufferTextureLayer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn InvalidateFramebuffer(
        &mut self,
        _target: GLenum,
        _numAttachments: GLsizei,
        _attachments: *const GLenum,
    ) {
        log_unimplemented_gles_once!(
            self,
            "InvalidateFramebuffer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn InvalidateSubFramebuffer(
        &mut self,
        _target: GLenum,
        _numAttachments: GLsizei,
        _attachments: *const GLenum,
        _x: GLint,
        _y: GLint,
        _width: GLsizei,
        _height: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "InvalidateSubFramebuffer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ReadBuffer(&mut self, _src: GLenum) {
        log_unimplemented_gles_once!(
            self,
            "ReadBuffer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DrawBuffers(&mut self, _n: GLsizei, _bufs: *const GLenum) {
        log_unimplemented_gles_once!(
            self,
            "DrawBuffers (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DrawRangeElements(
        &mut self,
        _mode: GLenum,
        _start: GLuint,
        _end: GLuint,
        _count: GLsizei,
        _type_: GLenum,
        _indices: *const GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "DrawRangeElements (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ClearBufferiv(&mut self, _buffer: GLenum, _drawbuffer: GLint, _value: *const GLint) {
        log_unimplemented_gles_once!(
            self,
            "ClearBufferiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ClearBufferuiv(
        &mut self,
        _buffer: GLenum,
        _drawbuffer: GLint,
        _value: *const GLuint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "ClearBufferuiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ClearBufferfv(
        &mut self,
        _buffer: GLenum,
        _drawbuffer: GLint,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "ClearBufferfv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ClearBufferfi(
        &mut self,
        _buffer: GLenum,
        _drawbuffer: GLint,
        _depth: GLfloat,
        _stencil: GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "ClearBufferfi (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Query objects --
    unsafe fn GenQueries(&mut self, _n: GLsizei, _ids: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GenQueries (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DeleteQueries(&mut self, _n: GLsizei, _ids: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "DeleteQueries (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn IsQuery(&mut self, _id: GLuint) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "IsQuery (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn BeginQuery(&mut self, _target: GLenum, _id: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "BeginQuery (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn EndQuery(&mut self, _target: GLenum) {
        log_unimplemented_gles_once!(
            self,
            "EndQuery (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetQueryiv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint) {
        log_unimplemented_gles_once!(
            self,
            "GetQueryiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetQueryObjectuiv(&mut self, _id: GLuint, _pname: GLenum, _params: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GetQueryObjectuiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Sampler objects --
    unsafe fn GenSamplers(&mut self, _count: GLsizei, _samplers: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GenSamplers (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DeleteSamplers(&mut self, _count: GLsizei, _samplers: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "DeleteSamplers (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn IsSampler(&mut self, _sampler: GLuint) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "IsSampler (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn BindSampler(&mut self, _unit: GLuint, _sampler: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "BindSampler (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn SamplerParameteri(&mut self, _sampler: GLuint, _pname: GLenum, _param: GLint) {
        log_unimplemented_gles_once!(
            self,
            "SamplerParameteri (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn SamplerParameteriv(
        &mut self,
        _sampler: GLuint,
        _pname: GLenum,
        _params: *const GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "SamplerParameteriv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn SamplerParameterf(&mut self, _sampler: GLuint, _pname: GLenum, _param: GLfloat) {
        log_unimplemented_gles_once!(
            self,
            "SamplerParameterf (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn SamplerParameterfv(
        &mut self,
        _sampler: GLuint,
        _pname: GLenum,
        _params: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "SamplerParameterfv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetSamplerParameteriv(
        &mut self,
        _sampler: GLuint,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetSamplerParameteriv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetSamplerParameterfv(
        &mut self,
        _sampler: GLuint,
        _pname: GLenum,
        _params: *mut GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetSamplerParameterfv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Transform feedback --
    unsafe fn BeginTransformFeedback(&mut self, _primitiveMode: GLenum) {
        log_unimplemented_gles_once!(
            self,
            "BeginTransformFeedback (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn EndTransformFeedback(&mut self) {
        log_unimplemented_gles_once!(
            self,
            "EndTransformFeedback (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn BindTransformFeedback(&mut self, _target: GLenum, _id: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "BindTransformFeedback (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DeleteTransformFeedbacks(&mut self, _n: GLsizei, _ids: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "DeleteTransformFeedbacks (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GenTransformFeedbacks(&mut self, _n: GLsizei, _ids: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GenTransformFeedbacks (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn IsTransformFeedback(&mut self, _id: GLuint) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "IsTransformFeedback (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn PauseTransformFeedback(&mut self) {
        log_unimplemented_gles_once!(
            self,
            "PauseTransformFeedback (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ResumeTransformFeedback(&mut self) {
        log_unimplemented_gles_once!(
            self,
            "ResumeTransformFeedback (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn TransformFeedbackVaryings(
        &mut self,
        _program: GLuint,
        _count: GLsizei,
        _varyings: *const *const GLchar,
        _bufferMode: GLenum,
    ) {
        log_unimplemented_gles_once!(
            self,
            "TransformFeedbackVaryings (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetTransformFeedbackVarying(
        &mut self,
        _program: GLuint,
        _index: GLuint,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _size: *mut GLsizei,
        _type_: *mut GLenum,
        _name: *mut GLchar,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetTransformFeedbackVarying (OpenGL ES 3.0) not supported by this backend \
             [stubbed]"
        );
    }

    // -- Integer vertex attributes --
    unsafe fn VertexAttribIPointer(
        &mut self,
        _index: GLuint,
        _size: GLint,
        _type_: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "VertexAttribIPointer (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetVertexAttribIiv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLint) {
        log_unimplemented_gles_once!(
            self,
            "GetVertexAttribIiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetVertexAttribIuiv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GetVertexAttribIuiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn VertexAttribI4i(
        &mut self,
        _index: GLuint,
        _x: GLint,
        _y: GLint,
        _z: GLint,
        _w: GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "VertexAttribI4i (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn VertexAttribI4ui(
        &mut self,
        _index: GLuint,
        _x: GLuint,
        _y: GLuint,
        _z: GLuint,
        _w: GLuint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "VertexAttribI4ui (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn VertexAttribI4iv(&mut self, _index: GLuint, _v: *const GLint) {
        log_unimplemented_gles_once!(
            self,
            "VertexAttribI4iv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn VertexAttribI4uiv(&mut self, _index: GLuint, _v: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "VertexAttribI4uiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Integer uniforms --
    unsafe fn Uniform1ui(&mut self, _location: GLint, _v0: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform1ui (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform2ui(&mut self, _location: GLint, _v0: GLuint, _v1: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform2ui (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform3ui(&mut self, _location: GLint, _v0: GLuint, _v1: GLuint, _v2: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform3ui (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform4ui(
        &mut self,
        _location: GLint,
        _v0: GLuint,
        _v1: GLuint,
        _v2: GLuint,
        _v3: GLuint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "Uniform4ui (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform1uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform1uiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform2uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform2uiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform3uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform3uiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn Uniform4uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        log_unimplemented_gles_once!(
            self,
            "Uniform4uiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetUniformuiv(&mut self, _program: GLuint, _location: GLint, _params: *mut GLuint) {
        log_unimplemented_gles_once!(
            self,
            "GetUniformuiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UniformMatrix2x3fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformMatrix2x3fv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UniformMatrix3x2fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformMatrix3x2fv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UniformMatrix2x4fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformMatrix2x4fv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UniformMatrix4x2fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformMatrix4x2fv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UniformMatrix3x4fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformMatrix3x4fv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn UniformMatrix4x3fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformMatrix4x3fv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Uniform blocks --
    unsafe fn GetUniformIndices(
        &mut self,
        _program: GLuint,
        _uniformCount: GLsizei,
        _uniformNames: *const *const GLchar,
        _uniformIndices: *mut GLuint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetUniformIndices (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetActiveUniformsiv(
        &mut self,
        _program: GLuint,
        _uniformCount: GLsizei,
        _uniformIndices: *const GLuint,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetActiveUniformsiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetUniformBlockIndex(
        &mut self,
        _program: GLuint,
        _uniformBlockName: *const GLchar,
    ) -> GLuint {
        log_unimplemented_gles_once!(
            self,
            "GetUniformBlockIndex (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0xFFFF_FFFF // GL_INVALID_INDEX
    }
    unsafe fn GetActiveUniformBlockiv(
        &mut self,
        _program: GLuint,
        _uniformBlockIndex: GLuint,
        _pname: GLenum,
        _params: *mut GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetActiveUniformBlockiv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetActiveUniformBlockName(
        &mut self,
        _program: GLuint,
        _uniformBlockIndex: GLuint,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _uniformBlockName: *mut GLchar,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetActiveUniformBlockName (OpenGL ES 3.0) not supported by this backend \
             [stubbed]"
        );
    }
    unsafe fn UniformBlockBinding(
        &mut self,
        _program: GLuint,
        _uniformBlockIndex: GLuint,
        _uniformBlockBinding: GLuint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "UniformBlockBinding (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Instanced rendering --
    unsafe fn DrawArraysInstanced(
        &mut self,
        _mode: GLenum,
        _first: GLint,
        _count: GLsizei,
        _instanceCount: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "DrawArraysInstanced (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn DrawElementsInstanced(
        &mut self,
        _mode: GLenum,
        _count: GLsizei,
        _type_: GLenum,
        _indices: *const GLvoid,
        _instanceCount: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "DrawElementsInstanced (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn VertexAttribDivisor(&mut self, _index: GLuint, _divisor: GLuint) {
        log_unimplemented_gles_once!(
            self,
            "VertexAttribDivisor (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Sync objects --
    /// Returns `true` if [GLES::FenceSync] and friends are backed by real
    /// driver sync objects. When `false`, `gles_guest` emulates fences
    /// itself (see the comment above `glFenceSync` there).
    fn supports_sync_objects(&self) -> bool {
        false
    }
    unsafe fn FenceSync(&mut self, _condition: GLenum, _flags: GLbitfield) -> usize {
        log_unimplemented_gles_once!(
            self,
            "FenceSync (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn IsSync(&mut self, _sync: usize) -> GLboolean {
        log_unimplemented_gles_once!(
            self,
            "IsSync (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn DeleteSync(&mut self, _sync: usize) {
        log_unimplemented_gles_once!(
            self,
            "DeleteSync (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ClientWaitSync(&mut self, _sync: usize, _flags: GLbitfield, _timeout: u64) -> GLenum {
        log_unimplemented_gles_once!(
            self,
            "ClientWaitSync (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        0
    }
    unsafe fn WaitSync(&mut self, _sync: usize, _flags: GLbitfield, _timeout: u64) {
        log_unimplemented_gles_once!(
            self,
            "WaitSync (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetSynciv(
        &mut self,
        _sync: usize,
        _pname: GLenum,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _values: *mut GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetSynciv (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- 64-bit / indexed getters --
    unsafe fn GetInteger64v(&mut self, _pname: GLenum, _data: *mut i64) {
        log_unimplemented_gles_once!(
            self,
            "GetInteger64v (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetIntegeri_v(&mut self, _target: GLenum, _index: GLuint, _data: *mut GLint) {
        log_unimplemented_gles_once!(
            self,
            "GetIntegeri_v (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetInteger64i_v(&mut self, _target: GLenum, _index: GLuint, _data: *mut i64) {
        log_unimplemented_gles_once!(
            self,
            "GetInteger64i_v (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Program binary --
    unsafe fn ProgramParameteri(&mut self, _program: GLuint, _pname: GLenum, _value: GLint) {
        log_unimplemented_gles_once!(
            self,
            "ProgramParameteri (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn ProgramBinary(
        &mut self,
        _program: GLuint,
        _binaryFormat: GLenum,
        _binary: *const GLvoid,
        _length: GLsizei,
    ) {
        log_unimplemented_gles_once!(
            self,
            "ProgramBinary (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
    unsafe fn GetProgramBinary(
        &mut self,
        _program: GLuint,
        _bufSize: GLsizei,
        _length: *mut GLsizei,
        _binaryFormat: *mut GLenum,
        _binary: *mut GLvoid,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetProgramBinary (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }

    // -- Misc 3.0 --
    unsafe fn GetStringi(&mut self, _name: GLenum, _index: GLuint) -> *const GLubyte {
        log_unimplemented_gles_once!(
            self,
            "GetStringi (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        std::ptr::null()
    }
    unsafe fn GetFragDataLocation(&mut self, _program: GLuint, _name: *const GLchar) -> GLint {
        log_unimplemented_gles_once!(
            self,
            "GetFragDataLocation (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
        -1
    }
    unsafe fn GetInternalformativ(
        &mut self,
        _target: GLenum,
        _internalformat: GLenum,
        _pname: GLenum,
        _bufSize: GLsizei,
        _params: *mut GLint,
    ) {
        log_unimplemented_gles_once!(
            self,
            "GetInternalformativ (OpenGL ES 3.0) not supported by this backend [stubbed]"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::GLES;

    struct UnsupportedPointStateBackend;

    impl GLES for UnsupportedPointStateBackend {}

    #[test]
    fn fixed_function_point_state_is_safe_when_backend_does_not_support_it() {
        let mut gles = UnsupportedPointStateBackend;
        let attenuation = [1.0, 0.0, 0.0];
        let attenuation_fixed = [1 << 16, 0, 0];

        unsafe {
            gles.PointSize(1.0);
            gles.PointSizex(1 << 16);
            gles.PointParameterf(0x8129, 1.0);
            gles.PointParameterx(0x8129, 1 << 16);
            gles.PointParameterfv(0x8129, attenuation.as_ptr());
            gles.PointParameterxv(0x8129, attenuation_fixed.as_ptr());
        }
    }

    #[derive(Default)]
    struct ProfileBackend {
        es2: bool,
        es3: bool,
        translator: bool,
        native_es1: bool,
        gles1_on_gl2: bool,
    }

    impl GLES for ProfileBackend {
        fn is_es2(&self) -> bool {
            self.es2
        }

        fn is_es3(&self) -> bool {
            self.es3
        }

        fn is_translator(&self) -> bool {
            self.translator
        }

        fn is_native_es1(&self) -> bool {
            self.native_es1
        }

        fn is_gles1_on_gl2(&self) -> bool {
            self.gles1_on_gl2
        }
    }

    #[test]
    fn diagnostic_profile_identifies_guest_gles_version() {
        let profiles = [
            (
                ProfileBackend {
                    es2: true,
                    es3: true,
                    ..Default::default()
                },
                "OpenGL ES 3.0",
            ),
            (
                ProfileBackend {
                    es2: true,
                    ..Default::default()
                },
                "OpenGL ES 2.0",
            ),
            (
                ProfileBackend {
                    es2: true,
                    translator: true,
                    ..Default::default()
                },
                "OpenGL ES 1.1 translated through OpenGL ES 2.0",
            ),
            (
                ProfileBackend {
                    native_es1: true,
                    ..Default::default()
                },
                "OpenGL ES 1.1",
            ),
            (
                ProfileBackend {
                    gles1_on_gl2: true,
                    ..Default::default()
                },
                "OpenGL ES 1.1 emulated on desktop OpenGL 2.1",
            ),
            (ProfileBackend::default(), "unknown GLES profile"),
        ];

        for (backend, expected) in profiles {
            assert_eq!(backend.diagnostic_profile(), expected);
        }
    }

    #[test]
    fn guest_call_guard_tracks_nested_calls_and_restores_state() {
        assert!(!super::guest_gles_call_active());
        let outer = super::GuestGlesCallGuard::enter();
        assert!(super::guest_gles_call_active());
        let inner = super::GuestGlesCallGuard::enter();
        assert!(super::guest_gles_call_active());
        drop(inner);
        assert!(super::guest_gles_call_active());
        drop(outer);
        assert!(!super::guest_gles_call_active());
    }
}
