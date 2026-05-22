use std::path::{Path, PathBuf};

fn main() {
    // OUT_DIR = <target>/<profile>/build/<crate>-<hash>/out
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let target_dir = out_dir
        .parent().unwrap()  // <crate>-<hash>
        .parent().unwrap()  // build
        .parent().unwrap()  // <profile>
        .parent().unwrap()  // target
        .to_path_buf();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());

    let kernel_path = target_dir
        .join("x86_64-unknown-none")
        .join(&profile)
        .join("marx-kernel");

    if !kernel_path.exists() {
        panic!(
            "kernel binary not found at {}.\n\
             Build the kernel first: \n  \
             cargo build -p marx-kernel --target x86_64-unknown-none\n\
             or run scripts\\build.ps1",
            kernel_path.display()
        );
    }

    let bios_image = out_dir.join("marx-bios.img");
    let uefi_image = out_dir.join("marx-uefi.img");

    bootloader::BiosBoot::new(&kernel_path)
        .create_disk_image(&bios_image)
        .expect("failed to create BIOS image");
    bootloader::UefiBoot::new(&kernel_path)
        .create_disk_image(&uefi_image)
        .expect("failed to create UEFI image");

    println!("cargo:rustc-env=BIOS_IMAGE={}", bios_image.display());
    println!("cargo:rustc-env=UEFI_IMAGE={}", uefi_image.display());
    println!("cargo:rerun-if-changed={}", kernel_path.display());

    // ----- MARXARCH filesystem image -----
    //
    // The image is built from two sources:
    //   1. Static text/data files in `runner/assets/disk/`
    //   2. Compiled app binaries from `target/x86_64-unknown-none/release-app/`
    //      named like `<app>.elf`
    //
    // App binaries are auto-discovered: any workspace member under `apps/`
    // that has already been compiled into the release-app target dir gets
    // included.  build.ps1 / build.bat compile the apps before the runner.
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let assets_dir   = manifest_dir.join("assets").join("disk");
    let workspace    = manifest_dir.parent().unwrap();
    let apps_dir     = workspace.join("apps");
    let app_target   = target_dir.join("x86_64-unknown-none").join("release-app");
    let fs_image     = out_dir.join("marx-fs.img");

    // Stage the inputs into a tmp dir so we don't pollute assets/disk/.
    let stage = out_dir.join("disk-stage");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).unwrap();

    // 1. Copy static asset files.
    if assets_dir.exists() {
        for entry in std::fs::read_dir(&assets_dir).unwrap().flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let to = stage.join(entry.file_name());
                std::fs::copy(entry.path(), &to).unwrap();
            }
        }
    }

    // 2. For every app under `apps/`, look for its compiled binary in the
    //    release-app target dir and copy it as `<name>.elf`.
    if apps_dir.exists() {
        for entry in std::fs::read_dir(&apps_dir).unwrap().flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let app_name = entry.file_name().into_string().expect("app name UTF-8");
            // On Windows cargo strips the .exe suffix for bare-metal targets.
            let bin = app_target.join(&app_name);
            if !bin.exists() {
                println!(
                    "cargo:warning=app '{}' not yet built (looked for {}); skipping. \
                     Run scripts/build.ps1 which builds apps before the runner.",
                    app_name, bin.display()
                );
                continue;
            }
            let dest_name = format!("{}.elf", app_name);
            let dest = stage.join(&dest_name);
            std::fs::copy(&bin, &dest).unwrap();
            println!("cargo:rerun-if-changed={}", bin.display());
            println!("cargo:warning=app  : {:>5} B  {}", std::fs::metadata(&dest).unwrap().len(), dest_name);
        }
    }

    build_fs_image(&stage, &fs_image);
    println!("cargo:rustc-env=FS_IMAGE={}", fs_image.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", assets_dir.display());
}

/// MARXARCH layout — see kernel/src/fs.rs for the on-disk spec.
///
/// Sector 0 (512 B):
///   [0..8]    "MARXARCH"
///   [8..12]   version (u32 LE, currently 1)
///   [12..16]  file count (u32 LE, max 15)
///   [16..496] up to 15 entries of 32 bytes:
///               [0..24]   name, NUL-padded UTF-8
///               [24..28]  LBA start (u32 LE)
///               [28..32]  size in bytes (u32 LE)
///   [496..512] reserved
///
/// Sectors 1+: file data, each file padded to a 512-byte boundary.
fn build_fs_image(src_dir: &Path, out_path: &Path) {
    const SECTOR: usize = 512;
    const MAX_FILES: usize = 15;
    const HEADER_SIZE: usize = 16;
    const ENTRY_SIZE: usize = 32;
    const NAME_LEN: usize = 24;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if src_dir.exists() {
        // Sort entries so the on-disk order is deterministic across rebuilds.
        let mut entries: Vec<_> = std::fs::read_dir(src_dir).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let name = entry.file_name().into_string()
                .expect("non-UTF-8 filename in assets/disk/");
            if name.len() > NAME_LEN {
                panic!("filename '{}' longer than {} bytes (MARXARCH limit)", name, NAME_LEN);
            }
            let data = std::fs::read(entry.path()).unwrap();
            println!("cargo:warning=fs: {:>5} B  {}", data.len(), name);
            files.push((name, data));
        }
    }

    if files.len() > MAX_FILES {
        panic!("too many files ({} > {} MARXARCH limit)", files.len(), MAX_FILES);
    }

    // Compute per-file LBAs. Data starts at sector 1.
    let mut current_lba: u32 = 1;
    let mut placed: Vec<(String, u32, u32)> = Vec::new();
    for (name, data) in &files {
        let sectors = ((data.len() + SECTOR - 1) / SECTOR).max(1) as u32;
        placed.push((name.clone(), current_lba, data.len() as u32));
        current_lba += sectors;
    }

    // Build sector 0.
    let mut sector0 = vec![0u8; SECTOR];
    sector0[0..8].copy_from_slice(b"MARXARCH");
    sector0[8..12].copy_from_slice(&1u32.to_le_bytes());
    sector0[12..16].copy_from_slice(&(placed.len() as u32).to_le_bytes());

    for (i, (name, lba, size)) in placed.iter().enumerate() {
        let off = HEADER_SIZE + i * ENTRY_SIZE;
        let name_bytes = name.as_bytes();
        sector0[off..off + name_bytes.len()].copy_from_slice(name_bytes);
        sector0[off + NAME_LEN..off + NAME_LEN + 4].copy_from_slice(&lba.to_le_bytes());
        sector0[off + NAME_LEN + 4..off + NAME_LEN + 8].copy_from_slice(&size.to_le_bytes());
    }

    // Build full image: sector 0, then each file's data padded to sector boundary.
    let mut image = sector0;
    for (_, data) in &files {
        image.extend_from_slice(data);
        let trailing = data.len() % SECTOR;
        if trailing != 0 {
            image.extend(std::iter::repeat(0u8).take(SECTOR - trailing));
        }
        // Empty files: ensure at least one sector is allocated so LBA arithmetic
        // matches what we wrote into the index.
        if data.is_empty() {
            image.extend(std::iter::repeat(0u8).take(SECTOR));
        }
    }

    std::fs::write(out_path, &image).expect("write marx-fs.img");
}
