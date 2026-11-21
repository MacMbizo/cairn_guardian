//! Living in the tray (spec §8): single instance, start with Windows, window lifecycle, tray
//! icon and menu, global search shortcut, and navigation into the app from outside it.

use crate::commands::{AEGIS_EVENT, AppState};
use aegis_core::contracts::AegisEvent;
use aegis_core::engine::Screen;
use aegis_core::engine::reminders::TrayState;
use chrono::{DateTime, Duration, Local, TimeZone};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

pub const TRAY_ID: &str = "aegis";
pub const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+Space";
const MAIN: &str = "main";

/// Order plugins are registered in. Single-instance must come first, so a second launch hands
/// over to the running engine before anything else starts (two engines on one database fight).
pub fn plugin_order() -> &'static [&'static str] {
    &["single-instance", "autostart", "global-shortcut", "dialog"]
}

/// A screen to open, waiting for a freshly created window's page to ask for it.
#[derive(Default)]
pub struct NavSlot(Mutex<Option<Screen>>);

impl NavSlot {
    pub fn set(&self, s: Screen) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
    }
    pub fn take(&self) -> Option<Screen> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
}

/// When a pause ends, in ms since epoch: `hour` = an hour from now, `tomorrow` = 07:00 tomorrow,
/// `resume` = 0 (not paused). `None` for an unknown mode.
pub fn pause_until(mode: &str, now: DateTime<Local>) -> Option<i64> {
    match mode {
        "hour" => Some(now.timestamp_millis() + 3_600_000),
        "tomorrow" => {
            let day = now.date_naive() + Duration::days(1);
            let at = day.and_hms_opt(7, 0, 0)?;
            Local.from_local_datetime(&at).earliest().map(|t| t.timestamp_millis())
        }
        "resume" => Some(0),
        _ => None,
    }
}

/// Opens (or creates) the main window, optionally on `screen`. Safe from any thread.
///
/// The window is built on a worker thread: on Windows, building a webview window from the main
/// thread (tray and menu handlers, the single-instance callback) deadlocks or silently fails.
pub fn open_main(app: &AppHandle, screen: Option<Screen>) {
    if let Some(s) = screen {
        app.state::<AppState>().nav.set(s);
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let window = match handle.get_webview_window(MAIN) {
            Some(w) => Ok(w),
            None => match handle.config().app.windows.iter().find(|w| w.label == MAIN) {
                Some(cfg) => tauri::WebviewWindowBuilder::from_config(&handle, cfg).and_then(|b| {
                    // A test run with its own database also gets its own WebView2 profile: a
                    // profile folder can't be shared with another running copy.
                    match std::env::var_os("AEGIS_DATA_DIR") {
                        Some(dir) => b.data_directory(std::path::PathBuf::from(dir).join("webview")).build(),
                        None => b.build(),
                    }
                }),
                None => Err(tauri::Error::WindowNotFound),
            },
        };
        match window {
            Ok(w) => {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            Err(e) => eprintln!("aegis: could not open the window: {e}"),
        }
        if let Some(s) = screen {
            let _ = handle.emit(AEGIS_EVENT, AegisEvent::Navigate { v: 1, screen: s });
        }
    });
}

fn icon(state: TrayState) -> Option<Image<'static>> {
    let bytes: &'static [u8] = match state {
        TrayState::Calm => include_bytes!("../icons/tray-calm.png"),
        TrayState::Waiting => include_bytes!("../icons/tray-waiting.png"),
        TrayState::Urgent => include_bytes!("../icons/tray-urgent.png"),
    };
    Image::from_bytes(bytes).ok()
}

/// Owner-facing tooltip for the tray icon.
pub fn tooltip(state: TrayState, waiting: usize) -> String {
    match state {
        TrayState::Calm => "Aegis: all calm".into(),
        TrayState::Waiting if waiting == 1 => "Aegis: 1 thing waiting".into(),
        TrayState::Waiting => format!("Aegis: {waiting} things waiting"),
        TrayState::Urgent => "Aegis: needs you now".into(),
    }
}

pub fn set_tray_state(app: &AppHandle, state: TrayState, waiting: usize, paused: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(icon(state));
        let mut tip = tooltip(state, waiting);
        if paused {
            tip.push_str(" (paused)");
        }
        let _ = tray.set_tooltip(Some(tip));
    }
}

/// The "Back up now" item, kept so the engine can enable it while the Backup drive is plugged in.
pub struct TrayItems {
    pub backup_now: MenuItem<Wry>,
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<TrayItems> {
    let open = MenuItem::with_id(app, "open", "Open Aegis", true, None::<&str>)?;
    let search = MenuItem::with_id(app, "search", "Search everywhere (Ctrl+Shift+Space)", true, None::<&str>)?;
    let week = MenuItem::with_id(app, "week", "This week's note", true, None::<&str>)?;
    let backup_now = MenuItem::with_id(app, "backup_now", "Back up now", false, None::<&str>)?;
    let pause = Submenu::with_items(
        app,
        "Pause",
        true,
        &[
            &MenuItem::with_id(app, "pause_hour", "For 1 hour", true, None::<&str>)?,
            &MenuItem::with_id(app, "pause_tomorrow", "Until tomorrow", true, None::<&str>)?,
            &MenuItem::with_id(app, "pause_resume", "Resume", true, None::<&str>)?,
        ],
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Aegis", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &search, &week, &sep, &backup_now, &pause, &sep2, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(TrayState::Calm).ok_or_else(|| tauri::Error::InvalidIcon(std::io::Error::other("tray icon")))?)
        .tooltip(tooltip(TrayState::Calm, 0))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => open_main(app, None),
            "search" => open_main(app, Some(Screen::Search)),
            "week" => open_main(app, Some(Screen::Week)),
            "backup_now" => crate::engine_host::backup_now(app),
            "pause_hour" => set_pause(app, "hour"),
            "pause_tomorrow" => set_pause(app, "tomorrow"),
            "pause_resume" => set_pause(app, "resume"),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                open_main(tray.app_handle(), None);
            }
        })
        .build(app)?;
    Ok(TrayItems { backup_now })
}

/// Pauses or resumes idle work and convenient reminders (safety keeps running).
pub fn set_pause(app: &AppHandle, mode: &str) {
    let Some(until) = pause_until(mode, Local::now()) else { return };
    let state = app.state::<AppState>();
    state.paused_until.store(until, Ordering::SeqCst);
    if let Ok(store) = state.store.lock() {
        let _ = store.set_setting("engine.paused_until", "global", &until.to_string());
    }
    crate::engine_host::wake(app);
}

/// The first time the window is closed, explain where Aegis went.
pub fn first_close_hint(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(store) = state.store.lock() else { return };
    if store.get_setting("engine.close_hint_shown", "global").ok().flatten().is_some() {
        return;
    }
    let _ = store.set_setting("engine.close_hint_shown", "global", "true");
    drop(store);
    crate::engine_host::toast(app, "Aegis keeps watching in the tray", "Right-click it to quit.", Screen::Home, None);
}

/// Registers the global search shortcut; returns whether Windows accepted it.
pub fn register_shortcut(app: &AppHandle, accel: &str) -> bool {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let ok = gs.register(accel).is_ok();
    app.state::<AppState>().shortcut_ok.store(ok, Ordering::SeqCst);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_core::engine::Screen;
    use chrono::{Local, TimeZone};

    #[test]
    fn pending_navigation_is_taken_once() {
        let slot = NavSlot::default();
        slot.set(Screen::Week);
        assert_eq!(slot.take(), Some(Screen::Week));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn single_instance_plugin_registered_first() {
        assert_eq!(plugin_order()[0], "single-instance");
    }

    #[test]
    fn pause_modes_compute_until() {
        let now = Local.with_ymd_and_hms(2026, 11, 21, 15, 30, 0).unwrap();
        let ms = now.timestamp_millis();
        assert_eq!(pause_until("hour", now), Some(ms + 3_600_000));
        let tomorrow_7 = Local.with_ymd_and_hms(2026, 11, 22, 7, 0, 0).unwrap().timestamp_millis();
        assert_eq!(pause_until("tomorrow", now), Some(tomorrow_7));
        assert_eq!(pause_until("resume", now), Some(0));
        assert_eq!(pause_until("forever", now), None);
    }

    #[test]
    fn tooltip_copy() {
        assert_eq!(tooltip(TrayState::Calm, 0), "Aegis: all calm");
        assert_eq!(tooltip(TrayState::Waiting, 2), "Aegis: 2 things waiting");
        assert_eq!(tooltip(TrayState::Urgent, 1), "Aegis: needs you now");
    }
}
