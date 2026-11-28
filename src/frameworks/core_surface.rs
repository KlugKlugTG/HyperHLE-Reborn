/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CoreSurface, the original iPhone OS shared framebuffer API.
//! <https://github.com/rileytestut/SNES4iOS/blob/master/CoreSurface/CoreSurface.h>
//! <https://developer.apple.com/documentation/iosurface>

use crate::dyld::{export_c_func, ConstantExports, HostConstant, HostDylib};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::image::Image;
use crate::mem::{Mem, MutVoidPtr, Ptr, PAGE_SIZE_ALIGN_MASK};
use crate::objc::{id, msg, nil, objc_classes, retain, HostObject, ObjC};
use crate::Environment;
use std::collections::HashMap;

const BAD_ARGUMENT: i32 = 0xe00002c2u32 as i32; // kIOReturnBadArgument

// CoreSurface takes CFString keys and CFNumber values (including the FourCC).
macro_rules! surface_keys {
    ($( $name:ident = $value:literal ),+ $(,)?) => {
        $(const $name: &str = $value;)+
        const KEYS: ConstantExports = &[
            $((concat!("_", stringify!($name)), HostConstant::NSString($name))),+
        ];
    }
}

surface_keys! {
    kCoreSurfaceBufferWidth = "CoreSurfaceBufferWidth",
    kCoreSurfaceBufferHeight = "CoreSurfaceBufferHeight",
    kCoreSurfaceBufferPitch = "CoreSurfaceBufferPitch",
    kCoreSurfaceBufferPixelFormat = "CoreSurfaceBufferPixelFormat",
    kCoreSurfaceBufferAllocSize = "CoreSurfaceBufferAllocSize",
    kCoreSurfaceBufferClientAddress = "CoreSurfaceBufferClientAddress",
    kCoreSurfaceBufferMemoryRegion = "CoreSurfaceBufferMemoryRegion",
    kCoreSurfaceBufferGlobal = "CoreSurfaceBufferGlobal",
    kCoreSurfaceBufferOffset = "CoreSurfaceBufferOffset",
}

#[derive(Default)]
pub(super) struct State {
    last_id: u32,
    // A lookup owns a new reference. The registry itself must not retain buffers.
    global_buffers: HashMap<u32, id>,
}

#[derive(Default)]
pub(crate) struct Surface {
    number: u32,
    width: u32,
    height: u32,
    pitch: u32,
    format: u32,
    size: u32,
    storage: MutVoidPtr,
    offset: u32,
    owns_storage: bool,
    lock_count: u32,
}
impl HostObject for Surface {}

pub(crate) fn surface(objc: &ObjC, object: id) -> Option<&Surface> {
    objc.get_host_object(object)?.as_any().downcast_ref()
}

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const RGBA: u32 = u32::from_be_bytes(*b"RGBA");
const ARGB: u32 = u32::from_be_bytes(*b"ARGB");
const ABGR: u32 = u32::from_be_bytes(*b"ABGR");
const RGB565_LE: u32 = u32::from_be_bytes(*b"L565");
const RGB565_BE: u32 = u32::from_be_bytes(*b"B565");
const RGB555_LE: u32 = u32::from_be_bytes(*b"L555");
const RGB555_BE: u32 = u32::from_be_bytes(*b"B555");

fn bytes_per_pixel(format: u32) -> Option<u32> {
    match format {
        BGRA | RGBA | ARGB | ABGR | 32 => Some(4),
        RGB565_LE | RGB565_BE | RGB555_LE | RGB555_BE | 16 => Some(2),
        24 => Some(3),
        _ => None,
    }
}

impl Surface {
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn address(&self) -> MutVoidPtr {
        (self.storage.cast::<u8>() + self.offset).cast()
    }

    pub(crate) fn image(&self, mem: &Mem, opaque: bool) -> Option<Image> {
        let bpp = bytes_per_pixel(self.format)? as usize;
        let rows = mem.bytes_at(self.address().cast(), self.pitch * self.height);
        let width = self.width as usize;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(width.checked_mul(self.height as usize)?.checked_mul(4)?)
            .ok()?;
        for row in rows.chunks_exact(self.pitch as usize) {
            for p in row[..width * bpp].chunks_exact(bpp) {
                let [r, g, b, a] = match self.format {
                    BGRA => [p[2], p[1], p[0], p[3]],
                    RGBA => [p[0], p[1], p[2], p[3]],
                    ABGR => [p[3], p[2], p[1], p[0]],
                    ARGB | 32 => [p[1], p[2], p[3], p[0]],
                    24 => [p[0], p[1], p[2], 255],
                    _ => {
                        let word = if matches!(self.format, RGB565_LE | RGB555_LE) {
                            u16::from_le_bytes([p[0], p[1]])
                        } else {
                            u16::from_be_bytes([p[0], p[1]])
                        };
                        let green_bits = if matches!(self.format, RGB565_LE | RGB565_BE) {
                            6
                        } else {
                            5
                        };
                        let g = ((word >> 5) & ((1 << green_bits) - 1)) as u8;
                        let r = ((word >> (5 + green_bits)) & 31) as u8;
                        let b = (word & 31) as u8;
                        [
                            r << 3 | r >> 2,
                            g << (8 - green_bits) | g >> (2 * green_bits - 8),
                            b << 3 | b >> 2,
                            255,
                        ]
                    }
                };
                pixels.extend_from_slice(&[r, g, b, if opaque { 255 } else { a }]);
            }
        }
        Some(Image::from_pixel_vec(pixels, self.dimensions()))
    }
}

fn property(env: &mut Environment, dict: id, key: &'static str) -> Option<u32> {
    let key = get_static_str(env, key);
    let value: id = msg![env; dict objectForKey:key];
    (value != nil).then(|| msg![env; value unsignedIntValue])
}

fn create(env: &mut Environment, properties: id) -> Option<id> {
    let width = property(env, properties, kCoreSurfaceBufferWidth)?;
    let height = property(env, properties, kCoreSurfaceBufferHeight)?;
    if width == 0 || height == 0 {
        return None;
    }
    let format = property(env, properties, kCoreSurfaceBufferPixelFormat).unwrap_or(0);
    // Unknown pixel formats can still be shared as raw memory with an explicit stride.
    let min_pitch = width.checked_mul(bytes_per_pixel(format).unwrap_or(0))?;
    let pitch = property(env, properties, kCoreSurfaceBufferPitch).unwrap_or(min_pitch);
    let offset = property(env, properties, kCoreSurfaceBufferOffset).unwrap_or(0);
    let required = pitch.checked_mul(height)?.checked_add(offset)?;
    let size = property(env, properties, kCoreSurfaceBufferAllocSize).unwrap_or(required);
    if pitch == 0 || pitch < min_pitch || size < required {
        return None;
    }
    let global = property(env, properties, kCoreSurfaceBufferGlobal).unwrap_or(0) != 0;
    let number = env.framework_state.core_surface.last_id.checked_add(1)?;
    let client = property(env, properties, kCoreSurfaceBufferClientAddress);
    let storage = if let Some(base) = client {
        if base < env.mem.null_segment_size() || base.checked_add(size - 1).is_none() {
            return None;
        }
        Ptr::from_bits(base)
    } else {
        let rounded = size.checked_add(PAGE_SIZE_ALIGN_MASK)? & !PAGE_SIZE_ALIGN_MASK;
        env.mem.alloc_aligned(rounded, PAGE_SIZE_ALIGN_MASK, 0)
    };
    if storage.is_null() {
        return None;
    }
    let buffer = Surface {
        number,
        width,
        height,
        format,
        pitch,
        offset,
        size,
        storage,
        owns_storage: client.is_none(),
        lock_count: 0,
    };
    let class = env
        .objc
        .get_known_class("_touchHLE_CoreSurfaceBuffer", &mut env.mem);
    let object = env.objc.alloc_object(class, Box::new(buffer), &mut env.mem);
    if object == nil {
        if client.is_none() {
            env.mem.free(storage);
        }
        return None;
    }
    let state = &mut env.framework_state.core_surface;
    state.last_id = number;
    if global {
        state.global_buffers.insert(number, object);
    }
    Some(object)
}

fn CoreSurfaceBufferCreate(env: &mut Environment, properties: id) -> id {
    create(env, properties).unwrap_or(nil)
}

fn CoreSurfaceBufferLookup(env: &mut Environment, number: u32) -> id {
    let object = env
        .framework_state
        .core_surface
        .global_buffers
        .get(&number)
        .copied()
        .unwrap_or(nil);
    retain(env, object)
}

fn CoreSurfaceBufferGetBaseAddress(env: &mut Environment, buffer: id) -> MutVoidPtr {
    surface(&env.objc, buffer).map_or(Ptr::null(), Surface::address)
}

fn CoreSurfaceBufferGetAllocSize(env: &mut Environment, buffer: id) -> u32 {
    surface(&env.objc, buffer).map_or(0, |s| s.size)
}
fn CoreSurfaceBufferGetWidth(env: &mut Environment, buffer: id) -> u32 {
    surface(&env.objc, buffer).map_or(0, |s| s.width)
}
fn CoreSurfaceBufferGetHeight(env: &mut Environment, buffer: id) -> u32 {
    surface(&env.objc, buffer).map_or(0, |s| s.height)
}
fn CoreSurfaceBufferGetBytesPerRow(env: &mut Environment, buffer: id) -> u32 {
    surface(&env.objc, buffer).map_or(0, |s| s.pitch)
}
fn CoreSurfaceBufferGetPixelFormatType(env: &mut Environment, buffer: id) -> u32 {
    surface(&env.objc, buffer).map_or(0, |s| s.format)
}
fn CoreSurfaceBufferGetID(env: &mut Environment, buffer: id) -> u32 {
    surface(&env.objc, buffer).map_or(0, |s| s.number)
}
fn CoreSurfaceBufferGetPlaneCount(_env: &mut Environment, _buffer: id) -> u32 {
    // Packed buffers have no separate planes.
    0
}

fn CoreSurfaceBufferLock(env: &mut Environment, buffer: id, _lock_type: u32) -> i32 {
    let Some(surface) = surface(&env.objc, buffer) else {
        return BAD_ARGUMENT;
    };
    let Some(count) = surface.lock_count.checked_add(1) else {
        return BAD_ARGUMENT;
    };
    env.objc.borrow_mut::<Surface>(buffer).lock_count = count;
    0
}

fn CoreSurfaceBufferUnlock(env: &mut Environment, buffer: id) -> i32 {
    let Some(surface) = surface(&env.objc, buffer) else {
        return BAD_ARGUMENT;
    };
    let Some(count) = surface.lock_count.checked_sub(1) else {
        return BAD_ARGUMENT;
    };
    env.objc.borrow_mut::<Surface>(buffer).lock_count = count;
    0
}

fn CoreSurfaceBufferWrapClientMemory(env: &mut Environment, buffer: id) -> i32 {
    // For a client allocation Create already mapped the supplied guest address.
    match surface(&env.objc, buffer) {
        Some(surface) if !surface.owns_storage => 0,
        _ => BAD_ARGUMENT,
    }
}

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/PrivateFrameworks/CoreSurface.framework/CoreSurface",
    aliases: &[],
    class_exports: &[objc_classes! {
        (env, this, _cmd);
        @implementation _touchHLE_CoreSurfaceBuffer: NSObject
        - (())dealloc {
            let surface = env.objc.borrow::<Surface>(this);
            env.framework_state.core_surface.global_buffers.remove(&surface.number);
            if surface.owns_storage {
                env.mem.free(surface.storage);
            }
            env.objc.dealloc_object(this, &mut env.mem)
        }
        @end
    }],
    constant_exports: &[KEYS],
    function_exports: &[&[
        export_c_func!(CoreSurfaceBufferCreate(_)),
        export_c_func!(CoreSurfaceBufferWrapClientMemory(_)),
        export_c_func!(CoreSurfaceBufferLookup(_)),
        export_c_func!(CoreSurfaceBufferGetBaseAddress(_)),
        export_c_func!(CoreSurfaceBufferGetAllocSize(_)),
        export_c_func!(CoreSurfaceBufferGetWidth(_)),
        export_c_func!(CoreSurfaceBufferGetHeight(_)),
        export_c_func!(CoreSurfaceBufferGetBytesPerRow(_)),
        export_c_func!(CoreSurfaceBufferGetPixelFormatType(_)),
        export_c_func!(CoreSurfaceBufferGetID(_)),
        export_c_func!(CoreSurfaceBufferGetPlaneCount(_)),
        export_c_func!(CoreSurfaceBufferLock(_, _)),
        export_c_func!(CoreSurfaceBufferUnlock(_)),
    ]],
};
