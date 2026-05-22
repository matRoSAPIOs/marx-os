# MarX-OS — disk contents

You're reading this because the kernel could:

  1. Talk to the PIIX3 IDE controller on QEMU via ports 0x1F0..0x1F7
  2. Issue an LBA28 READ SECTORS (0x20) command to the slave drive
  3. Poll BSY/DRQ status bits without blowing up
  4. Parse the MARXARCH archive in sector 0
  5. Resolve a filename to its (lba, size), read the right sectors,
     and hand the bytes back as a Vec<u8>

That's a full read-only filesystem stack, from inb/outb up to
String::from_utf8 — built from scratch in Rust.

Phases done so far:
  1   boot + serial
  2   framebuffer + println!
  2.5 boot splash with logo
  3   GDT + IDT + interrupts + keyboard
  4   heap + paging
  5   preemptive scheduler (asm context switch)
  6   ATA + filesystem  <-- you are here

Next: GUI (mouse + window manager + compositor).
