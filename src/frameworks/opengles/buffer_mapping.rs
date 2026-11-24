/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Guest-addressable buffer mappings, shared by an EAGL sharegroup.
//!
//! Mapping permissions and lifetimes follow GL_EXT_map_buffer_range and
//! GL_OES_mapbuffer. In particular, reading a write-only driver mapping or
//! writing a read-only one can cause a native SIGSEGV, and explicit flushes
//! must not be replaced with a whole-buffer upload at unmap.
//! <https://registry.khronos.org/OpenGL/extensions/EXT/EXT_map_buffer_range.txt>
//! <https://registry.khronos.org/OpenGL/extensions/OES/OES_mapbuffer.txt>

use crate::gles::{gles30_raw as gl, GLES};
use crate::mem::{ConstVoidPtr, GuestISize, GuestUSize, Mem, MutVoidPtr, Ptr};
use std::collections::HashMap;
use std::ops::Range;

/// A buffer object's mapping, independent of the target used to bind it.
#[derive(Clone, Copy)]
pub(super) struct Mapping {
    pub guest: MutVoidPtr,
    driver: Option<*mut u8>,
    pub offset: usize,
    pub length: usize,
    pub access: u32,
}

#[derive(Default)]
pub(super) struct BufferObjects {
    mappings: HashMap<u32, Mapping>,
    /// ES 1.1 drivers may lack range mapping and buffer readback entirely.
    /// Their buffers only receive CPU uploads, so a shadow preserves the
    /// unwritten bytes required by OES_mapbuffer. Backends with real range
    /// mapping read the actual store, including GPU-generated buffer data.
    contents: HashMap<u32, Vec<u8>>,
}

pub(super) fn binding(gles: &mut dyn GLES, target: u32) -> Result<u32, u32> {
    let pname = match target {
        gl::ARRAY_BUFFER => gl::ARRAY_BUFFER_BINDING,
        gl::ELEMENT_ARRAY_BUFFER => gl::ELEMENT_ARRAY_BUFFER_BINDING,
        gl::COPY_READ_BUFFER if gles.is_es3() => gl::COPY_READ_BUFFER_BINDING,
        gl::COPY_WRITE_BUFFER if gles.is_es3() => gl::COPY_WRITE_BUFFER_BINDING,
        gl::PIXEL_PACK_BUFFER if gles.is_es3() => gl::PIXEL_PACK_BUFFER_BINDING,
        gl::PIXEL_UNPACK_BUFFER if gles.is_es3() => gl::PIXEL_UNPACK_BUFFER_BINDING,
        gl::TRANSFORM_FEEDBACK_BUFFER if gles.is_es3() => gl::TRANSFORM_FEEDBACK_BUFFER_BINDING,
        gl::UNIFORM_BUFFER if gles.is_es3() => gl::UNIFORM_BUFFER_BINDING,
        _ => return Err(gl::INVALID_ENUM),
    };
    let mut name = 0;
    unsafe { gles.GetIntegerv(pname, &mut name) };
    if name == 0 {
        Err(gl::INVALID_OPERATION)
    } else {
        Ok(name as u32)
    }
}

fn buffer_size(gles: &mut dyn GLES, target: u32) -> usize {
    let mut size = 0;
    unsafe { gles.GetBufferParameteriv(target, gl::BUFFER_SIZE, &mut size) };
    usize::try_from(size).unwrap_or(0)
}

fn range(offset: GuestISize, length: GuestISize, limit: usize) -> Result<Range<usize>, u32> {
    let offset = usize::try_from(offset).map_err(|_| gl::INVALID_VALUE)?;
    let length = usize::try_from(length).map_err(|_| gl::INVALID_VALUE)?;
    let end = offset
        .checked_add(length)
        .filter(|&end| end <= limit)
        .ok_or(gl::INVALID_VALUE)?;
    Ok(offset..end)
}

fn validate_access(access: u32) -> Result<(), u32> {
    const ALLOWED: u32 = gl::MAP_READ_BIT
        | gl::MAP_WRITE_BIT
        | gl::MAP_INVALIDATE_RANGE_BIT
        | gl::MAP_INVALIDATE_BUFFER_BIT
        | gl::MAP_FLUSH_EXPLICIT_BIT
        | gl::MAP_UNSYNCHRONIZED_BIT;
    if access & !ALLOWED != 0 {
        return Err(gl::INVALID_VALUE);
    }
    if access & (gl::MAP_READ_BIT | gl::MAP_WRITE_BIT) == 0
        || access & gl::MAP_READ_BIT != 0
            && access
                & (gl::MAP_INVALIDATE_RANGE_BIT
                    | gl::MAP_INVALIDATE_BUFFER_BIT
                    | gl::MAP_UNSYNCHRONIZED_BIT)
                != 0
        || access & gl::MAP_FLUSH_EXPLICIT_BIT != 0 && access & gl::MAP_WRITE_BIT == 0
    {
        return Err(gl::INVALID_OPERATION);
    }
    Ok(())
}

impl BufferObjects {
    pub fn mapping(&self, buffer: u32) -> Option<Mapping> {
        self.mappings.get(&buffer).copied()
    }

    /// Called only after a successful glBufferData. Re-specifying storage
    /// implicitly unmaps it, invalidating both the host and guest pointers.
    pub fn data(
        &mut self,
        gles: &mut dyn GLES,
        mem: &mut Mem,
        buffer: u32,
        size: GuestISize,
        data: ConstVoidPtr,
    ) {
        self.delete_mapping(mem, buffer);
        if gles.supports_map_buffer_range() {
            self.contents.remove(&buffer);
        } else if data.is_null() {
            self.contents.insert(buffer, vec![0; size as usize]);
        } else {
            self.contents.insert(
                buffer,
                mem.bytes_at(data.cast(), size as GuestUSize).to_vec(),
            );
        }
    }

    pub fn sub_data(&mut self, buffer: u32, offset: usize, data: &[u8]) {
        if let Some(contents) = self.contents.get_mut(&buffer) {
            contents[offset..offset + data.len()].copy_from_slice(data);
        }
    }

    pub fn validate_sub_data(
        &self,
        gles: &mut dyn GLES,
        target: u32,
        buffer: u32,
        offset: GuestISize,
        length: GuestISize,
    ) -> Result<Range<usize>, u32> {
        if self.mappings.contains_key(&buffer) {
            return Err(gl::INVALID_OPERATION);
        }
        range(offset, length, buffer_size(gles, target))
    }

    pub fn delete(&mut self, mem: &mut Mem, buffer: u32) {
        self.delete_mapping(mem, buffer);
        self.contents.remove(&buffer);
    }

    fn delete_mapping(&mut self, mem: &mut Mem, buffer: u32) {
        if let Some(mapping) = self.mappings.remove(&buffer) {
            mem.free(mapping.guest);
        }
    }

    pub fn free_all(&mut self, mem: &mut Mem) {
        for (_, mapping) in self.mappings.drain() {
            mem.free(mapping.guest);
        }
        self.contents.clear();
    }

    pub fn map_oes(
        &mut self,
        gles: &mut dyn GLES,
        mem: &mut Mem,
        target: u32,
        access: u32,
    ) -> Result<MutVoidPtr, u32> {
        // OES_mapbuffer explicitly restricts access to WRITE_ONLY_OES.
        if access != 0x88b9 {
            return Err(gl::INVALID_ENUM);
        }
        binding(gles, target)?;
        let size =
            GuestISize::try_from(buffer_size(gles, target)).map_err(|_| gl::OUT_OF_MEMORY)?;
        self.map(gles, mem, target, 0, size, gl::MAP_WRITE_BIT)
    }

    pub fn map(
        &mut self,
        gles: &mut dyn GLES,
        mem: &mut Mem,
        target: u32,
        offset: GuestISize,
        length: GuestISize,
        access: u32,
    ) -> Result<MutVoidPtr, u32> {
        let buffer = binding(gles, target)?;
        validate_access(access)?;
        let mapped = range(offset, length, buffer_size(gles, target))?;
        if mapped.is_empty() || self.mappings.contains_key(&buffer) {
            return Err(gl::INVALID_OPERATION);
        }
        let preserve = access & (gl::MAP_INVALIDATE_RANGE_BIT | gl::MAP_INVALIDATE_BUFFER_BIT) == 0;
        // The guest cannot read a write-only mapping, but untouched bytes
        // must retain their original values. Request readable host storage
        // when seeding that guest mirror; never read a write-only pointer.
        let host_access = if preserve {
            (access | gl::MAP_READ_BIT) & !gl::MAP_UNSYNCHRONIZED_BIT
        } else {
            access
        };
        let driver = if gles.supports_map_buffer_range() {
            let pointer =
                unsafe { gles.MapBufferRange(target, offset as _, length as _, host_access) };
            if pointer.is_null() {
                return Ok(Ptr::null());
            }
            Some(pointer.cast::<u8>())
        } else {
            None
        };
        let guest: MutVoidPtr = mem.alloc(length as GuestUSize).cast();
        if preserve {
            let contents = mem.bytes_at_mut(guest.cast(), length as GuestUSize);
            if let Some(pointer) = driver {
                // SAFETY: the driver mapping includes MAP_READ_BIT and its
                // size was checked against the buffer's data store above.
                contents
                    .copy_from_slice(unsafe { std::slice::from_raw_parts(pointer, mapped.len()) });
            } else if let Some(source) = self.contents.get(&buffer) {
                contents.copy_from_slice(&source[mapped.clone()]);
            }
        }
        self.mappings.insert(
            buffer,
            Mapping {
                guest,
                driver,
                offset: mapped.start,
                length: mapped.len(),
                access,
            },
        );
        Ok(guest)
    }

    fn upload(
        &mut self,
        gles: &mut dyn GLES,
        mem: &Mem,
        target: u32,
        buffer: u32,
        mapping: Mapping,
        subrange: Range<usize>,
    ) {
        let bytes = mem.bytes_at(mapping.guest.cast(), mapping.length as GuestUSize);
        let bytes = &bytes[subrange.clone()];
        unsafe {
            if let Some(pointer) = mapping.driver {
                // Only writable mappings enter here. BufferData and
                // DeleteBuffers remove mappings before a stale pointer can
                // be used again, including through another shared context.
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    pointer.add(subrange.start),
                    bytes.len(),
                );
            } else {
                gles.BufferSubData(
                    target,
                    (mapping.offset + subrange.start) as _,
                    bytes.len() as _,
                    bytes.as_ptr().cast(),
                );
            }
        }
        self.sub_data(buffer, mapping.offset + subrange.start, bytes);
    }

    pub fn flush(
        &mut self,
        gles: &mut dyn GLES,
        mem: &Mem,
        target: u32,
        offset: GuestISize,
        length: GuestISize,
    ) -> Result<(), u32> {
        let buffer = binding(gles, target)?;
        let mapping = self.mapping(buffer).ok_or(gl::INVALID_OPERATION)?;
        if mapping.access & gl::MAP_FLUSH_EXPLICIT_BIT == 0 {
            return Err(gl::INVALID_OPERATION);
        }
        let subrange = range(offset, length, mapping.length)?;
        self.upload(gles, mem, target, buffer, mapping, subrange);
        if mapping.driver.is_some() {
            unsafe { gles.FlushMappedBufferRange(target, offset as _, length as _) };
        }
        Ok(())
    }

    pub fn unmap(&mut self, gles: &mut dyn GLES, mem: &mut Mem, target: u32) -> Result<u8, u32> {
        let buffer = binding(gles, target)?;
        let mapping = self.mappings.remove(&buffer).ok_or(gl::INVALID_OPERATION)?;
        if mapping.access & gl::MAP_WRITE_BIT != 0
            && mapping.access & gl::MAP_FLUSH_EXPLICIT_BIT == 0
        {
            self.upload(gles, mem, target, buffer, mapping, 0..mapping.length);
        }
        let result = if mapping.driver.is_some() {
            unsafe { gles.UnmapBuffer(target) }
        } else {
            gl::TRUE
        };
        mem.free(mapping.guest);
        Ok(result)
    }

    pub fn parameter(&self, buffer: u32, pname: u32) -> Option<i64> {
        let mapping = self.mapping(buffer);
        match pname {
            gl::BUFFER_MAPPED => Some(i64::from(mapping.is_some())),
            gl::BUFFER_ACCESS_FLAGS => Some(mapping.map_or(0, |m| m.access as i64)),
            gl::BUFFER_MAP_OFFSET => Some(mapping.map_or(0, |m| m.offset as i64)),
            gl::BUFFER_MAP_LENGTH => Some(mapping.map_or(0, |m| m.length as i64)),
            // BUFFER_ACCESS_OES also exists in ES 3.0's mapbuffer extension.
            0x88bb => Some(match mapping.map(|m| m.access & 3) {
                Some(gl::MAP_READ_BIT) => 0x88b8,
                Some(3) => 0x88ba,
                _ => 0x88b9,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Driver {
        data: Vec<u8>,
        native: bool,
        map_access: u32,
        flushes: Vec<(isize, isize)>,
        unmaps: usize,
    }
    #[allow(non_snake_case)]
    impl GLES for Driver {
        fn supports_map_buffer_range(&self) -> bool {
            self.native
        }
        unsafe fn GetIntegerv(&mut self, _: u32, value: *mut i32) {
            *value = 7;
        }
        unsafe fn GetBufferParameteriv(&mut self, _: u32, _: u32, value: *mut i32) {
            *value = self.data.len() as i32;
        }
        unsafe fn MapBufferRange(
            &mut self,
            _: u32,
            offset: isize,
            _: isize,
            access: u32,
        ) -> *mut std::ffi::c_void {
            self.map_access = access;
            self.data.as_mut_ptr().add(offset as usize).cast()
        }
        unsafe fn FlushMappedBufferRange(&mut self, _: u32, offset: isize, length: isize) {
            self.flushes.push((offset, length));
        }
        unsafe fn UnmapBuffer(&mut self, _: u32) -> u8 {
            self.unmaps += 1;
            gl::TRUE
        }
        unsafe fn BufferSubData(
            &mut self,
            _: u32,
            offset: isize,
            length: isize,
            data: *const std::ffi::c_void,
        ) {
            let bytes = std::slice::from_raw_parts(data.cast::<u8>(), length as usize);
            self.data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
        }
    }
    fn driver(native: bool) -> Driver {
        Driver {
            data: (0..32).collect(),
            native,
            map_access: 0,
            flushes: Vec::new(),
            unmaps: 0,
        }
    }
    fn setup(native: bool) -> (Driver, Mem, BufferObjects) {
        let driver = driver(native);
        let mut buffers = BufferObjects::default();
        buffers.contents.insert(7, driver.data.clone());
        let mut mem = Mem::new();
        // Like a loaded app's __PAGEZERO, reserve NULL before allocating
        // guest buffers. Large allocations otherwise legitimately start at 0.
        mem.set_null_segment_size(0x1000);
        (driver, mem, buffers)
    }

    #[test]
    fn ext_streaming_upload_works_without_native_range_mapping() {
        let (mut driver, mut mem, mut buffers) = setup(false);
        driver.data.resize(88_056, 0);
        buffers.contents.insert(7, driver.data.clone());
        let pointer = buffers
            .map(
                &mut driver,
                &mut mem,
                gl::ARRAY_BUFFER,
                0,
                88_056,
                gl::MAP_WRITE_BIT | gl::MAP_INVALIDATE_BUFFER_BIT,
            )
            .unwrap();
        assert!(!pointer.is_null());
        mem.bytes_at_mut(pointer.cast(), 88_056).fill(0x5a);
        assert_eq!(
            buffers.unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER),
            Ok(gl::TRUE)
        );
        assert!(driver.data.iter().all(|&b| b == 0x5a));
    }

    #[test]
    fn readonly_unmap_does_not_write_to_the_driver() {
        let (mut driver, mut mem, mut buffers) = setup(true);
        let pointer = buffers
            .map(
                &mut driver,
                &mut mem,
                gl::ARRAY_BUFFER,
                8,
                8,
                gl::MAP_READ_BIT,
            )
            .unwrap();
        assert_eq!(
            mem.bytes_at(pointer.cast(), 8),
            &(8..16).collect::<Vec<_>>()
        );
        // A guest mirror can be modified without making the host mapping
        // writable. The old unmap implementation wrote through it anyway.
        mem.bytes_at_mut(pointer.cast(), 8).fill(0xee);
        buffers
            .unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER)
            .unwrap();
        assert_eq!(driver.data, (0..32).collect::<Vec<_>>());
        assert_eq!(driver.unmaps, 1);
    }

    #[test]
    fn invalidated_write_mapping_is_never_read() {
        let (mut driver, mut mem, mut buffers) = setup(true);
        let access = gl::MAP_WRITE_BIT | gl::MAP_INVALIDATE_RANGE_BIT;
        let pointer = buffers
            .map(&mut driver, &mut mem, gl::ARRAY_BUFFER, 8, 8, access)
            .unwrap();
        assert_eq!(driver.map_access, access);
        mem.bytes_at_mut(pointer.cast(), 8).fill(0x55);
        buffers
            .unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER)
            .unwrap();
        assert_eq!(&driver.data[8..16], &[0x55; 8]);
    }

    #[test]
    fn explicit_flush_is_relative_to_mapping_and_unmap_does_not_upload_more() {
        for native in [false, true] {
            let (mut driver, mut mem, mut buffers) = setup(native);
            let access = gl::MAP_WRITE_BIT | gl::MAP_FLUSH_EXPLICIT_BIT;
            let pointer = buffers
                .map(&mut driver, &mut mem, gl::ARRAY_BUFFER, 8, 12, access)
                .unwrap();
            mem.bytes_at_mut(pointer.cast(), 12).fill(0xaa);
            buffers
                .flush(&mut driver, &mem, gl::ARRAY_BUFFER, 3, 4)
                .unwrap();
            mem.bytes_at_mut(pointer.cast(), 12).fill(0xbb);
            buffers
                .unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER)
                .unwrap();
            let mut expected: Vec<u8> = (0..32).collect();
            expected[11..15].fill(0xaa);
            assert_eq!(driver.data, expected);
            if native {
                assert_eq!(driver.flushes, [(3, 4)]);
            }
        }
    }

    #[test]
    fn oes_mapping_preserves_unwritten_bytes_and_rejects_double_map() {
        for native in [false, true] {
            let (mut driver, mut mem, mut buffers) = setup(native);
            let pointer = buffers
                .map_oes(&mut driver, &mut mem, gl::ARRAY_BUFFER, 0x88b9)
                .unwrap();
            assert_eq!(
                buffers.map_oes(&mut driver, &mut mem, gl::ARRAY_BUFFER, 0x88b9),
                Err(gl::INVALID_OPERATION)
            );
            mem.bytes_at_mut(pointer.cast(), 4).fill(0x44);
            buffers
                .unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER)
                .unwrap();
            assert_eq!(&driver.data[..4], &[0x44; 4]);
            assert_eq!(&driver.data[4..], &(4..32).collect::<Vec<_>>());
        }
    }

    #[test]
    fn queries_and_rebinding_follow_the_buffer_object() {
        let (mut driver, mut mem, mut buffers) = setup(true);
        let pointer = buffers
            .map(
                &mut driver,
                &mut mem,
                gl::ARRAY_BUFFER,
                4,
                8,
                gl::MAP_WRITE_BIT,
            )
            .unwrap();
        assert_eq!(buffers.mapping(7).unwrap().guest, pointer);
        assert_eq!(buffers.parameter(7, gl::BUFFER_MAP_OFFSET), Some(4));
        assert_eq!(buffers.parameter(7, gl::BUFFER_MAP_LENGTH), Some(8));
        assert_eq!(
            buffers.parameter(7, gl::BUFFER_ACCESS_FLAGS),
            Some(gl::MAP_WRITE_BIT as i64)
        );
        buffers
            .unmap(&mut driver, &mut mem, gl::ELEMENT_ARRAY_BUFFER)
            .unwrap();
        assert_eq!(buffers.parameter(7, gl::BUFFER_MAPPED), Some(0));
        assert!(buffers.mapping(7).is_none());
    }

    #[test]
    fn deleting_or_respecifying_storage_invalidates_mapping() {
        let (mut driver, mut mem, mut buffers) = setup(true);
        let _ = buffers
            .map(
                &mut driver,
                &mut mem,
                gl::ARRAY_BUFFER,
                0,
                8,
                gl::MAP_WRITE_BIT,
            )
            .unwrap();
        buffers.data(&mut driver, &mut mem, 7, 32, Ptr::null());
        assert_eq!(
            buffers.unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER),
            Err(gl::INVALID_OPERATION)
        );
        let _ = buffers
            .map(
                &mut driver,
                &mut mem,
                gl::ARRAY_BUFFER,
                0,
                8,
                gl::MAP_WRITE_BIT,
            )
            .unwrap();
        buffers.delete(&mut mem, 7);
        assert!(buffers.mapping(7).is_none());
        assert_eq!(driver.unmaps, 0);
    }

    #[test]
    fn invalid_ranges_and_access_flags_are_rejected_without_mapping() {
        let (mut driver, mut mem, mut buffers) = setup(true);
        for (offset, length, access, error) in [
            (-1, 8, gl::MAP_WRITE_BIT, gl::INVALID_VALUE),
            (24, 16, gl::MAP_WRITE_BIT, gl::INVALID_VALUE),
            (0, 0, gl::MAP_WRITE_BIT, gl::INVALID_OPERATION),
            (0, 8, 0, gl::INVALID_OPERATION),
            (0, 8, 0x100, gl::INVALID_VALUE),
            (
                0,
                8,
                gl::MAP_READ_BIT | gl::MAP_INVALIDATE_BUFFER_BIT,
                gl::INVALID_OPERATION,
            ),
            (
                0,
                8,
                gl::MAP_READ_BIT | gl::MAP_FLUSH_EXPLICIT_BIT,
                gl::INVALID_OPERATION,
            ),
        ] {
            assert_eq!(
                buffers.map(
                    &mut driver,
                    &mut mem,
                    gl::ARRAY_BUFFER,
                    offset,
                    length,
                    access
                ),
                Err(error)
            );
        }
        assert_eq!(driver.map_access, 0);
        let pointer = buffers
            .map(
                &mut driver,
                &mut mem,
                gl::ARRAY_BUFFER,
                0,
                8,
                gl::MAP_WRITE_BIT | gl::MAP_FLUSH_EXPLICIT_BIT,
            )
            .unwrap();
        assert_eq!(
            buffers.flush(&mut driver, &mem, gl::ARRAY_BUFFER, 4, 8),
            Err(gl::INVALID_VALUE)
        );
        assert_eq!(
            buffers.validate_sub_data(&mut driver, gl::ARRAY_BUFFER, 7, 0, 4),
            Err(gl::INVALID_OPERATION)
        );
        assert_eq!(buffers.mapping(7).unwrap().guest, pointer);
        buffers
            .unmap(&mut driver, &mut mem, gl::ARRAY_BUFFER)
            .unwrap();
    }
}
