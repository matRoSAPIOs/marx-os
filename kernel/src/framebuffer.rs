//! Tiny framebuffer console: 8×8 bitmap font rendered at 2× vertical scale
//! (effective 8×16 cell). Supports RGB / BGR pixel formats from bootloader 0.11.

use core::ptr;
use alloc::vec;
use alloc::vec::Vec;
use bootloader_api::info::{FrameBuffer, PixelFormat};
use spin::Mutex;

use crate::font::FONT;

// ---------- character cell ----------

const CHAR_W: usize = 8;
const CHAR_H: usize = 16; // 8 font rows × 2

// ---------- palette ----------
// Tuple is (R, G, B) regardless of memory layout — the writer swaps for BGR.

// Default palette. Both colours are mutable per-FbWriter so the panic handler
// can repaint into a BSOD-style scheme without touching the rest of the code.
// NOTE: the DEFAULT_BG value here must match `BG_R/G/B` in `kernel/build.rs` —
// splash composites the logo's alpha against that same matte.
const DEFAULT_BG: (u8, u8, u8) = (0x00, 0x00, 0x00);
const DEFAULT_FG: (u8, u8, u8) = (0xE6, 0xED, 0xF3);

// ---------- global writer ----------

static WRITER: Mutex<Option<FbWriter>> = Mutex::new(None);

struct FbWriter {
    /// Active draw target. Starts pointing at `fb_addr` (the hardware
    /// framebuffer). After `init_backbuffer()` it's repointed at the bb data
    /// so that `put_pixel` and friends write to RAM instead of MMIO.
    buf: *mut u8,
    /// Hardware framebuffer address. Saved separately so `present()` can
    /// memcpy from the backbuffer back onto the screen.
    fb_addr: *mut u8,
    /// Backbuffer storage. `None` until `init_backbuffer()` is called
    /// (which has to happen after the heap is initialised).
    bb: Option<Vec<u8>>,
    /// Total buffer size in bytes (== `height * stride_bytes`). Same for
    /// both bb and fb — they share dimensions.
    byte_len: usize,

    stride_bytes: usize, // bytes from one pixel row to the next
    width: usize,        // visible pixels per row
    height: usize,       // visible pixel rows
    bpp: usize,          // bytes per pixel
    bgr: bool,           // true => BGR memory order, false => RGB
    cols: usize,         // text columns
    rows: usize,         // text rows
    col: usize,
    row: usize,
    bg: (u8, u8, u8),    // active background colour
    fg: (u8, u8, u8),    // active foreground colour
}

// `*mut u8` is not Send/Sync; we only touch WRITER from a single CPU in Phase 2.
unsafe impl Send for FbWriter {}
unsafe impl Sync for FbWriter {}

/// Initialise the global text console using the bootloader-provided framebuffer.
/// Clears the screen to the background colour.
pub fn init(fb: &mut FrameBuffer) {
    let info = fb.info();
    let buf_ptr = fb.buffer_mut().as_mut_ptr();

    let bgr = matches!(info.pixel_format, PixelFormat::Bgr);
    let cols = info.width / CHAR_W;
    let rows = info.height / CHAR_H;

    let stride_bytes = info.stride * info.bytes_per_pixel;
    let mut w = FbWriter {
        buf: buf_ptr,
        fb_addr: buf_ptr,
        bb: None,
        byte_len: info.height * stride_bytes,
        stride_bytes,
        width: info.width,
        height: info.height,
        bpp: info.bytes_per_pixel,
        bgr,
        cols,
        rows,
        col: 0,
        row: 0,
        bg: DEFAULT_BG,
        fg: DEFAULT_FG,
    };

    clear_screen(&mut w);

    *WRITER.lock() = Some(w);
}

/// Write a raw string to the framebuffer console. No-op if framebuffer is
/// uninitialised. The caller is responsible for disabling interrupts around
/// any sequence that must be atomic — see `DualWriter` in `main.rs`.
pub fn write_str(s: &str) {
    if let Some(w) = WRITER.lock().as_mut() {
        for c in s.chars() {
            w.put_char(c);
        }
    }
}

/// Allocate the backbuffer and switch all subsequent drawing to write there
/// instead of straight into the hardware framebuffer. Idempotent.
///
/// Must be called AFTER the global heap is up. Once installed, every text
/// write / blit / pixel set lands in the bb; nothing reaches the screen
/// until `present()` is called.
///
/// We seed the bb with a copy of the current framebuffer so the visible
/// state doesn't blank out at the moment of installation.
pub fn init_backbuffer() {
    let mut guard = WRITER.lock();
    let Some(w) = guard.as_mut() else { return; };
    if w.bb.is_some() { return; }

    let mut bb: Vec<u8> = vec![0u8; w.byte_len];
    // SAFETY: fb_addr and bb are non-overlapping; both are sized to byte_len.
    unsafe {
        ptr::copy_nonoverlapping(w.fb_addr, bb.as_mut_ptr(), w.byte_len);
    }
    w.buf = bb.as_mut_ptr();
    w.bb = Some(bb);
}

/// Flush the backbuffer to the hardware framebuffer in one shot. No-op if
/// the backbuffer hasn't been installed yet (early-boot drawing goes
/// straight to the screen, so a present would be redundant).
///
/// Called from the timer IRQ (~18 Hz) and from the mouse IRQ (so cursor
/// motion feels immediate).
pub fn present() {
    let guard = WRITER.lock();
    let Some(w) = guard.as_ref() else { return; };
    if w.bb.is_none() { return; }
    // SAFETY: w.buf currently points to bb data, w.fb_addr to MMIO, no overlap.
    unsafe {
        ptr::copy_nonoverlapping(w.buf, w.fb_addr, w.byte_len);
    }
}

/// Wipe the screen back to the background colour and reset the text cursor.
pub fn clear() {
    if let Some(w) = WRITER.lock().as_mut() {
        clear_screen(w);
        w.col = 0;
        w.row = 0;
    }
}

/// Forcibly release the WRITER mutex. Reserved for soft-recovery / "kill task"
/// paths — unused at the moment but kept ready.
#[allow(dead_code)]
pub unsafe fn panic_unlock() {
    WRITER.force_unlock();
}

/// Ensure the next println starts on a fresh line.
#[allow(dead_code)]
pub fn ensure_newline() {
    let need = WRITER.lock().as_ref().map(|w| w.col > 0).unwrap_or(false);
    if need {
        write_str("\n");
    }
}

/// Screen dimensions in pixels, or `None` if framebuffer isn't initialised yet.
pub fn dimensions() -> Option<(usize, usize)> {
    WRITER.lock().as_ref().map(|w| (w.width, w.height))
}

/// Solid-colour rectangle. Clipped to screen.
pub fn fill_rect(x: usize, y: usize, width: usize, height: usize, color: (u8, u8, u8)) {
    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        for yy in y..y_end {
            for xx in x..x_end {
                put_pixel(w, xx, yy, color.0, color.1, color.2);
            }
        }
    }
}

/// Vertical gradient between two colours over rows `y_start..y_end`. Linear
/// interpolation per row. Cheap (no per-pixel arithmetic — colour computed
/// once per row, then used for the whole row).
pub fn fill_gradient_v(y_start: usize, y_end: usize, top: (u8, u8, u8), bottom: (u8, u8, u8)) {
    if y_end <= y_start { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        let band_h = y_end - y_start;
        let last = (y_end.min(w.height)).saturating_sub(1);
        for yy in y_start..=last {
            // t goes from 0 at y_start to 256 at y_end (exclusive end-cap)
            let t = ((yy - y_start) as u32) * 256 / band_h as u32;
            let inv = 256 - t;
            let r = ((top.0 as u32 * inv + bottom.0 as u32 * t) >> 8) as u8;
            let g = ((top.1 as u32 * inv + bottom.1 as u32 * t) >> 8) as u8;
            let b = ((top.2 as u32 * inv + bottom.2 as u32 * t) >> 8) as u8;
            for xx in 0..w.width {
                put_pixel(w, xx, yy, r, g, b);
            }
        }
    }
}

/// Draw a string at an absolute pixel position with the given foreground.
/// Transparent background — only the "on" bits of each glyph get written,
/// leaving the surrounding pixels untouched. Bitmap-font version; for
/// antialiased output use `ttf::draw_text` instead.
#[allow(dead_code)] // kept as a fallback / debug path; TTF is preferred now
pub fn draw_text_at(x: usize, y: usize, text: &str, fg: (u8, u8, u8)) {
    if let Some(w) = WRITER.lock().as_mut() {
        for (i, ch) in text.bytes().enumerate() {
            draw_glyph_transparent(w, ch, x + i * CHAR_W, y, fg);
        }
    }
}

#[allow(dead_code)]
fn draw_glyph_transparent(w: &mut FbWriter, c: u8, px0: usize, py0: usize, fg: (u8, u8, u8)) {
    let idx = if (0x20..=0x7F).contains(&c) { (c - 0x20) as usize } else { 0 };
    let glyph = &FONT[idx];
    for gy in 0..8usize {
        let bits = glyph[gy];
        for dy in 0..2usize {
            let y = py0 + gy * 2 + dy;
            if y >= w.height { return; }
            for gx in 0..8usize {
                let x = px0 + gx;
                if x >= w.width { break; }
                if (bits >> (7 - gx)) & 1 != 0 {
                    put_pixel(w, x, y, fg.0, fg.1, fg.2);
                }
            }
        }
    }
}

/// Set the active background colour. New text and subsequent `clear()` calls
/// will paint with this colour.
#[allow(dead_code)] // exposed for future GUI / theming code
pub fn set_bg(r: u8, g: u8, b: u8) {
    if let Some(w) = WRITER.lock().as_mut() {
        w.bg = (r, g, b);
    }
}

/// Set the active foreground (text) colour.
#[allow(dead_code)] // exposed for future GUI / theming code
pub fn set_fg(r: u8, g: u8, b: u8) {
    if let Some(w) = WRITER.lock().as_mut() {
        w.fg = (r, g, b);
    }
}

/// Fill the entire framebuffer with a flat RGB colour and reset the text
/// cursor. Used by the splash to paint a white matte under the logo.
pub fn fill_color(r: u8, g: u8, b: u8) {
    if let Some(w) = WRITER.lock().as_mut() {
        for y in 0..w.height {
            for x in 0..w.width {
                put_pixel(w, x, y, r, g, b);
            }
        }
        w.col = 0;
        w.row = 0;
    }
}

/// Blit a packed-RGB image (one R/G/B triplet per pixel, row-major) into the
/// framebuffer, centred. Out-of-bounds pixels are clipped silently. No-op if
/// the framebuffer is uninitialised.
#[allow(dead_code)] // RGB variant kept around; current logo path uses RGBA
pub fn blit_rgb_centered(rgb: &[u8], img_w: usize, img_h: usize) {
    if rgb.len() < img_w * img_h * 3 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        let top_x = w.width.saturating_sub(img_w) / 2;
        let top_y = w.height.saturating_sub(img_h) / 2;
        blit_rgb(w, rgb, img_w, img_h, top_x, top_y);
    }
}

/// Blit a packed-RGB image at an explicit (top_x, top_y). Clipped to screen.
#[allow(dead_code)] // kept for future widgets / screenshot tools
pub fn blit_rgb_at(rgb: &[u8], img_w: usize, img_h: usize, top_x: usize, top_y: usize) {
    if rgb.len() < img_w * img_h * 3 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        blit_rgb(w, rgb, img_w, img_h, top_x, top_y);
    }
}

/// Blit a packed-RGB image clipped to a specific row range [y_start, y_end).
/// Used by the desktop to repaint only the wallpaper above the taskbar.
pub fn blit_rgb_rows(rgb: &[u8], img_w: usize, img_h: usize, y_start: usize, y_end: usize) {
    if rgb.len() < img_w * img_h * 3 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        let y0 = y_start.min(w.height).min(img_h);
        let y1 = y_end.min(w.height).min(img_h);
        for y in y0..y1 {
            let row_off = y * img_w * 3;
            for x in 0..img_w.min(w.width) {
                let off = row_off + x * 3;
                put_pixel(w, x, y, rgb[off], rgb[off + 1], rgb[off + 2]);
            }
        }
    }
}

/// Alpha-blend an RGBA image onto the framebuffer at (top_x, top_y).
/// `rgba` is row-major, 4 bytes per pixel (R,G,B,A).
///
/// For each source pixel:
///   out = (src.rgb * src.a + bg.rgb * (255 - src.a)) / 255
///
/// Fully transparent pixels (a=0) are skipped; fully opaque (a=255) are
/// written directly. Antialiased edges blend smoothly with whatever was
/// already on screen.
pub fn blit_rgba_at(rgba: &[u8], img_w: usize, img_h: usize, top_x: usize, top_y: usize) {
    if rgba.len() < img_w * img_h * 4 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        for yy in 0..img_h {
            let dy = top_y + yy;
            if dy >= w.height { break; }
            for xx in 0..img_w {
                let dx = top_x + xx;
                if dx >= w.width { break; }
                let so = (yy * img_w + xx) * 4;
                let sr = rgba[so];
                let sg = rgba[so + 1];
                let sb = rgba[so + 2];
                let sa = rgba[so + 3] as u32;
                if sa == 0 { continue; }
                if sa == 255 {
                    put_pixel(w, dx, dy, sr, sg, sb);
                    continue;
                }
                // Read current destination pixel for alpha-blending.
                let dst_off = dy * w.stride_bytes + dx * w.bpp;
                unsafe {
                    let p = w.buf.add(dst_off);
                    let (dr, dg, db) = if w.bgr {
                        (*p.add(2), *p.add(1), *p)
                    } else {
                        (*p, *p.add(1), *p.add(2))
                    };
                    let inv = 255 - sa;
                    let r = ((sr as u32 * sa + dr as u32 * inv) / 255) as u8;
                    let g = ((sg as u32 * sa + dg as u32 * inv) / 255) as u8;
                    let b = ((sb as u32 * sa + db as u32 * inv) / 255) as u8;
                    put_pixel(w, dx, dy, r, g, b);
                }
            }
        }
    }
}

/// Alpha-blend an RGBA image centred on screen.
pub fn blit_rgba_centered(rgba: &[u8], img_w: usize, img_h: usize) {
    if let Some((sw, sh)) = dimensions() {
        let top_x = sw.saturating_sub(img_w) / 2;
        let top_y = sh.saturating_sub(img_h) / 2;
        blit_rgba_at(rgba, img_w, img_h, top_x, top_y);
    }
}

/// Read a rectangular region of the framebuffer into a packed-RGB buffer.
/// `out` must be at least `img_w * img_h * 3` bytes. Off-screen pixels are
/// silently skipped (their slots in `out` are left untouched).
#[allow(dead_code)] // window manager / screenshot tools will use this
pub fn read_rgb_at(out: &mut [u8], img_w: usize, img_h: usize, x: usize, y: usize) {
    if out.len() < img_w * img_h * 3 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        for yy in 0..img_h {
            let dy = y + yy;
            if dy >= w.height { break; }
            for xx in 0..img_w {
                let dx = x + xx;
                if dx >= w.width { break; }
                let off = dy * w.stride_bytes + dx * w.bpp;
                unsafe {
                    let p = w.buf.add(off);
                    let (r, g, b) = if w.bgr {
                        (*p.add(2), *p.add(1), *p)
                    } else {
                        (*p, *p.add(1), *p.add(2))
                    };
                    let out_off = (yy * img_w + xx) * 3;
                    out[out_off]     = r;
                    out[out_off + 1] = g;
                    out[out_off + 2] = b;
                }
            }
        }
    }
}

/// Write one pixel at (x, y). Silently no-op if out of bounds.
#[allow(dead_code)] // window manager / drawing primitives will use this
pub fn put_pixel_at(x: usize, y: usize, r: u8, g: u8, b: u8) {
    if let Some(w) = WRITER.lock().as_mut() {
        if x < w.width && y < w.height {
            put_pixel(w, x, y, r, g, b);
        }
    }
}

/// Alpha-blend a single pixel: result = src*alpha + dst*(255-alpha) / 255.
/// Used by the TTF rasteriser to anti-alias glyph edges against whatever
/// background is already on screen.
pub fn blend_pixel_at(x: usize, y: usize, color: (u8, u8, u8), alpha: u8) {
    if alpha == 0 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        if x >= w.width || y >= w.height { return; }
        if alpha == 255 {
            put_pixel(w, x, y, color.0, color.1, color.2);
            return;
        }
        let off = y * w.stride_bytes + x * w.bpp;
        unsafe {
            let p = w.buf.add(off);
            let (dr, dg, db) = if w.bgr {
                (*p.add(2), *p.add(1), *p)
            } else {
                (*p, *p.add(1), *p.add(2))
            };
            let a = alpha as u32;
            let inv = 255 - a;
            let r = ((color.0 as u32 * a + dr as u32 * inv) / 255) as u8;
            let g = ((color.1 as u32 * a + dg as u32 * inv) / 255) as u8;
            let b = ((color.2 as u32 * a + db as u32 * inv) / 255) as u8;
            put_pixel(w, x, y, r, g, b);
        }
    }
}

/// Inline single-pixel blend helper used by batch alpha primitives below.
/// Takes a *locked* writer reference — does NOT lock WRITER itself.
#[inline(always)]
fn blend_pixel_into(w: &FbWriter, x: usize, y: usize, color: (u8, u8, u8), alpha: u8) {
    if alpha == 0 { return; }
    if x >= w.width || y >= w.height { return; }
    if alpha == 255 {
        put_pixel(w, x, y, color.0, color.1, color.2);
        return;
    }
    let off = y * w.stride_bytes + x * w.bpp;
    unsafe {
        let p = w.buf.add(off);
        let (dr, dg, db) = if w.bgr {
            (*p.add(2), *p.add(1), *p)
        } else {
            (*p, *p.add(1), *p.add(2))
        };
        let a = alpha as u32;
        let inv = 255 - a;
        let r = ((color.0 as u32 * a + dr as u32 * inv) / 255) as u8;
        let g = ((color.1 as u32 * a + dg as u32 * inv) / 255) as u8;
        let b = ((color.2 as u32 * a + db as u32 * inv) / 255) as u8;
        put_pixel(w, x, y, r, g, b);
    }
}

/// Alpha-blend a solid colour over a plain (non-rounded) rectangle, with a
/// single WRITER lock. Handy for thin shadow lines under separators etc.
pub fn blend_rect(
    x: usize, y: usize, width: usize, height: usize,
    color: (u8, u8, u8), alpha: u8,
) {
    if width == 0 || height == 0 || alpha == 0 { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        for yy in y..y_end {
            for xx in x..x_end {
                blend_pixel_into(w, xx, yy, color, alpha);
            }
        }
    }
}

/// Alpha-blend a solid colour over a rounded-corner rectangle, with a single
/// WRITER lock. Used for soft drop shadows and glass overlays.
pub fn blend_rounded_rect(
    x: usize, y: usize, width: usize, height: usize,
    color: (u8, u8, u8), alpha: u8, radius: usize,
) {
    if width == 0 || height == 0 || alpha == 0 { return; }
    let r = radius.min(width / 2).min(height / 2);
    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        let r_sq = (r * r) as i32;
        for yy in y..y_end {
            for xx in x..x_end {
                let local_x = (xx - x) as i32;
                let local_y = (yy - y) as i32;
                let wi = width as i32;
                let hi = height as i32;
                let ri = r as i32;
                let in_left   = local_x < ri;
                let in_right  = local_x >= wi - ri;
                let in_top    = local_y < ri;
                let in_bottom = local_y >= hi - ri;
                if (in_left || in_right) && (in_top || in_bottom) {
                    let cx = if in_left  { ri } else { wi - ri - 1 };
                    let cy = if in_top   { ri } else { hi - ri - 1 };
                    let dx = local_x - cx;
                    let dy = local_y - cy;
                    if dx * dx + dy * dy > r_sq { continue; }
                }
                blend_pixel_into(w, xx, yy, color, alpha);
            }
        }
    }
}

/// Alpha-blend a vertical gradient over a band, clipped by an OUTER rounded
/// rect mask. Both colour AND alpha can interpolate between top and bottom.
/// Single WRITER lock — efficient for glass title bars (hundreds of pixels).
pub fn blend_v_gradient_in_rounded_rect(
    band_x: usize, band_y: usize, band_w: usize, band_h: usize,
    top: (u8, u8, u8), bot: (u8, u8, u8),
    top_alpha: u8, bot_alpha: u8,
    mask_x: usize, mask_y: usize, mask_w: usize, mask_h: usize, mask_radius: usize,
) {
    if band_w == 0 || band_h == 0 { return; }
    let mr = mask_radius.min(mask_w / 2).min(mask_h / 2);
    let mwi = mask_w as i32;
    let mhi = mask_h as i32;
    let mri = mr as i32;
    let r_sq = (mr * mr) as i32;

    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (band_x + band_w).min(w.width);
        let y_end = (band_y + band_h).min(w.height);
        for yy in band_y..y_end {
            let row = yy - band_y;
            let t = (row as u32 * 256 / band_h as u32) as u32;
            let inv = 256 - t;
            let r = ((top.0 as u32 * inv + bot.0 as u32 * t) >> 8) as u8;
            let g = ((top.1 as u32 * inv + bot.1 as u32 * t) >> 8) as u8;
            let b = ((top.2 as u32 * inv + bot.2 as u32 * t) >> 8) as u8;
            let a = ((top_alpha as u32 * inv + bot_alpha as u32 * t) >> 8) as u8;
            if a == 0 { continue; }
            for xx in band_x..x_end {
                let lx = xx as i32 - mask_x as i32;
                let ly = yy as i32 - mask_y as i32;
                if lx < 0 || ly < 0 || lx >= mwi || ly >= mhi { continue; }
                let in_left   = lx < mri;
                let in_right  = lx >= mwi - mri;
                let in_top    = ly < mri;
                let in_bottom = ly >= mhi - mri;
                if (in_left || in_right) && (in_top || in_bottom) {
                    let cx = if in_left  { mri } else { mwi - mri - 1 };
                    let cy = if in_top   { mri } else { mhi - mri - 1 };
                    let dx = lx - cx;
                    let dy = ly - cy;
                    if dx * dx + dy * dy > r_sq { continue; }
                }
                blend_pixel_into(w, xx, yy, (r, g, b), a);
            }
        }
    }
}

/// Solid-colour rectangle with rounded BOTTOM corners only — top edge is
/// flat.  Used for window bodies that sit underneath a glass title bar
/// (the title bar handles the top rounding).
#[allow(dead_code)] // gradient variant is what windows use; solid kept handy
pub fn fill_rect_rounded_bottom(
    x: usize, y: usize, width: usize, height: usize,
    color: (u8, u8, u8), radius: usize,
) {
    if width == 0 || height == 0 { return; }
    let r = radius.min(width / 2).min(height / 2);
    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        let r_sq = (r * r) as i32;
        for yy in y..y_end {
            for xx in x..x_end {
                let local_x = (xx - x) as i32;
                let local_y = (yy - y) as i32;
                let wi = width as i32;
                let hi = height as i32;
                let ri = r as i32;
                let in_left   = local_x < ri;
                let in_right  = local_x >= wi - ri;
                let in_bottom = local_y >= hi - ri;
                // Only the two bottom corners are rounded.
                if in_bottom && (in_left || in_right) {
                    let cx = if in_left { ri } else { wi - ri - 1 };
                    let cy = hi - ri - 1;
                    let dx = local_x - cx;
                    let dy = local_y - cy;
                    if dx * dx + dy * dy > r_sq { continue; }
                }
                put_pixel(w, xx, yy, color.0, color.1, color.2);
            }
        }
    }
}

/// Vertical-gradient rectangle with rounded BOTTOM corners only.
pub fn fill_v_gradient_rect_rounded_bottom(
    x: usize, y: usize, width: usize, height: usize,
    top: (u8, u8, u8), bot: (u8, u8, u8), radius: usize,
) {
    if width == 0 || height == 0 { return; }
    let r = radius.min(width / 2).min(height / 2);
    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        let r_sq = (r * r) as i32;
        for yy in y..y_end {
            let row = yy - y;
            let t = (row as u32 * 256 / height as u32) as u32;
            let inv = 256 - t;
            let r_c = ((top.0 as u32 * inv + bot.0 as u32 * t) >> 8) as u8;
            let g_c = ((top.1 as u32 * inv + bot.1 as u32 * t) >> 8) as u8;
            let b_c = ((top.2 as u32 * inv + bot.2 as u32 * t) >> 8) as u8;
            for xx in x..x_end {
                let local_x = (xx - x) as i32;
                let local_y = (yy - y) as i32;
                let wi = width as i32;
                let hi = height as i32;
                let ri = r as i32;
                let in_left   = local_x < ri;
                let in_right  = local_x >= wi - ri;
                let in_bottom = local_y >= hi - ri;
                if in_bottom && (in_left || in_right) {
                    let cx = if in_left { ri } else { wi - ri - 1 };
                    let cy = hi - ri - 1;
                    let dx = local_x - cx;
                    let dy = local_y - cy;
                    if dx * dx + dy * dy > r_sq { continue; }
                }
                put_pixel(w, xx, yy, r_c, g_c, b_c);
            }
        }
    }
}

/// Solid-colour rounded-corner rectangle. `radius` is clamped to `min(w,h)/2`.
/// Implemented by filling the body and then knocking out the four corners
/// via distance-from-corner-centre test.
pub fn fill_rounded_rect(
    x: usize, y: usize, width: usize, height: usize,
    color: (u8, u8, u8), radius: usize,
) {
    if width == 0 || height == 0 { return; }
    let r = radius.min(width / 2).min(height / 2);

    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        let r_sq = (r * r) as i32;
        for yy in y..y_end {
            for xx in x..x_end {
                // Decide if (xx, yy) is inside one of the four rounded corners
                // and outside the inscribed quarter-circle — if so, skip.
                let local_x = (xx - x) as i32;
                let local_y = (yy - y) as i32;
                let w_i32 = width as i32;
                let h_i32 = height as i32;
                let r_i32 = r as i32;

                let in_left   = local_x < r_i32;
                let in_right  = local_x >= w_i32 - r_i32;
                let in_top    = local_y < r_i32;
                let in_bottom = local_y >= h_i32 - r_i32;

                if (in_left || in_right) && (in_top || in_bottom) {
                    let cx = if in_left  { r_i32 } else { w_i32 - r_i32 - 1 };
                    let cy = if in_top   { r_i32 } else { h_i32 - r_i32 - 1 };
                    let dx = local_x - cx;
                    let dy = local_y - cy;
                    if dx * dx + dy * dy > r_sq { continue; }
                }
                put_pixel(w, xx, yy, color.0, color.1, color.2);
            }
        }
    }
}

/// Vertical gradient over `grad_*` rectangle, clipped to a rounded-rect mask
/// `mask_*`. Single WRITER lock — efficient for button glossy fills.
pub fn fill_v_gradient_in_rounded_rect(
    grad_x: usize, grad_y: usize, grad_w: usize, grad_h: usize,
    top: (u8, u8, u8), bot: (u8, u8, u8),
    mask_x: usize, mask_y: usize, mask_w: usize, mask_h: usize, mask_radius: usize,
) {
    if grad_w == 0 || grad_h == 0 { return; }
    let mr = mask_radius.min(mask_w / 2).min(mask_h / 2);
    let mw = mask_w as i32;
    let mh = mask_h as i32;
    let mr_i32 = mr as i32;
    let r_sq = (mr * mr) as i32;

    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (grad_x + grad_w).min(w.width);
        let y_end = (grad_y + grad_h).min(w.height);
        for yy in grad_y..y_end {
            let row = yy - grad_y;
            let t = (row as u32 * 256 / grad_h as u32) as u32;
            let inv = 256 - t;
            let r = ((top.0 as u32 * inv + bot.0 as u32 * t) >> 8) as u8;
            let g = ((top.1 as u32 * inv + bot.1 as u32 * t) >> 8) as u8;
            let b = ((top.2 as u32 * inv + bot.2 as u32 * t) >> 8) as u8;
            for xx in grad_x..x_end {
                // Mask test against the OUTER rounded rect.
                let lx = xx as i32 - mask_x as i32;
                let ly = yy as i32 - mask_y as i32;
                if lx < 0 || ly < 0 || lx >= mw || ly >= mh { continue; }
                let in_left   = lx < mr_i32;
                let in_right  = lx >= mw - mr_i32;
                let in_top    = ly < mr_i32;
                let in_bottom = ly >= mh - mr_i32;
                if (in_left || in_right) && (in_top || in_bottom) {
                    let cx = if in_left  { mr_i32 } else { mw - mr_i32 - 1 };
                    let cy = if in_top   { mr_i32 } else { mh - mr_i32 - 1 };
                    let dx = lx - cx;
                    let dy = ly - cy;
                    if dx * dx + dy * dy > r_sq { continue; }
                }
                put_pixel(w, xx, yy, r, g, b);
            }
        }
    }
}

/// Outline a rounded rectangle (1px thick). Companion to `fill_rounded_rect`.
pub fn stroke_rounded_rect(
    x: usize, y: usize, width: usize, height: usize,
    color: (u8, u8, u8), radius: usize,
) {
    if width == 0 || height == 0 { return; }
    let r = radius.min(width / 2).min(height / 2);

    if let Some(w) = WRITER.lock().as_mut() {
        let x_end = (x + width).min(w.width);
        let y_end = (y + height).min(w.height);
        let r_sq = (r * r) as i32;
        let r_inner_sq = ((r as i32 - 1).max(0)).pow(2);
        for yy in y..y_end {
            for xx in x..x_end {
                let local_x = (xx - x) as i32;
                let local_y = (yy - y) as i32;
                let w_i32 = width as i32;
                let h_i32 = height as i32;
                let r_i32 = r as i32;

                let in_left   = local_x < r_i32;
                let in_right  = local_x >= w_i32 - r_i32;
                let in_top    = local_y < r_i32;
                let in_bottom = local_y >= h_i32 - r_i32;

                if (in_left || in_right) && (in_top || in_bottom) {
                    // In a corner region: pixel belongs to outline if the
                    // distance from the corner centre falls in [r-1, r].
                    let cx = if in_left  { r_i32 } else { w_i32 - r_i32 - 1 };
                    let cy = if in_top   { r_i32 } else { h_i32 - r_i32 - 1 };
                    let dx = local_x - cx;
                    let dy = local_y - cy;
                    let d_sq = dx * dx + dy * dy;
                    if d_sq <= r_sq && d_sq >= r_inner_sq {
                        put_pixel(w, xx, yy, color.0, color.1, color.2);
                    }
                } else {
                    // Straight edge — outline is the outer 1-pixel ring.
                    let on_edge = local_x == 0 || local_x == w_i32 - 1
                               || local_y == 0 || local_y == h_i32 - 1;
                    if on_edge {
                        put_pixel(w, xx, yy, color.0, color.1, color.2);
                    }
                }
            }
        }
    }
}

/// Atomic cursor repaint: restore old position → capture new background →
/// draw the bitmap, all under a single WRITER lock. Text writes cannot
/// interleave between the steps, so we can't leave a "ghost" cursor behind
/// from a stale saved background.
///
/// `bitmap` is a row-major array of byte strings, each of length `width`:
///   `*` → fg colour, `.` → border colour, anything else → transparent.
pub fn paint_cursor(
    saved_bg: &mut [u8],
    old_pos: Option<(i32, i32)>,
    new_pos: (i32, i32),
    bitmap: &[&[u8]],
    width: usize,
    height: usize,
    fg: (u8, u8, u8),
    border: (u8, u8, u8),
) {
    if old_pos == Some(new_pos) { return; }
    if let Some(w) = WRITER.lock().as_mut() {
        // 1. Restore old position
        if let Some((ox, oy)) = old_pos {
            if ox >= 0 && oy >= 0 {
                blit_rgb(w, saved_bg, width, height, ox as usize, oy as usize);
            }
        }

        let (nx, ny) = (new_pos.0.max(0) as usize, new_pos.1.max(0) as usize);

        // 2. Capture pre-cursor background at new position. Clear the saved
        //    buffer first so out-of-bounds (cursor near edge) doesn't leave
        //    stale bytes that would corrupt a future restore.
        for byte in saved_bg.iter_mut() { *byte = 0; }
        for yy in 0..height {
            let dy = ny + yy;
            if dy >= w.height { break; }
            for xx in 0..width {
                let dx = nx + xx;
                if dx >= w.width { break; }
                let off = dy * w.stride_bytes + dx * w.bpp;
                unsafe {
                    let p = w.buf.add(off);
                    let (r, g, b) = if w.bgr {
                        (*p.add(2), *p.add(1), *p)
                    } else {
                        (*p, *p.add(1), *p.add(2))
                    };
                    let out_off = (yy * width + xx) * 3;
                    saved_bg[out_off]     = r;
                    saved_bg[out_off + 1] = g;
                    saved_bg[out_off + 2] = b;
                }
            }
        }

        // 3. Paint the bitmap on top.
        for (row, line) in bitmap.iter().enumerate() {
            for (col, &ch) in line.iter().enumerate() {
                let px = nx + col;
                let py = ny + row;
                if px >= w.width || py >= w.height { continue; }
                match ch {
                    b'*' => put_pixel(w, px, py, fg.0,     fg.1,     fg.2),
                    b'.' => put_pixel(w, px, py, border.0, border.1, border.2),
                    _ => {}
                }
            }
        }
    }
}

// ---------- rendering ----------

impl FbWriter {
    fn put_char(&mut self, c: char) {
        match c {
            '\n' => self.newline(),
            '\r' => { self.col = 0; }
            '\t' => {
                // round up to next multiple of 4
                let n = 4 - (self.col % 4);
                for _ in 0..n { self.put_char(' '); }
            }
            '\u{8}' | '\u{7F}' => self.backspace(), // BS or DEL
            ch => {
                if self.col >= self.cols { self.newline(); }
                let b = if (ch as u32) < 0x80 { ch as u8 } else { b'?' };
                draw_glyph(self, b, self.col, self.row);
                self.col += 1;
            }
        }
    }

    fn backspace(&mut self) {
        // Step back one cell, wrap to previous line if needed, then overwrite
        // with a blank glyph. Refuse to back up past (0,0).
        if self.col == 0 {
            if self.row == 0 { return; }
            self.row -= 1;
            self.col = self.cols - 1;
        } else {
            self.col -= 1;
        }
        draw_glyph(self, b' ', self.col, self.row);
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row >= self.rows {
            scroll_up(self);
            self.row = self.rows - 1;
        }
    }
}

#[inline]
fn put_pixel(w: &FbWriter, x: usize, y: usize, r: u8, g: u8, b: u8) {
    let offset = y * w.stride_bytes + x * w.bpp;
    unsafe {
        let p = w.buf.add(offset);
        if w.bgr {
            *p             = b;
            *p.add(1)      = g;
            *p.add(2)      = r;
        } else {
            *p             = r;
            *p.add(1)      = g;
            *p.add(2)      = b;
        }
        if w.bpp >= 4 {
            *p.add(3) = 0x00; // alpha / padding
        }
    }
}

fn draw_glyph(w: &mut FbWriter, c: u8, col: usize, row: usize) {
    let idx = if (0x20..=0x7F).contains(&c) {
        (c - 0x20) as usize
    } else {
        0 // unknown -> space
    };
    let glyph = &FONT[idx];

    let px0 = col * CHAR_W;
    let py0 = row * CHAR_H;

    let fg = w.fg;
    let bg = w.bg;
    for gy in 0..8usize {
        let bits = glyph[gy];
        // vertical doubling: draw each font row twice
        for dy in 0..2usize {
            let y = py0 + gy * 2 + dy;
            if y >= w.height { return; }
            for gx in 0..8usize {
                let x = px0 + gx;
                if x >= w.width { break; }
                let on = (bits >> (7 - gx)) & 1 != 0;
                let (r, g, b) = if on { fg } else { bg };
                put_pixel(w, x, y, r, g, b);
            }
        }
    }
}

/// Blit a packed-RGB image at the given top-left framebuffer coords. Clipped
/// to screen extents. Pixel order in `rgb` is row-major, 3 bytes per pixel.
fn blit_rgb(w: &mut FbWriter, rgb: &[u8], img_w: usize, img_h: usize, top_x: usize, top_y: usize) {
    for y in 0..img_h {
        let dst_y = top_y + y;
        if dst_y >= w.height { break; }
        let row_off = y * img_w * 3;
        for x in 0..img_w {
            let dst_x = top_x + x;
            if dst_x >= w.width { break; }
            let off = row_off + x * 3;
            let r = rgb[off];
            let g = rgb[off + 1];
            let b = rgb[off + 2];
            put_pixel(w, dst_x, dst_y, r, g, b);
        }
    }
}

fn clear_screen(w: &mut FbWriter) {
    let (r, g, b) = w.bg;
    for y in 0..w.height {
        for x in 0..w.width {
            put_pixel(w, x, y, r, g, b);
        }
    }
}

fn clear_text_row(w: &mut FbWriter, text_row: usize) {
    let (r, g, b) = w.bg;
    let py0 = text_row * CHAR_H;
    let py1 = (py0 + CHAR_H).min(w.height);
    for y in py0..py1 {
        for x in 0..w.width {
            put_pixel(w, x, y, r, g, b);
        }
    }
}

fn scroll_up(w: &mut FbWriter) {
    let shift_bytes = CHAR_H * w.stride_bytes;
    let total_bytes = w.height * w.stride_bytes;
    if shift_bytes >= total_bytes { return; }
    unsafe {
        // Overlapping copy: source is above destination, so copy (not copy_nonoverlapping).
        ptr::copy(
            w.buf.add(shift_bytes),
            w.buf,
            total_bytes - shift_bytes,
        );
    }
    clear_text_row(w, w.rows - 1);
}
