use core::fmt;

const COM1: u16 = 0x3F8;

/// One-time UART 16550 initialisation.
pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // disable interrupts
        outb(COM1 + 3, 0x80); // enable DLAB (set baud divisor)
        outb(COM1 + 0, 0x03); // divisor lo — 38400 baud
        outb(COM1 + 1, 0x00); // divisor hi
        outb(COM1 + 3, 0x03); // 8-N-1, DLAB off
        outb(COM1 + 2, 0xC7); // FIFO on, clear, 14-byte threshold
        outb(COM1 + 4, 0x0B); // IRQs enabled, RTS/DSR set
    }
}

/// Write a single byte, spin-waiting until the TX buffer is empty.
pub fn write_byte(b: u8) {
    unsafe {
        while (inb(COM1 + 5) & 0x20) == 0 {}
        outb(COM1, b);
    }
}

/// Write a string to COM1.
pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}

// ---------- fmt::Write adapter so format_args! works ----------

pub struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

// ---------- macros ----------

/// Print to serial (COM1) without a newline.
#[macro_export]
macro_rules! sprint {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = core::write!($crate::serial::SerialWriter, $($arg)*);
    }};
}

/// Print a line to serial (COM1).
#[macro_export]
macro_rules! sprintln {
    ()                              => { $crate::sprint!("\n") };
    ($fmt:expr $(, $($arg:tt)*)?)  => { $crate::sprint!(concat!($fmt, "\n") $(, $($arg)*)?) };
}

// ---------- port I/O helpers ----------

#[inline]
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    val
}
