//! Minimal ELF64 loader for static-PIE binaries (MarX-OS apps).
//!
//! Apps are compiled `--target x86_64-unknown-none` with Rust's default
//! `static-pie` linkage, which produces a position-independent executable
//! whose only relocations are `R_X86_64_RELATIVE`. This loader:
//!   1. Parses the ELF64 header + program headers.
//!   2. Computes the total virtual span of all PT_LOAD segments.
//!   3. Allocates a heap buffer big enough to hold it.
//!   4. Copies each PT_LOAD's file bytes into the buffer at the correct
//!      offset; BSS is implicitly zeroed by `vec![0u8; ...]`.
//!   5. Walks PT_DYNAMIC for `DT_RELA` and applies every
//!      `R_X86_64_RELATIVE` entry, rebasing pointers to where the buffer
//!      actually lives.
//!   6. Returns the loaded image + the absolute entry address.
//!
//! NX / W^X note: the buffer is allocated from the kernel heap, which is
//! mapped RW.  Bootloader-default page tables on `bootloader_api` 0.11 do
//! NOT set the NX bit on those pages, so executing from the buffer just
//! works.  If we ever tighten the page-table policy this will need an
//! explicit RWX mapping — for now it's adequate.

use alloc::vec;
use alloc::vec::Vec;

const ELFMAG:     [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8      = 2;
const ELFDATA2LSB:u8      = 1;
const EM_X86_64:  u16     = 0x3E;

const PT_LOAD:    u32 = 1;
const PT_DYNAMIC: u32 = 2;

const DT_NULL:    u64 = 0;
const DT_RELA:    u64 = 7;
const DT_RELASZ:  u64 = 8;
const DT_RELAENT: u64 = 9;

const R_X86_64_RELATIVE: u32 = 8;

#[repr(C)]
struct Elf64Header {
    e_ident:     [u8; 16],
    e_type:      u16,
    e_machine:   u16,
    e_version:   u32,
    e_entry:     u64,
    e_phoff:     u64,
    e_shoff:     u64,
    e_flags:     u32,
    e_ehsize:    u16,
    e_phentsize: u16,
    e_phnum:     u16,
    e_shentsize: u16,
    e_shnum:     u16,
    e_shstrndx:  u16,
}

#[repr(C)]
struct ProgramHeader {
    p_type:   u32,
    p_flags:  u32,
    p_offset: u64,
    p_vaddr:  u64,
    p_paddr:  u64,
    p_filesz: u64,
    p_memsz:  u64,
    p_align:  u64,
}

#[repr(C)]
struct DynEntry {
    d_tag: u64,
    d_val: u64,    // also d_ptr — same field
}

#[repr(C)]
struct Rela {
    r_offset: u64,
    r_info:   u64,
    r_addend: i64,
}

#[derive(Debug)]
#[allow(dead_code)] // variants read via Debug
pub enum ElfError {
    TooSmall,
    BadMagic,
    NotElf64Le,
    NotX86_64,
    NoLoadable,
}

/// A loaded, relocated app ready to execute.
pub struct LoadedApp {
    /// Owns the heap buffer holding the live image. Keeping this around
    /// keeps the code alive — when dropped, the app's memory is freed.
    /// (Read indirectly via the entry pointer, hence `allow(dead_code)`.)
    #[allow(dead_code)]
    pub memory: Vec<u8>,
    /// Absolute address of the app's `_start` symbol.
    pub entry:  usize,
}

/// Parse + load a static-PIE ELF64 from a byte slice.
pub fn load(bytes: &[u8]) -> Result<LoadedApp, ElfError> {
    if bytes.len() < core::mem::size_of::<Elf64Header>() {
        return Err(ElfError::TooSmall);
    }
    // SAFETY: we just bounds-checked the header size; the struct is repr(C)
    // and matches the on-disk ELF64 layout exactly.
    let hdr = unsafe { &*(bytes.as_ptr() as *const Elf64Header) };
    if hdr.e_ident[0..4] != ELFMAG                    { return Err(ElfError::BadMagic); }
    if hdr.e_ident[4] != ELFCLASS64                   { return Err(ElfError::NotElf64Le); }
    if hdr.e_ident[5] != ELFDATA2LSB                  { return Err(ElfError::NotElf64Le); }
    if hdr.e_machine  != EM_X86_64                    { return Err(ElfError::NotX86_64); }

    let ph_count = hdr.e_phnum     as usize;
    let ph_size  = hdr.e_phentsize as usize;
    let ph_off   = hdr.e_phoff     as usize;

    // --- 1. Compute the total virtual span of all PT_LOAD segments ---
    let mut min_v: u64 = u64::MAX;
    let mut max_v: u64 = 0;
    for i in 0..ph_count {
        let ph = unsafe {
            &*(bytes.as_ptr().add(ph_off + i * ph_size) as *const ProgramHeader)
        };
        if ph.p_type == PT_LOAD && ph.p_memsz > 0 {
            min_v = min_v.min(ph.p_vaddr);
            max_v = max_v.max(ph.p_vaddr + ph.p_memsz);
        }
    }
    if min_v == u64::MAX { return Err(ElfError::NoLoadable); }

    let img_size = (max_v - min_v) as usize;
    // Round up to 16-byte alignment so the buffer's base is a comfortable
    // alignment for code execution. (Most x86_64 ABIs only need 16 at call
    // sites; we don't need page alignment because we're not using paging
    // protections on this buffer.)
    let mut memory: Vec<u8> = vec![0u8; img_size + 16];
    let raw_base = memory.as_ptr() as usize;
    let aligned  = (raw_base + 15) & !15usize;
    let pad      = aligned - raw_base;
    // Operate on the aligned slice from here on.
    let load_base = unsafe { memory.as_mut_ptr().add(pad) };

    // PIE slide: every vaddr in the ELF is relative to min_v; the runtime
    // address is `load_base - min_v + vaddr`.  The "bias" we add to every
    // such vaddr to get a real pointer:
    let bias: i64 = (load_base as i64) - (min_v as i64);

    // --- 2. Copy PT_LOAD segments into the buffer ---
    for i in 0..ph_count {
        let ph = unsafe {
            &*(bytes.as_ptr().add(ph_off + i * ph_size) as *const ProgramHeader)
        };
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 { continue; }
        let dst_off = (ph.p_vaddr - min_v) as usize;
        let src_off = ph.p_offset as usize;
        let n_file  = ph.p_filesz as usize;
        if src_off + n_file > bytes.len() { return Err(ElfError::TooSmall); }
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(src_off),
                load_base.add(dst_off),
                n_file,
            );
        }
        // memsz > filesz tail (BSS) is already zero from `vec![0u8; ...]`.
    }

    // --- 3. Apply R_X86_64_RELATIVE relocations from PT_DYNAMIC ---
    for i in 0..ph_count {
        let ph = unsafe {
            &*(bytes.as_ptr().add(ph_off + i * ph_size) as *const ProgramHeader)
        };
        if ph.p_type != PT_DYNAMIC { continue; }
        let dyn_vaddr = ph.p_vaddr;
        let dyn_ptr   = (dyn_vaddr as i64 + bias) as *const DynEntry;
        unsafe { apply_dynamic_relocs(load_base, bias, dyn_ptr); }
    }

    let entry = (hdr.e_entry as i64 + bias) as usize;
    Ok(LoadedApp { memory, entry })
}

/// Walk a PT_DYNAMIC array looking for DT_RELA / DT_RELASZ / DT_RELAENT,
/// then apply every R_X86_64_RELATIVE relocation in that table.
unsafe fn apply_dynamic_relocs(load_base: *mut u8, bias: i64, dyn_ptr: *const DynEntry) {
    let mut rela_vaddr: Option<u64> = None;
    let mut rela_size:  Option<u64> = None;
    let mut rela_ent:   u64         = core::mem::size_of::<Rela>() as u64;

    let mut p = dyn_ptr;
    loop {
        let entry = &*p;
        match entry.d_tag {
            DT_NULL    => break,
            DT_RELA    => rela_vaddr = Some(entry.d_val),
            DT_RELASZ  => rela_size  = Some(entry.d_val),
            DT_RELAENT => rela_ent   = entry.d_val,
            _ => {}
        }
        p = p.add(1);
    }

    let (vaddr, size) = match (rela_vaddr, rela_size) {
        (Some(a), Some(s)) => (a, s),
        _ => return, // no relocations needed
    };

    let count = (size / rela_ent) as usize;
    let table = (vaddr as i64 + bias) as *const u8;
    for i in 0..count {
        let entry_ptr = table.add(i * rela_ent as usize) as *const Rela;
        let entry     = &*entry_ptr;
        let r_type    = (entry.r_info & 0xFFFFFFFF) as u32;
        if r_type != R_X86_64_RELATIVE { continue; }

        // Target = where to write the fixed-up pointer.
        // Value  = (load_base + addend), i.e. a pointer rebased to runtime.
        let target = (entry.r_offset as i64 + bias) as *mut u64;
        let value  = (load_base as i64 + entry.r_addend) as u64;
        core::ptr::write_unaligned(target, value);
    }

    // After mutating code/.data, fence so subsequent reads see the writes.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    let _ = load_base; // silence unused warning if we add no-op paths above
}
