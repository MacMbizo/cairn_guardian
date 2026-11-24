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

/// Restores any owner-selected local embedding model path from `settings` into `embedder`.
pub(crate) fn apply_saved_embed_model(
    store: &SqliteStore,
    embedder: &aegis_embed::OnDemandEmbedder,
) {
    let saved = store
        .get_setting("embed.model_path", "global")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Option<String>>(&j).ok())
        .flatten();
    embedder.set_selection(aegis_embed::ModelSelection::from_setting(saved.as_deref()));
}

/// Restores any owner-selected local RAG LLM model path from `settings` into `llm`.
pub(crate) fn apply_saved_rag_model(store: &SqliteStore, llm: &aegis_llm::OnDemandLlm) {
    let saved = store
        .get_setting("rag.model_path", "global")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Option<String>>(&j).ok())
        .flatten();
    if let Some(ref p) = saved
        && p != aegis_llm::BUILTIN_EXTRACTIVE_ID
        && !std::path::Path::new(p).is_file()
    {
        return;
    }
    let _ = llm.set_selection(aegis_llm::LlmSelection::from_setting(saved.as_deref()));
}

/// Seeds initial PC roots, records first-run autostart setting, and returns the search shortcut.
pub(crate) fn init_unlocked_settings(
    app: &tauri::AppHandle,
    store: &SqliteStore,
) -> aegis_core::error::Result<String> {
    // First run: start with the PC folders where clutter lands (spec §7.8.1).
    let _ = aegis_core::scanner::seed_default_roots(store);
    let first_run = store.get_setting("engine.autostart_initialised", "global")?.is_none();
    let shortcut = store
        .get_setting("engine.shortcut", "global")?
        .and_then(|j| serde_json::from_str::<String>(&j).ok())
        .unwrap_or_else(|| shell::DEFAULT_SHORTCUT.to_string());
    if first_run {
        store.set_setting("engine.autostart_initialised", "global", "true")?;
        // Dev builds never register themselves to run at login.
        if !cfg!(debug_assertions) {
            let _ = app.autolaunch().enable();
        }
    }
    Ok(shortcut)
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
            let embedder = std::sync::Arc::new(aegis_embed::OnDemandEmbedder::new(data_dir.join("models")));
            let llm = std::sync::Arc::new(aegis_llm::OnDemandLlm::for_app_data(data_dir.join("models")));
            let db_path = data_dir.join("aegis.db");
            let open_res = aegis_core::db::crypto::open_or_migrate_encrypted(&db_path)?;
            let (tx, rx) = std::sync::mpsc::channel();
            let handle = app.handle().clone();

            match open_res {
                aegis_core::db::crypto::VaultOpenResult::Ready {
                    store,
                    key,
                    meta: _,
                    migrated,
                } => {
                    apply_saved_embed_model(&store, &embedder);
                    apply_saved_rag_model(&store, &llm);
                    let shortcut = init_unlocked_settings(&handle, &store)?;
                    app.manage(AppState {
                        store: Mutex::new(store),
                        db_path,
                        db_key: Mutex::new(Some(std::sync::Arc::new(key))),
                        vault_locked: std::sync::atomic::AtomicBool::new(false),
                        vault_lock_reason: Mutex::new(None),
                        vault_migrated: std::sync::atomic::AtomicBool::new(migrated),
                        sensitive_unlocked_until: std::sync::atomic::AtomicI64::new(0),
                        pending_engine_rx: Mutex::new(None),
                        embedder,
                        embed_running: Default::default(),
                        llm,
                        rag_running: Default::default(),
                        rag_cancel: Default::default(),
                        topup_cancel: Default::default(),
                        topup_running: Default::default(),
                        cataloguing: Default::default(),
                        nav: Default::default(),
                        paused_until: Default::default(),
                        shortcut_ok: Default::default(),
                        engine_tx: Mutex::new(tx),
                        tray_items: Mutex::new(None),
                    });
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
                }
                aegis_core::db::crypto::VaultOpenResult::NeedsRecoveryKey { reason } => {
                    let store = SqliteStore::open_in_memory()?;
                    app.manage(AppState {
                        store: Mutex::new(store),
                        db_path,
                        db_key: Mutex::new(None),
                        vault_locked: std::sync::atomic::AtomicBool::new(true),
                        vault_lock_reason: Mutex::new(Some(reason)),
                        vault_migrated: std::sync::atomic::AtomicBool::new(false),
                        sensitive_unlocked_until: std::sync::atomic::AtomicI64::new(0),
                        pending_engine_rx: Mutex::new(Some(rx)),
                        embedder,
                        embed_running: Default::default(),
                        llm,
                        rag_running: Default::default(),
                        rag_cancel: Default::default(),
                        topup_cancel: Default::default(),
                        topup_running: Default::default(),
                        cataloguing: Default::default(),
                        nav: Default::default(),
                        paused_until: Default::default(),
                        shortcut_ok: Default::default(),
                        engine_tx: Mutex::new(tx),
                        tray_items: Mutex::new(None),
                    });
                    let items = shell::build_tray(&handle)?;
                    if let Ok(mut g) = app.state::<AppState>().tray_items.lock() {
                        *g = Some(items);
                    }
                    // Always open the window when recovery is needed so the owner sees the unlock screen.
                    shell::open_main(&handle, None);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                let state = window.app_handle().state::<AppState>();
                state.embedder.unload();
                state.llm.unload();
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
            commands::embed::embed_status,
            commands::embed::embed_sync_now,
            commands::embed::embed_select_model,
            commands::embed::embed_unload,
            commands::file_get,
            commands::file_set_lifecycle,
            commands::file_set_sensitivity,
            commands::vault::vault_status,
            commands::vault::vault_get_initial_recovery_key,
            commands::vault::vault_confirm_recovery_backup,
            commands::vault::vault_reveal_recovery_key,
            commands::vault::vault_unlock_with_recovery,
            commands::vault::vault_unlock_sensitive,
            commands::vault::vault_lock_sensitive,
            commands::care::care_scan_cleanup,
            commands::care::care_propose_cleanup,
            commands::care::care_installed_apps,
            commands::care::care_leftover_folders,
            commands::care::care_launch_uninstaller,
            commands::care::care_open_windows_apps,
            commands::care::care_startup_items,
            commands::care::care_open_startup_settings,
            commands::care::care_drive_health,
            commands::care::care_disk_trends,
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
            commands::context::context_projects,
            commands::context::context_recognising,
            commands::context::context_recognise_now,
            commands::context::context_answer,
            commands::context::context_plans,
            commands::context::context_run_plan,
            commands::context::context_reunite_plans,
            commands::context::context_run_reunite,
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
            commands::filing::filing_recent,
            commands::filing::search_facets,
            commands::filing::filing_rules,
            commands::filing::filing_rule_stop,
            commands::filing::filing_rule_delete,
            commands::rag::rag_status,
            commands::rag::rag_select_model,
            commands::rag::rag_unload,
            commands::rag::rag_cancel,
            commands::rag::rag_ask,
            commands::assets::assets_scan,
            commands::assets::assets_enrich_metadata,
            commands::assets::assets_dismiss,
            commands::assets::assets_undismiss,
            commands::assets::assets_propose_recovery,
            commands::versions::versions_scan,
            commands::versions::versions_diff,
            commands::versions::versions_summarize_diff,
            commands::versions::versions_pin_latest,
            commands::versions::versions_dismiss,
            commands::versions::versions_undismiss,
            commands::versions::versions_propose_archive,
            commands::versions::versions_propose_quarantine_dupes,
            commands::knowledge::knowledge_dashboard,
            commands::knowledge::knowledge_extract_scan,
            commands::knowledge::knowledge_extract_file_llm,
            commands::knowledge::knowledge_approve_proposal,
            commands::knowledge::knowledge_approve_all,
            commands::knowledge::knowledge_undo_proposal,
            commands::knowledge::knowledge_dismiss_proposal,
            commands::knowledge::knowledge_reopen_proposal,
            commands::knowledge::knowledge_set_entity_status,
            commands::knowledge::knowledge_set_audio_opt_in,
            commands::librarian::librarian_recent_docs,
            commands::librarian::librarian_related_for_file,
            commands::librarian::librarian_related_for_topic,
            commands::librarian::librarian_synthesize_brief,
            commands::librarian::librarian_pin,
            commands::librarian::librarian_unpin,
            commands::librarian::librarian_dismiss,
            commands::librarian::librarian_undismiss,
            commands::cross_project::cross_project_scan,
            commands::cross_project::cross_project_set_insight_status,
            commands::cross_project::cross_project_synthesize_brief,
            commands::folderless::folderless_list_rules,
            commands::folderless::folderless_compile_intent,
            commands::folderless::folderless_save_rule,
            commands::folderless::folderless_toggle_rule,
            commands::folderless::folderless_delete_rule,
            commands::folderless::folderless_preview_rule,
            commands::folderless::folderless_propose_batch,
            commands::folderless::folderless_recent_batches,
            commands::healing::healing_scan,
            commands::healing::healing_preview_patches,
            commands::healing::healing_propose_batch,
            commands::healing::healing_dismiss,
            commands::healing::healing_undismiss,
            commands::healing::healing_update_rule_path,
            commands::healing::healing_recent_batches,
            commands::healing::healing_run_doctor_check,
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
