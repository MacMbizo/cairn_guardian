//! Drive passport: a small marker file that travels with the drive (spec §7.1).

use crate::error::Result;
use crate::{new_id, now_ms};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Contents of `<root>\.aegis\volume.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passport {
    pub passport_id: String,
    pub created_at: i64,
    pub created_by: String,
    pub schema: u32,
}

fn passport_path(root: &Path) -> PathBuf {
    root.join(".aegis").join("volume.json")
}

/// Reads the passport at `root`. A missing or unreadable/malformed file yields `Ok(None)`.
pub fn read_passport(root: &Path) -> Result<Option<Passport>> {
    let path = passport_path(root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<Passport>(&text)
        .ok()
        .filter(|p| p.schema == 1 && !p.passport_id.is_empty()))
}

/// Writes a fresh passport to `root` (temp file + rename) and hides the `.aegis` folder.
pub fn write_passport(root: &Path) -> Result<Passport> {
    let dir = root.join(".aegis");
    std::fs::create_dir_all(&dir)?;
    let passport = Passport {
        passport_id: new_id(),
        created_at: now_ms(),
        created_by: format!("cairn {}", env!("CARGO_PKG_VERSION")),
        schema: 1,
    };
    let tmp = dir.join("volume.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&passport)?)?;
    let dest = passport_path(root);
    set_normal(&dest);
    std::fs::rename(&tmp, &dest)?;
    hide(&dir);
    hide(&dest);
    Ok(passport)
}

/// Replaces the passport at `root` with a new identity (used for cloned drives).
pub fn regenerate_passport(root: &Path) -> Result<Passport> {
    write_passport(root)
}

#[cfg(windows)]
fn set_attrs(path: &Path, attrs: u32) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetFileAttributesW(wide.as_ptr(), attrs);
    }
}

#[cfg(windows)]
fn hide(path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM,
    };
    let dir = if path.is_dir() { FILE_ATTRIBUTE_DIRECTORY } else { 0 };
    set_attrs(path, dir | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
}

/// Clears hidden/system so the file can be replaced by rename.
#[cfg(windows)]
fn set_normal(path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    if path.exists() {
        set_attrs(path, FILE_ATTRIBUTE_NORMAL);
    }
}

#[cfg(not(windows))]
fn hide(_path: &Path) {}

#[cfg(not(windows))]
fn set_normal(_path: &Path) {}
