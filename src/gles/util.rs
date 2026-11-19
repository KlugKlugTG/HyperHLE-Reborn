/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Shared utilities.

use super::gles11_raw as gles11; // constants only
use super::gles11_raw::types::{GLenum, GLfixed, GLfloat, GLint, GLsizei};
use super::GLES;

#[cfg(test)]
mod pvrtc_payload_size_tests {
    use super::pvrtc_payload_size;
    use crate::gles::gles11_raw as gles11;

    #[test]
    fn computes_full_size_for_pvrtc_formats() {
        assert_eq!(
            pvrtc_payload_size(gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG, 2048, 2048),
            Some(2_097_152)
        );
        assert_eq!(
            pvrtc_payload_size(gles11::COMPRESSED_RGBA_PVRTC_2BPPV1_IMG, 2048, 2048),
            Some(1_048_576)
        );
    }

    #[test]
    fn enforces_pvrtc_minimum_block_dimensions() {
        assert_eq!(
            pvrtc_payload_size(gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG, 1, 1),
            Some(32)
        );
        assert_eq!(
            pvrtc_payload_size(gles11::COMPRESSED_RGBA_PVRTC_2BPPV1_IMG, 1, 1),
            Some(32)
        );
    }

    #[test]
    fn rejects_non_pvrtc_formats_and_negative_dimensions() {
        assert_eq!(pvrtc_payload_size(gles11::RGBA, 32, 32), None);
        assert_eq!(
            pvrtc_payload_size(gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG, -1, 32),
            None
        );
    }
}

/// Return `(is_2bit, is_opaque)` for a PVRTC v1 internal format.
pub fn pvrtc_format_properties(internalformat: GLenum) -> Option<(bool, bool)> {
    match internalformat {
        gles11::COMPRESSED_RGB_PVRTC_2BPPV1_IMG => Some((true, true)),
        gles11::COMPRESSED_RGBA_PVRTC_2BPPV1_IMG => Some((true, false)),
        gles11::COMPRESSED_RGB_PVRTC_4BPPV1_IMG => Some((false, true)),
        gles11::COMPRESSED_RGBA_PVRTC_4BPPV1_IMG => Some((false, false)),
        _ => None,
    }
}

/// Compute the compressed byte size for a PVRTC v1 level without overflowing.
pub fn pvrtc_payload_size(
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
) -> Option<usize> {
    let (is_2bit, _) = pvrtc_format_properties(internalformat)?;
    let width = usize::try_from(width).ok()?;
    let height = usize::try_from(height).ok()?;
    width
        .max(if is_2bit { 16 } else { 8 })
        .checked_mul(height.max(8))?
        .checked_mul(if is_2bit { 2 } else { 4 })?
        .checked_add(7)
        .map(|bits| bits / 8)
}

/// Vertex array state overwritten by an internal ES2 draw. Each attribute
/// captures its own buffer binding when VertexAttribPointer is called.
/// <https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glVertexAttribPointer.xml>
#[derive(Debug, PartialEq)]
pub(crate) struct VertexAttribState {
    index: u32,
    enabled: i32,
    buffer: i32,
    size: i32,
    stride: i32,
    type_: i32,
    normalized: i32,
    pointer: *mut std::ffi::c_void,
}

impl VertexAttribState {
    pub(crate) unsafe fn capture(gles: &mut dyn GLES, index: u32) -> Self {
        use super::gles2_raw as gl;
        let mut state = Self {
            index,
            enabled: 0,
            buffer: 0,
            size: 0,
            stride: 0,
            type_: 0,
            normalized: 0,
            pointer: std::ptr::null_mut(),
        };
        gles.GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_ENABLED, &mut state.enabled);
        gles.GetVertexAttribiv(
            index,
            gl::VERTEX_ATTRIB_ARRAY_BUFFER_BINDING,
            &mut state.buffer,
        );
        gles.GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_SIZE, &mut state.size);
        gles.GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_STRIDE, &mut state.stride);
        gles.GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_TYPE, &mut state.type_);
        gles.GetVertexAttribiv(
            index,
            gl::VERTEX_ATTRIB_ARRAY_NORMALIZED,
            &mut state.normalized,
        );
        gles.GetVertexAttribPointerv(index, gl::VERTEX_ATTRIB_ARRAY_POINTER, &mut state.pointer);
        state
    }

    /// The caller restores the global ARRAY_BUFFER binding after all attributes.
    pub(crate) unsafe fn restore(&self, gles: &mut dyn GLES) {
        use super::gles2_raw as gl;
        gles.BindBuffer(gl::ARRAY_BUFFER, self.buffer as _);
        gles.VertexAttribPointer(
            self.index,
            self.size,
            self.type_ as _,
            self.normalized as _,
            self.stride,
            self.pointer.cast_const(),
        );
        if self.enabled != 0 {
            gles.EnableVertexAttribArray(self.index);
        } else {
            gles.DisableVertexAttribArray(self.index);
        }
    }
}

/// Convert a fixed-point scalar to a floating-point scalar.
///
/// Beware: Rust's type checker won't complain if you mix up [GLfixed] with
/// [GLint], but they have very different meanings.
pub fn fixed_to_float(fixed: GLfixed) -> GLfloat {
    ((fixed as f64) / ((1 << 16) as f64)) as f32
}

/// Convert a floating-point scalar to a fixed-point (16.16) scalar, saturating
/// on overflow. Used to implement `glGetFixedv` on top of a backend that only
/// exposes floating-point state.
pub fn float_to_fixed(float: GLfloat) -> GLfixed {
    let scaled = (float as f64) * ((1 << 16) as f64);
    if scaled >= GLfixed::MAX as f64 {
        GLfixed::MAX
    } else if scaled <= GLfixed::MIN as f64 {
        GLfixed::MIN
    } else if scaled.is_nan() {
        0
    } else {
        scaled.round() as GLfixed
    }
}

/// Convert a fixed-point 4-by-4 matrix to floating-point.
pub unsafe fn matrix_fixed_to_float(m: *const GLfixed) -> [GLfloat; 16] {
    let mut matrix = [0f32; 16];
    for (i, cell) in matrix.iter_mut().enumerate() {
        *cell = fixed_to_float(m.add(i).read_unaligned());
    }
    matrix
}

/// Type of a parameter, used in [ParamTable].
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ParamType {
    /// `GLboolean`
    Boolean,
    /// `GLfloat`
    Float,
    /// `GLint`
    Int,
    /// Normalized floating-point color components (RGBA in [0, 1]).
    ///
    /// Per the GLES 1.1 spec (Table 6.1, type "C"), color state is clamped
    /// to [0, 1] on set; when queried with an integer getter the components
    /// are scaled to the full integer range (see [ParamTable::get_type_info]
    /// users), unlike plain `Float` state which is rounded to nearest.
    Color,
    /// Placeholder type for things like colors which are floating-point
    /// but don't have the usual conversion behavior to/from integers etc.
    /// [ParamTable] will accept it for floating-point inputs only.
    /// TODO: Remove this and add proper types for colors etc.
    FloatSpecial,
    /// Hack to achieve `#[non_exhaustive]`-like behavior within this crate,
    /// since more types might be added in future
    _NonExhaustive,
}

/// Table of parameter names, component types and component counts.
///
/// This is a helper for implementing the common pattern in OpenGL where a set
/// of parameters named by [GLenum] values can be accessed via functions with
/// suffixes like `f`, `fv`, `i`, `iv`, etc.
pub struct ParamTable(pub &'static [(GLenum, ParamType, u8)]);

impl ParamTable {
    /// Look up the component type and count for a parameter. Returns a safe
    /// default (`ParamType::Float`, 1) for unknown names rather than panicking
    /// the host, so misbehaving guest code only loses correctness for the
    /// specific call rather than tearing the whole emulator down.
    pub fn get_type_info(&self, pname: GLenum) -> (ParamType, u8) {
        match self.0.iter().find(|&&(pname2, _, _)| pname == pname2) {
            Some(&(_, type_, count)) => (type_, count),
            None => {
                log!(
                    "Warning: ParamTable::get_type_info: unhandled parameter name {pname:#x}; defaulting to (Float, 1)."
                );
                (ParamType::Float, 1)
            }
        }
    }

    /// Assert that a parameter name is recognized.
    pub fn assert_known_param(&self, pname: GLenum) {
        self.get_type_info(pname);
    }

    pub fn contains(&self, pname: GLenum) -> bool {
        self.0.iter().any(|(pname2, _, _)| pname == *pname2)
    }

    /// Check that a parameter name is recognized and that the parameter has a
    /// particular component count. Logs a warning instead of panicking the
    /// host on mismatch, so the worst case is just a malformed GL call.
    pub fn assert_component_count(&self, pname: GLenum, provided_count: u8) {
        let (_type, actual_count) = self.get_type_info(pname);
        if actual_count != provided_count {
            log!(
                "Warning: ParamTable::assert_component_count: parameter {pname:#x} has component count {actual_count}, {provided_count} given; continuing anyway."
            );
        }
    }

    /// Implements a fixed-point scalar (`x`) setter by calling a provided
    /// floating-point scalar (`f`) or integer scalar (`i`) setter as
    /// as appropriate.
    ///
    /// This will panic if the name is not recognized or the parameter is not
    /// a scalar.
    pub unsafe fn setx<FF, FI>(&self, setf: FF, seti: FI, pname: GLenum, param: GLfixed)
    where
        FF: FnOnce(GLfloat),
        FI: FnOnce(GLint),
    {
        let (type_, component_count) = self.get_type_info(pname);
        assert!(component_count == 1);
        // Yes, the OpenGL standard lets you mismatch types. Yes, it requires
        // an implicit conversion. Yes, it requires no scaling of fixed-point
        // values when converting to integer. :(
        // On the other hand, fixed-to-float/float-to-fixed conversion is always
        // the same even for the weird float-ish values.
        match type_ {
            ParamType::Float | ParamType::Color | ParamType::FloatSpecial => {
                setf(fixed_to_float(param))
            }
            _ => seti(param),
        }
    }

    /// Implements a fixed-point vector (`xv`) setter by calling a provided
    /// floating-point vector (`fv`) or integer vector (`iv`) setter as
    /// as appropriate.
    ///
    /// This will panic if the name is not recognized.
    pub unsafe fn setxv<FFV, FIV>(
        &self,
        setfv: FFV,
        setiv: FIV,
        pname: GLenum,
        params: *const GLfixed,
    ) where
        FFV: FnOnce(*const GLfloat),
        FIV: FnOnce(*const GLint),
    {
        let (type_, count) = self.get_type_info(pname);
        // Yes, the OpenGL standard is like this (see above).
        match type_ {
            // Fixed-to-float/float-to-fixed conversion is always the same even
            // for the weird float-ish values.
            ParamType::Float | ParamType::Color | ParamType::FloatSpecial => {
                let mut params_float = [0.0; 16]; // probably the max?
                let params_float = &mut params_float[..usize::from(count)];
                for (i, param_float) in params_float.iter_mut().enumerate() {
                    *param_float = fixed_to_float(params.add(i).read())
                }
                setfv(params_float.as_ptr())
            }
            _ => setiv(params),
        }
    }
}

/// Helper for implementing `glCompressedTexImage2D`: if `internalformat` is
/// one of the `IMG_texture_compression_pvrtc` formats, decode it via the
/// software PVRTC decoder and call `glTexImage2D` to upload the resulting
/// RGBA8 texture. Returns:
/// * `true` if the format was a PVRTC variant and the upload was attempted
///   (regardless of whether the upload itself succeeded — that's between the
///   driver and the caller).
/// * `false` if `internalformat` is not a PVRTC variant; the caller is then
///   responsible for handling the format some other way (passthrough,
///   paletted-texture decode, ...).
///
/// Previously this helper used `assert!` for `border == 0` and for the
/// payload-size check inside `decode_pvrtc`. Real-world iPhone OS games
/// occasionally pass in slightly truncated payloads (broken builds, in-app
/// procedural textures with off-by-one mip sizes, third-party loaders that
/// trim trailing zeros), and a panic in the present pipeline brings down the
/// whole host process. Treat malformed input as a soft failure: log once and
/// return `true` *without* uploading anything, so the caller doesn't
/// re-attempt (the data is unusable either way) and the rest of the frame
/// can still draw.
#[allow(clippy::too_many_arguments)]
pub fn try_decode_pvrtc(
    gles: &mut dyn GLES,
    target: GLenum,
    level: GLint,
    internalformat: GLenum,
    width: GLsizei,
    height: GLsizei,
    border: GLint,
    pvrtc_data: &[u8],
) -> bool {
    let Some((is_2bit, is_opaque)) = pvrtc_format_properties(internalformat) else {
        return false;
    };

    if border != 0 {
        log!(
            "Warning: try_decode_pvrtc: invalid non-zero border ({border}) for PVRTC \
             upload {width}x{height} (level {level}, format {internalformat:#x}); \
             skipping upload."
        );
        return true;
    }

    let Ok(width_u) = u32::try_from(width) else {
        log!(
            "Warning: try_decode_pvrtc: invalid width {width} for PVRTC upload \
             (level {level}, format {internalformat:#x}); skipping upload."
        );
        return true;
    };
    let Ok(height_u) = u32::try_from(height) else {
        log!(
            "Warning: try_decode_pvrtc: invalid height {height} for PVRTC upload \
             (level {level}, format {internalformat:#x}); skipping upload."
        );
        return true;
    };

    // The IMG_texture_compression_pvrtc spec specifies a fixed payload size
    // for any given (is_2bit, width, height) tuple. Validate it here so that
    // a short / oversized input from the guest doesn't panic the host
    // PVRTC decoder. (`decode_pvrtc` itself still asserts internally as
    // a defence-in-depth measure, but we shouldn't rely on that — see the
    // function-level doc comment.)
    let Some(expected_size) = pvrtc_payload_size(internalformat, width, height) else {
        log!(
            "Warning: try_decode_pvrtc: payload size overflows for {width}x{height} \
             (level {level}, format {internalformat:#x}); skipping upload."
        );
        return true;
    };
    if pvrtc_data.len() != expected_size {
        log!(
            "Warning: try_decode_pvrtc: PVRTC payload size mismatch for \
             {width}x{height} (level {level}, format {internalformat:#x}, \
             is_2bit={is_2bit}): got {} bytes, expected {expected_size}; \
             skipping upload.",
            pvrtc_data.len(),
        );
        return true;
    }

    let upload_format = gles11::RGBA;

    let pixels =
        crate::image::decode_pvrtc_with_alpha(pvrtc_data, is_2bit, width_u, height_u, is_opaque);
    unsafe {
        gles.TexImage2D(
            target,
            level,
            upload_format as _,
            width,
            height,
            border,
            upload_format,
            gles11::UNSIGNED_BYTE,
            pixels.as_ptr() as *const _,
        )
    };
    true
}

/// Convert an uncompressed guest texture to RGBA8888 so GLES backends do not
/// depend on optional BGRA or packed-pixel upload support.
#[allow(clippy::too_many_arguments)]
pub unsafe fn decode_texture_to_rgba8(
    width: GLsizei,
    height: GLsizei,
    format: GLenum,
    type_: GLenum,
    pixels: *const std::ffi::c_void,
    unpack_alignment: GLint,
) -> Option<Vec<u8>> {
    if pixels.is_null() || width < 0 || height < 0 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let bytes_per_pixel = match type_ {
        gles11::UNSIGNED_BYTE => match format {
            gles11::ALPHA | gles11::LUMINANCE => 1,
            gles11::LUMINANCE_ALPHA => 2,
            gles11::RGB => 3,
            gles11::RGBA | gles11::BGRA_EXT => 4,
            _ => return None,
        },
        gles11::UNSIGNED_SHORT_5_6_5
        | gles11::UNSIGNED_SHORT_4_4_4_4
        | gles11::UNSIGNED_SHORT_5_5_5_1 => 2,
        _ => return None,
    };
    let alignment = unpack_alignment.max(1) as usize;
    let row_bytes = width.checked_mul(bytes_per_pixel)?;
    let row_stride = row_bytes.checked_add(alignment - 1)? / alignment * alignment;
    let output_len = width.checked_mul(height)?.checked_mul(4)?;
    let mut output = vec![0u8; output_len];
    for y in 0..height {
        let row = (pixels as *const u8).add(y * row_stride);
        for x in 0..width {
            let src = row.add(x * bytes_per_pixel);
            let dst = output.as_mut_ptr().add((y * width + x) * 4);
            let (r, g, b, a) = match type_ {
                gles11::UNSIGNED_BYTE => match format {
                    gles11::ALPHA => (255, 255, 255, src.read()),
                    gles11::LUMINANCE => {
                        let l = src.read();
                        (l, l, l, 255)
                    }
                    gles11::LUMINANCE_ALPHA => {
                        let l = src.read();
                        (l, l, l, src.add(1).read())
                    }
                    gles11::RGB => (src.read(), src.add(1).read(), src.add(2).read(), 255),
                    gles11::RGBA => (
                        src.read(),
                        src.add(1).read(),
                        src.add(2).read(),
                        src.add(3).read(),
                    ),
                    gles11::BGRA_EXT => (
                        src.add(2).read(),
                        src.add(1).read(),
                        src.read(),
                        src.add(3).read(),
                    ),
                    _ => return None,
                },
                gles11::UNSIGNED_SHORT_5_6_5
                | gles11::UNSIGNED_SHORT_4_4_4_4
                | gles11::UNSIGNED_SHORT_5_5_5_1 => {
                    let value = (src as *const u16).read_unaligned();
                    match type_ {
                        gles11::UNSIGNED_SHORT_5_6_5 => (
                            ((((value >> 11) & 0x1f) as u32 * 255 / 31) as u8),
                            ((((value >> 5) & 0x3f) as u32 * 255 / 63) as u8),
                            (((value & 0x1f) as u32 * 255 / 31) as u8),
                            255,
                        ),
                        gles11::UNSIGNED_SHORT_4_4_4_4 => (
                            ((((value >> 12) & 0xf) as u8) * 17),
                            ((((value >> 8) & 0xf) as u8) * 17),
                            ((((value >> 4) & 0xf) as u8) * 17),
                            (((value & 0xf) as u8) * 17),
                        ),
                        _ => (
                            ((((value >> 11) & 0x1f) as u32 * 255 / 31) as u8),
                            ((((value >> 6) & 0x1f) as u32 * 255 / 31) as u8),
                            ((((value >> 1) & 0x1f) as u32 * 255 / 31) as u8),
                            if value & 1 == 0 { 0 } else { 255 },
                        ),
                    }
                }
                _ => return None,
            };
            std::slice::from_raw_parts_mut(dst, 4).copy_from_slice(&[r, g, b, a]);
        }
    }
    Some(output)
}

pub struct PalettedTextureFormat {
    /// * `true` for 4-bit (nibble) index, 16-color palette.
    /// * `false` for 8-bit (byte) index, 256-color palette.
    pub index_is_nibble: bool,
    /// `glTexImage2D`-style `format` for palette entries: `GL_RGB` or `GL_RGBA`
    pub palette_entry_format: GLenum,
    /// `glTexImage2D`-style `type` for palette entries: `GL_UNSIGNED_BYTE` or
    /// some `GL_UNSIGNED_SHORT_` value
    pub palette_entry_type: GLenum,
}
impl PalettedTextureFormat {
    pub fn decode_rgba8(
        internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
        payload: &[u8],
    ) -> Option<Vec<u8>> {
        let info = Self::get_info(internalformat)?;
        let width = usize::try_from(width).ok()?;
        let height = usize::try_from(height).ok()?;
        let entry_size = match info.palette_entry_type {
            gles11::UNSIGNED_BYTE => {
                if info.palette_entry_format == gles11::RGB {
                    3
                } else {
                    4
                }
            }
            gles11::UNSIGNED_SHORT_5_6_5
            | gles11::UNSIGNED_SHORT_4_4_4_4
            | gles11::UNSIGNED_SHORT_5_5_5_1 => 2,
            _ => return None,
        };
        let palette_count: usize = if info.index_is_nibble { 16 } else { 256 };
        let palette_size = palette_count.checked_mul(entry_size)?;
        let pixel_count = width.checked_mul(height)?;
        let index_size = if info.index_is_nibble {
            pixel_count.checked_add(1)? / 2
        } else {
            pixel_count
        };
        let total_size = palette_size.checked_add(index_size)?;
        if payload.len() < total_size {
            return None;
        }
        let (palette, indices) = payload.split_at(palette_size);
        let mut output = Vec::with_capacity(pixel_count.checked_mul(4)?);
        for pixel in 0..pixel_count {
            let index = if info.index_is_nibble {
                let byte = indices[pixel / 2];
                if pixel % 2 == 0 {
                    byte >> 4
                } else {
                    byte & 0xf
                }
            } else {
                indices[pixel]
            } as usize;
            let entry = &palette[index * entry_size..][..entry_size];
            let (r, g, b, a) = match info.palette_entry_type {
                gles11::UNSIGNED_BYTE if entry_size == 3 => (entry[0], entry[1], entry[2], 255),
                gles11::UNSIGNED_BYTE => (entry[0], entry[1], entry[2], entry[3]),
                gles11::UNSIGNED_SHORT_5_6_5 => {
                    let value = u16::from_ne_bytes([entry[0], entry[1]]);
                    (
                        (((value >> 11) & 31) as u32 * 255 / 31) as u8,
                        (((value >> 5) & 63) as u32 * 255 / 63) as u8,
                        ((value & 31) as u32 * 255 / 31) as u8,
                        255,
                    )
                }
                gles11::UNSIGNED_SHORT_4_4_4_4 => {
                    let value = u16::from_ne_bytes([entry[0], entry[1]]);
                    (
                        (((value >> 12) & 15) as u8) * 17,
                        (((value >> 8) & 15) as u8) * 17,
                        (((value >> 4) & 15) as u8) * 17,
                        ((value & 15) as u8) * 17,
                    )
                }
                gles11::UNSIGNED_SHORT_5_5_5_1 => {
                    let value = u16::from_ne_bytes([entry[0], entry[1]]);
                    (
                        (((value >> 11) & 31) as u32 * 255 / 31) as u8,
                        (((value >> 6) & 31) as u32 * 255 / 31) as u8,
                        (((value >> 1) & 31) as u32 * 255 / 31) as u8,
                        if value & 1 == 0 { 0 } else { 255 },
                    )
                }
                _ => return None,
            };
            output.extend_from_slice(&[r, g, b, a]);
        }
        Some(output)
    }

    /// If the provided format is from `OES_compressed_paletted_texture`,
    /// return [Some] with information about it, or [None] otherwise.
    pub fn get_info(internalformat: GLenum) -> Option<Self> {
        match internalformat {
            gles11::PALETTE4_RGB8_OES => Some(Self {
                index_is_nibble: true,
                palette_entry_format: gles11::RGB,
                palette_entry_type: gles11::UNSIGNED_BYTE,
            }),
            gles11::PALETTE4_RGBA8_OES => Some(Self {
                index_is_nibble: true,
                palette_entry_format: gles11::RGBA,
                palette_entry_type: gles11::UNSIGNED_BYTE,
            }),
            gles11::PALETTE4_R5_G6_B5_OES => Some(Self {
                index_is_nibble: true,
                palette_entry_format: gles11::RGB,
                palette_entry_type: gles11::UNSIGNED_SHORT_5_6_5,
            }),
            gles11::PALETTE4_RGBA4_OES => Some(Self {
                index_is_nibble: true,
                palette_entry_format: gles11::RGBA,
                palette_entry_type: gles11::UNSIGNED_SHORT_4_4_4_4,
            }),
            gles11::PALETTE4_RGB5_A1_OES => Some(Self {
                index_is_nibble: true,
                palette_entry_format: gles11::RGBA,
                palette_entry_type: gles11::UNSIGNED_SHORT_5_5_5_1,
            }),
            gles11::PALETTE8_RGB8_OES => Some(Self {
                index_is_nibble: false,
                palette_entry_format: gles11::RGB,
                palette_entry_type: gles11::UNSIGNED_BYTE,
            }),
            gles11::PALETTE8_RGBA8_OES => Some(Self {
                index_is_nibble: false,
                palette_entry_format: gles11::RGBA,
                palette_entry_type: gles11::UNSIGNED_BYTE,
            }),
            gles11::PALETTE8_R5_G6_B5_OES => Some(Self {
                index_is_nibble: false,
                palette_entry_format: gles11::RGB,
                palette_entry_type: gles11::UNSIGNED_SHORT_5_6_5,
            }),
            gles11::PALETTE8_RGBA4_OES => Some(Self {
                index_is_nibble: false,
                palette_entry_format: gles11::RGBA,
                palette_entry_type: gles11::UNSIGNED_SHORT_4_4_4_4,
            }),
            gles11::PALETTE8_RGB5_A1_OES => Some(Self {
                index_is_nibble: false,
                palette_entry_format: gles11::RGBA,
                palette_entry_type: gles11::UNSIGNED_SHORT_5_5_5_1,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod vertex_attrib_state_tests {
    use super::VertexAttribState;
    use crate::gles::gles11_raw::types::*;
    use crate::gles::gles2_raw as gl;
    use crate::gles::GLES;

    // Models GL's per-attribute buffer capture, rather than treating the
    // global ARRAY_BUFFER binding as the source for every attribute.
    struct ArrayDriver {
        buffer: GLuint,
        arrays: [VertexAttribState; 2],
    }

    impl GLES for ArrayDriver {
        unsafe fn GetVertexAttribiv(&mut self, index: GLuint, pname: GLenum, value: *mut GLint) {
            let a = &self.arrays[index as usize];
            *value = match pname {
                gl::VERTEX_ATTRIB_ARRAY_ENABLED => a.enabled,
                gl::VERTEX_ATTRIB_ARRAY_BUFFER_BINDING => a.buffer,
                gl::VERTEX_ATTRIB_ARRAY_SIZE => a.size,
                gl::VERTEX_ATTRIB_ARRAY_STRIDE => a.stride,
                gl::VERTEX_ATTRIB_ARRAY_TYPE => a.type_,
                gl::VERTEX_ATTRIB_ARRAY_NORMALIZED => a.normalized,
                _ => panic!("unexpected attribute query"),
            };
        }
        unsafe fn GetVertexAttribPointerv(
            &mut self,
            index: GLuint,
            pname: GLenum,
            pointer: *mut *mut GLvoid,
        ) {
            assert_eq!(pname, gl::VERTEX_ATTRIB_ARRAY_POINTER);
            *pointer = self.arrays[index as usize].pointer;
        }
        unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
            assert_eq!(target, gl::ARRAY_BUFFER);
            self.buffer = buffer;
        }
        unsafe fn VertexAttribPointer(
            &mut self,
            index: GLuint,
            size: GLint,
            type_: GLenum,
            normalized: GLboolean,
            stride: GLsizei,
            pointer: *const GLvoid,
        ) {
            let a = &mut self.arrays[index as usize];
            a.buffer = self.buffer as GLint;
            a.size = size;
            a.type_ = type_ as GLint;
            a.normalized = normalized as GLint;
            a.stride = stride;
            a.pointer = pointer.cast_mut();
        }
        unsafe fn EnableVertexAttribArray(&mut self, index: GLuint) {
            self.arrays[index as usize].enabled = 1;
        }
        unsafe fn DisableVertexAttribArray(&mut self, index: GLuint) {
            self.arrays[index as usize].enabled = 0;
        }
    }

    #[test]
    fn internal_quad_preserves_cached_mesh_and_uv_arrays() {
        let mut driver = ArrayDriver {
            buffer: 99,
            arrays: [
                VertexAttribState {
                    index: 0,
                    enabled: 1,
                    buffer: 7,
                    size: 3,
                    stride: 32,
                    type_: gl::FLOAT as _,
                    normalized: 0,
                    pointer: 12usize as _,
                },
                VertexAttribState {
                    index: 1,
                    enabled: 0,
                    buffer: 0,
                    size: 2,
                    stride: 8,
                    type_: gl::UNSIGNED_SHORT as _,
                    normalized: 1,
                    pointer: 0x1234usize as _,
                },
            ],
        };
        unsafe {
            let saved = [0, 1].map(|index| VertexAttribState::capture(&mut driver, index));
            driver.BindBuffer(gl::ARRAY_BUFFER, 55);
            for index in [0, 1] {
                driver.EnableVertexAttribArray(index);
                driver.VertexAttribPointer(index, 2, gl::FLOAT, gl::FALSE, 16, std::ptr::null());
            }
            assert_ne!(driver.arrays, saved);
            for array in &saved {
                array.restore(&mut driver);
            }
            driver.BindBuffer(gl::ARRAY_BUFFER, 99);
            assert_eq!(driver.arrays, saved);
            assert_eq!(driver.buffer, 99);
        }
    }
}
