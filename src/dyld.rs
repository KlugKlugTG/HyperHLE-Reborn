/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Dynamic linker.
//!
//! iPhone OS's dynamic linker, `dyld`, is the namesake of this module.
//!
//! This is where the magic of "high-level emulation" can begin to happen.
//! The guest app will reference various functions, constants, classes etc from
//! iPhone OS's system frameworks and other dynamically-linked libraries, but
//! instead of actually loading and linking the original framework binaries,
//! this "dynamic linker" will generate appropriate stubs for calling into
//! touchHLE's own implementations of the frameworks, which are "host code"
//! (i.e. not themselves running under emulation).
//!
//! This also does normal dynamic linking for libgcc, libstdc++, etc.
//!
//! See [crate::mach_o] for resources.

mod dylib_list;
pub(crate) mod dylib_paths;
mod swift_runtime;

use crate::abi::{CallFromGuest, GuestFunction};
use crate::bundle;
use crate::cpu::Cpu;
use crate::frameworks::foundation::ns_string;
use crate::mach_o::{MachO, SectionType};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{nil, ClassExports, ObjC};
use crate::Environment;
use std::collections::HashMap;

pub use dylib_list::DYLIB_LIST;

/// Struct used to expose a host implementation of a dynamic library (usually a
/// framework) to the linker.
///
/// Each module that wants to expose a library to guest code should export a
/// constant using this type, which collects all the relevant [ClassExports],
/// [ConstantExports] and [FunctionExports] for the library. For example:
///
/// ```ignore
/// pub const DYLIB: HostDylib = HostDylib {
///     path: "/System/Library/Frameworks/FooBarKit.framework/FooBarKit",
///     aliases: &[],
///     class_exports: &[baz::CLASSES],
///     constant_exports: &[qux::CONSTANTS],
///     function_exports: &[qux::FUNCTIONS, baz::FUNCTIONS],
/// };
/// ```
///
/// The `path` should be the canonical notional filesystem path that the library
/// is referenced by on the real OS, for example `"/usr/lib/libobjc.A.dylib"`
/// or `"/System/Library/Frameworks/Foundation.framework/Foundation"`. For
/// libraries that have several symlinked paths, non-canonical alternate
/// paths can be listed under `aliases`, for example `"/usr/lib/libobjc.dylib"`.
pub struct HostDylib {
    pub path: &'static str,
    pub aliases: &'static [&'static str],
    pub class_exports: &'static [ClassExports],
    pub constant_exports: &'static [ConstantExports],
    pub function_exports: &'static [FunctionExports],
}

/// Debug: number of upcoming host calls to log.
pub static TRACE_HOST_CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub type HostFunction = &'static dyn CallFromGuest;

/// Type for lists of functions exported by host implementations of dynamic
/// libraries (usually frameworks).
///
/// Each module that wants to expose functions to guest code should export a
/// constant using this type, e.g.:
///
/// ```ignore
/// pub const FUNCTIONS: FunctionExports = &[
///    ("_NSFoo", &/* ... */),
///    ("_NSBar", &/* ... */),
///    /* ... */
/// ];
/// ```
///
/// All the constants like this can then be collected into a [HostDylib].
///
/// The strings are the mangled symbol names. For C functions, this is just the
/// name prefixed with an underscore.
///
/// For convenience, use [export_c_func]:
///
/// ```ignore
/// pub const FUNCTIONS: FunctionExports = &[
///     export_c_func!(NSFoo(_, _)),
///     export_c_func!(NSBar()),
/// ];
/// ```
///
/// See also [ConstantExports] and [ClassExports].
pub type FunctionExports = &'static [(&'static str, HostFunction)];

/// Macro for exporting a function with C-style name mangling. See
/// [FunctionExports].
///
/// ```ignore
/// export_c_func!(NSFoo(_, _))
/// ```
///
/// will desugar to:
///
/// ```ignore
/// ("_NSFoo", &(NSFoo as (&mut Environment, _, _) -> _))
/// ```
///
/// The function needs to be explicitly casted because a bare function reference
/// defaults to a different type than a pure fn pointer, which is the type that
/// [CallFromGuest] is implemented on. This macro will do the casting for you,
/// but you will need to supply an underscore for each parameter.
#[macro_export]
macro_rules! export_c_func {
    ($name:ident ($($_:ty),*)) => {
        (
            concat!("_", stringify!($name)),
            &($name as fn(&mut $crate::Environment, $($_),*) -> _)
        )
    };
}
pub use crate::export_c_func; // #[macro_export] is weird...

/// Other variant of [export_c_func] macro, allowing to define an alias
/// for the exporting function. This is useful then alias may contain
/// characters not normally allowed for Rust function's names. (e.g. `$`)
#[macro_export]
macro_rules! export_c_func_aliased {
    ($alias:literal, $name:ident ($($_:ty),*)) => {
        (
            concat!("_", $alias),
            &($name as fn(&mut $crate::Environment, $($_),*) -> _)
        )
    };
}
pub use crate::export_c_func_aliased; // #[macro_export] is weird...

/// Type for describing a constant (C `extern const` symbol) that will be
/// created by the linker if the guest app references it. See [ConstantExports].
pub enum HostConstant {
    NSString(&'static str),
    NullPtr,
    Custom(fn(&mut Environment) -> ConstVoidPtr),
}

/// Type for lists of constants exported by host implementations of  dynamic
/// libraries (usually frameworks).
///
/// Each module that wants to expose functions to guest code should export a
/// constant using this type, e.g.:
///
/// ```ignore
/// pub const CONSTANT: ConstantExports = &[
///    ("_kNSFooBar", HostConstant::NSString("NSFooBar")),
///    /* ... */
/// ];
/// ```
///
/// All the constants like this can then be collected into a [HostDylib].
///
/// The strings are the mangled symbol names. For C constants, this is just the
/// name prefixed with an underscore.
///
/// See also [FunctionExports], [ClassExports].
pub type ConstantExports = &'static [(&'static str, HostConstant)];

/// Search the list of [HostDylib]s for a class/constant/function by its symbol.
///
/// Example usage: `search_host_dylibs(|dylib| dylib.function_exports, "_foo")`
pub fn search_host_dylibs<T, F>(get_exports: F, symbol: &str) -> Option<&'static (&'static str, T)>
where
    F: Fn(&HostDylib) -> &'static [&'static [(&'static str, T)]],
{
    // TODO: In general, we should rarely if ever need to search the full set
    //       of dylibs for a symbol. Now that we know which symbols belong to
    //       which libraries, we should at least only search libraries that are
    //       referenced by the app and currently "loaded". We probably should
    //       also implement the Mach-O two-level symbol namespacing eventually.
    DYLIB_LIST
        .iter()
        .copied()
        .map(get_exports)
        .find_map(|lists| search_lists(lists, symbol))
}

/// Helper for working with [ClassExports]/[ConstantExports]/[FunctionExports].
fn search_lists<T>(
    lists: &'static [&'static [(&'static str, T)]],
    symbol: &str,
) -> Option<&'static (&'static str, T)> {
    lists
        .iter()
        .flat_map(|&n| n)
        .find(|&(sym, _)| *sym == symbol)
}

fn encode_a32_svc(imm: u32) -> u32 {
    assert!(imm & 0xff000000 == 0);
    imm | 0xef000000
}
fn encode_a32_ret() -> u32 {
    0xe12fff1e
}
fn encode_a32_trap() -> u32 {
    0xe7ffdefe
}

fn alloc_a32_ret_stub(mem: &mut Mem) -> MutPtr<u32> {
    let fn_ptr: MutPtr<u32> = mem.alloc(8).cast();
    mem.write(fn_ptr + 0, encode_a32_ret());
    mem.write(fn_ptr + 1, encode_a32_trap());
    fn_ptr
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CxxAbiTypeInfoKind {
    Class,
    SingleInheritance,
    MultipleInheritance,
}

fn cxxabi_typeinfo_vtable_kind(name: &str) -> Option<CxxAbiTypeInfoKind> {
    match name {
        "__ZTVN10__cxxabiv117__class_type_infoE" => Some(CxxAbiTypeInfoKind::Class),
        "__ZTVN10__cxxabiv120__si_class_type_infoE" => Some(CxxAbiTypeInfoKind::SingleInheritance),
        "__ZTVN10__cxxabiv121__vmi_class_type_infoE" => {
            Some(CxxAbiTypeInfoKind::MultipleInheritance)
        }
        _ => None,
    }
}

fn register_cxxabi_typeinfo_vtable(
    name: &str,
    addr: u32,
    typeinfo_vtable_kinds: &mut HashMap<u32, CxxAbiTypeInfoKind>,
) {
    let Some(kind) = cxxabi_typeinfo_vtable_kind(name) else {
        return;
    };
    let inserted_base = typeinfo_vtable_kinds.insert(addr, kind).is_none();
    let inserted_address_point = if let Some(address_point) = addr.checked_add(8) {
        typeinfo_vtable_kinds.insert(address_point, kind).is_none()
    } else {
        false
    };
    if (inserted_base || inserted_address_point)
        && std::env::var_os("TOUCHHLE_TRACE_DYNAMIC_CAST").is_some()
    {
        log!(
            "__dynamic_cast RTTI vtable registered: name={name} base={addr:#010x} address_point={:#010x} kind={kind:?}",
            addr.saturating_add(8)
        );
    }
}

fn link_cxxabi_vtable(
    name: &str,
    cxxabi_vtable_addrs: &mut HashMap<String, u32>,
    typeinfo_vtable_kinds: &mut HashMap<u32, CxxAbiTypeInfoKind>,
    mem: &mut Mem,
) -> ConstVoidPtr {
    let addr = *cxxabi_vtable_addrs
        .entry(name.to_string())
        .or_insert_with(|| {
            let stub = alloc_a32_ret_stub(mem);
            let stub_addr = stub.to_bits();
            let v: MutPtr<u32> = mem.alloc(40).cast();
            mem.write(v + 0, 0);
            mem.write(v + 1, 0);
            for i in 2..10 {
                mem.write(v + i, stub_addr);
            }
            v.to_bits()
        });
    register_cxxabi_typeinfo_vtable(name, addr, typeinfo_vtable_kinds);
    Ptr::from_bits(addr).cast_const()
}

fn link_cxxabi_typeinfo(
    name: &str,
    cxxabi_vtable_addrs: &mut HashMap<String, u32>,
    typeinfo_vtable_kinds: &mut HashMap<u32, CxxAbiTypeInfoKind>,
    mem: &mut Mem,
) -> ConstVoidPtr {
    let name_bytes = name.strip_prefix('_').unwrap_or(name);
    let name_cstr = mem.alloc_and_write_cstr(name_bytes.as_bytes());
    let ti: MutPtr<u32> = mem.alloc(8).cast();
    let vtable = link_cxxabi_vtable(
        "__ZTVN10__cxxabiv117__class_type_infoE",
        cxxabi_vtable_addrs,
        typeinfo_vtable_kinds,
        mem,
    );
    mem.write(ti + 0, vtable.to_bits() + 8);
    mem.write(ti + 1, name_cstr.to_bits());
    ti.cast().cast_const()
}

/// Make `std::string((const char*)NULL)` construct an empty string instead of
/// looping forever / corrupting memory.
///
/// See the call site in [Dyld::do_initial_linking] for the full rationale. In
/// short: the char specialisation of libstdc++'s
/// `basic_string::_S_construct<const char*>(beg, end, alloc,
/// forward_iterator_tag)` handles a NULL `beg` by branching to a block that
/// calls `std::__throw_logic_error`. Because touchHLE neuters that helper to a
/// bare `BX LR`, control falls through into the normal copy path with
/// `beg == NULL` and `end - beg == npos`, producing a `memcpy(dest, NULL,
/// npos)` and a string whose length is `npos`.
///
/// The function already contains the correct behaviour we want for this case:
/// an "empty string" block (taken when `beg == end`) that returns
/// `_S_empty_rep()._M_refdata()`. We simply retarget the NULL-pointer branch so
/// it jumps there instead.
///
/// The relevant Thumb-2 code (from the bundled `libstdc++.6.0.9.dylib`) is:
/// ```text
///   <entry>:   cmp r0, r1        ; beg == end ?
///              ...
///              bne  <dispatch>    ; fall through == "empty string" block
///   <empty>:   ldr r0, [pc, ...]  ; r0 = &_S_empty_rep_storage   <-- retarget here
///              ...
///   <dispatch>:cmp r0, #0         ; beg == NULL ?
///              bne <normal-copy>
///              b   <throw>        ; beg == NULL                  <-- patched
/// ```
/// We locate the `cmp r0,#0` / `bne` / `b` triple and rewrite the final
/// unconditional `b` so it targets `<empty>` (the halfword right after the
/// first `bne`). The patch is verified against the expected instruction
/// encodings and skipped (with a warning) if the library doesn't match, so an
/// unexpected libstdc++ build can never be silently mis-patched.
fn patch_string_s_construct_null(dylib: &MachO, mem: &mut Mem) {
    const SYM: &str = "__ZNSs12_S_constructIPKcEEPcT_S3_RKSaIcESt20forward_iterator_tag";
    let Some(&entry_with_thumb_bit) = dylib.exported_symbols.get(SYM) else {
        return;
    };
    // This patch only understands the Thumb encoding shipped with touchHLE.
    if entry_with_thumb_bit & 1 == 0 {
        return;
    }
    let entry = entry_with_thumb_bit & !1;

    let read_hw = |mem: &Mem, addr: u32| -> u16 { mem.read(Ptr::<u16, false>::from_bits(addr)) };

    // Scan a bounded window of the function body.
    const SCAN_HALFWORDS: u32 = 128;
    // Address (halfword) of the empty-string block: the instruction right after
    // the first `bne` that follows the initial `cmp r0, r1` (0x4288).
    let mut empty_block: Option<u32> = None;
    let mut seen_cmp_r0_r1 = false;
    for i in 0..SCAN_HALFWORDS {
        let addr = entry + i * 2;
        let hw = read_hw(mem, addr);
        if !seen_cmp_r0_r1 {
            if hw == 0x4288 {
                seen_cmp_r0_r1 = true;
            }
            continue;
        }
        // `bne` is a T1 conditional branch: 0xD1xx.
        if hw & 0xff00 == 0xd100 {
            empty_block = Some(addr + 2);
            break;
        }
    }
    let Some(empty_block) = empty_block else {
        log!(
            "Warning: could not locate std::string _S_construct empty-string \
             block in {}; skipping NULL-construction patch.",
            dylib.name
        );
        return;
    };

    // Find the `cmp r0,#0` (0x2800), `bne` (0xD1xx), `b` (0xE7xx) triple that
    // dispatches the `beg != end` case.
    for i in 0..SCAN_HALFWORDS {
        let addr = entry + i * 2;
        if read_hw(mem, addr) != 0x2800 {
            continue;
        }
        let bne = read_hw(mem, addr + 2);
        let b = read_hw(mem, addr + 4);
        if bne & 0xff00 != 0xd100 || b & 0xf800 != 0xe000 {
            continue;
        }
        // Re-encode the unconditional branch at `addr + 4` to target
        // `empty_block`. T1 B: imm11 = (target - (pc + 4)) / 2, pc == addr + 4.
        let b_addr = addr + 4;
        let delta = (empty_block as i32) - (b_addr as i32 + 4);
        let imm11 = (delta >> 1) & 0x7ff;
        let new_b = 0xe000u16 | imm11 as u16;
        mem.write(Ptr::<u16, true>::from_bits(b_addr), new_b);
        log!(
            "Patched libstdc++ std::string _S_construct in {} so \
             std::string((char*)NULL) yields an empty string instead of \
             looping/corrupting (b {:#06x} -> {:#06x}).",
            dylib.name,
            b,
            new_b
        );
        return;
    }

    log!(
        "Warning: could not locate std::string _S_construct NULL branch in {}; \
         skipping NULL-construction patch.",
        dylib.name
    );
}

fn write_return_to_host_routine(mem: &mut Mem, svc: u32) -> GuestFunction {
    let routine = [
        encode_a32_svc(svc),
        // When a return-to-host occurs, it's the host's responsibility
        // to reset the PC to somewhere else. So something has gone
        // wrong if this is executed.
        encode_a32_trap(),
    ];
    let ptr: MutPtr<u32> = mem.alloc(4 * 2).cast();
    mem.write(ptr + 0, routine[0]);
    mem.write(ptr + 1, routine[1]);
    let ptr = GuestFunction::from_addr_with_thumb_bit(ptr.to_bits());
    assert!(!ptr.is_thumb());
    ptr
}

pub struct Dyld {
    /// List of host functions that have been "linked" and had SVCs assigned.
    ///
    /// The `&'static str` part here is purely for debugging and could be
    /// removed in release builds if it's ever necessary.
    linked_host_functions: Vec<(&'static str, HostFunction)>,
    return_to_host_routine: Option<GuestFunction>,
    thread_exit_routine: Option<GuestFunction>,
    constants_to_link_later: Vec<(MutPtr<ConstVoidPtr>, &'static str)>,
    host_constant_addresses: HashMap<&'static str, ConstVoidPtr>,
    non_lazy_host_functions: HashMap<&'static str, GuestFunction>,
    /// Cache of Swift runtime function trampolines (see `swift_runtime`).
    swift_fn_cache: HashMap<String, GuestFunction>,
    /// Interned `&'static str` copies of Swift symbol names.
    swift_fn_names: HashMap<String, &'static str>,
    /// Stable data slots for Swift metadata symbols.
    swift_data_slots: HashMap<String, u32>,
    cxxabi_typeinfo_vtable_kinds: HashMap<u32, CxxAbiTypeInfoKind>,
    guest_sjlj_runtime_available: bool,
    /// Guest (main executable or bundled dylib) implementations of the
    /// Itanium/SjLj C++ ABI.
    ///
    /// When an app ships a complete SjLj runtime (libgcc_s + libc++abi /
    /// libstdc++), its own unwinder can handle `throw`/`catch` correctly.
    /// Host-side stubs can only fake a non-local return, so resolving any of
    /// these symbols to the host breaks C++ exception handling. This map is
    /// built from *defined* symbols (not just external ones) so that a
    /// non-exported-but-defined `___cxa_throw` still wins over the host stub.
    guest_cxxabi_exports: HashMap<String, u32>,
    /// Guest-originated SVC traps that don't map to any function we linked,
    /// keyed by `(svc_pc, svc)`, with the number of times each was reported.
    /// A stray trap can be hit on every frame, so logging is capped.
    stray_svc_reports: HashMap<(u32, u32), u32>,
}

impl Dyld {
    /// We reserve this SVC ID for invoking the lazy linker.
    pub const SVC_LAZY_LINK: u32 = 0;
    /// We reserve this SVC ID for the exit routine for spawned threads.
    pub const SVC_THREAD_EXIT: u32 = 1;
    /// We reserve this SVC ID for the special return-to-host routine.
    pub const SVC_RETURN_TO_HOST: u32 = 2;
    /// The range of SVC IDs `SVC_LINKED_FUNCTIONS_BASE..` is used to reference
    /// [Self::linked_host_functions] entries.
    pub const SVC_LINKED_FUNCTIONS_BASE: u32 = Self::SVC_RETURN_TO_HOST + 1;
    /// We reserve this SVC ID for lazy linking and returning right after.
    /// It is also a mask for the linked functions to indicate that an
    /// additional return instruction needs to be manually executed after
    /// handling the SVC.
    pub const SVC_LAZY_LINK_RET_FLAG: u32 = 0x800000;

    const SYMBOL_STUB1_INSTRUCTIONS: [u32; 1] = [0xe59ff000];
    // mask this with lowest 12 bits to restore instructions
    const SYMBOL_STUB_INSTRUCTIONS: [u32; 2] = [0xe59fc000, 0xe59cf000];
    const PIC_SYMBOL_STUB_INSTRUCTIONS: [u32; 3] = [0xe59fc004, 0xe08fc00c, 0xe59cf000];

    pub fn new() -> Dyld {
        Dyld {
            linked_host_functions: Vec::new(),
            return_to_host_routine: None,
            thread_exit_routine: None,
            constants_to_link_later: Vec::new(),
            host_constant_addresses: HashMap::new(),
            non_lazy_host_functions: HashMap::new(),
            swift_fn_cache: HashMap::new(),
            swift_fn_names: HashMap::new(),
            swift_data_slots: HashMap::new(),
            cxxabi_typeinfo_vtable_kinds: HashMap::new(),
            guest_sjlj_runtime_available: false,
            guest_cxxabi_exports: HashMap::new(),
            stray_svc_reports: HashMap::new(),
        }
    }

    pub(crate) fn cxxabi_typeinfo_kind(
        &self,
        typeinfo_vtable_addrpoint: u32,
    ) -> Option<CxxAbiTypeInfoKind> {
        self.cxxabi_typeinfo_vtable_kinds
            .get(&typeinfo_vtable_addrpoint)
            .copied()
    }

    /// Guest address of an app-provided C++ ABI implementation, if the app
    /// ships a working SjLj runtime and defines the symbol itself.
    pub(crate) fn guest_cxxabi_export(&self, name: &str) -> Option<u32> {
        if !self.guest_sjlj_runtime_available {
            return None;
        }
        self.guest_cxxabi_exports.get(name).copied()
    }

    pub fn return_to_host_routine(&self) -> GuestFunction {
        self.return_to_host_routine.unwrap()
    }

    pub fn thread_exit_routine(&self) -> GuestFunction {
        self.thread_exit_routine.unwrap()
    }

    /// Do linking-related tasks that need doing right after loading the
    /// binaries.
    pub fn do_initial_linking(
        &mut self,
        bundle: &bundle::Bundle,
        bins: &[MachO],
        mem: &mut Mem,
        objc: &mut ObjC,
        preserve_stubs: bool,
    ) {
        assert!(self.return_to_host_routine.is_none());
        assert!(self.thread_exit_routine.is_none());
        self.guest_sjlj_runtime_available = has_guest_sjlj_runtime(|symbol| {
            bins.iter()
                .any(|bin| bin.external_symbols.contains_key(symbol))
        });
        log!(
            "Guest C++ SjLj runtime available: {}",
            self.guest_sjlj_runtime_available
        );
        self.guest_cxxabi_exports.clear();
        if self.guest_sjlj_runtime_available {
            // Prefer the dylibs' definitions to the app's own. An app may
            // define a C++ ABI function as a hook that forwards to the real
            // one through `dlsym(RTLD_NEXT, ...)` (Unity's IL2CPP does this
            // for `__cxa_throw`); resolving to the hook then makes it call
            // itself until its stack overflows into another thread's.
            for bin in bins.iter().skip(1).chain(bins.first()) {
                for (name, &addr) in bin.exported_symbols.iter() {
                    if addr != 0 && is_guest_cxxabi_symbol(name) {
                        self.guest_cxxabi_exports
                            .entry(name.clone())
                            .or_insert(addr);
                    }
                }
            }
            log!(
                "Guest C++ ABI implementations: {} symbols will bypass host stubs",
                self.guest_cxxabi_exports.len()
            );
        }
        self.return_to_host_routine =
            Some(write_return_to_host_routine(mem, Self::SVC_RETURN_TO_HOST));
        self.thread_exit_routine = Some(write_return_to_host_routine(mem, Self::SVC_THREAD_EXIT));
        for bin in bins {
            objc.register_bin_selectors(bin, mem);
        }
        objc.register_host_selectors(mem);
        for bin in bins {
            // Must happen before `register_bin_classes`, else superclass
            // pointers will be wrong.
            self.do_non_lazy_linking(bin, bins, mem, objc);
            // Eager binding overwrites pointer slots. Apply relocation addends
            // first so a slot shared with an external relocation is not bound
            // twice, treating the first resolved address as an addend.
            self.setup_lazy_linking(bin, mem, preserve_stubs.then_some(bins));
        }

        for bin in bins {
            objc.register_bin_classes(bundle, bin, mem);
        }
        // A superclass may belong to another image. Reconcile only once all
        // images' class objects have been registered.
        objc.reconcile_bin_class_ivars(mem);
        for bin in bins {
            objc.register_bin_categories(bin, mem);
            ns_string::register_constant_strings(bin, mem, objc);
        }

        // The `std::__throw_*` helpers in the bundled libstdc++ dylib are
        // `noreturn` — they construct and throw a C++ exception. touchHLE
        // has no real unwinder, so a thrown exception inside libstdc++
        // calls `__cxa_terminate()`, which aborts the host process. The
        // user-visible message is something like:
        //
        //   terminate called after throwing an instance of 'std::logic_error'
        //     what():  basic_string::_S_construct NULL not valid
        //
        // To allow games to recover from "soft" failures (e.g. Geometry
        // Dash 1.0 hitting `std::string(const char*)` with a NULL argument
        // during Cocos2d initialization), we splice an immediate `BX LR` at
        // the entry of each `__throw_*` helper. Callers in libstdc++
        // already check the failure condition before calling the helper —
        // making the helper a no-op causes them to continue with the
        // failed state (e.g. `_S_construct` continues with NULL/zero
        // iterators and produces an empty string).
        //
        // This is undefined behaviour in strict ISO C++, but in practice
        // the caller paths in libstdc++ degrade to a no-op or empty
        // result, which is far less disruptive than aborting the entire
        // emulator. iOS dyld did roughly the same thing for these games:
        // historically `std::string(NULL)` would yield an empty string on
        // Apple's libstdc++ as well (and Geometry Dash 1.0 launched on
        // Adreno devices that hit this code path).
        if !self.guest_sjlj_runtime_available {
            for dylib in bins.iter().skip(1) {
                let mut patch_count = 0;
                for sym in [
                    "__ZSt19__throw_logic_errorPKc",
                    "__ZSt20__throw_length_errorPKc",
                    "__ZSt20__throw_out_of_rangePKc",
                    "__ZSt17__throw_bad_allocv",
                    "__ZSt16__throw_bad_castv",
                    "__ZSt19__throw_range_errorPKc",
                    "__ZSt22__throw_overflow_errorPKc",
                    "__ZSt23__throw_underflow_errorPKc",
                    "__ZSt21__throw_runtime_errorPKc",
                    "__ZSt24__throw_invalid_argumentPKc",
                    "__ZSt23__throw_ios_failurePKc",
                    "__ZSt18__throw_bad_typeidv",
                    "__ZSt19__throw_bad_exceptionv",
                    "__ZSt25__throw_bad_function_callv",
                ] {
                    let Some(&entry_with_thumb_bit) = dylib.exported_symbols.get(sym) else {
                        continue;
                    };
                    let entry = entry_with_thumb_bit & !1;
                    let is_thumb = (entry_with_thumb_bit & 1) != 0;
                    if is_thumb {
                        // Thumb-1 BX LR = 0x4770. Pad the 32-bit word with a NOP
                        // (0x46C0 = mov r8, r8) so the surrounding code stays
                        // valid if execution somehow falls through.
                        let function_ptr: MutPtr<u32> = Ptr::from_bits(entry);
                        mem.write(function_ptr, 0x46C0_4770);
                    } else {
                        // ARM `BX LR` = 0xE12FFF1E.
                        let function_ptr: MutPtr<u32> = Ptr::from_bits(entry);
                        mem.write(function_ptr, 0xE12FFF1E);
                    }
                    patch_count += 1;
                    log_dbg!(
                        "Patched libstdc++ {} at {:#x} (thumb={}) -> bx lr",
                        sym,
                        entry_with_thumb_bit,
                        is_thumb
                    );
                }
                if patch_count > 0 {
                    log!(
                        "Patched {} libstdc++ std::__throw_* helpers to return \
                         instead of throwing (avoids host-process abort when \
                         guest C++ code hits soft failures like std::string(NULL)).",
                        patch_count
                    );
                }
            }
        }

        // Always redirect `std::string((const char*)NULL)` in bundled libstdc++
        // to the empty-string representation, even when the guest SjLj runtime
        // is active. Games such as Geometry Dash 1.0 and Turbo Dismount pass
        // NULL to `std::string(const char*)` during startup without catching
        // `std::logic_error`.
        for dylib in bins.iter().skip(1) {
            patch_string_s_construct_null(dylib, mem);
        }
    }

    /// Dumps all lazy symbols (functions) referenced by the binary
    /// as JSON to stdout.
    ///
    /// The JSON has the following form:
    /// ```json
    /// {
    ///     "object": "lazy_symbols",
    ///     "symbols": [
    ///         {
    ///             "symbol": ((name of symbol)),
    ///             "linked_to": "host" | "dylib" | "local" | null,
    ///             "dylib": ((name of dylib)) | null,
    ///         },
    ///         ...
    ///     ]
    /// }
    /// ```
    pub fn dump_lazy_symbols(
        &mut self,
        bins: &[MachO],
        file: &mut std::fs::File,
    ) -> Result<(), std::io::Error> {
        use std::io::Write;
        // Guest binary is always bin 0.
        let stubs = bins[0].get_section(SectionType::SymbolStubs).unwrap();
        let info = stubs.dyld_indirect_symbol_info.as_ref().unwrap();
        writeln!(
            file,
            "{{\n    \"object\":\"lazy_symbols\",\n    \"symbols\": ["
        )?;
        'sym: for (i, symbol) in info.indirect_undef_symbols.iter().enumerate() {
            // Why doesn't json allow trailing commas...
            let comma = if i == info.indirect_undef_symbols.len() - 1 {
                ""
            } else {
                ","
            };
            let symbol = symbol.as_ref().unwrap();
            if let Some(sym) = cxxabi_intercept_symbol(symbol, self.guest_sjlj_runtime_available) {
                writeln!(
                    file,
                    "        {{ \"symbol\": \"{sym}\", \"linked_to\": \"host\"}}{comma}"
                )?;
                continue 'sym;
            }
            for dylib in bins.iter() {
                if dylib.external_symbols.contains_key(symbol) {
                    writeln!(
                        file,
                        "        {{ \"symbol\": \"{}\", \"linked_to\": \"dylib\", \"dylib\": \"{}\"}}{}",
                        symbol, dylib.name, comma
                    )?;
                    continue 'sym;
                }
            }
            if let Some((_, _)) = search_host_dylibs(|dylib| dylib.function_exports, symbol) {
                writeln!(
                    file,
                    "        {{ \"symbol\": \"{symbol}\", \"linked_to\": \"host\"}}{comma}"
                )?;
                continue;
            }
            if bins
                .first()
                .is_some_and(|bin| local_code_symbol(bin, symbol).is_some())
            {
                writeln!(
                    file,
                    "        {{ \"symbol\": \"{symbol}\", \"linked_to\": \"local\" }}{comma}"
                )?;
                continue;
            }
            writeln!(file, "        {{ \"symbol\": \"{symbol}\" }}{comma}")?;
        }
        writeln!(file, "    ]\n}}")
    }

    /// Dumps all non-objc symbols provided by touchHLE.
    ///
    /// The dump format is Objective-C code (with meaningless types) that can be
    /// compiled to generate stub libraries that can be linked against, with
    /// comments providing the paths each library would be installed to.
    /// Each library ends with an empty line, so that a library with no
    /// exports is not mistaken for aliases of the next one.
    /// This is used for building the integration tests.
    pub fn dump_host_symbols(file: &mut std::fs::File) -> Result<(), std::io::Error> {
        use std::io::Write;
        for dylib in DYLIB_LIST {
            writeln!(file, "// {}", dylib.path)?;
            for alias in dylib.aliases {
                writeln!(file, "// {alias}")?;
            }
            for (class_name, _) in dylib.class_exports.iter().copied().flatten() {
                writeln!(file, "@interface {class_name}")?;
                writeln!(file, "@end")?;
                writeln!(file, "@implementation {class_name}")?;
                writeln!(file, "@end")?;
            }
            for (constant_symbol, _) in dylib.constant_exports.iter().copied().flatten() {
                let name = constant_symbol.strip_prefix("_").unwrap();
                // Some symbols (e.g. `OBJC_IVAR_$_NSObject.isa`) contain
                // characters that aren't valid in a C identifier, so they
                // can't be used as a declaration name directly. Emit a
                // sanitized identifier with an `asm()` label so the stub still
                // exports the real symbol name the app links against.
                if name.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$')) {
                    let sanitized: String = name
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || c == '$' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    writeln!(file, "int {sanitized} asm(\"{constant_symbol}\");")?;
                } else {
                    writeln!(file, "int {name};")?;
                }
            }
            for (function_symbol, _) in dylib.function_exports.iter().copied().flatten() {
                writeln!(
                    file,
                    "void {}() {{}}",
                    function_symbol.strip_prefix("_").unwrap()
                )?;
            }
            writeln!(file)?;
        }
        Ok(())
    }

    /// [Self::do_initial_linking] but for when this is the app picker's special
    /// environment with no binary (see [crate::Environment::new_without_app]).
    pub fn do_initial_linking_with_no_bins(&mut self, mem: &mut Mem, objc: &mut ObjC) {
        assert!(self.return_to_host_routine.is_none());
        assert!(self.thread_exit_routine.is_none());
        self.return_to_host_routine =
            Some(write_return_to_host_routine(mem, Self::SVC_RETURN_TO_HOST));
        self.thread_exit_routine = Some(write_return_to_host_routine(mem, Self::SVC_THREAD_EXIT));

        objc.register_host_selectors(mem);
    }

    /// Set up lazy-linking stubs for a loaded binary.
    ///
    /// Dynamic linking of functions on iPhone OS usually happens "lazily",
    /// which means that the linking is delayed until the function is first
    /// called. This is achieved by using stub functions. Instead of calling the
    /// external function directly, the app code will call a stub function, and
    /// that stub will either jump to the dynamic linker (which will link in the
    /// external function and then jump to it), or on subsequent calls, jump
    /// straight to the external function.
    ///
    /// These stubs already exist in the binary, but they need to be rewritten
    /// so that they will invoke our dynamic linker.
    fn setup_lazy_linking(&mut self, bin: &MachO, mem: &mut Mem, preserve: Option<&[MachO]>) {
        let Some(stubs) = bin.get_section(SectionType::SymbolStubs) else {
            return;
        };

        let Some(indirect_info) = stubs.dyld_indirect_symbol_info.as_ref() else {
            log!(
                "Warning: setup_lazy_linking: __symbol_stub section is missing dyld indirect symbol info; skipping lazy stub rewrite."
            );
            return;
        };
        let entry_size = indirect_info.entry_size;

        // two or three A32 instructions (PIC stub needs one more) followed by
        // the address or offset of the corresponding __la_symbol_ptr
        let expected_instructions: &[u32] = match entry_size {
            4 => &[],
            12 => Self::SYMBOL_STUB_INSTRUCTIONS.as_slice(),
            16 => Self::PIC_SYMBOL_STUB_INSTRUCTIONS.as_slice(),
            other => {
                log!(
                    "Warning: setup_lazy_linking: unsupported stub entry size {}; skipping lazy stub rewrite.",
                    other
                );
                return;
            }
        };

        assert!(stubs.size % entry_size == 0);
        let stub_count = stubs.size / entry_size;
        let mut skipped_stubs: Vec<u32> = Vec::new();
        let mut skipped_stub_count = 0usize;
        for i in 0..stub_count {
            // LOCAL/ABS entries describe already-bound pointers, rather than
            // undefined symbols. Preserve their original call sequence.
            // https://github.com/apple-oss-distributions/dyld/blob/dyld-360.22/src/ImageLoaderMachOClassic.cpp
            if indirect_info
                .indirect_undef_symbols
                .get(i as usize)
                .and_then(Option::as_ref)
                .is_none()
            {
                continue;
            }
            let ptr: MutPtr<u32> = Ptr::from_bits(stubs.addr + i * entry_size);

            // Check that the stub matches the expected pattern.
            // Some apps (e.g. Thumb-compiled) may have different patterns —
            // skip them rather than panicking.
            let mut mismatch = false;
            for (j, &instr) in expected_instructions.iter().enumerate() {
                if mem.read(ptr + j.try_into().unwrap()) != instr {
                    log_dbg!(
                        "Warning: stub {} at {:#x} has unexpected instruction at offset {} \
                         (expected {:#010x}, got {:#010x}), skipping",
                        i,
                        stubs.addr + i * entry_size,
                        j,
                        instr,
                        mem.read::<u32, true>(ptr + j.try_into().unwrap())
                    );
                    mismatch = true;
                    break;
                }
            }
            if mismatch {
                skipped_stub_count += 1;
                if skipped_stubs.len() < 8 {
                    skipped_stubs.push(stubs.addr + i * entry_size);
                }
                continue;
            }

            if let Some(bins) = preserve {
                // iOS dyld binds the pointer slot, leaving these instructions
                // intact. Engines can therefore inspect/checksum their code.
                // https://github.com/apple-oss-distributions/dyld/blob/dyld-210.2.3/src/ImageLoaderMachOClassic.cpp
                let slot = match entry_size {
                    4 => {
                        let instruction = mem.read(ptr);
                        assert_eq!(instruction & !0xfff, Self::SYMBOL_STUB1_INSTRUCTIONS[0]);
                        ptr.to_bits().wrapping_add(8 + (instruction & 0xfff))
                    }
                    12 => mem.read(ptr + 2),
                    16 => ptr
                        .to_bits()
                        .wrapping_add(12)
                        .wrapping_add(mem.read(ptr + 3)),
                    _ => unreachable!(),
                };
                let symbol = indirect_info.indirect_undef_symbols[i as usize]
                    .as_deref()
                    .unwrap();
                let target = self.resolve_preserved_stub(bin, bins, mem, symbol);
                mem.write(Ptr::<u32, true>::from_bits(slot), target);
                continue;
            }

            // For convenience, make the stub return once the SVC is done
            // (Otherwise we have to manually update the PC)
            if entry_size == 4 {
                mem.write(ptr + 0, encode_a32_svc(Self::SVC_LAZY_LINK_RET_FLAG));
            } else {
                mem.write(ptr + 0, encode_a32_svc(Self::SVC_LAZY_LINK));
                mem.write(ptr + 1, encode_a32_ret());
            }
            if entry_size == 16 {
                // This is preceded by a return instruction, so if we do execute
                // it, something has gone wrong.
                mem.write(ptr + 2, encode_a32_trap());
            }
            // Leave the __la_symbol_ptr intact in case we want to link it to
            // a real symbol later.
        }
        if !skipped_stubs.is_empty() {
            log!(
                "Warning: setup_lazy_linking: {} of {} stubs in {:?} did not match the \
                 expected pattern and were left unpatched (first: {:#x?}). Calls through \
                 them jump to the unrelocated __la_symbol_ptr and can land on the image base.",
                skipped_stub_count,
                stub_count,
                bin.name,
                skipped_stubs
            );
        }
    }

    /// Use the same symbol precedence as the lazy linker, but bind up front
    /// so calls need no patches inside the app's instruction stream.
    fn resolve_preserved_stub(
        &mut self,
        bin: &MachO,
        bins: &[MachO],
        mem: &mut Mem,
        symbol: &str,
    ) -> u32 {
        if let Some(address) = self.guest_cxxabi_export(symbol) {
            register_cxxabi_typeinfo_vtable(
                symbol,
                address,
                &mut self.cxxabi_typeinfo_vtable_kinds,
            );
            return address;
        }
        if let Some(address) = self.non_lazy_host_functions.get(symbol) {
            return address.addr_with_thumb_bit();
        }
        if let Some(address) = self.cxxabi_intercept(mem, symbol) {
            return address.addr_with_thumb_bit();
        }
        if let Some(address) = bins
            .iter()
            .find_map(|image| image.external_symbols.get(symbol))
        {
            let address = *address;
            register_cxxabi_typeinfo_vtable(
                symbol,
                address,
                &mut self.cxxabi_typeinfo_vtable_kinds,
            );
            return address;
        }
        if let Ok(address) = self.create_proc_address_no_inval(mem, symbol) {
            return address.addr_with_thumb_bit();
        }
        if let Some(address) = local_code_symbol(bin, symbol) {
            return address;
        }
        log!("Warning: unimplemented import {symbol}; binding existing return-0 handler");
        let symbol: &'static str = Box::leak(symbol.to_owned().into_boxed_str());
        let function = self.create_guest_function(
            mem,
            symbol,
            &(unimplemented_function_stub as fn(&mut Environment) -> i32),
        );
        self.non_lazy_host_functions.insert(symbol, function);
        function.addr_with_thumb_bit()
    }

    /// Link non-lazy symbols for a loaded binary.
    ///
    /// These are usually constants, Objective-C classes, or vtable pointers.
    /// Since the linking must be done upfront, we can't in general delay errors
    /// about missing implementations until the point of use. For that reason,
    /// this will spit out a warning to stderr for everything missing, so that
    /// there's at least some indication about why the emulator might crash.
    ///
    /// `bin` is the binary to link non-lazy symbols for, `bins` is the set of
    /// binaries symbols may be looked up in.
    fn do_non_lazy_linking(&mut self, bin: &MachO, bins: &[MachO], mem: &mut Mem, objc: &mut ObjC) {
        let mut unhandled_relocations: HashMap<&str, Vec<u32>> = HashMap::new();
        // Cache for Objective-C blocks runtime class descriptors.
        // All relocation sites for the same name must resolve to the same
        // non-null guest address so that `block->isa ==
        // &_NSConcreteGlobalBlock`
        // identity checks in the blocks runtime work correctly.
        // Without this, the isa field of every global/stack block is NULL
        // (0x0),
        // causing a NULL-page read the first time the ObjC runtime tries to
        // retain or release a block object.
        let mut block_class_addrs: HashMap<String, u32> = HashMap::new();
        // Cache for C++ Itanium ABI type_info vtables. All references to the
        // same vtable symbol must resolve to the same address so that vtable
        // identity checks in dynamic_cast work correctly.
        let mut cxxabi_vtable_addrs: HashMap<String, u32> = HashMap::new();
        for other_bin in bins {
            for (name, &addr) in &other_bin.exported_symbols {
                register_cxxabi_typeinfo_vtable(name, addr, &mut self.cxxabi_typeinfo_vtable_kinds);
            }
        }
        for &(ptr_ptr, ref name) in &bin.external_relocations {
            let ptr_ptr: MutPtr<ConstVoidPtr> = Ptr::from_bits(ptr_ptr);
            // There will be an existing value at the address, which is an
            // offset that should be applied to the external symbol's address.
            // It is often 0, but not always.
            let offset: u32 = mem.read(ptr_ptr).to_bits();
            let guest_cxxabi_export =
                if self.guest_sjlj_runtime_available && is_guest_cxxabi_symbol(name) {
                    self.guest_cxxabi_export(name)
                } else {
                    None
                };
            // Keep guest vtable methods for exception handling; the host RTTI walker
            // needs the Itanium class kind at each guest vtable address point.
            if let Some(addr) = guest_cxxabi_export {
                register_cxxabi_typeinfo_vtable(name, addr, &mut self.cxxabi_typeinfo_vtable_kinds);
            }
            let guest_external = bins
                .iter()
                .find_map(|other_bin| other_bin.external_symbols.get(name))
                .copied();
            let target: ConstVoidPtr = if let Some(target) = crate::objc::exceptions::link_symbol(self, objc, mem, name) {
                target
            } else if let Some(class_name) = name.strip_prefix("_OBJC_CLASS_$_")
            {
                guest_external.map(Ptr::from_bits).unwrap_or_else(|| {
                    objc.link_class(class_name, /* is_metaclass: */ false, mem)
                        .cast()
                        .cast_const()
                })
            } else if let Some(class_name) = name.strip_prefix("_OBJC_METACLASS_$_") {
                guest_external.map(Ptr::from_bits).unwrap_or_else(|| {
                    objc.link_class(class_name, /* is_metaclass: */ true, mem)
                        .cast()
                        .cast_const()
                })
            } else if name == "___CFConstantStringClassReference" {
                // See ns_string::register_constant_strings
                nil.cast().cast_const()
            } else if let Some(addr) = guest_cxxabi_export {
                log_dbg!("Linked guest C++ ABI symbol {} at {:#x}", name, addr);
                Ptr::from_bits(addr)
            } else if name == "___mb_cur_max" {
                // __mb_cur_max is a pointer to the C locale's max
                // multibyte character byte count. libstdc++ reads
                // *__mb_cur_max in locale/string code; if this pointer
                // is NULL the dereference crashes. Provide a static
                // value of 1 (single-byte / ASCII locale).
                let val_ptr: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(val_ptr, 1u32);
                log_dbg!("Stubbed ___mb_cur_max at {:?}", val_ptr);
                val_ptr.cast().cast_const()
            } else if name == "dyld_stub_binder" || name == "_dyld_stub_binder" {
                // In iOS, dyld_stub_binder handles lazy symbol binding.
                // However, touchHLE resolves lazy symbols entirely via SVC
                // traps,
                // bypassing the need for a guest-side binder.
                // We create a minimal ARM32 function (BX LR) so if it's somehow
                // called, it just returns safely without executing random
                // memory.
                let fn_ptr: MutPtr<u32> = mem.alloc(8).cast();
                mem.write(fn_ptr + 0, encode_a32_ret());
                mem.write(fn_ptr + 1, encode_a32_trap());
                log_dbg!("Stubbed dyld_stub_binder at {:?}", fn_ptr);
                fn_ptr.cast().cast_const()
            } else if name == "__NSConcreteGlobalBlock"
                || name == "__NSConcreteStackBlock"
                || name == "__NSConcreteMallocBlock"
            {
                // Blocks runtime class descriptor. Point the block `isa` at
                // the real `_touchHLE_NSBlock` class (see ns_object.rs) so
                // that `[block copy]` / `[block retain]` / `[block release]`
                // route to the Blocks runtime instead of hitting a dummy
                // class with no methods (which returned nil and left callback
                // blocks, e.g. cocos2d v3 `CCTimer._block`, nil). All block
                // symbols share this one class; `_Block_copy` uses the block's
                // flags (not its isa) to distinguish stack/global/malloc.
                let _ = &mut block_class_addrs;
                objc.get_known_class("_touchHLE_NSBlock", mem)
                    .cast()
                    .cast_const()
            } else if name == "__ZTVN10__cxxabiv117__class_type_infoE"
                || name == "__ZTVN10__cxxabiv120__si_class_type_infoE"
                || name == "__ZTVN10__cxxabiv121__vmi_class_type_infoE"
                || name == "__ZTVN10__cxxabiv119__pointer_type_infoE"
                || name == "__ZTVN10__cxxabiv120__function_type_infoE"
                || name == "__ZTVN10__cxxabiv116__enum_type_infoE"
                || name == "__ZTVN10__cxxabiv117__pbase_type_infoE"
                || name == "__ZTVN10__cxxabiv129__pointer_to_member_type_infoE"
            {
                // C++ Itanium ABI type_info vtable. Without this every
                // type_info object in the app has a NULL vptr, and any call
                // through the vptr (dynamic_cast, exception type matching,
                // type_info comparison) lands on address 0 and crashes.
                //
                // Layout (32-bit ARM, addend +8 from the symbol so callers
                // land on the first method):
                //   +0   offset_to_top   (= 0)
                //   +4   typeinfo for the vtable's class (= 0; never used)
                //   +8   ~type_info() complete dtor   -> BX LR stub
                //   +12  ~type_info() deleting dtor   -> BX LR stub
                //   +16  __is_pointer_p()             -> returns 0 (false)
                //   +20  __is_function_p()            -> returns 0 (false)
                //   +24  __do_catch()                 -> returns 0 (no match)
                //   +28  __do_upcast()                -> returns 0 (no match)
                //   +32  reserved
                //   +36  reserved
                //
                // The relocation addend (usually +8) is applied below, so we
                // return the vtable base here.
                let target = link_cxxabi_vtable(
                    name,
                    &mut cxxabi_vtable_addrs,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                    mem,
                );
                log_dbg!("Stubbed C++ vtable {} -> {:#x}", name, target.to_bits());
                target
            } else if name == "___gxx_personality_sj0" {
                // C++ SjLj exception personality routine. Called by the
                // unwinder for every frame; returning 0 (_URC_NO_REASON)
                // tells it "no handler here, keep going". touchHLE never
                // actually invokes the unwinder, but other code may store
                // this pointer in an LSDA and read it back.
                let fn_ptr: MutPtr<u32> = mem.alloc(8).cast();
                mem.write(fn_ptr + 0, encode_a32_ret());
                mem.write(fn_ptr + 1, encode_a32_trap());
                log_dbg!("Stubbed ___gxx_personality_sj0 -> {:#x}", fn_ptr.to_bits());
                fn_ptr.cast().cast_const()
            } else if name == "__ZTId"
                || name == "__ZTIf"
                || name == "__ZTIi"
                || name == "__ZTIl"
                || name == "__ZTIj"
                || name == "__ZTIs"
                || name == "__ZTIc"
                || name == "__ZTIv"
                || name == "__ZTIb"
                || name == "__ZTIPKc"
                || name == "__ZTIPc"
                || name == "__ZTIPv"
                || name == "__ZTIPKv"
                || name == "__ZTIw"
                || name == "__ZTIPw"
                || name == "__ZTIPKw"
                || name == "__ZTIw"
                || name == "__ZTIPw"
                || name == "__ZTIPKw"
            {
                // C++ RTTI type_info objects for fundamental types (double,
                // float, int, long, unsigned int, short, char, void, bool,
                // wchar_t, const char*, char*, void*, const void*,
                // wchar_t*, wchar_t const*).
                //
                // The Itanium ABI requires each fundamental type to have a
                // unique type_info object with a specific mangled name. Apps
                // that use dynamic_cast or exception handling may reference
                // these. We allocate a minimal type_info struct:
                //   +0: vptr (→ __cxxabiv117__class_type_infoE vtable + 8)
                //   +4: name pointer (→ mangled type name C string)
                //
                // The name string is never actually used for comparison (RTTI
                // compares by pointer identity on most embedded platforms), but
                // providing a non-NULL value prevents crashes in debuggers.
                let ti = link_cxxabi_typeinfo(
                    name,
                    &mut cxxabi_vtable_addrs,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                    mem,
                );
                log_dbg!("Stubbed C++ typeinfo {} at {:#x}", name, ti.to_bits());
                ti
            } else if name == "___cxa_terminate_handler"
                || name == "___cxa_unexpected_handler"
                || name == "___cxa_new_handler"
            {
                // Global function-pointer variables: std::terminate_handler,
                // std::unexpected_handler, std::new_handler. The default
                // value is a NULL function pointer; the real C++ runtime
                // checks for NULL and falls back to abort. Provide a single
                // word of zero-initialised guest memory.
                let p: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(p, 0);
                p.cast().cast_const()
            } else if name == "__dispatch_source_type_read"
                || name == "__dispatch_source_type_write"
                || name == "__dispatch_source_type_timer"
                || name == "__dispatch_source_type_data_add"
                || name == "__dispatch_source_type_data_or"
                || name == "__dispatch_source_type_data_replace"
                || name == "__dispatch_source_type_signal"
                || name == "__dispatch_source_type_proc"
                || name == "__dispatch_source_type_vnode"
                || name == "__dispatch_source_type_mach_send"
                || name == "__dispatch_source_type_mach_recv"
                || name == "__dispatch_source_type_memorypressure"
                || name == "__dispatch_queue_attr_concurrent"
                || name == "__dispatch_queue_attr_serial"
                || name == "__dispatch_data_empty"
                || name == "__dispatch_data_destructor_default"
                || name == "__dispatch_data_destructor_free"
                || name == "__dispatch_data_destructor_munmap"
            {
                // Dispatch source types carry small internal tags; other
                // libdispatch constants remain opaque non-NULL markers.
                let p: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(p, crate::libc::dispatch_source::source_type_tag(name));
                p.cast().cast_const()
            } else if name == "_in6addr_any" || name == "_in6addr_loopback" {
                // `struct in6_addr` is 16 bytes. `in6addr_any` is all zeros
                // (`::`) and `in6addr_loopback` is `::1` (last byte = 1).
                let p: MutPtr<u8> = mem.alloc(16).cast();
                for i in 0..16 {
                    mem.write(p + i, 0);
                }
                if name == "_in6addr_loopback" {
                    mem.write(p + 15, 1u8);
                }
                p.cast().cast_const()
            } else if name == "_objc_ehtype_vtable" {
                // Apple libobjc (objc4) ships `objc_ehtype_vtable` as the
                // Itanium-ABI C++ typeinfo vtable shared by every
                // per-class `__objc_ehtype` struct used in Objective-C++
                // `@catch (ObjCClass *)` blocks. touchHLE's `__cxa_throw`
                // is a no-op (we never actually unwind), so the vtable is
                // not consulted for type matching — but t
                let p: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(p, 0);
                p.cast().cast_const()
            } else if name == "_NDR_record" {
                // MIG `NDR_record` is a 12-byte transfer-syntax descriptor
                // that's part of the (unused-by-touchHLE) Mach IPC stack.
                // Apps reference the symbol from auto-generated MIG stubs but
                // never actually transmit the bytes. Provide a zero-filled
                // slot so the linker doesn't leave a NULL pointer behind.
                let p: MutPtr<u8> = mem.alloc(12).cast();
                for i in 0..12 {
                    mem.write(p + i, 0);
                }
                p.cast().cast_const()
            } else if let Some(stub) = self.cxxabi_intercept(mem, name) {
                Ptr::<std::ffi::c_void, false>::from_bits(stub.to_ptr().to_bits())
            } else if let Some(external_addr) = guest_external {
                Ptr::from_bits(external_addr)
            } else if let Some(link) = self.swift_intercept(mem, name) {
                match link {
                    swift_runtime::SwiftLink::Function(f) => {
                        Ptr::<std::ffi::c_void, false>::from_bits(f.to_ptr().to_bits())
                    }
                    swift_runtime::SwiftLink::Data(slot) => slot,
                }
            } else if let Some((symbol, _)) =
                search_host_dylibs(|dylib| dylib.function_exports, name)
            {
                // We want the same symbol name to always point to the same
                // function.
                let trampoline_ptr = self
                    .create_proc_address_no_inval(mem, symbol)
                    .unwrap()
                    .to_ptr();
                log_dbg!(
                    "Linked external relocation to host function {} at {:?}",
                    symbol,
                    trampoline_ptr
                );
                trampoline_ptr
            } else if let Some((symbol, _)) =
                search_host_dylibs(|dylib| dylib.constant_exports, name)
            {
                // Constants from host dylibs need late linking (they may
                // require a full Environment to resolve, e.g. NSString
                // objects). Store for resolution in do_late_linking().
                // NOTE: We must NOT skip these — a binary may reference a
                // constant ONLY via external relocations without a matching
                // __nl_symbol_ptr entry (e.g. ___sF in Digital Chocolate
                // games compiled with older toolchains).
                self.constants_to_link_later.push((ptr_ptr, symbol));
                continue;
            } else {
                unhandled_relocations
                    .entry(name)
                    .or_default()
                    .push(ptr_ptr.to_bits());
                continue;
            };
            // wrapping_add() is used in case the offset is negative. I haven't
            // seen it happen, but it would make sense if that is allowed.
            mem.write(
                ptr_ptr,
                Ptr::from_bits(target.to_bits().wrapping_add(offset)),
            )
        }
        // Collecting unhandled relocations for the same symbol onto one line
        // makes the log output much less spammy.
        for (name, addrs) in unhandled_relocations {
            log!(
                "Warning: unhandled external relocation {:?} in {:?} at {}",
                name,
                bin.name,
                addrs
                    .into_iter()
                    .map(|addr| format!("{addr:#x}"))
                    .collect::<Vec<String>>()
                    .join(", "),
            );
        }

        let Some(ptrs) = bin.get_section(SectionType::NonLazySymbolPointers) else {
            return;
        };
        let info = ptrs.dyld_indirect_symbol_info.as_ref().unwrap();

        let entry_size = info.entry_size;
        assert!(entry_size == 4);
        assert!(ptrs.size % entry_size == 0);
        let ptr_count = ptrs.size / entry_size;
        'ptr_loop: for i in 0..ptr_count {
            let Some(symbol) = info.indirect_undef_symbols[i as usize].as_deref() else {
                continue;
            };

            let ptr_ptr: MutPtr<ConstVoidPtr> = Ptr::from_bits(ptrs.addr + i * entry_size);
            for other_bin in bins {
                if let Some(&addr) = other_bin.external_symbols.get(symbol) {
                    register_cxxabi_typeinfo_vtable(
                        symbol,
                        addr,
                        &mut self.cxxabi_typeinfo_vtable_kinds,
                    );
                    mem.write(ptr_ptr, Ptr::from_bits(addr));
                    continue 'ptr_loop;
                }
            }

            // A defined-but-not-external C++ ABI symbol (common in
            // libc++abi.dylib, whose `___cxa_throw` is not marked N_EXT)
            // would otherwise fall through to the host stub below.
            if let Some(addr) = self.guest_cxxabi_export(symbol) {
                register_cxxabi_typeinfo_vtable(
                    symbol,
                    addr,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                );
                mem.write(ptr_ptr, Ptr::from_bits(addr));
                continue 'ptr_loop;
            }

            if symbol == "dyld_stub_binder" || symbol == "_dyld_stub_binder" {
                // Используем наш паникующий хэндлер, который вы
                // зарегистрировали в create_proc_address_no_inval
                let trampoline_ptr = self
                    .create_proc_address_no_inval(mem, symbol)
                    .unwrap()
                    .to_ptr();
                mem.write(ptr_ptr, trampoline_ptr);
                log_dbg!(
                    "Linked non-lazy host function {} at {:?}",
                    symbol,
                    trampoline_ptr
                );
                continue;
            }

            if let Some((symbol, _)) = search_host_dylibs(|dylib| dylib.function_exports, symbol) {
                // We want the same symbol name to always point to the same
                // function. It could point to a specific stub entry, but it's
                // easier to just create a new function and point all the stub
                // entries to it.
                let trampoline_ptr = self
                    .create_proc_address_no_inval(mem, symbol)
                    .unwrap()
                    .to_ptr();
                mem.write(ptr_ptr, trampoline_ptr);
                log_dbg!(
                    "Linked non-lazy host function {} at {:?}",
                    symbol,
                    trampoline_ptr
                );
                log_dbg!("{:?}", self.non_lazy_host_functions);
                continue;
            }
            if let Some((symbol, _)) = search_host_dylibs(|dylib| dylib.constant_exports, symbol) {
                // Delay linking of constant until we have a `&mut Environment`,
                // that makes it much easier to build NSString objects etc.
                self.constants_to_link_later.push((ptr_ptr, symbol));
                continue;
            }

            // Blocks runtime class descriptors in __nl_symbol_ptr.
            // Must be non-null so block->isa != NULL; otherwise the
            // first retain/release of any stack block causes a
            // NULL-page read at 0x0.
            if symbol == "__NSConcreteStackBlock"
                || symbol == "__NSConcreteGlobalBlock"
                || symbol == "__NSConcreteMallocBlock"
            {
                // Point the block `isa` at the real `_touchHLE_NSBlock` class
                // so block copy/retain/release work (see ns_object.rs and the
                // matching branch in the external-relocation loop above).
                let cls = objc.get_known_class("_touchHLE_NSBlock", mem);
                mem.write(ptr_ptr, cls.cast().cast_const());
                log_dbg!("Patched non-lazy block class {} -> {:?}", symbol, cls);
                continue;
            }

            if let Some(target) = crate::objc::exceptions::link_symbol(self, objc, mem, symbol) {
                mem.write(ptr_ptr, target);
                continue;
            }

            // __mb_cur_max is a pointer-to-int used by libstdc++
            // locale code. Provide a value of 1 (single-byte locale).
            if symbol == "___mb_cur_max" {
                let val_ptr: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(val_ptr, 1u32);
                mem.write(ptr_ptr, val_ptr.cast().cast_const());
                log_dbg!("Stubbed ___mb_cur_max -> {:#x}", val_ptr.to_bits());
                continue;
            }

            if symbol == "___gxx_personality_sj0" {
                let fn_ptr: MutPtr<u32> = mem.alloc(8).cast();
                mem.write(fn_ptr + 0, encode_a32_ret());
                mem.write(fn_ptr + 1, encode_a32_trap());
                mem.write(ptr_ptr, fn_ptr.cast().cast_const());
                log_dbg!("Stubbed ___gxx_personality_sj0 -> {:#x}", fn_ptr.to_bits());
                continue;
            }

            if symbol == "___cxa_terminate_handler"
                || symbol == "___cxa_unexpected_handler"
                || symbol == "___cxa_new_handler"
            {
                // Default-NULL global handler pointer.
                let p: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(p, 0);
                mem.write(ptr_ptr, p.cast().cast_const());
                continue;
            }

            if symbol == "__dispatch_source_type_read"
                || symbol == "__dispatch_source_type_write"
                || symbol == "__dispatch_source_type_timer"
                || symbol == "__dispatch_source_type_data_add"
                || symbol == "__dispatch_source_type_data_or"
                || symbol == "__dispatch_source_type_data_replace"
                || symbol == "__dispatch_source_type_signal"
                || symbol == "__dispatch_source_type_proc"
                || symbol == "__dispatch_source_type_vnode"
                || symbol == "__dispatch_source_type_mach_send"
                || symbol == "__dispatch_source_type_mach_recv"
                || symbol == "__dispatch_source_type_memorypressure"
                || symbol == "__dispatch_queue_attr_concurrent"
                || symbol == "__dispatch_queue_attr_serial"
                || symbol == "__dispatch_data_empty"
                || symbol == "__dispatch_data_destructor_default"
                || symbol == "__dispatch_data_destructor_free"
                || symbol == "__dispatch_data_destructor_munmap"
            {
                // See the matching branch in the external-relocation loop.
                let p: MutPtr<u32> = mem.alloc(4).cast();
                mem.write(p, crate::libc::dispatch_source::source_type_tag(symbol));
                mem.write(ptr_ptr, p.cast().cast_const());
                log_dbg!(
                    "libdispatch identity tag {} -> {:#x}",
                    symbol,
                    p.to_bits()
                );
                continue;
            }

            if symbol == "_in6addr_any" || symbol == "_in6addr_loopback" {
                let p: MutPtr<u8> = mem.alloc(16).cast();
                for i in 0..16 {
                    mem.write(p + i, 0);
                }
                if symbol == "_in6addr_loopback" {
                    mem.write(p + 15, 1u8);
                }
                mem.write(ptr_ptr, p.cast().cast_const());
                continue;
            }

            if symbol == "_NDR_record" {
                let p: MutPtr<u8> = mem.alloc(12).cast();
                for i in 0..12 {
                    mem.write(p + i, 0);
                }
                mem.write(ptr_ptr, p.cast().cast_const());
                continue;
            }

            // C++ Itanium ABI type_info vtables. Same handling as the
            // external-relocation loop above: without these, every
            // `__cxxabivN...__*_type_infoE` reference referenced through the
            // `__nl_symbol_ptr` table is left NULL, so any virtual call on a
            // type_info object (dynamic_cast, exception type matching) lands
            // on address 0 and crashes.
            if symbol == "__ZTVN10__cxxabiv117__class_type_infoE"
                || symbol == "__ZTVN10__cxxabiv120__si_class_type_infoE"
                || symbol == "__ZTVN10__cxxabiv121__vmi_class_type_infoE"
                || symbol == "__ZTVN10__cxxabiv119__pointer_type_infoE"
                || symbol == "__ZTVN10__cxxabiv120__function_type_infoE"
                || symbol == "__ZTVN10__cxxabiv116__enum_type_infoE"
                || symbol == "__ZTVN10__cxxabiv117__pbase_type_infoE"
                || symbol == "__ZTVN10__cxxabiv129__pointer_to_member_type_infoE"
            {
                let target = link_cxxabi_vtable(
                    symbol,
                    &mut cxxabi_vtable_addrs,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                    mem,
                );
                // A non-lazy symbol pointer holds the plain address of the
                // named symbol (no addend), so it resolves to the vtable base.
                mem.write(ptr_ptr, target);
                log_dbg!("Stubbed C++ vtable {} -> {:#x}", symbol, target.to_bits());
                continue;
            }

            // C++ RTTI type_info objects for fundamental types (double,
            // float, int, long, unsigned int, short, char, void, bool,
            // const char*, char*, void*, const void*). Without these the
            // Swift runtime symbols (`__swift_retain`, `__T0SSN` type
            // metadata, `__T0*Ma` accessors, …). Swift binaries (e.g. Alto's
            // Adventure) reference these from `__nl_symbol_ptr`; leaving them
            // NULL kills the app during Swift initialization.
            if let Some(link) = self.swift_intercept(mem, symbol) {
                let target = match link {
                    swift_runtime::SwiftLink::Function(f) => f.to_ptr(),
                    swift_runtime::SwiftLink::Data(slot) => {
                        crate::mem::Ptr::from_bits(slot.to_bits())
                    }
                };
                mem.write(ptr_ptr, target.cast());
                log_dbg!("Linked Swift runtime symbol {} at {:#x}", symbol, target.to_bits());
                continue;
            }

            // referenced type_info object has a NULL vptr; the first call
            // through it (dynamic_cast / exception type matching / the
            // `typeid(...)` comparison libstdc++ does for `const char*`)
            // branches to 0x0 and crashes. This mirrors the external-
            // relocation loop: the bundled libstdc++ in iOS 8 apps emits
            // `__ZTIPKc` (typeinfo for `char const*`) into __nl_symbol_ptr,
            // which previously fell through to the unhandled warning.
            if symbol == "__ZTId"
                || symbol == "__ZTIf"
                || symbol == "__ZTIi"
                || symbol == "__ZTIl"
                || symbol == "__ZTIj"
                || symbol == "__ZTIs"
                || symbol == "__ZTIc"
                || symbol == "__ZTIv"
                || symbol == "__ZTIb"
                || symbol == "__ZTIPKc"
                || symbol == "__ZTIPc"
                || symbol == "__ZTIPv"
                || symbol == "__ZTIPKv"
                || symbol == "__ZTIw"
                || symbol == "__ZTIPw"
                || symbol == "__ZTIPKw"
                || symbol == "__ZTIw"
                || symbol == "__ZTIPw"
                || symbol == "__ZTIPKw"
            {
                let ti = link_cxxabi_typeinfo(
                    symbol,
                    &mut cxxabi_vtable_addrs,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                    mem,
                );
                mem.write(ptr_ptr, ti);
                log_dbg!("Stubbed C++ typeinfo {} at {:#x}", symbol, ti.to_bits());
                continue;
            }

            log!(
                "Warning: unhandled non-lazy symbol {:?} at {:?} in \"{}\"",
                symbol,
                ptr_ptr,
                bin.name
            );
        }

        // FIXME: check for internal relocations?
    }

    /// Do linking that can only be done once there is a full [Environment].
    /// Not to be confused with lazy linking.
    pub fn do_late_linking(env: &mut Environment) {
        // TODO: do symbols ever appear in __nl_symbol_ptr multiple times?
        let to_link = std::mem::take(&mut env.dyld.constants_to_link_later);
        for (symbol_ptr_ptr, symbol) in to_link {
            let symbol_ptr = Self::constant_address(env, symbol).unwrap();
            env.mem.write(symbol_ptr_ptr, symbol_ptr.cast());
        }
    }

    /// Return the address of an exported data symbol, sharing its storage
    /// between normal linking and dlsym. A pointer-valued constant needs a
    /// slot containing the pointer, not the pointee's address.
    pub(crate) fn constant_address(env: &mut Environment, symbol: &str) -> Option<ConstVoidPtr> {
        if let Some(&address) = env.dyld.host_constant_addresses.get(symbol) {
            return Some(address);
        }
        let &(symbol, ref template) = search_host_dylibs(|dylib| dylib.constant_exports, symbol)?;
        let address = match template {
            HostConstant::NSString(value) => {
                let value = ns_string::get_static_str(env, value);
                env.mem.alloc_and_write(value).cast().cast_const()
            }
            HostConstant::NullPtr => env
                .mem
                .alloc_and_write(ConstVoidPtr::null())
                .cast()
                .cast_const(),
            HostConstant::Custom(create) => create(env),
        };
        env.dyld.host_constant_addresses.insert(symbol, address);
        Some(address)
    }

    /// Return a host function that can be called to handle an SVC instruction
    /// encountered during CPU emulation. If `None` is returned, the execution
    /// needs to resume at `svc_pc`.
    pub fn get_svc_handler(
        &mut self,
        bins: &[MachO],
        mem: &mut Mem,
        cpu: &mut Cpu,
        svc_pc: u32,
        svc: u32,
    ) -> Option<HostFunction> {
        match svc {
            Self::SVC_LAZY_LINK | Self::SVC_LAZY_LINK_RET_FLAG => {
                self.do_lazy_link(bins, mem, cpu, svc_pc)
            }
            Self::SVC_THREAD_EXIT | Self::SVC_RETURN_TO_HOST => {
                // These are handled before this dispatch; reaching here
                // would indicate the SVC numbering scheme is corrupted.
                log!(
                    "Warning: Dyld::get_svc_handler received SVC #{} (thread exit / return to host) at {:#x}; this should be handled earlier.",
                    svc, svc_pc
                );
                None
            }
            Self::SVC_LINKED_FUNCTIONS_BASE.. => {
                let f = self.linked_host_functions.get(
                    ((svc & !Self::SVC_LAZY_LINK_RET_FLAG) - Self::SVC_LINKED_FUNCTIONS_BASE)
                        as usize,
                );
                let Some(&(symbol, f)) = f else {
                    // No stub of ours can encode this id, so the guest itself
                    // executed the instruction (a raw `svc` in app code, or a
                    // legacy trap like `svc #0xab` from pre-armv7 stubs).
                    // There is nothing to call, and re-executing the SVC would
                    // trap forever, so report it (rate-limited) and let the
                    // caller skip the instruction.
                    let count = self.stray_svc_reports.entry((svc_pc, svc)).or_insert(0);
                    *count += 1;
                    if *count <= 2 {
                        log!(
                            "Warning: unexpected SVC #{} at {:#x} (not a dyld-linked function); skipping it.",
                            svc,
                            svc_pc
                        );
                        if *count == 2 {
                            log!(
                                "(further occurrences of SVC #{} at {:#x} will not be logged)",
                                svc,
                                svc_pc
                            );
                        }
                    }
                    return None;
                };
                log_dbg!("Call to host function, already linked: {}", symbol);
                // Record the symbol for nil-page-write diagnostics (see
                // mem::null_check_fail), which otherwise only know that the
                // write came from the generic bytes_at_mut accessor.
                std::sync::atomic::AtomicUsize::store(
                    &crate::environment::LAST_HOST_CALL_SYMBOL_PTR,
                    symbol.as_ptr() as usize,
                    std::sync::atomic::Ordering::Relaxed,
                );
                std::sync::atomic::AtomicUsize::store(
                    &crate::environment::LAST_HOST_CALL_SYMBOL_LEN,
                    symbol.len(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                if TRACE_HOST_CALLS.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                    TRACE_HOST_CALLS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    log!("HOSTCALL {} lr={:#x}", symbol, cpu.regs()[14]);
                }
                Some(f)
            }
        }
    }

    fn do_lazy_link(
        &mut self,
        bins: &[MachO],
        mem: &mut Mem,
        cpu: &mut Cpu,
        svc_pc: u32,
    ) -> Option<HostFunction> {
        // Links by restoring the original stub function, then updating
        // __la_symbol_ptr to the appropriate function.
        fn link_by_restoring_stub(
            mem: &mut Mem,
            cpu: &mut Cpu,
            linked_function: u32,
            svc_pc: u32,
            entry_size: u32,
            pic_offset: u32,
        ) -> (MutPtr<u32>, MutPtr<u32>) {
            let original_instructions: &[u32] = match entry_size {
                4 => Dyld::SYMBOL_STUB1_INSTRUCTIONS.as_slice(),
                12 => Dyld::SYMBOL_STUB_INSTRUCTIONS.as_slice(),
                16 => Dyld::PIC_SYMBOL_STUB_INSTRUCTIONS.as_slice(),
                other => {
                    // setup_lazy_linking already filters out unsupported
                    // sizes; if we reach here something is badly out of
                    // sync. Treat as a 12-byte normal stub to keep things
                    // moving rather than aborting the host.
                    log!(
                        "Warning: link_by_restoring_stub: unsupported entry size {}; falling back to 12-byte stub.",
                        other
                    );
                    Dyld::SYMBOL_STUB_INSTRUCTIONS.as_slice()
                }
            };
            let instruction_count: GuestUSize = original_instructions.len().try_into().unwrap();

            // Restore the original stub, which calls the __la_symbol_ptr
            let stub_function_ptr: MutPtr<u32> = Ptr::from_bits(svc_pc);
            if entry_size == 4 {
                mem.write(stub_function_ptr, original_instructions[0] | pic_offset)
            } else {
                for (i, &instr) in original_instructions.iter().enumerate() {
                    mem.write(stub_function_ptr + i.try_into().unwrap(), instr)
                }
            }

            cpu.invalidate_cache_range(stub_function_ptr.to_bits(), instruction_count * 4);
            // Update the __la_symbol_ptr
            let la_symbol_ptr: MutPtr<u32> = if entry_size == 12 {
                // Normal stub: absolute address
                let addr = mem.read(stub_function_ptr + instruction_count);
                Ptr::from_bits(addr)
            } else {
                // The PIC (position-independent code) stub uses a
                // PC-relative offset rather than an absolute address.
                if entry_size == 4 {
                    let offset = mem.read(stub_function_ptr) & 0xFFF;
                    Ptr::from_bits(stub_function_ptr.to_bits() + offset + 8)
                } else {
                    let offset = mem.read(stub_function_ptr + instruction_count);
                    Ptr::from_bits(stub_function_ptr.to_bits() + offset + 12)
                }
            };
            mem.write(la_symbol_ptr, linked_function);
            (stub_function_ptr, la_symbol_ptr)
        }

        let Some((stub_bin, stubs, pic_offset, offset)) = bins.iter().find_map(|bin| {
            let stubs = bin.get_section(SectionType::SymbolStubs)?;
            let offset = lazy_stub_section_offset(stubs.addr, stubs.size, svc_pc)?;
            let pic_offset = bin
                .get_section(SectionType::LazySymbolPointers)
                .map_or(0, |lazy_ptrs| lazy_ptrs.addr - stubs.addr);
            Some((bin, stubs, pic_offset, offset))
        }) else {
            return fallback_for_unmapped_lazy_link(
                cpu,
                svc_pc,
                "address is outside loaded symbol stub sections",
            );
        };
        let Some(info) = stubs.dyld_indirect_symbol_info.as_ref() else {
            return fallback_for_unmapped_lazy_link(
                cpu,
                svc_pc,
                "symbol stub section has no indirect-symbol metadata",
            );
        };
        if !matches!(info.entry_size, 4 | 12 | 16) {
            return fallback_for_unmapped_lazy_link(
                cpu,
                svc_pc,
                "symbol stub section has an unsupported entry size",
            );
        }
        if !offset.is_multiple_of(info.entry_size) {
            return fallback_for_unmapped_lazy_link(
                cpu,
                svc_pc,
                "SVC address is not aligned to a symbol stub",
            );
        }
        let idx = (offset / info.entry_size) as usize;
        let Some(symbol) = info
            .indirect_undef_symbols
            .get(idx)
            .and_then(|symbol| symbol.as_deref())
        else {
            return fallback_for_unmapped_lazy_link(
                cpu,
                svc_pc,
                "symbol stub has no indirect undefined symbol",
            );
        };

        if self.guest_sjlj_runtime_available && is_guest_cxxabi_symbol(symbol) {
            if let Some(addr) = self.guest_cxxabi_export(symbol) {
                register_cxxabi_typeinfo_vtable(
                    symbol,
                    addr,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                );
                let (stub_function_ptr, la_symbol_ptr) = link_by_restoring_stub(
                    mem,
                    cpu,
                    addr,
                    svc_pc,
                    info.entry_size,
                    pic_offset,
                );
                log!(
                    "Linked guest C++ ABI symbol {} from a bundled dylib at {:?}/{:?}",
                    symbol,
                    stub_function_ptr,
                    la_symbol_ptr
                );
                return None;
            }
        }

        if let Some(&addr) = self.non_lazy_host_functions.get(symbol) {
            // The host function was already linked non-lazily, point the
            // stub and __la_symbol_ptr to the function.
            let (stub_function_ptr, la_symbol_ptr) = link_by_restoring_stub(
                mem,
                cpu,
                addr.addr_with_thumb_bit(),
                svc_pc,
                info.entry_size,
                pic_offset,
            );
            log_dbg!(
                "Linked host function {} at {:?}/{:?} to existing stub ({:?}).",
                symbol,
                stub_function_ptr,
                la_symbol_ptr,
                addr,
            );
            // The stub jumps to the non-lazy function, which calls the
            // host function.
            return None;
        }

        // Intercept C++ exception ABI symbols (e.g. `__cxa_throw`) before the
        // guest dylibs: letting a real throw reach the guest unwinder ends in
        // `std::terminate` → guest `exit(0)` (no unwinder on our side).
        if let Some(stub) = self.cxxabi_intercept(mem, symbol) {
            let (stub_function_ptr, la_symbol_ptr) = link_by_restoring_stub(
                mem,
                cpu,
                stub.addr_with_thumb_bit(),
                svc_pc,
                info.entry_size,
                pic_offset,
            );
            log!(
                "Intercepted guest C++ exception ABI symbol {} -> host stub ({:?}/{:?})",
                symbol,
                stub_function_ptr,
                la_symbol_ptr
            );
            // Restart execution at the (now rewritten) stub.
            return None;
        }

        // Prefer guest dylibs (libstdc++.6.dylib, libgcc_s.1.dylib, …) over
        // host dylib stubs. Apps that bundle their own libstdc++ rely on the
        // proper guest C++ ABI (`__cxa_throw`, `__cxa_begin_catch`, the SjLj
        // unwinder, …) reaching the actual library — if a partial host stub
        // wins the lookup, exception handling silently breaks. This mirrors
        // what `do_non_lazy_linking` already does for non-lazy bindings, and
        // matches iOS dyld's behaviour of preferring the app's own linked
        // dylibs over fallback implementations.
        for dylib in bins.iter() {
            if let Some(&addr) = dylib.external_symbols.get(symbol) {
                register_cxxabi_typeinfo_vtable(
                    symbol,
                    addr,
                    &mut self.cxxabi_typeinfo_vtable_kinds,
                );
                let (stub_function_ptr, la_symbol_ptr) =
                    link_by_restoring_stub(mem, cpu, addr, svc_pc, info.entry_size, pic_offset);
                log_dbg!(
                    "Linked {} at {:?}/{:?} to {:#x} from {}",
                    symbol,
                    stub_function_ptr,
                    la_symbol_ptr,
                    addr,
                    dylib.name
                );
                // Tell the caller it needs to restart execution at svc_pc.
                return None;
            }
        }

        if let Some(&(symbol, f)) = search_host_dylibs(|dylib| dylib.function_exports, symbol) {
            // Allocate an SVC ID for this host function
            let idx: u32 = self.linked_host_functions.len().try_into().unwrap();
            let mut svc = idx + Self::SVC_LINKED_FUNCTIONS_BASE;
            // Indicate to the handler to return manually after call
            if info.entry_size == 4 {
                assert!(svc < Self::SVC_LAZY_LINK_RET_FLAG);
                svc |= Self::SVC_LAZY_LINK_RET_FLAG;
            }
            self.linked_host_functions.push((symbol, f));
            // Rewrite stub function to call this host function
            let stub_function_ptr: MutPtr<u32> = Ptr::from_bits(svc_pc);
            mem.write(stub_function_ptr, encode_a32_svc(svc));
            if info.entry_size != 4 {
                assert!(mem.read(stub_function_ptr + 1) == encode_a32_ret());
            }

            cpu.invalidate_cache_range(stub_function_ptr.to_bits(), 4);
            log_dbg!(
                "Linked {} at {:?} to host implementation",
                symbol,
                stub_function_ptr
            );
            // Return the host function so that we can call it now that we're
            // done.
            return Some(f);
        }

        if let Some(addr) = local_code_symbol(stub_bin, symbol) {
            let (stub_function_ptr, la_symbol_ptr) =
                link_by_restoring_stub(mem, cpu, addr, svc_pc, info.entry_size, pic_offset);
            log!(
                "Linked same-image private function {} at {:?}/{:?} to {:#x} from {}",
                symbol,
                stub_function_ptr,
                la_symbol_ptr,
                addr,
                stub_bin.name
            );
            return None;
        }

        // Fallback: the symbol isn't implemented by any host dylib and isn't
        // exported by any loaded guest dylib. Instead of panicking (which kills
        // the app), install a stub that logs a warning and returns 0. This
        // lets the emulator keep running for relatively harmless symbols like
        // `_getuid`, `_geteuid`, `_getpid`, etc.
        let symbol_kind = if is_gles_function_symbol(symbol) {
            "GLES function"
        } else {
            "function"
        };
        log!(
            "Warning: call to unimplemented {} {} at {:#x}; installing return-0 stub",
            symbol_kind,
            symbol,
            svc_pc
        );
        // `linked_host_functions` requires a `&'static str`, so leak the name.
        let leaked_symbol: &'static str = Box::leak(symbol.to_string().into_boxed_str());
        let f: HostFunction = &(unimplemented_function_stub as fn(&mut Environment) -> i32);
        // Allocate an SVC ID for this stub (same pattern as the host-dylib
        // branch above).
        let idx: u32 = self.linked_host_functions.len().try_into().unwrap();
        let mut svc = idx + Self::SVC_LINKED_FUNCTIONS_BASE;
        if info.entry_size == 4 {
            assert!(svc < Self::SVC_LAZY_LINK_RET_FLAG);
            svc |= Self::SVC_LAZY_LINK_RET_FLAG;
        }
        self.linked_host_functions.push((leaked_symbol, f));
        // Rewrite the stub function to trap into our SVC handler.
        let stub_function_ptr: MutPtr<u32> = Ptr::from_bits(svc_pc);
        mem.write(stub_function_ptr, encode_a32_svc(svc));
        if info.entry_size != 4 {
            assert!(mem.read(stub_function_ptr + 1) == encode_a32_ret());
        }
        cpu.invalidate_cache_range(stub_function_ptr.to_bits(), 4);
        Some(f)
    }

    /// Creates a guest function that will call a host function with the name
    /// `symbol`. This can be used to implement "get proc address" functions.
    /// Note that no attempt is made to deduplicate or deallocate these, so
    /// excessive use would create a memory leak.
    ///
    /// The name must be the mangled symbol name. Returns [Err] if there's no
    /// such function.
    pub fn create_proc_address(
        &mut self,
        mem: &mut Mem,
        cpu: &mut Cpu,
        symbol: &str,
    ) -> Result<GuestFunction, ()> {
        let function_ptr = self.create_proc_address_no_inval(mem, symbol)?;
        // Just in case
        cpu.invalidate_cache_range(function_ptr.addr_without_thumb_bit(), 8);
        Ok(function_ptr)
    }

    /// Internal [Self::create_proc_address] that doesn't invalidate the cache.
    /// For use before a [Cpu] is available.
    pub(crate) fn create_proc_address_no_inval(
        &mut self,
        mem: &mut Mem,
        symbol: &str,
    ) -> Result<GuestFunction, ()> {
        // Нативно обрабатываем рудимент ленивой загрузки Apple:
        if symbol == "dyld_stub_binder" || symbol == "_dyld_stub_binder" {
            // Используем "dyld_stub_binder" (это &'static str), а не переменную
            // symbol
            let symbol_name = "dyld_stub_binder";
            if let Some(&cached_fn) = self.non_lazy_host_functions.get(symbol_name) {
                return Ok(cached_fn);
            }

            let (_, f) = export_c_func!(dyld_stub_binder(_));
            // Передаем именно статическую строку
            let function_ptr = self.create_guest_function(mem, symbol_name, f);
            self.non_lazy_host_functions
                .insert(symbol_name, function_ptr);
            return Ok(function_ptr);
        }

        if let Some(stub) = self.cxxabi_intercept(mem, symbol) {
            return Ok(stub);
        }

        // `dlsym`-style lookups must not hand out a host C++ ABI stub when the
        // app brought its own working SjLj runtime: the host can only fake a
        // non-local return, which makes `throw` skip the guest's catch block
        // and re-throw forever. Return the real guest implementation instead.
        if let Some(addr) = self.guest_cxxabi_export(symbol) {
            return Ok(GuestFunction::from_addr_with_thumb_bit(addr));
        }

        let &(symbol, f) = search_host_dylibs(|dylib| dylib.function_exports, symbol).ok_or(())?;
        if let Some(&cached_fn) = self.non_lazy_host_functions.get(symbol) {
            return Ok(cached_fn);
        }
        let function_ptr = self.create_guest_function(mem, symbol, f);
        self.non_lazy_host_functions.insert(symbol, function_ptr);
        Ok(function_ptr)
    }

    pub fn create_guest_function(
        &mut self,
        mem: &mut Mem,
        symbol: &'static str,
        f: HostFunction,
    ) -> GuestFunction {
        // Allocate an SVC ID for this host function
        let idx: u32 = self.linked_host_functions.len().try_into().unwrap();
        let svc = idx + Self::SVC_LINKED_FUNCTIONS_BASE;
        self.linked_host_functions.push((symbol, f));

        // Create guest function to call this host function
        let function_ptr = mem.alloc(8);
        let function_ptr: MutPtr<u32> = function_ptr.cast();
        mem.write(function_ptr + 0, encode_a32_svc(svc));
        mem.write(function_ptr + 1, encode_a32_ret());
        // Keep address-to-symbol mappings available when dyld debugging is enabled.
        log_dbg!("host fn stub {} at {:#x}", symbol, function_ptr.to_bits());
        GuestFunction::from_addr_with_thumb_bit(function_ptr.to_bits())
    }

    /// Intercepts guest C++ ABI symbols that require host-side behavior.
    fn cxxabi_intercept(&mut self, mem: &mut Mem, name: &str) -> Option<GuestFunction> {
        let sym = cxxabi_intercept_symbol(name, self.guest_sjlj_runtime_available)?;
        if let Some(&cached) = self.non_lazy_host_functions.get(sym) {
            return Some(cached);
        }
        let f = if sym == "___dynamic_cast" {
            // Guest libstdc++ uses type_info vtable methods that touchHLE stubs.
            // Route casts through the host RTTI walker even when guest SjLj is present.
            let (_, f) = *search_host_dylibs(|dylib| dylib.function_exports, sym)?;
            f
        } else {
            let (_, f) = export_c_func!(cxxabi_throw_intercept(_, _, _));
            f
        };
        let function_ptr = self.create_guest_function(mem, sym, f);
        self.non_lazy_host_functions.insert(sym, function_ptr);
        Some(function_ptr)
    }
}

/// Host-side intercept for `__cxa_throw` (Itanium C++ ABI).
///
/// Direct `throw` statements in app/libstdc++ code call `__cxa_throw`, which
/// normally starts real unwinding. touchHLE has no unwinder: the SjLj/LSDA
/// walk ends in `std::terminate` → guest `exit(0)` (observed with CSR Racing:
/// PlayHaven init threw, unwinder bailed, app exited during startup).
///
/// Instead of letting the throw reach the unwinder, first use the shared,
/// validated frame-pointer recovery to return to an app caller. If there is no
/// safe frame, retain the old no-op return as a last resort. The log line makes
/// the root cause of each suppressed exception visible.
fn cxxabi_throw_intercept(
    env: &mut Environment,
    exception: MutVoidPtr,
    tinfo: MutVoidPtr,
    _dest: MutVoidPtr,
) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static LOGGED: AtomicU32 = AtomicU32::new(0);

    // type_info layout (Itanium, 32-bit): +0 vptr, +4 `char const* name`.
    let type_name = if !tinfo.is_null() {
        let type_info: ConstPtr<ConstPtr<u8>> = tinfo.cast_const().cast();
        let name_ptr: ConstPtr<u8> = env.mem.read(type_info + 1);
        read_printable_guest_string(env, name_ptr, 64)
    } else {
        String::new()
    };

    // Best-effort `what()`: for `std::runtime_error`-style exceptions the
    // user object is `[vptr, std::string]`; COW std::string holds a pointer
    // to its rep at +4, and the character data lives at rep + 12.
    let what = if !exception.is_null() {
        let rep: ConstPtr<u8> = env.mem.read(exception.cast());
        if !rep.is_null() {
            read_printable_guest_string(env, unsafe { rep + 12 }, 96)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let continuation = crate::libc::cxxabi::unwind_to_app_frame(env);
    let n = LOGGED.fetch_add(1, Ordering::Relaxed);
    if n < 8 {
        if let Some(continuation) = continuation {
            log!(
                "Suppressed guest C++ exception (no unwinder): type={} what={} at \
                 exception={:?}; unwound to app frame {:#010x}.",
                if type_name.is_empty() { "?" } else { &type_name[..] },
                if what.is_empty() { "?" } else { &what[..] },
                exception,
                continuation.addr_with_thumb_bit()
            );
        } else {
            log!(
                "Suppressed guest C++ exception (no unwinder): type={} what={} at \
                 exception={:?}; no safe frame, returning after the throw point.",
                if type_name.is_empty() { "?" } else { &type_name[..] },
                if what.is_empty() { "?" } else { &what[..] },
                exception
            );
        }
    }
}

/// Reads a NUL-terminated guest string defensively, keeping only printable
/// ASCII. Never panics on NULL/garbage pointers (null-page reads are handled
/// by `Mem::bytes_at`, garbage yields an empty string).
fn read_printable_guest_string(env: &Environment, ptr: ConstPtr<u8>, max: usize) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let bytes = env.mem.bytes_at(ptr, max as GuestUSize);
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end]
        .iter()
        .filter(|&&b| (0x20..0x7f).contains(&b))
        .map(|&b| b as char)
        .collect()
}

fn dyld_stub_binder(_env: &mut Environment, _arg: u32) {
    // Under HLE we eagerly bind every lazy symbol at link time, so the
    // dyld_stub_binder should never be invoked. If a guest binary still
    // jumps here (e.g. through a hand-rolled lazy-binding scheme), log it
    // instead of crashing the emulator; the caller will simply see a
    // no-op return.
    log!(
        "Warning: dyld_stub_binder was called! Under HLE all lazy symbols are bound eagerly, so this is unexpected. Continuing as a no-op."
    );
}

fn is_gles_function_symbol(symbol: &str) -> bool {
    let symbol = symbol.trim_start_matches('_');
    symbol
        .strip_prefix("gl")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|character| character.is_ascii_uppercase())
}

/// Generic fallback stub for functions referenced by the guest binary but
/// not implemented by any host dylib. Returns 0 so that calls to things like
/// `getuid`, `geteuid`, `getpid`, etc. don't panic the emulator.
///
/// On ARM32 the return value is placed in `r0`, which matches the C ABI for
/// `int`/`uid_t`/`pid_t`/pointer return types.
fn unimplemented_function_stub(_env: &mut Environment) -> i32 {
    0
}

fn local_code_symbol(bin: &MachO, symbol: &str) -> Option<u32> {
    if bin.external_symbols.contains_key(symbol) {
        return None;
    }
    let &address = bin.exported_symbols.get(symbol)?;
    let address_without_thumb_bit = address & !GuestFunction::THUMB_BIT;
    bin.sections
        .iter()
        .any(|section| {
            matches!(
                section.name.as_str(),
                "__text" | "__textcoal" | "__textcoal_nt"
            ) && address_without_thumb_bit
                .checked_sub(section.addr)
                .is_some_and(|offset| offset < section.size)
        })
        .then_some(address)
}

fn lazy_stub_section_offset(section_addr: u32, section_size: u32, svc_pc: u32) -> Option<u32> {
    let offset = svc_pc.checked_sub(section_addr)?;
    (offset < section_size).then_some(offset)
}

fn fallback_for_unmapped_lazy_link(
    cpu: &mut Cpu,
    svc_pc: u32,
    reason: &str,
) -> Option<HostFunction> {
    let lr = cpu.regs()[Cpu::LR];
    log_once!("Warning: lazy-link SVC could not be matched to a symbol stub; returning 0 to the caller.");
    log_dbg!(
        "Unmapped lazy-link SVC at {:#x} ({}); returning to LR {:#x}.",
        svc_pc,
        reason,
        lr
    );
    cpu.branch(GuestFunction::from_addr_with_thumb_bit(lr));
    Some(&(unimplemented_function_stub as fn(&mut Environment) -> i32))
}

fn has_guest_sjlj_runtime(mut has_symbol: impl FnMut(&str) -> bool) -> bool {
    has_symbol("___cxa_throw")
        && has_symbol("___gxx_personality_sj0")
        && has_symbol("__Unwind_SjLj_RaiseException")
        && has_symbol("__Unwind_SjLj_Register")
}

fn is_guest_cxxabi_symbol(name: &str) -> bool {
    name == "__ZSt9terminatev"
        || name.starts_with("___cxa_")
        || name.starts_with("__Unwind_")
        || name.starts_with("___gxx_personality_")
        || name.starts_with("__ZTVN10__cxxabiv1")
        || name.starts_with("__ZTI")
        || name.starts_with("__ZTS")
}

fn cxxabi_intercept_symbol(name: &str, guest_sjlj_runtime_available: bool) -> Option<&'static str> {
    match name {
        "___dynamic_cast" | "__dynamic_cast" => Some("___dynamic_cast"),
        "__cxa_throw" if !guest_sjlj_runtime_available => Some("__cxa_throw"),
        "___cxa_throw" if !guest_sjlj_runtime_available => Some("___cxa_throw"),
        _ => None,
    }
}

#[cfg(test)]
mod gles_symbol_detection_tests {
    use super::is_gles_function_symbol;

    #[test]
    fn recognizes_mangled_and_unmangled_gles_entry_points() {
        for symbol in [
            "_glBindBuffer",
            "glGetAttachedShaders",
            "_glDiscardFramebufferEXT",
        ] {
            assert!(is_gles_function_symbol(symbol));
        }
    }

    #[test]
    fn ignores_non_gles_symbols() {
        for symbol in ["_getuid", "_global_init", "_GLESContext", "_gl_lowercase"] {
            assert!(!is_gles_function_symbol(symbol));
        }
    }
}

#[cfg(test)]
mod cxxabi_runtime_detection_tests {
    use super::{
        cxxabi_intercept_symbol, cxxabi_typeinfo_vtable_kind, has_guest_sjlj_runtime,
        is_guest_cxxabi_symbol, register_cxxabi_typeinfo_vtable, CxxAbiTypeInfoKind,
    };
    use std::collections::HashMap;

    const REQUIRED_SYMBOLS: [&str; 4] = [
        "___cxa_throw",
        "___gxx_personality_sj0",
        "__Unwind_SjLj_RaiseException",
        "__Unwind_SjLj_Register",
    ];

    #[test]
    fn guest_sjlj_runtime_requires_throw_personality_and_unwinder() {
        assert!(has_guest_sjlj_runtime(|symbol| {
            REQUIRED_SYMBOLS.contains(&symbol)
        }));
        for missing in REQUIRED_SYMBOLS {
            assert!(!has_guest_sjlj_runtime(|symbol| {
                REQUIRED_SYMBOLS
                    .iter()
                    .any(|candidate| *candidate != missing && *candidate == symbol)
            }));
        }
    }

    #[test]
    fn dynamic_cast_uses_the_host_rtti_walker_with_a_guest_runtime() {
        assert!(!is_guest_cxxabi_symbol("___dynamic_cast"));
        assert_eq!(
            cxxabi_intercept_symbol("___dynamic_cast", true),
            Some("___dynamic_cast")
        );
        assert_eq!(
            cxxabi_intercept_symbol("__dynamic_cast", true),
            Some("___dynamic_cast")
        );
        assert_eq!(cxxabi_intercept_symbol("___cxa_throw", true), None);
    }

    #[test]
    fn cxxabi_typeinfo_vtable_names_match_the_rtti_walker_kinds() {
        assert_eq!(
            cxxabi_typeinfo_vtable_kind("__ZTVN10__cxxabiv117__class_type_infoE"),
            Some(CxxAbiTypeInfoKind::Class)
        );
        assert_eq!(
            cxxabi_typeinfo_vtable_kind("__ZTVN10__cxxabiv120__si_class_type_infoE"),
            Some(CxxAbiTypeInfoKind::SingleInheritance)
        );
        assert_eq!(
            cxxabi_typeinfo_vtable_kind("__ZTVN10__cxxabiv121__vmi_class_type_infoE"),
            Some(CxxAbiTypeInfoKind::MultipleInheritance)
        );
        assert_eq!(
            cxxabi_typeinfo_vtable_kind("__ZTVsome_other_type_info"),
            None
        );
    }

    #[test]
    fn cxxabi_typeinfo_vtable_registration_covers_both_symbol_address_conventions() {
        let mut kinds = HashMap::new();
        register_cxxabi_typeinfo_vtable(
            "__ZTVN10__cxxabiv120__si_class_type_infoE",
            0x1000,
            &mut kinds,
        );
        assert_eq!(
            kinds.get(&0x1000),
            Some(&CxxAbiTypeInfoKind::SingleInheritance)
        );
        assert_eq!(
            kinds.get(&0x1008),
            Some(&CxxAbiTypeInfoKind::SingleInheritance)
        );
        assert_eq!(kinds.len(), 2);
        register_cxxabi_typeinfo_vtable(
            "__ZTVN10__cxxabiv120__si_class_type_infoE",
            u32::MAX,
            &mut kinds,
        );
        assert_eq!(
            kinds.get(&u32::MAX),
            Some(&CxxAbiTypeInfoKind::SingleInheritance)
        );
        assert_eq!(kinds.len(), 3);
        assert!(!kinds.contains_key(&0x7));
    }

    #[test]
    fn guest_cxxabi_symbol_matcher_covers_unwind_and_rtti_symbols() {
        use super::is_guest_cxxabi_symbol;

        for symbol in [
            "___cxa_throw",
            "__Unwind_SjLj_RaiseException",
            "___gxx_personality_sj0",
            "__ZTVN10__cxxabiv117__class_type_infoE",
            "__ZTIi",
            "__ZTSi",
        ] {
            assert!(is_guest_cxxabi_symbol(symbol));
        }
        assert!(!is_guest_cxxabi_symbol("___objc_msgSend"));
    }
}

#[cfg(test)]
mod lazy_link_recovery_tests {
    use super::{
        fallback_for_unmapped_lazy_link, lazy_stub_section_offset, local_code_symbol, Dyld,
    };
    use crate::cpu::Cpu;
    use crate::mach_o::{DyldIndirectSymbolInfo, MachO, Section, SectionType};
    use crate::mem::{Mem, MutPtr};
    use std::collections::HashMap;

    #[test]
    fn lazy_stub_section_offset_checks_boundaries_without_overflow() {
        assert_eq!(lazy_stub_section_offset(0x1000, 0x10, 0x1000), Some(0));
        assert_eq!(lazy_stub_section_offset(0x1000, 0x10, 0x100f), Some(0xf));
        assert_eq!(lazy_stub_section_offset(0x1000, 0x10, 0x1010), None);
        assert_eq!(lazy_stub_section_offset(0x1000, 0x10, 0x0fff), None);
        assert_eq!(
            lazy_stub_section_offset(u32::MAX - 7, 0x10, u32::MAX),
            Some(7)
        );
    }

    #[test]
    fn unmapped_lazy_link_returns_to_arm_or_thumb_link_register() {
        let mut cpu = Cpu::new(None);

        for (lr, expected_pc, expected_thumb) in [(0x1000, 0x1000, false), (0x2001, 0x2000, true)] {
            cpu.regs_mut()[Cpu::LR] = lr;
            assert!(fallback_for_unmapped_lazy_link(&mut cpu, 0x3000, "test").is_some());
            assert_eq!(cpu.regs()[Cpu::PC], expected_pc);
            assert_eq!(cpu.cpsr() & Cpu::CPSR_THUMB != 0, expected_thumb);
        }
    }

    fn mock_binary(symbol: &str, address: u32, section_name: &str, external: bool) -> MachO {
        let mut exported_symbols = HashMap::new();
        exported_symbols.insert(symbol.to_owned(), address);
        let mut external_symbols = HashMap::new();
        if external {
            external_symbols.insert(symbol.to_owned(), address);
        }

        MachO {
            name: "test".to_owned(),
            loaded_path: "/test".to_owned(),
            dynamic_libraries: Vec::new(),
            run_paths: Vec::new(),
            slide: 0,
            sections: vec![Section {
                name: section_name.to_owned(),
                segment_name: "__TEXT".to_owned(),
                addr: 0x1000,
                size: 0x100,
                type_: SectionType::Normal,
                dyld_indirect_symbol_info: None,
            }],
            exported_symbols,
            symbol_locations: HashMap::new(),
            external_symbols,
            external_relocations: Vec::new(),
            entry_point_pc: None,
            entry_point_is_lc_main: false,
            last_segment_end: 0x2000,
            text_base: 0x1000,
            mapped_segments: vec![0x1000..0x2000],
        }
    }

    #[test]
    fn same_image_private_code_symbols_are_resolved_without_exposing_data() {
        let symbol = "__ZN8TMapBaseIP9UPropertyi14TSIMDAllocatorE6RehashEv";
        let private_code = mock_binary(symbol, 0x1011, "__text", false);
        assert_eq!(local_code_symbol(&private_code, symbol), Some(0x1011));

        let private_data = mock_binary(symbol, 0x1011, "__const", false);
        assert_eq!(local_code_symbol(&private_data, symbol), None);

        let external_code = mock_binary(symbol, 0x1011, "__text", true);
        assert_eq!(local_code_symbol(&external_code, symbol), None);
        assert_eq!(local_code_symbol(&private_code, "__other"), None);
    }

    // A local/absolute entry has no symbol to bind. Rewriting its PIC stub
    // into an SVC loses the existing target and corrupts the return value.
    // https://github.com/apple-oss-distributions/dyld/blob/dyld-360.22/src/ImageLoaderMachOClassic.cpp
    #[test]
    fn lazy_linking_preserves_already_bound_pic_stubs() {
        let mut mem = Mem::new();
        let stubs: MutPtr<u32> = mem.alloc(32).cast();
        let mut original = Dyld::PIC_SYMBOL_STUB_INSTRUCTIONS.to_vec();
        original.push(0x2000);
        for stub_index in 0..2 {
            for (index, &instruction) in original.iter().enumerate() {
                mem.write(stubs + stub_index * 4 + index as u32, instruction);
            }
        }
        let mut bin = mock_binary("_unused", 0x1011, "__text", false);
        bin.sections = vec![Section {
            name: "__picsymbolstub4".to_owned(),
            segment_name: "__TEXT".to_owned(),
            addr: stubs.to_bits(),
            size: 32,
            type_: SectionType::SymbolStubs,
            dyld_indirect_symbol_info: Some(DyldIndirectSymbolInfo {
                entry_size: 16,
                indirect_undef_symbols: vec![None, Some("_objc_retain".to_owned())],
            }),
        }];

        Dyld::new().setup_lazy_linking(&bin, &mut mem, None);
        for (index, &instruction) in original.iter().enumerate() {
            assert_eq!(mem.read(stubs + index as u32), instruction);
        }
        assert_eq!(
            mem.read(stubs + 4),
            super::encode_a32_svc(Dyld::SVC_LAZY_LINK)
        );
        assert_eq!(mem.read(stubs + 5), super::encode_a32_ret());
        assert_eq!(mem.read(stubs + 7), original[3]);
    }

    // Exercise the original ARM stub instructions, including PC-relative
    // pointer loads. Binding must not change the code a checksum would read.
    // https://github.com/apple-oss-distributions/dyld/blob/dyld-210.2.3/src/ImageLoaderMachOClassic.cpp
    #[test]
    fn preserved_stubs_execute_through_bound_pointers() {
        for entry_size in [4, 12, 16] {
            let mut mem = Mem::new();
            mem.set_null_segment_size(0x1000);
            let code: MutPtr<u32> = mem.alloc(12).cast();
            mem.write(code, 0xe3a0002a); // MOV r0, #42
            mem.write(code + 1, super::encode_a32_ret());
            mem.write(code + 2, 0xe1200070); // BKPT
            let stub: MutPtr<u32> = mem.alloc(32).cast();
            let slot = stub + 6;
            let instructions = match entry_size {
                4 => vec![Dyld::SYMBOL_STUB1_INSTRUCTIONS[0] | 16],
                12 => vec![0xe59fc000, 0xe59cf000, slot.to_bits()],
                16 => vec![0xe59fc004, 0xe08fc00c, 0xe59cf000, 12],
                _ => unreachable!(),
            };
            for (index, &instruction) in instructions.iter().enumerate() {
                mem.write(stub + index as u32, instruction);
            }
            let mut bin = mock_binary("_stub_target", code.to_bits(), "__text", true);
            bin.sections = vec![Section {
                name: "__symbol_stubs".to_owned(),
                segment_name: "__TEXT".to_owned(),
                addr: stub.to_bits(),
                size: entry_size,
                type_: SectionType::SymbolStubs,
                dyld_indirect_symbol_info: Some(DyldIndirectSymbolInfo {
                    entry_size,
                    indirect_undef_symbols: vec![Some("_stub_target".to_owned())],
                }),
            }];
            // Compressed Mach-O binding can also describe this lazy slot as
            // an external relocation with addend zero. Initial linking must
            // not add the function address to an already bound pointer.
            bin.external_relocations
                .push((slot.to_bits(), "_stub_target".to_owned()));
            mem.write(slot, 0);
            let mut dyld = Dyld::new();
            dyld.do_initial_linking(
                &crate::bundle::Bundle::new_fake_bundle(),
                std::slice::from_ref(&bin),
                &mut mem,
                &mut crate::objc::ObjC::new(),
                true,
            );
            assert_eq!(mem.read(slot), code.to_bits());
            for (index, &instruction) in instructions.iter().enumerate() {
                assert_eq!(mem.read(stub + index as u32), instruction);
            }
            let mut cpu = Cpu::new(None);
            cpu.branch(crate::abi::GuestFunction::from_addr_with_thumb_bit(
                stub.to_bits(),
            ));
            cpu.regs_mut()[Cpu::LR] = (code + 2).to_bits();
            let state = cpu.run_or_step(&mut mem, Some(&mut 100));
            assert!(matches!(
                state,
                crate::cpu::CpuState::Error(crate::cpu::CpuError::Breakpoint)
            ));
            assert_eq!(cpu.regs()[0], 42);
        }
    }
}
