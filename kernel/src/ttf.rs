//! TTF text rendering via `ab_glyph`.
//!
//! Embeds Roboto-Regular at compile time, exposes `draw_text` /
//! `text_width` so other modules can paint Frutiger-Aero-grade text on the
//! framebuffer with proper antialiasing (alpha-blended coverage values).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use spin::Once;

use crate::{app_wm, framebuffer};
use marx_sdk::WindowId;

// Inter (https://rsms.me/inter/) — SIL Open Font License 1.1.
// Chosen for its Segoe-UI-like proportions, which suit a Frutiger Aero look.
const FONT_BYTES: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");

static FONT: Once<FontRef<'static>> = Once::new();

/// Parse the embedded TTF. Idempotent. Call once during boot.
pub fn init() {
    FONT.call_once(|| {
        FontRef::try_from_slice(FONT_BYTES).expect("Roboto-Regular.ttf failed to parse")
    });
}

fn font() -> Option<&'static FontRef<'static>> {
    FONT.get()
}

/// Pixel width of `text` rendered at `size_px`. Returns 0 if font isn't loaded.
pub fn text_width(text: &str, size_px: f32) -> u32 {
    let Some(font) = font() else { return 0; };
    let scaled = font.as_scaled(PxScale::from(size_px));
    let mut w = 0.0_f32;
    for ch in text.chars() {
        w += scaled.h_advance(scaled.glyph_id(ch));
    }
    // ceil without std/std-float trait: cast + bump-if-fractional.
    let int = w as u32;
    if (int as f32) < w { int + 1 } else { int }
}

/// The "ascent" of the chosen size — distance from the baseline up to the
/// top of typical capitals. Callers position the BASELINE; pass
/// `y + ascent` if they want the top of the text at `y`.
pub fn ascent(size_px: f32) -> f32 {
    match font() {
        Some(f) => f.as_scaled(PxScale::from(size_px)).ascent(),
        None    => 0.0,
    }
}

/// Render `text` at (x, y), where y is the BASELINE (not the top).
/// Each glyph's coverage value (0.0–1.0) drives an alpha blend against the
/// current framebuffer content, so antialiased edges merge with any
/// background — gradient, solid colour, image, doesn't matter.
pub fn draw_text(x: i32, y_baseline: i32, text: &str, size_px: f32, color: (u8, u8, u8)) {
    let Some(font) = font() else { return; };
    let scaled = font.as_scaled(PxScale::from(size_px));

    let mut cursor_x = x as f32;
    for ch in text.chars() {
        let mut glyph = scaled.scaled_glyph(ch);
        glyph.position = point(cursor_x, y_baseline as f32);

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bb.min.x as i32 + gx as i32;
                let py = bb.min.y as i32 + gy as i32;
                if px < 0 || py < 0 { return; }
                let a = (coverage.clamp(0.0, 1.0) * 255.0) as u8;
                framebuffer::blend_pixel_at(px as usize, py as usize, color, a);
            });
        }
        cursor_x += scaled.h_advance(scaled.glyph_id(ch));
    }
}

/// Convenience: draw text horizontally centred between `x_left` and `x_right`.
#[allow(dead_code)] // exported for future GUI labels
pub fn draw_text_centered_h(
    x_left: i32, x_right: i32, y_baseline: i32,
    text: &str, size_px: f32, color: (u8, u8, u8),
) {
    let w = text_width(text, size_px) as i32;
    let x = x_left + ((x_right - x_left) - w) / 2;
    draw_text(x, y_baseline, text, size_px, color);
}

/// Like `draw_text`, but rasterise into an app window's content buffer
/// instead of the framebuffer. Coordinates are content-local; `y_baseline`
/// is the text baseline within the content area (not the window chrome).
/// Called by the kernel-side `draw_text` service for ELF apps.
pub fn draw_text_into_rgb_buffer_for_app(
    app_id: WindowId, x: i32, y_baseline: i32,
    text: &str, size_px: f32, color: (u8, u8, u8),
) {
    let Some(font) = font() else { return; };
    let scaled = font.as_scaled(PxScale::from(size_px));
    let mut cursor_x = x as f32;
    for ch in text.chars() {
        let mut glyph = scaled.scaled_glyph(ch);
        glyph.position = point(cursor_x, y_baseline as f32);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bb.min.x as i32 + gx as i32;
                let py = bb.min.y as i32 + gy as i32;
                let a = (coverage.clamp(0.0, 1.0) * 255.0) as u8;
                app_wm::blend_pixel(app_id, px, py, color, a);
            });
        }
        cursor_x += scaled.h_advance(scaled.glyph_id(ch));
    }
}
