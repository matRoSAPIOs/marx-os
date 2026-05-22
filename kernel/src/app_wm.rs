//! Per-app-window shared state: pixel content buffer + event queue.
//!
//! The desktop's `WM` holds the chrome (title bar, position, focus state)
//! for app windows just like for built-in windows.  When the desktop
//! paints an app window, instead of drawing built-in text it blits the
//! pixel buffer from [`AppWindowState::content`] into the body area.
//!
//! App services (`window_open`, `draw_rect`, `present`, `event_poll`,
//! `window_close`) live in `app.rs` and forward to functions in this
//! module under a global lock.  Mouse-event delivery: when the desktop
//! sees a mouse interaction over an app window, it enqueues an `Event`
//! into the app's window state, which the app's `event_poll` then drains.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::AtomicBool;

use marx_sdk::{Event, WindowId};
use spin::Mutex;

/// Set whenever an app's `present()` is called — desktop repaints on next tick.
pub static APP_DIRTY: AtomicBool = AtomicBool::new(false);

/// Operations apps ask the desktop to apply to its window manager.
///
/// Apps run on their own scheduler tasks and don't own the WM directly;
/// they push ops here, and the desktop's event loop drains them at the
/// start of every iteration.
pub enum AppOp {
    /// Spawn chrome (title bar + frame) around a content area.
    OpenWindow {
        app_id:    WindowId,
        title:     String,
        content_w: u32,
        content_h: u32,
    },
    /// Tear down chrome AND free the AppWindowState.
    CloseWindow { app_id: WindowId },
}

static PENDING_OPS: Mutex<Vec<AppOp>> = Mutex::new(Vec::new());

pub fn enqueue_op(op: AppOp) {
    PENDING_OPS.lock().push(op);
}

/// Drain all pending ops, returning them in submission order.
/// Desktop calls this once per event-loop iteration.
pub fn drain_ops() -> Vec<AppOp> {
    let mut q = PENDING_OPS.lock();
    core::mem::take(&mut *q)
}

pub struct AppWindowState {
    pub content_w: usize,
    pub content_h: usize,
    /// Row-major RGB pixels for the window's content area (size = w*h*3).
    pub content:   Vec<u8>,
    /// Pending events to deliver to the app on its next `event_poll`.
    pub events:    Vec<Event>,
    /// True once the WM has been told to remove this window's chrome
    /// (e.g. user clicked X).  The app still owns the AppWindowState
    /// until it calls `window_close`.
    pub closed_by_user: bool,
}

impl AppWindowState {
    fn new(w: usize, h: usize) -> Self {
        AppWindowState {
            content_w: w,
            content_h: h,
            content:   vec![0xFF; w * h * 3],   // start fully white
            events:    Vec::new(),
            closed_by_user: false,
        }
    }
}

static APP_WINDOWS: Mutex<BTreeMap<WindowId, AppWindowState>> =
    Mutex::new(BTreeMap::new());

static NEXT_ID: Mutex<WindowId> = Mutex::new(1);

/// Allocate a fresh AppWindowState and return its id.
pub fn create(w: usize, h: usize) -> WindowId {
    let id = {
        let mut n = NEXT_ID.lock();
        let id = *n;
        *n = id.wrapping_add(1);
        id
    };
    APP_WINDOWS.lock().insert(id, AppWindowState::new(w, h));
    id
}

/// Tear down an AppWindowState (called when the app exits / closes).
pub fn destroy(id: WindowId) {
    APP_WINDOWS.lock().remove(&id);
}

/// Push an event into the window's queue.  No-op if window is unknown.
pub fn push_event(id: WindowId, evt: Event) {
    if let Some(state) = APP_WINDOWS.lock().get_mut(&id) {
        state.events.push(evt);
    }
}

/// Pop the oldest event from the queue, or None if empty / unknown.
pub fn take_event(id: WindowId) -> Option<Event> {
    let mut map = APP_WINDOWS.lock();
    let state = map.get_mut(&id)?;
    if state.events.is_empty() { return None; }
    Some(state.events.remove(0))
}

/// Mark this window as having been closed by the user (via the X button).
/// The app sees `Event::CloseRequested` and is expected to call
/// `window_close` shortly thereafter.
pub fn mark_closed_by_user(id: WindowId) {
    if let Some(state) = APP_WINDOWS.lock().get_mut(&id) {
        if !state.closed_by_user {
            state.closed_by_user = true;
            state.events.push(Event::CloseRequested);
        }
    }
}

/// Alpha-blend a single pixel into the content buffer.  Used by the TTF
/// rasteriser to anti-alias glyph edges against the existing background.
pub fn blend_pixel(id: WindowId, x: i32, y: i32, color: (u8, u8, u8), alpha: u8) {
    if alpha == 0 { return; }
    let mut map = APP_WINDOWS.lock();
    let Some(state) = map.get_mut(&id) else { return; };
    if x < 0 || y < 0 { return; }
    let (xu, yu) = (x as usize, y as usize);
    if xu >= state.content_w || yu >= state.content_h { return; }
    let off = (yu * state.content_w + xu) * 3;
    if alpha == 255 {
        state.content[off]     = color.0;
        state.content[off + 1] = color.1;
        state.content[off + 2] = color.2;
        return;
    }
    let inv = 255 - alpha as u32;
    let dr = state.content[off]     as u32;
    let dg = state.content[off + 1] as u32;
    let db = state.content[off + 2] as u32;
    state.content[off]     = ((color.0 as u32 * alpha as u32 + dr * inv) / 255) as u8;
    state.content[off + 1] = ((color.1 as u32 * alpha as u32 + dg * inv) / 255) as u8;
    state.content[off + 2] = ((color.2 as u32 * alpha as u32 + db * inv) / 255) as u8;
}

/// Paint a filled rectangle (in window-local coords) into the content buffer.
pub fn draw_rect(id: WindowId, x: i32, y: i32, w: u32, h: u32, rgb: (u8, u8, u8)) {
    let mut map = APP_WINDOWS.lock();
    let Some(state) = map.get_mut(&id) else { return; };
    let cw = state.content_w as i32;
    let ch = state.content_h as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(cw);
    let y1 = (y + h as i32).min(ch);
    if x1 <= x0 || y1 <= y0 { return; }
    for yy in y0..y1 {
        for xx in x0..x1 {
            let off = (yy as usize * state.content_w + xx as usize) * 3;
            state.content[off]     = rgb.0;
            state.content[off + 1] = rgb.1;
            state.content[off + 2] = rgb.2;
        }
    }
}

/// Zero-copy paint helper: invokes the closure with a borrow of the
/// pixel buffer while the lock is held.  Callers MUST NOT acquire
/// other app_wm locks inside `f` — only framebuffer-side ones.
pub fn with_content<F: FnOnce(&[u8], usize, usize)>(id: WindowId, f: F) {
    let map = APP_WINDOWS.lock();
    if let Some(state) = map.get(&id) {
        f(&state.content, state.content_w, state.content_h);
    }
}
