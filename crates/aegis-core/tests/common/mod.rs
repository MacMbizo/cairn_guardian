//! Shared helpers for multi-drive integration tests.
#![allow(dead_code)]

use aegis_core::drives::MountedVolume;
use std::path::PathBuf;
use tempfile::TempDir;

/// Creates `tmp/<name>/` and describes it as a removable 1 TB drive.
pub fn fake_drive(tmp: &TempDir, name: &str, serial: &str) -> MountedVolume {
    let mount: PathBuf = tmp.path().join(name);
    std::fs::create_dir_all(&mount).unwrap();
    MountedVolume {
        mount_point: mount,
        guid_path: Some(format!(r"\\?\Volume{{{serial}-guid}}\")),
        serial: Some(serial.to_string()),
        label: Some(name.to_string()),
        fs_type: "ntfs".to_string(),
        total_bytes: 1_000_000_000_000,
        free_bytes: 900_000_000_000,
        is_removable: true,
        is_readonly: false,
        is_system: false,
    }
}
