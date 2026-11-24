//! Read-only Windows startup item inventory (`R3`: never deletes or modifies `Run` / `StartupApproved`).

use super::StartupEntry;
use std::path::PathBuf;

/// Extracts the target executable or file path from a startup command line, expanding `%ENV%` vars.
pub fn parse_command_executable(cmd: &str) -> Option<PathBuf> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }
    let raw_target = if let Some(after_q) = trimmed.strip_prefix('"') {
        let end = after_q.find('"')?;
        after_q[..end].trim().to_string()
    } else {
        let low = trimmed.to_ascii_lowercase();
        let ext_end = [".exe", ".bat", ".cmd", ".lnk", ".vbs", ".msc"]
            .iter()
            .filter_map(|ext| low.find(ext).map(|idx| idx + ext.len()))
            .filter(|&end| trimmed[end..].is_empty() || trimmed[end..].starts_with(char::is_whitespace))
            .min();
        match ext_end {
            Some(end) => trimmed[..end].trim().to_string(),
            None => trimmed.split_whitespace().next()?.to_string(),
        }
    };

    if raw_target.is_empty() {
        return None;
    }
    Some(PathBuf::from(expand_env_vars(&raw_target)))
}

fn expand_env_vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let var = &after[..end];
            if !var.is_empty()
                && let Some(val) = std::env::var_os(var)
            {
                out.push_str(&val.to_string_lossy());
            } else {
                out.push('%');
                out.push_str(var);
                out.push('%');
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn impact_hint_for(source: &str, orphaned: bool) -> String {
    if orphaned {
        return "Orphaned entry (target missing); inspect or disable via Windows Startup Apps.".into();
    }
    if source.contains("RunOnce") {
        return "Runs once on next sign-in.".into();
    }
    if source.contains("All users") || source.contains("HKLM") {
        return "Starts at sign-in for all users.".into();
    }
    "Starts automatically when you sign in.".into()
}

#[cfg(windows)]
mod win_startup {
    use super::{StartupEntry, impact_hint_for, parse_command_executable};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
        RRF_RT_REG_BINARY, RegCloseKey, RegEnumValueW, RegGetValueW, RegOpenKeyExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_approved_map(root: HKEY, view: u32, subkey: &str) -> HashMap<String, bool> {
        let mut out = HashMap::new();
        let path = wide(subkey);
        let mut key: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) } != 0 {
            return out;
        }
        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            let rc = unsafe {
                RegEnumValueW(
                    key,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc != 0 {
                break;
            }
            idx += 1;
            let mut name_wide = name_buf[..name_len as usize].to_vec();
            let name = String::from_utf16_lossy(&name_wide);
            name_wide.push(0);
            let mut data = [0u8; 32];
            let mut size = data.len() as u32;
            let grc = unsafe {
                RegGetValueW(
                    key,
                    std::ptr::null(),
                    name_wide.as_ptr(),
                    RRF_RT_REG_BINARY,
                    std::ptr::null_mut(),
                    data.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if grc == 0 && size >= 1 {
                // In StartupApproved REG_BINARY: even first byte (e.g. 0x02, 0x06) means enabled; odd (0x03, 0x07) disabled.
                out.insert(name.to_lowercase(), (data[0] % 2) == 0);
            }
        }
        unsafe { RegCloseKey(key) };
        out
    }

    fn enum_run_key(
        id_prefix: &str,
        source_label: &str,
        root: HKEY,
        view: u32,
        subkey: &str,
        approved: &HashMap<String, bool>,
        out: &mut Vec<StartupEntry>,
    ) {
        let path = wide(subkey);
        let mut key: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) } != 0 {
            return;
        }
        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            let mut val_type = 0u32;
            let mut data_buf = [0u8; 2048];
            let mut data_len = data_buf.len() as u32;
            let rc = unsafe {
                RegEnumValueW(
                    key,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null_mut(),
                    &mut val_type,
                    data_buf.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if rc != 0 {
                break;
            }
            idx += 1;
            let name = String::from_utf16_lossy(&name_buf[..name_len as usize]).trim().to_string();
            if name.is_empty() {
                continue;
            }
            let u16_slice: Vec<u16> = data_buf[..data_len as usize]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&w| w != 0)
                .collect();
            let command = String::from_utf16_lossy(&u16_slice).trim().to_string();
            if command.is_empty() {
                continue;
            }
            let enabled = approved.get(&name.to_lowercase()).copied().unwrap_or(true);
            let target_pb = parse_command_executable(&command);
            let target_exists = target_pb.as_ref().is_some_and(|p| p.exists());
            let orphaned = target_pb.as_ref().is_some_and(|p| p.is_absolute() && !p.exists());
            out.push(StartupEntry {
                id: format!("{id_prefix}:{name}"),
                name,
                command,
                source: source_label.to_string(),
                enabled,
                target_path: target_pb.map(|p| p.to_string_lossy().to_string()),
                target_exists,
                orphaned,
                impact_hint: impact_hint_for(source_label, orphaned),
            });
        }
        unsafe { RegCloseKey(key) };
    }

    fn scan_startup_folder(
        id_prefix: &str,
        source_label: &str,
        dir: &Path,
        approved: &HashMap<String, bool>,
        out: &mut Vec<StartupEntry>,
    ) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let p = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let enabled = approved.get(&file_name.to_lowercase()).copied().unwrap_or(true);
            let disp = p.to_string_lossy().to_string();
            let exists = p.exists();
            out.push(StartupEntry {
                id: format!("{id_prefix}:{file_name}"),
                name: Path::new(&file_name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or(file_name),
                command: disp.clone(),
                source: source_label.to_string(),
                enabled,
                target_path: Some(disp),
                target_exists: exists,
                orphaned: !exists,
                impact_hint: impact_hint_for(source_label, !exists),
            });
        }
    }

    pub fn list_startup_items() -> Vec<StartupEntry> {
        let hkcu_run_app = read_approved_map(
            HKEY_CURRENT_USER,
            0,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        );
        let hklm_run_app = read_approved_map(
            HKEY_LOCAL_MACHINE,
            KEY_WOW64_64KEY,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        );
        let hklm32_run_app = read_approved_map(
            HKEY_LOCAL_MACHINE,
            KEY_WOW64_32KEY,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32",
        );
        let hkcu_dir_app = read_approved_map(
            HKEY_CURRENT_USER,
            0,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder",
        );
        let hklm_dir_app = read_approved_map(
            HKEY_LOCAL_MACHINE,
            KEY_WOW64_64KEY,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder",
        );

        let mut out = Vec::new();
        enum_run_key(
            "hkcu_run",
            "User Run (HKCU)",
            HKEY_CURRENT_USER,
            0,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
            &hkcu_run_app,
            &mut out,
        );
        enum_run_key(
            "hkcu_runonce",
            "User RunOnce (HKCU)",
            HKEY_CURRENT_USER,
            0,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
            &hkcu_run_app,
            &mut out,
        );
        enum_run_key(
            "hklm64_run",
            "All users (HKLM)",
            HKEY_LOCAL_MACHINE,
            KEY_WOW64_64KEY,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
            &hklm_run_app,
            &mut out,
        );
        enum_run_key(
            "hklm32_run",
            "All users 32-bit (HKLM)",
            HKEY_LOCAL_MACHINE,
            KEY_WOW64_32KEY,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
            &hklm32_run_app,
            &mut out,
        );

        if let Some(appdata) = std::env::var_os("APPDATA") {
            let dir = PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup");
            scan_startup_folder("user_startup", "User Startup folder", &dir, &hkcu_dir_app, &mut out);
        }
        if let Some(progdata) = std::env::var_os("ProgramData") {
            let dir = PathBuf::from(progdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup");
            scan_startup_folder("common_startup", "All users Startup folder", &dir, &hklm_dir_app, &mut out);
        }

        out.sort_by(|a, b| {
            b.enabled
                .cmp(&a.enabled)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }
}

/// Lists Windows startup items strictly read-only (`R3`).
#[cfg(windows)]
pub fn list_startup_items() -> Vec<StartupEntry> {
    win_startup::list_startup_items()
}

#[cfg(not(windows))]
pub fn list_startup_items() -> Vec<StartupEntry> {
    Vec::new()
}

pub fn make_impact_note_for_tests(source: &str, cmd: &str, exists_override: bool) -> StartupEntry {
    let target = parse_command_executable(cmd);
    let orphaned = target.is_some() && !exists_override;
    StartupEntry {
        id: "test:item".into(),
        name: "Test".into(),
        command: cmd.into(),
        source: source.into(),
        enabled: true,
        target_path: target.map(|p| p.to_string_lossy().to_string()),
        target_exists: exists_override,
        orphaned,
        impact_hint: impact_hint_for(source, orphaned),
    }
}
