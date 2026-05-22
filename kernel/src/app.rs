//! App runtime: load an ELF from MARXARCH, spawn it as a scheduler task,
//! and provide it with a [`KernelServices`] vtable.
//!
//! Phase 9.2 implements the minimal vtable (debug_log only).  Phase 9.3
//! will add window + drawing services so apps can actually paint UI.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use marx_sdk::{Event, KernelServices, Rgb, WindowId, ABI_VERSION};
use spin::Mutex;

use crate::{app_wm, elf, fs, serial, task, ttf};

/// Slot used to hand `(entry, services_ptr)` from `launch` to the freshly-
/// spawned task's trampoline.  Mutex because `launch` and the new task can
/// race momentarily; tasks pop the slot under the lock the first time they
/// run.
static PENDING: Mutex<Option<PendingLaunch>> = Mutex::new(None);

struct PendingLaunch {
    /// Owns the loaded ELF image; dropped when the slot is cleared.  We
    /// keep it alive by transferring ownership into a global registry so
    /// the app's code pages stay live for its lifetime.
    _image:   elf::LoadedApp,
    /// Boxed-and-leaked services vtable; the raw pointer goes to the app.
    services: *const KernelServices,
}

unsafe impl Send for PendingLaunch {}

/// Apps that are currently running.  We keep the loaded image alive here
/// so its memory isn't freed mid-execution; cleanup is for a future phase.
static RUNNING: Mutex<Vec<RunningApp>> = Mutex::new(Vec::new());

struct RunningApp {
    _image:    elf::LoadedApp,
    _services: Box<KernelServices>,
}

unsafe impl Send for RunningApp {}

/// Read `name` from MARXARCH, parse + load it as an ELF, and spawn it as
/// a fresh kernel task.  Returns `Err` with a static reason on failure.
pub fn launch(name: &str) -> Result<u64, &'static str> {
    let bytes = fs::read(name).map_err(|_| "fs::read failed")?;
    let image = elf::load(&bytes).map_err(|_| "elf::load failed")?;

    // Build the services vtable.  Box::into_raw transfers ownership to a
    // raw pointer without running the destructor; we'll recover the box
    // inside the task body to keep its lifetime explicit.
    let services_box = Box::new(make_services());
    let svc_ptr: *const KernelServices = Box::into_raw(services_box);

    serial::write_str("[app] loaded ELF, entry = 0x");
    write_hex(image.entry as u64);
    serial::write_str("\n");

    // Stash the launch info, then spawn the task.  The first time it
    // runs, it pops PENDING and jumps to the entry point.
    {
        let mut slot = PENDING.lock();
        if slot.is_some() {
            return Err("another app launch is already pending");
        }
        *slot = Some(PendingLaunch { _image: image, services: svc_ptr });
    }

    let id = task::spawn(app_task_main as fn() -> !);
    Ok(id)
}

/// First-run shim for an app task.  Pops the PENDING slot, transfers the
/// image into RUNNING, then jumps to the app's `_start(services)`.
fn app_task_main() -> ! {
    // Pop the pending launch.
    let pl = {
        let mut slot = PENDING.lock();
        slot.take().expect("app task started with no PENDING entry")
    };
    let entry = pl._image.entry;
    let svc   = pl.services;

    // Keep the image + services alive in the global RUNNING registry.
    // SAFETY: we created `svc` from a Box::new above; recover it here so
    // Drop won't run twice.
    let services_box: Box<KernelServices> = unsafe { Box::from_raw(svc as *mut KernelServices) };
    RUNNING.lock().push(RunningApp { _image: pl._image, _services: services_box });

    // Tail-call the app's entry point with the services pointer in RDI.
    let entry_fn: extern "C" fn(*const KernelServices) -> ! =
        unsafe { core::mem::transmute(entry) };
    entry_fn(svc)
}

// ===================================================================== ABI
//
// Each function below has C linkage and matches a slot in `KernelServices`.
// Apps call these via the vtable they were handed at launch.

unsafe extern "C" fn svc_window_open(title: *const u8, w: u32, h: u32) -> WindowId {
    // Title is NUL-terminated UTF-8 (sdk::api::open_window enforces this).
    let mut n = 0;
    while n < 256 && *title.add(n) != 0 { n += 1; }
    let slice = core::slice::from_raw_parts(title, n);
    let title = core::str::from_utf8(slice).unwrap_or("(invalid utf-8)");

    let id = app_wm::create(w as usize, h as usize);
    app_wm::enqueue_op(app_wm::AppOp::OpenWindow {
        app_id:    id,
        title:     String::from(title),
        content_w: w,
        content_h: h,
    });
    id
}

unsafe extern "C" fn svc_window_close(id: WindowId) {
    app_wm::enqueue_op(app_wm::AppOp::CloseWindow { app_id: id });
}

unsafe extern "C" fn svc_draw_rect(
    id: WindowId, x: i32, y: i32, w: u32, h: u32, color: Rgb,
) {
    app_wm::draw_rect(id, x, y, w, h, (color.r, color.g, color.b));
}

unsafe extern "C" fn svc_draw_text(
    id: WindowId, x: i32, y: i32, text: *const u8, len: u32, size_px: f32, color: Rgb,
) {
    let slice = core::slice::from_raw_parts(text, len as usize);
    let Ok(s) = core::str::from_utf8(slice) else { return; };
    // We bypass the framebuffer here: text needs to land in the app's
    // pixel buffer, not on screen.  Use ttf to rasterise into a tiny
    // temp glyph blender, then write to app_wm.  For Phase 9.3 we
    // implement a simple path: render directly onto the framebuffer at
    // the app window's body offset.  Better text routing is Phase 9.4.
    //
    // Workaround: app_wm's content is row-major RGB; we'll call ttf's
    // direct draw which writes via blend_pixel_at to the framebuffer.
    // We translate window-local coords into screen coords by looking up
    // the window in the WM through a callback added later.  For now,
    // draw into app content via a per-pixel routine.
    //
    // Simplified approach: render text glyphs into app content using
    // a CPU rasteriser exposed by ttf.
    ttf::draw_text_into_rgb_buffer_for_app(
        id, x, y, s, size_px, (color.r, color.g, color.b),
    );
}

unsafe extern "C" fn svc_text_width(text: *const u8, len: u32, size_px: f32) -> u32 {
    let slice = core::slice::from_raw_parts(text, len as usize);
    let Ok(s) = core::str::from_utf8(slice) else { return 0; };
    ttf::text_width(s, size_px)
}

unsafe extern "C" fn svc_present(_id: WindowId) {
    // Signal the desktop to repaint on its next iteration.  Drawing has
    // already been deposited into the app's content buffer via
    // draw_rect / draw_text; "present" just makes it visible.
    app_wm::APP_DIRTY.store(true, core::sync::atomic::Ordering::Release);
}

unsafe extern "C" fn svc_event_poll(id: WindowId) -> Event {
    // Block-wait for the next event: pop the queue, or yield + retry.
    loop {
        if let Some(evt) = app_wm::take_event(id) {
            return evt;
        }
        crate::task::yield_now();
    }
}

unsafe extern "C" fn svc_fs_read(
    name_ptr: *const u8, name_len: u32, out_buf: *mut u8, buf_cap: u32,
) -> u32 {
    let name_slice = core::slice::from_raw_parts(name_ptr, name_len as usize);
    let Ok(name)   = core::str::from_utf8(name_slice) else { return u32::MAX; };
    let Ok(bytes)  = crate::fs::read(name)            else { return u32::MAX; };
    let n = bytes.len().min(buf_cap as usize);
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_buf, n);
    n as u32
}

unsafe extern "C" fn svc_debug_log(text: *const u8, len: u32) {
    let slice = core::slice::from_raw_parts(text, len as usize);
    if let Ok(s) = core::str::from_utf8(slice) {
        serial::write_str(s);
    }
}

fn make_services() -> KernelServices {
    KernelServices {
        abi_version:  ABI_VERSION,
        window_open:  svc_window_open,
        window_close: svc_window_close,
        draw_rect:    svc_draw_rect,
        draw_text:    svc_draw_text,
        text_width:   svc_text_width,
        present:      svc_present,
        event_poll:   svc_event_poll,
        fs_read:      svc_fs_read,
        debug_log:    svc_debug_log,
    }
}

// ----------------------------------------------------------------------
// Serial helpers (minimal hex printing — kept local to avoid pulling fmt)

fn write_hex(mut v: u64) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut buf = [b'0'; 16];
    for i in (0..16).rev() {
        buf[i] = HEX[(v & 0xF) as usize];
        v >>= 4;
    }
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    serial::write_str(s);
}
