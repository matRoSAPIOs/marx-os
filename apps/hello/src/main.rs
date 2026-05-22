//! Hello, MarX-OS! — proof-of-concept ELF app with a real window.
//!
//! Phase 9.3: opens a window through the kernel's WM, paints a coloured
//! banner + centred greeting into its content buffer, and exits cleanly
//! when the user clicks the X.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use marx_sdk::{api, Event, KernelServices, Rgb};

const W: u32 = 380;
const H: u32 = 200;

const BG:        Rgb = Rgb::new(0xF4, 0xF7, 0xFB);
const BANNER_TOP:Rgb = Rgb::new(0x6F, 0xB6, 0xF0); // same sky as wallpaper
const BANNER_BOT:Rgb = Rgb::new(0x3D, 0x86, 0xD4);
const TXT_TITLE: Rgb = Rgb::new(0x10, 0x2A, 0x4E);
const TXT_SUB:   Rgb = Rgb::new(0x44, 0x55, 0x66);

#[no_mangle]
pub unsafe extern "C" fn _start(svc: *const KernelServices) -> ! {
    api::init(svc);
    api::debug_log("[hello] starting up\n");

    let win = api::open_window("Hello", W, H);
    api::debug_log("[hello] window opened\n");

    paint(win);
    api::present(win);

    // Blocking event loop — exits when desktop signals CloseRequested.
    loop {
        match api::poll_event(win) {
            Event::CloseRequested => {
                api::debug_log("[hello] close requested, exiting\n");
                api::close_window(win);
                halt();
            }
            // Repaint banner on any click so the user sees feedback.
            Event::MouseDown { .. } => {
                paint(win);
                api::present(win);
            }
            _ => {}
        }
    }
}

fn paint(id: marx_sdk::WindowId) {
    // Wipe background to off-white.
    api::draw_rect(id, 0, 0, W, H, BG);

    // Fake vertical "gradient" via 8 horizontal bands — Aero-ish accent.
    let bands = 8;
    let band_h = 40_u32 / bands;
    for i in 0..bands {
        let t = i as u32;
        let r = (BANNER_TOP.r as u32 * (bands - t) + BANNER_BOT.r as u32 * t) / bands;
        let g = (BANNER_TOP.g as u32 * (bands - t) + BANNER_BOT.g as u32 * t) / bands;
        let b = (BANNER_TOP.b as u32 * (bands - t) + BANNER_BOT.b as u32 * t) / bands;
        api::draw_rect(id, 0, (i * band_h) as i32, W, band_h,
                       Rgb::new(r as u8, g as u8, b as u8));
    }

    // Centred greeting.
    let greeting = "Hello, MarX-OS!";
    let sub      = "First real ELF app with a window.";
    let title_sz = 22.0;
    let sub_sz   = 14.0;
    let gw = api::text_width(greeting, title_sz) as i32;
    let sw = api::text_width(sub,      sub_sz)   as i32;
    api::draw_text(id, (W as i32 - gw) / 2, 90,  greeting, title_sz, TXT_TITLE);
    api::draw_text(id, (W as i32 - sw) / 2, 130, sub,      sub_sz,   TXT_SUB);

    // Subtle hint at the bottom.
    let hint    = "Click anywhere to redraw.";
    let hint_sz = 12.0;
    let hw = api::text_width(hint, hint_sz) as i32;
    api::draw_text(id, (W as i32 - hw) / 2, 175, hint, hint_sz,
                   Rgb::new(0x78, 0x82, 0x90));
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    api::debug_log("[hello] PANIC\n");
    halt()
}
