//! Tiny line-oriented shell that reads from the keyboard input ring and
//! dispatches a handful of built-in commands.
//!
//! Dormant since Phase 7.3.3 — the desktop took over `kernel_main`'s tail.
//! Kept around for a future "Terminal" window.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

use crate::{fs, framebuffer, input, mouse, print, println};

const PROMPT: &str = "marx> ";
const MAX_LINE: usize = 200;

/// Entry point — pass to `task::spawn(shell::task)`.
pub fn task() -> ! {
    println!();
    println!("MarX-OS shell. Type 'help' for the command list.");
    print_prompt();

    let mut line = String::with_capacity(MAX_LINE);
    loop {
        let b = input::pop_blocking();
        match b {
            b'\n' | b'\r' => {
                println!();
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    execute(trimmed);
                }
                line.clear();
                print_prompt();
            }
            0x08 | 0x7F => {
                // Backspace / Delete — erase one character on screen.
                if !line.is_empty() {
                    line.pop();
                    print!("\u{8}");
                }
            }
            0x20..=0x7E => {
                // Printable ASCII — append + echo.
                if line.len() < MAX_LINE {
                    line.push(b as char);
                    print!("{}", b as char);
                }
            }
            _ => {} // ignore other control bytes
        }
    }
}

fn print_prompt() {
    print!("{}", PROMPT);
}

fn execute(line: &str) {
    let mut parts = line.split_whitespace();
    let cmd = match parts.next() { Some(c) => c, None => return };
    let args: Vec<&str> = parts.collect();

    match cmd {
        "help"          => cmd_help(),
        "about"         => cmd_about(),
        "ls"            => cmd_ls(),
        "cat"           => cmd_cat(&args),
        "echo"          => cmd_echo(&args),
        "clear" | "cls" => framebuffer::clear(),
        "mem"           => cmd_mem(),
        "meow"          => cmd_meow(),
        "mouse"         => cmd_mouse(),
        "panic"         => panic!("user requested panic from the shell"),
        "crash"         => cmd_crash(),
        _ => println!("unknown command: '{}' (try 'help')", cmd),
    }
}

// ---------------- command handlers ----------------

fn cmd_help() {
    println!("Built-in commands:");
    println!("  help            this list");
    println!("  about           about MarX-OS");
    println!("  ls              list files on the MARXARCH disk");
    println!("  cat <file>      print contents of <file>");
    println!("  echo <text>...  print the arguments");
    println!("  clear           clear the screen (alias: cls)");
    println!("  mem             show kernel heap parameters");
    println!("  meow            say hi to the kitty");
    println!("  mouse           show current mouse position + buttons");
    println!();
    println!("  panic           trigger a kernel panic   (halts forever)");
    println!("  crash           trigger a page fault     (halts forever)");
}

fn cmd_about() {
    println!("MarX-OS — a from-scratch x86_64 hobby OS in Rust.");
    println!("Phases done: serial, framebuffer, splash, GDT+IDT, keyboard,");
    println!("heap+paging, preemptive scheduler, ATA+MARXARCH FS, shell.");
}

fn cmd_ls() {
    match fs::list() {
        Ok(entries) => {
            if entries.is_empty() {
                println!("(disk is empty)");
                return;
            }
            for e in entries {
                println!("  {:>6} B   lba {:>3}   {}", e.size, e.lba, e.name);
            }
        }
        Err(e) => println!("ls: {:?}", e),
    }
}

fn cmd_cat(args: &[&str]) {
    if args.is_empty() {
        println!("cat: usage: cat <filename>");
        return;
    }
    let name = args[0];
    match fs::read(name) {
        Ok(bytes) => match core::str::from_utf8(&bytes) {
            Ok(text) => {
                // Print as-is; if the file ends without a newline, drop a
                // trailing one so the next prompt starts cleanly.
                print!("{}", text);
                if !text.ends_with('\n') { println!(); }
            }
            Err(_) => println!("cat: {} is not valid UTF-8 ({} bytes)", name, bytes.len()),
        },
        Err(e) => println!("cat: {:?}", e),
    }
}

fn cmd_echo(args: &[&str]) {
    println!("{}", args.join(" "));
}

fn cmd_mem() {
    println!("Heap base : {:#x}", crate::allocator::HEAP_START);
    println!("Heap size : {} bytes ({} KiB)",
             crate::allocator::HEAP_SIZE,
             crate::allocator::HEAP_SIZE / 1024);
}

fn cmd_mouse() {
    let s = mouse::state();
    println!("  position: ({}, {})", s.x, s.y);
    println!("  buttons : left={}  right={}  middle={}", s.left, s.right, s.middle);
    println!("  hint    : move the mouse in the QEMU window (click in once to grab,");
    println!("            Ctrl+Alt+G to release), then run 'mouse' again.");
}

fn cmd_meow() {
    println!("       /\\_/\\");
    println!("      ( -.- )    *purr*");
    println!("       > ~ <");
    println!("      /     \\");
}

fn cmd_crash() {
    println!("crash: dereferencing 0xDEADBEEF_DEAD0000 ...");
    unsafe {
        // Triggers a page fault — the IDT handler funnels into panic!()
        // which paints the KITTY BSOD.
        core::ptr::write_volatile(0xDEAD_BEEF_DEAD_0000 as *mut u64, 42);
    }
}
