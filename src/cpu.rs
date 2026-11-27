/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CPU emulation.
//!
//! Implemented using the C++ library dynarmic, which is a dynamic recompiler.
//!
//! iPhone OS apps used either ARMv6 or ARMv7-A, which are both 32-bit ISAs.
//! For the moment, only ARMv6 has been tested.
// KlugKlugTG was here!
use crate::abi::GuestFunction;
use crate::mem::{ConstPtr, GuestUSize, Mem, MutPtr, Ptr, SafeRead, SafeWrite};

// Import functions from C++
use touchHLE_dynarmic_wrapper::*;

pub mod asimd_fallback;

type VAddr = u32;
pub type CpuContext = touchHLE_DynarmicContext;

fn touchHLE_cpu_read_impl<T: SafeRead + Default>(
    mem: *mut touchHLE_Mem,
    addr: VAddr,
    error: *mut bool,
) -> T {
    // If a panic occurs (probably due to a null-pointer access), we can't let
    // it keep unwinding as it will hit non-Rust stack frames (dynarmic).
    // Instead we catch the unwind and then tell the C++ code a problem occurred
    // so it can immediately halt CPU execution and then panic itself, now
    // with only Rust stack frames to worry about and with CPU state information
    // available that's useful for debugging.
    //
    // TODO: Disable this in debug mode? This relies on dynarmic's
    // check_halt_on_memory_access option which surely has a significant
    // performance impact.
    //
    // I'm not sure if this actually is unwind-safe, but considering
    // the emulator will crash anyway, maybe this is okay.
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem>() };
        let ptr: ConstPtr<T> = Ptr::from_bits(addr);
        mem.read(ptr)
    }));
    unsafe {
        error.write(res.is_err());
    }
    res.unwrap_or_default()
}

fn touchHLE_cpu_write_impl<T: SafeWrite>(mem: *mut touchHLE_Mem, addr: VAddr, value: T) -> bool {
    // See comments above about catch_unwind
    let mut null_segment_size = 0;
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem>() };
        let ptr: MutPtr<T> = Ptr::from_bits(addr);
        null_segment_size = mem.null_segment_size();
        mem.write(ptr, value)
    }));
    if res.is_err() {
        return true;
    }
    // Diagnostic: report a `true` (memory abort) for the first guest store
    // into the null segment, so the emulator can print the exact faulting PC
    // and stack. Returning false for it, as before, lets the guest write
    // through NULL and carry on with no clue where it happened.
    addr < null_segment_size && crate::mem::null_write_probe_claim()
}

// Export functions for use by C++
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u8(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u8 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u16(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u16 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u32(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u32 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u64(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u64 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u8(mem: *mut touchHLE_Mem, addr: VAddr, value: u8) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u16(mem: *mut touchHLE_Mem, addr: VAddr, value: u16) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u32(mem: *mut touchHLE_Mem, addr: VAddr, value: u32) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u64(mem: *mut touchHLE_Mem, addr: VAddr, value: u64) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}

pub struct Cpu {
    dynarmic_wrapper: *mut touchHLE_DynarmicWrapper,
    /// Copy of the direct memory access pointer used to check it has not
    /// changed. If this is null, direct memory access is not in use.
    direct_memory_access_ptr: *const std::ffi::c_void,
}

impl Drop for Cpu {
    fn drop(&mut self) {
        unsafe { touchHLE_DynarmicWrapper_delete(self.dynarmic_wrapper) }
    }
}

/// Why CPU execution ended.
#[derive(Debug)]
pub enum CpuState {
    /// Execution halted due to using up all remaining ticks (normal execution)
    /// or after the single instruction was executed (step execution).
    Normal,
    /// SVC instruction encountered.
    Svc(u32),
    /// An error was encountered.
    Error(CpuError),
}

/// A reason that can cause CPU execution to be interrupted.
#[derive(Debug, Clone, PartialEq)]
pub enum CpuError {
    /// Memory error during execution (probably a null page access).
    MemoryError,
    /// Undefined instruction (perhaps from a GDB software breakpoint).
    UndefinedInstruction,
    /// Breakpoint (`bkpt` instruction).
    Breakpoint,
}

impl Cpu {
    /// The register number of the stack pointer.
    pub const SP: usize = 13;
    /// The register number of the link register.
    #[allow(unused)]
    pub const LR: usize = 14;
    /// The register number of the program counter.
    pub const PC: usize = 15;

    /// When this bit is set in CPSR, the CPU is in Thumb mode.
    pub const CPSR_THUMB: u32 = 0x00000020;

    /// When this bit is set in CPSR, the CPU is in user mode.
    pub const CPSR_USER_MODE: u32 = 0x00000010;

    /// Construct a new CPU instance. If a mutable reference to a [Mem] instance
    /// is provided, direct memory access is enabled, and the CPU instance
    /// becomes bound to that [Mem] instance (subsequent calls must use the same
    /// one).
    pub fn new(mut direct_memory_access: Option<&mut Mem>) -> Cpu {
        // Null page count is in pages rather than bytes. Mem ensures it is
        // page aligned.
        let null_page_count: usize = direct_memory_access
            .as_ref()
            .map_or(0, |mem| mem.null_segment_size() / 0x1000)
            .try_into()
            .unwrap();
        let fastmem_enabled = direct_memory_access
            .as_deref_mut()
            .map_or(false, Mem::enable_fastmem);
        // Safety: the direct memory access pointer will be retained directly by
        // the dynarmic wrapper and indirectly by cached JIT code, so we must
        // ensure we only execute the CPU while holding a &mut on the Mem object
        // to which that pointer belongs.
        let direct_memory_access_ptr = direct_memory_access
            .map_or(std::ptr::null_mut(), |mem| unsafe {
                mem.direct_memory_access_ptr()
            });
        let dynarmic_wrapper = unsafe {
            touchHLE_DynarmicWrapper_new(direct_memory_access_ptr, null_page_count, fastmem_enabled)
        };
        Cpu {
            dynarmic_wrapper,
            direct_memory_access_ptr,
        }
    }

    pub fn regs(&self) -> &[u32; 16] {
        unsafe {
            let ptr = touchHLE_DynarmicWrapper_regs_const(self.dynarmic_wrapper);
            &*(ptr as *const [u32; 16])
        }
    }
    pub fn regs_mut(&mut self) -> &mut [u32; 16] {
        unsafe {
            let ptr = touchHLE_DynarmicWrapper_regs_mut(self.dynarmic_wrapper);
            &mut *(ptr as *mut [u32; 16])
        }
    }

    /// The VFP/Advanced SIMD register file, as 32 64-bit D registers (each
    /// stored as its low word followed by its high word).
    pub fn extregs(&self) -> &[u32; 64] {
        unsafe {
            let ptr = touchHLE_DynarmicWrapper_extregs_const(self.dynarmic_wrapper);
            &*(ptr as *const [u32; 64])
        }
    }
    pub fn extregs_mut(&mut self) -> &mut [u32; 64] {
        unsafe {
            let ptr = touchHLE_DynarmicWrapper_extregs_mut(self.dynarmic_wrapper);
            &mut *(ptr as *mut [u32; 64])
        }
    }

    /// Dump the registers of the current cpu to the log output.
    /// Silently ignores panics.
    pub fn dump_regs(&self) {
        let regs = self.regs();
        Self::echo_regs(regs);
    }

    pub fn echo_regs(regs: &[u32; 16]) {
        // Silently ignore panics so it's safe to use in contexts where we
        // can't panic.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for row in 0..4 {
                use std::fmt::Write;
                let mut line = String::new();
                for col in 0..4 {
                    let reg_idx = row * 4 + col;
                    match reg_idx {
                        Self::SP => write!(&mut line, "\t SP: "),
                        Self::LR => write!(&mut line, "\t LR: "),
                        Self::PC => write!(&mut line, "\t PC: "),
                        _ if reg_idx <= 9 => write!(&mut line, "\t R{reg_idx}: "),
                        _ => write!(&mut line, "\tR{reg_idx}: "),
                    }
                    .unwrap();
                    write!(&mut line, "{:#010x}", regs[reg_idx]).unwrap();
                }
                echo!("{}", line);
            }
        }));
    }

    pub fn cpsr(&self) -> u32 {
        unsafe { touchHLE_DynarmicWrapper_cpsr(self.dynarmic_wrapper) }
    }
    pub fn set_cpsr(&mut self, cpsr: u32) {
        unsafe { touchHLE_DynarmicWrapper_set_cpsr(self.dynarmic_wrapper, cpsr) }
    }
    pub fn fpscr(&self) -> u32 {
        unsafe { touchHLE_DynarmicWrapper_fpscr(self.dynarmic_wrapper) }
    }
    pub fn set_fpscr(&mut self, fpscr: u32) {
        unsafe { touchHLE_DynarmicWrapper_set_fpscr(self.dynarmic_wrapper, fpscr) }
    }
    /// Set the value guest code reads from the CP15 TPIDRURO register
    /// (`mrc p15, 0, rX, c13, c0, 3`). This is per-thread state that is not
    /// part of [CpuContext], so it must be set again on every thread switch.
    pub fn set_tpidruro(&mut self, value: u32) {
        unsafe { touchHLE_DynarmicWrapper_set_tpidruro(self.dynarmic_wrapper, value) }
    }

    /// Start address of the instruction that raised the most recent
    /// [CpuError::UndefinedInstruction] or [CpuError::Breakpoint].
    ///
    /// When dynarmic raises one of these exceptions it leaves the PC register
    /// pointing at the *next* instruction, and because Thumb instructions can
    /// be either 2 or 4 bytes long, the faulting address can't be recovered
    /// from the register file alone. Dynarmic does pass the exact address to
    /// the exception callback though, so the wrapper records it.
    ///
    /// Only meaningful immediately after [Self::run_or_step] returned one of
    /// the errors above; the value is stale at any other time.
    pub fn last_exception_pc(&self) -> VAddr {
        unsafe { touchHLE_DynarmicWrapper_last_exception_pc(self.dynarmic_wrapper) }
    }

    /// Swap the current state of the CPU (registers etc) with the state stored
    /// in the context object.
    pub fn swap_context(&mut self, context: &mut CpuContext) {
        unsafe { touchHLE_DynarmicWrapper_swap_context(self.dynarmic_wrapper, context) }
    }

    /// Get PC with the Thumb bit appropriately set.
    pub fn pc_with_thumb_bit(&self) -> GuestFunction {
        let pc = self.regs()[Self::PC];
        let thumb = (self.cpsr() & Self::CPSR_THUMB) == Self::CPSR_THUMB;
        GuestFunction::from_addr_and_thumb_flag(pc, thumb)
    }

    /// Set PC and the Thumb flag for executing a guest function. Note that this
    /// does not touch LR.
    pub fn branch(&mut self, new_pc: GuestFunction) {
        self.regs_mut()[Self::PC] = new_pc.addr_without_thumb_bit();
        let cpsr_without_thumb = self.cpsr() & (!Self::CPSR_THUMB);
        self.set_cpsr(cpsr_without_thumb | ((new_pc.is_thumb() as u32) * Self::CPSR_THUMB))
    }

    /// Set the PC and Thumb flag (like [Self::branch]), but also set the LR,
    /// and return the original PC and LR.
    pub fn branch_with_link(
        &mut self,
        new_pc: GuestFunction,
        new_lr: GuestFunction,
    ) -> (GuestFunction, GuestFunction) {
        let old_pc = self.pc_with_thumb_bit();
        let old_lr = GuestFunction::from_addr_with_thumb_bit(self.regs()[Self::LR]);
        self.branch(new_pc);
        self.regs_mut()[Self::LR] = new_lr.addr_with_thumb_bit();
        (old_pc, old_lr)
    }

    /// Clear dynarmic's instruction cache for some range of addresses.
    /// This is of interest to the dynamic linker, which will sometimes rewrite
    /// code.
    pub fn invalidate_cache_range(&mut self, base: VAddr, size: GuestUSize) {
        unsafe {
            touchHLE_DynarmicWrapper_invalidate_cache_range(self.dynarmic_wrapper, base, size)
        }
    }

    /// Start CPU execution.
    ///
    /// If `ticks` is [Some], it is used as an abstract time limit. The value
    /// will be reduced proportionately with the amount of ticks expended.
    ///
    /// If `ticks` is [None], the CPU executes only a single instruction. This
    /// is also known as "stepping".
    ///
    /// This will return either because the CPU ran out of time, or because
    /// something else happened which requires attention from the host.
    #[must_use]
    pub fn run_or_step(&mut self, mem: &mut Mem, ticks: Option<&mut u64>) -> CpuState {
        // See ::new() for why this is done.
        if !self.direct_memory_access_ptr.is_null() {
            assert!(self.direct_memory_access_ptr == unsafe { mem.direct_memory_access_ptr() });
        }

        let res = unsafe {
            touchHLE_DynarmicWrapper_run_or_step(
                self.dynarmic_wrapper,
                mem as *mut Mem as *mut touchHLE_Mem,
                ticks,
            )
        };
        match res {
            -1 => CpuState::Normal,
            -2 => CpuState::Error(CpuError::MemoryError),
            -3 => CpuState::Error(CpuError::UndefinedInstruction),
            -4 => CpuState::Error(CpuError::Breakpoint),
            _ if res < -4 => panic!("Unexpected CPU execution result"),
            svc => CpuState::Svc(svc as u32),
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod fastmem_tests {
    use super::{Cpu, CpuState};
    use crate::mem::{Mem, MutPtr, Ptr, PAGE_SIZE};

    #[test]
    fn guarded_null_reads_and_direct_guest_memory_access_work() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);

        let code: MutPtr<u32> = Ptr::from_bits(PAGE_SIZE);
        mem.write(code, 0xE591_0000);
        mem.write(code + 1, 0xE581_2000);
        mem.write(code + 2, 0xE591_0000);
        mem.write(Ptr::<u32, true>::from_bits(PAGE_SIZE * 2), 0xCAFE_BABE);

        let mut cpu = Cpu::new(Some(&mut mem));
        cpu.set_cpsr(Cpu::CPSR_USER_MODE);
        cpu.regs_mut()[1] = 0;
        cpu.regs_mut()[2] = 0x1234_5678;
        cpu.regs_mut()[Cpu::PC] = PAGE_SIZE;
        assert!(matches!(cpu.run_or_step(&mut mem, None), CpuState::Normal));
        assert_eq!(cpu.regs()[0], 0);

        cpu.regs_mut()[1] = PAGE_SIZE * 2;
        cpu.regs_mut()[Cpu::PC] = PAGE_SIZE + 4;
        assert!(matches!(cpu.run_or_step(&mut mem, None), CpuState::Normal));
        assert_eq!(
            mem.read(Ptr::<u32, true>::from_bits(PAGE_SIZE * 2).cast_const()),
            0x1234_5678
        );

        cpu.regs_mut()[Cpu::PC] = PAGE_SIZE + 8;
        assert!(matches!(cpu.run_or_step(&mut mem, None), CpuState::Normal));
        assert_eq!(cpu.regs()[0], 0x1234_5678);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_straight_line_blocks_yield_and_resume_at_the_next_instruction() {
        for thumb in [false, true] {
            let mut mem = Mem::new();
            mem.set_null_segment_size(0x1000);
            let code = mem.alloc(0x2000);
            let size = if thumb { 2 } else { 4 };
            for i in 0..1024 {
                if thumb {
                    mem.write(code.cast::<u16>() + i, 0x3001); // adds r0, #1
                } else {
                    mem.write(code.cast::<u32>() + i, 0xe2800001); // add r0, r0, #1
                }
            }
            let stop = code.to_bits() + size * 1024;
            if thumb {
                mem.write(Ptr::<u16, true>::from_bits(stop), 0xdf01);
            } else {
                mem.write(Ptr::<u32, true>::from_bits(stop), 0xef000001);
            }
            let mut cpu = Cpu::new(None);
            cpu.branch(GuestFunction::from_addr_with_thumb_bit(code.to_bits() | u32::from(thumb)));
            cpu.regs_mut()[0] = 0;
            assert!(matches!(cpu.run_or_step(&mut mem, Some(&mut 1)), CpuState::Normal));
            let count = cpu.regs()[0];
            assert!(count > 0 && count < 1024);
            assert_eq!(cpu.regs()[Cpu::PC], code.to_bits() + count * size);
            assert!(matches!(cpu.run_or_step(&mut mem, Some(&mut 2048)), CpuState::Svc(1)));
            assert_eq!(cpu.regs()[0], 1024);
        }
    }

    #[test]
    fn exclusive_store_requires_a_reservation_from_the_current_guest_thread() {
        // Thumb-2 LDREXD r2, r3, [r0] followed by STREXD r1, r4, r5, [r0].
        // XNU's context-switch path issues CLREX before resuming the thread.
        // https://github.com/darwin-on-arm/xnu/blob/master/osfmk/arm/cswitch.s
        let mut mem = Mem::new();
        mem.set_null_segment_size(0x1000);
        let code = mem.alloc(8).cast::<u16>();
        for (i, halfword) in [0xe8d0, 0x237f, 0xe8c0, 0x4571].into_iter().enumerate() {
            mem.write(code + i as u32, halfword);
        }
        let initial = 0x1234_5678_0123_4567u64;
        let word = mem.alloc_and_write(initial);
        let mut cpu = Cpu::new(None);
        let start = crate::abi::GuestFunction::from_addr_with_thumb_bit(code.to_bits() | 1);
        let step = |cpu: &mut Cpu, mem: &mut Mem| {
            assert!(matches!(cpu.run_or_step(mem, None), CpuState::Normal));
        };

        cpu.branch(start);
        cpu.regs_mut()[0] = word.to_bits();
        cpu.regs_mut()[4] = 42;
        cpu.regs_mut()[5] = 24;
        step(&mut cpu, &mut mem);
        assert_eq!(cpu.regs()[2], initial as u32);
        assert_eq!(cpu.regs()[3], (initial >> 32) as u32);
        step(&mut cpu, &mut mem);
        assert_eq!(cpu.regs()[1], 0);
        assert_eq!(mem.read(word), (24u64 << 32) | 42);

        mem.write(word, initial);
        cpu.branch(start);
        step(&mut cpu, &mut mem);
        let mut other = CpuContext::new();
        other.cpsr = cpu.cpsr();
        other.regs[Cpu::PC] = (code + 2).to_bits();
        other.regs[0] = word.to_bits();
        cpu.swap_context(&mut other);
        step(&mut cpu, &mut mem);
        assert_ne!(cpu.regs()[1], 0);
        cpu.swap_context(&mut other);
        step(&mut cpu, &mut mem);
        assert_ne!(cpu.regs()[1], 0);
        assert_eq!(mem.read(word), initial);
    }
}
