/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Software emulation of `GL_OES_vertex_array_object` state.
//!
//! The ES 1.1 extension string advertises `GL_OES_vertex_array_object`, but
//! the desktop backends (GLES1-on-GL2 and friends) can't back it with real
//! host VAOs. Without emulation, binding a VAO changed nothing but the element
//! array binding, so every draw used whatever vertex/normal/color/texcoord
//! pointers were specified last, no matter which VAO they were recorded in.
//! Unity 3.x's GLES1 renderer creates one VAO per mesh and only rebinds it
//! before drawing, so every object was drawn with another object's arrays:
//! text quads sampled the whole font atlas, logos followed the scrolling
//! background and 3D meshes turned into long spikes (Falling Fred Z).
//!
//! [VertexArrayState] mirrors every piece of client array state that the
//! extension says belongs to a VAO, except for the element array binding
//! (tracked separately by [super::eagl::GLShadowState]). When the guest
//! switches VAOs, [apply_vertex_array_state] issues the GL calls needed to turn
//! the outgoing VAO's state into the incoming one.

use crate::gles::gles11_raw as gles11;
use crate::gles::gles11_raw::types::{GLboolean, GLenum, GLint, GLsizei, GLuint, GLvoid};
use crate::gles::GLES;
use crate::mem::{ConstVoidPtr, Mem, Ptr};
use touchHLE_gl_bindings::gles11::ARRAY_BUFFER;

/// Texture units whose texture coordinate arrays are tracked. This is the
/// largest `GL_MAX_TEXTURE_UNITS` of any iPhone OS device.
pub const MAX_TEXTURE_UNITS: usize = 8;
/// Generic vertex attributes tracked for ES 2.0 contexts (the
/// `GL_MAX_VERTEX_ATTRIBS` of iPhone OS devices).
pub const MAX_VERTEX_ATTRIBS: usize = 16;

/// The fixed-function client arrays other than texture coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedArray {
    Vertex = 0,
    Normal,
    Color,
    PointSize,
    MatrixIndex,
    Weight,
}
const FIXED_ARRAY_COUNT: usize = 6;
const FIXED_ARRAYS: [FixedArray; FIXED_ARRAY_COUNT] = [
    FixedArray::Vertex,
    FixedArray::Normal,
    FixedArray::Color,
    FixedArray::PointSize,
    FixedArray::MatrixIndex,
    FixedArray::Weight,
];
impl FixedArray {
    pub(super) fn client_state(self) -> GLenum {
        match self {
            FixedArray::Vertex => gles11::VERTEX_ARRAY,
            FixedArray::Normal => gles11::NORMAL_ARRAY,
            FixedArray::Color => gles11::COLOR_ARRAY,
            FixedArray::PointSize => gles11::POINT_SIZE_ARRAY_OES,
            FixedArray::MatrixIndex => gles11::MATRIX_INDEX_ARRAY_OES,
            FixedArray::Weight => gles11::WEIGHT_ARRAY_OES,
        }
    }
}

/// Arguments of the last `gl*Pointer` call for one array.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrayPointer {
    /// Whether a pointer was ever specified while this VAO was bound.
    pub set: bool,
    pub size: GLint,
    pub type_: GLenum,
    pub normalized: GLboolean,
    pub stride: GLsizei,
    /// Guest pointer, or offset into `buffer` if that is non-zero.
    pub pointer: u32,
    /// `GL_ARRAY_BUFFER_BINDING` at the time of the `gl*Pointer` call.
    pub buffer: GLuint,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrayState {
    pub pointer: ArrayPointer,
    pub enabled: bool,
}

/// Client array state belonging to a single vertex array object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VertexArrayState {
    fixed: [ArrayState; FIXED_ARRAY_COUNT],
    tex_coords: [ArrayState; MAX_TEXTURE_UNITS],
    generic: [ArrayState; MAX_VERTEX_ATTRIBS],
}

impl VertexArrayState {
    pub fn fixed(&self, array: FixedArray) -> &ArrayState {
        &self.fixed[array as usize]
    }
    pub fn tex_coord(&self, unit: usize) -> Option<&ArrayState> {
        self.tex_coords.get(unit)
    }

    pub fn set_fixed_pointer(&mut self, array: FixedArray, pointer: ArrayPointer) {
        self.fixed[array as usize].pointer = pointer;
    }
    pub fn set_tex_coord_pointer(&mut self, unit: usize, pointer: ArrayPointer) {
        if let Some(state) = self.tex_coords.get_mut(unit) {
            state.pointer = pointer;
        }
    }
    pub fn set_generic_pointer(&mut self, index: usize, pointer: ArrayPointer) {
        if let Some(state) = self.generic.get_mut(index) {
            state.pointer = pointer;
        }
    }

    /// Record `glEnableClientState`/`glDisableClientState`. `client_unit` is
    /// the client active texture unit, used for `GL_TEXTURE_COORD_ARRAY`.
    pub fn set_client_state(&mut self, array: GLenum, client_unit: usize, enabled: bool) {
        if array == gles11::TEXTURE_COORD_ARRAY {
            if let Some(state) = self.tex_coords.get_mut(client_unit) {
                state.enabled = enabled;
            }
        } else if let Some(fixed) = FIXED_ARRAYS.iter().find(|a| a.client_state() == array) {
            self.fixed[*fixed as usize].enabled = enabled;
        }
    }
    pub fn set_generic_enabled(&mut self, index: usize, enabled: bool) {
        if let Some(state) = self.generic.get_mut(index) {
            state.enabled = enabled;
        }
    }
}

/// Where a pointer for `array_state` lives on the host: an offset into the
/// bound buffer, or a translated client-memory pointer.
fn host_pointer(mem: &Mem, pointer: &ArrayPointer) -> *const GLvoid {
    if pointer.buffer != 0 {
        pointer.pointer as usize as *const GLvoid
    } else if pointer.pointer == 0 {
        std::ptr::null()
    } else {
        let guest: ConstVoidPtr = Ptr::from_bits(pointer.pointer);
        mem.unchecked_ptr_at(guest.cast::<u8>(), 0).cast()
    }
}

/// Issue the GL calls that turn the client array state `from` (the state the
/// host context is currently in) into `to`.
///
/// `array_buffer` and `client_unit` are the guest's current
/// `GL_ARRAY_BUFFER_BINDING` and client active texture unit. Neither is VAO
/// state, so both are put back before returning.
///
/// # Safety
/// Must be called with `gles` current.
pub unsafe fn apply_vertex_array_state(
    gles: &mut dyn GLES,
    mem: &Mem,
    from: &VertexArrayState,
    to: &VertexArrayState,
    array_buffer: GLuint,
    client_unit: usize,
) {
    let mut bound_buffer = array_buffer;
    let mut bind = |gles: &mut dyn GLES, buffer: GLuint| {
        if bound_buffer != buffer {
            gles.BindBuffer(ARRAY_BUFFER, buffer);
            bound_buffer = buffer;
        }
    };
    let set_client_state = |gles: &mut dyn GLES, array: GLenum, enabled: bool| {
        if enabled {
            gles.EnableClientState(array)
        } else {
            gles.DisableClientState(array)
        }
    };

    for array in FIXED_ARRAYS {
        let old = from.fixed(array);
        let new = to.fixed(array);
        let p = new.pointer;
        // A pointer this VAO never specified is left alone: it's undefined
        // for the guest to draw with it anyway.
        if p.set && p != old.pointer {
            bind(gles, p.buffer);
            let ptr = host_pointer(mem, &p);
            match array {
                FixedArray::Vertex => gles.VertexPointer(p.size, p.type_, p.stride, ptr),
                FixedArray::Normal => gles.NormalPointer(p.type_, p.stride, ptr),
                FixedArray::Color => gles.ColorPointer(p.size, p.type_, p.stride, ptr),
                FixedArray::PointSize => gles.PointSizePointerOES(p.type_, p.stride, ptr),
                FixedArray::MatrixIndex => {
                    gles.MatrixIndexPointerOES(p.size, p.type_, p.stride, ptr)
                }
                FixedArray::Weight => gles.WeightPointerOES(p.size, p.type_, p.stride, ptr),
            }
        }
        if new.enabled != old.enabled {
            set_client_state(gles, array.client_state(), new.enabled);
        }
    }

    let mut active_unit = client_unit;
    for unit in 0..MAX_TEXTURE_UNITS {
        let old = &from.tex_coords[unit];
        let new = &to.tex_coords[unit];
        let p = new.pointer;
        let pointer_changed = p.set && p != old.pointer;
        if !pointer_changed && new.enabled == old.enabled {
            continue;
        }
        if active_unit != unit {
            gles.ClientActiveTexture(gles11::TEXTURE0 + unit as GLenum);
            active_unit = unit;
        }
        if pointer_changed {
            bind(gles, p.buffer);
            gles.TexCoordPointer(p.size, p.type_, p.stride, host_pointer(mem, &p));
        }
        if new.enabled != old.enabled {
            set_client_state(gles, gles11::TEXTURE_COORD_ARRAY, new.enabled);
        }
    }
    if active_unit != client_unit {
        gles.ClientActiveTexture(gles11::TEXTURE0 + client_unit as GLenum);
    }

    for index in 0..MAX_VERTEX_ATTRIBS {
        let old = &from.generic[index];
        let new = &to.generic[index];
        let p = new.pointer;
        if p.set && p != old.pointer {
            bind(gles, p.buffer);
            gles.VertexAttribPointer(
                index as GLuint,
                p.size,
                p.type_,
                p.normalized,
                p.stride,
                host_pointer(mem, &p),
            );
        }
        if new.enabled != old.enabled {
            if new.enabled {
                gles.EnableVertexAttribArray(index as GLuint);
            } else {
                gles.DisableVertexAttribArray(index as GLuint);
            }
        }
    }

    bind(gles, array_buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frameworks::opengles::eagl::GLShadowState;

    fn pointer(pointer: u32, buffer: GLuint) -> ArrayPointer {
        ArrayPointer {
            set: true,
            size: 3,
            type_: gles11::FLOAT,
            normalized: 0,
            stride: 24,
            pointer,
            buffer,
        }
    }

    #[test]
    fn client_state_tracks_the_right_array() {
        let mut state = VertexArrayState::default();
        state.set_client_state(gles11::VERTEX_ARRAY, 0, true);
        state.set_client_state(gles11::TEXTURE_COORD_ARRAY, 1, true);
        state.set_client_state(gles11::WEIGHT_ARRAY_OES, 0, true);
        assert!(state.fixed(FixedArray::Vertex).enabled);
        assert!(state.fixed(FixedArray::Weight).enabled);
        assert!(!state.fixed(FixedArray::Normal).enabled);
        assert!(!state.tex_coord(0).unwrap().enabled);
        assert!(state.tex_coord(1).unwrap().enabled);

        state.set_client_state(gles11::VERTEX_ARRAY, 0, false);
        assert!(!state.fixed(FixedArray::Vertex).enabled);
        // Out-of-range units and unknown arrays are ignored, not a panic.
        state.set_client_state(gles11::TEXTURE_COORD_ARRAY, MAX_TEXTURE_UNITS, true);
        state.set_client_state(0xdead, 0, true);
    }

    #[test]
    fn states_are_independent_per_vao() {
        let mut a = VertexArrayState::default();
        a.set_fixed_pointer(FixedArray::Vertex, pointer(0x10, 7));
        a.set_tex_coord_pointer(0, pointer(0x1c, 7));
        let mut b = a.clone();
        b.set_fixed_pointer(FixedArray::Vertex, pointer(0x2000_0000, 0));
        assert_eq!(a.fixed(FixedArray::Vertex).pointer, pointer(0x10, 7));
        assert_ne!(a, b);
        assert_eq!(a.tex_coord(0), b.tex_coord(0));
    }

    /// Client array state of a host driver without vertex array objects:
    /// one global set of arrays, as with the desktop GLES1 backends.
    #[derive(Default)]
    struct HostArrays {
        array_buffer: GLuint,
        client_unit: usize,
        vertex: Option<(GLuint, usize)>,
        color: Option<(GLuint, usize)>,
        tex_coords: [Option<(GLuint, usize)>; MAX_TEXTURE_UNITS],
        enabled: Vec<(GLenum, usize)>,
    }
    impl HostArrays {
        fn is_enabled(&self, array: GLenum, unit: usize) -> bool {
            self.enabled.contains(&(array, unit))
        }
    }
    #[allow(non_snake_case)]
    impl GLES for HostArrays {
        unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
            assert_eq!(target, ARRAY_BUFFER);
            self.array_buffer = buffer;
        }
        unsafe fn ClientActiveTexture(&mut self, texture: GLenum) {
            self.client_unit = (texture - gles11::TEXTURE0) as usize;
        }
        unsafe fn VertexPointer(&mut self, _: GLint, _: GLenum, _: GLsizei, p: *const GLvoid) {
            self.vertex = Some((self.array_buffer, p as usize));
        }
        unsafe fn ColorPointer(&mut self, _: GLint, _: GLenum, _: GLsizei, p: *const GLvoid) {
            self.color = Some((self.array_buffer, p as usize));
        }
        unsafe fn TexCoordPointer(&mut self, _: GLint, _: GLenum, _: GLsizei, p: *const GLvoid) {
            self.tex_coords[self.client_unit] = Some((self.array_buffer, p as usize));
        }
        unsafe fn EnableClientState(&mut self, array: GLenum) {
            let unit = if array == gles11::TEXTURE_COORD_ARRAY {
                self.client_unit
            } else {
                0
            };
            if !self.is_enabled(array, unit) {
                self.enabled.push((array, unit));
            }
        }
        unsafe fn DisableClientState(&mut self, array: GLenum) {
            let unit = if array == gles11::TEXTURE_COORD_ARRAY {
                self.client_unit
            } else {
                0
            };
            self.enabled.retain(|&e| e != (array, unit));
        }
    }

    /// The guest-facing side, doing what the `gles_guest` wrappers do.
    struct Guest {
        shadow: GLShadowState,
        host: HostArrays,
        mem: Mem,
    }
    impl Guest {
        fn new() -> Guest {
            Guest {
                shadow: GLShadowState::default(),
                host: HostArrays::default(),
                mem: Mem::new(),
            }
        }
        fn bind_buffer(&mut self, buffer: GLuint) {
            self.shadow.array_buffer = buffer;
            unsafe { self.host.BindBuffer(ARRAY_BUFFER, buffer) };
        }
        fn client_active_texture(&mut self, unit: usize) {
            self.shadow.client_active_texture = unit;
            unsafe {
                self.host
                    .ClientActiveTexture(gles11::TEXTURE0 + unit as GLenum)
            };
        }
        fn vertex_pointer(&mut self, offset: u32) {
            let record = ArrayPointer {
                pointer: offset,
                ..pointer(0, self.shadow.array_buffer)
            };
            self.shadow
                .vertex_array_state
                .set_fixed_pointer(FixedArray::Vertex, record);
            unsafe {
                self.host
                    .VertexPointer(3, gles11::FLOAT, 24, offset as usize as _)
            };
        }
        fn color_pointer(&mut self, offset: u32) {
            let record = ArrayPointer {
                pointer: offset,
                ..pointer(0, self.shadow.array_buffer)
            };
            self.shadow
                .vertex_array_state
                .set_fixed_pointer(FixedArray::Color, record);
            unsafe {
                self.host
                    .ColorPointer(4, gles11::UNSIGNED_BYTE, 24, offset as usize as _)
            };
        }
        fn tex_coord_pointer(&mut self, offset: u32) {
            let record = ArrayPointer {
                pointer: offset,
                ..pointer(0, self.shadow.array_buffer)
            };
            let unit = self.shadow.client_active_texture;
            self.shadow
                .vertex_array_state
                .set_tex_coord_pointer(unit, record);
            unsafe {
                self.host
                    .TexCoordPointer(2, gles11::FLOAT, 24, offset as usize as _)
            };
        }
        fn enable(&mut self, array: GLenum) {
            let unit = self.shadow.client_active_texture;
            self.shadow
                .vertex_array_state
                .set_client_state(array, unit, true);
            unsafe { self.host.EnableClientState(array) };
        }
        fn apply(&mut self, outgoing: Option<VertexArrayState>) {
            if let Some(outgoing) = outgoing {
                unsafe {
                    apply_vertex_array_state(
                        &mut self.host,
                        &self.mem,
                        &outgoing,
                        &self.shadow.vertex_array_state,
                        self.shadow.array_buffer,
                        self.shadow.client_active_texture,
                    )
                };
            }
        }
        fn bind_vertex_array(&mut self, array: GLuint) {
            let (_, outgoing) = self.shadow.bind_software_vertex_array(array);
            self.apply(outgoing);
        }
        fn delete_vertex_array(&mut self, array: GLuint) {
            if let Some((_, outgoing)) = self.shadow.delete_software_vertex_array(array) {
                self.apply(outgoing);
            }
        }
    }

    /// How Unity 3.x's GLES1 renderer uses VAOs: set every mesh's arrays up
    /// once, then only rebind the VAO before each draw.
    #[test]
    fn rebinding_a_vao_restores_its_arrays() {
        let mut guest = Guest::new();
        let a = guest.shadow.generate_software_vertex_array();
        let b = guest.shadow.generate_software_vertex_array();

        guest.bind_vertex_array(a);
        guest.bind_buffer(10);
        guest.vertex_pointer(0);
        guest.client_active_texture(1);
        guest.tex_coord_pointer(12);
        guest.enable(gles11::TEXTURE_COORD_ARRAY);
        guest.client_active_texture(0);
        guest.enable(gles11::VERTEX_ARRAY);

        guest.bind_vertex_array(b);
        guest.bind_buffer(20);
        guest.vertex_pointer(4);
        guest.color_pointer(16);
        guest.enable(gles11::VERTEX_ARRAY);
        guest.enable(gles11::COLOR_ARRAY);
        guest.bind_buffer(0);

        guest.bind_vertex_array(a);
        let host = &guest.host;
        assert_eq!(host.vertex, Some((10, 0)));
        assert_eq!(host.tex_coords[1], Some((10, 12)));
        assert!(host.is_enabled(gles11::VERTEX_ARRAY, 0));
        assert!(host.is_enabled(gles11::TEXTURE_COORD_ARRAY, 1));
        assert!(!host.is_enabled(gles11::COLOR_ARRAY, 0));
        // Neither the array buffer binding nor the client active texture
        // belongs to the VAO.
        assert_eq!(host.array_buffer, 0);
        assert_eq!(host.client_unit, 0);

        guest.bind_vertex_array(b);
        let host = &guest.host;
        assert_eq!(host.vertex, Some((20, 4)));
        assert_eq!(host.color, Some((20, 16)));
        assert!(host.is_enabled(gles11::VERTEX_ARRAY, 0));
        assert!(host.is_enabled(gles11::COLOR_ARRAY, 0));
        assert!(!host.is_enabled(gles11::TEXTURE_COORD_ARRAY, 1));

        // Binding the bound VAO again must not lose its state.
        guest.bind_vertex_array(b);
        assert_eq!(guest.host.vertex, Some((20, 4)));
        assert!(guest.host.is_enabled(gles11::COLOR_ARRAY, 0));

        // The default VAO never enabled anything.
        guest.bind_vertex_array(0);
        assert!(guest.host.enabled.is_empty());

        // Deleting the bound VAO falls back to the default VAO's state.
        guest.bind_vertex_array(a);
        assert!(!guest.host.enabled.is_empty());
        guest.delete_vertex_array(a);
        assert!(guest.host.enabled.is_empty());
        assert_eq!(guest.shadow.current_vertex_array, 0);
    }
}
