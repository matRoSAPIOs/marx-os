//! MARXARCH — a read-only single-sector-header archive on a raw block device.
//!
//! On-disk layout (must stay in sync with `runner/build.rs::build_fs_image`):
//!
//!   Sector 0 (512 B):
//!     [  0.. 8] "MARXARCH"          (magic)
//!     [  8..12] version u32 LE       (currently 1)
//!     [ 12..16] file_count u32 LE    (≤ 15)
//!     [ 16..496] up to 15 entries of 32 bytes:
//!         [ 0..24] name, NUL-padded UTF-8
//!         [24..28] lba_start u32 LE
//!         [28..32] size_bytes u32 LE
//!     [496..512] reserved (zeros)
//!
//!   Sector 1+: file payloads, each padded to a 512-byte boundary.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Once;

use crate::ata::{self, AtaError, Drive};

const SECTOR: usize = 512;
const MAGIC: &[u8; 8] = b"MARXARCH";
const HEADER_SIZE: usize = 16;
const ENTRY_SIZE: usize = 32;
const NAME_LEN: usize = 24;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub lba:  u32,
    pub size: u32,
}

#[derive(Debug)]
#[allow(dead_code)] // variants are read indirectly via the Debug impl
pub enum FsError {
    Ata(AtaError),
    BadMagic,
    NotInit,
    NotFound,
}

struct Mount {
    drive:   Drive,
    entries: Vec<FileEntry>,
}

static MOUNT: Once<Mount> = Once::new();

/// Read sector 0 from `drive`, validate the magic, and cache the file table.
/// Safe to call once; subsequent calls return `Ok(())` without re-reading.
pub fn init(drive: Drive) -> Result<(), FsError> {
    if MOUNT.get().is_some() {
        return Ok(());
    }

    let mut sector0 = [0u8; SECTOR];
    ata::read_sector(drive, 0, &mut sector0).map_err(FsError::Ata)?;

    if &sector0[0..8] != MAGIC {
        return Err(FsError::BadMagic);
    }

    let count = u32::from_le_bytes(sector0[12..16].try_into().unwrap()) as usize;
    let mut entries: Vec<FileEntry> = Vec::with_capacity(count);
    for i in 0..count {
        let off = HEADER_SIZE + i * ENTRY_SIZE;
        let name_bytes = &sector0[off..off + NAME_LEN];
        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        let name = core::str::from_utf8(&name_bytes[..name_end])
            .unwrap_or("")
            .to_string();
        let lba  = u32::from_le_bytes(sector0[off + NAME_LEN..off + NAME_LEN + 4].try_into().unwrap());
        let size = u32::from_le_bytes(sector0[off + NAME_LEN + 4..off + NAME_LEN + 8].try_into().unwrap());
        entries.push(FileEntry { name, lba, size });
    }

    MOUNT.call_once(|| Mount { drive, entries });
    Ok(())
}

/// Borrow the file table.
pub fn list() -> Result<&'static [FileEntry], FsError> {
    MOUNT.get()
        .map(|m| m.entries.as_slice())
        .ok_or(FsError::NotInit)
}

/// Read a whole file into memory by name. Returns its exact byte length
/// (trailing pad bytes from the last sector are stripped).
pub fn read(name: &str) -> Result<Vec<u8>, FsError> {
    let mount = MOUNT.get().ok_or(FsError::NotInit)?;
    let entry = mount.entries.iter().find(|e| e.name == name).ok_or(FsError::NotFound)?;

    let sectors_needed = ((entry.size as usize) + SECTOR - 1) / SECTOR;
    let mut out: Vec<u8> = Vec::with_capacity(sectors_needed * SECTOR);

    for i in 0..sectors_needed {
        let mut sector = [0u8; SECTOR];
        ata::read_sector(mount.drive, entry.lba + i as u32, &mut sector).map_err(FsError::Ata)?;
        out.extend_from_slice(&sector);
    }

    out.truncate(entry.size as usize);
    Ok(out)
}
