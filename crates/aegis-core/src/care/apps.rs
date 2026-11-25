//! Installed Windows application inventory (`R3` strictly read-only registry access), safe
//! app-owned uninstaller lookup/parsing, and stale leftover folder detection.

use super::{
    CleanupCandidate, CleanupCategory, InstalledApp, LeftoverFolder, cleanup::dir_stats_bounded,
    display_path, path_contains_aegis_internal,
};
use crate::contracts::TargetKind;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const DAY_MS: i64 = 86_400_000;
const UNUSED_DAYS_THRESHOLD: i64 = 90;
const MAX_LEFTOVER_DEPTH: usize = 5;

const PROTECTED_VENDOR_DIRS: &[&str] = &[
    ".aegis",
    ".aegis-quarantine",
    ".cairn-quarantine",
    "aegis",
    "cairn",
    "cairn guardian",
    "adobe",
    "amd",
    "anthropic",
    "antigravity",
    "apple",
    "bravesoftware",
    "cargo",
    "cef",
    "chromium",
    "claude",
    "code",
    "comms",
    "com.aegis.guardian",
    "connecteddevicesplatform",
    "crashdumps",
    "cursor",
    "d3dscache",
    "diagnostics",
    "docker",
    "elevateddiagnostics",
    "fontcache",
    "git",
    "github",
    "google",
    "gtk",
    "iconcache",
    "intel",
    "jetbrains",
    "microsoft",
    "mozilla",
    "node",
    "npm-cache",
    "nuget",
    "nvidia",
    "nvidia corporation",
    "ollama",
    "openai",
    "packages",
    "pip",
    "pnpm",
    "programs",
    "python",
    "qtproject",
    "rustup",
    "speech",
    "squirreltemp",
    "temp",
    "uv",
    "virtualstore",
    "windows",
    "windowsapps",
    "yarn",
];

/// True when `folder_name` is safe to inspect as a leftover app folder (`R3` vendor/runtime guard).
pub fn is_safe_leftover_name(folder_name: &str) -> bool {
    let trimmed = folder_name.trim();
    if trimmed.is_empty() || trimmed.starts_with('.') || trimmed.starts_with('{') {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !PROTECTED_VENDOR_DIRS.iter().any(|p| *p == lower)
}

/// Converts a detected leftover folder into a queueable `CleanupCandidate` (`UninstallLeftover`).
pub fn leftover_to_candidate(f: &LeftoverFolder) -> CleanupCandidate {
    let mut h = blake3::Hasher::new();
    h.update(b"leftover:");
    h.update(f.path.to_lowercase().as_bytes());
    let id = h.finalize().to_hex()[..16].to_string();
    CleanupCandidate {
        id,
        category: CleanupCategory::UninstallLeftover,
        group_label: f.parent_label.clone(),
        path: f.path.clone(),
        target_kind: TargetKind::Dir,
        bytes: f.bytes,
        file_count: f.file_count,
        age_days: f.age_days,
        reason: f.reason.clone(),
        file_id: None,
        safety_note: "Moves only the stale profile folder into 30-day quarantine; never touches the Windows Registry.".into(),
    }
}

/// Splits a registry `UninstallString` into `(executable, args)` safely, stripping silent flags
/// for `msiexec` so uninstallers always run with their native interactive UI (`R3`).
pub fn parse_uninstall_invocation(raw: &str) -> Option<(String, Vec<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (exe, rest) = if let Some(after_q) = trimmed.strip_prefix('"') {
        let end = after_q.find('"')?;
        let exe = after_q[..end].trim().to_string();
        let rest = after_q[end + 1..].trim();
        (exe, rest)
    } else if trimmed.to_ascii_lowercase().starts_with("msiexec") {
        let mut split_at = trimmed.len();
        for (i, ch) in trimmed.char_indices() {
            if ch.is_whitespace() {
                split_at = i;
                break;
            }
        }
        (trimmed[..split_at].to_string(), trimmed[split_at..].trim())
    } else if let Some(pos) = trimmed.to_ascii_lowercase().find(".exe") {
        let end = pos + 4;
        (trimmed[..end].to_string(), trimmed[end..].trim())
    } else {
        let mut split_at = trimmed.len();
        for (i, ch) in trimmed.char_indices() {
            if ch.is_whitespace() {
                split_at = i;
                break;
            }
        }
        (trimmed[..split_at].to_string(), trimmed[split_at..].trim())
    };

    if exe.is_empty() {
        return None;
    }

    let mut args = split_args(rest);
    if Path::new(&exe)
        .file_name()
        .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("msiexec"))
    {
        args.retain(|a| {
            let low = a.to_ascii_lowercase();
            !(low == "/qn" || low == "/quiet" || low == "/qb" || low == "/passive" || low == "-qn" || low == "-quiet")
        });
    }
    Some((exe, args))
}

fn split_args(rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in rest.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn norm_tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.trim().to_ascii_lowercase())
        .filter(|w| w.len() >= 3)
        .collect()
}

fn matches_known_owner(folder_name: &str, apps: &[InstalledApp], program_dir_names: &HashSet<String>) -> bool {
    let f_low = folder_name.trim().to_ascii_lowercase();
    if f_low.len() < 3 || program_dir_names.contains(&f_low) {
        return true;
    }
    let f_compact: String = f_low.chars().filter(|c| c.is_alphanumeric()).collect();
    if f_compact.len() < 3 {
        return true;
    }

    for app in apps {
        if let Some(ref loc) = app.install_location {
            let loc_low = loc.to_ascii_lowercase();
            if loc_low.contains(&f_low) {
                return true;
            }
        }
        let app_compact: String = app
            .name
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        if !app_compact.is_empty()
            && (app_compact.contains(&f_compact) || f_compact.contains(&app_compact))
        {
            return true;
        }
        if let Some(ref pub_name) = app.publisher {
            let pub_compact: String = pub_name
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            if pub_compact.len() >= 3
                && (pub_compact.contains(&f_compact) || f_compact.contains(&pub_compact))
            {
                return true;
            }
        }
        let app_words = norm_tokens(&app.name);
        if app_words.contains(&f_low) {
            return true;
        }
    }
    false
}

/// Scans `parents` (`%LOCALAPPDATA%`, `%APPDATA%`) for stale leftover folders with no matching
/// installed application (`R3`: folders only, proposed via quarantine, never touching Registry).
pub fn scan_leftover_folders(
    parents: &[(String, PathBuf)],
    installed_apps: &[InstalledApp],
    program_dirs: &[PathBuf],
    now_ms: i64,
    min_age_days: i64,
    min_bytes: i64,
) -> Vec<LeftoverFolder> {
    let mut program_dir_names = HashSet::new();
    for pdir in program_dirs {
        if let Ok(rd) = std::fs::read_dir(pdir) {
            for entry in rd.flatten() {
                if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                    program_dir_names.insert(entry.file_name().to_string_lossy().to_ascii_lowercase());
                }
            }
        }
    }

    let min_age_ms = min_age_days.max(1) * DAY_MS;
    let mut out = Vec::new();
    for (parent_label, parent_path) in parents {
        if !parent_path.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(parent_path) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path_contains_aegis_internal(&path) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() || !meta.is_dir() {
                continue;
            }
            let folder_name = entry.file_name().to_string_lossy().to_string();
            if !is_safe_leftover_name(&folder_name) {
                continue;
            }
            if matches_known_owner(&folder_name, installed_apps, &program_dir_names) {
                continue;
            }
            let (bytes, file_count, newest) = dir_stats_bounded(&path, MAX_LEFTOVER_DEPTH);
            if bytes < min_bytes || file_count == 0 || newest <= 0 || now_ms - newest < min_age_ms {
                continue;
            }
            let age_days = ((now_ms - newest).max(0)) / DAY_MS;
            out.push(LeftoverFolder {
                path: display_path(&path),
                folder_name,
                parent_label: parent_label.clone(),
                bytes,
                file_count,
                age_days,
                reason: format!(
                    "No matching installed app found; untouched for {age_days} days inside {parent_label}"
                ),
            });
        }
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    out
}

/// Scans standard Windows `%LOCALAPPDATA%` and `%APPDATA%` using host `Program Files` + registry apps.
pub fn scan_host_leftover_folders(now_ms: i64) -> Vec<LeftoverFolder> {
    let mut parents = Vec::new();
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        parents.push(("AppData\\Local".to_string(), PathBuf::from(la)));
    }
    if let Some(ra) = std::env::var_os("APPDATA") {
        parents.push(("AppData\\Roaming".to_string(), PathBuf::from(ra)));
    }
    let mut program_dirs = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(p) = std::env::var_os(var) {
            program_dirs.push(PathBuf::from(p));
        }
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        program_dirs.push(PathBuf::from(la).join("Programs"));
    }
    let apps = list_installed_apps(now_ms);
    scan_leftover_folders(&parents, &apps, &program_dirs, now_ms, 30, 1_048_576)
}

#[cfg(windows)]
mod win_reg {
    use super::{InstalledApp, UNUSED_DAYS_THRESHOLD, DAY_MS};
    use std::path::Path;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
        RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW,
        RegGetValueW, RegOpenKeyExW,
    };

    const UNINSTALL_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn reg_str(root: HKEY, sub: &[u16], value_name: &str) -> Option<String> {
        let val = wide(value_name);
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                root,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != 0 || size < 2 {
            return None;
        }
        let n = (size as usize / 2).saturating_sub(1);
        let s = String::from_utf16_lossy(&buf[..n.min(buf.len())]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn reg_dword(root: HKEY, sub: &[u16], value_name: &str) -> Option<u32> {
        let val = wide(value_name);
        let mut out = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                root,
                sub.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut out as *mut u32).cast(),
                &mut size,
            )
        };
        (rc == 0).then_some(out)
    }

    fn hive_sources() -> [(&'static str, HKEY, u32); 3] {
        [
            ("hklm64", HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
            ("hklm32", HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
            ("hkcu", HKEY_CURRENT_USER, 0),
        ]
    }

    fn estimate_last_used(install_loc: Option<&str>, now_ms: i64) -> Option<i64> {
        let loc = install_loc?;
        let p = Path::new(loc);
        let mut latest: i64 = 0;
        let mut inspect = |m: std::fs::Metadata| {
            let t = m
                .accessed()
                .ok()
                .or_else(|| m.modified().ok())
                .and_then(|st| st.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            if t > latest && t <= now_ms + DAY_MS {
                latest = t;
            }
        };
        if p.is_dir() {
            if let Ok(m) = std::fs::metadata(p) {
                inspect(m);
            }
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.take(12).flatten() {
                    if let Ok(m) = std::fs::metadata(e.path()) {
                        inspect(m);
                    }
                }
            }
        }
        (latest > 0).then_some(latest)
    }

    pub fn list_installed_apps(now_ms: i64) -> Vec<InstalledApp> {
        let base_path = wide(UNINSTALL_PATH);
        let mut apps = Vec::new();
        for (scope_id, root, view) in hive_sources() {
            let mut key: HKEY = std::ptr::null_mut();
            if unsafe { RegOpenKeyExW(root, base_path.as_ptr(), 0, KEY_READ | view, &mut key) } != 0 {
                continue;
            }
            let mut idx = 0u32;
            loop {
                let mut name_buf = [0u16; 256];
                let mut len = name_buf.len() as u32;
                let rc = unsafe {
                    RegEnumKeyExW(
                        key,
                        idx,
                        name_buf.as_mut_ptr(),
                        &mut len,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if rc != 0 {
                    break;
                }
                idx += 1;
                let mut sub_wide = name_buf[..len as usize].to_vec();
                let sub_name = String::from_utf16_lossy(&sub_wide);
                sub_wide.push(0);

                if reg_dword(key, &sub_wide, "SystemComponent") == Some(1) {
                    continue;
                }
                if reg_str(key, &sub_wide, "ParentKeyName").is_some() {
                    continue;
                }
                let Some(display_name) = reg_str(key, &sub_wide, "DisplayName") else {
                    continue;
                };
                let release_type = reg_str(key, &sub_wide, "ReleaseType").unwrap_or_default();
                if matches!(release_type.as_str(), "Update" | "Hotfix" | "Security Update") {
                    continue;
                }
                let uninstall = reg_str(key, &sub_wide, "UninstallString");
                let install_location = reg_str(key, &sub_wide, "InstallLocation");
                let estimated_bytes = i64::from(reg_dword(key, &sub_wide, "EstimatedSize").unwrap_or(0)) * 1024;
                let last_used_ms = estimate_last_used(install_location.as_deref(), now_ms);
                let days_since_used = last_used_ms.map(|u| ((now_ms - u).max(0)) / DAY_MS);
                let unused_likely = days_since_used.is_some_and(|d| d >= UNUSED_DAYS_THRESHOLD);

                apps.push(InstalledApp {
                    id: format!("{scope_id}:{sub_name}"),
                    name: display_name,
                    publisher: reg_str(key, &sub_wide, "Publisher"),
                    version: reg_str(key, &sub_wide, "DisplayVersion"),
                    install_date: reg_str(key, &sub_wide, "InstallDate"),
                    install_location,
                    estimated_bytes,
                    has_uninstaller: uninstall.is_some(),
                    last_used_ms,
                    days_since_used,
                    unused_likely,
                    scope: if scope_id == "hkcu" { "Current User" } else { "Machine" }.to_string(),
                });
            }
            unsafe { RegCloseKey(key) };
        }
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        apps
    }

    pub fn lookup_uninstall_command(app_id: &str) -> Option<String> {
        let (scope_id, sub_name) = app_id.split_once(':')?;
        let (_, root, view) = hive_sources().into_iter().find(|(id, _, _)| *id == scope_id)?;
        let base_path = wide(UNINSTALL_PATH);
        let mut key: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, base_path.as_ptr(), 0, KEY_READ | view, &mut key) } != 0 {
            return None;
        }
        let sub_wide = wide(sub_name);
        let cmd = reg_str(key, &sub_wide, "UninstallString");
        unsafe { RegCloseKey(key) };
        cmd
    }
}

#[cfg(windows)]
pub fn list_installed_apps(now_ms: i64) -> Vec<InstalledApp> {
    win_reg::list_installed_apps(now_ms)
}

#[cfg(not(windows))]
pub fn list_installed_apps(_now_ms: i64) -> Vec<InstalledApp> {
    Vec::new()
}

/// Re-reads the exact `UninstallString` from the Windows Registry for `app_id` (`R3` read-only).
/// Keeping lookup backend-side prevents frontend command-injection strings.
#[cfg(windows)]
pub fn lookup_uninstall_command(app_id: &str) -> Option<String> {
    win_reg::lookup_uninstall_command(app_id)
}

#[cfg(not(windows))]
pub fn lookup_uninstall_command(_app_id: &str) -> Option<String> {
    None
}
