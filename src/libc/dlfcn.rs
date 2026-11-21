/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `dlfcn.h` (`dlopen()` and friends).
//!
//! Apple's iOS manual pages describe handles, symbol lookup and errors:
//! https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/dlopen.3.html
//! https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/dlsym.3.html
//! https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/dlerror.3.html

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::{Environment, ThreadId};
use std::collections::HashMap;

const RTLD_DEFAULT: MutVoidPtr = Ptr::from_bits(-2 as _);
const RTLD_NEXT: MutVoidPtr = Ptr::from_bits(-1 as _);
const RTLD_SELF: MutVoidPtr = Ptr::from_bits(-3 as _);
const RTLD_MAIN_ONLY: MutVoidPtr = Ptr::from_bits(-5 as _);
const RTLD_FIRST: i32 = 0x100;

struct LibraryHandle {
    path: String,
    first: bool,
    references: u32,
}

#[derive(Default)]
pub struct State {
    handles: HashMap<MutVoidPtr, LibraryHandle>,
    errors: HashMap<ThreadId, String>,
    returned_errors: HashMap<ThreadId, MutPtr<u8>>,
    image_paths: HashMap<usize, ConstPtr<u8>>,
    symbol_names: HashMap<(usize, String), ConstPtr<u8>>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DlInfo {
    filename: ConstPtr<u8>,
    base: ConstVoidPtr,
    symbol_name: ConstPtr<u8>,
    symbol_address: ConstVoidPtr,
}
unsafe impl SafeRead for DlInfo {}

/// Resolve an address to its image header and nearest defined symbol. Strings
/// belong to the loaded image and remain valid across subsequent lookups.
/// https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/dladdr.3.html
/// https://github.com/apple-oss-distributions/dyld/blob/dyld-210.2.3/src/dyldAPIs.cpp
fn dladdr(env: &mut Environment, address: ConstVoidPtr, info: MutPtr<DlInfo>) -> i32 {
    // Default to the historical failure stub. The real implementation is
    // opt-in because several legacy games hang when it starts resolving
    // their images and symbols.
    if !env.options.dladdr {
        return 0;
    }
    let address = address.to_bits();
    let Some(index) = env.bins.iter().position(|image| {
        image
            .mapped_segments
            .iter()
            .any(|range| range.contains(&address))
    }) else {
        return 0;
    };
    let image = &env.bins[index];
    let base = image.text_base;
    let symbol = if address == base {
        Some(("__dso_handle".to_owned(), base))
    } else {
        // Compare the raw nlist address, preserving bit zero on data symbols.
        // The returned function pointer still carries its recorded Thumb bit.
        image
            .exported_symbols
            .iter()
            .map(|(name, value)| {
                (
                    name,
                    *value,
                    image.symbol_locations.get(name).copied().unwrap_or(*value),
                )
            })
            .filter(|(_, _, location)| *location <= address)
            .max_by_key(|(name, _, location)| {
                (*location, image.external_symbols.contains_key(*name))
            })
            .filter(|(_, _, location)| *location != base)
            .map(|(name, value, _)| (name.strip_prefix('_').unwrap_or(name).to_owned(), value))
    };
    let filename = *env
        .libc_state
        .dlfcn
        .image_paths
        .entry(index)
        .or_insert_with(|| {
            env.mem
                .alloc_and_write_cstr(image.loaded_path.as_bytes())
                .cast_const()
        });
    let (symbol_name, symbol_address) = if let Some((name, value)) = symbol {
        let name = *env
            .libc_state
            .dlfcn
            .symbol_names
            .entry((index, name.clone()))
            .or_insert_with(|| env.mem.alloc_and_write_cstr(name.as_bytes()).cast_const());
        (name, Ptr::from_bits(value))
    } else {
        (Ptr::null(), Ptr::null())
    };
    env.mem.write(
        info,
        DlInfo {
            filename,
            base: Ptr::from_bits(base),
            symbol_name,
            symbol_address,
        },
    );
    log_dbg!("dladdr({address:#x}) => image {index}, base {base:#x}");
    1
}

fn error(env: &mut Environment, message: String) {
    log_dbg!("{}", message);
    env.libc_state
        .dlfcn
        .errors
        .insert(env.current_thread, message);
}

fn dlopen(env: &mut Environment, path: ConstPtr<u8>, mode: i32) -> MutVoidPtr {
    env.libc_state.dlfcn.errors.remove(&env.current_thread);
    if path.is_null() {
        return if mode & RTLD_FIRST != 0 {
            RTLD_MAIN_ONLY
        } else {
            RTLD_DEFAULT
        };
    }
    let Ok(path) = env.mem.cstr_at_utf8(path) else {
        error(env, "dlopen: invalid library path".to_string());
        return Ptr::null();
    };
    // Preserve the existing aggregate SQLite lookup until loading that system
    // library is implemented.
    if path.contains("sqlite3") {
        return RTLD_DEFAULT;
    }
    let canonical_path = crate::dyld::DYLIB_LIST
        .iter()
        .find(|library| library.path == path || library.aliases.contains(&path))
        .map(|library| library.path.to_string())
        .or_else(|| {
            env.bins
                .iter()
                .find(|image| {
                    image.loaded_path == path
                        || (!path.contains('/')
                            && image.loaded_path.rsplit('/').next() == Some(path))
                })
                .map(|image| image.loaded_path.clone())
        });
    let Some(path) = canonical_path else {
        error(env, format!("dlopen: library not loaded: {path}"));
        return Ptr::null();
    };
    let first = mode & RTLD_FIRST != 0;
    if let Some((&handle, library)) = env
        .libc_state
        .dlfcn
        .handles
        .iter_mut()
        .find(|(_, library)| library.path == path && library.first == first)
    {
        library.references += 1;
        return handle;
    }
    // The handle belongs to dyld, not to the caller's path allocation.
    let handle = env.mem.alloc(4);
    env.libc_state.dlfcn.handles.insert(
        handle,
        LibraryHandle {
            path,
            first,
            references: 1,
        },
    );
    handle
}

/// An image's lookup scope, including its loaded dependencies.
fn image_scope(env: &Environment, path: String, first: bool) -> Vec<String> {
    let mut paths = vec![path];
    let mut index = 0;
    while !first && index < paths.len() {
        if let Some(image) = env
            .bins
            .iter()
            .find(|image| image.loaded_path == paths[index])
        {
            for dependency in &image.dynamic_libraries {
                let dependency = crate::dyld::DYLIB_LIST
                    .iter()
                    .find(|library| {
                        library.path == dependency || library.aliases.contains(&dependency.as_str())
                    })
                    .map(|library| library.path.to_owned())
                    .unwrap_or_else(|| dependency.clone());
                if !paths.contains(&dependency) {
                    paths.push(dependency);
                }
            }
        }
        index += 1;
    }
    paths
}

fn dlsym(env: &mut Environment, handle: MutVoidPtr, symbol: ConstPtr<u8>) -> MutVoidPtr {
    // Apple's dyld clears the calling thread's previous error at entry.
    // https://github.com/apple-oss-distributions/dyld/blob/dyld-210.2.3/src/dyldAPIs.cpp
    env.libc_state.dlfcn.errors.remove(&env.current_thread);
    let paths = if handle == RTLD_MAIN_ONLY {
        env.bins
            .first()
            .map(|image| vec![image.loaded_path.clone()])
            .unwrap_or_default()
    } else if handle == RTLD_DEFAULT {
        env.bins
            .iter()
            .map(|image| image.loaded_path.clone())
            .chain(
                crate::dyld::DYLIB_LIST
                    .iter()
                    .map(|library| library.path.to_owned()),
            )
            .collect()
    } else if handle == RTLD_NEXT || handle == RTLD_SELF {
        let caller = env.cpu.regs()[crate::cpu::Cpu::LR] & !1;
        let Some(image) = env.bins.iter().find(|image| {
            image
                .mapped_segments
                .iter()
                .any(|range| range.contains(&caller))
        }) else {
            error(env, "dlsym: calling image not found".to_owned());
            return Ptr::null();
        };
        let mut paths = image_scope(env, image.loaded_path.clone(), false);
        if handle == RTLD_NEXT {
            paths.remove(0);
        }
        paths
    } else if let Some(library) = env.libc_state.dlfcn.handles.get(&handle) {
        image_scope(env, library.path.clone(), library.first)
    } else {
        error(env, format!("dlsym: invalid handle {handle:?}"));
        return Ptr::null();
    };
    if symbol.is_null() {
        error(env, "dlsym: NULL symbol name".to_string());
        return Ptr::null();
    }
    let Ok(symbol) = env.mem.cstr_at_utf8(symbol) else {
        error(env, "dlsym: invalid symbol name".to_string());
        return Ptr::null();
    };
    // Unlike Mach-O symbol names, dlsym names omit the leading underscore.
    let symbol = format!("_{symbol}");
    // Search the requested image before its dependencies. The loader's order
    // can put a dependency first, but that must not shadow the handle's image.
    for path in paths {
        if let Some(image) = env.bins.iter().find(|image| image.loaded_path == path) {
            if let Some(&address) = image.external_symbols.get(&symbol) {
                // MachO preserves Thumb bits on function symbols.
                return Ptr::from_bits(address);
            }
        }
        let Some(library) = crate::dyld::DYLIB_LIST
            .iter()
            .find(|library| library.path == path)
        else {
            continue;
        };
        if library
            .constant_exports
            .iter()
            .flat_map(|list| list.iter())
            .any(|(name, _)| *name == symbol)
        {
            return crate::dyld::Dyld::constant_address(env, &symbol)
                .unwrap()
                .cast_mut();
        }
        if library
            .function_exports
            .iter()
            .flat_map(|list| list.iter())
            .any(|(name, _)| *name == symbol)
        {
            if let Ok(address) = env
                .dyld
                .create_proc_address(&mut env.mem, &mut env.cpu, &symbol)
            {
                return Ptr::from_bits(address.addr_with_thumb_bit());
            }
        }
    }
    error(env, format!("dlsym: symbol not found: {symbol}"));
    Ptr::null()
}

fn dlclose(env: &mut Environment, handle: MutVoidPtr) -> i32 {
    if handle == RTLD_DEFAULT || handle == RTLD_MAIN_ONLY {
        return 0;
    }
    env.libc_state.dlfcn.errors.remove(&env.current_thread);
    let Some(library) = env.libc_state.dlfcn.handles.get_mut(&handle) else {
        error(env, format!("dlclose: invalid handle {handle:?}"));
        return -1;
    };
    library.references -= 1;
    if library.references == 0 {
        env.libc_state.dlfcn.handles.remove(&handle);
        env.mem.free(handle);
    }
    0
}

fn dlerror(env: &mut Environment) -> ConstPtr<u8> {
    if let Some(previous) = env
        .libc_state
        .dlfcn
        .returned_errors
        .remove(&env.current_thread)
    {
        env.mem.free(previous.cast());
    }
    let Some(message) = env.libc_state.dlfcn.errors.remove(&env.current_thread) else {
        return Ptr::null();
    };
    let pointer = env.mem.alloc_and_write_cstr(message.as_bytes());
    env.libc_state
        .dlfcn
        .returned_errors
        .insert(env.current_thread, pointer);
    pointer.cast_const()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(dladdr(_, _)),
    export_c_func!(dlopen(_, _)),
    export_c_func!(dlsym(_, _)),
    export_c_func!(dlclose(_)),
    export_c_func!(dlerror()),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mach_o::MachO;

    // The requested library must win over earlier-loaded dependencies, and
    // RTLD_SELF/NEXT must use the caller's dependency scope rather than every
    // loaded image. Exercise opaque handles and dlerror at the same boundary.
    // https://github.com/apple-oss-distributions/dyld/blob/dyld-210.2.3/src/dyldAPIs.cpp
    #[test]
    fn dynamic_lookup_obeys_handle_scope_and_clears_errors() {
        let _lock = crate::environment::ENVIRONMENT_TEST_LOCK.lock().unwrap();
        let options = crate::options::Options {
            headless: true,
            popup_errors: false,
            ..Default::default()
        };
        let icon = crate::image::Image::from_pixels(1, 1, vec![0; 4]);
        let mut env = Environment::new_without_app(options, icon).unwrap();
        let image = |path: &str, base| MachO {
            name: path.to_owned(),
            loaded_path: path.to_owned(),
            dynamic_libraries: Vec::new(),
            run_paths: Vec::new(),
            slide: 0,
            sections: Vec::new(),
            exported_symbols: HashMap::new(),
            symbol_locations: HashMap::new(),
            external_symbols: HashMap::new(),
            external_relocations: Vec::new(),
            entry_point_pc: None,
            entry_point_is_lc_main: false,
            last_segment_end: base + 0x1000,
            text_base: base,
            mapped_segments: vec![base..base + 0x1000],
        };
        let mut main = image("/app/Main", 0x1000);
        let mut dependency = image("/app/Dependency.dylib", 0x3000);
        let mut requested = image("/app/Requested.dylib", 0x5000);
        main.external_symbols.insert("_shared".to_owned(), 0x1101);
        dependency
            .external_symbols
            .insert("_shared".to_owned(), 0x3101);
        dependency
            .external_symbols
            .insert("_dependency_only".to_owned(), 0x3201);
        requested
            .external_symbols
            .insert("_shared".to_owned(), 0x5101);
        requested
            .dynamic_libraries
            .push(dependency.loaded_path.clone());
        env.bins = vec![main, dependency, requested];
        let path = env.mem.alloc_and_write_cstr(b"/app/Requested.dylib");
        let shared = env.mem.alloc_and_write_cstr(b"shared").cast_const();
        let dependency_only = env
            .mem
            .alloc_and_write_cstr(b"dependency_only")
            .cast_const();
        let missing = env.mem.alloc_and_write_cstr(b"missing").cast_const();
        let handle = dlopen(&mut env, path.cast_const(), 1);
        env.mem.free(path.cast_void());
        assert!(!handle.is_null());
        assert_eq!(dlsym(&mut env, handle, shared).to_bits(), 0x5101);
        assert_eq!(dlsym(&mut env, handle, dependency_only).to_bits(), 0x3201);
        assert_eq!(dlsym(&mut env, RTLD_DEFAULT, shared).to_bits(), 0x1101);
        assert!(dlsym(&mut env, handle, missing).is_null());
        assert_eq!(dlsym(&mut env, handle, shared).to_bits(), 0x5101);
        assert!(dlerror(&mut env).is_null());
        env.cpu.regs_mut()[crate::cpu::Cpu::LR] = 0x5011;
        assert_eq!(dlsym(&mut env, RTLD_SELF, shared).to_bits(), 0x5101);
        assert_eq!(dlsym(&mut env, RTLD_NEXT, shared).to_bits(), 0x3101);
        let path = env.mem.alloc_and_write_cstr(b"/app/Requested.dylib");
        let first = dlopen(&mut env, path.cast_const(), 1 | RTLD_FIRST);
        assert!(dlsym(&mut env, first, dependency_only).is_null());
        assert!(!dlerror(&mut env).is_null());
        assert!(dlerror(&mut env).is_null());
        assert!(dlsym(&mut env, Ptr::null(), shared).is_null());
        assert_eq!(dlclose(&mut env, handle), 0);
        assert!(dlerror(&mut env).is_null());
        assert!(dlsym(&mut env, handle, shared).is_null());
        assert!(!dlerror(&mut env).is_null());
        assert_eq!(dlclose(&mut env, first), 0);
    }
}
