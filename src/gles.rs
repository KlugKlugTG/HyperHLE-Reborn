/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenGL ES abstraction and implementations.
//!
//! touchHLE uses OpenGL ES for several things. OpenGL ES is part of iPhone OS's
//! API surface and can be used by apps for rendering, so there must be an
//! implementation of it to expose to the app. Beyond that, there are various
//! internal uses for which any graphics API would work, but using the same one
//! makes things simpler:
//! - Presenting frames rendered by the app to the screen, with appropriate
//!   rotation and scaling.
//! - Drawing touchHLE's virtual cursor.
//! - Drawing the app's splash screen.
//! - Compositing the app's Core Animation layers (usually for UIKit views).
//!
//! touchHLE's OpenGL ES implementation consists of a series of layers. This
//! module contains the layers that aren't specific to a particular use:
//!
//! - [gles_generic] provides an abstraction over OpenGL ES implementations.
//! - Various modules provide implementations:
//!   - [gles1_native] passes through native OpenGL ES 1.1.
//!   - [gles1_on_gl2] provides an implementation of OpenGL ES 1.1 using OpenGL
//!     2.1 compatibility profile.
//!   - There might be more in future.
//! - [gles11_raw] provides raw bindings for OpenGL ES 1.1 generated from the
//!   Khronos API headers. **The function bindings are only for use within this
//!   module.** The constants and types can be used outside it, however.
//!   - [gl21compat_raw] is the same thing, but for OpenGL 2.1 compatibility
//!     profile, which can't be used outside this module at all.
//! - [present] provides utilities for presenting frames to the window using an
//!   abstract OpenGL ES implementation.
//!
//! In contrast, [crate::frameworks::opengles] is a layer specific to OpenGL
//! ES's role as a part of the iPhone OS API surface. It wraps [gles_generic] to
//! expose OpenGL ES to the guest app.
//!
//! Useful resources for OpenGL ES 1.1:
//! - [Reference pages](https://registry.khronos.org/OpenGL-Refpages/es1.1/xhtml/)
//! - [Specification](https://registry.khronos.org/OpenGL/specs/es/1.1/es_full_spec_1.1.pdf)
//! - Apple's [OpenGL ES Hardware Platform Guide for iOS](https://developer.apple.com/library/archive/documentation/OpenGLES/Conceptual/OpenGLESHardwarePlatformGuide_iOS/OpenGLESPlatforms/OpenGLESPlatforms.html)
//! - Extensions:
//!   - [OES_framebuffer_object](https://registry.khronos.org/OpenGL/extensions/OES/OES_framebuffer_object.txt)
//!   - [IMG_texture_compression_pvrtc](https://registry.khronos.org/OpenGL/extensions/IMG/IMG_texture_compression_pvrtc.txt)
//!   - [OES_compressed_paletted_texture](https://registry.khronos.org/OpenGL/extensions/OES/OES_compressed_paletted_texture.txt) (also incorporated into the main spec)
//!   - [OES_matrix_palette](https://registry.khronos.org/OpenGL/extensions/OES/OES_matrix_palette.txt)
//!   - [EXT_texture_format_BGRA8888](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_texture_format_BGRA8888.txt)
//!   - [OES_blend_subtract](https://registry.khronos.org/OpenGL/extensions/OES/OES_blend_subtract.txt)
//!
//! Useful resources for OpenGL 2.1:
//! - [Reference pages](https://registry.khronos.org/OpenGL-Refpages/gl2.1/)
//! - [Specification](https://registry.khronos.org/OpenGL/specs/gl/glspec21.pdf)
//! - Extensions:
//!   - [EXT_framebuffer_object](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_framebuffer_object.txt)
//!   - [ARB_matrix_palette](https://registry.khronos.org/OpenGL/extensions/ARB/ARB_matrix_palette.txt)
//!   - [ARB_vertex_blend](https://registry.khronos.org/OpenGL/extensions/ARB/ARB_vertex_blend.txt)
//!   - [EXT_blend_subtract](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_blend_subtract.txt)
//!
//! Useful resources for both:
//! - Extensions:
//!   - [EXT_texture_filter_anisotropic](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_texture_filter_anisotropic.txt)
//!   - [EXT_texture_lod_bias](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_texture_lod_bias.txt)

pub mod gles1_native;
pub mod gles1_on_gl2;
pub mod gles1_on_gles2;
pub mod gles2_glsl;
pub mod gles2_native;
pub mod gles2_on_gl3;
pub mod gles3_native;
pub mod gles3_on_gl3;
mod gles_generic;
pub mod present;
pub mod perf_overlay;
pub mod util;
use touchHLE_gl_bindings::gl21compat as gl21compat_raw;
use touchHLE_gl_bindings::gl33core as gl33core_raw;
pub use touchHLE_gl_bindings::gles11 as gles11_raw;
pub use touchHLE_gl_bindings::gles2 as gles2_raw;
pub use touchHLE_gl_bindings::gles30 as gles30_raw;
pub use touchHLE_gl_bindings::gles11::types::*;
pub use util::try_decode_pvrtc;

use crate::environment::Environment;
use crate::window::{GLContext, GLVersion};
use gles1_native::GLES1NativeContext;
use gles1_on_gl2::GLES1OnGL2Context;
use gles1_on_gles2::GLES1OnGLES2Context;
use gles2_native::GLES2NativeContext;
use gles2_on_gl3::GLES2OnGL3Context;
use gles3_native::GLES3NativeContext;
use gles3_on_gl3::GLES3OnGL3Context;
pub use gles_generic::GLESContext;
pub use gles_generic::GLES;
pub(crate) use gles_generic::GuestGlesCallGuard;

pub struct LoggingGLES<'a> {
    pub inner: Box<dyn GLES + 'a>,
    pub verbose: bool,
}

pub struct LoggingGLESContext {
    pub inner: Box<dyn GLESContext>,
    pub verbose: bool,
}

impl GLESContext for LoggingGLESContext {
    fn description() -> &'static str {
        "Logging wrapper for GLES context"
    }

    fn new(window: &mut crate::window::Window) -> Result<Self, String> {
        // This is a wrapper, so it's not created directly via `new`.
        // It's created by wrapping an existing context.
        Err("LoggingGLESContext cannot be created directly via new()".to_string())
    }

    fn make_current<'gl_ctx, 'win: 'gl_ctx>(
        &'gl_ctx mut self,
        window: &'win mut crate::window::Window,
    ) -> Box<dyn GLES + 'gl_ctx> {
        let gles = self.inner.make_current(window);
        Box::new(LoggingGLES {
            inner: gles,
            verbose: self.verbose,
        })
    }

    unsafe fn make_current_unchecked_for_window<'gl_ctx>(
        &'gl_ctx mut self,
        make_current_fn: &mut dyn FnMut(&GLContext),
        loader_fn: &mut dyn FnMut(&'static str) -> *const std::ffi::c_void,
    ) -> Box<dyn GLES + 'gl_ctx> {
        let gles = self.inner.make_current_unchecked_for_window(make_current_fn, loader_fn);
        Box::new(LoggingGLES {
            inner: gles,
            verbose: self.verbose,
        })
    }
}

impl<'a> GLES for LoggingGLES<'a> {
    fn is_native_es1(&self) -> bool {
        self.inner.is_native_es1()
    }
    fn is_gles1_on_gl2(&self) -> bool {
        self.inner.is_gles1_on_gl2()
    }
    // The logging wrapper must forward every capability query: profiles such
    // as ES 2.0 are selected from these, not from the raw GL strings.
    fn is_es2(&self) -> bool {
        self.inner.is_es2()
    }
    fn is_es3(&self) -> bool {
        self.inner.is_es3()
    }
    fn is_translator(&self) -> bool {
        self.inner.is_translator()
    }
    fn pvrtc_native(&self) -> bool {
        self.inner.pvrtc_native()
    }
    fn discard_ext_supported(&self) -> bool {
        self.inner.discard_ext_supported()
    }
    unsafe fn DiscardFramebufferEXT(
        &mut self,
        target: GLenum,
        num_attachments: GLsizei,
        attachments: *const GLenum,
    ) -> bool {
        self.inner
            .DiscardFramebufferEXT(target, num_attachments, attachments)
    }
    unsafe fn driver_description(&self) -> String {
        self.inner.driver_description()
    }

    unsafe fn GetError(&mut self) -> GLenum {
        let err = self.inner.GetError();
        if self.verbose {
            log_file_only!("GL Error: {:#x}", err);
        }
        err
    }

    unsafe fn Clear(&mut self, mask: GLbitfield) {
        if self.verbose {
            log_file_only!("glClear(mask={:#x})", mask);
        }
        self.inner.Clear(mask);
    }

    unsafe fn Viewport(&mut self, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
        if self.verbose {
            log_file_only!("glViewport({}, {}, {}, {})", x, y, width, height);
        }
        self.inner.Viewport(x, y, width, height);
    }

    unsafe fn DrawArrays(&mut self, mode: GLenum, first: GLint, count: GLsizei) {
        if self.verbose {
            log_file_only!("glDrawArrays(mode={:#x}, first={}, count={})", mode, first, count);
        }
        self.inner.DrawArrays(mode, first, count);
    }

    unsafe fn DrawElements(&mut self, mode: GLenum, count: GLsizei, type_: GLenum, indices: *const GLvoid) {
        if self.verbose {
            log_file_only!("glDrawElements(mode={:#x}, count={}, type={:#x})", mode, count, type_);
        }
        self.inner.DrawElements(mode, count, type_, indices);
    }

    unsafe fn BindFramebuffer(&mut self, target: GLenum, framebuffer: GLuint) {
        if self.verbose {
            log_file_only!("glBindFramebuffer(target={:#x}, fb={})", target, framebuffer);
        }
        self.inner.BindFramebuffer(target, framebuffer);
    }

    unsafe fn FramebufferRenderbuffer(&mut self, target: GLenum, attachment: GLenum, renderbuffertarget: GLenum, renderbuffer: GLuint) {
        if self.verbose {
            log_file_only!("glFramebufferRenderbuffer(target={:#x}, attach={:#x}, rb_target={:#x}, rb={})", target, attachment, renderbuffertarget, renderbuffer);
        }
        self.inner.FramebufferRenderbuffer(target, attachment, renderbuffertarget, renderbuffer);
    }

    unsafe fn TexImage2D(
        &mut self,
        target: GLenum,
        level: GLint,
        internalformat: GLint,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
        format: GLenum,
        type_: GLenum,
        pixels: *const GLvoid,
    ) {
        if self.verbose {
            log_file_only!("glTexImage2D(target={:#x}, level={}, int_fmt={:#x}, size={}x{}, format={:#x}, type={:#x})", target, level, internalformat, width, height, format, type_);
        }
        self.inner.TexImage2D(target, level, internalformat, width, height, border, format, type_, pixels);
    }

    unsafe fn BindTexture(&mut self, target: GLenum, texture: GLuint) {
        if self.verbose {
            log_file_only!("glBindTexture(target={:#x}, tex={})", target, texture);
        }
        self.inner.BindTexture(target, texture);
    }

    unsafe fn ReadPixels(
        &mut self,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *mut GLvoid,
    ) {
        if self.verbose {
            log_file_only!("glReadPixels({}, {}, {}, {}, format={:#x}, type={:#x})", x, y, width, height, format, type_);
        }
        self.inner.ReadPixels(x, y, width, height, format, type_, pixels);
    }

    unsafe fn GetIntegerv(&mut self, pname: GLenum, params: *mut GLint) {
        if self.verbose {
            log_file_only!("glGetIntegerv(pname={:#x})", pname);
        }
        self.inner.GetIntegerv(pname, params);
    }

    unsafe fn Finish(&mut self) {
        if self.verbose {
            log_file_only!("glFinish()");
        }
        self.inner.Finish();
    }

    unsafe fn Flush(&mut self) {
        if self.verbose {
            log_file_only!("glFlush()");
        }
        self.inner.Flush();
    }

    // --- Forwarding the rest of the methods to avoid panics ---

    unsafe fn ClearColor(&mut self, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) {
        self.inner.ClearColor(r, g, b, a);
    }

    unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
        self.inner.BindBuffer(target, buffer);
    }

    unsafe fn EnableClientState(&mut self, array: GLenum) {
        self.inner.EnableClientState(array);
    }

    unsafe fn VertexPointer(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        self.inner.VertexPointer(size, type_, stride, pointer);
    }

    unsafe fn TexCoordPointer(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        self.inner.TexCoordPointer(size, type_, stride, pointer);
    }

    unsafe fn MatrixMode(&mut self, mode: GLenum) {
        self.inner.MatrixMode(mode);
    }

    unsafe fn LoadMatrixf(&mut self, m: *const GLfloat) {
        self.inner.LoadMatrixf(m);
    }

    unsafe fn Enable(&mut self, cap: GLenum) {
        self.inner.Enable(cap);
    }

    unsafe fn LoadIdentity(&mut self) {
        self.inner.LoadIdentity();
    }

    unsafe fn DisableClientState(&mut self, array: GLenum) {
        self.inner.DisableClientState(array);
    }

    unsafe fn Disable(&mut self, cap: GLenum) {
        self.inner.Disable(cap);
    }

    unsafe fn BlendFunc(&mut self, sfactor: GLenum, dfactor: GLenum) {
        self.inner.BlendFunc(sfactor, dfactor);
    }

    unsafe fn Color4f(&mut self, r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat) {
        self.inner.Color4f(r, g, b, a);
    }

    unsafe fn GenTextures(&mut self, n: GLsizei, textures: *mut GLuint) {
        self.inner.GenTextures(n, textures);
    }

    unsafe fn TexParameteri(&mut self, target: GLenum, pname: GLenum, param: GLint) {
        self.inner.TexParameteri(target, pname, param);
    }

    unsafe fn PushMatrix(&mut self) {
        self.inner.PushMatrix();
    }

    unsafe fn PopMatrix(&mut self) {
        self.inner.PopMatrix();
    }

    unsafe fn Orthof(&mut self, left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) {
        self.inner.Orthof(left, right, bottom, top, near, far);
    }

    unsafe fn IsEnabled(&mut self, cap: GLenum) -> GLboolean {
        self.inner.IsEnabled(cap)
    }

    unsafe fn ClientActiveTexture(&mut self, texture: GLenum) {
        self.inner.ClientActiveTexture(texture);
    }

    unsafe fn GetBooleanv(&mut self, pname: GLenum, params: *mut GLboolean) {
        self.inner.GetBooleanv(pname, params);
    }

    unsafe fn GetFloatv(&mut self, pname: GLenum, params: *mut GLfloat) {
        self.inner.GetFloatv(pname, params);
    }

    unsafe fn GetFixedv(&mut self, pname: GLenum, params: *mut GLfixed) {
        self.inner.GetFixedv(pname, params);
    }

    unsafe fn GetTexEnviv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        self.inner.GetTexEnviv(target, pname, params);
    }

    unsafe fn GetTexEnvfv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfloat) {
        self.inner.GetTexEnvfv(target, pname, params);
    }

    unsafe fn GetTexEnvxv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfixed) {
        self.inner.GetTexEnvxv(target, pname, params);
    }

    unsafe fn GetTexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        self.inner.GetTexParameteriv(target, pname, params);
    }

    unsafe fn GetTexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfloat) {
        self.inner.GetTexParameterfv(target, pname, params);
    }

    unsafe fn GetTexParameterxv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfixed) {
        self.inner.GetTexParameterxv(target, pname, params);
    }

    unsafe fn GetClipPlanef(&mut self, plane: GLenum, equation: *mut GLfloat) {
        self.inner.GetClipPlanef(plane, equation);
    }

    unsafe fn GetClipPlanex(&mut self, plane: GLenum, equation: *mut GLfixed) {
        self.inner.GetClipPlanex(plane, equation);
    }

    unsafe fn GetLightfv(&mut self, light: GLenum, pname: GLenum, params: *mut GLfloat) {
        self.inner.GetLightfv(light, pname, params);
    }

    unsafe fn GetLightxv(&mut self, light: GLenum, pname: GLenum, params: *mut GLfixed) {
        self.inner.GetLightxv(light, pname, params);
    }

    unsafe fn GetMaterialfv(&mut self, face: GLenum, pname: GLenum, params: *mut GLfloat) {
        self.inner.GetMaterialfv(face, pname, params);
    }

    unsafe fn GetMaterialxv(&mut self, face: GLenum, pname: GLenum, params: *mut GLfixed) {
        self.inner.GetMaterialxv(face, pname, params);
    }

    unsafe fn GetPointerv(&mut self, pname: GLenum, params: *mut *const GLvoid) {
        self.inner.GetPointerv(pname, params);
    }

    unsafe fn Hint(&mut self, target: GLenum, mode: GLenum) {
        self.inner.Hint(target, mode);
    }

    unsafe fn GetString(&mut self, name: GLenum) -> *const GLubyte {
        self.inner.GetString(name)
    }

    unsafe fn AlphaFunc(&mut self, func: GLenum, ref_: GLclampf) {
        self.inner.AlphaFunc(func, ref_);
    }

    unsafe fn AlphaFuncx(&mut self, func: GLenum, ref_: GLclampx) {
        self.inner.AlphaFuncx(func, ref_);
    }

    unsafe fn BlendEquationOES(&mut self, mode: GLenum) {
        self.inner.BlendEquationOES(mode);
    }

    unsafe fn ColorMask(&mut self, red: GLboolean, green: GLboolean, blue: GLboolean, alpha: GLboolean) {
        self.inner.ColorMask(red, green, blue, alpha);
    }

    unsafe fn ClipPlanef(&mut self, plane: GLenum, equation: *const GLfloat) {
        self.inner.ClipPlanef(plane, equation);
    }

    unsafe fn ClipPlanex(&mut self, plane: GLenum, equation: *const GLfixed) {
        self.inner.ClipPlanex(plane, equation);
    }

    unsafe fn CullFace(&mut self, mode: GLenum) {
        self.inner.CullFace(mode);
    }

    unsafe fn DepthFunc(&mut self, func: GLenum) {
        self.inner.DepthFunc(func);
    }

    unsafe fn DepthMask(&mut self, flag: GLboolean) {
        self.inner.DepthMask(flag);
    }

    unsafe fn DepthRangef(&mut self, near: GLclampf, far: GLclampf) {
        self.inner.DepthRangef(near, far);
    }

    unsafe fn DepthRangex(&mut self, near: GLclampx, far: GLclampx) {
        self.inner.DepthRangex(near, far);
    }

    unsafe fn FrontFace(&mut self, mode: GLenum) {
        self.inner.FrontFace(mode);
    }

    unsafe fn PolygonOffset(&mut self, factor: GLfloat, units: GLfloat) {
        self.inner.PolygonOffset(factor, units);
    }

    unsafe fn PolygonOffsetx(&mut self, factor: GLfixed, units: GLfixed) {
        self.inner.PolygonOffsetx(factor, units);
    }

    unsafe fn SampleCoverage(&mut self, value: GLclampf, invert: GLboolean) {
        self.inner.SampleCoverage(value, invert);
    }

    unsafe fn SampleCoveragex(&mut self, value: GLclampx, invert: GLboolean) {
        self.inner.SampleCoveragex(value, invert);
    }

    unsafe fn ShadeModel(&mut self, mode: GLenum) {
        self.inner.ShadeModel(mode);
    }

    unsafe fn Scissor(&mut self, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
        self.inner.Scissor(x, y, width, height);
    }

    unsafe fn LineWidth(&mut self, val: GLfloat) {
        self.inner.LineWidth(val);
    }

    unsafe fn LineWidthx(&mut self, val: GLfixed) {
        self.inner.LineWidthx(val);
    }

    unsafe fn StencilFunc(&mut self, func: GLenum, ref_: GLint, mask: GLuint) {
        self.inner.StencilFunc(func, ref_, mask);
    }

    unsafe fn StencilOp(&mut self, sfail: GLenum, dpfail: GLenum, dppass: GLenum) {
        self.inner.StencilOp(sfail, dpfail, dppass);
    }

    unsafe fn StencilMask(&mut self, mask: GLuint) {
        self.inner.StencilMask(mask);
    }

    unsafe fn LogicOp(&mut self, opcode: GLenum) {
        self.inner.LogicOp(opcode);
    }

    unsafe fn PointSize(&mut self, size: GLfloat) {
        self.inner.PointSize(size);
    }

    unsafe fn PointSizex(&mut self, size: GLfixed) {
        self.inner.PointSizex(size);
    }

    unsafe fn PointParameterf(&mut self, pname: GLenum, param: GLfloat) {
        self.inner.PointParameterf(pname, param);
    }

    unsafe fn PointParameterx(&mut self, pname: GLenum, param: GLfixed) {
        self.inner.PointParameterx(pname, param);
    }

    unsafe fn PointParameterfv(&mut self, pname: GLenum, params: *const GLfloat) {
        self.inner.PointParameterfv(pname, params);
    }

    unsafe fn PointParameterxv(&mut self, pname: GLenum, params: *const GLfixed) {
        self.inner.PointParameterxv(pname, params);
    }

    unsafe fn Fogf(&mut self, _pname: GLenum, _param: GLfloat) {
        self.inner.Fogf(_pname, _param);
    }
    unsafe fn Fogx(&mut self, _pname: GLenum, _param: GLfixed) {
        self.inner.Fogx(_pname, _param);
    }
    unsafe fn Fogfv(&mut self, _pname: GLenum, _params: *const GLfloat) {
        self.inner.Fogfv(_pname, _params);
    }
    unsafe fn Fogxv(&mut self, _pname: GLenum, _params: *const GLfixed) {
        self.inner.Fogxv(_pname, _params);
    }
    unsafe fn Lightf(&mut self, _light: GLenum, _pname: GLenum, _param: GLfloat) {
        self.inner.Lightf(_light, _pname, _param);
    }
    unsafe fn Lightx(&mut self, _light: GLenum, _pname: GLenum, _param: GLfixed) {
        self.inner.Lightx(_light, _pname, _param);
    }
    unsafe fn Lightfv(&mut self, _light: GLenum, _pname: GLenum, _params: *const GLfloat) {
        self.inner.Lightfv(_light, _pname, _params);
    }
    unsafe fn Lightxv(&mut self, _light: GLenum, _pname: GLenum, _params: *const GLfixed) {
        self.inner.Lightxv(_light, _pname, _params);
    }
    unsafe fn LightModelf(&mut self, _pname: GLenum, _param: GLfloat) {
        self.inner.LightModelf(_pname, _param);
    }
    unsafe fn LightModelx(&mut self, _pname: GLenum, _param: GLfixed) {
        self.inner.LightModelx(_pname, _param);
    }
    unsafe fn LightModelfv(&mut self, _pname: GLenum, _params: *const GLfloat) {
        self.inner.LightModelfv(_pname, _params);
    }
    unsafe fn LightModelxv(&mut self, _pname: GLenum, _params: *const GLfixed) {
        self.inner.LightModelxv(_pname, _params);
    }
    unsafe fn Materialf(&mut self, _face: GLenum, _pname: GLenum, _param: GLfloat) {
        self.inner.Materialf(_face, _pname, _param);
    }
    unsafe fn Materialx(&mut self, _face: GLenum, _pname: GLenum, _param: GLfixed) {
        self.inner.Materialx(_face, _pname, _param);
    }
    unsafe fn Materialfv(&mut self, _face: GLenum, _pname: GLenum, _params: *const GLfloat) {
        self.inner.Materialfv(_face, _pname, _params);
    }
    unsafe fn Materialxv(&mut self, _face: GLenum, _pname: GLenum, _params: *const GLfixed) {
        self.inner.Materialxv(_face, _pname, _params);
    }
    unsafe fn IsBuffer(&mut self, _buffer: GLuint) -> GLboolean {
        self.inner.IsBuffer(_buffer)
    }
    unsafe fn GenBuffers(&mut self, _n: GLsizei, _buffers: *mut GLuint) {
        self.inner.GenBuffers(_n, _buffers);
    }
    unsafe fn DeleteBuffers(&mut self, _n: GLsizei, _buffers: *const GLuint) {
        self.inner.DeleteBuffers(_n, _buffers);
    }
    unsafe fn BufferData( &mut self, _target: GLenum, _size: GLsizeiptr, _data: *const GLvoid, _usage: GLenum, ) {
        self.inner.BufferData(_target, _size, _data, _usage);
    }
    unsafe fn BufferSubData( &mut self, _target: GLenum, _offset: GLintptr, _size: GLsizeiptr, _data: *const GLvoid, ) {
        self.inner.BufferSubData(_target, _offset, _size, _data);
    }
    unsafe fn Color4x(&mut self, _red: GLfixed, _green: GLfixed, _blue: GLfixed, _alpha: GLfixed) {
        self.inner.Color4x(_red, _green, _blue, _alpha);
    }
    unsafe fn Color4ub(&mut self, _red: GLubyte, _green: GLubyte, _blue: GLubyte, _alpha: GLubyte) {
        self.inner.Color4ub(_red, _green, _blue, _alpha);
    }
    unsafe fn Normal3f(&mut self, _nx: GLfloat, _ny: GLfloat, _nz: GLfloat) {
        self.inner.Normal3f(_nx, _ny, _nz);
    }
    unsafe fn Normal3x(&mut self, _nx: GLfixed, _ny: GLfixed, _nz: GLfixed) {
        self.inner.Normal3x(_nx, _ny, _nz);
    }
    unsafe fn ColorPointer( &mut self, _size: GLint, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid, ) {
        self.inner.ColorPointer(_size, _type_, _stride, _pointer);
    }
    unsafe fn NormalPointer(&mut self, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid) {
        self.inner.NormalPointer(_type_, _stride, _pointer);
    }
    unsafe fn PointSizePointerOES( &mut self, type_: GLenum, stride: GLsizei, pointer: *const GLvoid, ) {
        self.inner.PointSizePointerOES(type_, stride, pointer);
    }
    unsafe fn CurrentPaletteMatrixOES(&mut self, _matrixpaletteindex: GLuint) {
        self.inner.CurrentPaletteMatrixOES(_matrixpaletteindex);
    }
    unsafe fn LoadPaletteFromModelViewMatrixOES(&mut self) {
        self.inner.LoadPaletteFromModelViewMatrixOES();
    }
    unsafe fn MatrixIndexPointerOES( &mut self, _size: GLint, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid, ) {
        self.inner.MatrixIndexPointerOES(_size, _type_, _stride, _pointer);
    }
    unsafe fn WeightPointerOES( &mut self, _size: GLint, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid, ) {
        self.inner.WeightPointerOES(_size, _type_, _stride, _pointer);
    }
    unsafe fn DrawTexfOES( &mut self, x: GLfloat, y: GLfloat, z: GLfloat, width: GLfloat, height: GLfloat, ) {
        self.inner.DrawTexfOES(x, y, z, width, height);
    }
    unsafe fn ClearColorx( &mut self, _red: GLclampx, _green: GLclampx, _blue: GLclampx, _alpha: GLclampx, ) {
        self.inner.ClearColorx(_red, _green, _blue, _alpha);
    }
    unsafe fn ClearDepthf(&mut self, _depth: GLclampf) {
        self.inner.ClearDepthf(_depth);
    }
    unsafe fn ClearDepthx(&mut self, _depth: GLclampx) {
        self.inner.ClearDepthx(_depth);
    }
    unsafe fn ClearStencil(&mut self, _s: GLint) {
        self.inner.ClearStencil(_s);
    }
    unsafe fn PixelStorei(&mut self, _pname: GLenum, _param: GLint) {
        self.inner.PixelStorei(_pname, _param);
    }
    unsafe fn DeleteTextures(&mut self, _n: GLsizei, _textures: *const GLuint) {
        self.inner.DeleteTextures(_n, _textures);
    }
    unsafe fn ActiveTexture(&mut self, _texture: GLenum) {
        self.inner.ActiveTexture(_texture);
    }
    unsafe fn IsTexture(&mut self, _texture: GLuint) -> GLboolean {
        self.inner.IsTexture(_texture)
    }
    unsafe fn TexParameterf(&mut self, _target: GLenum, _pname: GLenum, _param: GLfloat) {
        self.inner.TexParameterf(_target, _pname, _param);
    }
    unsafe fn TexParameterx(&mut self, _target: GLenum, _pname: GLenum, _param: GLfixed) {
        self.inner.TexParameterx(_target, _pname, _param);
    }
    unsafe fn TexParameteriv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLint) {
        self.inner.TexParameteriv(_target, _pname, _params);
    }
    unsafe fn TexParameterfv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfloat) {
        self.inner.TexParameterfv(_target, _pname, _params);
    }
    unsafe fn TexParameterxv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfixed) {
        self.inner.TexParameterxv(_target, _pname, _params);
    }
    unsafe fn TexSubImage2D( &mut self, _target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _width: GLsizei, _height: GLsizei, _format: GLenum, _type_: GLenum, _pixels: *const GLvoid, ) {
        self.inner.TexSubImage2D(_target, _level, _xoffset, _yoffset, _width, _height, _format, _type_, _pixels);
    }
    unsafe fn CompressedTexImage2D( &mut self, _target: GLenum, _level: GLint, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, _border: GLint, _image_size: GLsizei, _data: *const GLvoid, ) {
        self.inner.CompressedTexImage2D(_target, _level, _internalformat, _width, _height, _border, _image_size, _data);
    }
    unsafe fn CompressedTexSubImage2D( &mut self, _target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _width: GLsizei, _height: GLsizei, _format: GLenum, _image_size: GLsizei, _data: *const GLvoid, ) {
        self.inner.CompressedTexSubImage2D(_target, _level, _xoffset, _yoffset, _width, _height, _format, _image_size, _data);
    }
    unsafe fn CopyTexImage2D( &mut self, _target: GLenum, _level: GLint, _internalformat: GLenum, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei, _border: GLint, ) {
        self.inner.CopyTexImage2D(_target, _level, _internalformat, _x, _y, _width, _height, _border);
    }
    unsafe fn CopyTexSubImage2D( &mut self, _target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei, ) {
        self.inner.CopyTexSubImage2D(_target, _level, _xoffset, _yoffset, _x, _y, _width, _height);
    }
    unsafe fn TexEnvf(&mut self, _target: GLenum, _pname: GLenum, _param: GLfloat) {
        self.inner.TexEnvf(_target, _pname, _param);
    }
    unsafe fn TexEnvx(&mut self, _target: GLenum, _pname: GLenum, _param: GLfixed) {
        self.inner.TexEnvx(_target, _pname, _param);
    }
    unsafe fn TexEnvi(&mut self, _target: GLenum, _pname: GLenum, _param: GLint) {
        self.inner.TexEnvi(_target, _pname, _param);
    }
    unsafe fn TexEnvfv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfloat) {
        self.inner.TexEnvfv(_target, _pname, _params);
    }
    unsafe fn TexEnvxv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLfixed) {
        self.inner.TexEnvxv(_target, _pname, _params);
    }
    unsafe fn TexEnviv(&mut self, _target: GLenum, _pname: GLenum, _params: *const GLint) {
        self.inner.TexEnviv(_target, _pname, _params);
    }
    unsafe fn DrawTexsOES(&mut self, x: i16, y: i16, z: i16, width: i16, height: i16) {
        self.inner.DrawTexsOES(x, y, z, width, height);
    }
    unsafe fn DrawTexiOES(&mut self, x: GLint, y: GLint, z: GLint, width: GLint, height: GLint) {
        self.inner.DrawTexiOES(x, y, z, width, height);
    }
    unsafe fn DrawTexxOES( &mut self, x: GLfixed, y: GLfixed, z: GLfixed, width: GLfixed, height: GLfixed, ) {
        self.inner.DrawTexxOES(x, y, z, width, height);
    }
    unsafe fn DrawTexsvOES(&mut self, coords: *const i16) {
        self.inner.DrawTexsvOES(coords);
    }
    unsafe fn DrawTexivOES(&mut self, coords: *const GLint) {
        self.inner.DrawTexivOES(coords);
    }
    unsafe fn DrawTexxvOES(&mut self, coords: *const GLfixed) {
        self.inner.DrawTexxvOES(coords);
    }
    unsafe fn DrawTexfvOES(&mut self, coords: *const GLfloat) {
        self.inner.DrawTexfvOES(coords);
    }
    unsafe fn MultiTexCoord4f( &mut self, _target: GLenum, _s: GLfloat, _t: GLfloat, _r: GLfloat, _q: GLfloat, ) {
        self.inner.MultiTexCoord4f(_target, _s, _t, _r, _q);
    }
    unsafe fn MultiTexCoord4x( &mut self, _target: GLenum, _s: GLfixed, _t: GLfixed, _r: GLfixed, _q: GLfixed, ) {
        self.inner.MultiTexCoord4x(_target, _s, _t, _r, _q);
    }
    unsafe fn LoadMatrixx(&mut self, _m: *const GLfixed) {
        self.inner.LoadMatrixx(_m);
    }
    unsafe fn MultMatrixf(&mut self, _m: *const GLfloat) {
        self.inner.MultMatrixf(_m);
    }
    unsafe fn MultMatrixx(&mut self, _m: *const GLfixed) {
        self.inner.MultMatrixx(_m);
    }
    unsafe fn Orthox( &mut self, _left: GLfixed, _right: GLfixed, _bottom: GLfixed, _top: GLfixed, _near: GLfixed, _far: GLfixed, ) {
        self.inner.Orthox(_left, _right, _bottom, _top, _near, _far);
    }
    unsafe fn Frustumf( &mut self, _left: GLfloat, _right: GLfloat, _bottom: GLfloat, _top: GLfloat, _near: GLfloat, _far: GLfloat, ) {
        self.inner.Frustumf(_left, _right, _bottom, _top, _near, _far);
    }
    unsafe fn Frustumx( &mut self, _left: GLfixed, _right: GLfixed, _bottom: GLfixed, _top: GLfixed, _near: GLfixed, _far: GLfixed, ) {
        self.inner.Frustumx(_left, _right, _bottom, _top, _near, _far);
    }
    unsafe fn Rotatef(&mut self, _angle: GLfloat, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        self.inner.Rotatef(_angle, _x, _y, _z);
    }
    unsafe fn Rotatex(&mut self, _angle: GLfixed, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
        self.inner.Rotatex(_angle, _x, _y, _z);
    }
    unsafe fn Scalef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        self.inner.Scalef(_x, _y, _z);
    }
    unsafe fn Scalex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
        self.inner.Scalex(_x, _y, _z);
    }
    unsafe fn Translatef(&mut self, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        self.inner.Translatef(_x, _y, _z);
    }
    unsafe fn Translatex(&mut self, _x: GLfixed, _y: GLfixed, _z: GLfixed) {
        self.inner.Translatex(_x, _y, _z);
    }
    unsafe fn GenFramebuffersOES(&mut self, _n: GLsizei, _framebuffers: *mut GLuint) {
        self.inner.GenFramebuffersOES(_n, _framebuffers);
    }
    unsafe fn GenRenderbuffersOES(&mut self, _n: GLsizei, _renderbuffers: *mut GLuint) {
        self.inner.GenRenderbuffersOES(_n, _renderbuffers);
    }
    unsafe fn IsFramebufferOES(&mut self, _framebuffer: GLuint) -> GLboolean {
        self.inner.IsFramebufferOES(_framebuffer)
    }
    unsafe fn IsRenderbufferOES(&mut self, _renderbuffer: GLuint) -> GLboolean {
        self.inner.IsRenderbufferOES(_renderbuffer)
    }
    unsafe fn BindFramebufferOES(&mut self, _target: GLenum, _framebuffer: GLuint) {
        self.inner.BindFramebufferOES(_target, _framebuffer);
    }
    unsafe fn BindRenderbufferOES(&mut self, _target: GLenum, _renderbuffer: GLuint) {
        self.inner.BindRenderbufferOES(_target, _renderbuffer);
    }
    unsafe fn RenderbufferStorageOES( &mut self, _target: GLenum, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, ) {
        self.inner.RenderbufferStorageOES(_target, _internalformat, _width, _height);
    }
    unsafe fn FramebufferRenderbufferOES( &mut self, _target: GLenum, _attachment: GLenum, _renderbuffertarget: GLenum, _renderbuffer: GLuint, ) {
        self.inner.FramebufferRenderbufferOES(_target, _attachment, _renderbuffertarget, _renderbuffer);
    }
    unsafe fn FramebufferTexture2DOES( &mut self, _target: GLenum, _attachment: GLenum, _textarget: GLenum, _texture: GLuint, _level: i32, ) {
        self.inner.FramebufferTexture2DOES(_target, _attachment, _textarget, _texture, _level);
    }
    unsafe fn GetFramebufferAttachmentParameterivOES( &mut self, _target: GLenum, _attachment: GLenum, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetFramebufferAttachmentParameterivOES(_target, _attachment, _pname, _params);
    }
    unsafe fn GetRenderbufferParameterivOES( &mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetRenderbufferParameterivOES(_target, _pname, _params);
    }
    unsafe fn CheckFramebufferStatusOES(&mut self, _target: GLenum) -> GLenum {
        self.inner.CheckFramebufferStatusOES(_target)
    }
    unsafe fn DeleteFramebuffersOES(&mut self, _n: GLsizei, _framebuffers: *const GLuint) {
        self.inner.DeleteFramebuffersOES(_n, _framebuffers);
    }
    unsafe fn DeleteRenderbuffersOES(&mut self, _n: GLsizei, _renderbuffers: *const GLuint) {
        self.inner.DeleteRenderbuffersOES(_n, _renderbuffers);
    }
    unsafe fn GenerateMipmapOES(&mut self, _target: GLenum) {
        self.inner.GenerateMipmapOES(_target);
    }
    unsafe fn RenderbufferStorageMultisampleAPPLE( &mut self, target: GLenum, samples: GLsizei, internalformat: GLenum, width: GLsizei, height: GLsizei, ) {
        self.inner.RenderbufferStorageMultisampleAPPLE(target, samples, internalformat, width, height);
    }
    unsafe fn ResolveMultisampleFramebufferAPPLE(&mut self) {
        self.inner.ResolveMultisampleFramebufferAPPLE();
    }
    unsafe fn GenFramebuffers(&mut self, _n: GLsizei, _framebuffers: *mut GLuint) {
        self.inner.GenFramebuffers(_n, _framebuffers);
    }
    unsafe fn GenRenderbuffers(&mut self, _n: GLsizei, _renderbuffers: *mut GLuint) {
        self.inner.GenRenderbuffers(_n, _renderbuffers);
    }
    unsafe fn IsFramebuffer(&mut self, _framebuffer: GLuint) -> GLboolean {
        self.inner.IsFramebuffer(_framebuffer)
    }
    unsafe fn IsRenderbuffer(&mut self, _renderbuffer: GLuint) -> GLboolean {
        self.inner.IsRenderbuffer(_renderbuffer)
    }
    unsafe fn BindRenderbuffer(&mut self, _target: GLenum, _renderbuffer: GLuint) {
        self.inner.BindRenderbuffer(_target, _renderbuffer);
    }
    unsafe fn RenderbufferStorage( &mut self, _target: GLenum, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, ) {
        self.inner.RenderbufferStorage(_target, _internalformat, _width, _height);
    }
    unsafe fn FramebufferTexture2D( &mut self, _target: GLenum, _attachment: GLenum, _textarget: GLenum, _texture: GLuint, _level: i32, ) {
        self.inner.FramebufferTexture2D(_target, _attachment, _textarget, _texture, _level);
    }
    unsafe fn CheckFramebufferStatus(&mut self, _target: GLenum) -> GLenum {
        self.inner.CheckFramebufferStatus(_target)
    }
    unsafe fn DeleteFramebuffers(&mut self, _n: GLsizei, _framebuffers: *const GLuint) {
        self.inner.DeleteFramebuffers(_n, _framebuffers);
    }
    unsafe fn DeleteRenderbuffers(&mut self, _n: GLsizei, _renderbuffers: *const GLuint) {
        self.inner.DeleteRenderbuffers(_n, _renderbuffers);
    }
    unsafe fn GenerateMipmap(&mut self, _target: GLenum) {
        self.inner.GenerateMipmap(_target);
    }
    unsafe fn GetFramebufferAttachmentParameteriv( &mut self, _target: GLenum, _attachment: GLenum, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetFramebufferAttachmentParameteriv(_target, _attachment, _pname, _params);
    }
    unsafe fn GetRenderbufferParameteriv( &mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetRenderbufferParameteriv(_target, _pname, _params);
    }
    unsafe fn GetBufferParameteriv( &mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetBufferParameteriv(_target, _pname, _params);
    }
    unsafe fn MapBufferOES(&mut self, _target: GLenum, _access: GLenum) -> *mut GLvoid {
        self.inner.MapBufferOES(_target, _access)
    }
    unsafe fn UnmapBufferOES(&mut self, _target: GLenum) -> GLboolean {
        self.inner.UnmapBufferOES(_target)
    }
    unsafe fn CreateShader(&mut self, _type_: GLenum) -> GLuint {
        self.inner.CreateShader(_type_)
    }
    unsafe fn DeleteShader(&mut self, _shader: GLuint) {
        self.inner.DeleteShader(_shader);
    }
    unsafe fn ShaderSource( &mut self, _shader: GLuint, _count: GLsizei, _string: *const *const GLchar, _length: *const GLint, ) {
        self.inner.ShaderSource(_shader, _count, _string, _length);
    }
    unsafe fn CompileShader(&mut self, _shader: GLuint) {
        self.inner.CompileShader(_shader);
    }
    unsafe fn GetShaderiv(&mut self, _shader: GLuint, _pname: GLenum, _params: *mut GLint) {
        self.inner.GetShaderiv(_shader, _pname, _params);
    }
    unsafe fn GetShaderInfoLog( &mut self, _shader: GLuint, _maxLength: GLsizei, _length: *mut GLsizei, _infoLog: *mut GLchar, ) {
        self.inner.GetShaderInfoLog(_shader, _maxLength, _length, _infoLog);
    }
    unsafe fn IsShader(&mut self, _shader: GLuint) -> GLboolean {
        self.inner.IsShader(_shader)
    }
    unsafe fn CreateProgram(&mut self) -> GLuint {
        self.inner.CreateProgram()
    }
    unsafe fn DeleteProgram(&mut self, _program: GLuint) {
        self.inner.DeleteProgram(_program);
    }
    unsafe fn AttachShader(&mut self, _program: GLuint, _shader: GLuint) {
        self.inner.AttachShader(_program, _shader);
    }
    unsafe fn DetachShader(&mut self, _program: GLuint, _shader: GLuint) {
        self.inner.DetachShader(_program, _shader);
    }
    unsafe fn LinkProgram(&mut self, _program: GLuint) {
        self.inner.LinkProgram(_program);
    }
    unsafe fn UseProgram(&mut self, _program: GLuint) {
        self.inner.UseProgram(_program);
    }
    unsafe fn GetProgramiv(&mut self, _program: GLuint, _pname: GLenum, _params: *mut GLint) {
        self.inner.GetProgramiv(_program, _pname, _params);
    }
    unsafe fn GetProgramInfoLog( &mut self, _program: GLuint, _maxLength: GLsizei, _length: *mut GLsizei, _infoLog: *mut GLchar, ) {
        self.inner.GetProgramInfoLog(_program, _maxLength, _length, _infoLog);
    }
    unsafe fn IsProgram(&mut self, _program: GLuint) -> GLboolean {
        self.inner.IsProgram(_program)
    }
    unsafe fn ValidateProgram(&mut self, _program: GLuint) {
        self.inner.ValidateProgram(_program);
    }
    unsafe fn BindAttribLocation( &mut self, _program: GLuint, _index: GLuint, _name: *const GLchar, ) {
        self.inner.BindAttribLocation(_program, _index, _name);
    }
    unsafe fn GetAttribLocation(&mut self, _program: GLuint, _name: *const GLchar) -> GLint {
        self.inner.GetAttribLocation(_program, _name)
    }
    unsafe fn GetUniformLocation(&mut self, _program: GLuint, _name: *const GLchar) -> GLint {
        self.inner.GetUniformLocation(_program, _name)
    }
    unsafe fn GetActiveAttrib( &mut self, _program: GLuint, _index: GLuint, _bufSize: GLsizei, _length: *mut GLsizei, _size: *mut GLint, _type_: *mut GLenum, _name: *mut GLchar, ) {
        self.inner.GetActiveAttrib(_program, _index, _bufSize, _length, _size, _type_, _name);
    }
    unsafe fn GetActiveUniform( &mut self, _program: GLuint, _index: GLuint, _bufSize: GLsizei, _length: *mut GLsizei, _size: *mut GLint, _type_: *mut GLenum, _name: *mut GLchar, ) {
        self.inner.GetActiveUniform(_program, _index, _bufSize, _length, _size, _type_, _name);
    }
    unsafe fn EnableVertexAttribArray(&mut self, _index: GLuint) {
        self.inner.EnableVertexAttribArray(_index);
    }
    unsafe fn DisableVertexAttribArray(&mut self, _index: GLuint) {
        self.inner.DisableVertexAttribArray(_index);
    }
    unsafe fn VertexAttribPointer( &mut self, _index: GLuint, _size: GLint, _type_: GLenum, _normalized: GLboolean, _stride: GLsizei, _pointer: *const GLvoid, ) {
        self.inner.VertexAttribPointer(_index, _size, _type_, _normalized, _stride, _pointer);
    }
    unsafe fn VertexAttrib1f(&mut self, _index: GLuint, _x: GLfloat) {
        self.inner.VertexAttrib1f(_index, _x);
    }
    unsafe fn VertexAttrib2f(&mut self, _index: GLuint, _x: GLfloat, _y: GLfloat) {
        self.inner.VertexAttrib2f(_index, _x, _y);
    }
    unsafe fn VertexAttrib3f(&mut self, _index: GLuint, _x: GLfloat, _y: GLfloat, _z: GLfloat) {
        self.inner.VertexAttrib3f(_index, _x, _y, _z);
    }
    unsafe fn VertexAttrib4f( &mut self, _index: GLuint, _x: GLfloat, _y: GLfloat, _z: GLfloat, _w: GLfloat, ) {
        self.inner.VertexAttrib4f(_index, _x, _y, _z, _w);
    }
    unsafe fn VertexAttrib1fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        self.inner.VertexAttrib1fv(_index, _v);
    }
    unsafe fn VertexAttrib2fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        self.inner.VertexAttrib2fv(_index, _v);
    }
    unsafe fn VertexAttrib3fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        self.inner.VertexAttrib3fv(_index, _v);
    }
    unsafe fn VertexAttrib4fv(&mut self, _index: GLuint, _v: *const GLfloat) {
        self.inner.VertexAttrib4fv(_index, _v);
    }
    unsafe fn Uniform1f(&mut self, _location: GLint, _v0: GLfloat) {
        self.inner.Uniform1f(_location, _v0);
    }
    unsafe fn Uniform2f(&mut self, _location: GLint, _v0: GLfloat, _v1: GLfloat) {
        self.inner.Uniform2f(_location, _v0, _v1);
    }
    unsafe fn Uniform3f(&mut self, _location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat) {
        self.inner.Uniform3f(_location, _v0, _v1, _v2);
    }
    unsafe fn Uniform4f( &mut self, _location: GLint, _v0: GLfloat, _v1: GLfloat, _v2: GLfloat, _v3: GLfloat, ) {
        self.inner.Uniform4f(_location, _v0, _v1, _v2, _v3);
    }
    unsafe fn Uniform1i(&mut self, _location: GLint, _v0: GLint) {
        self.inner.Uniform1i(_location, _v0);
    }
    unsafe fn Uniform2i(&mut self, _location: GLint, _v0: GLint, _v1: GLint) {
        self.inner.Uniform2i(_location, _v0, _v1);
    }
    unsafe fn Uniform3i(&mut self, _location: GLint, _v0: GLint, _v1: GLint, _v2: GLint) {
        self.inner.Uniform3i(_location, _v0, _v1, _v2);
    }
    unsafe fn Uniform4i( &mut self, _location: GLint, _v0: GLint, _v1: GLint, _v2: GLint, _v3: GLint, ) {
        self.inner.Uniform4i(_location, _v0, _v1, _v2, _v3);
    }
    unsafe fn Uniform1fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        self.inner.Uniform1fv(_location, _count, _value);
    }
    unsafe fn Uniform2fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        self.inner.Uniform2fv(_location, _count, _value);
    }
    unsafe fn Uniform3fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        self.inner.Uniform3fv(_location, _count, _value);
    }
    unsafe fn Uniform4fv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLfloat) {
        self.inner.Uniform4fv(_location, _count, _value);
    }
    unsafe fn Uniform1iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        self.inner.Uniform1iv(_location, _count, _value);
    }
    unsafe fn Uniform2iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        self.inner.Uniform2iv(_location, _count, _value);
    }
    unsafe fn Uniform3iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        self.inner.Uniform3iv(_location, _count, _value);
    }
    unsafe fn Uniform4iv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLint) {
        self.inner.Uniform4iv(_location, _count, _value);
    }
    unsafe fn UniformMatrix2fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix2fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix3fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix3fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix4fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix4fv(_location, _count, _transpose, _value);
    }
    unsafe fn BlendColor(&mut self, _r: GLclampf, _g: GLclampf, _b: GLclampf, _a: GLclampf) {
        self.inner.BlendColor(_r, _g, _b, _a);
    }
    unsafe fn BlendEquation(&mut self, _mode: GLenum) {
        self.inner.BlendEquation(_mode);
    }
    unsafe fn BlendEquationSeparate(&mut self, _modeRGB: GLenum, _modeAlpha: GLenum) {
        self.inner.BlendEquationSeparate(_modeRGB, _modeAlpha);
    }
    unsafe fn BlendFuncSeparate( &mut self, _srcRGB: GLenum, _dstRGB: GLenum, _srcAlpha: GLenum, _dstAlpha: GLenum, ) {
        self.inner.BlendFuncSeparate(_srcRGB, _dstRGB, _srcAlpha, _dstAlpha);
    }
    unsafe fn StencilFuncSeparate( &mut self, _face: GLenum, _func: GLenum, _ref_: GLint, _mask: GLuint, ) {
        self.inner.StencilFuncSeparate(_face, _func, _ref_, _mask);
    }
    unsafe fn StencilOpSeparate( &mut self, _face: GLenum, _sfail: GLenum, _dpfail: GLenum, _dppass: GLenum, ) {
        self.inner.StencilOpSeparate(_face, _sfail, _dpfail, _dppass);
    }
    unsafe fn StencilMaskSeparate(&mut self, _face: GLenum, _mask: GLuint) {
        self.inner.StencilMaskSeparate(_face, _mask);
    }
    unsafe fn GetVertexAttribiv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLint) {
        self.inner.GetVertexAttribiv(_index, _pname, _params);
    }
    unsafe fn GetVertexAttribfv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLfloat) {
        self.inner.GetVertexAttribfv(_index, _pname, _params);
    }
    unsafe fn GetVertexAttribPointerv( &mut self, _index: GLuint, _pname: GLenum, _pointer: *mut *mut GLvoid, ) {
        self.inner.GetVertexAttribPointerv(_index, _pname, _pointer);
    }
    unsafe fn GetUniformiv(&mut self, _program: GLuint, _location: GLint, _params: *mut GLint) {
        self.inner.GetUniformiv(_program, _location, _params);
    }
    unsafe fn GetUniformfv(&mut self, _program: GLuint, _location: GLint, _params: *mut GLfloat) {
        self.inner.GetUniformfv(_program, _location, _params);
    }
    unsafe fn GetAttachedShaders( &mut self, _program: GLuint, _maxCount: GLsizei, _count: *mut GLsizei, _shaders: *mut GLuint, ) {
        self.inner.GetAttachedShaders(_program, _maxCount, _count, _shaders);
    }
    unsafe fn GetShaderSource( &mut self, _shader: GLuint, _bufSize: GLsizei, _length: *mut GLsizei, _source: *mut GLchar, ) {
        self.inner.GetShaderSource(_shader, _bufSize, _length, _source);
    }
    unsafe fn ReleaseShaderCompiler(&mut self) {
        self.inner.ReleaseShaderCompiler();
    }
    unsafe fn GetShaderPrecisionFormat( &mut self, _shadertype: GLenum, _precisiontype: GLenum, _range: *mut GLint, _precision: *mut GLint, ) {
        self.inner.GetShaderPrecisionFormat(_shadertype, _precisiontype, _range, _precision);
    }
    unsafe fn ShaderBinary( &mut self, _count: GLsizei, _shaders: *const GLuint, _binaryformat: GLenum, _binary: *const GLvoid, _length: GLsizei, ) {
        self.inner.ShaderBinary(_count, _shaders, _binaryformat, _binary, _length);
    }
    unsafe fn IsVertexArray(&mut self, _array: GLuint) -> GLboolean {
        self.inner.IsVertexArray(_array)
    }
    unsafe fn BindVertexArray(&mut self, _array: GLuint) {
        self.inner.BindVertexArray(_array);
    }
    unsafe fn DeleteVertexArrays(&mut self, _n: GLsizei, _arrays: *const GLuint) {
        self.inner.DeleteVertexArrays(_n, _arrays);
    }
    unsafe fn GenVertexArrays(&mut self, _n: GLsizei, _arrays: *mut GLuint) {
        self.inner.GenVertexArrays(_n, _arrays);
    }
    unsafe fn BindVertexArrayOES(&mut self, _array: GLuint) {
        self.inner.BindVertexArrayOES(_array);
    }
    unsafe fn GenVertexArraysOES(&mut self, _n: GLsizei, _arrays: *mut GLuint) {
        self.inner.GenVertexArraysOES(_n, _arrays);
    }
    unsafe fn DeleteVertexArraysOES(&mut self, _n: GLsizei, _arrays: *const GLuint) {
        self.inner.DeleteVertexArraysOES(_n, _arrays);
    }
    unsafe fn IsVertexArrayOES(&mut self, _array: GLuint) -> GLboolean {
        self.inner.IsVertexArrayOES(_array)
    }
    unsafe fn MapBufferRange( &mut self, _target: GLenum, _offset: GLintptr, _length: GLsizeiptr, _access: GLbitfield, ) -> *mut GLvoid {
        self.inner.MapBufferRange(_target, _offset, _length, _access)
    }
    unsafe fn FlushMappedBufferRange( &mut self, _target: GLenum, _offset: GLintptr, _length: GLsizeiptr, ) {
        self.inner.FlushMappedBufferRange(_target, _offset, _length);
    }
    unsafe fn GetBufferPointerv( &mut self, _target: GLenum, _pname: GLenum, _params: *mut *mut GLvoid, ) {
        self.inner.GetBufferPointerv(_target, _pname, _params);
    }
    unsafe fn GetBufferParameteri64v( &mut self, _target: GLenum, _pname: GLenum, _params: *mut i64, ) {
        self.inner.GetBufferParameteri64v(_target, _pname, _params);
    }
    unsafe fn CopyBufferSubData( &mut self, _readTarget: GLenum, _writeTarget: GLenum, _readOffset: GLintptr, _writeOffset: GLintptr, _size: GLsizeiptr, ) {
        self.inner.CopyBufferSubData(_readTarget, _writeTarget, _readOffset, _writeOffset, _size);
    }
    unsafe fn BindBufferBase(&mut self, _target: GLenum, _index: GLuint, _buffer: GLuint) {
        self.inner.BindBufferBase(_target, _index, _buffer);
    }
    unsafe fn BindBufferRange( &mut self, _target: GLenum, _index: GLuint, _buffer: GLuint, _offset: GLintptr, _size: GLsizeiptr, ) {
        self.inner.BindBufferRange(_target, _index, _buffer, _offset, _size);
    }
    unsafe fn UnmapBuffer(&mut self, _target: GLenum) -> GLboolean {
        self.inner.UnmapBuffer(_target)
    }
    unsafe fn TexImage3D( &mut self, _target: GLenum, _level: GLint, _internalformat: GLint, _width: GLsizei, _height: GLsizei, _depth: GLsizei, _border: GLint, _format: GLenum, _type_: GLenum, _pixels: *const GLvoid, ) {
        self.inner.TexImage3D(_target, _level, _internalformat, _width, _height, _depth, _border, _format, _type_, _pixels);
    }
    unsafe fn TexSubImage3D( &mut self, _target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _zoffset: GLint, _width: GLsizei, _height: GLsizei, _depth: GLsizei, _format: GLenum, _type_: GLenum, _pixels: *const GLvoid, ) {
        self.inner.TexSubImage3D(_target, _level, _xoffset, _yoffset, _zoffset, _width, _height, _depth, _format, _type_, _pixels);
    }
    unsafe fn CopyTexSubImage3D( &mut self, _target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _zoffset: GLint, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei, ) {
        self.inner.CopyTexSubImage3D(_target, _level, _xoffset, _yoffset, _zoffset, _x, _y, _width, _height);
    }
    unsafe fn CompressedTexImage3D( &mut self, _target: GLenum, _level: GLint, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, _depth: GLsizei, _border: GLint, _imageSize: GLsizei, _data: *const GLvoid, ) {
        self.inner.CompressedTexImage3D(_target, _level, _internalformat, _width, _height, _depth, _border, _imageSize, _data);
    }
    unsafe fn CompressedTexSubImage3D( &mut self, _target: GLenum, _level: GLint, _xoffset: GLint, _yoffset: GLint, _zoffset: GLint, _width: GLsizei, _height: GLsizei, _depth: GLsizei, _format: GLenum, _imageSize: GLsizei, _data: *const GLvoid, ) {
        self.inner.CompressedTexSubImage3D(_target, _level, _xoffset, _yoffset, _zoffset, _width, _height, _depth, _format, _imageSize, _data);
    }
    unsafe fn TexStorage2D( &mut self, _target: GLenum, _levels: GLsizei, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, ) {
        self.inner.TexStorage2D(_target, _levels, _internalformat, _width, _height);
    }
    unsafe fn TexStorage3D( &mut self, _target: GLenum, _levels: GLsizei, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, _depth: GLsizei, ) {
        self.inner.TexStorage3D(_target, _levels, _internalformat, _width, _height, _depth);
    }
    unsafe fn BlitFramebuffer( &mut self, _srcX0: GLint, _srcY0: GLint, _srcX1: GLint, _srcY1: GLint, _dstX0: GLint, _dstY0: GLint, _dstX1: GLint, _dstY1: GLint, _mask: GLbitfield, _filter: GLenum, ) {
        self.inner.BlitFramebuffer(_srcX0, _srcY0, _srcX1, _srcY1, _dstX0, _dstY0, _dstX1, _dstY1, _mask, _filter);
    }
    unsafe fn RenderbufferStorageMultisample( &mut self, _target: GLenum, _samples: GLsizei, _internalformat: GLenum, _width: GLsizei, _height: GLsizei, ) {
        self.inner.RenderbufferStorageMultisample(_target, _samples, _internalformat, _width, _height);
    }
    unsafe fn FramebufferTextureLayer( &mut self, _target: GLenum, _attachment: GLenum, _texture: GLuint, _level: GLint, _layer: GLint, ) {
        self.inner.FramebufferTextureLayer(_target, _attachment, _texture, _level, _layer);
    }
    unsafe fn InvalidateFramebuffer( &mut self, _target: GLenum, _numAttachments: GLsizei, _attachments: *const GLenum, ) {
        self.inner.InvalidateFramebuffer(_target, _numAttachments, _attachments);
    }
    unsafe fn InvalidateSubFramebuffer( &mut self, _target: GLenum, _numAttachments: GLsizei, _attachments: *const GLenum, _x: GLint, _y: GLint, _width: GLsizei, _height: GLsizei, ) {
        self.inner.InvalidateSubFramebuffer(_target, _numAttachments, _attachments, _x, _y, _width, _height);
    }
    unsafe fn ReadBuffer(&mut self, _src: GLenum) {
        self.inner.ReadBuffer(_src);
    }
    unsafe fn DrawBuffers(&mut self, _n: GLsizei, _bufs: *const GLenum) {
        self.inner.DrawBuffers(_n, _bufs);
    }
    unsafe fn DrawRangeElements( &mut self, _mode: GLenum, _start: GLuint, _end: GLuint, _count: GLsizei, _type_: GLenum, _indices: *const GLvoid, ) {
        self.inner.DrawRangeElements(_mode, _start, _end, _count, _type_, _indices);
    }
    unsafe fn ClearBufferiv(&mut self, _buffer: GLenum, _drawbuffer: GLint, _value: *const GLint) {
        self.inner.ClearBufferiv(_buffer, _drawbuffer, _value);
    }
    unsafe fn ClearBufferuiv( &mut self, _buffer: GLenum, _drawbuffer: GLint, _value: *const GLuint, ) {
        self.inner.ClearBufferuiv(_buffer, _drawbuffer, _value);
    }
    unsafe fn ClearBufferfv( &mut self, _buffer: GLenum, _drawbuffer: GLint, _value: *const GLfloat, ) {
        self.inner.ClearBufferfv(_buffer, _drawbuffer, _value);
    }
    unsafe fn ClearBufferfi( &mut self, _buffer: GLenum, _drawbuffer: GLint, _depth: GLfloat, _stencil: GLint, ) {
        self.inner.ClearBufferfi(_buffer, _drawbuffer, _depth, _stencil);
    }
    unsafe fn GenQueries(&mut self, _n: GLsizei, _ids: *mut GLuint) {
        self.inner.GenQueries(_n, _ids);
    }
    unsafe fn DeleteQueries(&mut self, _n: GLsizei, _ids: *const GLuint) {
        self.inner.DeleteQueries(_n, _ids);
    }
    unsafe fn IsQuery(&mut self, _id: GLuint) -> GLboolean {
        self.inner.IsQuery(_id)
    }
    unsafe fn BeginQuery(&mut self, _target: GLenum, _id: GLuint) {
        self.inner.BeginQuery(_target, _id);
    }
    unsafe fn EndQuery(&mut self, _target: GLenum) {
        self.inner.EndQuery(_target);
    }
    unsafe fn GetQueryiv(&mut self, _target: GLenum, _pname: GLenum, _params: *mut GLint) {
        self.inner.GetQueryiv(_target, _pname, _params);
    }
    unsafe fn GetQueryObjectuiv(&mut self, _id: GLuint, _pname: GLenum, _params: *mut GLuint) {
        self.inner.GetQueryObjectuiv(_id, _pname, _params);
    }
    unsafe fn GenSamplers(&mut self, _count: GLsizei, _samplers: *mut GLuint) {
        self.inner.GenSamplers(_count, _samplers);
    }
    unsafe fn DeleteSamplers(&mut self, _count: GLsizei, _samplers: *const GLuint) {
        self.inner.DeleteSamplers(_count, _samplers);
    }
    unsafe fn IsSampler(&mut self, _sampler: GLuint) -> GLboolean {
        self.inner.IsSampler(_sampler)
    }
    unsafe fn BindSampler(&mut self, _unit: GLuint, _sampler: GLuint) {
        self.inner.BindSampler(_unit, _sampler);
    }
    unsafe fn SamplerParameteri(&mut self, _sampler: GLuint, _pname: GLenum, _param: GLint) {
        self.inner.SamplerParameteri(_sampler, _pname, _param);
    }
    unsafe fn SamplerParameteriv( &mut self, _sampler: GLuint, _pname: GLenum, _params: *const GLint, ) {
        self.inner.SamplerParameteriv(_sampler, _pname, _params);
    }
    unsafe fn SamplerParameterf(&mut self, _sampler: GLuint, _pname: GLenum, _param: GLfloat) {
        self.inner.SamplerParameterf(_sampler, _pname, _param);
    }
    unsafe fn SamplerParameterfv( &mut self, _sampler: GLuint, _pname: GLenum, _params: *const GLfloat, ) {
        self.inner.SamplerParameterfv(_sampler, _pname, _params);
    }
    unsafe fn GetSamplerParameteriv( &mut self, _sampler: GLuint, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetSamplerParameteriv(_sampler, _pname, _params);
    }
    unsafe fn GetSamplerParameterfv( &mut self, _sampler: GLuint, _pname: GLenum, _params: *mut GLfloat, ) {
        self.inner.GetSamplerParameterfv(_sampler, _pname, _params);
    }
    unsafe fn BeginTransformFeedback(&mut self, _primitiveMode: GLenum) {
        self.inner.BeginTransformFeedback(_primitiveMode);
    }
    unsafe fn EndTransformFeedback(&mut self) {
        self.inner.EndTransformFeedback();
    }
    unsafe fn BindTransformFeedback(&mut self, _target: GLenum, _id: GLuint) {
        self.inner.BindTransformFeedback(_target, _id);
    }
    unsafe fn DeleteTransformFeedbacks(&mut self, _n: GLsizei, _ids: *const GLuint) {
        self.inner.DeleteTransformFeedbacks(_n, _ids);
    }
    unsafe fn GenTransformFeedbacks(&mut self, _n: GLsizei, _ids: *mut GLuint) {
        self.inner.GenTransformFeedbacks(_n, _ids);
    }
    unsafe fn IsTransformFeedback(&mut self, _id: GLuint) -> GLboolean {
        self.inner.IsTransformFeedback(_id)
    }
    unsafe fn PauseTransformFeedback(&mut self) {
        self.inner.PauseTransformFeedback();
    }
    unsafe fn ResumeTransformFeedback(&mut self) {
        self.inner.ResumeTransformFeedback();
    }
    unsafe fn TransformFeedbackVaryings( &mut self, _program: GLuint, _count: GLsizei, _varyings: *const *const GLchar, _bufferMode: GLenum, ) {
        self.inner.TransformFeedbackVaryings(_program, _count, _varyings, _bufferMode);
    }
    unsafe fn GetTransformFeedbackVarying( &mut self, _program: GLuint, _index: GLuint, _bufSize: GLsizei, _length: *mut GLsizei, _size: *mut GLsizei, _type_: *mut GLenum, _name: *mut GLchar, ) {
        self.inner.GetTransformFeedbackVarying(_program, _index, _bufSize, _length, _size, _type_, _name);
    }
    unsafe fn VertexAttribIPointer( &mut self, _index: GLuint, _size: GLint, _type_: GLenum, _stride: GLsizei, _pointer: *const GLvoid, ) {
        self.inner.VertexAttribIPointer(_index, _size, _type_, _stride, _pointer);
    }
    unsafe fn GetVertexAttribIiv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLint) {
        self.inner.GetVertexAttribIiv(_index, _pname, _params);
    }
    unsafe fn GetVertexAttribIuiv(&mut self, _index: GLuint, _pname: GLenum, _params: *mut GLuint) {
        self.inner.GetVertexAttribIuiv(_index, _pname, _params);
    }
    unsafe fn VertexAttribI4i( &mut self, _index: GLuint, _x: GLint, _y: GLint, _z: GLint, _w: GLint, ) {
        self.inner.VertexAttribI4i(_index, _x, _y, _z, _w);
    }
    unsafe fn VertexAttribI4ui( &mut self, _index: GLuint, _x: GLuint, _y: GLuint, _z: GLuint, _w: GLuint, ) {
        self.inner.VertexAttribI4ui(_index, _x, _y, _z, _w);
    }
    unsafe fn VertexAttribI4iv(&mut self, _index: GLuint, _v: *const GLint) {
        self.inner.VertexAttribI4iv(_index, _v);
    }
    unsafe fn VertexAttribI4uiv(&mut self, _index: GLuint, _v: *const GLuint) {
        self.inner.VertexAttribI4uiv(_index, _v);
    }
    unsafe fn Uniform1ui(&mut self, _location: GLint, _v0: GLuint) {
        self.inner.Uniform1ui(_location, _v0);
    }
    unsafe fn Uniform2ui(&mut self, _location: GLint, _v0: GLuint, _v1: GLuint) {
        self.inner.Uniform2ui(_location, _v0, _v1);
    }
    unsafe fn Uniform3ui(&mut self, _location: GLint, _v0: GLuint, _v1: GLuint, _v2: GLuint) {
        self.inner.Uniform3ui(_location, _v0, _v1, _v2);
    }
    unsafe fn Uniform4ui( &mut self, _location: GLint, _v0: GLuint, _v1: GLuint, _v2: GLuint, _v3: GLuint, ) {
        self.inner.Uniform4ui(_location, _v0, _v1, _v2, _v3);
    }
    unsafe fn Uniform1uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        self.inner.Uniform1uiv(_location, _count, _value);
    }
    unsafe fn Uniform2uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        self.inner.Uniform2uiv(_location, _count, _value);
    }
    unsafe fn Uniform3uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        self.inner.Uniform3uiv(_location, _count, _value);
    }
    unsafe fn Uniform4uiv(&mut self, _location: GLint, _count: GLsizei, _value: *const GLuint) {
        self.inner.Uniform4uiv(_location, _count, _value);
    }
    unsafe fn GetUniformuiv(&mut self, _program: GLuint, _location: GLint, _params: *mut GLuint) {
        self.inner.GetUniformuiv(_program, _location, _params);
    }
    unsafe fn UniformMatrix2x3fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix2x3fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix3x2fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix3x2fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix2x4fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix2x4fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix4x2fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix4x2fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix3x4fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix3x4fv(_location, _count, _transpose, _value);
    }
    unsafe fn UniformMatrix4x3fv( &mut self, _location: GLint, _count: GLsizei, _transpose: GLboolean, _value: *const GLfloat, ) {
        self.inner.UniformMatrix4x3fv(_location, _count, _transpose, _value);
    }
    unsafe fn GetUniformIndices( &mut self, _program: GLuint, _uniformCount: GLsizei, _uniformNames: *const *const GLchar, _uniformIndices: *mut GLuint, ) {
        self.inner.GetUniformIndices(_program, _uniformCount, _uniformNames, _uniformIndices);
    }
    unsafe fn GetActiveUniformsiv( &mut self, _program: GLuint, _uniformCount: GLsizei, _uniformIndices: *const GLuint, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetActiveUniformsiv(_program, _uniformCount, _uniformIndices, _pname, _params);
    }
    unsafe fn GetUniformBlockIndex( &mut self, _program: GLuint, _uniformBlockName: *const GLchar, ) -> GLuint {
        self.inner.GetUniformBlockIndex(_program, _uniformBlockName)
    }
    unsafe fn GetActiveUniformBlockiv( &mut self, _program: GLuint, _uniformBlockIndex: GLuint, _pname: GLenum, _params: *mut GLint, ) {
        self.inner.GetActiveUniformBlockiv(_program, _uniformBlockIndex, _pname, _params);
    }
    unsafe fn GetActiveUniformBlockName( &mut self, _program: GLuint, _uniformBlockIndex: GLuint, _bufSize: GLsizei, _length: *mut GLsizei, _uniformBlockName: *mut GLchar, ) {
        self.inner.GetActiveUniformBlockName(_program, _uniformBlockIndex, _bufSize, _length, _uniformBlockName);
    }
    unsafe fn UniformBlockBinding( &mut self, _program: GLuint, _uniformBlockIndex: GLuint, _uniformBlockBinding: GLuint, ) {
        self.inner.UniformBlockBinding(_program, _uniformBlockIndex, _uniformBlockBinding);
    }
    unsafe fn DrawArraysInstanced( &mut self, _mode: GLenum, _first: GLint, _count: GLsizei, _instanceCount: GLsizei, ) {
        self.inner.DrawArraysInstanced(_mode, _first, _count, _instanceCount);
    }
    unsafe fn DrawElementsInstanced( &mut self, _mode: GLenum, _count: GLsizei, _type_: GLenum, _indices: *const GLvoid, _instanceCount: GLsizei, ) {
        self.inner.DrawElementsInstanced(_mode, _count, _type_, _indices, _instanceCount);
    }
    unsafe fn VertexAttribDivisor(&mut self, _index: GLuint, _divisor: GLuint) {
        self.inner.VertexAttribDivisor(_index, _divisor);
    }
    fn supports_sync_objects(&self) -> bool {
        self.inner.supports_sync_objects()
    }
    unsafe fn FenceSync(&mut self, _condition: GLenum, _flags: GLbitfield) -> usize {
        self.inner.FenceSync(_condition, _flags)
    }
    unsafe fn IsSync(&mut self, _sync: usize) -> GLboolean {
        self.inner.IsSync(_sync)
    }
    unsafe fn DeleteSync(&mut self, _sync: usize) {
        self.inner.DeleteSync(_sync);
    }
    unsafe fn ClientWaitSync(&mut self, _sync: usize, _flags: GLbitfield, _timeout: u64) -> GLenum {
        self.inner.ClientWaitSync(_sync, _flags, _timeout)
    }
    unsafe fn WaitSync(&mut self, _sync: usize, _flags: GLbitfield, _timeout: u64) {
        self.inner.WaitSync(_sync, _flags, _timeout);
    }
    unsafe fn GetSynciv( &mut self, _sync: usize, _pname: GLenum, _bufSize: GLsizei, _length: *mut GLsizei, _values: *mut GLint, ) {
        self.inner.GetSynciv(_sync, _pname, _bufSize, _length, _values);
    }
    unsafe fn GetInteger64v(&mut self, _pname: GLenum, _data: *mut i64) {
        self.inner.GetInteger64v(_pname, _data);
    }
    unsafe fn GetIntegeri_v(&mut self, _target: GLenum, _index: GLuint, _data: *mut GLint) {
        self.inner.GetIntegeri_v(_target, _index, _data);
    }
    unsafe fn GetInteger64i_v(&mut self, _target: GLenum, _index: GLuint, _data: *mut i64) {
        self.inner.GetInteger64i_v(_target, _index, _data);
    }
    unsafe fn ProgramParameteri(&mut self, _program: GLuint, _pname: GLenum, _value: GLint) {
        self.inner.ProgramParameteri(_program, _pname, _value);
    }
    unsafe fn ProgramBinary( &mut self, _program: GLuint, _binaryFormat: GLenum, _binary: *const GLvoid, _length: GLsizei, ) {
        self.inner.ProgramBinary(_program, _binaryFormat, _binary, _length);
    }
    unsafe fn GetProgramBinary( &mut self, _program: GLuint, _bufSize: GLsizei, _length: *mut GLsizei, _binaryFormat: *mut GLenum, _binary: *mut GLvoid, ) {
        self.inner.GetProgramBinary(_program, _bufSize, _length, _binaryFormat, _binary);
    }
    unsafe fn GetStringi(&mut self, _name: GLenum, _index: GLuint) -> *const GLubyte {
        self.inner.GetStringi(_name, _index)
    }
    unsafe fn GetFragDataLocation(&mut self, _program: GLuint, _name: *const GLchar) -> GLint {
        self.inner.GetFragDataLocation(_program, _name)
    }
    unsafe fn GetInternalformativ( &mut self, _target: GLenum, _internalformat: GLenum, _pname: GLenum, _bufSize: GLsizei, _params: *mut GLint, ) {
        self.inner.GetInternalformativ(_target, _internalformat, _pname, _bufSize, _params);
    }
}

    // Forward other methods to inner

// We need to implement the rest of the GLES trait for LoggingGLES.
// Since there are many, we can use a macro or just implement the key ones.
// However, since I can't easily implement the whole trait without boilerplate,
// I'll just implement the most critical ones and the rest can be handled via a
// generic "log and call" mechanism if I had one.
//
// Actually, I can't partially implement a trait. I must implement ALL methods.
// This is the problem.

use std::sync::atomic::{AtomicU32, Ordering};

static TRANSLATOR_TRACE_EVENTS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn configure_translator_tracing(_enabled: bool) {}

pub(crate) fn translator_tracing_enabled() -> bool {
    // PERF: cached read-once flag; called from hot GL-translation paths.
    crate::env_flag_cached!("TOUCHHLE_TRACE_TRANSLATOR")
}

pub(crate) fn trace_translator_event(event: String) {
    if !translator_tracing_enabled() {
        return;
    }
    let number = TRANSLATOR_TRACE_EVENTS.fetch_add(1, Ordering::Relaxed);
    if number < 512 {
        log!("[translator] #{:03} {}", number + 1, event);
    } else if number == 512 {
        log!("[translator] further events suppressed after 512 entries");
    }
}

pub fn configure_angle_driver(enabled: bool) {
    if !enabled {
        return;
    }

    let default_egl = if cfg!(target_os = "windows") {
        "libEGL.dll"
    } else if cfg!(target_os = "macos") {
        "libEGL.dylib"
    } else {
        "libEGL.so"
    };
    let default_gles = if cfg!(target_os = "windows") {
        "libGLESv2.dll"
    } else if cfg!(target_os = "macos") {
        "libGLESv2.dylib"
    } else {
        "libGLESv2.so"
    };
    let egl_path = std::env::var("TOUCHHLE_ANGLE_EGL").unwrap_or_else(|_| default_egl.to_owned());
    let gles_path =
        std::env::var("TOUCHHLE_ANGLE_GLES").unwrap_or_else(|_| default_gles.to_owned());
    let egl_exists = std::path::Path::new(&egl_path).exists();
    let gles_exists = std::path::Path::new(&gles_path).exists();

    unsafe {
        std::env::set_var("SDL_VIDEO_EGL_DRIVER", &egl_path);
        std::env::set_var("SDL_VIDEO_GL_DRIVER", &gles_path);
    }
    sdl2::hint::set("SDL_OPENGL_ES_DRIVER", "1");
    log!(
        "ANGLE override requested: EGL={} (exists={}), GLES={} (exists={}); SDL will try these before the first window",
        egl_path,
        egl_exists,
        gles_path,
        gles_exists
    );
    if !egl_exists || !gles_exists {
        log!("ANGLE libraries are not present at the configured paths; SDL may fall back or context creation may fail");
    }
}

/// Labels for [GLES] implementations and an abstraction for constructing them.
#[derive(Copy, Clone)]
pub enum GLESImplementation {
    /// [gles1_native::GLES1Native].
    GLES1Native,
    /// [gles1_on_gl2::GLES1OnGL2].
    GLES1OnGL2,
    /// [gles1_on_gles2::GLES1OnGLES2].
    GLES1OnGLES2,
}
impl GLESImplementation {
    /// List of OpenGL ES 1.1 implementations in order of preference.
    pub const GLES1_IMPLEMENTATIONS: &'static [Self] = &[Self::GLES1Native, Self::GLES1OnGL2];
    /// Convert from short name used for command-line arguments. Returns [Err]
    /// if name is not recognized..
    pub fn from_short_name(name: &str) -> Result<Self, ()> {
        match name {
            "gles1_on_gl2" => Ok(Self::GLES1OnGL2),
            "gles1_on_gles2" => Ok(Self::GLES1OnGLES2),
            "gles1_native" => Ok(Self::GLES1Native),
            _ => Err(()),
        }
    }
    /// See [GLESContext::description].
    pub fn description(self) -> &'static str {
        match self {
            Self::GLES1Native => GLES1NativeContext::description(),
            Self::GLES1OnGL2 => GLES1OnGL2Context::description(),
            Self::GLES1OnGLES2 => GLES1OnGLES2Context::description(),
        }
    }
    /// See [GLESContext::new].
    pub fn construct(
        self,
        window: &mut crate::window::Window,
    ) -> Result<Box<dyn GLESContext>, String> {
        fn boxer<T: GLESContext + 'static>(ctx: T) -> Box<dyn GLESContext> {
            Box::new(ctx)
        }
        match self {
            Self::GLES1Native => GLES1NativeContext::new(window).map(boxer),
            Self::GLES1OnGL2 => GLES1OnGL2Context::new(window).map(boxer),
            Self::GLES1OnGLES2 => GLES1OnGLES2Context::new(window).map(boxer),
        }
    }
}

pub fn create_gles1_translator_ctx_no_parent_stack(
    window: &mut crate::window::Window,
) -> Box<dyn GLESContext> {
    assert!(window.on_main_stack());
    log!("Creating the OpenGL ES 1.1 to native OpenGL ES 2.0 translator");
    Box::new(
        GLES1OnGLES2Context::new(window)
            .expect("Couldn't create OpenGL ES 1.1-on-GLES2 translator context!"),
    )
}
pub fn create_gles1_gles3_translator_ctx_no_parent_stack(
    window: &mut crate::window::Window,
) -> Box<dyn GLESContext> {
    assert!(window.on_main_stack());
    log!("Creating the OpenGL ES 1.1 to native OpenGL ES 3.0 translator");
    Box::new(
        GLES1OnGLES2Context::new_with_gl_version(window, GLVersion::GLES30)
            .expect("Couldn't create OpenGL ES 1.1-on-GLES3 translator context!"),
    )
}

pub fn create_gles1_gles3_translator_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        create_gles1_gles3_translator_ctx_no_parent_stack(window)
    })
}

pub fn create_gles1_translator_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        create_gles1_translator_ctx_no_parent_stack(window)
    })
}

/// Try to create an OpenGL ES 1.1 context using the configured strategies,
/// panicking on failure.
pub fn create_gles1_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, options| {
        create_gles1_ctx_no_parent_stack(window, options).unwrap_or_else(|err| panic!("{}", err))
    })
}

/// Try to create an OpenGL ES 2.0 context, panicking on failure.
///
/// The preference order, from "most-correct" to "only as a last resort":
///
/// 1. [`GLES2NativeContext`] — a real OpenGL ES 2.0 driver. This is the
///    only thing that works on platforms without desktop OpenGL such as
///    Android, and on real iOS hardware emulation. Every ES 2.0 entry point
///    is a direct passthrough to the host driver.
/// 2. [`GLES2OnGL3Context`] — a full ES 2.0 backend built on top of
///    desktop OpenGL 3.3 Core. This shares its implementation with the ES
///    3.0 fallback ([`GLES3OnGL3Context`]), giving us a single source of
///    truth for ES 2.0 / ES 3.0 emulation, full shader support, and proper
///    GLSL ES → desktop GLSL translation via
///    [`gles2_glsl::translate_glsl_es_to_120`]. This is the preferred
///    fallback on x86 Linux/macOS desktops where Mesa lacks a native ES 2.0
///    surface.
/// 3. [`GLES1OnGL2Context`] — legacy fallback that piggy-backs on a desktop
///    OpenGL 2.1 compatibility profile context. Only used on the rare host
///    that has GL 2.1 compat but no GL 3.3 Core (e.g. very old macOS
///    installations); kept around for backwards compatibility.
pub fn create_gles2_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, options| {
        assert!(window.on_main_stack());
        log!("Creating an OpenGL ES 2.0 context:");

        let ctx = {
            log!("Trying: {}", GLES2NativeContext::description());
            match GLES2NativeContext::new(window) {
                Ok(ctx) => {
                    log!("=> Success!");
                    Some(Box::new(ctx) as Box<dyn GLESContext>)
                }
                Err(err) => {
                    log!("=> Failed: {}.", err);
                    None
                }
            }
            .or_else(|| {
                log!(
                    "Trying: {} (used for OpenGL ES 2.0)",
                    GLES2OnGL3Context::description()
                );
                match GLES2OnGL3Context::new(window) {
                    Ok(ctx) => {
                        log!("=> Success!");
                        Some(Box::new(ctx) as Box<dyn GLESContext>)
                    }
                    Err(err) => {
                        log!("=> Failed: {}.", err);
                        None
                    }
                }
            })
            .or_else(|| {
                log!(
                    "Trying: {} (legacy GL 2.1 fallback for OpenGL ES 2.0)",
                    GLES1OnGL2Context::description()
                );
                match GLES1OnGL2Context::new(window) {
                    Ok(ctx) => {
                        log!("=> Success!");
                        Some(Box::new(ctx) as Box<dyn GLESContext>)
                    }
                    Err(err) => {
                        log!("=> Failed: {}.", err);
                        None
                    }
                }
            })
            .expect("Couldn't create OpenGL ES 2.0 context")
        };

        if options.trace_gl_errors {
            Box::new(LoggingGLESContext {
                inner: ctx,
                verbose: options.trace_gl_errors || options.verbose_gles,
            })
        } else {
            ctx
        }
    })
}

/// Try to create an OpenGL ES 3.0 context, panicking on failure.
///
/// This is the entry point used by [crate::frameworks::opengles::eagl] when
/// `EAGLContext initWithAPI:` is called with `kEAGLRenderingAPIOpenGLES3` (=
/// 3). It tries the native ES 3.0 backend first — the only thing that works
/// on Android and on desktop drivers configured for an ES context — and
/// falls back to the desktop GL 3.3 Core translation backend on hosts
/// without a native ES 3.0 driver (most x86 Linux/macOS desktops).
pub fn create_gles3_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, options| {
        assert!(window.on_main_stack());
        log!("Creating an OpenGL ES 3.0 context:");

        let ctx = {
            log!("Trying: {}", GLES3NativeContext::description());
            match GLES3NativeContext::new(window) {
                Ok(ctx) => {
                    log!("=> Success!");
                    Some(Box::new(ctx) as Box<dyn GLESContext>)
                }
                Err(err) => {
                    log!("=> Failed: {}.", err);
                    None
                }
            }
            .or_else(|| {
                log!(
                    "Trying: {} (used for OpenGL ES 3.0)",
                    GLES3OnGL3Context::description()
                );
                match GLES3OnGL3Context::new(window) {
                    Ok(ctx) => {
                        log!("=> Success!");
                        Some(Box::new(ctx) as Box<dyn GLESContext>)
                    }
                    Err(err) => {
                        log!("=> Failed: {}.", err);
                        None
                    }
                }
            })
            .expect("Couldn't create OpenGL ES 3.0 context")
        };

        if options.trace_gl_errors {
            Box::new(LoggingGLESContext {
                inner: ctx,
                verbose: options.trace_gl_errors || options.verbose_gles,
            })
        } else {
            ctx
        }
    })
}

/// Create an OpenGL ES 2.0 context from the window's main stack.
///
/// The window owns the internal context used for compositing and the splash
/// screen, so it must be created before the guest environment exists.
pub fn create_gles2_ctx_no_parent_stack(
    window: &mut crate::window::Window,
) -> Result<Box<dyn GLESContext>, String> {
    assert!(window.on_main_stack());
    log!("Creating an OpenGL ES 2.0 context:");
    let mut failures = Vec::new();

    let description = GLES2NativeContext::description();
    log!("Trying: {}", description);
    match GLES2NativeContext::new(window) {
        Ok(ctx) => {
            log!("=> Success!");
            return Ok(Box::new(ctx));
        }
        Err(err) => {
            log!("=> Failed: {}.", err);
            failures.push(format!("{}: {}", description, err));
        }
    }

    let description = GLES2OnGL3Context::description();
    log!("Trying: {} (used for OpenGL ES 2.0)", description);
    match GLES2OnGL3Context::new(window) {
        Ok(ctx) => {
            log!("=> Success!");
            return Ok(Box::new(ctx));
        }
        Err(err) => {
            log!("=> Failed: {}.", err);
            failures.push(format!("{}: {}", description, err));
        }
    }

    let description = GLES1OnGL2Context::description();
    log!(
        "Trying: {} (legacy GL 2.1 fallback for OpenGL ES 2.0)",
        description
    );
    match GLES1OnGL2Context::new(window) {
        Ok(ctx) => {
            log!("=> Success!");
            Ok(Box::new(ctx))
        }
        Err(err) => {
            log!("=> Failed: {}.", err);
            failures.push(format!("{}: {}", description, err));
            Err(format!(
                "Couldn't create OpenGL ES 2.0 context. Tried: {}",
                failures.join("; ")
            ))
        }
    }
}

/// Same as [create_gles1_ctx], but without calling
/// [Environment::on_parent_stack_in_coroutine]. Only should be called by
/// functions not inside a coroutine that can't use [Environment].
pub fn create_gles1_ctx_no_parent_stack(
    window: &mut crate::window::Window,
    options: &crate::options::Options,
) -> Result<Box<dyn GLESContext>, String> {
    assert!(window.on_main_stack());
    log!("Creating an OpenGL ES 1.1 context:");
    // Hardcoded GPU/backend pin: TOUCHHLE_FORCE_GLES1 overrides everything
    // (CLI flag, host capability probing) so the exact backend is used even
    // when probing would pick a different one.
    let forced = std::env::var("TOUCHHLE_FORCE_GLES1")
        .ok()
        .and_then(|name| GLESImplementation::from_short_name(&name).ok());
    let forced_list: [GLESImplementation; 1] = match forced {
        Some(impl_) => [impl_],
        None => [GLESImplementation::GLES1OnGL2],
    };
    // When the host GLES stack is ANGLE (the bundled default on Android),
    // ANGLE implements ES 1.1 natively (libGLESv1_CM_angle) so the native path
    // is preferred; the ES 1.1-on-ES 2.0 translator stays as a fallback and
    // the desktop-GL backend is last, since ANGLE has no desktop-GL support.
    let using_angle = std::env::var("SDL_VIDEO_EGL_DRIVER")
        .map(|driver| driver.contains("angle"))
        .unwrap_or(false);
    let list: &[GLESImplementation] = match forced {
        Some(_) => &forced_list[..],
        None => match options.gles1_implementation {
            Some(ref preference) => std::slice::from_ref(preference),
            None if using_angle => &[
                GLESImplementation::GLES1Native,
                GLESImplementation::GLES1OnGL2,
            ],
            None => GLESImplementation::GLES1_IMPLEMENTATIONS,
        },
    };
    let mut failures = Vec::new();
    for implementation in list {
        let description = implementation.description();
        log!("Trying: {}", description);
        match implementation.construct(window) {
            Ok(ctx) => {
                log!("=> Success!");
                return Ok(ctx);
            }
            Err(err) => {
                log!("=> Failed: {}.", err);
                failures.push(format!("{}: {}", description, err));
            }
        }
    }
    Err(format!(
        "Couldn't create OpenGL ES 1.1 context. Tried: {}",
        failures.join("; ")
    ))
}
