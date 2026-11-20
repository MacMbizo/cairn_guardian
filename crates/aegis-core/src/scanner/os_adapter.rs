//! OS filesystem metadata & cloud-placeholder adapter (`A2 §5`, `B1/B2 §5`).
//!
//! Never reads file payloads when collecting identity, link counts, or cloud attributes.

use crate::error::{AegisError, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata-only snapshot of a filesystem entry (`0` payload bytes read).
#[derive(Debug, Clone)]
pub struct OsFileStat {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub file_index: Option<Vec<u8>>,
    pub nlink: i64,
    pub size: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub cloud_state: String,
}

/// Volume metadata used to seed `B4.volumes` (`has_stable_inode`, `case_sensitive`, free space).
#[derive(Debug, Clone)]
pub struct OsVolumeMeta {
    pub id: String,
    pub mount_point: String,
    pub fs_type: String,
    pub is_removable: bool,
    pub is_readonly: bool,
    pub has_stable_inode: bool,
    pub case_sensitive: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

fn system_time_to_ms(st: SystemTime) -> i64 {
    st.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
        ERROR_SHARING_VIOLATION, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
        FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo,
        GetDiskFreeSpaceExW, GetDriveTypeW, GetFileAttributesW, GetFileInformationByHandle,
        GetFileInformationByHandleEx, GetVolumeInformationW, GetVolumePathNameW,
        INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
    };

    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_REMOTE: u32 = 4;

    fn to_wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn map_last_err(path: &Path, err: u32) -> AegisError {
        let p = path.display().to_string();
        match err {
            ERROR_ACCESS_DENIED => AegisError::PermissionDenied(p),
            ERROR_SHARING_VIOLATION => AegisError::LockedFile(p),
            _ => AegisError::Io(std::io::Error::from_raw_os_error(err as i32)),
        }
    }

    pub fn inspect_path_metadata(path: &Path) -> Result<Option<OsFileStat>> {
        let wide = to_wide(path);
        // SAFETY: wide is null-terminated UTF-16 path.
        let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            let err = unsafe { GetLastError() };
            if err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND {
                return Ok(None);
            }
            return Err(map_last_err(path, err));
        }

        let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
        let has_reparse_point = (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
        let is_placeholder = (attrs
            & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_OFFLINE))
            != 0;
        let cloud_state = if is_placeholder {
            "placeholder_remote".to_string()
        } else {
            "local".to_string()
        };

        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(AegisError::PermissionDenied(path.display().to_string()));
            }
            Err(e) => return Err(AegisError::Io(e)),
        };

        // Only name-surrogate reparse points (symlinks, junctions) are links. Cloud Files
        // (OneDrive, Dropbox, iCloud) also set FILE_ATTRIBUTE_REPARSE_POINT on ordinary folders
        // and files; std's `is_symlink` checks the reparse tag and correctly excludes them.
        let is_symlink = has_reparse_point && meta.file_type().is_symlink();

        let size = if is_dir { 0 } else { meta.file_size() as i64 };
        let mtime = meta.modified().map(system_time_to_ms).unwrap_or(0);
        let ctime = meta.created().map(system_time_to_ms).unwrap_or(mtime);

        // Zero-access handle strictly for metadata/inode lookup (never hydrates or reads payload).
        let (file_index, nlink) = if is_placeholder {
            (None, 1)
        } else {
            query_handle_identity(&wide, path)?
        };

        Ok(Some(OsFileStat {
            is_dir,
            is_symlink,
            file_index,
            nlink,
            size,
            mtime,
            ctime,
            cloud_state,
        }))
    }

    fn query_handle_identity(wide: &[u16], path: &Path) -> Result<(Option<Vec<u8>>, i64)> {
        // SAFETY: dwDesiredAccess = 0 queries attributes/IDs only without opening content stream.
        let handle: HANDLE = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let err = unsafe { GetLastError() };
            if err == ERROR_ACCESS_DENIED {
                return Err(AegisError::PermissionDenied(path.display().to_string()));
            }
            if err == ERROR_SHARING_VIOLATION {
                return Err(AegisError::LockedFile(path.display().to_string()));
            }
            return Ok((None, 1));
        }

        let mut bhfi = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        let got_bhfi =
            unsafe { GetFileInformationByHandle(handle, bhfi.as_mut_ptr()) } != 0;
        let (fallback_idx, nlink) = if got_bhfi {
            let info = unsafe { bhfi.assume_init() };
            let idx64 = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
            (Some(idx64.to_le_bytes().to_vec()), (info.nNumberOfLinks as i64).max(1))
        } else {
            (None, 1)
        };

        let mut id_info = MaybeUninit::<FILE_ID_INFO>::zeroed();
        let got_id_info = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                id_info.as_mut_ptr().cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } != 0;

        unsafe {
            CloseHandle(handle);
        }

        let file_index = if got_id_info {
            let info = unsafe { id_info.assume_init() };
            Some(info.FileId.Identifier.to_vec())
        } else {
            fallback_idx
        };

        Ok((file_index, nlink))
    }

    pub fn inspect_volume_for_path(path: &Path) -> Result<OsVolumeMeta> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let wide = to_wide(&canonical);
        let mut mount_buf = vec![0u16; 512];
        let ok_mount = unsafe {
            GetVolumePathNameW(wide.as_ptr(), mount_buf.as_mut_ptr(), mount_buf.len() as u32)
        };
        let mount_point = if ok_mount != 0 {
            let len = mount_buf.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&mount_buf[..len])
        } else {
            canonical
                .ancestors()
                .last()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "C:\\".to_string())
        };

        let mount_wide: Vec<u16> = mount_point.encode_utf16().chain(std::iter::once(0)).collect();
        let mut fs_name = [0u16; 64];
        let mut serial: u32 = 0;
        let mut flags: u32 = 0;
        let mut max_comp: u32 = 0;

        let ok_vol = unsafe {
            GetVolumeInformationW(
                mount_wide.as_ptr(),
                null_mut(),
                0,
                &mut serial,
                &mut max_comp,
                &mut flags,
                fs_name.as_mut_ptr(),
                fs_name.len() as u32,
            )
        };

        let fs_type = if ok_vol != 0 {
            let len = fs_name.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&fs_name[..len]).to_lowercase()
        } else {
            "ntfs".to_string()
        };

        let drive_type = unsafe { GetDriveTypeW(mount_wide.as_ptr()) };
        let is_removable = drive_type == DRIVE_REMOVABLE;
        let is_remote = drive_type == DRIVE_REMOTE;
        let is_readonly = (flags & FILE_READ_ONLY_VOLUME) != 0;
        // Win32 NTFS sets FILE_CASE_SENSITIVE_SEARCH to advertise POSIX capability,
        // while Win32 path resolution remains case-insensitive by default (`B4 v2.3.1`).
        let case_sensitive = false;
        let effective_fs = if is_remote && fs_type.is_empty() {
            "smb".to_string()
        } else {
            fs_type
        };
        let has_stable_inode = !is_remote && matches!(effective_fs.as_str(), "ntfs" | "refs");

        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        unsafe {
            GetDiskFreeSpaceExW(
                mount_wide.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut total_free,
            );
        }

        let clean_mount = mount_point
            .strip_prefix(r"\\?\")
            .unwrap_or(&mount_point)
            .to_string();
        let id = if serial != 0 {
            format!("win-{serial:08x}")
        } else {
            let h = blake3::hash(clean_mount.as_bytes());
            format!("vol-{}", &h.to_hex()[..16])
        };

        Ok(OsVolumeMeta {
            id,
            mount_point: clean_mount,
            fs_type: effective_fs,
            is_removable,
            is_readonly,
            has_stable_inode,
            case_sensitive,
            total_bytes,
            free_bytes: free_avail,
        })
    }

    pub fn disk_space_for_mount(mount_point: &str) -> (u64, u64) {
        let wide: Vec<u16> = mount_point.encode_utf16().chain(std::iter::once(0)).collect();
        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut total_free,
            )
        };
        if ok != 0 {
            (total_bytes, free_avail)
        } else {
            (0, 0)
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub fn inspect_path_metadata(path: &Path) -> Result<Option<OsFileStat>> {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(AegisError::PermissionDenied(path.display().to_string()));
            }
            Err(e) => return Err(AegisError::Io(e)),
        };

        let is_dir = meta.is_dir();
        let is_symlink = meta.file_type().is_symlink();
        let file_index = Some(meta.ino().to_le_bytes().to_vec());
        let nlink = (meta.nlink() as i64).max(1);
        let size = if is_dir { 0 } else { meta.size() as i64 };
        let mtime = meta.modified().map(system_time_to_ms).unwrap_or(0);
        let ctime = meta.ctime() * 1000;

        Ok(Some(OsFileStat {
            is_dir,
            is_symlink,
            file_index,
            nlink,
            size,
            mtime,
            ctime,
            cloud_state: "local".to_string(),
        }))
    }

    /// Mount point = highest ancestor still on the same device as `path`.
    fn mount_point_for(path: &Path, dev: u64) -> std::path::PathBuf {
        let mut mount = path.to_path_buf();
        for anc in path.ancestors().skip(1) {
            match std::fs::metadata(anc) {
                Ok(m) if m.dev() == dev => mount = anc.to_path_buf(),
                _ => break,
            }
        }
        mount
    }

    fn sysinfo_disk_for(mount: &Path) -> Option<(String, bool, bool, u64, u64)> {
        let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
        for disk in &disks {
            if disk.mount_point() == mount {
                return Some((
                    disk.file_system().to_string_lossy().to_lowercase(),
                    disk.is_removable(),
                    disk.is_read_only(),
                    disk.total_space(),
                    disk.available_space(),
                ));
            }
        }
        None
    }

    pub fn inspect_volume_for_path(path: &Path) -> Result<OsVolumeMeta> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let meta = std::fs::metadata(&canonical).map_err(AegisError::Io)?;
        let dev = meta.dev();
        let mount = mount_point_for(&canonical, dev);
        let (fs_type, is_removable, is_readonly, total_bytes, free_bytes) = sysinfo_disk_for(&mount)
            .unwrap_or_else(|| {
                let fallback = if cfg!(target_os = "macos") { "apfs" } else { "ext4" };
                (fallback.to_string(), false, false, 0, 0)
            });
        // APFS/HFS+ default to case-insensitive; Linux filesystems are case-sensitive.
        let case_sensitive = !cfg!(target_os = "macos")
            && !matches!(fs_type.as_str(), "vfat" | "exfat" | "msdos" | "ntfs" | "ntfs3");
        let has_stable_inode = !matches!(
            fs_type.as_str(),
            "vfat" | "exfat" | "msdos" | "cifs" | "smbfs" | "smb3"
        );
        Ok(OsVolumeMeta {
            id: format!("unix-{dev}"),
            mount_point: mount.to_string_lossy().to_string(),
            fs_type,
            is_removable,
            is_readonly,
            has_stable_inode,
            case_sensitive,
            total_bytes,
            free_bytes,
        })
    }

    pub fn disk_space_for_mount(mount_point: &str) -> (u64, u64) {
        sysinfo_disk_for(Path::new(mount_point))
            .map(|(_, _, _, total, free)| (total, free))
            .unwrap_or((0, 0))
    }
}

/// Inspects metadata without reading file contents or hydrating cloud placeholders.
pub fn inspect_path_metadata(path: &Path) -> Result<Option<OsFileStat>> {
    platform::inspect_path_metadata(path)
}

/// Inspects volume characteristics (`has_stable_inode`, `case_sensitive`, free space) for a root path.
pub fn inspect_volume_for_path(path: &Path) -> Result<OsVolumeMeta> {
    platform::inspect_volume_for_path(path)
}

/// Returns `(total_bytes, free_bytes)` for a mounted volume.
pub fn disk_space_for_mount(mount_point: &str) -> (u64, u64) {
    platform::disk_space_for_mount(mount_point)
}
