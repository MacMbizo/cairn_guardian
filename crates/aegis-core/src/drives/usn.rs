//! NTFS USN change-journal reader (spec §7.4). Windows only; every failure degrades to the
//! walk fallback, so nothing here is required for correctness.

use crate::error::Result;
#[cfg(not(windows))]
use crate::error::AegisError;
use std::path::{Path, PathBuf};

/// One change record, resolved to a path where the file or its parent still exists.
#[derive(Debug, Clone)]
pub struct UsnChange {
    pub file_ref: u64,
    pub parent_ref: u64,
    pub name: String,
    pub reason_flags: u32,
    pub is_dir: bool,
    /// The file's current path, resolved by its file reference; `None` once it no longer exists.
    /// (Non-administrator journal reads omit `name`, so the reference is the source of truth.)
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum UsnRead {
    Changes(Vec<UsnChange>),
    /// The journal was recreated, or `from_usn` has been overwritten: do a full walk.
    Reset,
}

pub const REASON_DATA: u32 = 0x1 | 0x2 | 0x4 | 0x10 | 0x20 | 0x40;
pub const REASON_FILE_CREATE: u32 = 0x100;
pub const REASON_FILE_DELETE: u32 = 0x200;
pub const REASON_RENAME_OLD_NAME: u32 = 0x1000;
pub const REASON_RENAME_NEW_NAME: u32 = 0x2000;

#[cfg(windows)]
mod win {
    use super::*;
    use crate::error::AegisError;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileIdType, GetFinalPathNameByHandleW, OPEN_EXISTING, OpenFileById,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F4;
    const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;
    const FSCTL_READ_UNPRIVILEGED_USN_JOURNAL: u32 = 0x0009_03AB;
    const GENERIC_READ: u32 = 0x8000_0000;
    const ERROR_JOURNAL_ENTRY_DELETED: u32 = 1181;
    const ERROR_JOURNAL_NOT_ACTIVE: u32 = 1179;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn letter(mount: &Path) -> Result<String> {
        let s = mount.to_string_lossy();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let b = s.as_bytes();
        if b.len() >= 2 && b[1] == b':' {
            Ok(s[..2].to_string())
        } else {
            Err(AegisError::UnsupportedFs(format!("not a drive-letter volume: {s}")))
        }
    }

    fn open_volume(mount: &Path) -> Result<Handle> {
        let path = wide(&format!(r"\\.\{}", letter(mount)?));
        for access in [GENERIC_READ, FILE_READ_ATTRIBUTES] {
            let h = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    null_mut(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if h != INVALID_HANDLE_VALUE {
                return Ok(Handle(h));
            }
        }
        Err(std::io::Error::last_os_error().into())
    }

    fn open_root(mount: &Path) -> Result<Handle> {
        let path = wide(&format!(r"{}\", letter(mount)?));
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Handle(h))
    }

    fn u64_at(b: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
    }
    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }
    fn u16_at(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
    }

    /// `(journal id, first usn, next usn)`, or `None` when no journal is active.
    fn query(vol: &Handle) -> Result<Option<(u64, i64, i64)>> {
        let mut out = [0u8; 80];
        let mut got: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                vol.0,
                FSCTL_QUERY_USN_JOURNAL,
                null_mut(),
                0,
                out.as_mut_ptr().cast(),
                out.len() as u32,
                &mut got,
                null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_JOURNAL_NOT_ACTIVE {
                return Ok(None);
            }
            return Err(std::io::Error::from_raw_os_error(err as i32).into());
        }
        Ok(Some((u64_at(&out, 0), u64_at(&out, 8) as i64, u64_at(&out, 16) as i64)))
    }

    /// A handle that answers journal FSCTLs: the volume itself when we may open it for reading
    /// (administrator), otherwise the root folder, which Windows 10 1709+ accepts for the
    /// unprivileged journal calls. Returns the handle and whether it is the privileged kind.
    /// `(journal id, first usn, next usn)`.
    type JournalInfo = (u64, i64, i64);

    fn journal_handle(mount: &Path) -> Result<(Handle, Option<JournalInfo>, bool)> {
        if let Ok(vol) = open_volume(mount)
            && let Ok(q) = query(&vol)
        {
            return Ok((vol, q, true));
        }
        let root = open_root(mount)?;
        let q = query(&root)?;
        Ok((root, q, false))
    }

    pub fn query_journal(mount: &Path) -> Result<Option<(String, i64)>> {
        let (_, q, _) = journal_handle(mount)?;
        Ok(q.map(|(id, _, next)| (format!("{id:016x}"), next)))
    }

    fn path_of_ref(root: &Handle, file_ref: u64) -> Option<PathBuf> {
        let desc = FILE_ID_DESCRIPTOR {
            dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
            Type: FileIdType,
            Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: file_ref as i64 },
        };
        let h = unsafe {
            OpenFileById(
                root.0,
                &desc,
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                FILE_FLAG_BACKUP_SEMANTICS,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let h = Handle(h);
        let mut buf = vec![0u16; 1024];
        let n = unsafe { GetFinalPathNameByHandleW(h.0, buf.as_mut_ptr(), buf.len() as u32, 0) };
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        let s = OsString::from_wide(&buf[..n as usize]);
        let s = s.to_string_lossy();
        Some(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)))
    }

    pub fn read_changes(mount: &Path, journal_id: &str, from_usn: i64) -> Result<UsnRead> {
        let (vol, q, privileged) = journal_handle(mount)?;
        let Some((jid, first, _next)) = q else {
            return Ok(UsnRead::Reset);
        };
        if format!("{jid:016x}") != journal_id || from_usn < first {
            return Ok(UsnRead::Reset);
        }
        let root = open_root(mount)?;
        let mut resolved: HashMap<u64, Option<PathBuf>> = HashMap::new();
        let mut out = Vec::new();
        let mut start = from_usn;
        let mut buf = vec![0u8; 64 * 1024];
        let mut fsctl = if privileged {
            FSCTL_READ_USN_JOURNAL
        } else {
            FSCTL_READ_UNPRIVILEGED_USN_JOURNAL
        };
        loop {
            // READ_USN_JOURNAL_DATA_V0: StartUsn, ReasonMask, ReturnOnlyOnClose, Timeout, BytesToWaitFor, UsnJournalID.
            let mut input = Vec::with_capacity(40);
            input.extend_from_slice(&start.to_le_bytes());
            input.extend_from_slice(&u32::MAX.to_le_bytes());
            input.extend_from_slice(&0u32.to_le_bytes());
            input.extend_from_slice(&0u64.to_le_bytes());
            input.extend_from_slice(&0u64.to_le_bytes());
            input.extend_from_slice(&jid.to_le_bytes());
            let mut got: u32 = 0;
            let ok = unsafe {
                DeviceIoControl(
                    vol.0,
                    fsctl,
                    input.as_ptr().cast(),
                    input.len() as u32,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    &mut got,
                    null_mut(),
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                if err == ERROR_JOURNAL_ENTRY_DELETED {
                    return Ok(UsnRead::Reset);
                }
                if fsctl == FSCTL_READ_USN_JOURNAL {
                    fsctl = FSCTL_READ_UNPRIVILEGED_USN_JOURNAL; // non-admin path (Windows 10 1709+)
                    continue;
                }
                return Err(std::io::Error::from_raw_os_error(err as i32).into());
            }
            let got = got as usize;
            if got <= 8 {
                break;
            }
            let next = u64_at(&buf, 0) as i64;
            let mut off = 8;
            while off + 60 <= got {
                let len = u32_at(&buf, off) as usize;
                if len == 0 || off + len > got {
                    break;
                }
                let rec = &buf[off..off + len];
                if u16_at(rec, 4) == 2 {
                    let file_ref = u64_at(rec, 8);
                    let parent_ref = u64_at(rec, 16);
                    let reason = u32_at(rec, 40);
                    let attrs = u32_at(rec, 52);
                    let name_len = u16_at(rec, 56) as usize;
                    let name_off = u16_at(rec, 58) as usize;
                    let units: Vec<u16> = rec[name_off..name_off + name_len]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c))
                        .collect();
                    let name = String::from_utf16_lossy(&units);
                    let path = resolved
                        .entry(file_ref)
                        .or_insert_with(|| path_of_ref(&root, file_ref))
                        .clone();
                    out.push(UsnChange {
                        file_ref,
                        parent_ref,
                        path,
                        name,
                        reason_flags: reason,
                        is_dir: attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
                    });
                }
                off += len;
            }
            if next <= start {
                break;
            }
            start = next;
        }
        Ok(UsnRead::Changes(out))
    }
}

#[cfg(windows)]
pub use win::{query_journal, read_changes};

#[cfg(not(windows))]
pub fn query_journal(_mount: &Path) -> Result<Option<(String, i64)>> {
    Ok(None)
}

#[cfg(not(windows))]
pub fn read_changes(_mount: &Path, _journal_id: &str, _from_usn: i64) -> Result<UsnRead> {
    Err(AegisError::UnsupportedFs("USN journal is Windows-only".into()))
}

/// True when `mount` is the root of a volume (e.g. `E:\`), the only place USN applies.
pub fn is_volume_root(mount: &Path) -> bool {
    let s = mount.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    b.len() <= 3 && b.len() >= 2 && b[1] == b':'
}

/// The `files.file_index` encodings an NTFS file reference can have been stored under:
/// the 128-bit `FILE_ID_INFO` form (reference, little-endian, zero-padded) and the 64-bit form.
pub fn file_index_forms(file_ref: u64) -> [Vec<u8>; 2] {
    let le = file_ref.to_le_bytes();
    let mut wide = le.to_vec();
    wide.extend_from_slice(&[0u8; 8]);
    [wide, le.to_vec()]
}
