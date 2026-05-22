//! Global Descriptor Table + Task State Segment.
//!
//! In long mode the GDT is mostly vestigial (segmentation is bypassed for code/data),
//! but we still need it to load CS and to install a TSS — and we *need* the TSS
//! to have a dedicated Interrupt Stack Table (IST) entry for the double-fault
//! handler. Without that, a stack-overflow page-fault → double-fault would
//! recurse on the same broken stack and triple-fault the CPU.

use spin::Lazy;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

/// IST slot reserved for the double-fault handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// 20 KiB dedicated stack used when the CPU dispatches a double-fault.
const STACK_SIZE: usize = 4096 * 5;
static mut DOUBLE_FAULT_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        // Stacks grow down on x86 — IST entry must point past the top of the buffer.
        let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
        stack_start + STACK_SIZE as u64
    };
    tss
});

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let code_selector = gdt.append(Descriptor::kernel_code_segment());
    let data_selector = gdt.append(Descriptor::kernel_data_segment());
    let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
    (gdt, Selectors { code_selector, data_selector, tss_selector })
});

/// Install the GDT, reload all segment regs, and load the TSS.
///
/// Why we touch SS even though long mode "doesn't use segmentation":
/// `IRET` from any interrupt unconditionally pops SS from the saved frame.
/// The bootloader handed us an SS selector valid in *its* GDT; once we swap
/// GDTs, the same selector value may point to a non-data descriptor in ours,
/// and the first IRET then #GPs. So we must rewrite SS with a valid data
/// selector from our own GDT before enabling interrupts.
pub fn init() {
    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        SS::set_reg(GDT.1.data_selector);
        DS::set_reg(SegmentSelector::NULL);
        ES::set_reg(SegmentSelector::NULL);
        load_tss(GDT.1.tss_selector);
    }
}
