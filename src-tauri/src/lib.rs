pub mod commands;
pub mod watch;

use aegis_core::db::SqliteStore;
use commands::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_local_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("aegis-guardian"));
            let db_path = data_dir.join("aegis.db");
            let store = SqliteStore::open(&db_path)?;
            // First run: start with the PC folders where clutter lands (spec §7.8.1).
            let _ = aegis_core::scanner::seed_default_roots(&store);
            app.manage(AppState {
                store: Mutex::new(store),
                db_path,
                topup_cancel: Default::default(),
                topup_running: Default::default(),
            });
            watch::spawn(app.handle().clone());
            Ok(())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aegis Guardian");
}
