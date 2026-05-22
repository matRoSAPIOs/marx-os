//! PS/2 mouse driver.
//!
//! The PS/2 controller has two devices: keyboard (port 1, IRQ1) and mouse
//! (port 2 / "aux", IRQ12). Both share I/O ports 0x60 (data) and 0x64
//! (command/status); we disambiguate by *which* IRQ fired.
//!
//! Init sequence (polling, no IRQs needed):
//!   1. enable aux device
//!   2. read controller config, set "enable IRQ12" + "enable mouse clock"
//!   3. send "set defaults" to mouse, wait for ACK (0xFA)
//!   4. send "enable streaming" to mouse, wait for ACK
//!
//! After enable, the mouse sends 3-byte packets on every movement/click:
//!   byte 0: 1 1 Y_OVF X_OVF Y_SIGN X_SIGN MIDDLE RIGHT LEFT (well, kind of —
//!           bit 3 is always 1; bits 4-5 are sign bits; bits 6-7 overflow)
//!   byte 1: X delta (with sign from byte 0)
//!   byte 2: Y delta (with sign from byte 0) — POSITIVE means up

use core::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use spin::Mutex;

// ----- PS/2 controller ports -----
const DATA_PORT: u16 = 0x60;
const CMD_PORT:  u16 = 0x64;

// ----- Controller commands -----
const CMD_ENABLE_AUX:   u8 = 0xA8; // turn on mouse port
const CMD_READ_CONFIG:  u8 = 0x20; // read config byte
const CMD_WRITE_CONFIG: u8 = 0x60; // write config byte
const CMD_MOUSE_PREFIX: u8 = 0xD4; // "next byte at 0x60 goes to the mouse"

// ----- Mouse commands -----
const MOUSE_SET_DEFAULTS:    u8 = 0xF6;
const MOUSE_ENABLE_STREAMING: u8 = 0xF4;
const MOUSE_ACK: u8 = 0xFA;

// ---------- public state ----------

#[derive(Debug, Clone, Copy, Default)]
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub left:   bool,
    pub right:  bool,
    pub middle: bool,
}

static STATE: Mutex<MouseState> = Mutex::new(MouseState {
    x: 0, y: 0, left: false, right: false, middle: false,
});
static SCREEN_W: AtomicI32 = AtomicI32::new(1280);
static SCREEN_H: AtomicI32 = AtomicI32::new(720);

/// Snapshot the current cursor position and button state.
pub fn state() -> MouseState {
    *STATE.lock()
}

// ---------- init ----------

#[derive(Debug)]
#[allow(dead_code)]
pub enum MouseError {
    Timeout,
    NoAck(u8),
}

/// Configure the PS/2 controller + mouse and start streaming packets.
/// Call AFTER the IDT is loaded so IRQ12 has a handler ready.
pub fn init(screen_w: u32, screen_h: u32) -> Result<(), MouseError> {
    SCREEN_W.store(screen_w as i32, Ordering::Relaxed);
    SCREEN_H.store(screen_h as i32, Ordering::Relaxed);

    {
        // Start cursor at screen centre.
        let mut s = STATE.lock();
        s.x = (screen_w / 2) as i32;
        s.y = (screen_h / 2) as i32;
    }

    // Drain any junk already sitting in the output buffer.
    while unsafe { inb(CMD_PORT) } & 0b01 != 0 {
        let _ = unsafe { inb(DATA_PORT) };
    }

    // 1. Enable the mouse port.
    wait_input_empty()?;
    unsafe { outb(CMD_PORT, CMD_ENABLE_AUX); }

    // 2. Read controller config.
    wait_input_empty()?;
    unsafe { outb(CMD_PORT, CMD_READ_CONFIG); }
    wait_output_full()?;
    let mut config = unsafe { inb(DATA_PORT) };

    //    Enable IRQ12 (bit 1), clear "mouse clock disabled" (bit 5).
    config |=  0b0000_0010;
    config &= !0b0010_0000;

    wait_input_empty()?;
    unsafe { outb(CMD_PORT, CMD_WRITE_CONFIG); }
    wait_input_empty()?;
    unsafe { outb(DATA_PORT, config); }

    // 3. Tell mouse to load its defaults (and ACK).
    send_mouse_cmd(MOUSE_SET_DEFAULTS)?;

    // 4. Start packet streaming.
    send_mouse_cmd(MOUSE_ENABLE_STREAMING)?;

    Ok(())
}

fn send_mouse_cmd(cmd: u8) -> Result<(), MouseError> {
    wait_input_empty()?;
    unsafe { outb(CMD_PORT, CMD_MOUSE_PREFIX); }
    wait_input_empty()?;
    unsafe { outb(DATA_PORT, cmd); }

    wait_output_full()?;
    let ack = unsafe { inb(DATA_PORT) };
    if ack != MOUSE_ACK {
        return Err(MouseError::NoAck(ack));
    }
    Ok(())
}

fn wait_input_empty() -> Result<(), MouseError> {
    for _ in 0..1_000_000 {
        if unsafe { inb(CMD_PORT) } & 0b10 == 0 { return Ok(()); }
    }
    Err(MouseError::Timeout)
}

fn wait_output_full() -> Result<(), MouseError> {
    for _ in 0..1_000_000 {
        if unsafe { inb(CMD_PORT) } & 0b01 != 0 { return Ok(()); }
    }
    Err(MouseError::Timeout)
}

// ---------- IRQ-side packet assembly ----------

static PACKET_BUF: Mutex<[u8; 3]> = Mutex::new([0; 3]);
static PACKET_IDX: AtomicU8 = AtomicU8::new(0);

/// Called from the IRQ12 handler with the byte just read off port 0x60.
/// Assembles 3-byte packets, decodes them into MouseState updates.
pub fn handle_byte(byte: u8) {
    let idx = PACKET_IDX.load(Ordering::Relaxed);

    // First byte sanity: bit 3 must be 1. If not, we're out of sync — drop
    // and stay at idx=0 so we wait for the start of a fresh packet.
    if idx == 0 && (byte & 0b1000) == 0 {
        return;
    }

    {
        let mut buf = PACKET_BUF.lock();
        buf[idx as usize] = byte;
    }
    let new_idx = (idx + 1) % 3;
    PACKET_IDX.store(new_idx, Ordering::Relaxed);

    if new_idx != 0 {
        return; // not a full packet yet
    }

    // Full packet received — decode and apply.
    let (status, raw_x, raw_y) = {
        let buf = PACKET_BUF.lock();
        (buf[0], buf[1], buf[2])
    };

    let dx = if status & 0b0001_0000 != 0 {
        raw_x as i32 - 256
    } else {
        raw_x as i32
    };
    let dy = if status & 0b0010_0000 != 0 {
        raw_y as i32 - 256
    } else {
        raw_y as i32
    };

    let left   = status & 0b0000_0001 != 0;
    let right  = status & 0b0000_0010 != 0;
    let middle = status & 0b0000_0100 != 0;

    let sw = SCREEN_W.load(Ordering::Relaxed);
    let sh = SCREEN_H.load(Ordering::Relaxed);

    let mut s = STATE.lock();
    s.x = (s.x + dx).clamp(0, sw - 1);
    // Mouse Y is "positive = up"; screen Y is "positive = down". Invert.
    s.y = (s.y - dy).clamp(0, sh - 1);
    s.left   = left;
    s.right  = right;
    s.middle = middle;
}

// ---------- port I/O helpers ----------

#[inline] unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al", in("dx") port, in("al") val,
        options(nomem, nostack, preserves_flags));
}
#[inline] unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx", out("al") val, in("dx") port,
        options(nomem, nostack, preserves_flags));
    val
}
