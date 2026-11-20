pub mod commands;

use aegis_core::db::SqliteStore;
use commands::AppState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app
                .path()
                .app_local_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("aegis-guardian"));
            let db_path = data_dir.join("aegis.db");
            let store = SqliteStore::open(&db_path)?;
            app.manage(AppState {
                store: Mutex::new(store),
            });
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aegis Guardian");
}
