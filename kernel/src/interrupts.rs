//! Interrupt Descriptor Table, CPU-exception handlers, and 8259 PIC plumbing
//! (timer + keyboard).
//!
//! Layout:
//! - CPU exceptions occupy vectors 0..=31 (hard-wired by Intel).
//! - We remap the master/slave PICs to 32..=47 so legacy IRQs don't collide
//!   with exceptions.

use core::sync::atomic::{AtomicU64, Ordering};

use spin::{Lazy, Mutex};
use pic8259::ChainedPics;
use pc_keyboard::{layouts::Us104Key, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::gdt;
use crate::println;

// ---------- PIC ----------

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// SAFETY: PIC_*_OFFSET must not collide with CPU exception vectors (0..32).
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,            // IRQ0  → 32
    Keyboard,                         // IRQ1  → 33
    Mouse = PIC_1_OFFSET + 12,        // IRQ12 → 44   (slave PIC)
    IdePrimary = PIC_1_OFFSET + 14,   // IRQ14 → 46
    IdeSecondary,                     // IRQ15 → 47
}

impl InterruptIndex {
    fn as_u8(self) -> u8 { self as u8 }
}

// ---------- IDT ----------

static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

    // CPU exceptions we care about.
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.general_protection_fault.set_handler_fn(gpf_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.stack_segment_fault.set_handler_fn(stack_segment_handler);
    idt.segment_not_present.set_handler_fn(segment_not_present_handler);
    unsafe {
        // Double-fault MUST run on its own stack — otherwise a stack-overflow
        // page-fault double-faults onto the same broken stack and triple-faults.
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }

    // PIC IRQs.
    idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_handler);
    idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_handler);
    idt[InterruptIndex::Mouse.as_u8()].set_handler_fn(mouse_handler);
    // ATA drives can latch IRQ14/15 even when we're operating in PIO; install
    // no-op handlers that just EOI so a stray completion IRQ doesn't trigger
    // SEGMENT-NOT-PRESENT when we later `sti`.
    idt[InterruptIndex::IdePrimary.as_u8()].set_handler_fn(ide_primary_handler);
    idt[InterruptIndex::IdeSecondary.as_u8()].set_handler_fn(ide_secondary_handler);

    idt
});

/// Load the IDT. Call once during boot.
pub fn init_idt() {
    IDT.load();
}

// ---------- exception handlers ----------

extern "x86-interrupt" fn breakpoint_handler(stack: InterruptStackFrame) {
    println!("[exc] BREAKPOINT @ {:?}", stack.instruction_pointer);
}

// All fatal CPU exceptions funnel through panic!() so the kernel renders the
// same KITTY BSOD regardless of whether the kernel killed itself or the CPU
// did. Double-fault is the only one that's a `-> !` handler (CPU never
// returns to us after it).

extern "x86-interrupt" fn double_fault_handler(stack: InterruptStackFrame, code: u64) -> ! {
    panic!("DOUBLE FAULT (err {:#x}) at {:?}", code, stack.instruction_pointer);
}

extern "x86-interrupt" fn page_fault_handler(stack: InterruptStackFrame, code: PageFaultErrorCode) {
    use x86_64::registers::control::Cr2;
    let addr = Cr2::read();
    panic!("PAGE FAULT ({:?}) at {:?}, tried to touch {:?}",
           code, stack.instruction_pointer, addr);
}

extern "x86-interrupt" fn gpf_handler(stack: InterruptStackFrame, code: u64) {
    panic!("GENERAL PROTECTION FAULT (err {:#x}) at {:?}",
           code, stack.instruction_pointer);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack: InterruptStackFrame) {
    panic!("INVALID OPCODE at {:?}", stack.instruction_pointer);
}

extern "x86-interrupt" fn divide_error_handler(stack: InterruptStackFrame) {
    panic!("DIVIDE BY ZERO at {:?}", stack.instruction_pointer);
}

extern "x86-interrupt" fn stack_segment_handler(stack: InterruptStackFrame, code: u64) {
    panic!("STACK-SEGMENT FAULT (err {:#x}) at {:?}",
           code, stack.instruction_pointer);
}

extern "x86-interrupt" fn segment_not_present_handler(stack: InterruptStackFrame, code: u64) {
    panic!("SEGMENT NOT PRESENT (err {:#x}) at {:?}",
           code, stack.instruction_pointer);
}

// ---------- IRQ handlers ----------

/// Number of PIT ticks since boot. PIT defaults to ~18.2 Hz on PC, so this is
/// `uptime ≈ TICKS / 18.2` seconds.
static TICKS: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)] // for an `uptime` shell command later
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

extern "x86-interrupt" fn timer_handler(_stack: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    // EOI first so the PIC will deliver the NEXT timer tick even if we never
    // come back from yield_now (we will, but conceptually correct).
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
    // Repaint the mouse cursor if it moved since the last tick. Cheap when
    // idle (one Mutex acquire + position compare).
    crate::cursor::tick();
    // Flush backbuffer to the hardware framebuffer so anything drawn since
    // the previous tick becomes visible. No-op until init_backbuffer().
    crate::framebuffer::present();
    // Preempt: round-robin to the next ready task. No-op if scheduler is
    // uninitialised or only one task exists.
    crate::task::yield_now();
}

static KEYBOARD: Lazy<Mutex<Keyboard<Us104Key, ScancodeSet1>>> = Lazy::new(|| {
    Mutex::new(Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore))
});

extern "x86-interrupt" fn keyboard_handler(_stack: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port: Port<u8> = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    let mut kb = KEYBOARD.lock();
    if let Ok(Some(event)) = kb.add_byte(scancode) {
        if let Some(key) = kb.process_keyevent(event) {
            match key {
                // Push only basic-Latin chars into the input ring. Non-ASCII
                // would need wider encoding; we don't have a use for them yet.
                DecodedKey::Unicode(c) if (c as u32) < 128 => {
                    crate::input::push(c as u8);
                }
                _ => {}
            }
        }
    }

    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

extern "x86-interrupt" fn mouse_handler(_stack: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let mut data: Port<u8> = Port::new(0x60);
    let byte = unsafe { data.read() };
    crate::mouse::handle_byte(byte);
    // Repaint the cursor at full mouse-IRQ rate (~100 Hz when moving) so
    // motion feels smooth instead of locked to the 18 Hz PIT tick. `tick()`
    // is a no-op when position hasn't changed (e.g. between the 3 bytes of
    // a single packet, only the third one actually moves the cursor).
    crate::cursor::tick();
    // Push the change to the screen. Without this, cursor would only become
    // visible at the next 18 Hz timer present.
    crate::framebuffer::present();
    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::Mouse.as_u8()); }
}

extern "x86-interrupt" fn ide_primary_handler(_stack: InterruptStackFrame) {
    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::IdePrimary.as_u8()); }
}

extern "x86-interrupt" fn ide_secondary_handler(_stack: InterruptStackFrame) {
    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::IdeSecondary.as_u8()); }
}


