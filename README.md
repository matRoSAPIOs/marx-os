# MarX-OS

A hobby x86_64 operating system written from scratch in Rust. Boots on bare
metal (under QEMU), Frutiger-Aero-styled glass desktop, runtime ELF app loader.

Not a Linux distro — its own kernel, scheduler, filesystem, window manager,
and app SDK.

## Features

- **Kernel** — bare-metal x86_64, `no_std` Rust, boots via the `bootloader`
  crate
- **Memory** — paging + frame allocator + heap (`Box`, `Vec`, `String`)
- **Scheduler** — preemptive, multi-task, context switch in hand-written
  assembly, timer-driven preemption
- **Drivers** — PIO ATA disk, PS/2 keyboard, PS/2 mouse, framebuffer
  compositor with backbuffer
- **Filesystem** — *MARXARCH*, a custom read-only archive on a real IDE disk
- **TTF text** — antialiased glyphs (Inter) alpha-blended onto the framebuffer
- **Window manager** — multi-window, z-order, click-to-raise focus, drag,
  minimize, taskbar with Start button + clock, Start menu, right-click context
  menu
- **Frutiger Aero theme** — translucent glass title bars, multi-layer soft
  drop shadows, glossy highlights, bubble wallpaper
- **ELF loader** — loads static-PIE ELF executables from disk at runtime,
  applies `R_X86_64_RELATIVE` relocations, runs them as scheduler tasks
- **App SDK** (`marx-sdk`) — stable ABI: window mgmt, draw, fs, mouse +
  keyboard events
- **Apps** — `hello` (window demo) and `calculator` (four-function, mouse +
  keyboard), each a separate `.elf` shipped in MARXARCH
- **Power** — ACPI shutdown + 8042 reboot from the Start menu

## Build & run

Requirements: Windows, [Rust nightly](https://rustup.rs/) (toolchain pinned in
`rust-toolchain.toml`), [QEMU](https://www.qemu.org/) at `C:\Program Files\qemu`.

One-click:

- `run.bat` — debug build, launches QEMU
- `run-fast.bat` — release build (smoother under QEMU TCG)

Both build kernel + apps + runner, assemble the disk images, boot. Serial log
is saved to `C:\marx-build\serial.log` and printed when QEMU exits.

Manual:

```powershell
cargo build -p marx-kernel --target x86_64-unknown-none --release
cargo build -p hello      --target x86_64-unknown-none --profile release-app
cargo build -p calculator --target x86_64-unknown-none --profile release-app
cargo run   -p marx-runner --release
```

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

Apps are Rust crates that depend on `marx-sdk`, compile to
`x86_64-unknown-none` as static-PIE ELF binaries, and get bundled into the
MARXARCH disk image at build time. At runtime the kernel:

1. Reads the `.elf` off the disk
2. Parses it, copies its segments into a fresh heap buffer, applies
   `R_X86_64_RELATIVE` relocations
3. Hands the app a `KernelServices` vtable (`window_open`, `draw_rect`,
   `draw_text`, `event_poll`, `fs_read`, …)
4. Spawns it as a scheduler task

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
- [ ] **10** Networking (e1000 + smoltcp) → HTTP/HTTPS → text web browser
- [ ] **11+** Audio, video, more apps

## Credits

- [Rust](https://www.rust-lang.org/) (nightly, `no_std`)
- [`bootloader`](https://github.com/rust-osdev/bootloader) — boot + framebuffer
- [`x86_64`](https://github.com/rust-osdev/x86_64) — CPU primitives
- [`ab_glyph`](https://github.com/alexheretic/ab-glyph) — TTF rasterisation
- [`image`](https://github.com/image-rs/image) — build-time asset decoding
- [Inter](https://rsms.me/inter/) font (SIL OFL 1.1)
- Architecture follows [Philipp Oppermann's *Writing an OS in
  Rust*](https://os.phil-opp.com/)

## License

[MIT](LICENSE)
