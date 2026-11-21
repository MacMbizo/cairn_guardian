//! OS seam listing mounted volumes, so drive logic can be tested with temp directories.

use crate::error::Result;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A mounted volume as reported by the OS (or a fake in tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedVolume {
    pub mount_point: PathBuf,
    pub guid_path: Option<String>,
    pub serial: Option<String>,
    pub label: Option<String>,
    pub fs_type: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_removable: bool,
    pub is_readonly: bool,
    pub is_system: bool,
}

/// Lists currently mounted volumes.
pub trait VolumeProvider: Send + Sync {
    fn mounted(&self) -> Result<Vec<MountedVolume>>;
}

/// In-memory provider: tests (and previews) plug and unplug directories as drives.
#[derive(Debug, Default)]
pub struct FakeVolumeProvider {
    inner: Mutex<Vec<MountedVolume>>,
}

impl FakeVolumeProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plug(&self, v: MountedVolume) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|x| x.mount_point != v.mount_point);
        g.push(v);
    }

    pub fn unplug(&self, mount: &Path) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|x| x.mount_point != mount);
    }

    pub fn set(&self, vs: Vec<MountedVolume>) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = vs;
    }
}

impl VolumeProvider for FakeVolumeProvider {
    fn mounted(&self) -> Result<Vec<MountedVolume>> {
        Ok(self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }
}

/// Real provider backed by Win32 drive enumeration.
#[cfg(windows)]
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsProvider;

#[cfg(windows)]
impl VolumeProvider for WindowsProvider {
    fn mounted(&self) -> Result<Vec<MountedVolume>> {
        use windows_sys::Win32::Storage::FileSystem::{
            GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
            GetVolumeInformationW, GetVolumeNameForVolumeMountPointW,
        };
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

        let system_drive = std::env::var("SystemDrive")
            .unwrap_or_else(|_| "C:".into())
            .to_ascii_uppercase();
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{letter}:\\");
            let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
            if drive_type != DRIVE_REMOVABLE && drive_type != DRIVE_FIXED {
                continue; // network, optical and RAM disks are out of scope
            }
            let mut label = [0u16; 261];
            let mut fs_name = [0u16; 64];
            let mut serial: u32 = 0;
            let mut max_comp: u32 = 0;
            let mut flags: u32 = 0;
            let ok = unsafe {
                GetVolumeInformationW(
                    wide.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    &mut serial,
                    &mut max_comp,
                    &mut flags,
                    fs_name.as_mut_ptr(),
                    fs_name.len() as u32,
                )
            };
            if ok == 0 {
                continue; // no media (empty card reader) or unreadable
            }
            let utf16 = |buf: &[u16]| {
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                String::from_utf16_lossy(&buf[..len])
            };
            let mut guid = [0u16; 64];
            let guid_ok = unsafe {
                GetVolumeNameForVolumeMountPointW(wide.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
            };
            let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
            unsafe {
                GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free);
            }
            let label = utf16(&label);
            out.push(MountedVolume {
                mount_point: PathBuf::from(&root),
                guid_path: (guid_ok != 0).then(|| utf16(&guid)),
                serial: (serial != 0).then(|| format!("{serial:08x}")),
                label: (!label.is_empty()).then_some(label),
                fs_type: utf16(&fs_name).to_lowercase(),
                total_bytes: total,
                free_bytes: free,
                is_removable: drive_type == DRIVE_REMOVABLE,
                is_readonly: flags & FILE_READ_ONLY_VOLUME != 0,
                is_system: format!("{letter}:") == system_drive,
            });
        }
        Ok(out)
    }
}
