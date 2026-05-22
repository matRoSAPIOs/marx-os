//! Kernel heap. Carves out a fixed virtual range, fills it with frames from
//! the frame allocator, and hands it to a linked-list `#[global_allocator]`.
//!
//! After `init()` succeeds, `extern crate alloc;` works — Box / Vec / String /
//! BTreeMap and friends all just work via the global allocator.

use linked_list_allocator::LockedHeap;
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};

/// Virtual address where the kernel heap starts. Picked from the empty middle
/// of the 64-bit address space so it can't collide with the kernel image,
/// stacks, framebuffer mapping, or the bootloader's linear phys-mem mapping.
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// Initial heap size. Bumped to 8 MiB for Phase 7.3: the compositor backbuffer
/// alone is 1280×720×3 ≈ 2.7 MiB, plus window buffers, FS reads, etc.
pub const HEAP_SIZE: usize = 8 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Map `HEAP_SIZE` bytes of fresh physical frames at `HEAP_START`, then point
/// the global allocator at that region.
pub fn init(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + (HEAP_SIZE - 1) as u64;
        let start_page = Page::containing_address(heap_start);
        let end_page = Page::containing_address(heap_end);
        Page::range_inclusive(start_page, end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe { mapper.map_to(page, frame, flags, frame_allocator)?.flush() };
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}
