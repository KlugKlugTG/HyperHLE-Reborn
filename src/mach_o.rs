/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Reading of Mach-O files, the executable and library format on iPhone OS.
//!
//! Implemented using the mach_object crate. All usage of that crate should be
//! confined to this module. The goal is to read the Mach-O binary exactly once,
//! storing any information we'll need later.
//!
//! Useful resources:
//! - Apple's [Overview of the Mach-O Executable Format](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/CodeFootprint/Articles/MachOOverview.html) explains what "segments" and "sections" are, and provides short descriptions of the purposes of some common sections.
//! - Apple's old "OS X ABI Mach-O File Format Reference", which is mirrored in [various](https://github.com/aidansteele/osx-abi-macho-file-format-reference) [places](https://www.symbolcrash.com/wp-content/uploads/2019/02/ABI_MachOFormat.pdf) online.
//! - Alex Drummond's [Inside a Hello World executable on OS X](https://adrummond.net/posts/macho) is about macOS circa 2017 rather than iPhone OS circa 2008, so not all of what it says applies, but the sections up to and including "9. The indirect symbol table" are helpful.
//! - The LLVM functions [`RuntimeDyldMachO::populateIndirectSymbolPointersSection`](https://github.com/llvm/llvm-project/blob/2e999b7dd1934a44d38c3a753460f1e5a217e9a5/llvm/lib/ExecutionEngine/RuntimeDyld/RuntimeDyldMachO.cpp#L179-L220) and [`MachOObjectFile::getIndirectSymbolTableEntry`](https://github.com/llvm/llvm-project/blob/3c09ed006ab35dd8faac03311b14f0857b01949c/llvm/lib/Object/MachOObjectFile.cpp#L4803-L4808) are references for how to read the indirect symbol table.
//! - `/usr/include/mach-o/reloc.h` in the macOS SDK was the reference for the
//!   format of relocation entries.
//! - The [source code of the mach_object crate](https://docs.rs/mach_object/latest/src/mach_object/commands.rs.html) has useful comments that don't show up in the generated documentation, e.g. around `DySymTab`.

use crate::abi::GuestFunction;
use crate::fs::{Fs, GuestPath};
use crate::mem::{GuestUSize, Mem, Ptr};
use mach_object::{
    cpu_subtype_t, vm_prot_t, Bind, BindSymbolType, DyLib, ExportKind, ExportTrie, LoadCommand,
    MachCommand, OFile, Rebase, Symbol, SymbolIter, ThreadState, N_ARM_THUMB_DEF,
    S_LAZY_SYMBOL_POINTERS, S_MOD_INIT_FUNC_POINTERS, S_NON_LAZY_SYMBOL_POINTERS, S_SYMBOL_STUBS,
};
use std::collections::HashMap;
use std::io::{Cursor, Seek, SeekFrom};

const VM_PROT_READ: vm_prot_t = 1;
const VM_PROT_WRITE: vm_prot_t = 2;
#[allow(dead_code)]
const VM_PROT_EXECUTE: vm_prot_t = 4;

#[derive(Debug)]
pub struct MachO {
    /// Name (for debugging purposes and sorting)
    pub name: String,
    /// Guest filesystem path used to load the image.
    pub loaded_path: String,
    /// Paths of dynamic libraries referenced by the binary.
    pub dynamic_libraries: Vec<String>,
    /// LC_RPATH entries, in the order recorded by the linker.
    pub run_paths: Vec<String>,
    /// Offset added to the image's preferred virtual addresses.
    pub slide: u32,
    /// Metadata related to sections.
    pub sections: Vec<Section>,
    /// All defined symbols in the binary, including local symbols. Use
    /// `external_symbols` for dependent-dylib resolution. Thumb function symbols
    /// always have the Thumb bit set.
    pub exported_symbols: HashMap<String, u32>,
    /// Defined symbols marked external in the Mach-O symbol table, used for
    /// dependent-dylib resolution.
    pub external_symbols: HashMap<String, u32>,
    /// List of addresses and names of external relocations for the dynamic
    /// linker to resolve.
    pub external_relocations: Vec<(u32, String)>,
    /// Address/program counter value for the entry point.
    pub entry_point_pc: Option<u32>,
    /// Whether the entry point came from an `LC_MAIN` load command rather
    /// than `LC_UNIXTHREAD`/`LC_THREAD`. With `LC_MAIN`, the entry point is
    /// the C `main` function itself, which dyld calls with
    /// `(argc, argv, envp, apple)` passed in registers per the calling
    /// convention, rather than a `start` routine that reads them from the
    /// stack.
    pub entry_point_is_lc_main: bool,
    /// End address of the highest-addressed segment.
    /// This is used by get_end() to return the first address after the last
    /// segment in the executable.
    pub last_segment_end: u32,
    /// Base address of the __TEXT segment (where the mach_header lives in
    /// memory). Used by `_dyld_get_image_header`.
    pub text_base: u32,
}

#[derive(Debug)]
pub struct Section {
    /// Section name.
    pub name: String,
    /// Segment containing the section; section names can repeat by segment.
    pub segment_name: String,
    /// Section address in memory.
    pub addr: u32,
    /// Section size in bytes.
    pub size: u32,
    /// What type of section is this?
    pub type_: SectionType,
    /// Information specific to special dynamic linker sections, if this is one.
    pub dyld_indirect_symbol_info: Option<DyldIndirectSymbolInfo>,
}

impl Section {
    /// Returns address of the next section.
    pub fn next_section_addr(&self) -> u32 {
        self.addr + self.size
    }
}

/// Various kinds of sections. We're only interested in the ones used by the
/// dynamic linker for now (see also [DyldIndirectSymbolInfo]).
#[derive(Debug, PartialEq, Eq)]
pub enum SectionType {
    /// Normal section, or some section type we don't care about.
    Normal,
    /// Symbol stub section, usually called `__symbol_stub4` or
    /// `__picsymbolstub4`.
    SymbolStubs,
    /// Lazy symbol pointer section, usually called `__la_symbol_ptr`.
    LazySymbolPointers,
    /// Non-lazy symbol pointer section, usually called `__nl_symbol_ptr`.
    NonLazySymbolPointers,
    /// Initialization function pointer section, usually called
    /// `__mod_init_func`.
    ModInitFuncPointers,
}

/// Information relevant to certain special sections which contain a series of
/// pointers or stub functions for indirectly referencing dynamically-linked
/// symbols (see also [SectionType]).
#[derive(Debug)]
pub struct DyldIndirectSymbolInfo {
    /// The size in bytes of an entry (pointer or stub function) in the section.
    pub entry_size: u32,
    /// A list of symbol names corresponding to the entries.
    pub indirect_undef_symbols: Vec<Option<String>>,
}

/// Helper trait that makes [MachO::get_section] work. Yes, this is overkill. :)
pub trait SectionPredicate {
    fn test(&self, section: &Section) -> bool;
}
impl SectionPredicate for &str {
    fn test(&self, section: &Section) -> bool {
        section.name == *self
    }
}
impl SectionPredicate for SectionType {
    fn test(&self, section: &Section) -> bool {
        section.type_ == *self
    }
}

fn get_sym_by_idx<'a>(
    idx: u32,
    (symoff, nsyms, stroff, strsize): (u32, u32, u32, u32),
    is_bigend: bool,
    is_64bit: bool,
    cursor: &'a mut Cursor<&'a [u8]>,
) -> Option<mach_object::Symbol<'a>> {
    if idx >= nsyms {
        return None;
    }

    let symoff = (symoff + idx * 12) as u64;

    cursor.seek(SeekFrom::Start(symoff)).unwrap();

    // This is not how you're supposed to use SymbolIter but the parse_symbol()
    // method on it requires the bytestring crate, so...
    let mut iter = SymbolIter::new(cursor, Vec::new(), 1, stroff, strsize, is_bigend, is_64bit);
    iter.next()
}

/// Parsed relocation entry
#[derive(Debug)]
enum Reloc {
    External {
        addr: u32,
        sym_idx: u32,
        is_pc_relative: bool,
        size: u32,
        type_: u32,
    },
    #[allow(dead_code)]
    Local {
        addr: u32,
        section_idx: u32,
        is_pc_relative: bool,
        size: u32,
        type_: u32,
    },
    #[allow(dead_code)]
    Scattered {
        offset: u32,
        value: u32,
        is_pc_relative: bool,
        size: u32,
        type_: u32,
    },
}
impl Reloc {
    fn parse(is_bigend: bool, entry: [u8; 8]) -> Self {
        assert!(!is_bigend);

        let word1 = u32::from_le_bytes(entry[..4].try_into().unwrap());
        let word2 = u32::from_le_bytes(entry[4..8].try_into().unwrap());

        if (word1 & 0x80000000) != 0 {
            let bitfield = word1;
            let value = word2;

            let offset = bitfield & 0xffffff;
            let type_ = (bitfield >> 24) & 0xf;
            let size = 1 << ((bitfield >> 28) & 3); // log2-encoded pointer size
            let is_pc_relative = ((bitfield >> 30) & 1) == 1;

            Reloc::Scattered {
                offset,
                value,
                is_pc_relative,
                size,
                type_,
            }
        } else {
            let addr = word1;
            let bitfield = word2;

            let sym_or_section_idx = bitfield & 0xffffff;
            let is_pc_relative = ((bitfield >> 24) & 1) == 1;
            let size = 1 << ((bitfield >> 25) & 3); // log2-encoded pointer size
            let is_external = (bitfield >> 27) & 1;
            let type_ = (bitfield >> 28) & 0xf;

            if is_external == 1 {
                Reloc::External {
                    addr,
                    sym_idx: sym_or_section_idx,
                    is_pc_relative,
                    size,
                    type_,
                }
            } else {
                Reloc::Local {
                    addr,
                    section_idx: sym_or_section_idx,
                    is_pc_relative,
                    size,
                    type_,
                }
            }
        }
    }
}

/// Subtypes supported by the 32-bit iPhone OS ARM backend, in selection
/// preference order. Keep generic ARMv7 ahead of specialized ARMv7 variants.
/// ARM_ALL is a generic ARM image, not a wildcard accepting arbitrary ISAs.
fn arm_subtype_priority(subtype: cpu_subtype_t) -> Option<u8> {
    // Mach-O reserves the high byte for capability requirements. No such
    // requirements are supported by this ARM32 backend (in particular LIB64
    // is not valid here). Do not silently strip unknown required capabilities.
    if (subtype as u32) & 0xff00_0000 != 0 {
        return None;
    }
    match subtype {
        mach_object::CPU_SUBTYPE_ARM_V7 => Some(4),
        mach_object::CPU_SUBTYPE_ARM_V7S => Some(3),
        mach_object::CPU_SUBTYPE_ARM_V7F => Some(2),
        mach_object::CPU_SUBTYPE_ARM_V6 => Some(1),
        mach_object::CPU_SUBTYPE_ARM_ALL => Some(0),
        // Excludes older ARM/XScale, watchOS ARMv7k, ARMv8 and unknown values.
        _ => None,
    }
}

fn cpu_subtype_to_str(ty: cpu_subtype_t) -> &'static str {
    match ty {
        mach_object::CPU_SUBTYPE_ARM_ALL => "armv???",
        mach_object::CPU_SUBTYPE_ARM_V4T => "armv4t",
        mach_object::CPU_SUBTYPE_ARM_V5TEJ => "armv5tej",
        mach_object::CPU_SUBTYPE_ARM_V6 => "armv6",
        mach_object::CPU_SUBTYPE_ARM_XSCALE => "armxscale",
        mach_object::CPU_SUBTYPE_ARM_V7 => "armv7",
        mach_object::CPU_SUBTYPE_ARM_V7F => "armv7f",
        mach_object::CPU_SUBTYPE_ARM_V7S => "armv7s",
        mach_object::CPU_SUBTYPE_ARM_V7K => "armv7k",
        mach_object::CPU_SUBTYPE_ARM_V8 => "armv8",
        _ => {
            log!(
                "Warning: cpu_subtype_to_str: unexpected cpu subtype: {:?}; using 'armv???'.",
                ty
            );
            "armv???"
        }
    }
}

fn record_defined_symbol(
    defined_symbols: &mut HashMap<String, u32>,
    external_symbols: &mut HashMap<String, u32>,
    name: &str,
    address: u32,
    external: bool,
) {
    defined_symbols.insert(name.to_string(), address);
    if external {
        external_symbols.insert(name.to_string(), address);
    }
}

// Modern stripped dylibs keep their public symbols in the export trie,
// which is authoritative even when the nlist table marks them private.
// Regular and thread-local addresses are relative to the Mach-O header;
// absolute exports retain their recorded address.
// https://github.com/apple-oss-distributions/dyld/blob/dyld-360.22/src/ImageLoaderMachOCompressed.cpp
fn record_export_trie(
    bytes: &[u8],
    offset: u32,
    size: u32,
    image_base: u32,
    defined_symbols: &mut HashMap<String, u32>,
    external_symbols: &mut HashMap<String, u32>,
) -> Result<(), &'static str> {
    if size == 0 {
        return Ok(());
    }
    let start = offset as usize;
    let end = start
        .checked_add(size as usize)
        .ok_or("Mach-O export trie range overflows")?;
    let payload = bytes
        .get(start..end)
        .ok_or("Mach-O export trie extends past end of file")?;
    let trie = ExportTrie::parse(payload).map_err(|_| "Malformed Mach-O export trie")?;
    for symbol in trie.symbols() {
        // Reexported definitions are supplied by the dependency's image.
        let Some(address) = symbol.address() else {
            continue;
        };
        let address = u32::try_from(address).map_err(|_| "Export address exceeds 32 bits")?;
        let address = match symbol.kind {
            ExportKind::Absolute => address,
            _ => image_base
                .checked_add(address)
                .ok_or("Export address overflows")?,
        };
        record_defined_symbol(
            defined_symbols,
            external_symbols,
            &symbol.name,
            address,
            true,
        );
    }
    Ok(())
}

/// Only the virtual tail (vmsize - filesize) is zero-filled. Missing bytes
/// inside the declared file range are corruption, not BSS.
fn validate_segment_file_range(
    fileoff: u64,
    filesize: u64,
    vmsize: u64,
    file_len: usize,
) -> Result<(), &'static str> {
    if filesize > vmsize {
        return Err("Mach-O segment file size exceeds its virtual size");
    }
    if filesize == 0 {
        return Ok(());
    }
    let end = fileoff
        .checked_add(filesize)
        .ok_or("Mach-O segment file range overflows")?;
    if end > file_len as u64 {
        return Err("Mach-O segment extends past end of file; executable is truncated or malformed (check the IPA)");
    }
    Ok(())
}

fn validate_segment_ranges(commands: &[MachCommand], file_len: usize) -> Result<(), &'static str> {
    for MachCommand(command, _) in commands {
        if let LoadCommand::Segment {
            segname,
            fileoff,
            filesize,
            vmsize,
            ..
        } = command
        {
            // mach_object exposes these fields as usize. Use checked
            // conversions: usize does not implement Into<u64> in Rust.
            let offset =
                u64::try_from(*fileoff).map_err(|_| "Segment file offset does not fit u64")?;
            let size =
                u64::try_from(*filesize).map_err(|_| "Segment file size does not fit u64")?;
            let virtual_size =
                u64::try_from(*vmsize).map_err(|_| "Segment virtual size does not fit u64")?;
            if let Err(error) = validate_segment_file_range(offset, size, virtual_size, file_len) {
                log!("Rejecting Mach-O: segment {} fileoff={:#x} filesize={:#x} vmsize={:#x}, file length={:#x}: {}",
                    segname, fileoff, filesize, vmsize, file_len, error);
                return Err(error);
            }
        }
    }
    Ok(())
}

impl MachO {
    /// Load the all the sections from a Mach-O binary (provided as `bytes`)
    /// into the guest memory (`into_mem`), and return a struct containing
    /// metadata (e.g. symbols).
    pub fn load_from_bytes(
        bytes: &[u8],
        into_mem: &mut Mem,
        name: String,
        slide_to_address: u32,
    ) -> Result<MachO, &'static str> {
        Self::load_with_placement(bytes, into_mem, name, Some(slide_to_address))
    }

    fn load_with_placement(
        bytes: &[u8],
        into_mem: &mut Mem,
        name: String,
        requested_slide: Option<u32>,
    ) -> Result<MachO, &'static str> {
        log_dbg!("Reading {:?}", name);

        let mut cursor = Cursor::new(bytes);

        let file = OFile::parse(&mut cursor).map_err(|_| "Could not parse Mach-O file")?;

        let (header, commands) = match file {
            OFile::MachFile { header, commands } => (header, commands),
            OFile::FatFile { files, .. } => {
                let mut best_subslice: Option<&[u8]> = None;
                let mut best_priority = None;
                let mut had_invalid_slice = false;
                for (arch, _) in files {
                    if arch.cputype != mach_object::CPU_TYPE_ARM {
                        continue;
                    }
                    let Some(priority) = arm_subtype_priority(arch.cpusubtype) else {
                        log!(
                            "Skipping unsupported ARM FAT subtype {:#x}",
                            arch.cpusubtype
                        );
                        had_invalid_slice = true;
                        continue;
                    };
                    // Per Apple's Mach-O FAT documentation, each `fat_arch`
                    // gives an absolute offset and size into the wrapping FAT
                    // file. Some malformed / partially-downloaded IPAs (e.g.
                    // Bike Race Free) have a header that describes a slice
                    // larger than the bytes we actually have on disk, which
                    // used to crash with `range end index N out of range for
                    // slice of length M`. Bounds-check here and skip the slice
                    // instead of panicking; we'll fall back to whatever other
                    // slice is present, or surface a clean error.
                    let off = arch.offset as usize;
                    let size = arch.size as usize;
                    let end = off.checked_add(size);
                    let in_bounds = matches!(end, Some(e) if e <= bytes.len());
                    if !in_bounds {
                        log!(
                            "Warning: FAT slice (cputype {:#x}, cpusubtype {:#x}) declares \
                             range {:#x}..{:#x}, past end of file ({:#x}); skipping this slice.",
                            arch.cputype,
                            arch.cpusubtype,
                            off,
                            off.saturating_add(size),
                            bytes.len(),
                        );
                        had_invalid_slice = true;
                        continue;
                    }
                    // A FAT range can fit while the Mach-O inside it declares
                    // missing segment bytes. Validate before ranking candidates,
                    // and before any guest memory is reserved for a candidate.
                    let subslice = &bytes[off..off + size];
                    let usable = match OFile::parse(&mut Cursor::new(subslice)) {
                        Ok(OFile::MachFile { header, commands }) => {
                            header.cputype == mach_object::CPU_TYPE_ARM
                                && header.cpusubtype == arch.cpusubtype
                                && arm_subtype_priority(header.cpusubtype).is_some()
                                && !header.is_64bit()
                                && !header.is_bigend()
                                && validate_segment_ranges(&commands, subslice.len()).is_ok()
                        }
                        _ => false,
                    };
                    if !usable {
                        log!(
                            "Skipping invalid ARM FAT slice at {:#x} (subtype {:#x})",
                            off,
                            arch.cpusubtype
                        );
                        had_invalid_slice = true;
                        continue;
                    }
                    if best_priority.map_or(true, |best| priority > best) {
                        best_subslice = Some(subslice);
                        best_priority = Some(priority);
                    }
                }
                return if let Some(subslice) = best_subslice {
                    MachO::load_with_placement(subslice, into_mem, name, requested_slide)
                } else if had_invalid_slice {
                    Err("FAT binary has no usable ARM slice: slices are truncated, malformed or unsupported (check the IPA)")
                } else {
                    Err("No supported architecture in the fat binary")
                };
            }
            OFile::ArFile { .. } | OFile::SymDef { .. } => {
                return Err("Unexpected Mach-O file kind: not an executable");
            }
        };

        if header.cputype != mach_object::CPU_TYPE_ARM {
            return Err("Executable is not for an ARM CPU!");
        }

        let is_bigend = header.is_bigend();
        if is_bigend {
            return Err("Executable is not little-endian!");
        }
        let is_64bit = header.is_64bit();
        if is_64bit {
            return Err("Executable is not 32-bit!");
        }
        if arm_subtype_priority(header.cpusubtype).is_none() {
            log!(
                "Rejecting {:?}: unsupported ARM CPU subtype {:#x}",
                name,
                header.cpusubtype
            );
            return Err("Unsupported ARM CPU subtype or capabilities (expected ARM_ALL, ARMv6, ARMv7, ARMv7f or ARMv7s)");
        }

        log!(
            "Loading {} slice for {:?}",
            cpu_subtype_to_str(header.cpusubtype),
            name
        );

        // Do not mutate guest memory until every file-backed segment, including
        // __LINKEDIT, is known to fit the selected thin image.
        validate_segment_ranges(&commands, bytes.len())?;

        let slide = match requested_slide {
            Some(slide) => slide,
            None => Self::available_image_slide(&commands, into_mem)?,
        };

        let split_segs = (header.flags & mach_object::MH_SPLIT_SEGS) != 0;

        // Info used while parsing file
        let mut first_segment_base: Option<u32> = None;
        let mut first_read_write_segment_base: Option<u32> = None;
        let mut text_segment_base: Option<u32> = None;
        let mut all_sections = Vec::new();
        let mut sym_tab_info: Option<(u32, u32, u32, u32)> = None;
        let mut segment_offsets = Vec::new();
        let mut last_segment_end: u32 = 0;

        // Info used for the result
        let mut dynamic_libraries = Vec::new();
        let mut run_paths = Vec::new();
        let mut exported_symbols = HashMap::new();
        let mut external_symbols = HashMap::new();
        let mut indirect_undef_symbols: Vec<Option<String>> = Vec::new();
        let mut external_relocations: Vec<(u32, String)> = Vec::new();
        let mut entry_point_pc: Option<u32> = None;
        let mut entry_point_is_lc_main = false;

        for MachCommand(command, _size) in commands {
            match command {
                LoadCommand::Segment {
                    segname,
                    vmaddr,
                    vmsize,
                    fileoff,
                    filesize,
                    initprot,
                    sections,
                    ..
                } => {
                    let vmaddr: u32 = vmaddr.try_into().unwrap();
                    let vmsize: u32 = vmsize.try_into().unwrap();
                    let filesize: u32 = filesize.try_into().unwrap();

                    if first_segment_base.is_none() {
                        first_segment_base = Some(vmaddr + slide);
                    }
                    if first_read_write_segment_base.is_none()
                        && (initprot & VM_PROT_READ) != 0
                        && (initprot & VM_PROT_WRITE) != 0
                    {
                        first_read_write_segment_base = Some(vmaddr + slide);
                    }

                    let load_me = match &*segname {
                        // Special linker data section, not meant to be loaded.
                        "__LINKEDIT" => false,
                        // Zero page needs to be handled seperately.
                        "__PAGEZERO" => {
                            assert!(vmaddr == 0);
                            assert!(filesize == 0);
                            into_mem.set_null_segment_size(vmsize);
                            false
                        }
                        "__TEXT" => {
                            assert!(text_segment_base.is_none());
                            text_segment_base = Some(vmaddr + slide);
                            true
                        }
                        "__DATA" => true,
                        // `__RESTRICT` is a one-section segment Apple's
                        // linker emits to mark a binary as non-injectable
                        // (it disables `DYLD_INSERT_LIBRARIES` and the
                        // task-port debugging path in dyld). It contains a
                        // `__restrict` C-string section; the segment is
                        // documented in Apple's open-source dyld
                        // (`dyld/src/ImageLoaderMachO.cpp`) and xnu's
                        // `bsd/kern/mach_loader.c`. We don't enforce that
                        // policy in HLE, so treat the segment as a regular
                        // read-only data segment without spamming a warning.
                        "__RESTRICT" => true,
                        // `__S3E_DATA` is a data segment used by the Marmalade SDK
                        // (formerly Airplay SDK), which some older iOS games link with.
                        "__S3E_DATA" => true,
                        _ => {
                            log!("Warning: Unexpected segment name: {}", segname);
                            true
                        }
                    };

                    // Apple's xnu kernel (`bsd/kern/mach_loader.c`,
                    // `parse_machfile()` → `load_segment()`) explicitly
                    // skips `LC_SEGMENT` commands whose `vmsize == 0` —
                    // they reserve no address space and have no data.
                    // Some shipping iOS binaries (e.g. games containing a
                    // dummy `__RESTRICT` segment from older linkers) emit
                    // zero-vmsize segments; reserving 0 bytes used to
                    // panic our allocator (`NonZeroU32::new(0).unwrap()`
                    // in `Chunk::new`), so honour the kernel's contract
                    // and silently ignore them here.
                    if load_me && vmsize == 0 {
                        log_dbg!(
                            "Skipping zero-vmsize segment {} at {:#x} (matches xnu mach_loader.c behaviour)",
                            segname,
                            vmaddr + slide
                        );
                        all_sections.extend_from_slice(&sections);
                        segment_offsets.push(vmaddr);
                        last_segment_end = last_segment_end.max(vmaddr + slide);
                        continue;
                    }

                    if load_me {
                        log_dbg!(
                            "reserve {} addr {:#x} size {}",
                            segname,
                            vmaddr + slide,
                            vmsize
                        );
                        into_mem.reserve(vmaddr + slide, vmsize);

                        // If filesize is less than vmsize, the rest of the
                        // segment should be filled with zeroes. We are assuming
                        // the memory is already zeroed!
                        if filesize > 0 {
                            assert!(filesize <= vmsize);
                            let fileoff: usize = fileoff
                                .try_into()
                                .map_err(|_| "Segment file offset does not fit host usize")?;
                            let filesize_usize = filesize as usize;
                            let file_end = fileoff
                                .checked_add(filesize_usize)
                                .ok_or("Segment file range overflows host usize")?;
                            let src = &bytes[fileoff..file_end];
                            let dst =
                                into_mem.bytes_at_mut(Ptr::from_bits(vmaddr + slide), filesize);
                            dst.copy_from_slice(src);
                        }
                    }

                    all_sections.extend_from_slice(&sections);
                    segment_offsets.push(vmaddr);
                    last_segment_end = last_segment_end.max(vmaddr + vmsize + slide);
                }
                LoadCommand::SymTab {
                    symoff,
                    nsyms,
                    stroff,
                    strsize,
                } => {
                    sym_tab_info = Some((symoff, nsyms, stroff, strsize));
                    if cursor.seek(SeekFrom::Start(symoff.into())).is_ok() {
                        let mut cursor = cursor.clone();
                        let symbols = SymbolIter::new(
                            &mut cursor,
                            all_sections.clone(),
                            nsyms,
                            stroff,
                            strsize,
                            is_bigend,
                            is_64bit,
                        );
                        for symbol in symbols {
                            if let Symbol::Debug { .. } = symbol {
                                continue;
                            }
                            if let Symbol::Defined {
                                name: Some(name),
                                external,
                                entry,
                                desc,
                                ..
                            } = symbol
                            {
                                let entry: u32 = entry.try_into().unwrap();
                                let entry = if desc & N_ARM_THUMB_DEF != 0 {
                                    entry | GuestFunction::THUMB_BIT
                                } else {
                                    entry
                                };
                                record_defined_symbol(
                                    &mut exported_symbols,
                                    &mut external_symbols,
                                    name,
                                    slide + entry,
                                    external,
                                );
                            };
                        }
                    }
                }
                LoadCommand::DySymTab {
                    indirectsymoff,
                    nindirectsyms,
                    extreloff,
                    nextrel,
                    locreloff,
                    nlocrel,
                    ..
                } => {
                    let indirectsyms = {
                        let start = indirectsymoff as usize;
                        let len = nindirectsyms as usize * 4;
                        if len == 0 || start >= bytes.len() || start + len > bytes.len() {
                            log!(
                                "Warning: indirect symbol table out of bounds (offset {:#x}, {} entries, file size {:#x}); skipping.",
                                indirectsymoff,
                                nindirectsyms,
                                bytes.len()
                            );
                            &[][..]
                        } else {
                            &bytes[start..start + len]
                        }
                    };
                    for idx in indirectsyms.chunks(4) {
                        assert!(!is_bigend);
                        let idx = u32::from_le_bytes(idx.try_into().unwrap());

                        let mut cursor = cursor.clone();
                        let sym = get_sym_by_idx(
                            idx,
                            sym_tab_info.unwrap(),
                            is_bigend,
                            is_64bit,
                            &mut cursor,
                        );
                        indirect_undef_symbols.push(match sym {
                            // Если имя есть (Some), оно превратится в
                            // Some(String).
                            // Если имени нет (None), вернется None, и мы
                            // избежим паники.
                            Some(Symbol::Undefined { name, .. }) => name.map(String::from),
                            Some(Symbol::Prebound { name, .. }) => name.map(String::from),
                            Some(Symbol::Defined { name, .. }) => name.map(String::from),
                            // Debug-символы по-прежнему игнорируем
                            Some(Symbol::Debug { .. }) => None,
                            None => None,
                            other => {
                                log!(
                                    "Warning: indirect symbol table contains an unexpected symbol kind {:?}; treating as anonymous.",
                                    other
                                );
                                None
                            }
                        })
                    }

                    // Classic dyld images record internal pointers in the
                    // local relocation table instead of LC_DYLD_INFO.
                    // The relocation offset is relative to the first segment
                    // (first writable segment for split-segment images).
                    // https://github.com/apple-oss-distributions/dyld/blob/dyld-360.22/src/ImageLoaderMachOClassic.cpp
                    if nlocrel != 0 {
                        let start = locreloff as usize;
                        let end = start
                            .checked_add(nlocrel as usize * 8)
                            .ok_or("Local relocation table range overflows")?;
                        let local_relocations = bytes
                            .get(start..end)
                            .ok_or("Local relocation table extends past end of file")?;
                        let relocation_base = if split_segs {
                            first_read_write_segment_base
                        } else {
                            first_segment_base
                        }
                        .ok_or("Local relocations have no segment base")?;
                        for entry in local_relocations.chunks_exact(8) {
                            let relocation = Reloc::parse(is_bigend, entry.try_into().unwrap());
                            let offset = match relocation {
                                Reloc::Local { section_idx: 0, .. } => continue,
                                Reloc::Local {
                                    addr,
                                    is_pc_relative: false,
                                    size: 4,
                                    type_: 0,
                                    ..
                                } => addr,
                                Reloc::Scattered {
                                    offset,
                                    is_pc_relative: false,
                                    size: 4,
                                    type_: 0,
                                    ..
                                } => offset,
                                // Prebound lazy pointers are reset using their
                                // recorded value rather than the existing word.
                                Reloc::Scattered {
                                    offset,
                                    value,
                                    is_pc_relative: false,
                                    size: 4,
                                    type_: 4,
                                } => {
                                    let address = relocation_base
                                        .checked_add(offset)
                                        .ok_or("Local relocation address overflows")?;
                                    into_mem
                                        .write(Ptr::from_bits(address), value.wrapping_add(slide));
                                    continue;
                                }
                                _ => return Err("Unsupported classic local relocation"),
                            };
                            let address = relocation_base
                                .checked_add(offset)
                                .ok_or("Local relocation address overflows")?;
                            let pointer = Ptr::from_bits(address);
                            let original: u32 = into_mem.read(pointer);
                            into_mem.write(pointer, original.wrapping_add(slide));
                        }
                    }

                    let extrels = {
                        let start = extreloff as usize;
                        let len = nextrel as usize * 8;
                        if len == 0 {
                            // A zero-length external relocation table is normal
                            // (no externally-relocated symbols) — don't warn.
                            &[][..]
                        } else if start >= bytes.len() || start + len > bytes.len() {
                            log!(
                                "Warning: external relocation table out of bounds (offset {:#x}, {} entries, file size {:#x}); skipping.",
                                extreloff,
                                nextrel,
                                bytes.len()
                            );
                            &[][..]
                        } else {
                            &bytes[start..start + len]
                        }
                    };
                    for entry in extrels.chunks(8) {
                        let entry_arr: [u8; 8] = match entry.try_into() {
                            Ok(a) => a,
                            Err(_) => {
                                log!(
                                    "Warning: external relocation table entry is not 8 bytes; truncated dynamic symbol table."
                                );
                                break;
                            }
                        };
                        let reloc = Reloc::parse(is_bigend, entry_arr);
                        let Reloc::External {
                            addr,
                            sym_idx,
                            is_pc_relative: false,
                            size: 4,
                            type_: 0, // generic
                        } = reloc
                        else {
                            log!(
                                "Warning: skipping unsupported external relocation entry: {:?}",
                                reloc
                            );
                            continue;
                        };
                        let addr = if split_segs {
                            addr + first_read_write_segment_base.unwrap()
                        } else {
                            addr + first_segment_base.unwrap()
                        };

                        let mut cursor = cursor.clone();
                        let sym = get_sym_by_idx(
                            sym_idx,
                            sym_tab_info.unwrap(),
                            is_bigend,
                            is_64bit,
                            &mut cursor,
                        );
                        match sym {
                            Some(Symbol::Undefined { name: Some(n), .. }) => {
                                external_relocations.push((addr, String::from(n)));
                            }
                            Some(Symbol::Defined { entry, desc, .. }) => {
                                // Apparently these are used for internal
                                // (intra-binary) relocations, despite being
                                // in the external section?
                                //
                                // Resolve them immediately, there is no value
                                // in passing these on to Dyld.
                                let addr = Ptr::from_bits(addr);
                                let addend: u32 = into_mem.read(addr);
                                let entry = entry as u32;
                                let entry = if desc & N_ARM_THUMB_DEF != 0 {
                                    entry | GuestFunction::THUMB_BIT
                                } else {
                                    entry
                                };
                                into_mem
                                    .write(addr, entry.wrapping_add(slide).wrapping_add(addend));
                            }
                            Some(Symbol::Prebound { name: Some(n), .. }) => {
                                let ptr_ptr = Ptr::<u32, true>::from_bits(addr);
                                into_mem.write(ptr_ptr, 0); // Clear prebinding.
                                external_relocations.push((addr, String::from(n)));
                            }
                            other => {
                                log!(
                                    "Warning: external relocation references an unexpected symbol kind {:?}; skipping fixup at {:#x}.",
                                    other,
                                    addr
                                );
                            }
                        };
                    }
                }
                LoadCommand::EncryptionInfo { id, .. } => {
                    if id != 0 {
                        return Err(
                            "The executable is encrypted. touchHLE can't run encrypted apps!",
                        );
                    }
                }
                LoadCommand::LoadDyLib(DyLib { name, .. })
                | LoadCommand::LoadWeakDyLib(DyLib { name, .. })
                | LoadCommand::ReexportDyLib(DyLib { name, .. })
                | LoadCommand::LoadUpwardDylib(DyLib { name, .. })
                | LoadCommand::LazyLoadDylib(DyLib { name, .. }) => {
                    dynamic_libraries.push(String::from(&*name));
                }
                LoadCommand::Rpath(path) => run_paths.push(path),
                // Old-style entry point PC command
                LoadCommand::UnixThread { state, .. } => {
                    let ThreadState::Arm {
                        __r: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                        __sp: 0,
                        __lr: 0,
                        __pc: pc,
                        __cpsr: 0,
                    } = state
                    else {
                        log!(
                            "Warning: unexpected initial thread state in {:?}: {:?}; falling back to extracted PC if present.",
                            name,
                            state
                        );
                        // Try to read the PC anyway; otherwise leave entry
                        // unset which will surface as a clear error later.
                        if let ThreadState::Arm { __pc, .. } = state {
                            if entry_point_pc.is_none() {
                                entry_point_pc = Some(__pc);
                            }
                        }
                        continue;
                    };
                    if entry_point_pc.is_some() {
                        log!(
                            "Warning: multiple initial thread states in {:?}; ignoring extra entries.",
                            name
                        );
                        continue;
                    }
                    entry_point_pc = Some(pc);
                }
                // New-style entry point PC command
                LoadCommand::EntryPoint {
                    entryoff,
                    stacksize,
                } => {
                    if stacksize != 0 {
                        log!("TODO: stack size of {:#x} bytes requested", stacksize);
                    }
                    // There should only be a single entry point.
                    // (Presumably an executable won't use both commands?)
                    assert!(entry_point_pc.is_none());
                    let entryoff: u32 = entryoff.try_into().unwrap();
                    entry_point_pc = Some(text_segment_base.unwrap() + entryoff);
                    entry_point_is_lc_main = true;
                }
                // Compressed dyld fixups used in iOS 3.1+ apps.
                LoadCommand::DyldInfo {
                    rebase_off,
                    rebase_size,
                    bind_off,
                    bind_size,
                    lazy_bind_off,
                    lazy_bind_size,
                    export_off,
                    export_size,
                    ..
                } => {
                    fn checked_dyld_info_slice<'a>(
                        bytes: &'a [u8],
                        off: u64,
                        size: u64,
                        kind: &str,
                        name: &str,
                    ) -> &'a [u8] {
                        // Convert to host usize with bounds checking — a truncated
                        // Mach-O (e.g. a corrupt IPA) can declare a dyld_info range
                        // that runs past EOF, and the raw slice panics with
                        // "range end index N out of range for slice of length M".
                        // Apple's dyld bails out on the binary; we degrade
                        // gracefully and load the rest.
                        let Ok(off_usize) = usize::try_from(off) else {
                            log!(
                                "Warning: {} dyld_info range starts at {:#x} past host usize \
                                 in {:?}; skipping.",
                                kind,
                                off,
                                name
                            );
                            return &[];
                        };
                        let Ok(size_usize) = usize::try_from(size) else {
                            log!(
                                "Warning: {} dyld_info range size {:#x} past host usize \
                                 in {:?}; skipping.",
                                kind,
                                size,
                                name
                            );
                            return &[];
                        };
                        let Some(end) = off_usize.checked_add(size_usize) else {
                            log!(
                                "Warning: {} dyld_info range {:#x}..{:#x}+{:#x} overflows \
                                 in {:?}; skipping.",
                                kind,
                                off_usize,
                                off_usize,
                                size_usize,
                                name
                            );
                            return &[];
                        };
                        if end > bytes.len() {
                            log!(
                                "Warning: {} dyld_info range {:#x}..{:#x} past end of \
                                 Mach-O file ({:#x}) in {:?}; skipping.",
                                kind,
                                off_usize,
                                end,
                                bytes.len(),
                                name
                            );
                            return &[];
                        }
                        &bytes[off_usize..end]
                    }

                    let rebase_opcodes = Rebase::parse(
                        checked_dyld_info_slice(
                            bytes,
                            rebase_off as u64,
                            rebase_size as u64,
                            "rebase",
                            &name,
                        ),
                        size_of::<GuestUSize>(),
                    );

                    for symb in rebase_opcodes {
                        match symb.symbol_type {
                            BindSymbolType::Pointer => {
                                let addr = segment_offsets[symb.segment_index]
                                    + symb.symbol_offset as u32
                                    + slide;
                                let original_location = Ptr::from_bits(addr);
                                let old: u32 = into_mem.read(original_location);
                                log_dbg!(
                                    "Pointer rebase at {:#x} from {:#x} to {:#x}",
                                    addr,
                                    old,
                                    old + slide
                                );
                                into_mem.write(original_location, old + slide);
                            }
                            other => {
                                log!(
                                    "Warning: unhandled DyldInfo rebase symbol type: {:?}; skipping.",
                                    other
                                );
                            }
                        }
                    }

                    let bind_opcodes = Bind::parse(
                        checked_dyld_info_slice(
                            bytes,
                            bind_off as u64,
                            bind_size as u64,
                            "bind",
                            &name,
                        ),
                        size_of::<GuestUSize>(),
                    );
                    for symb in bind_opcodes {
                        match symb.symbol_type {
                            BindSymbolType::Pointer => {
                                let addr = segment_offsets[symb.segment_index]
                                    + symb.symbol_offset as u32
                                    + slide;
                                log_dbg!("Pointer bind: {:#x} -> {}", addr, symb.name);
                                external_relocations.push((addr, symb.name));
                            }
                            other => {
                                log!(
                                    "Warning: unhandled DyldInfo bind symbol type: {:?}; skipping.",
                                    other
                                );
                            }
                        }
                    }
                    record_lazy_bindings(
                        checked_dyld_info_slice(
                            bytes,
                            lazy_bind_off as u64,
                            lazy_bind_size as u64,
                            "lazy bind",
                            &name,
                        ),
                        &segment_offsets,
                        slide,
                        into_mem,
                        &mut external_relocations,
                    )?;
                    record_export_trie(
                        bytes,
                        export_off,
                        export_size,
                        text_segment_base.ok_or("Export trie has no Mach-O header")?,
                        &mut exported_symbols,
                        &mut external_symbols,
                    )?;
                }
                LoadCommand::DyldExportsTrie(data) => {
                    record_export_trie(
                        bytes,
                        data.off,
                        data.size,
                        text_segment_base.ok_or("Export trie has no Mach-O header")?,
                        &mut exported_symbols,
                        &mut external_symbols,
                    )?;
                }
                _ => (),
            }
        }

        let sections = all_sections
            .iter()
            .map(|section| {
                let section = &**section;

                let name = section.sectname.clone();
                let addr: u32 = TryInto::<u32>::try_into(section.addr).unwrap() + slide;
                let size: u32 = section.size.try_into().unwrap();
                let type_ = section.flags.sect_type();

                log_dbg!(
                    "Section: {:?} {:#x} ({:#x} bytes), type {}",
                    name,
                    addr,
                    size,
                    type_,
                );

                use SectionType as ST;
                let (type_, dyld_entry_size) = match type_ {
                    S_MOD_INIT_FUNC_POINTERS => (ST::ModInitFuncPointers, None),
                    // Symbol stub sections have a variable size depending on
                    // whether PIC is in use.
                    S_SYMBOL_STUBS => (ST::SymbolStubs, Some(section.reserved2)),
                    // 4 bytes per pointer
                    S_LAZY_SYMBOL_POINTERS => (ST::LazySymbolPointers, Some(4)),
                    S_NON_LAZY_SYMBOL_POINTERS => (ST::NonLazySymbolPointers, Some(4)),
                    _ => (ST::Normal, None),
                };
                let dyld_indirect_symbol_info = dyld_entry_size.and_then(|entry_size| {
                    if entry_size == 0 || !size.is_multiple_of(entry_size) {
                        log!(
                            "Warning: section {:?} has invalid entry size {} for size {:#x}; skipping indirect symbol info.",
                            name,
                            entry_size,
                            size
                        );
                        return None;
                    }
                    let indirect_start = section.reserved1 as usize;
                    let indirect_count = (size / entry_size) as usize;
                    // Corrupt or trimmed binaries (e.g. Bug Heroes Quest) can
                    // declare an indirect symbol table that is past EOF; in
                    // that case the table above was skipped and is empty.
                    // Pad with anonymous entries instead of panicking: the
                    // dyld code skips `None` entries gracefully.
                    let indirects = match indirect_undef_symbols
                        .get(indirect_start..)
                        .and_then(|s| s.get(..indirect_count))
                    {
                        Some(s) => s.to_vec(),
                        None => {
                            log!(
                                "Warning: indirect symbol entries [{}, {}) out of range for table of length {}; treating as anonymous.",
                                indirect_start,
                                indirect_start + indirect_count,
                                indirect_undef_symbols.len()
                            );
                            vec![None; indirect_count]
                        }
                    };
                    Some(DyldIndirectSymbolInfo {
                        entry_size,
                        indirect_undef_symbols: indirects,
                    })
                });

                Section {
                    name,
                    segment_name: section.segname.clone(),
                    addr,
                    size,
                    type_,
                    dyld_indirect_symbol_info,
                }
            })
            .collect();

        Ok(MachO {
            loaded_path: name.clone(),
            name,
            dynamic_libraries,
            run_paths,
            slide,
            sections,
            exported_symbols,
            external_symbols,
            external_relocations,
            entry_point_pc,
            entry_point_is_lc_main,
            last_segment_end,
            text_base: text_segment_base.unwrap_or(first_segment_base.unwrap_or(0)),
        })
    }

    /// Load the all the sections from a Mach-O binary (from `path`) into the
    /// guest memory (`into_mem`), and return a struct containing metadata
    /// (e.g. symbols).
    pub fn load_from_file<P: AsRef<GuestPath>>(
        path: P,
        fs: &Fs,
        into_mem: &mut Mem,
        slide_to_address: u32,
    ) -> Result<MachO, &'static str> {
        let name = path.as_ref().file_name().unwrap().to_string();
        let mut image = Self::load_from_bytes(
            &fs.read(path.as_ref())
                .map_err(|_| "Could not read executable file")?,
            into_mem,
            name,
            slide_to_address,
        )?;
        image.loaded_path = path.as_ref().as_str().to_owned();
        Ok(image)
    }

    pub(crate) fn load_from_file_automatically_slid(
        path: &GuestPath,
        fs: &Fs,
        into_mem: &mut Mem,
    ) -> Result<MachO, &'static str> {
        let mut image = Self::load_with_placement(
            &fs.read(path)
                .map_err(|_| "Could not read executable file")?,
            into_mem,
            path.file_name().unwrap().to_owned(),
            None,
        )?;
        image.loaded_path = path.as_str().to_owned();
        Ok(image)
    }

    // dyld relocates images whose preferred segment ranges are occupied.
    // Keep the image's relative layout and use page-aligned free space.
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/dyld.3.html
    fn available_image_slide(commands: &[MachCommand], mem: &Mem) -> Result<u32, &'static str> {
        let mut first = u32::MAX;
        let mut last = 0;
        for MachCommand(command, _) in commands {
            if let LoadCommand::Segment {
                segname,
                vmaddr,
                vmsize,
                ..
            } = command
            {
                if matches!(&**segname, "__PAGEZERO" | "__LINKEDIT") || *vmsize == 0 {
                    continue;
                }
                let base = u32::try_from(*vmaddr).map_err(|_| "Image address exceeds 32 bits")?;
                let size = u32::try_from(*vmsize).map_err(|_| "Image size exceeds 32 bits")?;
                let end = base
                    .checked_add(size)
                    .ok_or("Image address range overflows")?;
                first = first.min(base);
                last = last.max(end);
            }
        }
        let size = last
            .checked_sub(first)
            .filter(|size| *size != 0)
            .ok_or("Image has no loadable segments")?;
        if first != 0 && mem.is_range_free(first, size) {
            return Ok(0);
        }
        let mut base = first
            .max(0x1000_0000)
            .checked_add(0xffff)
            .map(|value| value & !0xffff)
            .ok_or("Image address range overflows")?;
        while base.checked_add(size).is_some_and(|end| end < 0xf000_0000) {
            if mem.is_range_free(base, size) {
                return Ok(base - first);
            }
            base = base
                .checked_add(0x1_0000)
                .ok_or("Image address range overflows")?;
        }
        Err("No free address range for bundled image")
    }

    /// Get a section by its name (`&str`) or type ([SectionType]).
    pub fn get_section<P: SectionPredicate>(&self, by: P) -> Option<&Section> {
        self.sections.iter().find(|section| by.test(section))
    }
}

/// Bind compressed lazy pointers at load time, including pointers whose
/// indirect-symbol entries are LOCAL/ABS. DONE ends one record, not the table.
/// The initial pointer targets a stub helper; it is not a relocation addend.
/// https://github.com/apple-oss-distributions/dyld/blob/dyld-360.22/src/ImageLoaderMachOCompressed.cpp
fn record_lazy_bindings(
    payload: &[u8],
    segment_offsets: &[u32],
    slide: u32,
    mem: &mut Mem,
    relocations: &mut Vec<(u32, String)>,
) -> Result<(), &'static str> {
    let mut bytes = payload.iter();
    let mut name = None;
    let mut segment_index = 0usize;
    let mut offset = 0u32;
    let mut addend = 0i64;
    let mut symbol_type = 1;
    while let Some(&byte) = bytes.next() {
        let immediate = byte & 0x0f;
        match byte & 0xf0 {
            0x00 => {}        // BIND_OPCODE_DONE
            0x10 | 0x30 => {} // Library ordinal; existing linking uses flat lookup.
            0x20 => {
                read_unsigned_leb128(&mut bytes)?;
            }
            0x40 => {
                let mut symbol = Vec::new();
                loop {
                    match bytes.next().ok_or("Truncated lazy binding symbol")? {
                        0 => break,
                        byte => symbol.push(*byte),
                    }
                }
                name = Some(String::from_utf8(symbol).map_err(|_| "Invalid lazy binding symbol")?);
            }
            0x50 => symbol_type = immediate,
            0x60 => addend = read_signed_leb128(&mut bytes)?,
            0x70 => {
                segment_index = immediate as usize;
                offset = read_unsigned_leb128(&mut bytes)?
                    .try_into()
                    .map_err(|_| "Lazy binding offset exceeds ARM32")?;
            }
            0x80 => {
                let delta: u32 = read_unsigned_leb128(&mut bytes)?
                    .try_into()
                    .map_err(|_| "Lazy binding offset exceeds ARM32")?;
                offset = offset
                    .checked_add(delta)
                    .ok_or("Lazy binding offset overflow")?;
            }
            0x90 => {
                if symbol_type != 1 {
                    return Err("Unsupported lazy binding type");
                }
                let name = name.as_ref().ok_or("Lazy binding has no symbol name")?;
                let addr = segment_offsets
                    .get(segment_index)
                    .ok_or("Lazy binding has an invalid segment index")?
                    .checked_add(offset)
                    .and_then(|addr| addr.checked_add(slide))
                    .ok_or("Lazy binding address overflow")?;
                // do_non_lazy_linking applies the stored addend to the symbol.
                mem.write(Ptr::<u32, true>::from_bits(addr), addend as u32);
                relocations.push((addr, name.clone()));
                offset = offset
                    .checked_add(4)
                    .ok_or("Lazy binding offset overflow")?;
            }
            _ => return Err("Invalid lazy binding opcode"),
        }
    }
    Ok(())
}

fn read_unsigned_leb128<'a>(bytes: &mut impl Iterator<Item = &'a u8>) -> Result<u64, &'static str> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.next().ok_or("Truncated lazy binding operand")?;
        if shift == 63 && byte > 1 {
            return Err("Lazy binding operand overflow");
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("Lazy binding operand overflow")
}

fn read_signed_leb128<'a>(bytes: &mut impl Iterator<Item = &'a u8>) -> Result<i64, &'static str> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.next().ok_or("Truncated lazy binding addend")?;
        if shift == 63 && byte != 0 && byte != 0x7f {
            return Err("Lazy binding addend overflow");
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            if byte & 0x40 != 0 && shift < 57 {
                value |= u64::MAX << (shift + 7);
            }
            return Ok(value as i64);
        }
    }
    Err("Lazy binding addend overflow")
}

#[cfg(test)]
mod segment_validation_tests {
    use super::*;

    #[test]
    fn missing_file_bytes_are_not_bss() {
        // Ranges from the BH Quest crash report.
        assert!(validate_segment_file_range(0, 0x427000, 0x427000, 0x32fad0).is_err());
        assert!(validate_segment_file_range(0x427000, 0x23000, 0x23000, 0x32fad0).is_err());
        assert!(validate_segment_file_range(0x100, 0x100, 0x1000, 0x200).is_ok());
        assert!(validate_segment_file_range(0x100, 0x100, 0x1000, 0x1ff).is_err());
    }

    #[test]
    fn empty_segments_and_overflow() {
        assert!(validate_segment_file_range(u64::MAX, 0, 0x1000, 0).is_ok());
        assert!(validate_segment_file_range(0, 0, 0, 0).is_ok());
        assert!(validate_segment_file_range(u64::MAX, 1, 1, usize::MAX).is_err());
        assert!(validate_segment_file_range(0, 2, 1, 2).is_err());
    }

    // Minimal 32-bit ARM Mach-O with a single LC_SEGMENT and no sections.
    fn thin(subtype: u32, address: u32, file_size: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [0xfeedfaceu32, 12, subtype, 2, 1, 56, 0, 1, 56] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut name = [0u8; 16];
        name[..6].copy_from_slice(b"__TEXT");
        bytes.extend_from_slice(&name);
        for word in [address, 0x2000, 0, file_size, 7, 5, 0, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(bytes.len(), 84);
        bytes
    }

    #[test]
    fn bundled_images_relocate_without_overlapping_each_other() {
        let bytes = thin(9, 0, 84);
        let mut mem = Mem::new();
        mem.set_null_segment_size(0x1000);
        let first = MachO::load_with_placement(&bytes, &mut mem, "first".into(), None).unwrap();
        let second = MachO::load_with_placement(&bytes, &mut mem, "second".into(), None).unwrap();

        assert_ne!(first.slide, 0);
        assert_ne!(second.slide, 0);
        assert!(first.last_segment_end <= second.text_base);
        assert_eq!(first.slide & 0x3fff, 0);
        assert_eq!(second.slide & 0x3fff, 0);
        assert_eq!(
            mem.read::<u32, false>(Ptr::from_bits(first.text_base)),
            0xfeedface
        );
        assert_eq!(
            mem.read::<u32, false>(Ptr::from_bits(second.text_base)),
            0xfeedface
        );
    }

    #[test]
    fn loader_retains_run_path_order_and_weak_dependencies() {
        let mut bytes = thin(9, 0x9000, 84);
        let mut commands = Vec::new();
        for (command, prefix_words, name) in [
            (0x8000001cu32, vec![12u32], "@executable_path/Frameworks"),
            (0x8000001c, vec![12u32], "/usr/lib"),
            (0x80000018, vec![24u32, 0, 0, 0], "@rpath/Support.dylib"),
        ] {
            let size = (8 + prefix_words.len() * 4 + name.len() + 1 + 3) & !3;
            commands.extend_from_slice(&command.to_le_bytes());
            commands.extend_from_slice(&(size as u32).to_le_bytes());
            for word in prefix_words {
                commands.extend_from_slice(&word.to_le_bytes());
            }
            commands.extend_from_slice(name.as_bytes());
            commands.push(0);
            while commands.len() % 4 != 0 {
                commands.push(0);
            }
        }
        bytes[16..20].copy_from_slice(&4u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&(56u32 + commands.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&commands);
        let file_size = bytes.len() as u32;
        bytes[64..68].copy_from_slice(&file_size.to_le_bytes());
        let mut mem = Mem::new();
        let image = MachO::load_from_bytes(&bytes, &mut mem, "rpaths".into(), 0).unwrap();

        assert_eq!(image.run_paths, ["@executable_path/Frameworks", "/usr/lib"]);
        assert_eq!(image.dynamic_libraries, ["@rpath/Support.dylib"]);
    }

    #[test]
    fn stripped_export_trie_keeps_thumb_addresses_and_absolute_symbols() {
        for (flags, expected) in [(0, 0x29045), (2, 0x45)] {
            let mut bytes = thin(9, 0x9000, 84);
            let edge = b"_trieOnly\0";
            let mut trie = vec![0, 1];
            trie.extend_from_slice(edge);
            trie.push((2 + edge.len() + 1) as u8);
            trie.extend_from_slice(&[2, flags, 0x45, 0]);
            let export_offset = bytes.len() as u32 + 48;
            for word in [
                0x80000022,
                48,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                export_offset,
                trie.len() as u32,
            ] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            bytes.extend_from_slice(&trie);
            bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
            bytes[20..24].copy_from_slice(&104u32.to_le_bytes());
            let size = bytes.len() as u32;
            bytes[64..68].copy_from_slice(&size.to_le_bytes());
            let mut mem = Mem::new();
            let image =
                MachO::load_from_bytes(&bytes, &mut mem, "stripped".into(), 0x20000).unwrap();

            assert_eq!(image.external_symbols["_trieOnly"], expected);
            assert_eq!(image.exported_symbols["_trieOnly"], expected);
        }
    }

    #[test]
    fn compressed_lazy_bindings_continue_after_done_and_replace_stub_helpers() {
        let mut bytes = thin(9, 0x9000, 84);
        let mut binds = vec![0x70, 0x80, 0x02, 0x11, 0x40];
        binds.extend_from_slice(b"_first\0");
        binds.extend_from_slice(&[0x51, 0x90, 0x00, 0x70, 0x84, 0x02, 0x11, 0x40]);
        binds.extend_from_slice(b"_second\0");
        binds.extend_from_slice(&[0x60, 0x7c, 0x90, 0x00]); // Signed addend -4.
        for word in [
            0x80000022,
            48,
            0,
            0,
            0,
            0,
            0,
            0,
            132,
            binds.len() as u32,
            0,
            0,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&binds);
        bytes.resize(0x108, 0);
        bytes[0x100..0x104].copy_from_slice(&0x9200u32.to_le_bytes());
        bytes[0x104..0x108].copy_from_slice(&0x9210u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&104u32.to_le_bytes());
        bytes[64..68].copy_from_slice(&0x108u32.to_le_bytes());
        let mut mem = Mem::new();
        let image = MachO::load_from_bytes(&bytes, &mut mem, "lazy-binds".into(), 0x20000).unwrap();

        assert_eq!(
            image.external_relocations,
            [(0x29100, "_first".into()), (0x29104, "_second".into())]
        );
        assert_eq!(mem.read::<u32, false>(Ptr::from_bits(0x29100)), 0);
        assert_eq!(
            mem.read::<u32, false>(Ptr::from_bits(0x29104)),
            u32::MAX - 3
        );
    }

    #[test]
    fn thin_loader_rejects_truncation() {
        let bytes = thin(6, 0x1000, 128);
        let mut mem = Mem::new();
        let result = MachO::load_from_bytes(&bytes, &mut mem, "truncated".into(), 0);
        assert!(matches!(result, Err(e) if e.contains("extends past end of file")));
    }

    #[test]
    fn fat_loader_falls_back_from_internally_truncated_armv7() {
        let invalid = thin(9, 0x1000, 128); // ARMv7, preferred but incomplete
        let valid = thin(6, 0x9000, 84); // ARMv6, complete with a virtual zero tail
        let mut bytes = Vec::new();
        for word in [0xcafebabeu32, 2, 12, 9, 48, 84, 0, 12, 6, 132, 84, 0] {
            bytes.extend_from_slice(&word.to_be_bytes());
        }
        bytes.extend_from_slice(&invalid);
        bytes.extend_from_slice(&valid);
        let mut mem = Mem::new();
        let image = MachO::load_from_bytes(&bytes, &mut mem, "fat-test".into(), 0).unwrap();
        assert_eq!(image.text_base, 0x9000);
        assert_eq!(
            mem.bytes_at(Ptr::<u8, false>::from_bits(0x9000), 84),
            &valid[..]
        );
        assert!(mem
            .bytes_at(Ptr::<u8, false>::from_bits(0x9000 + 84), 16)
            .iter()
            .all(|&b| b == 0));
    }

    #[test]
    fn arm_subtype_allowlist_and_capabilities() {
        for subtype in [0, 6, 9, 10, 11] {
            assert!(arm_subtype_priority(subtype).is_some());
        }
        for subtype in [
            5u32,
            7,
            8,
            12,
            13,
            14,
            0x1234,
            0x8000_0009,
            0x0100_0006,
            u32::MAX,
        ] {
            assert!(arm_subtype_priority(subtype as cpu_subtype_t).is_none());
        }
        let priorities: Vec<_> = [0, 6, 10, 11, 9]
            .into_iter()
            .map(|s| arm_subtype_priority(s).unwrap())
            .collect();
        assert!(priorities.windows(2).all(|p| p[0] < p[1]));
    }

    #[test]
    fn thin_loader_rejects_unsupported_cpu_subtype() {
        for subtype in [12, 13, 0x1234, 0x8000_0009] {
            let bytes = thin(subtype, 0x1000, 84);
            let mut mem = Mem::new();
            let result = MachO::load_from_bytes(&bytes, &mut mem, "unsupported".into(), 0);
            assert!(matches!(result, Err(e) if e.contains("Unsupported ARM CPU subtype")));
        }
    }

    fn fat_pair(first_type: u32, first: &[u8], second_type: u32, second: &[u8]) -> Vec<u8> {
        assert_eq!((first.len(), second.len()), (84, 84));
        let mut bytes = Vec::new();
        for word in [
            0xcafebabeu32,
            2,
            12,
            first_type,
            48,
            84,
            0,
            12,
            second_type,
            132,
            84,
            0,
        ] {
            bytes.extend_from_slice(&word.to_be_bytes());
        }
        bytes.extend_from_slice(first);
        bytes.extend_from_slice(second);
        bytes
    }

    #[test]
    fn fat_loader_skips_unsupported_and_mismatched_subtypes() {
        for (table_subtype, header_subtype) in [(12, 12), (9, 6), (9, 13)] {
            let bytes = fat_pair(
                table_subtype,
                &thin(header_subtype, 0x1000, 84),
                6,
                &thin(6, 0x9000, 84),
            );
            let mut mem = Mem::new();
            let image = MachO::load_from_bytes(&bytes, &mut mem, "subtypes".into(), 0).unwrap();
            assert_eq!(image.text_base, 0x9000);
        }
    }

    #[test]
    fn fat_loader_prefers_armv7_regardless_of_slice_order() {
        for reverse in [false, true] {
            let v6 = thin(6, 0x1000, 84);
            let v7 = thin(9, 0x9000, 84);
            let bytes = if reverse {
                fat_pair(9, &v7, 6, &v6)
            } else {
                fat_pair(6, &v6, 9, &v7)
            };
            let mut mem = Mem::new();
            let image = MachO::load_from_bytes(&bytes, &mut mem, "preference".into(), 0).unwrap();
            assert_eq!(image.text_base, 0x9000);
        }
    }

    #[test]
    fn fat_loader_rejects_images_with_no_supported_subtype() {
        let bytes = fat_pair(12, &thin(12, 0x1000, 84), 13, &thin(13, 0x9000, 84));
        let mut mem = Mem::new();
        assert!(MachO::load_from_bytes(&bytes, &mut mem, "unsupported-fat".into(), 0).is_err());
    }

    #[test]
    fn thin_loader_accepts_supported_cpu_subtypes() {
        for subtype in [0, 6, 9, 10, 11] {
            let bytes = thin(subtype, 0x9000, 84);
            let mut mem = Mem::new();
            let image = MachO::load_from_bytes(&bytes, &mut mem, "supported".into(), 0).unwrap();
            assert_eq!(image.text_base, 0x9000);
        }
    }
}

#[cfg(test)]
mod symbol_visibility_tests {
    use super::*;

    #[test]
    fn local_symbols_are_not_visible_to_dependent_dylibs() {
        let mut defined_symbols = HashMap::new();
        let mut external_symbols = HashMap::new();

        record_defined_symbol(
            &mut defined_symbols,
            &mut external_symbols,
            "_log2",
            0x0048_85c0,
            false,
        );
        record_defined_symbol(
            &mut defined_symbols,
            &mut external_symbols,
            "_memcpy",
            0x0030_1001,
            true,
        );

        assert_eq!(defined_symbols.get("_log2"), Some(&0x0048_85c0));
        assert!(!external_symbols.contains_key("_log2"));
        assert_eq!(external_symbols.get("_memcpy"), Some(&0x0030_1001));
    }
}
