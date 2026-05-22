//! Windows 7 / Aero-style glass button.
//!
//! Layout:
//!   * Rounded-rect background (radius 6 px)
//!   * Top ~45 % of the body: light gloss gradient
//!   * Bottom ~55 % of the body: deeper blue gradient
//!   * 1-px dark border on the outside
//!   * Centred TTF label (white, with a soft 1-px dark shadow)
//!
//! Three visual states: `Normal`, `Hover`, `Pressed`. Hovered = slightly
//! brighter palette + cyan-tinged top half. Pressed = palette swapped /
//! darker, label nudged 1 px down to feel "clicked".

use crate::{framebuffer, ttf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    Hover,
    Pressed,
}

pub struct ButtonRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl ButtonRect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && py >= self.y as i32
            && px < (self.x + self.w) as i32
            && py < (self.y + self.h) as i32
    }
}

/// Paint the button. Caller passes the current state.
pub fn draw(rect: &ButtonRect, label: &str, state: ButtonState) {
    let (top_a, top_b, bot_a, bot_b) = match state {
        ButtonState::Normal => (
            (0xCB, 0xE2, 0xF8), (0x9D, 0xC4, 0xEF), // top gloss
            (0x66, 0xA8, 0xE5), (0x3D, 0x86, 0xCB), // bottom body
        ),
        ButtonState::Hover => (
            (0xE0, 0xEF, 0xFB), (0xB3, 0xD4, 0xF3), // brighter
            (0x82, 0xBC, 0xEC), (0x52, 0x9A, 0xD8),
        ),
        ButtonState::Pressed => (
            (0x6B, 0xA0, 0xD0), (0x4F, 0x86, 0xBA), // darker, inverted feel
            (0x2D, 0x68, 0xA5), (0x1E, 0x55, 0x8B),
        ),
    };
    let border       = (0x14, 0x4A, 0x82);
    let label_color  = (0xFF, 0xFF, 0xFF);
    let label_shadow = (0x0A, 0x2E, 0x55);

    let radius = 6;
    let split = rect.h * 45 / 100;

    // 1. Solid bottom fill (rounded-corner masked).
    framebuffer::fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, bot_a, radius);
    // 2. Bottom gradient — clipped to rounded mask.
    framebuffer::fill_v_gradient_in_rounded_rect(
        rect.x, rect.y + split, rect.w, rect.h - split,
        bot_a, bot_b,
        rect.x, rect.y, rect.w, rect.h, radius,
    );
    // 3. Top half gloss gradient.
    framebuffer::fill_v_gradient_in_rounded_rect(
        rect.x, rect.y, rect.w, split,
        top_a, top_b,
        rect.x, rect.y, rect.w, rect.h, radius,
    );
    // 4. 1-px navy outline.
    framebuffer::stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, border, radius);

    // 5. Centred label, pushed 1 px down when pressed.
    let size = 22.0_f32;
    let label_w = ttf::text_width(label, size) as i32;
    let ascent  = ttf::ascent(size);
    let baseline = (rect.y as f32 + (rect.h as f32 + ascent) * 0.5) as i32;
    let baseline = if state == ButtonState::Pressed { baseline + 1 } else { baseline };
    let text_x = rect.x as i32 + (rect.w as i32 - label_w) / 2;
    // 1-px drop shadow for legibility on glossy bg.
    ttf::draw_text(text_x + 1, baseline + 1, label, size, label_shadow);
    ttf::draw_text(text_x,     baseline,     label, size, label_color);
}
