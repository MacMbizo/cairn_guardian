pub mod commands;
pub mod engine_host;
pub mod shell;
pub mod watch;

use aegis_core::db::SqliteStore;
use aegis_core::engine::Screen;
use commands::AppState;
use std::sync::Mutex;
use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::ShortcutState;

/// Started by Windows at login: stay in the tray, no window.
const BACKGROUND_ARG: &str = "--background";

/// The system time zone's offset from UTC, in seconds, at a UTC instant (ms). Handles daylight saving.
fn local_offset_secs(utc_ms: i64) -> i32 {
    use chrono::{Local, Offset, TimeZone};
    Local.timestamp_millis_opt(utc_ms).single().map_or(0, |d| d.offset().fix().local_minus_utc())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    aegis_core::filing::set_local_offset(local_offset_secs);
    // Plugin order matters: see `shell::plugin_order`.
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| shell::open_main(app, None)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![BACKGROUND_ARG])))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        shell::open_main(app, Some(Screen::Search));
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // AEGIS_DATA_DIR points test and dev runs at their own database, never the owner's.
            let data_dir = std::env::var_os("AEGIS_DATA_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| {
                app.path().app_local_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("aegis-guardian"))
            });
            let db_path = data_dir.join("aegis.db");
            let store = SqliteStore::open(&db_path)?;
            // First run: start with the PC folders where clutter lands (spec §7.8.1).
            let _ = aegis_core::scanner::seed_default_roots(&store);
            let first_run = store.get_setting("engine.autostart_initialised", "global")?.is_none();
            let shortcut = store
                .get_setting("engine.shortcut", "global")?
                .and_then(|j| serde_json::from_str::<String>(&j).ok())
                .unwrap_or_else(|| shell::DEFAULT_SHORTCUT.to_string());
            if first_run {
                store.set_setting("engine.autostart_initialised", "global", "true")?;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            app.manage(AppState {
                store: Mutex::new(store),
                db_path,
                topup_cancel: Default::default(),
                topup_running: Default::default(),
                cataloguing: Default::default(),
                nav: Default::default(),
                paused_until: Default::default(),
                shortcut_ok: Default::default(),
                engine_tx: Mutex::new(tx),
                tray_items: Mutex::new(None),
            });
            // Start with Windows by default (spec §8); the owner can turn it off in Settings.
            // Dev builds never register themselves to run at login.
            if first_run && !cfg!(debug_assertions) {
                let _ = app.autolaunch().enable();
            }
            let handle = app.handle().clone();
            let items = shell::build_tray(&handle)?;
            if let Ok(mut g) = app.state::<AppState>().tray_items.lock() {
                *g = Some(items);
            }
            shell::register_shortcut(&handle, &shortcut);
            if !std::env::args().any(|a| a == BACKGROUND_ARG) {
                shell::open_main(&handle, None);
            }
            watch::spawn(handle.clone());
            engine_host::spawn(handle, rx);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                shell::first_close_hint(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::roots_list,
            commands::roots_add,
            commands::roots_remove,
            commands::roots_toggle,
            commands::ignore_get,
            commands::ignore_set,
            commands::scan_start,
            commands::scan_status,
            commands::scan_coverage_report,
            commands::volumes_list,
            commands::index_stats,
            commands::search_query,
            commands::file_get,
            commands::file_set_lifecycle,
            commands::atomic_unit_list,
            commands::atomic_unit_override,
            commands::dedupe_report,
            commands::dedupe_propose_quarantine,
            commands::actions_propose,
            commands::actions_simulate,
            commands::actions_approve,
            commands::actions_execute,
            commands::actions_reject,
            commands::actions_rollback,
            commands::actions_resolve_conflict,
            commands::actions_list,
            commands::quarantine_list,
            commands::quarantine_space_summary,
            commands::quarantine_restore,
            commands::quarantine_purge,
            commands::health_get,
            commands::playbook_disk_rescue,
            commands::audit_list,
            commands::hw_profile,
            commands::doctor_run,
            commands::drives::drives_list,
            commands::drives::drives_sync,
            commands::drives::drive_catalog,
            commands::drives::drives_cataloguing,
            commands::drives::drive_refresh,
            commands::drives::drive_plan_get,
            commands::drives::drive_plan_apply,
            commands::drives::dupes_grouped,
            commands::drives::dupe_decide,
            commands::drives::dupe_decisions_list,
            commands::drives::dupe_decision_undo,
            commands::drives::backup_coverage,
            commands::drives::backup_sources_list,
            commands::drives::backup_source_add,
            commands::drives::backup_source_remove,
            commands::drives::backup_topup_plan,
            commands::drives::backup_topup_start,
            commands::drives::backup_topup_pause,
            commands::drives::backup_guard_override,
            commands::drives::backup_standing_grant,
            commands::drives::backup_standing_status,
            commands::drives::previous_versions_list,
            commands::drives::previous_version_restore,
            commands::engine::reminders_open,
            commands::engine::reminder_dismiss,
            commands::engine::reminder_act,
            commands::engine::reminder_undemote,
            commands::engine::note_latest,
            commands::engine::note_mark_read,
            commands::engine::engine_pause,
            commands::engine::engine_status,
            commands::engine::engine_settings_set,
            commands::engine::nav_take,
            commands::engine::tripwire_was_me,
            commands::engine::tripwire_locks,
            commands::filing::filing_scopes,
            commands::filing::filing_blueprints,
            commands::filing::filing_choose,
            commands::filing::filing_next_batch,
            commands::filing::filing_approve,
            commands::filing::filing_undo,
            commands::filing::filing_skip,
            commands::filing::filing_to_sort,
            commands::filing::filing_destinations,
            commands::filing::filing_place,
            commands::filing::filing_rules,
            commands::filing::filing_rule_stop,
            commands::filing::filing_rule_delete,
            #[cfg(debug_assertions)]
            commands::engine::engine_debug_raise,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Aegis Guardian");
    app.run(|_app, event| {
        // Closing the last window keeps Aegis running in the tray; only "Quit Aegis" exits.
        if let RunEvent::ExitRequested { code: None, api, .. } = event {
            api.prevent_exit();
        }
    });
}
