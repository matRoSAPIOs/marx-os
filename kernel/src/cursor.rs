//! Software mouse cursor.
//!
//! Classic 12×19 arrow bitmap. We keep a tiny "background backup" — the
//! pixels currently under the cursor — so that when the mouse moves we can
//! restore the prior screen content and re-paint the arrow at the new spot
//! without leaving a trail.
//!
//! `tick()` is called from the timer IRQ (≈18 Hz on the PIT). That's the
//! upper bound on cursor refresh rate. The mouse driver itself runs off
//! IRQ12, so the latest mouse coordinates are always available — `tick()`
//! just paints whatever the latest state says.
//!
//! Known limitation (will be fixed by the Phase 7.3 compositor): if text
//! is written under the cursor between two ticks, the "saved background"
//! goes stale and the restore step will briefly overwrite that text when
//! the cursor moves away. Acceptable for now — the only artefact is a
//! short visual hiccup if you actively type while the cursor sits in the
//! shell area.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::framebuffer;
use crate::mouse;

const CW: usize = 12;
const CH: usize = 19;

/// Pixel kind for each cell of the arrow:
///   `*` = white fill
///   `.` = black outline
///   ` ` = transparent (no draw)
const BITMAP: [&[u8]; CH] = [
    b"*           ",
    b"**          ",
    b"*.*         ",
    b"*..*        ",
    b"*...*       ",
    b"*....*      ",
    b"*.....*     ",
    b"*......*    ",
    b"*.......*   ",
    b"*........*  ",
    b"*.........* ",
    b"*......*****",
    b"*...*..*    ",
    b"*..**..*    ",
    b"*.*  *..*   ",
    b"**   *..*   ",
    b"*     *..*  ",
    b"      *..*  ",
    b"       **   ",
];

struct Backup {
    /// Pre-cursor pixels at `saved_at`, packed RGB.
    pixels: Vec<u8>,
    /// (x, y) where `pixels` was captured. `None` until the first tick.
    saved_at: Option<(i32, i32)>,
}

static BACKUP: Mutex<Backup> = Mutex::new(Backup {
    pixels: Vec::new(),
    saved_at: None,
});

/// Allocate the background-backup buffer. Must be called after the heap is up.
pub fn init() {
    let mut b = BACKUP.lock();
    b.pixels = vec![0u8; CW * CH * 3];
}

/// Drop the cached "background under cursor" — call this whenever something
/// repaints the screen behind the cursor (welcome screen, window draws, etc).
/// The next `tick()` will treat the cursor's location as untouched and
/// capture a fresh sample instead of restoring stale pre-repaint pixels.
pub fn invalidate() {
    BACKUP.lock().saved_at = None;
}

/// Called from the PIT timer IRQ and from the mouse IRQ. Cheap no-op if the
/// cursor hasn't moved. Otherwise restores the prior background, captures
/// the new background, and paints the arrow — all under a single WRITER
/// lock so text writes can't interleave with the cursor paint and leave
/// "ghost" cursors behind.
pub fn tick() {
    let s = mouse::state();
    let new_pos = (s.x, s.y);

    let mut b = BACKUP.lock();
    if b.pixels.is_empty() {
        return; // init() not called yet
    }
    if b.saved_at == Some(new_pos) {
        return; // nothing changed
    }

    let old_pos = b.saved_at;
    framebuffer::paint_cursor(
        &mut b.pixels,
        old_pos,
        new_pos,
        &BITMAP,
        CW,
        CH,
        (0xFF, 0xFF, 0xFF), // white fill
        (0x00, 0x00, 0x00), // black outline
    );
    b.saved_at = Some(new_pos);
}
