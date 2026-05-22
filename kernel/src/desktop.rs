//! Tiny desktop / window manager. Phase 8.5 (Aero glass + right-click menu).
//!
//! Multi-window WM with click-to-raise focus, sky-gradient wallpaper, and a
//! taskbar at the bottom.  Each window has a glass title bar (Aero-style),
//! a close X button, and a content area that shows the contents of
//! `/welcome.txt` from MARXARCH.
//!
//! Window manager:
//!   • Windows are stored back-to-front (last in vec = topmost = focused).
//!   • Click anywhere on a window raises it to the top.
//!   • Focused window has a bright title bar; inactive windows are dimmed.
//!   • Hit testing iterates topmost → bottom, first hit wins.
//!
//! Taskbar:
//!   • Gradient dark-blue bar across the full screen bottom (40 px tall).
//!   • "Start" button — green pill on the left; opens a new About window
//!     (cascaded). Will be replaced by a real Start menu in Phase 8.4.
//!   • Window task buttons in the middle — one per open window, the
//!     focused one is highlighted.
//!   • Uptime clock on the right — ticks from the PIT (HH:MM:SS since boot).

use alloc::string::String;
use alloc::vec::Vec;

use crate::{app_wm, cursor, framebuffer, fs, input, interrupts, mouse, ttf, wallpaper};
use marx_sdk::{Event as AppEvent, WindowId as AppWinId};

// Wallpaper is now an embedded JPG (assets/wallpaper.jpg) — see wallpaper.rs.
// The gradient/glow constants from earlier attempts have been retired.

// ---- window body palette ----
// Body uses a near-imperceptible vertical gradient — no longer flat white.
const BODY_TOP:      (u8, u8, u8) = (0xFD, 0xFE, 0xFF);
const BODY_BOT:      (u8, u8, u8) = (0xEE, 0xF1, 0xF6);
const BODY_TEXT:     (u8, u8, u8) = (0x18, 0x22, 0x33);

// ---- focused title-bar palette ----
// Glass tint: a cool blue translucent overlay.
const GLASS_TOP:     (u8, u8, u8) = (0xC4, 0xDC, 0xF0);
const GLASS_BOT:     (u8, u8, u8) = (0x84, 0xB4, 0xDE);
const TITLE_TEXT:    (u8, u8, u8) = (0x10, 0x2A, 0x4E);
// Inactive — neutral grey.
const GLASS_TOP_DIM: (u8, u8, u8) = (0xE0, 0xE3, 0xE8);
const GLASS_BOT_DIM: (u8, u8, u8) = (0xB4, 0xBB, 0xC4);
const TITLE_TEXT_DIM:(u8, u8, u8) = (0x4A, 0x52, 0x5C);

const WINDOW_BORDER:     (u8, u8, u8) = (0x3A, 0x60, 0x8C);
const WINDOW_BORDER_DIM: (u8, u8, u8) = (0x76, 0x80, 0x8A);

const CLOSE_NORMAL: (u8, u8, u8) = (0x20, 0x3A, 0x66);
const CLOSE_HOVER:  (u8, u8, u8) = (0xC8, 0x3A, 0x3A);
const CLOSE_PRESS:  (u8, u8, u8) = (0x8E, 0x1F, 0x1F);

// Minimize button — neutral grey hover (so it doesn't shout like the close button).
const MIN_HOVER:    (u8, u8, u8) = (0x5C, 0x80, 0xB0);
const MIN_PRESS:    (u8, u8, u8) = (0x32, 0x55, 0x80);

const TITLE_H:       usize = 30;
const CLOSE_BOX:     usize = 22;
const CLOSE_PAD_R:   usize = 6;
const BTN_GAP:       usize = 2;  // gap between minimize and close buttons
const WINDOW_RADIUS: usize = 6;

// ---- taskbar palette ----
const TASKBAR_H:    usize = 40;
const TBAR_TOP:     (u8, u8, u8) = (0x1C, 0x54, 0xB2);
const TBAR_BOT:     (u8, u8, u8) = (0x0F, 0x36, 0x82);
const TBAR_EDGE:    (u8, u8, u8) = (0x4A, 0x8A, 0xD8); // top highlight line
const TBAR_FG:      (u8, u8, u8) = (0xFF, 0xFF, 0xFF);

const START_W:  usize = 96;
const START_H:  usize = 30;
const START_NML: (u8, u8, u8) = (0x3E, 0xAA, 0x52); // XP green
const START_HOV: (u8, u8, u8) = (0x58, 0xCC, 0x6A);
const START_PRS: (u8, u8, u8) = (0x15, 0x6A, 0x2C);

// Task button (per open window in the taskbar)
const TASK_BTN_W:    usize = 160;
const TASK_BTN_H:    usize = 28;
const TASK_BTN_NML:  (u8, u8, u8) = (0x14, 0x4A, 0x96); // unfocused (darker)
const TASK_BTN_ACT:  (u8, u8, u8) = (0x3A, 0x8E, 0xE6); // focused (brighter, "pressed-in")
const TASK_BTN_EDGE_N:(u8, u8, u8) = (0x2A, 0x68, 0xB8);
const TASK_BTN_EDGE_A:(u8, u8, u8) = (0x9C, 0xCC, 0xFF);

// ---- Start menu ----
const MENU_W:        usize = 224;
const MENU_ROW_H:    usize = 34;
const MENU_PAD_TOP:  usize = 6;
const MENU_PAD_BOT:  usize = 6;
const MENU_HDR_H:    usize = 36;   // "MarX-OS" header strip at top
const MENU_RADIUS:   usize = 6;
const MENU_X:        usize = 4;    // left margin from screen edge

const MENU_TOP:      (u8, u8, u8) = (0xF2, 0xF6, 0xFC);
const MENU_BOT:      (u8, u8, u8) = (0xCE, 0xDB, 0xEC);
const MENU_BORDER:   (u8, u8, u8) = (0x3A, 0x5F, 0x8E);
const MENU_TEXT:     (u8, u8, u8) = (0x14, 0x22, 0x36);
const MENU_HOV_BG:   (u8, u8, u8) = (0x4A, 0x98, 0xE6);
const MENU_PRS_BG:   (u8, u8, u8) = (0x22, 0x6A, 0xC4);
const MENU_HDR_TOP:  (u8, u8, u8) = (0x1C, 0x4A, 0x9E);
const MENU_HDR_BOT:  (u8, u8, u8) = (0x0E, 0x2A, 0x68);

/// Menu item rows. Indices map to actions in `menu_invoke`.
const MENU_ITEMS: &[&str] = &[
    "About MarX-OS",
    "Run hello (.elf)",
    "Calculator",
    "Shut down",
    "Restart",
];

/// Total height of the menu given its current item count.
const fn menu_height() -> usize {
    MENU_HDR_H + MENU_PAD_TOP + MENU_ITEMS.len() * MENU_ROW_H + MENU_PAD_BOT
}

// ---- Right-click context menu (smaller, no header) ----
const CTX_W:        usize = 180;
const CTX_ROW_H:    usize = 28;
const CTX_PAD_TOP:  usize = 4;
const CTX_PAD_BOT:  usize = 4;
const CTX_RADIUS:   usize = 5;

const CTX_ITEMS: &[&str] = &[
    "Refresh",
    "About MarX-OS",
];

const fn ctx_menu_height() -> usize {
    CTX_PAD_TOP + CTX_ITEMS.len() * CTX_ROW_H + CTX_PAD_BOT
}

// ---- Window ----

struct Window {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    title: String,
    body: Vec<String>,
    close_hover: bool,
    close_pressed: bool,
    min_hover: bool,
    min_pressed: bool,
    /// Minimized windows are not painted on the desktop, but their entry
    /// remains in the WM so they keep their taskbar button.
    minimized: bool,
    dragging: bool,
    drag_offset: (i32, i32),
    /// If Some, this window's content area is owned by a user ELF app:
    /// the desktop blits the app's pixel buffer (from `app_wm`) instead
    /// of drawing the built-in `body` text.
    app_id: Option<AppWinId>,
}

impl Window {
    fn close_rect(&self) -> (i32, i32, usize, usize) {
        let cx = self.x + self.w as i32 - CLOSE_BOX as i32 - CLOSE_PAD_R as i32;
        let cy = self.y + (TITLE_H as i32 - CLOSE_BOX as i32) / 2;
        (cx, cy, CLOSE_BOX, CLOSE_BOX)
    }

    /// Minimize button sits immediately to the left of the close button.
    fn min_rect(&self) -> (i32, i32, usize, usize) {
        let (cx, cy, cw, ch) = self.close_rect();
        let mx = cx - CLOSE_BOX as i32 - BTN_GAP as i32;
        (mx, cy, cw, ch)
    }

    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y
            && px < self.x + self.w as i32
            && py < self.y + self.h as i32
    }

    fn close_contains(&self, px: i32, py: i32) -> bool {
        let (cx, cy, cw, ch) = self.close_rect();
        px >= cx && py >= cy && px < cx + cw as i32 && py < cy + ch as i32
    }

    fn min_contains(&self, px: i32, py: i32) -> bool {
        let (mx, my, mw, mh) = self.min_rect();
        px >= mx && py >= my && px < mx + mw as i32 && py < my + mh as i32
    }

    fn title_bar_contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y
            && px < self.x + self.w as i32
            && py < self.y + TITLE_H as i32
    }
}

// ---- WM ----

struct WM {
    windows: Vec<Window>,
    start_hover:   bool,
    start_pressed: bool,
    /// Start menu visibility.
    menu_open:     bool,
    /// Index of the menu row currently hovered, if any.
    menu_hover:    Option<usize>,
    /// Index of the menu row currently being pressed (mouse down), if any.
    menu_pressed:  Option<usize>,
    /// Right-click context menu state.
    ctx_open:      bool,
    ctx_x:         i32,
    ctx_y:         i32,
    ctx_hover:     Option<usize>,
    ctx_pressed:   Option<usize>,
    /// Index of the task button currently being hovered, if any.
    task_hover:    Option<usize>,
    /// Index of the task button currently being pressed (mouse down), if any.
    task_pressed:  Option<usize>,
    /// Total windows ever opened — used to number cascaded About windows
    /// and to offset their position so they don't perfectly stack.
    n_opened: u32,
}

impl WM {
    fn new() -> Self {
        Self {
            windows:       Vec::new(),
            start_hover:   false,
            start_pressed: false,
            menu_open:     false,
            menu_hover:    None,
            menu_pressed:  None,
            ctx_open:      false,
            ctx_x:         0,
            ctx_y:         0,
            ctx_hover:     None,
            ctx_pressed:   None,
            task_hover:    None,
            task_pressed:  None,
            n_opened:      0,
        }
    }

    /// Close the Start menu and clear any leftover hover/press feedback.
    fn close_menu(&mut self) {
        self.menu_open    = false;
        self.menu_hover   = None;
        self.menu_pressed = None;
    }

    /// Close the right-click context menu.
    fn close_ctx(&mut self) {
        self.ctx_open    = false;
        self.ctx_hover   = None;
        self.ctx_pressed = None;
    }

    fn open_about(&mut self) {
        self.n_opened += 1;
        let id = self.n_opened;

        let text = fs::read("welcome.txt")
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| String::from("Welcome to MarX-OS."));
        let body: Vec<String> = text.lines().map(|s| String::from(s)).collect();

        let title = if id == 1 {
            String::from("About MarX-OS")
        } else {
            let mut s = String::from("About MarX-OS #");
            push_uint(&mut s, id);
            s
        };

        let (sw, sh) = framebuffer::dimensions().unwrap_or((1280, 720));
        let desktop_h = sh.saturating_sub(TASKBAR_H);
        let w = 700_usize;
        let h = 420_usize;
        let base_x = ((sw - w) / 2) as i32;
        let base_y = ((desktop_h - h) / 2) as i32;
        // Cascade: each new window shifts +28 px down-right; wraps every 6.
        let off = ((id - 1) % 6) as i32 * 28;

        self.windows.push(Window {
            x: base_x + off,
            y: base_y + off,
            w, h,
            title,
            body,
            close_hover: false,
            close_pressed: false,
            min_hover: false,
            min_pressed: false,
            minimized: false,
            dragging: false,
            drag_offset: (0, 0),
            app_id: None,
        });
    }

    /// Create chrome for an app-owned window.  Sized so the title bar +
    /// content (TITLE_H px high + `content_h` px) fit; w == `content_w`.
    fn open_app_chrome(&mut self, app_id: AppWinId, title: String,
                       content_w: u32, content_h: u32) {
        self.n_opened += 1;
        let (sw, sh) = framebuffer::dimensions().unwrap_or((1280, 720));
        let desktop_h = sh.saturating_sub(TASKBAR_H);
        let w = content_w as usize;
        let h = content_h as usize + TITLE_H;
        // Centre, then cascade like About windows.
        let base_x = (sw.saturating_sub(w) / 2) as i32;
        let base_y = (desktop_h.saturating_sub(h) / 2) as i32;
        let off    = ((self.n_opened - 1) % 6) as i32 * 28;
        self.windows.push(Window {
            x: base_x + off,
            y: base_y + off,
            w, h,
            title,
            body: Vec::new(),
            close_hover: false,
            close_pressed: false,
            min_hover: false,
            min_pressed: false,
            minimized: false,
            dragging: false,
            drag_offset: (0, 0),
            app_id: Some(app_id),
        });
    }

    /// Move the window at index `idx` to the top of the z-order (end of vec).
    /// Returns `true` if anything actually changed.
    fn raise(&mut self, idx: usize) -> bool {
        if idx + 1 >= self.windows.len() { return false; }
        let win = self.windows.remove(idx);
        self.windows.push(win);
        true
    }

    /// Index of the topmost VISIBLE (non-minimized) window under (px, py).
    /// Minimized windows are invisible, so they're skipped.
    fn topmost_at(&self, px: i32, py: i32) -> Option<usize> {
        self.windows.iter().enumerate().rev()
            .find(|(_, w)| !w.minimized && w.contains(px, py))
            .map(|(i, _)| i)
    }

    /// Index of the currently focused window — the topmost non-minimized one.
    fn focused_idx(&self) -> Option<usize> {
        self.windows.iter().enumerate().rev()
            .find(|(_, w)| !w.minimized)
            .map(|(i, _)| i)
    }

    /// Task-button click semantics:
    ///   • Window is focused & visible → minimize it.
    ///   • Window is minimized        → restore + raise + focus.
    ///   • Window is unfocused        → raise + focus.
    fn toggle_task(&mut self, idx: usize) {
        if idx >= self.windows.len() { return; }
        let focused = self.focused_idx() == Some(idx);
        let was_minimized = self.windows[idx].minimized;
        if was_minimized {
            self.windows[idx].minimized = false;
            self.raise(idx);
        } else if focused {
            self.windows[idx].minimized = true;
            // Clear any leftover hover/press feedback.
            self.windows[idx].close_hover   = false;
            self.windows[idx].close_pressed = false;
            self.windows[idx].min_hover     = false;
            self.windows[idx].min_pressed   = false;
            self.windows[idx].dragging      = false;
        } else {
            self.raise(idx);
        }
    }
}

// ---- helpers ----

/// Format a tick count as "HH:MM:SS" (PIT runs at ~18.2 Hz).
fn format_uptime(ticks: u64) -> String {
    let secs = ticks / 18;
    let h = (secs / 3600) % 100; // cap at 99 h
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut out = String::with_capacity(8);
    push_two(&mut out, h as u8);
    out.push(':');
    push_two(&mut out, m as u8);
    out.push(':');
    push_two(&mut out, s as u8);
    out
}

fn push_two(s: &mut String, n: u8) {
    s.push((b'0' + (n / 10).min(9)) as char);
    s.push((b'0' + (n % 10))       as char);
}

/// Append a base-10 unsigned integer to a String. Used for window numbering.
fn push_uint(s: &mut String, n: u32) {
    if n == 0 { s.push('0'); return; }
    let mut buf = [0u8; 10];
    let mut i = 0usize;
    let mut v = n;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        i += 1;
        v /= 10;
    }
    for k in (0..i).rev() {
        s.push(buf[k] as char);
    }
}

// ---- taskbar geometry ----

/// Returns (x, y, w, h) of the Start button in screen coordinates.
fn start_rect(sh: usize) -> (i32, i32, usize, usize) {
    let ty = sh - TASKBAR_H;
    let x  = 6_i32;
    let y  = ty as i32 + (TASKBAR_H as i32 - START_H as i32) / 2;
    (x, y, START_W, START_H)
}

fn start_contains(sh: usize, px: i32, py: i32) -> bool {
    let (sx, sy, sw, sh2) = start_rect(sh);
    px >= sx && py >= sy && px < sx + sw as i32 && py < sy + sh2 as i32
}

/// Returns (x, y, w, h) of the k-th task button in the taskbar.
fn task_btn_rect(sh: usize, k: usize) -> (i32, i32, usize, usize) {
    let ty   = sh - TASKBAR_H;
    let left = 6 + START_W + 8; // right of Start + gap
    let x    = left + k * (TASK_BTN_W + 4);
    let y    = ty as i32 + (TASKBAR_H as i32 - TASK_BTN_H as i32) / 2;
    (x as i32, y, TASK_BTN_W, TASK_BTN_H)
}

/// Returns the index of the task button that contains (px, py), or None.
/// Used for mouse hit-testing on the taskbar.
fn task_btn_at(wm: &WM, sh: usize, px: i32, py: i32) -> Option<usize> {
    for k in 0..wm.windows.len() {
        let (x, y, w, h) = task_btn_rect(sh, k);
        if px >= x && py >= y && px < x + w as i32 && py < y + h as i32 {
            return Some(k);
        }
    }
    None
}

// ---- Start menu geometry ----

/// Returns (x, y, w, h) of the Start menu in screen coordinates.
fn menu_rect(sh: usize) -> (i32, i32, usize, usize) {
    let h = menu_height();
    let y = sh.saturating_sub(TASKBAR_H + h);
    (MENU_X as i32, y as i32, MENU_W, h)
}

/// True if (px, py) is inside the Start menu rectangle.
fn menu_contains(sh: usize, px: i32, py: i32) -> bool {
    let (mx, my, mw, mh) = menu_rect(sh);
    px >= mx && py >= my && px < mx + mw as i32 && py < my + mh as i32
}

/// Returns the index of the menu row at (px, py), or None.
fn menu_item_at(sh: usize, px: i32, py: i32) -> Option<usize> {
    let (mx, my, _mw, _mh) = menu_rect(sh);
    let rows_x = mx + 4;
    let rows_w = MENU_W as i32 - 8;
    let rows_y0 = my + MENU_HDR_H as i32 + MENU_PAD_TOP as i32;
    for i in 0..MENU_ITEMS.len() {
        let row_y = rows_y0 + (i * MENU_ROW_H) as i32;
        if px >= rows_x && py >= row_y
            && px < rows_x + rows_w
            && py < row_y + MENU_ROW_H as i32 - 2
        {
            return Some(i);
        }
    }
    None
}

// ---- Start menu actions ----

/// Execute the action associated with menu row `idx`.
fn menu_invoke(idx: usize, wm: &mut WM) {
    match idx {
        0 => wm.open_about(),
        1 => launch_app("hello.elf"),
        2 => launch_app("calculator.elf"),
        3 => shutdown(),
        4 => reboot(),
        _ => {}
    }
}

fn launch_app(name: &str) {
    match crate::app::launch(name) {
        Ok(id) => crate::sprintln!("[desktop] launched {} as task #{}", name, id),
        Err(e) => crate::sprintln!("[desktop] failed to launch {}: {}", name, e),
    }
}

// ---- Right-click context menu geometry ----

/// Returns (x, y, w, h) of the context menu, clamped to the screen so it
/// doesn't fall off the right/bottom edges.
fn ctx_rect(sw: usize, sh: usize, anchor_x: i32, anchor_y: i32) -> (i32, i32, usize, usize) {
    let h = ctx_menu_height();
    let max_x = (sw as i32).saturating_sub(CTX_W as i32);
    let max_y = (sh as i32 - TASKBAR_H as i32).saturating_sub(h as i32);
    let x = anchor_x.min(max_x).max(0);
    let y = anchor_y.min(max_y).max(0);
    (x, y, CTX_W, h)
}

fn ctx_contains(wm: &WM, sw: usize, sh: usize, px: i32, py: i32) -> bool {
    let (cx, cy, cw, ch) = ctx_rect(sw, sh, wm.ctx_x, wm.ctx_y);
    px >= cx && py >= cy && px < cx + cw as i32 && py < cy + ch as i32
}

fn ctx_item_at(wm: &WM, sw: usize, sh: usize, px: i32, py: i32) -> Option<usize> {
    if !wm.ctx_open { return None; }
    let (mx, my, _mw, _mh) = ctx_rect(sw, sh, wm.ctx_x, wm.ctx_y);
    let rows_x  = mx + 3;
    let rows_w  = CTX_W as i32 - 6;
    let rows_y0 = my + CTX_PAD_TOP as i32;
    for i in 0..CTX_ITEMS.len() {
        let row_y = rows_y0 + (i * CTX_ROW_H) as i32;
        if px >= rows_x && py >= row_y
            && px < rows_x + rows_w
            && py < row_y + CTX_ROW_H as i32 - 2
        {
            return Some(i);
        }
    }
    None
}

/// Execute the action associated with context-menu row `idx`.
/// Returns `true` if the desktop should be force-repainted afterwards.
fn ctx_invoke(idx: usize, wm: &mut WM) -> bool {
    match idx {
        0 => true,                    // Refresh — just trigger repaint
        1 => { wm.open_about(); true }
        _ => false,
    }
}

/// Try to shut the machine down. Under QEMU we use the PIIX4 / Bochs ACPI
/// "soft off" registers — these cause QEMU to exit cleanly. On real hardware
/// they're no-ops, so we fall through to a permanent halt.
fn shutdown() -> ! {
    use x86_64::instructions::port::Port;
    unsafe {
        // PIIX4 power management (QEMU's default machine).
        Port::<u16>::new(0x604).write(0x2000);
        // Bochs / older QEMU.
        Port::<u16>::new(0xB004).write(0x2000);
        // VirtualBox / Bochs alternate.
        Port::<u16>::new(0x4004).write(0x3400);
    }
    // We're still alive → fall back to a permanent halt.
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// Try to reboot the machine. The classic trick: tell the 8042 keyboard
/// controller to pulse the CPU reset line by writing 0xFE to port 0x64.
/// QEMU recognises this and restarts. Falls back to a triple-fault.
fn reboot() -> ! {
    use x86_64::instructions::port::Port;
    unsafe {
        Port::<u8>::new(0x64).write(0xFE);
    }
    // 8042 reset failed → force a triple fault by loading a null IDT and
    // raising an interrupt. CPU has no IDT entry, can't dispatch fault,
    // can't dispatch the resulting double fault either, so it triple-faults
    // and the platform resets.
    unsafe {
        use x86_64::structures::DescriptorTablePointer;
        let null = DescriptorTablePointer { limit: 0, base: x86_64::VirtAddr::new(0) };
        x86_64::instructions::tables::lidt(&null);
        core::arch::asm!("int3", options(nomem, nostack));
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

// ---- painting ----

fn paint_wallpaper(sh: usize) {
    if framebuffer::dimensions().is_none() { return; }
    // Blit the embedded Frutiger Aero wallpaper, but only rows above the
    // taskbar — that area gets repainted by `paint_taskbar` anyway.
    let desktop_h = sh - TASKBAR_H;
    framebuffer::blit_rgb_rows(
        wallpaper::WALLPAPER_RGB,
        wallpaper::WALLPAPER_W,
        wallpaper::WALLPAPER_H,
        0, desktop_h,
    );
}

fn paint_taskbar(wm: &WM, sw: usize, sh: usize) {
    let ty = sh - TASKBAR_H;

    // Background gradient.
    framebuffer::fill_gradient_v(ty, sh, TBAR_TOP, TBAR_BOT);
    // Top highlight line.
    framebuffer::fill_rect(0, ty, sw, 1, TBAR_EDGE);

    // ---- Start button ----
    let (sx, sy, s_w, s_h) = start_rect(sh);
    let start_col = if wm.start_pressed { START_PRS }
                    else if wm.start_hover { START_HOV }
                    else { START_NML };
    framebuffer::fill_rounded_rect(sx as usize, sy as usize, s_w, s_h, start_col, 8);
    // Subtle inner highlight strip on the upper half.
    if !wm.start_pressed {
        framebuffer::fill_rounded_rect(
            sx as usize + 2, sy as usize + 2, s_w - 4, s_h / 2 - 2,
            lighten(start_col, 30), 6,
        );
    }
    // "Start" label.
    let lbl      = "Start";
    let lbl_sz   = 15.0_f32;
    let lbl_w    = ttf::text_width(lbl, lbl_sz) as i32;
    let lbl_x    = sx + (s_w as i32 - lbl_w) / 2;
    let lbl_y    = sy + (s_h as i32 + ttf::ascent(lbl_sz) as i32) / 2 - 1;
    // Soft drop-shadow then white text.
    ttf::draw_text(lbl_x + 1, lbl_y + 1, lbl, lbl_sz, (0x00, 0x30, 0x00));
    ttf::draw_text(lbl_x,     lbl_y,     lbl, lbl_sz, TBAR_FG);

    // ---- Window task buttons ----
    let focused = wm.focused_idx();
    for (k, win) in wm.windows.iter().enumerate() {
        let (bx, by, bw, bh) = task_btn_rect(sh, k);
        let is_focused = Some(k) == focused;
        let is_hover   = wm.task_hover   == Some(k);
        let is_press   = wm.task_pressed == Some(k);

        // Base colour: pressed > focused > hover > normal.
        let (fill, edge) = if is_press {
            ((0x10, 0x3E, 0x82), (0x70, 0xA8, 0xE0))
        } else if is_focused {
            (TASK_BTN_ACT, TASK_BTN_EDGE_A)
        } else if is_hover {
            ((0x24, 0x6A, 0xC8), (0x4A, 0x98, 0xE6))
        } else {
            (TASK_BTN_NML, TASK_BTN_EDGE_N)
        };
        framebuffer::fill_rounded_rect(bx as usize, by as usize, bw, bh, fill, 4);
        framebuffer::stroke_rounded_rect(bx as usize, by as usize, bw, bh, edge, 4);
        // Inner top highlight on the focused button for a "pressed-in" look.
        if is_focused && !is_press {
            framebuffer::fill_rect(
                bx as usize + 2, by as usize + 2, bw - 4, 1,
                (0xCC, 0xE6, 0xFF),
            );
        }
        // Small bracket on the left for minimized windows (┤ look).
        if win.minimized {
            framebuffer::fill_rect(
                bx as usize + 4, by as usize + 4, 2, (bh - 8).max(1),
                (0xA8, 0xC0, 0xE0),
            );
        }
        // Truncate title to fit inside the button.
        let btn_sz  = 13.0_f32;
        let text_left = if win.minimized { 14 } else { 12 };
        let max_w   = bw.saturating_sub(text_left + 4) as u32;
        let title   = truncate_to_width(&win.title, btn_sz, max_w);
        let tw      = ttf::text_width(&title, btn_sz) as i32;
        // Left-align text if minimized (so the bracket shows), centre otherwise.
        let tx = if win.minimized {
            bx + text_left as i32
        } else {
            bx + (bw as i32 - tw) / 2
        };
        let ty2 = by + (bh as i32 + ttf::ascent(btn_sz) as i32) / 2 - 1;
        let txt_col = if win.minimized { (0xCC, 0xD8, 0xEC) } else { TBAR_FG };
        ttf::draw_text(tx, ty2, &title, btn_sz, txt_col);
    }

    // ---- Clock (right-aligned) ----
    let clock    = format_uptime(interrupts::ticks());
    let clk_sz   = 13.0_f32;
    let clk_w    = ttf::text_width(&clock, clk_sz) as i32;
    let clk_x    = sw as i32 - clk_w - 12;
    let clk_y    = ty as i32 + (TASKBAR_H as i32 + ttf::ascent(clk_sz) as i32) / 2 - 1;
    // Shadow then white.
    ttf::draw_text(clk_x + 1, clk_y + 1, &clock, clk_sz, (0x00, 0x00, 0x40));
    ttf::draw_text(clk_x,     clk_y,     &clock, clk_sz, TBAR_FG);
}

/// Paint the Start menu popup. No-op when the menu is closed.
fn paint_start_menu(wm: &WM, sh: usize) {
    if !wm.menu_open { return; }
    let (mx, my, mw, mh) = menu_rect(sh);
    let (mx, my) = (mx as usize, my as usize);

    // Drop shadow underneath.
    framebuffer::fill_rounded_rect(mx + 3, my + 3, mw, mh,
                                   (0x0F, 0x21, 0x3D), MENU_RADIUS);
    // Body gradient (Aero glass).
    framebuffer::fill_v_gradient_in_rounded_rect(
        mx, my, mw, mh,
        MENU_TOP, MENU_BOT,
        mx, my, mw, mh, MENU_RADIUS,
    );

    // Header strip: dark blue gradient with "MarX-OS" branding.
    framebuffer::fill_v_gradient_in_rounded_rect(
        mx, my, mw, MENU_HDR_H,
        MENU_HDR_TOP, MENU_HDR_BOT,
        mx, my, mw, mh, MENU_RADIUS,
    );
    // 1-px separator below header.
    framebuffer::fill_rect(mx + 1, my + MENU_HDR_H, mw - 2, 1,
                           (0x0A, 0x20, 0x4A));
    framebuffer::fill_rect(mx + 1, my + MENU_HDR_H + 1, mw - 2, 1,
                           (0xE8, 0xEE, 0xF6));

    // Brand text.
    let brand    = "MarX-OS";
    let brand_sz = 19.0_f32;
    let brand_y  = my as i32
        + (MENU_HDR_H as i32 + ttf::ascent(brand_sz) as i32) / 2 - 2;
    ttf::draw_text(mx as i32 + 12 + 1, brand_y + 1, brand, brand_sz,
                   (0x05, 0x10, 0x28));
    ttf::draw_text(mx as i32 + 12,     brand_y,     brand, brand_sz,
                   (0xFF, 0xFF, 0xFF));

    // Rows.
    let rows_x = mx + 4;
    let rows_w = mw - 8;
    let rows_y0 = my + MENU_HDR_H + MENU_PAD_TOP;
    for (i, label) in MENU_ITEMS.iter().enumerate() {
        let row_y = rows_y0 + i * MENU_ROW_H;
        let row_h = MENU_ROW_H - 2;

        let is_hov = wm.menu_hover   == Some(i);
        let is_prs = wm.menu_pressed == Some(i);
        if is_hov || is_prs {
            let bg = if is_prs { MENU_PRS_BG } else { MENU_HOV_BG };
            framebuffer::fill_rounded_rect(rows_x, row_y, rows_w, row_h, bg, 4);
        }

        // Icon swatch (24×24) on the left — colour-coded per action.
        let icon_col = match i {
            0 => (0x4A, 0x90, 0xE0), // About — blue
            1 => (0xD8, 0x4A, 0x3A), // Shut down — red
            2 => (0x4A, 0xB8, 0x5A), // Restart — green
            _ => (0x80, 0x80, 0x80),
        };
        let icon_x = rows_x as i32 + 4;
        let icon_y = row_y as i32 + (row_h as i32 - 22) / 2;
        framebuffer::fill_rounded_rect(icon_x as usize, icon_y as usize,
                                       22, 22, icon_col, 4);
        // Tiny inner highlight on the icon.
        framebuffer::fill_rect(icon_x as usize + 2, icon_y as usize + 2,
                               18, 2, lighten(icon_col, 50));

        // Label text.
        let txt_sz = 14.0_f32;
        let txt_x  = icon_x + 32;
        let txt_y  = row_y as i32 + (row_h as i32 + ttf::ascent(txt_sz) as i32) / 2 - 1;
        let txt_c  = if is_hov || is_prs { (0xFF, 0xFF, 0xFF) } else { MENU_TEXT };
        ttf::draw_text(txt_x, txt_y, label, txt_sz, txt_c);
    }

    // Outer border last — sits on top of everything so it's crisp.
    framebuffer::stroke_rounded_rect(mx, my, mw, mh, MENU_BORDER, MENU_RADIUS);
}

/// Paint the right-click context menu. No-op when closed.
fn paint_ctx_menu(wm: &WM, sw: usize, sh: usize) {
    if !wm.ctx_open { return; }
    let (mx, my, mw, mh) = ctx_rect(sw, sh, wm.ctx_x, wm.ctx_y);
    let (mx, my) = (mx as usize, my as usize);

    // Drop shadow.
    framebuffer::fill_rounded_rect(mx + 2, my + 2, mw, mh,
                                   (0x0F, 0x21, 0x3D), CTX_RADIUS);
    // Aero glass body.
    framebuffer::fill_v_gradient_in_rounded_rect(
        mx, my, mw, mh,
        MENU_TOP, MENU_BOT,
        mx, my, mw, mh, CTX_RADIUS,
    );

    // Rows.
    let rows_x  = mx + 3;
    let rows_w  = mw - 6;
    let rows_y0 = my + CTX_PAD_TOP;
    for (i, label) in CTX_ITEMS.iter().enumerate() {
        let row_y = rows_y0 + i * CTX_ROW_H;
        let row_h = CTX_ROW_H - 2;
        let is_hov = wm.ctx_hover   == Some(i);
        let is_prs = wm.ctx_pressed == Some(i);
        if is_hov || is_prs {
            let bg = if is_prs { MENU_PRS_BG } else { MENU_HOV_BG };
            framebuffer::fill_rounded_rect(rows_x, row_y, rows_w, row_h, bg, 3);
        }
        let sz = 13.0_f32;
        let tx = rows_x as i32 + 12;
        let ty = row_y as i32 + (row_h as i32 + ttf::ascent(sz) as i32) / 2 - 1;
        let col = if is_hov || is_prs { (0xFF, 0xFF, 0xFF) } else { MENU_TEXT };
        ttf::draw_text(tx, ty, label, sz, col);
    }

    // Border last.
    framebuffer::stroke_rounded_rect(mx, my, mw, mh, MENU_BORDER, CTX_RADIUS);
}

/// Lighten an RGB colour by adding `amt` to each channel (saturating).
fn lighten(c: (u8, u8, u8), amt: u8) -> (u8, u8, u8) {
    (c.0.saturating_add(amt), c.1.saturating_add(amt), c.2.saturating_add(amt))
}

/// Return a prefix of `text` that fits within `max_px` pixels at `size`.
/// Appends "…" if the string was cut.
fn truncate_to_width(text: &str, size: f32, max_px: u32) -> String {
    if ttf::text_width(text, size) <= max_px {
        return String::from(text);
    }
    let ellipsis_w = ttf::text_width("…", size);
    let mut out = String::new();
    for ch in text.chars() {
        let candidate = {
            let mut s = out.clone();
            s.push(ch);
            s
        };
        if ttf::text_width(&candidate, size) + ellipsis_w > max_px {
            break;
        }
        out = candidate;
    }
    out.push('…');
    out
}

fn paint_window(win: &Window, focused: bool) {
    let wx = win.x as usize;
    let wy = win.y as usize;
    let ww = win.w;
    let wh = win.h;
    let body_y = wy + TITLE_H;
    let body_h = wh - TITLE_H;

    // Pick palette based on focus state.
    let (g_top, g_bot, t_txt, border) = if focused {
        (GLASS_TOP, GLASS_BOT, TITLE_TEXT, WINDOW_BORDER)
    } else {
        (GLASS_TOP_DIM, GLASS_BOT_DIM, TITLE_TEXT_DIM, WINDOW_BORDER_DIM)
    };

    // ---- 1. Soft multi-layer drop shadow ----
    // Outer = wide & faint; inner = tight & darker → smooth falloff.
    let shadow_col = (0x00, 0x10, 0x28);
    let layers: &[(i32, u8)] = if focused {
        &[(7, 14), (5, 22), (3, 38), (2, 60)]
    } else {
        &[(6, 10), (4, 18), (2, 30)]
    };
    for &(off, alpha) in layers {
        framebuffer::blend_rounded_rect(
            (win.x + off) as usize, (win.y + off) as usize,
            ww, wh, shadow_col, alpha, WINDOW_RADIUS + 1,
        );
    }

    // ---- 2. GLASS TITLE BAR — alpha-blended OVER WALLPAPER ----
    // Painted BEFORE the body so the wallpaper actually shows through.
    // Low alpha (95→145) means the title is visibly translucent.
    framebuffer::blend_v_gradient_in_rounded_rect(
        wx, wy, ww, TITLE_H,
        g_top, g_bot,
        if focused {  95 } else { 130 },
        if focused { 145 } else { 175 },
        wx, wy, ww, wh, WINDOW_RADIUS,
    );

    // ---- 3. Glossy specular highlight at the very top ----
    framebuffer::blend_v_gradient_in_rounded_rect(
        wx, wy, ww, 10,
        (0xFF, 0xFF, 0xFF), (0xFF, 0xFF, 0xFF),
        if focused { 160 } else { 110 }, 0,
        wx, wy, ww, wh, WINDOW_RADIUS,
    );

    // ---- 4. Body — opaque gradient, rounded BOTTOM corners only ----
    framebuffer::fill_v_gradient_rect_rounded_bottom(
        wx, body_y, ww, body_h, BODY_TOP, BODY_BOT, WINDOW_RADIUS,
    );

    // ---- 5. Thin separator between title and body ----
    let sep_top = if focused { (0x32, 0x58, 0x88) } else { (0x6C, 0x76, 0x80) };
    framebuffer::fill_rect(wx + 1, wy + TITLE_H,     ww - 2, 1, sep_top);
    framebuffer::blend_rect(wx + 1, wy + TITLE_H + 1, ww - 2, 1,
                            (0xFF, 0xFF, 0xFF), 90);

    // ---- 6. Outer border (last so it's crisp) ----
    framebuffer::stroke_rounded_rect(wx, wy, ww, wh, border, WINDOW_RADIUS);

    // Title text — tiny shadow under it for depth on the focused window.
    let title_size = 16.0_f32;
    let title_y = win.y + (TITLE_H as i32 + ttf::ascent(title_size) as i32) / 2;
    if focused {
        ttf::draw_text(win.x + 13, title_y + 1, &win.title, title_size,
                       (0xFF, 0xFF, 0xFF));
    }
    ttf::draw_text(win.x + 12, title_y, &win.title, title_size, t_txt);

    // ---- Minimize button ----
    let (mx, my, mw, mh) = win.min_rect();
    let show_min_hi = focused && (win.min_hover || win.min_pressed);
    if show_min_hi {
        let bg = if win.min_pressed { MIN_PRESS } else { MIN_HOVER };
        framebuffer::fill_rounded_rect(mx as usize, my as usize, mw, mh, bg, 3);
        framebuffer::stroke_rounded_rect(mx as usize, my as usize, mw, mh,
                                         (0x20, 0x3A, 0x66), 3);
    }
    // The "_" glyph: a 2-px-tall horizontal bar near the bottom of the cell.
    let icon_col = if show_min_hi { (0xFF, 0xFF, 0xFF) }
                   else if focused { CLOSE_NORMAL }
                   else            { (0x70, 0x78, 0x82) };
    let pad = 6_usize;
    let bar_y0 = my as usize + mh - pad - 2;
    for dy in 0..2usize {
        for i in 0..(mw - pad * 2) {
            framebuffer::put_pixel_at(
                mx as usize + pad + i, bar_y0 + dy,
                icon_col.0, icon_col.1, icon_col.2,
            );
        }
    }

    // ---- Close button ----
    let (cx, cy, cw, ch) = win.close_rect();
    let show_close_hi = focused && (win.close_hover || win.close_pressed);
    if show_close_hi {
        let bg = if win.close_pressed { CLOSE_PRESS } else { CLOSE_HOVER };
        framebuffer::fill_rounded_rect(cx as usize, cy as usize, cw, ch, bg, 3);
        framebuffer::stroke_rounded_rect(cx as usize, cy as usize, cw, ch,
                                         (0x60, 0x14, 0x14), 3);
    }
    let x_color = if show_close_hi { (0xFF, 0xFF, 0xFF) }
                  else if focused  { CLOSE_NORMAL }
                  else             { (0x70, 0x78, 0x82) };
    for i in 0..(cw - pad * 2) {
        framebuffer::put_pixel_at(
            cx as usize + pad + i,     cy as usize + pad + i, x_color.0, x_color.1, x_color.2);
        framebuffer::put_pixel_at(
            cx as usize + pad + i + 1, cy as usize + pad + i, x_color.0, x_color.1, x_color.2);
        framebuffer::put_pixel_at(
            cx as usize + cw - pad - 1 - i, cy as usize + pad + i, x_color.0, x_color.1, x_color.2);
        framebuffer::put_pixel_at(
            cx as usize + cw - pad - 2 - i, cy as usize + pad + i, x_color.0, x_color.1, x_color.2);
    }

    // Body content: app windows blit their pixel buffer; built-in windows
    // render the welcome.txt lines stored in `win.body`.
    if let Some(app_id) = win.app_id {
        let body_x = wx;
        let body_y = wy + TITLE_H + 1;          // +1 to clear the separator
        app_wm::with_content(app_id, |content, cw, ch| {
            framebuffer::blit_rgb_at(content, cw, ch, body_x, body_y);
        });
    } else {
        let body_size = 15.0_f32;
        let line_h    = (body_size * 1.35) as i32;
        let mut baseline = win.y + TITLE_H as i32 + 24;
        for line in &win.body {
            if baseline > win.y + win.h as i32 - 12 { break; }
            ttf::draw_text(win.x + 16, baseline, line, body_size, BODY_TEXT);
            baseline += line_h;
        }
    }
}

fn repaint_all(wm: &WM) {
    use x86_64::instructions::interrupts as ints;
    let (sw, sh) = match framebuffer::dimensions() { Some(d) => d, None => return };
    let focused = wm.focused_idx();
    ints::without_interrupts(|| {
        paint_wallpaper(sh);
        // Paint visible windows back-to-front; minimized ones are hidden.
        for (i, win) in wm.windows.iter().enumerate() {
            if win.minimized { continue; }
            paint_window(win, Some(i) == focused);
        }
        paint_taskbar(wm, sw, sh);
        // Menu floats over taskbar + windows.
        paint_start_menu(wm, sh);
        // Right-click context menu sits on top of everything.
        paint_ctx_menu(wm, sw, sh);
        cursor::invalidate();
        framebuffer::present();
    });
}

// ---- event loop ----

pub fn run() -> ! {
    use x86_64::instructions::interrupts as ints;

    let mut wm = WM::new();
    wm.open_about();
    repaint_all(&wm);

    let mut prev       = mouse::state();
    let mut prev_ticks = interrupts::ticks();

    loop {
        ints::enable_and_hlt();
        let now   = mouse::state();
        let ticks = interrupts::ticks();
        let mut dirty = false;

        let pressed_just_now  = !prev.left &&  now.left;
        let released_just_now =  prev.left && !now.left;
        let rpressed_just_now = !prev.right && now.right;
        let (sw, sh) = framebuffer::dimensions().unwrap_or((1280, 720));

        // ---- clock tick → redraw once per second ----
        if ticks / 18 != prev_ticks / 18 {
            dirty = true;
        }

        // ---- 0a. App-presented frame → desktop must repaint ----
        if app_wm::APP_DIRTY.swap(false, core::sync::atomic::Ordering::AcqRel) {
            dirty = true;
        }

        // ---- 0a'. Drain keyboard ring → focused app's event queue ----
        // PS/2 keyboard IRQ pushes ASCII bytes into `input::INPUT`. If the
        // currently focused window is an app, deliver each byte as a
        // `KeyDown` event.  Built-in windows (About) ignore keystrokes.
        // If nothing is focused, keystrokes are dropped.
        while let Some(b) = input::try_pop() {
            if let Some(idx) = wm.focused_idx() {
                if let Some(app_id) = wm.windows[idx].app_id {
                    app_wm::push_event(app_id, AppEvent::KeyDown { ch: b });
                }
            }
        }

        // ---- 0b. Drain pending app ops (open/close window) ----
        for op in app_wm::drain_ops() {
            match op {
                app_wm::AppOp::OpenWindow { app_id, title, content_w, content_h } => {
                    wm.open_app_chrome(app_id, title, content_w, content_h);
                    dirty = true;
                }
                app_wm::AppOp::CloseWindow { app_id } => {
                    wm.windows.retain(|w| w.app_id != Some(app_id));
                    app_wm::destroy(app_id);
                    dirty = true;
                }
            }
        }

        // ---- 0c. Route mouse events into the focused app window ----
        // Forward MouseDown/Up/Move (in window-local coords) when the
        // topmost-under-cursor is an app window AND the cursor is in its
        // content area (below the title bar).
        if let Some(idx) = wm.topmost_at(now.x, now.y) {
            if let Some(app_id) = wm.windows[idx].app_id {
                let w = &wm.windows[idx];
                let content_top = w.y + TITLE_H as i32 + 1;
                if now.y >= content_top {
                    let lx = now.x - w.x;
                    let ly = now.y - content_top;
                    if pressed_just_now {
                        app_wm::push_event(app_id, AppEvent::MouseDown { x: lx, y: ly });
                    }
                    if released_just_now {
                        app_wm::push_event(app_id, AppEvent::MouseUp { x: lx, y: ly });
                    }
                }
            }
        }

        // ---- 0. Press outside menu (and not on Start) closes the menu ----
        // Done first so subsequent window-press logic still propagates.
        if pressed_just_now && wm.menu_open
            && !menu_contains(sh, now.x, now.y)
            && !start_contains(sh, now.x, now.y)
        {
            wm.close_menu();
            dirty = true;
        }

        // ---- 0b. Press outside context menu closes it ----
        if pressed_just_now && wm.ctx_open
            && !ctx_contains(&wm, sw, sh, now.x, now.y)
        {
            wm.close_ctx();
            dirty = true;
        }

        // ---- 0c. Right-click opens a context menu (or closes the open one) ----
        if rpressed_just_now {
            // Don't open on top of the taskbar / Start menu / existing ctx.
            let in_taskbar = now.y >= (sh as i32) - TASKBAR_H as i32;
            let on_menu    = wm.menu_open && menu_contains(sh, now.x, now.y);
            let on_window  = wm.topmost_at(now.x, now.y).is_some();
            if wm.ctx_open {
                wm.close_ctx();
                dirty = true;
            } else if !in_taskbar && !on_menu && !on_window {
                wm.ctx_open    = true;
                wm.ctx_x       = now.x;
                wm.ctx_y       = now.y;
                wm.ctx_hover   = None;
                wm.ctx_pressed = None;
                // Close the Start menu if it was open — only one popup at a time.
                if wm.menu_open { wm.close_menu(); }
                dirty = true;
            }
        }

        // ---- 1. Press on a window → raise it (unless on close/min button) ----
        // Skip window press handling if the press is inside an open popup —
        // popup rows shouldn't accidentally raise underlying windows.
        let press_in_menu = pressed_just_now && wm.menu_open
            && menu_contains(sh, now.x, now.y);
        let press_in_ctx  = pressed_just_now && wm.ctx_open
            && ctx_contains(&wm, sw, sh, now.x, now.y);
        if pressed_just_now && !press_in_menu && !press_in_ctx {
            if let Some(idx) = wm.topmost_at(now.x, now.y) {
                if wm.raise(idx) {
                    dirty = true;
                }
                // After raise, target window is at the end.
                let last_idx = wm.windows.len() - 1;
                let last = &mut wm.windows[last_idx];
                if last.title_bar_contains(now.x, now.y)
                    && !last.close_contains(now.x, now.y)
                    && !last.min_contains(now.x, now.y)
                {
                    last.dragging    = true;
                    last.drag_offset = (now.x - last.x, now.y - last.y);
                }
            }
        }

        // ---- 2. Drag continue/end on the topmost window ----
        // Clamp window position to the screen so it stays fully visible —
        // negative coords break the framebuffer primitives (`x as usize`
        // wraps and fill-rect loops never iterate). The title-bar text uses
        // signed i32 in ttf, so it stays visible — that's how the bug looked
        // like "the body disappeared but text remained".
        if let Some(last) = wm.windows.last_mut() {
            if released_just_now && last.dragging {
                last.dragging = false;
                dirty = true;
            }
            if last.dragging {
                let max_x = (sw as i32).saturating_sub(last.w as i32);
                let max_y = (sh as i32) - TASKBAR_H as i32 - TITLE_H as i32;
                let new_x = (now.x - last.drag_offset.0).max(0).min(max_x);
                let new_y = (now.y - last.drag_offset.1).max(0).min(max_y);
                if new_x != last.x || new_y != last.y {
                    last.x = new_x;
                    last.y = new_y;
                    dirty = true;
                }
            }
        }

        // ---- 3. Close/min hover & press — only on the focused window ----
        // (Covered windows can't be clicked through, and minimized ones are
        // hidden, so feedback would be invisible anyway.)
        let focused = wm.focused_idx();
        let topmost_under_now = wm.topmost_at(now.x, now.y);
        for (i, win) in wm.windows.iter_mut().enumerate() {
            let is_top = Some(i) == topmost_under_now && Some(i) == focused;
            let c_hov  = is_top && !win.dragging && win.close_contains(now.x, now.y);
            let c_prs  = c_hov && now.left;
            let m_hov  = is_top && !win.dragging && win.min_contains(now.x, now.y);
            let m_prs  = m_hov && now.left;
            if c_hov != win.close_hover || c_prs != win.close_pressed
                || m_hov != win.min_hover || m_prs != win.min_pressed
            {
                win.close_hover   = c_hov;
                win.close_pressed = c_prs;
                win.min_hover     = m_hov;
                win.min_pressed   = m_prs;
                dirty = true;
            }
        }

        // ---- 4. Release on close/min button → action ----
        if released_just_now {
            let prev_top = wm.topmost_at(prev.x, prev.y);
            let now_top  = wm.topmost_at(now.x,  now.y);
            if let (Some(p), Some(n)) = (prev_top, now_top) {
                if p == n {
                    // Close: press+release both inside close button.
                    if wm.windows[p].close_contains(prev.x, prev.y)
                        && wm.windows[n].close_contains(now.x,  now.y)
                    {
                        // If this is an app window, signal CloseRequested
                        // so the app's poll_event sees it and can exit
                        // cleanly.  Chrome goes away immediately either way.
                        if let Some(app_id) = wm.windows[p].app_id {
                            app_wm::mark_closed_by_user(app_id);
                        }
                        wm.windows.remove(p);
                        dirty = true;
                    }
                    // Minimize: press+release both inside min button.
                    else if wm.windows[p].min_contains(prev.x, prev.y)
                        && wm.windows[n].min_contains(now.x,  now.y)
                    {
                        wm.toggle_task(p); // p is focused & visible → minimizes
                        dirty = true;
                    }
                }
            }
        }

        // ---- 5. Start button hover / press ----
        let in_taskbar = now.y >= (sh as i32) - TASKBAR_H as i32;
        let start_hov  = in_taskbar && start_contains(sh, now.x, now.y);
        let start_prs  = start_hov && now.left;
        if start_hov != wm.start_hover || start_prs != wm.start_pressed {
            wm.start_hover   = start_hov;
            wm.start_pressed = start_prs;
            dirty = true;
        }

        // ---- 6. Start click → toggle Start menu ----
        if released_just_now
            && start_contains(sh, prev.x, prev.y)
            && start_contains(sh, now.x, now.y)
        {
            if wm.menu_open {
                wm.close_menu();
            } else {
                wm.menu_open = true;
            }
            dirty = true;
        }

        // ---- 7. Task-button hover / press ----
        let t_hov = if in_taskbar { task_btn_at(&wm, sh, now.x, now.y) } else { None };
        let t_prs = if now.left { t_hov } else { None };
        if t_hov != wm.task_hover || t_prs != wm.task_pressed {
            wm.task_hover   = t_hov;
            wm.task_pressed = t_prs;
            dirty = true;
        }

        // ---- 8. Task-button click → minimize/restore/focus toggle ----
        if released_just_now {
            let prev_t = task_btn_at(&wm, sh, prev.x, prev.y);
            let now_t  = task_btn_at(&wm, sh, now.x,  now.y);
            if let (Some(p), Some(n)) = (prev_t, now_t) {
                if p == n {
                    wm.toggle_task(p);
                    wm.task_hover   = task_btn_at(&wm, sh, now.x, now.y);
                    wm.task_pressed = None;
                    dirty = true;
                }
            }
        }

        // ---- 9. Menu hover / press ----
        let m_hov = if wm.menu_open { menu_item_at(sh, now.x, now.y) } else { None };
        let m_prs = if now.left { m_hov } else { None };
        if m_hov != wm.menu_hover || m_prs != wm.menu_pressed {
            wm.menu_hover   = m_hov;
            wm.menu_pressed = m_prs;
            dirty = true;
        }

        // ---- 10. Menu row click → invoke action + close menu ----
        if released_just_now && wm.menu_open {
            let p_item = menu_item_at(sh, prev.x, prev.y);
            let n_item = menu_item_at(sh, now.x,  now.y);
            if let (Some(p), Some(n)) = (p_item, n_item) {
                if p == n {
                    wm.close_menu();
                    menu_invoke(p, &mut wm); // may not return (shutdown / reboot)
                    dirty = true;
                }
            }
        }

        // ---- 11. Context-menu hover / press ----
        let c_hov = if wm.ctx_open { ctx_item_at(&wm, sw, sh, now.x, now.y) } else { None };
        let c_prs = if now.left { c_hov } else { None };
        if c_hov != wm.ctx_hover || c_prs != wm.ctx_pressed {
            wm.ctx_hover   = c_hov;
            wm.ctx_pressed = c_prs;
            dirty = true;
        }

        // ---- 12. Context-menu row click → invoke + close ----
        if released_just_now && wm.ctx_open {
            let p_item = ctx_item_at(&wm, sw, sh, prev.x, prev.y);
            let n_item = ctx_item_at(&wm, sw, sh, now.x,  now.y);
            if let (Some(p), Some(n)) = (p_item, n_item) {
                if p == n {
                    wm.close_ctx();
                    if ctx_invoke(p, &mut wm) {
                        dirty = true;
                    }
                }
            }
        }

        if dirty {
            repaint_all(&wm);
        }
        prev       = now;
        prev_ticks = ticks;
    }
}
