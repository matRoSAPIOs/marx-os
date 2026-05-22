//! MarX-OS app SDK.
//!
//! Shared between the kernel and every app. Defines the binary interface
//! the kernel exposes to ELF apps loaded at runtime:
//!   • [`KernelServices`] — a vtable-style struct of `extern "C"` function
//!     pointers handed to each app via its `_start(svc: *const KernelServices)`.
//!   • Common types (events, colours, handles) used by both sides.
//!
//! Apps depend on this crate (`#![no_std]`) and use the high-level helpers
//! in [`api`] which forward to the kernel through the services pointer
//! stashed during init.

#![no_std]

// ---- ABI version (bump when KernelServices layout changes) ----
pub const ABI_VERSION: u32 = 1;

/// Opaque window handle returned by `window_open`.
pub type WindowId = u32;

/// 8-bit-per-channel RGB colour.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self { Rgb { r, g, b } }
}

/// Mouse / keyboard event delivered to an app via `event_poll`.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub enum Event {
    None,
    /// Window's close button was clicked. App should exit.
    CloseRequested,
    /// Left mouse button pressed inside the app's window.
    /// Coords are window-local (0,0 = top-left of content area).
    MouseDown { x: i32, y: i32 },
    MouseUp   { x: i32, y: i32 },
    MouseMove { x: i32, y: i32 },
    /// Keyboard key down. `ch` is the ASCII char (0 if non-printable).
    KeyDown   { ch: u8 },
}

/// vtable handed to every app at launch. All function pointers are
/// `extern "C"` so the ABI is stable across Rust versions and matches
/// what a C app could in principle target.
#[repr(C)]
pub struct KernelServices {
    pub abi_version: u32,

    // ---- Window management ----
    /// Open a new window. `title` is a NUL-terminated UTF-8 string.
    pub window_open:  unsafe extern "C" fn(title: *const u8, w: u32, h: u32) -> WindowId,
    pub window_close: unsafe extern "C" fn(id: WindowId),

    // ---- Drawing into a window's content area ----
    pub draw_rect:   unsafe extern "C" fn(id: WindowId, x: i32, y: i32, w: u32, h: u32, color: Rgb),
    pub draw_text:   unsafe extern "C" fn(id: WindowId, x: i32, y: i32, text: *const u8, len: u32, size_px: f32, color: Rgb),
    pub text_width:  unsafe extern "C" fn(text: *const u8, len: u32, size_px: f32) -> u32,
    /// Mark the window dirty so the next compositor pass picks it up.
    pub present:     unsafe extern "C" fn(id: WindowId),

    // ---- Events ----
    /// Blocking poll: yields the CPU until something happens, then returns
    /// the next pending event for the given window.
    pub event_poll:  unsafe extern "C" fn(id: WindowId) -> Event,

    // ---- Filesystem (read-only MARXARCH for now) ----
    /// Read a file into `out_buf`. Returns the actual bytes read, or
    /// `u32::MAX` on error.
    pub fs_read:     unsafe extern "C" fn(name: *const u8, name_len: u32, out_buf: *mut u8, buf_cap: u32) -> u32,

    // ---- Misc ----
    /// Print a debug message to the host serial console.
    pub debug_log:   unsafe extern "C" fn(text: *const u8, len: u32),
}

// =====================================================================
// High-level helpers used by app code. They hide the raw pointers /
// function-table indirection.
// =====================================================================

pub mod api {
    use super::*;
    use core::cell::UnsafeCell;

    struct ServicesCell(UnsafeCell<*const KernelServices>);
    unsafe impl Sync for ServicesCell {}
    static SERVICES: ServicesCell = ServicesCell(UnsafeCell::new(core::ptr::null()));

    /// Store the kernel-supplied services pointer. Call once from `_start`.
    ///
    /// SAFETY: `svc` must be a valid pointer that remains live for the
    /// entire lifetime of the app process.
    pub unsafe fn init(svc: *const KernelServices) {
        *SERVICES.0.get() = svc;
    }

    /// Access the services vtable. Panics if `init` wasn't called.
    fn svc() -> &'static KernelServices {
        unsafe {
            let p = *SERVICES.0.get();
            assert!(!p.is_null(), "marx_sdk::api::init not called");
            &*p
        }
    }

    pub fn debug_log(s: &str) {
        let s = s.as_bytes();
        unsafe { (svc().debug_log)(s.as_ptr(), s.len() as u32); }
    }

    pub fn open_window(title: &str, w: u32, h: u32) -> WindowId {
        // Title is passed as a length+pointer rather than NUL-terminated for
        // simplicity (the kernel side reads `name_len` from the wrapper).
        // For window_open specifically the API expects NUL-terminated, so
        // we copy into a small stack buffer.
        let mut buf = [0u8; 64];
        let n = title.len().min(63);
        buf[..n].copy_from_slice(&title.as_bytes()[..n]);
        // buf[n] is already 0 (initialised), giving us NUL termination.
        unsafe { (svc().window_open)(buf.as_ptr(), w, h) }
    }

    pub fn close_window(id: WindowId) {
        unsafe { (svc().window_close)(id); }
    }

    pub fn draw_rect(id: WindowId, x: i32, y: i32, w: u32, h: u32, color: Rgb) {
        unsafe { (svc().draw_rect)(id, x, y, w, h, color); }
    }

    pub fn draw_text(id: WindowId, x: i32, y: i32, text: &str, size_px: f32, color: Rgb) {
        let b = text.as_bytes();
        unsafe { (svc().draw_text)(id, x, y, b.as_ptr(), b.len() as u32, size_px, color); }
    }

    pub fn text_width(text: &str, size_px: f32) -> u32 {
        let b = text.as_bytes();
        unsafe { (svc().text_width)(b.as_ptr(), b.len() as u32, size_px) }
    }

    pub fn present(id: WindowId) {
        unsafe { (svc().present)(id); }
    }

    pub fn poll_event(id: WindowId) -> Event {
        unsafe { (svc().event_poll)(id) }
    }
}
