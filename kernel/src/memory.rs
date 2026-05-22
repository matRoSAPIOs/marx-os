//! Physical memory + paging support.
//!
//! Two pieces:
//!   * `init()` constructs an `OffsetPageTable` over the active CR3, using the
//!     linear physical-memory map the bootloader installed at
//!     `physical_memory_offset`. This is our handle for `map_to` calls.
//!   * `BootInfoFrameAllocator` hands out free physical 4 KiB frames from the
//!     bootloader's memory map. It's a naive O(n)-per-alloc walker — perfect
//!     for boot-time heap setup, but if the kernel later does heavy frame
//!     churn we'll want a bitmap/buddy allocator instead.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

/// Build an `OffsetPageTable` for the currently-loaded address space.
///
/// SAFETY: caller guarantees that `physical_memory_offset` is the base of a
/// complete linear mapping of physical memory, and that no other live
/// `OffsetPageTable` for this address space exists.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    let (frame, _) = Cr3::read();
    let phys: PhysAddr = frame.start_address();
    let virt: VirtAddr = physical_memory_offset + phys.as_u64();
    &mut *virt.as_mut_ptr::<PageTable>()
}

/// Bump-style frame allocator that walks the bootloader memory map.
pub struct BootInfoFrameAllocator {
    memory_regions: &'static MemoryRegions,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// SAFETY: `memory_regions` must accurately describe RAM and nothing else
    /// may hand out the same frames.
    pub unsafe fn new(memory_regions: &'static MemoryRegions) -> Self {
        Self { memory_regions, next: 0 }
    }

    /// Iterate every 4 KiB-aligned physical address that lies in a Usable region.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.memory_regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .map(|r| r.start..r.end)
            .flat_map(|range| range.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}
