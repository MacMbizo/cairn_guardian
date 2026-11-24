//! Windows System Care Tauri IPC commands (`Project 4`: `K1`, `K2`, `R1`, `R3`, `R8`).

use crate::commands::{AEGIS_EVENT, AppState, lock_store, map_err};
use crate::engine_host::wake;
use aegis_core::care::apps::{
    list_installed_apps, lookup_uninstall_command, parse_uninstall_invocation,
    scan_host_leftover_folders,
};
use aegis_core::care::cleanup::{CleanupScanConfig, propose_care_quarantine, scan_cleanup};
use aegis_core::care::smart::probe_drive_health;
use aegis_core::care::startup::list_startup_items;
use aegis_core::care::trends::{build_disk_trends, ensure_today_space_sample};
use aegis_core::care::{
    CleanupCandidate, CleanupScanReport, DiskTrendsReport, DriveHealthItem, InstalledApp,
    LeftoverFolder, StartupEntry,
};
use aegis_core::contracts::AegisEvent;
use aegis_core::error::{AegisError, IpcError};
use aegis_core::now_ms;
use tauri::{AppHandle, Emitter, State};

#[cfg(windows)]
fn shell_open_uri_or_exe(target: &str, params: Option<&str>) -> std::result::Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let op_w = wide("open");
    let target_w = wide(target);
    let params_w = params.map(wide);
    let params_ptr = params_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());

    // `SW_SHOWNORMAL = 1`; return value > 32 indicates success per Win32 ShellExecuteW docs.
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op_w.as_ptr(),
            target_w.as_ptr(),
            params_ptr,
            std::ptr::null(),
            1,
        )
    };
    if (rc as isize) <= 32 {
        return Err(format!("Windows shell returned code {}", rc as isize));
    }
    Ok(())
}

#[cfg(not(windows))]
fn shell_open_uri_or_exe(_target: &str, _params: Option<&str>) -> std::result::Result<(), String> {
    Err("System settings and native uninstallers are only supported on Windows.".into())
}

#[tauri::command(async)]
pub fn care_scan_cleanup(state: State<'_, AppState>) -> Result<CleanupScanReport, IpcError> {
    let store = map_err(state.open_store())?;
    let cfg = CleanupScanConfig::for_host();
    map_err(scan_cleanup(store.conn(), &cfg))
}

#[tauri::command(async)]
pub fn care_propose_cleanup(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<CleanupCandidate>,
) -> Result<String, IpcError> {
    let count = items.len();
    let batch_id = {
        let mut store = lock_store(&state)?;
        map_err(propose_care_quarantine(&mut store, &items))?
    };
    let _ = app.emit(
        AEGIS_EVENT,
        AegisEvent::ActionProposed {
            v: 1,
            batch_id: batch_id.clone(),
            count,
        },
    );
    wake(&app);
    Ok(batch_id)
}

#[tauri::command(async)]
pub fn care_installed_apps() -> Result<Vec<InstalledApp>, IpcError> {
    Ok(list_installed_apps(now_ms()))
}

#[tauri::command(async)]
pub fn care_leftover_folders() -> Result<Vec<LeftoverFolder>, IpcError> {
    Ok(scan_host_leftover_folders(now_ms()))
}

#[tauri::command(async)]
pub fn care_launch_uninstaller(app_id: String) -> Result<(), IpcError> {
    let raw = lookup_uninstall_command(&app_id).ok_or_else(|| {
        AegisError::NotFound("No Windows uninstaller entry found for this application.".into())
            .to_ipc()
    })?;
    let (exe, args) = parse_uninstall_invocation(&raw).ok_or_else(|| {
        AegisError::InvalidInput("Could not parse the app's registered UninstallString.".into())
            .to_ipc()
    })?;

    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(740) => {
            // `ERROR_ELEVATION_REQUIRED`: delegate to ShellExecuteW so Windows shows standard UAC.
            let joined = if args.is_empty() {
                None
            } else {
                Some(
                    args.iter()
                        .map(|a| {
                            if a.contains(' ') && !a.starts_with('"') {
                                format!("\"{a}\"")
                            } else {
                                a.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            };
            shell_open_uri_or_exe(&exe, joined.as_deref())
                .map_err(|msg| AegisError::Io(std::io::Error::other(msg)).to_ipc())
        }
        Err(e) => Err(AegisError::Io(e).to_ipc()),
    }
}

#[tauri::command(async)]
pub fn care_open_windows_apps() -> Result<(), IpcError> {
    shell_open_uri_or_exe("ms-settings:appsfeatures", None)
        .map_err(|msg| AegisError::Io(std::io::Error::other(msg)).to_ipc())
}

#[tauri::command(async)]
pub fn care_startup_items() -> Result<Vec<StartupEntry>, IpcError> {
    Ok(list_startup_items())
}

#[tauri::command(async)]
pub fn care_open_startup_settings() -> Result<(), IpcError> {
    shell_open_uri_or_exe("ms-settings:startupapps", None)
        .map_err(|msg| AegisError::Io(std::io::Error::other(msg)).to_ipc())
}

#[tauri::command(async)]
pub fn care_drive_health(state: State<'_, AppState>) -> Result<Vec<DriveHealthItem>, IpcError> {
    let store = map_err(state.open_store())?;
    Ok(probe_drive_health(store.conn()))
}

#[tauri::command(async)]
pub fn care_disk_trends(
    state: State<'_, AppState>,
    days: Option<i64>,
) -> Result<DiskTrendsReport, IpcError> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let store = map_err(state.open_store())?;
    map_err(ensure_today_space_sample(&store, &today))?;
    map_err(build_disk_trends(&store, &today, days.unwrap_or(30)))
}
