//! Frutiger Aero wallpaper, baked into the kernel binary at build time.
//!
//! `assets/wallpaper.jpg` is decoded and "cover"-fitted to 1280×720 by
//! `kernel/build.rs`, which writes packed RGB (3 bytes/pixel) to
//! `OUT_DIR/wallpaper.bin` and constants to `OUT_DIR/wallpaper_consts.rs`.

include!(concat!(env!("OUT_DIR"), "/wallpaper_consts.rs"));

/// Packed RGB pixels, row-major.  `WALLPAPER_W * WALLPAPER_H * 3` bytes.
pub static WALLPAPER_RGB: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/wallpaper.bin"));
