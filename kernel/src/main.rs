#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use core::panic::PanicInfo;
use bootloader_api::{config::Mapping, entry_point, BootInfo, BootloaderConfig};
use x86_64::VirtAddr;

mod font;
mod serial;
mod framebuffer;
mod splash;
mod gdt;
mod interrupts;
mod memory;
mod allocator;
mod task;
mod ata;
mod fs;
mod input;
mod shell;
mod mouse;
mod cursor;
mod ttf;
mod button;
mod welcome;
mod wallpaper;
mod elf;
mod app_wm;
mod app;
mod desktop;

/// Tell the bootloader to map *all* physical memory into the kernel's virtual
/// address space at a dynamic offset. We need this so `memory::init` can build
/// an `OffsetPageTable` (each phys frame becomes reachable as
/// `physical_memory_offset + phys_addr`).
static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut c = BootloaderConfig::new_default();
    c.mappings.physical_memory = Some(Mapping::Dynamic);
    c
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();

    let mut screen_dims: Option<(u32, u32)> = None;

    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        screen_dims = Some((info.width as u32, info.height as u32));
        framebuffer::init(fb);
        sprintln!(
            "serial+fb online ({}x{} {:?} bpp={})",
            info.width, info.height, info.pixel_format, info.bytes_per_pixel
        );
        splash::show();
    } else {
        sprintln!("serial online (no framebuffer)");
    }

    println!("================================");
    println!("        MarX-OS v0.1");
    println!("    Phase 6.5 boot sequence");
    println!("================================");
    println!();

    gdt::init();
    println!("[ok] GDT + TSS loaded");

    interrupts::init_idt();
    println!("[ok] IDT loaded");

    unsafe { interrupts::PICS.lock().initialize(); }
    println!("[ok] 8259 PIC remapped to 0x20..0x2F");

    x86_64::instructions::interrupts::int3();
    println!("[ok] kernel survived int3");

    // PS/2 mouse — needs IDT loaded so IRQ12 has a target. Polling init is
    // safe with IF=0; the mouse only starts streaming packets after we sti.
    if let Some((sw, sh)) = screen_dims {
        match mouse::init(sw, sh) {
            Ok(()) => println!("[ok] PS/2 mouse online (cursor starts at {},{})", sw / 2, sh / 2),
            Err(e) => println!("[!!] mouse init failed: {:?}", e),
        }
    }

    // ---------------- Phase 4: paging + heap ----------------

    let phys_offset = VirtAddr::new(
        boot_info
            .physical_memory_offset
            .into_option()
            .expect("bootloader did not map physical memory — check BOOTLOADER_CONFIG"),
    );
    println!("[ok] phys-mem mapping @ {:?}", phys_offset);

    // Borrow memory_regions with a 'static lifetime. SAFETY: the underlying
    // data is owned by the bootloader-allocated BootInfo, which lives forever.
    let memory_regions: &'static bootloader_api::info::MemoryRegions = unsafe {
        core::mem::transmute(&boot_info.memory_regions)
    };

    let mut mapper = unsafe { memory::init(phys_offset) };
    let mut frame_allocator = unsafe { memory::BootInfoFrameAllocator::new(memory_regions) };

    allocator::init(&mut mapper, &mut frame_allocator)
        .expect("heap init failed");
    println!(
        "[ok] kernel heap @ {:#x} ({} KiB)",
        allocator::HEAP_START,
        allocator::HEAP_SIZE / 1024
    );

    // Smoke-test that the global allocator actually works.
    heap_smoke_test();

    // Switch the framebuffer pipeline from "direct-to-MMIO" to a Vec<u8>
    // backbuffer + periodic present. After this, every draw lands in RAM
    // first and only becomes visible at the next timer or mouse IRQ.
    framebuffer::init_backbuffer();
    println!("[ok] backbuffer compositor online ({}x{} px)",
        1280, 720); // (actual numbers are dynamic but boot fb is 1280x720)

    // Cursor backup buffer lives on the heap, so init after the allocator
    // is up. After this, the timer IRQ will paint the cursor on every tick.
    cursor::init();
    println!("[ok] cursor allocated; arrow will appear once sti is set");

    // Parse the embedded Roboto TTF so welcome screen can render proper text.
    ttf::init();
    println!("[ok] TTF font loaded (Roboto-Regular)");

    // ---------------- Phase 6: ATA + MARXARCH filesystem ----------------

    match fs::init(ata::Drive::Slave) {
        Ok(()) => {
            println!("[ok] MARXARCH mounted from primary IDE slave");
            print_disk_contents();
        }
        Err(e) => println!("[!!] fs init failed: {:?} (booting without disk)", e),
    }

    // ---------------- Phase 5: scheduler ----------------

    task::init();
    println!("[ok] scheduler initialised (this thread = task 0)");

    // Arm interrupts BEFORE spawning shell, so that all of kernel_main's
    // status prints happen first (with only one task in the scheduler, the
    // timer IRQ's yield_now is a no-op). The shell task then claims the CPU
    // on the next tick after we go idle.
    x86_64::instructions::interrupts::enable();
    println!("[ok] CPU interrupts enabled (sti)");

    // Welcome screen with click-to-continue button.
    welcome::show_and_wait();

    // Enter the GUI desktop. Doesn't return — runs an event loop forever.
    desktop::run();
}

/// Read welcome.txt + list the disk index, all from MARXARCH.
fn print_disk_contents() {
    use alloc::string::String;

    // Welcome file
    match fs::read("welcome.txt") {
        Ok(bytes) => {
            println!();
            println!("-------- /welcome.txt --------");
            match String::from_utf8(bytes) {
                Ok(text) => {
                    for line in text.lines() {
                        println!("{}", line);
                    }
                }
                Err(_) => println!("(file is not valid UTF-8)"),
            }
            println!("------------------------------");
            println!();
        }
        Err(e) => println!("[!!] welcome.txt: {:?}", e),
    }

    // Directory listing
    if let Ok(entries) = fs::list() {
        println!("[disk] {} file(s):", entries.len());
        let mut n_apps = 0;
        for e in entries {
            let kind = if e.name.ends_with(".elf") { n_apps += 1; "[app]" } else { "[dat]" };
            println!("       {:>5} B @ lba {:>3}  {} {}", e.size, e.lba, kind, e.name);
        }
        if n_apps > 0 {
            println!("[ok] discovered {} installed app(s)", n_apps);
        }
        println!();
    }
}


fn heap_smoke_test() {
    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::vec::Vec;

    let boxed = Box::new(0xC0FFEE_u64);
    let mut v: Vec<u32> = Vec::with_capacity(100);
    for i in 0..100u32 {
        v.push(i);
    }
    let sum: u32 = v.iter().sum();

    let mut s = String::new();
    s.push_str("dynamic strings work");

    println!("[ok] Box<u64> = 0x{:X}", *boxed);
    println!("[ok] Vec<u32> len={} sum={}", v.len(), sum);
    println!("[ok] String   = \"{}\" ({} bytes)", s, s.len());

    // Stress: allocate and free a bunch of small vecs to exercise the free-list.
    let mut keep: Vec<Vec<u8>> = Vec::new();
    for i in 0..256u32 {
        let mut buf: Vec<u8> = Vec::with_capacity(64);
        for _ in 0..64 { buf.push(i as u8); }
        keep.push(buf);
    }
    let total_bytes: usize = keep.iter().map(|v| v.len()).sum();
    drop(keep);
    println!("[ok] alloc/dealloc stress: 256 x 64 B = {} bytes ok", total_bytes);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // From this point on the kernel is done. Disable IRQs so nothing can
    // overwrite our final paint, and so println-from-IRQ can't recurse.
    x86_64::instructions::interrupts::disable();

    // BSOD palette: classic XP-blue background, white text.
    framebuffer::set_bg(0x00, 0x00, 0xAA);
    framebuffer::set_fg(0xFF, 0xFF, 0xFF);
    framebuffer::clear();

    let (rax, rbx, rcx, rdx, rsp, rbp, rip) = snapshot_regs();
    let cr2 = x86_64::registers::control::Cr2::read_raw();
    let stack = unsafe { read_stack(rsp) };
    let ticks = interrupts::ticks();
    let uptime_s = ticks / 18; // PIT defaults to ~18.2 Hz

    let (loc_file, loc_line) = match info.location() {
        Some(l) => (l.file(), l.line()),
        None    => ("?", 0),
    };

    println!("+------------------------------------------------------------------------------+");
    println!("|  [!]  KERNEL PANIC: UNHANDLED HISS-CEPTION                                   |");
    println!("+------------------------------------------------------------------------------+");
    println!();
    println!("    /\\_/\\       [ FATAL ERROR ]: {}", info.message());
    println!("   ( o.o )      --------------------------------------------------------------");
    println!("    > ^ <       The kernel found a bug, played with it for a bit,");
    println!("   /     \\      and then knocked the entire stack off the table.");
    println!("  |       |");
    println!(" / \\_|_/\\_ \\    Naps taken before crash: 0   (Uptime: {} s / {} ticks)", uptime_s, ticks);
    println!();
    println!("    at {}:{}", loc_file, loc_line);
    println!();
    println!("+--[ MEOW-RY DUMP & CATASTROPHE ANALYSIS ]------------------------------------+");
    println!("|  RIP: 0x{:016X}    CR2: 0x{:016X}        |", rip, cr2);
    println!("|  RAX: 0x{:016X}    RBX: 0x{:016X}        |", rax, rbx);
    println!("|  RCX: 0x{:016X}    RDX: 0x{:016X}        |", rcx, rdx);
    println!("|  RSP: 0x{:016X}    RBP: 0x{:016X}        |", rsp, rbp);
    println!("+--[ STACK DUMP (top 4 qwords) ]----------------------------------------------+");
    println!("|  [RSP+00]: 0x{:016X}    [RSP+08]: 0x{:016X}      |", stack[0], stack[1]);
    println!("|  [RSP+10]: 0x{:016X}    [RSP+18]: 0x{:016X}      |", stack[2], stack[3]);
    println!("+-----------------------------------------------------------------------------+");
    println!();
    println!("  [ RECOMMENDED ACTIONS ]");
    println!("    1. Do not panic. The cat is sleeping soundly inside the CPU registers.");
    println!("    2. Shake a bag of treats to see if the kernel wakes up (it won't).");
    println!("    3. Press the RESET button on your PC -- or Ctrl+Alt+Q in QEMU.");

    halt_loop_dead();
}

/// Snapshot of general-purpose registers + current RIP. Caller-saved regs
/// are inevitably clobbered by the call itself, so values describe "what we
/// see at this instruction" — useful for orientation, not forensic accuracy.
#[inline(never)]
fn snapshot_regs() -> (u64, u64, u64, u64, u64, u64, u64) {
    let rax: u64; let rbx: u64; let rcx: u64; let rdx: u64;
    let rsp: u64; let rbp: u64; let rip: u64;
    unsafe {
        core::arch::asm!(
            "mov {0}, rax",
            "mov {1}, rbx",
            "mov {2}, rcx",
            "mov {3}, rdx",
            "mov {4}, rsp",
            "mov {5}, rbp",
            "lea {6}, [rip]",
            out(reg) rax, out(reg) rbx, out(reg) rcx, out(reg) rdx,
            out(reg) rsp, out(reg) rbp, out(reg) rip,
            options(nomem, nostack, preserves_flags),
        );
    }
    (rax, rbx, rcx, rdx, rsp, rbp, rip)
}

/// Read four 64-bit words at and above the given stack pointer.
unsafe fn read_stack(rsp: u64) -> [u64; 4] {
    let p = rsp as *const u64;
    [
        p.add(0).read_volatile(),
        p.add(1).read_volatile(),
        p.add(2).read_volatile(),
        p.add(3).read_volatile(),
    ]
}


#[allow(dead_code)] // dormant: desktop::run never returns
fn halt_loop_idle() -> ! {
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

fn halt_loop_dead() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

// ------------- dual-output print!/println! -------------

pub struct DualWriter;

impl core::fmt::Write for DualWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        x86_64::instructions::interrupts::without_interrupts(|| {
            serial::write_str(s);
            framebuffer::write_str(s);
        });
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = core::write!($crate::DualWriter, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println {
    ()                             => { $crate::print!("\n") };
    ($fmt:expr $(, $($arg:tt)*)?) => { $crate::print!(concat!($fmt, "\n") $(, $($arg)*)?) };
}
