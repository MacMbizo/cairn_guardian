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

use aegis_core::db::SqliteStore;
use std::path::Path;

/// Writes `content` to `root/rel`, creating parent folders.
pub fn put(root: &Path, rel: &str, content: &[u8]) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    p
}

/// Canonical display form of a path, as stored in `volumes.mount_point` / `files.path`.
pub fn canon(path: &Path) -> String {
    aegis_core::db::files::normalize_path(&std::fs::canonicalize(path).unwrap(), false).0
}

/// Volume id of the registered drive whose mount is `mount`.
pub fn volume_id_at(store: &SqliteStore, mount: &Path) -> String {
    let want = canon(mount);
    store
        .list_volumes()
        .unwrap()
        .into_iter()
        .find(|v| v.mount_point.eq_ignore_ascii_case(&want) && v.status == "online")
        .unwrap_or_else(|| panic!("no online volume at {want}"))
        .id
}

pub fn status_of(store: &SqliteStore, volume_id: &str) -> String {
    store
        .conn()
        .query_row("SELECT status FROM volumes WHERE id = ?1", [volume_id], |r| r.get(0))
        .unwrap()
}

pub fn live_file_paths(store: &SqliteStore, volume_id: &str) -> Vec<String> {
    let mut stmt = store
        .conn()
        .prepare("SELECT path FROM files WHERE volume_id = ?1 AND deleted_at IS NULL ORDER BY path")
        .unwrap();
    stmt.query_map([volume_id], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}
