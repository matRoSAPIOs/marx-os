//! MarX-OS Calculator — four-function (+, −, ×, ÷) with f64 arithmetic.
//!
//! Pure marx-sdk app: opens a 240×380 window, paints a display + 5×4
//! button grid (the bottom row is a wide "=" bar), handles MouseDown to
//! detect button hits.  No alloc, no std, no libc.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use marx_sdk::{api, Event, KernelServices, Rgb, WindowId};

// ---- geometry ----
const W: u32 = 240;
const H: u32 = 380;
const PAD: i32 = 8;
const DISPLAY_H: i32 = 60;
const GRID_TOP: i32 = PAD + DISPLAY_H + PAD;
const COLS: usize = 4;
const ROWS: usize = 5;

// ---- palette (Aero / Frutiger style) ----
const BG_TOP:        Rgb = Rgb::new(0xEC, 0xF2, 0xF8);
const BG_BOT:        Rgb = Rgb::new(0xCC, 0xD8, 0xE6);
const DISPLAY_TOP:   Rgb = Rgb::new(0x10, 0x18, 0x24);
const DISPLAY_BOT:   Rgb = Rgb::new(0x1E, 0x2A, 0x3A);
const DISPLAY_BORDER:Rgb = Rgb::new(0x4A, 0x6F, 0x9E);
const DISPLAY_FG:    Rgb = Rgb::new(0xCD, 0xE8, 0xFF);
const BTN_NUM_TOP:   Rgb = Rgb::new(0xFC, 0xFD, 0xFF);
const BTN_NUM_BOT:   Rgb = Rgb::new(0xCC, 0xD6, 0xE2);
const BTN_OP_TOP:    Rgb = Rgb::new(0xC4, 0xDC, 0xF0);
const BTN_OP_BOT:    Rgb = Rgb::new(0x6F, 0xA6, 0xD8);
const BTN_EQ_TOP:    Rgb = Rgb::new(0x84, 0xCC, 0x6A);
const BTN_EQ_BOT:    Rgb = Rgb::new(0x3A, 0x8E, 0x32);
const BTN_C_TOP:     Rgb = Rgb::new(0xF4, 0x8A, 0x6A);
const BTN_C_BOT:     Rgb = Rgb::new(0xC8, 0x3A, 0x3A);
const BTN_BORDER:    Rgb = Rgb::new(0x3A, 0x60, 0x8C);
const BTN_FG_DARK:   Rgb = Rgb::new(0x10, 0x1A, 0x28);
const BTN_FG_LIGHT:  Rgb = Rgb::new(0xFF, 0xFF, 0xFF);

// 5×4 grid. Bottom row is rendered as a single wide "=" bar; each cell
// still triggers `=` so hit testing stays uniform.
const LABELS: [[&str; COLS]; ROWS] = [
    ["7", "8", "9", "/"],
    ["4", "5", "6", "*"],
    ["1", "2", "3", "-"],
    ["0", ".", "C", "+"],
    ["=", "=", "=", "="],
];

// ---- calculator state (f64) ----
struct Calc {
    current: f64,
    accum:   f64,
    op:      u8,    // '+', '-', '*', '/', or 0
    fresh:   bool,  // next digit starts a new number
    error:   bool,
    /// True after "." was pressed and we're now appending fractional digits.
    decimal_mode: bool,
    /// Place value for the next decimal digit (10 → tenths, 100 → hundredths…).
    decimal_div: f64,
    pressed: Option<(usize, usize)>,
}

impl Calc {
    fn new() -> Self {
        Calc {
            current: 0.0, accum: 0.0, op: 0, fresh: true, error: false,
            decimal_mode: false, decimal_div: 10.0,
            pressed: None,
        }
    }

    fn input_digit(&mut self, d: u8) {
        if self.error { *self = Calc::new(); }
        let d = d as f64;
        if self.fresh {
            self.current = d;
            self.fresh = false;
            self.decimal_mode = false;
            self.decimal_div = 10.0;
        } else if self.decimal_mode {
            self.current += d / self.decimal_div;
            self.decimal_div *= 10.0;
        } else {
            self.current = self.current * 10.0 + d;
        }
    }

    fn input_dot(&mut self) {
        if self.error { *self = Calc::new(); }
        if self.fresh {
            self.current = 0.0;
            self.fresh = false;
        }
        self.decimal_mode = true;
        self.decimal_div = 10.0;
    }

    fn apply_pending(&mut self) {
        let r = match self.op {
            0    => Some(self.current),
            b'+' => Some(self.accum + self.current),
            b'-' => Some(self.accum - self.current),
            b'*' => Some(self.accum * self.current),
            b'/' => if self.current == 0.0 { None } else { Some(self.accum / self.current) },
            _    => None,
        };
        match r {
            Some(v) => { self.accum = v; self.current = v; }
            None    => { self.error = true; self.current = 0.0; self.accum = 0.0; }
        }
    }

    fn input_op(&mut self, op: u8) {
        if self.error { return; }
        self.apply_pending();
        self.op = op;
        self.fresh = true;
        self.decimal_mode = false;
    }

    fn input_equals(&mut self) {
        if self.error { return; }
        self.apply_pending();
        self.op = 0;
        self.fresh = true;
        self.decimal_mode = false;
    }

    fn clear(&mut self) { *self = Calc::new(); }
}

// ---- main ----

#[no_mangle]
pub unsafe extern "C" fn _start(svc: *const KernelServices) -> ! {
    api::init(svc);
    api::debug_log("[calculator] starting\n");

    let win = api::open_window("Calculator", W, H);
    let mut calc = Calc::new();
    paint(win, &calc);
    api::present(win);

    loop {
        match api::poll_event(win) {
            Event::CloseRequested => {
                api::close_window(win);
                halt();
            }
            Event::MouseDown { x, y } => {
                if let Some((r, c)) = hit_test(x, y) {
                    calc.pressed = Some((r, c));
                    paint(win, &calc);
                    api::present(win);
                }
            }
            Event::MouseUp { x, y } => {
                if let (Some((pr, pc)), Some((r, c))) = (calc.pressed, hit_test(x, y)) {
                    if pr == r && pc == c {
                        trigger(&mut calc, r, c);
                    }
                }
                calc.pressed = None;
                paint(win, &calc);
                api::present(win);
            }
            Event::KeyDown { ch } => {
                match ch {
                    b'0'..=b'9' => calc.input_digit(ch - b'0'),
                    b'+' | b'-' | b'*' | b'/' => calc.input_op(ch),
                    b'.' | b',' => calc.input_dot(),
                    b'=' | b'\n' | b'\r' => calc.input_equals(),
                    b'c' | b'C' | 0x08 | 0x1B => calc.clear(),
                    _ => continue,
                }
                paint(win, &calc);
                api::present(win);
            }
            _ => {}
        }
    }
}

fn hit_test(x: i32, y: i32) -> Option<(usize, usize)> {
    if y < GRID_TOP { return None; }
    let grid_w = W as i32 - 2 * PAD;
    let grid_h = H as i32 - GRID_TOP - PAD;
    let cell_w = grid_w / COLS as i32;
    let cell_h = grid_h / ROWS as i32;
    let lx = x - PAD;
    let ly = y - GRID_TOP;
    if lx < 0 || ly < 0 { return None; }
    let c = (lx / cell_w) as usize;
    let r = (ly / cell_h) as usize;
    if r >= ROWS || c >= COLS { return None; }
    Some((r, c))
}

fn trigger(calc: &mut Calc, r: usize, c: usize) {
    let lbl = LABELS[r][c];
    match lbl {
        "C" => calc.clear(),
        "=" => calc.input_equals(),
        "." => calc.input_dot(),
        "+" | "-" | "*" | "/" => calc.input_op(lbl.as_bytes()[0]),
        d   => calc.input_digit(d.as_bytes()[0] - b'0'),
    }
}

// ---- painting ----

fn paint(id: WindowId, calc: &Calc) {
    fill_v_gradient(id, 0, 0, W, H, BG_TOP, BG_BOT);

    // Display.
    let dw = W as i32 - 2 * PAD;
    fill_v_gradient(id, PAD, PAD, dw as u32, DISPLAY_H as u32, DISPLAY_TOP, DISPLAY_BOT);
    stroke_rect(id, PAD, PAD, dw as u32, DISPLAY_H as u32, DISPLAY_BORDER);

    let mut buf = [0u8; 32];
    let txt = if calc.error { "Error" } else { format_f64(calc.current, &mut buf) };
    let txt_sz = 28.0_f32;
    let tw = api::text_width(txt, txt_sz) as i32;
    let tx = PAD + dw - tw - 12;
    let ty = PAD + (DISPLAY_H + 20) / 2;
    api::draw_text(id, tx, ty, txt, txt_sz, DISPLAY_FG);

    // Button grid.
    let grid_w = W as i32 - 2 * PAD;
    let grid_h = H as i32 - GRID_TOP - PAD;
    let cell_w = grid_w / COLS as i32;
    let cell_h = grid_h / ROWS as i32;
    let gap = 3_i32;

    for r in 0..ROWS {
        // Bottom "=" bar spans all four columns.
        if r == ROWS - 1 && LABELS[r].iter().all(|&l| l == "=") {
            let bx = PAD + gap;
            let by = GRID_TOP + r as i32 * cell_h + gap;
            let bw = (grid_w - 2 * gap) as u32;
            let bh = (cell_h - 2 * gap) as u32;
            let pressed = matches!(calc.pressed, Some((pr, _)) if pr == r);
            paint_button(id, bx, by, bw, bh, "=", pressed);
            continue;
        }
        for c in 0..COLS {
            let bx = PAD + c as i32 * cell_w + gap;
            let by = GRID_TOP + r as i32 * cell_h + gap;
            let bw = (cell_w - 2 * gap) as u32;
            let bh = (cell_h - 2 * gap) as u32;
            let pressed = calc.pressed == Some((r, c));
            paint_button(id, bx, by, bw, bh, LABELS[r][c], pressed);
        }
    }
}

fn paint_button(id: WindowId, x: i32, y: i32, w: u32, h: u32, label: &str, pressed: bool) {
    let (mut top, mut bot, fg) = match label {
        "="                     => (BTN_EQ_TOP,  BTN_EQ_BOT,  BTN_FG_LIGHT),
        "C"                     => (BTN_C_TOP,   BTN_C_BOT,   BTN_FG_LIGHT),
        "+"|"-"|"*"|"/"         => (BTN_OP_TOP,  BTN_OP_BOT,  BTN_FG_DARK),
        _                       => (BTN_NUM_TOP, BTN_NUM_BOT, BTN_FG_DARK),
    };
    if pressed {
        top = darken(top, 24);
        bot = darken(bot, 24);
    }
    fill_v_gradient(id, x, y, w, h, top, bot);
    stroke_rect(id, x, y, w, h, BTN_BORDER);
    api::draw_rect(id, x + 1, y + 1, w - 2, 1, lighten(top, 40));

    let sz = 18.0_f32;
    let lw = api::text_width(label, sz) as i32;
    let lx = x + (w as i32 - lw) / 2;
    let ly = y + (h as i32 + 12) / 2;
    api::draw_text(id, lx, ly, label, sz, fg);
}

// ---- helpers ----

fn fill_v_gradient(id: WindowId, x: i32, y: i32, w: u32, h: u32, top: Rgb, bot: Rgb) {
    if h == 0 { return; }
    for row in 0..h {
        let t   = row as u32;
        let inv = h - t;
        let r = (top.r as u32 * inv + bot.r as u32 * t) / h;
        let g = (top.g as u32 * inv + bot.g as u32 * t) / h;
        let b = (top.b as u32 * inv + bot.b as u32 * t) / h;
        api::draw_rect(id, x, y + row as i32, w, 1,
                       Rgb::new(r as u8, g as u8, b as u8));
    }
}

fn stroke_rect(id: WindowId, x: i32, y: i32, w: u32, h: u32, color: Rgb) {
    api::draw_rect(id, x, y, w, 1, color);
    api::draw_rect(id, x, y + h as i32 - 1, w, 1, color);
    api::draw_rect(id, x, y, 1, h, color);
    api::draw_rect(id, x + w as i32 - 1, y, 1, h, color);
}

fn lighten(c: Rgb, amt: u8) -> Rgb {
    Rgb::new(c.r.saturating_add(amt), c.g.saturating_add(amt), c.b.saturating_add(amt))
}
fn darken(c: Rgb, amt: u8) -> Rgb {
    Rgb::new(c.r.saturating_sub(amt), c.g.saturating_sub(amt), c.b.saturating_sub(amt))
}

/// Format an f64 as decimal text into `buf`, return &str.
/// Truncates / rounds to 6 fractional digits; strips trailing zeros.
fn format_f64(n: f64, buf: &mut [u8]) -> &str {
    if n != n { return "NaN"; }
    if n.is_infinite() { return if n > 0.0 { "Inf" } else { "-Inf" }; }

    let neg = n < 0.0;
    let v = if neg { -n } else { n };

    // Truncate integer part; round fractional to 6 places.
    let int_part = v as u64;
    let frac     = v - int_part as f64;
    let mut frac_scaled = (frac * 1_000_000.0 + 0.5) as u64;
    let mut int_part    = int_part;
    if frac_scaled >= 1_000_000 {
        int_part += 1;
        frac_scaled -= 1_000_000;
    }

    let mut out = 0usize;
    if neg { buf[out] = b'-'; out += 1; }

    // Integer part — write reverse then flip.
    if int_part == 0 {
        buf[out] = b'0'; out += 1;
    } else {
        let mut tmp = [0u8; 20];
        let mut i = 0;
        let mut iv = int_part;
        while iv > 0 {
            tmp[i] = b'0' + (iv % 10) as u8;
            i += 1;
            iv /= 10;
        }
        for k in (0..i).rev() {
            buf[out] = tmp[k]; out += 1;
        }
    }

    // Fractional part — 6 digits, strip trailing zeros.
    if frac_scaled > 0 {
        buf[out] = b'.'; out += 1;
        let mut frac_buf = [0u8; 6];
        let mut fv = frac_scaled;
        for i in (0..6).rev() {
            frac_buf[i] = b'0' + (fv % 10) as u8;
            fv /= 10;
        }
        let mut flen = 6;
        while flen > 0 && frac_buf[flen - 1] == b'0' { flen -= 1; }
        for i in 0..flen {
            buf[out] = frac_buf[i]; out += 1;
        }
    }

    unsafe { core::str::from_utf8_unchecked(&buf[..out]) }
}

fn halt() -> ! {
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    api::debug_log("[calculator] PANIC\n");
    halt()
}
