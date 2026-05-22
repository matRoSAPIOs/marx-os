//! Minimal blocking ATA-PIO driver for the primary IDE channel.
//!
//! Read-only. LBA28 only (covers up to 128 GiB — way past any hobby image).
//! No DMA, no interrupts, no overlap: we poll the BSY / DRQ status bits.
//! Calls are wrapped in `without_interrupts` so the scheduler can't preempt
//! us mid-transaction and let a parallel `read_sector` clobber the port
//! state.

use core::arch::asm;
use x86_64::instructions::interrupts;

// Primary IDE channel I/O ports.
const PORT_DATA:         u16 = 0x1F0;
const PORT_ERROR:        u16 = 0x1F1;
const PORT_SECTOR_COUNT: u16 = 0x1F2;
const PORT_LBA_LO:       u16 = 0x1F3;
const PORT_LBA_MID:      u16 = 0x1F4;
const PORT_LBA_HI:       u16 = 0x1F5;
const PORT_DRIVE:        u16 = 0x1F6;
const PORT_STATUS:       u16 = 0x1F7;
const PORT_CMD:          u16 = 0x1F7;
const PORT_DEV_CTRL:     u16 = 0x3F6; // device control / alt-status

// ATA commands.
const CMD_READ_SECTORS: u8 = 0x20;

// Status register bits.
const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_DF:  u8 = 0x20;
const STATUS_BSY: u8 = 0x80;

/// Which drive on the primary channel.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Master is part of the public API even though we only mount the slave for now.
pub enum Drive {
    Master,
    Slave,
}

#[derive(Debug)]
#[allow(dead_code)] // variants surface through the Debug impl
pub enum AtaError {
    /// Drive raised the DF (drive-fault) bit.
    DriveFault,
    /// Drive raised the ERR bit.
    Error,
    /// Polling loop hit its iteration cap without the expected status.
    Timeout,
}

#[inline] unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}
#[inline] unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    val
}
#[inline] unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    asm!("in ax, dx", out("ax") val, in("dx") port, options(nomem, nostack));
    val
}

/// Read one 512-byte sector at `lba` from `drive` into `buf`.
///
/// Atomic w.r.t. the scheduler — we disable interrupts for the duration so
/// the transfer can't be torn by a context-switch into a concurrent reader.
pub fn read_sector(drive: Drive, lba: u32, buf: &mut [u8; 512]) -> Result<(), AtaError> {
    assert!(lba < (1 << 28), "LBA exceeds LBA28 range");

    interrupts::without_interrupts(|| read_sector_inner(drive, lba, buf))
}

fn read_sector_inner(drive: Drive, lba: u32, buf: &mut [u8; 512]) -> Result<(), AtaError> {
    // Set nIEN so the IDE controller won't raise IRQ14 after completion.
    // Idempotent — costs one outb per call, simpler than tracking init state.
    unsafe { outb(PORT_DEV_CTRL, 0x02); }

    poll_bsy_clear()?;

    // Bits 7..4: 0b1110 for master / 0b1111 for slave (LBA mode + reserved).
    // Bits 3..0: top 4 bits of LBA.
    let drive_byte: u8 = match drive {
        Drive::Master => 0xE0 | ((lba >> 24) as u8 & 0x0F),
        Drive::Slave  => 0xF0 | ((lba >> 24) as u8 & 0x0F),
    };

    unsafe {
        outb(PORT_DRIVE, drive_byte);
        // 400 ns "select-drive" delay — four status reads is the ATA-spec idiom.
        for _ in 0..4 { let _ = inb(PORT_STATUS); }

        outb(PORT_ERROR,        0);
        outb(PORT_SECTOR_COUNT, 1);
        outb(PORT_LBA_LO,  lba        as u8);
        outb(PORT_LBA_MID, (lba >> 8) as u8);
        outb(PORT_LBA_HI,  (lba >> 16) as u8);
        outb(PORT_CMD, CMD_READ_SECTORS);
    }

    poll_drq_set()?;

    unsafe {
        // Transfer 256 little-endian words = 512 bytes.
        for i in 0..256 {
            let w = inw(PORT_DATA);
            buf[i * 2]     = (w & 0xFF) as u8;
            buf[i * 2 + 1] = (w >> 8)   as u8;
        }
    }

    Ok(())
}

fn poll_bsy_clear() -> Result<(), AtaError> {
    for _ in 0..1_000_000 {
        let s = unsafe { inb(PORT_STATUS) };
        if s & STATUS_BSY == 0 { return Ok(()); }
    }
    Err(AtaError::Timeout)
}

fn poll_drq_set() -> Result<(), AtaError> {
    for _ in 0..1_000_000 {
        let s = unsafe { inb(PORT_STATUS) };
        if s & STATUS_ERR != 0 { return Err(AtaError::Error); }
        if s & STATUS_DF  != 0 { return Err(AtaError::DriveFault); }
        if s & STATUS_BSY == 0 && s & STATUS_DRQ != 0 { return Ok(()); }
    }
    Err(AtaError::Timeout)
}
