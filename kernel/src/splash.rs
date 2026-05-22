//! Boot splash: centred logo + brief delay, then back to normal text console.
//!
//! The logo bytes are baked into the kernel image by `build.rs`:
//!   * `LOGO`   — packed RGBA pixels (alpha preserved for runtime blending)
//!   * `LOGO_W`, `LOGO_H` — dimensions in pixels

use crate::framebuffer;

// LOGO_W / LOGO_H are generated as `pub const` by build.rs.
include!(concat!(env!("OUT_DIR"), "/logo_consts.rs"));
pub const LOGO: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/logo.bin"));

/// Show the logo on a white background, hold briefly, then clear the screen
/// back to the console's normal black background.
///
/// Alpha is preserved in the baked logo; we paint white first, then alpha-
/// blend the logo over it, so anti-aliased edges merge cleanly with the
/// white matte. Same logo data works against the sky gradient in welcome.rs.
pub fn show() {
    framebuffer::fill_color(0xFF, 0xFF, 0xFF);
    framebuffer::blit_rgba_centered(LOGO, LOGO_W, LOGO_H);
    // ~3 billion TSC ticks. On a ~2 GHz host that's ~1.5s; under QEMU TCG
    // (no KVM) it may run noticeably longer — fine for a splash.
    delay_tsc(3_000_000_000);
    framebuffer::clear();
}

#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn delay_tsc(cycles: u64) {
    let start = rdtsc();
    while rdtsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}
