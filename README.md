# MarX-OS

> A hobby x86_64 operating system written **from scratch** in Rust — booting on
> bare metal, with a **Frutiger Aero** glass desktop. Not a Linux distro. Its own
> kernel, scheduler, filesystem, window manager, and ELF app loader.

<!-- TODO: add a screenshot / GIF of the desktop here — it's the hook.
     A short clip of dragging windows + opening the calculator + the Start
     menu shows it off best. Once you have docs/screenshot.png, uncomment:
     ![MarX-OS desktop](docs/screenshot.png) -->

---

## About this project (read me — it's honest)

I always dreamed of building my own operating system — not a themed Linux
distro, a **real one from scratch**. The problem: I couldn't program. For years
I just watched videos and wished.

With the arrival of AI I finally started building. To be upfront: **most of the
code was written with heavy AI assistance.** My role was the architect /
director — deciding what to build and in what order, testing every step,
hunting bugs, choosing the look and feel, and learning how an OS actually works
along the way. I don't claim to be a systems programmer. This is a passion
project and a way to understand computers at the deepest level.

I'm sharing it because building it makes me genuinely happy, and maybe it'll
make someone else happy too. ✨

If you think that's not "real" — that's okay. Linux 0.01 was ~10,000 lines with
no GUI. Every developer stands on tools: compilers, libraries, Stack Overflow.
AI is just the newest one.

---

## What it actually does

This is **not** a toy that prints "Hello World" and halts. It is a working
operating system with:

- **Custom kernel** (bare-metal x86_64, `no_std` Rust) — boots via the
  `bootloader` crate
- **Memory management** — paging + frame allocator + a heap (`Box`, `Vec`,
  `String` all work)
- **Preemptive scheduler** — multiple tasks, context switching in hand-written
  assembly, timer-driven preemption
- **Drivers** — PIO ATA disk, PS/2 keyboard, PS/2 mouse, a framebuffer
  compositor with a backbuffer
- **Filesystem** — *MARXARCH*, a custom read-only archive format on a real IDE
  disk
- **TTF text rendering** — antialiased glyphs (Inter font) blended onto the
  framebuffer
- **Window manager** — multiple windows, z-order, click-to-raise focus,
  dragging, minimize, a taskbar with Start button + clock, a Start menu, and a
  right-click desktop context menu
- **Frutiger Aero theme** — translucent glass title bars, soft multi-layer
  drop shadows, glossy highlights, a real bubble wallpaper
- **ELF app loader** — loads static-PIE `.elf` executables from disk at
  runtime, applies relocations, and runs them as scheduler tasks
- **App SDK** (`marx-sdk`) — a stable ABI so apps can open windows, draw, read
  files, and receive mouse/keyboard events from the kernel
- **Real apps** — a `hello` window demo and a four-function `calculator`
  (mouse + keyboard), each compiled to its own `.elf`
- **Power** — ACPI shutdown and 8042 reboot from the Start menu

---

## Build & run

**Requirements:** Windows, [Rust nightly](https://rustup.rs/) (the toolchain is
pinned in `rust-toolchain.toml`), and [QEMU](https://www.qemu.org/) installed at
`C:\Program Files\qemu`.

The easiest way — just double-click:

- **`run.bat`** — debug build, builds everything and launches QEMU
- **`run-fast.bat`** — release build (smoother under QEMU's TCG)

Both build the kernel, the apps, and the runner, assemble the disk images, and
boot. After QEMU exits, the serial log is printed (also saved to
`C:\marx-build\serial.log`).

Manual build:

```powershell
# Kernel (bare-metal target)
cargo build -p marx-kernel --target x86_64-unknown-none --release

# Apps (size-optimised profile)
cargo build -p hello      --target x86_64-unknown-none --profile release-app
cargo build -p calculator --target x86_64-unknown-none --profile release-app

# Runner (host) — assembles disk images and launches QEMU
cargo run -p marx-runner --release
```

---

## Project layout

```
marx-os/
├── kernel/        # the OS kernel (no_std)
│   ├── src/
│   │   ├── main.rs        # boot sequence
│   │   ├── framebuffer.rs # compositor + drawing primitives
│   │   ├── desktop.rs     # window manager + taskbar + menus
│   │   ├── elf.rs         # ELF64 static-PIE loader
│   │   ├── app.rs         # app runtime + KernelServices vtable
│   │   ├── app_wm.rs      # per-app window state (pixels + events)
│   │   ├── task.rs        # preemptive scheduler + context switch
│   │   ├── ata.rs / fs.rs # disk driver + MARXARCH filesystem
│   │   ├── ttf.rs         # antialiased TTF text
│   │   └── ...            # gdt, idt, paging, mouse, cursor, ...
│   └── assets/            # logo, wallpaper, Inter font
├── sdk/           # marx-sdk — the app ABI shared by kernel + apps
├── apps/
│   ├── hello/     # demo window app
│   └── calculator/# four-function calculator
├── runner/        # host tool: builds disk images, launches QEMU
└── scripts/       # build.ps1
```

## How apps work

Apps are ordinary Rust crates that depend on `marx-sdk`, compile to
`x86_64-unknown-none` as static-PIE ELF binaries, and get bundled into the
MARXARCH disk image. At runtime the kernel:

1. Reads the `.elf` off the disk
2. Parses it, copies its segments into a fresh heap buffer, applies
   `R_X86_64_RELATIVE` relocations
3. Hands the app a `KernelServices` vtable (`window_open`, `draw_rect`,
   `draw_text`, `event_poll`, `fs_read`, …)
4. Spawns it as a scheduler task

This is the same idea as a real OS executable format — apps are genuinely
separate programs loaded from disk, not compiled into the kernel.

---

## Roadmap

- [x] **0–6** Toolchain, bootable kernel, VGA/serial, GDT/IDT/PIC, paging +
  heap, scheduler, ATA + filesystem
- [x] **7** Framebuffer, drawing primitives, TTF fonts, PS/2 mouse
- [x] **8** Window manager: multi-window, taskbar, Start menu, context menu,
  Aero glass theme, Frutiger Aero wallpaper
- [x] **9.1–9.5** App workspace, ELF loader, SDK, calculator, keyboard input
- [ ] **9.6** Terminal app
- [ ] **9.7** File manager
- [ ] **9.8** Visual polish (icons, animations, tooltips, toasts)
- [ ] **10** Networking (e1000 + smoltcp) → HTTP/HTTPS → a text web browser
- [ ] **11+** Audio, video, more apps…

---

## Credits & tech

Built on the shoulders of great open-source work:

- [Rust](https://www.rust-lang.org/) (nightly, `no_std`)
- [`bootloader`](https://github.com/rust-osdev/bootloader) — boot + framebuffer
- [`x86_64`](https://github.com/rust-osdev/x86_64) — CPU primitives
- [`ab_glyph`](https://github.com/alexheretic/ab-glyph) — TTF rasterisation
- [`image`](https://github.com/image-rs/image) — build-time asset decoding
- [Inter](https://rsms.me/inter/) font (SIL OFL 1.1)
- Architecture inspired by [Philipp Oppermann's *Writing an OS in
  Rust*](https://os.phil-opp.com/)

## License

[MIT](LICENSE) — do whatever you like, just keep the notice.
