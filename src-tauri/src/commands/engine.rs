//! Always-on engine IPC (spec §8–§10): reminders, the weekly note, pause, settings, navigation.

use super::{AppState, lock_store, map_err};
use crate::engine_host::wake;
use crate::shell::{DEFAULT_SHORTCUT, pause_until, register_shortcut};
use aegis_core::engine::note::{WeeklyNote, latest, mark_read};
use aegis_core::engine::reminders::{Reminder, act, demoted_kinds, dismiss, list_open, tray_state, undemote};
use aegis_core::engine::{EngineStatus, Screen};
use aegis_core::error::IpcError;
use aegis_core::now_ms;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

fn setting(state: &State<'_, AppState>, key: &str) -> Option<String> {
    lock_store(state).ok()?.get_setting(key, "global").ok().flatten().map(|v| v.trim_matches('"').to_string())
}

#[tauri::command(async)]
pub fn reminders_open(state: State<'_, AppState>) -> Result<Vec<Reminder>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_open(store.conn()))
}

/// "Not now". Returns true when this moved the reminder's kind to the weekly note.
#[tauri::command(async)]
pub fn reminder_dismiss(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<bool, IpcError> {
    let demoted = {
        let store = lock_store(&state)?;
        map_err(dismiss(store.conn(), &id, now_ms()))?
    };
    wake(&app);
    Ok(demoted)
}

#[tauri::command(async)]
pub fn reminder_act(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), IpcError> {
    {
        let store = lock_store(&state)?;
        map_err(act(store.conn(), &id, now_ms()))?;
    }
    wake(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn reminder_undemote(app: AppHandle, state: State<'_, AppState>, kind: String) -> Result<(), IpcError> {
    {
        let store = lock_store(&state)?;
        map_err(undemote(store.conn(), &kind))?;
    }
    wake(&app);
    Ok(())
}

#[tauri::command(async)]
pub fn note_latest(state: State<'_, AppState>) -> Result<Option<WeeklyNote>, IpcError> {
    let store = lock_store(&state)?;
    map_err(latest(store.conn()))
}

#[tauri::command(async)]
pub fn note_mark_read(state: State<'_, AppState>, week: String) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(mark_read(store.conn(), &week, now_ms()))
}

/// `mode`: "hour", "tomorrow" or "resume".
#[tauri::command(async)]
pub fn engine_pause(app: AppHandle, mode: String) -> Result<(), IpcError> {
    if pause_until(&mode, chrono::Local::now()).is_none() {
        return Err(aegis_core::error::AegisError::InvalidInput(format!("unknown pause mode {mode}")).to_ipc());
    }
    crate::shell::set_pause(&app, &mode);
    Ok(())
}

#[tauri::command(async)]
pub fn engine_status(app: AppHandle, state: State<'_, AppState>) -> Result<EngineStatus, IpcError> {
    let paused = state.paused_until.load(Ordering::SeqCst);
    let (tray, demoted) = {
        let store = lock_store(&state)?;
        (map_err(tray_state(store.conn()))?, map_err(demoted_kinds(store.conn()))?)
    };
    Ok(EngineStatus {
        paused_until: (paused > now_ms()).then_some(paused),
        tray,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        note_day: setting(&state, "engine.note_day").and_then(|v| v.parse().ok()).unwrap_or(6),
        note_hour: setting(&state, "engine.note_hour").and_then(|v| v.parse().ok()).unwrap_or(18),
        shortcut: setting(&state, "engine.shortcut").unwrap_or_else(|| DEFAULT_SHORTCUT.to_string()),
        shortcut_ok: state.shortcut_ok.load(Ordering::SeqCst),
        demoted: demoted.into_iter().map(|(k, _)| k).collect(),
    })
}

#[tauri::command(async)]
pub fn engine_settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    autostart: Option<bool>,
    note_day: Option<u8>,
    note_hour: Option<u8>,
    shortcut: Option<String>,
) -> Result<(), IpcError> {
    if let Some(on) = autostart {
        let al = app.autolaunch();
        let res = if on { al.enable() } else { al.disable() };
        res.map_err(|e| aegis_core::error::AegisError::InvalidInput(e.to_string()).to_ipc())?;
    }
    {
        let store = lock_store(&state)?;
        if let Some(d) = note_day.filter(|d| *d <= 6) {
            map_err(store.set_setting("engine.note_day", "global", &d.to_string()))?;
        }
        if let Some(h) = note_hour.filter(|h| *h <= 23) {
            map_err(store.set_setting("engine.note_hour", "global", &h.to_string()))?;
        }
        if let Some(s) = &shortcut {
            map_err(store.set_setting("engine.shortcut", "global", &serde_json::to_string(s).unwrap_or_default()))?;
        }
    }
    if let Some(s) = shortcut {
        register_shortcut(&app, &s);
    }
    wake(&app);
    Ok(())
}

/// The screen a notification or the tray asked for before this page loaded (taken once).
#[tauri::command(async)]
pub fn nav_take(state: State<'_, AppState>) -> Option<Screen> {
    state.nav.take()
}

/// Folders whose backup the tripwire has locked (the banner shows these on every start).
#[tauri::command(async)]
pub fn tripwire_locks(state: State<'_, AppState>) -> Result<Vec<String>, IpcError> {
    let store = lock_store(&state)?;
    map_err(aegis_core::backup::guard::locked_folders(store.conn()))
}

/// "It was me" on the tripwire banner: unlocks the folder's backup and lets those changes
/// through on the next top-up.
#[tauri::command(async)]
pub fn tripwire_was_me(app: AppHandle, state: State<'_, AppState>, folder: String) -> Result<(), IpcError> {
    {
        let mut store = lock_store(&state)?;
        let backup = map_err(aegis_core::backup::backup_volume(store.conn()))?.map(|(id, _, _)| id);
        map_err(aegis_core::backup::guard::release_lock(&mut store, backup.as_deref(), &folder))?;
        let open = map_err(list_open(store.conn()))?;
        for r in open.iter().filter(|r| r.kind == "tripwire" && r.subject == folder) {
            map_err(act(store.conn(), &r.id, now_ms()))?;
        }
    }
    wake(&app);
    Ok(())
}

/// Debug builds only: raises a sample reminder so notifications can be checked by hand.
#[cfg(debug_assertions)]
#[tauri::command(async)]
pub fn engine_debug_raise(app: AppHandle, state: State<'_, AppState>, level: String) -> Result<(), IpcError> {
    use aegis_core::engine::{Finding, FindingAction, Level, reminders::sync_findings};
    let lvl = match level.as_str() {
        "now" => Level::Now,
        "note" => Level::Note,
        _ => Level::Convenient,
    };
    {
        let store = lock_store(&state)?;
        let f = Finding {
            kind: "debug".into(),
            subject: format!("debug-{level}"),
            level: lvl,
            title: "Test reminder from Cairn".into(),
            detail: "Click to open This week.".into(),
            bytes: None,
            action: FindingAction::Open { screen: Screen::Week },
        };
        map_err(sync_findings(store.conn(), &[], &[f], now_ms()))?;
    }
    wake(&app);
    Ok(())
}
