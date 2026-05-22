use std::process::Command;

const BIOS_IMAGE: &str = env!("BIOS_IMAGE");
const UEFI_IMAGE: &str = env!("UEFI_IMAGE");
const FS_IMAGE: &str = env!("FS_IMAGE");

fn main() {
    let mut args = std::env::args().skip(1);
    let mut mode = "bios";
    let mut headless = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--uefi" => mode = "uefi",
            "--bios" => mode = "bios",
            "--headless" => headless = true,
            other => {
                eprintln!("unknown arg: {}", other);
                std::process::exit(2);
            }
        }
    }

    let image = if mode == "uefi" { UEFI_IMAGE } else { BIOS_IMAGE };
    println!("MarX-OS runner");
    println!("  mode  : {}", mode);
    println!("  boot  : {}", image);
    println!("  fs    : {}", FS_IMAGE);

    let mut cmd = Command::new("qemu-system-x86_64");
    if mode == "uefi" {
        cmd.arg("-bios").arg("OVMF.fd");
    }
    // Primary IDE master: the bootable disk
    cmd.arg("-drive").arg(format!("format=raw,file={},if=ide,index=0", image));
    // Primary IDE slave: our MARXARCH filesystem image, mounted by the kernel
    cmd.arg("-drive").arg(format!("format=raw,file={},if=ide,index=1", FS_IMAGE));
    // Serial → file so it survives QEMU exit and can be read after the run.
    // run-fast.bat tails this at the end so the user sees app logs without
    // any second window.  The path is fixed (and ASCII-only) to dodge
    // PowerShell + QEMU + Cyrillic-path quirks.
    let serial_log = "C:/marx-build/serial.log";
    // Best-effort: clear stale contents so each run starts fresh.
    let _ = std::fs::write(serial_log, b"");
    cmd.arg("-serial").arg(format!("file:{}", serial_log));
    println!("serial: {}", serial_log);
    // NOTE: `-no-reboot` was useful early on (so a panic-triple-fault loop
    // would terminate QEMU instead of restarting). Now our panic handler
    // halts forever with `hlt`, and the only reboot path is the explicit
    // 8042 reset from the Start menu's Restart item — so we want QEMU to
    // honour it and actually reboot the machine.
    if headless {
        cmd.arg("-display").arg("none");
    }

    println!("launching: {:?}", cmd);
    let status = cmd.status().expect("failed to launch qemu-system-x86_64");
    std::process::exit(status.code().unwrap_or(1));
}
