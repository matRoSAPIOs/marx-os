//! XP/Vista-style welcome screen with TTF text and an Aero glass button.
//!
//! Layout (1280×720 reference):
//!     +========================================+ ← deep-blue band (60 px)
//!     |              sky gradient              |
//!     |          [ MarX-OS logo ~400 px ]      |
//!     |          "Welcome to MarX-OS" (TTF)    |
//!     |             [Continue button]          |
//!     +========================================+ ← gold + deep-blue band

use crate::{button, cursor, framebuffer, input, mouse, splash, ttf};

const DEEP_BLUE:  (u8, u8, u8) = (0x0A, 0x2E, 0x73);
const SKY_TOP:    (u8, u8, u8) = (0x5B, 0xA0, 0xF0);
const SKY_BOTTOM: (u8, u8, u8) = (0x1E, 0x68, 0xD6);
const GOLD:       (u8, u8, u8) = (0xF5, 0xA6, 0x23);
const WHITE:      (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
const TITLE_SHADOW: (u8, u8, u8) = (0x0A, 0x2E, 0x55);

const TOP_BAND_H:    usize = 60;
const BOTTOM_BAND_H: usize = 37;
const GOLD_STRIP_H:  usize = 3;

const BTN_W: usize = 220;
const BTN_H: usize = 56;

/// Paint the welcome scene, then poll the mouse and wait for a click on the
/// "Continue" button. Returns once the user has clicked-and-released inside
/// the button area. Caller is responsible for clearing the screen and
/// continuing boot.
pub fn show_and_wait() {
    use x86_64::instructions::interrupts as ints;

    let Some((sw, sh)) = framebuffer::dimensions() else { return; };
    let btn = compute_button_rect(sw, sh);

    // ---- background + logo + title (one IRQ-safe paint) ----
    ints::without_interrupts(|| {
        paint_scene(sw, sh);
        button::draw(&btn, "Continue", button::ButtonState::Normal);
        cursor::invalidate();
        framebuffer::present();
    });

    // Drop any keystrokes queued during boot.
    input::drain();

    // ---- interaction loop ----
    let mut state = button::ButtonState::Normal;
    let mut prev_pressed_inside = false;
    loop {
        // Sleep until next IRQ (mouse moves and timer ticks both wake us).
        ints::enable_and_hlt();

        let m = mouse::state();
        let inside = btn.contains(m.x, m.y);

        // State machine: detect click as a press-inside followed by a
        // release that's still inside (standard button semantics).
        let new_state = if m.left && inside {
            button::ButtonState::Pressed
        } else if inside {
            button::ButtonState::Hover
        } else {
            button::ButtonState::Normal
        };

        if new_state != state {
            ints::without_interrupts(|| {
                button::draw(&btn, "Continue", new_state);
                cursor::invalidate();
                framebuffer::present();
            });
            state = new_state;
        }

        let pressed_inside_now = m.left && inside;
        if prev_pressed_inside && !m.left && inside {
            // Click released inside the button → fire.
            break;
        }
        prev_pressed_inside = pressed_inside_now;

        // Also accept Enter / Space as a keyboard shortcut.
        if let Some(b) = pop_key_nonblocking() {
            if b == b'\n' || b == b'\r' || b == b' ' {
                break;
            }
        }
    }

    // ---- transition ----
    ints::without_interrupts(|| {
        framebuffer::clear();
        cursor::invalidate();
        framebuffer::present();
    });
}

/// Non-blocking pop of one byte from the keyboard ring, if any.
fn pop_key_nonblocking() -> Option<u8> {
    use x86_64::instructions::interrupts as ints;
    ints::without_interrupts(|| input::try_pop())
}

fn compute_button_rect(sw: usize, sh: usize) -> button::ButtonRect {
    let x = (sw - BTN_W) / 2;
    let y = sh - BOTTOM_BAND_H - GOLD_STRIP_H - BTN_H - 40;
    button::ButtonRect { x, y, w: BTN_W, h: BTN_H }
}

fn paint_scene(sw: usize, sh: usize) {
    // ---- background bands ----
    framebuffer::fill_rect(0, 0, sw, TOP_BAND_H, DEEP_BLUE);
    let grad_y0 = TOP_BAND_H;
    let grad_y1 = sh - BOTTOM_BAND_H - GOLD_STRIP_H;
    framebuffer::fill_gradient_v(grad_y0, grad_y1, SKY_TOP, SKY_BOTTOM);
    framebuffer::fill_rect(0, grad_y1, sw, GOLD_STRIP_H, GOLD);
    framebuffer::fill_rect(0, grad_y1 + GOLD_STRIP_H, sw, BOTTOM_BAND_H, DEEP_BLUE);

    // ---- logo, alpha-blended over the sky ----
    let logo_w = splash::LOGO_W;
    let logo_h = splash::LOGO_H;
    let logo_x = sw.saturating_sub(logo_w) / 2;
    // Position the logo's vertical centre at ~35 % down the gradient so
    // there's room below for the title and the button.
    let logo_y = grad_y0 + ((grad_y1 - grad_y0) * 30 / 100).saturating_sub(logo_h / 2);
    framebuffer::blit_rgba_at(splash::LOGO, logo_w, logo_h, logo_x, logo_y);

    // ---- title (TTF, big) ----
    let title = "Welcome to MarX-OS";
    let title_size = 36.0_f32;
    let title_w = ttf::text_width(title, title_size) as i32;
    let title_x = (sw as i32 - title_w) / 2;
    let title_baseline = (logo_y + logo_h + 60) as i32;
    // Soft shadow + bright text.
    ttf::draw_text(title_x + 2, title_baseline + 2, title, title_size, TITLE_SHADOW);
    ttf::draw_text(title_x,     title_baseline,     title, title_size, WHITE);
}
