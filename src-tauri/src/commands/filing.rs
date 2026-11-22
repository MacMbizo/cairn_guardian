//! Auto-filing IPC (spec §14). Planning a whole drive can take a while, so those commands use
//! their own database connection instead of holding the UI's.

use super::{AppState, lock_store, map_err};
use crate::engine_host::wake;
use aegis_core::contracts::RollbackResult;
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, rollback_batch};
use aegis_core::error::IpcError;
use aegis_core::filing::batches::{RecentBatch, RunReport, after_execute, after_rollback, next_batch, propose_placements, recent_batches, run_report};
use aegis_core::filing::blueprint::{BlueprintSummary, Placement, Target, destinations, plan, rule_key, summarize};
use aegis_core::filing::rules::{FilingRule, delete_rule, list_rules, stop_automatic};
use aegis_core::filing::{BlueprintChoice, Scope, ScopeStatus, change_blueprint, load_meta, scope_statuses};
use std::path::Path;
use tauri::{AppHandle, State};

fn own_store(state: &State<'_, AppState>) -> Result<SqliteStore, IpcError> {
    map_err(SqliteStore::open(&state.db_path))
}

#[tauri::command(async)]
pub fn filing_scopes(state: State<'_, AppState>) -> Result<Vec<ScopeStatus>, IpcError> {
    let mut store = own_store(&state)?;
    map_err(scope_statuses(&mut store))
}

/// Both structures for a scope, side by side: `(tidy, fresh)`.
#[tauri::command(async)]
pub fn filing_blueprints(state: State<'_, AppState>, scope: Scope) -> Result<(BlueprintSummary, BlueprintSummary), IpcError> {
    let store = own_store(&state)?;
    let one = |c: BlueprintChoice| map_err(plan(&store, &scope, c).and_then(|ps| summarize(&ps, c, &store)));
    Ok((one(BlueprintChoice::Tidy)?, one(BlueprintChoice::Fresh)?))
}

#[tauri::command(async)]
pub fn filing_choose(state: State<'_, AppState>, scope: Scope, choice: BlueprintChoice) -> Result<(), IpcError> {
    let mut store = lock_store(&state)?;
    map_err(change_blueprint(&mut store, &scope, choice))
}

/// Prepares the next batch (or returns the one already waiting).
#[tauri::command(async)]
pub fn filing_next_batch(state: State<'_, AppState>, scope: Scope) -> Result<Option<String>, IpcError> {
    let mut store = own_store(&state)?;
    if let Some(open) = map_err(aegis_core::filing::batches::open_filing_batch(store.conn(), &scope))? {
        return Ok(Some(open));
    }
    map_err(next_batch(&mut store, &scope))
}

/// Approves the ticked items (all when `selection` is empty), runs them and records the result,
/// with a plain reason for anything that couldn't move.
#[tauri::command(async)]
pub fn filing_approve(
    app: AppHandle,
    state: State<'_, AppState>,
    batch: String,
    selection: Option<Vec<String>>,
) -> Result<RunReport, IpcError> {
    let res = {
        let mut store = lock_store(&state)?;
        map_err(approve_batch(&mut store, &batch, selection.as_deref()))?;
        let res = map_err(execute_batch(&mut store, &batch))?;
        map_err(after_execute(&mut store, &batch))?;
        res
    };
    wake(&app);
    Ok(run_report(&res))
}

/// Undo a filing batch. Files that changed since can't always go back; the result lists them.
#[tauri::command(async)]
pub fn filing_undo(app: AppHandle, state: State<'_, AppState>, batch: String) -> Result<RollbackResult, IpcError> {
    let res = {
        let mut store = lock_store(&state)?;
        let res = rollback_batch(&mut store, &batch);
        // Whatever did go back is recorded, even if the undo stopped part-way.
        map_err(after_rollback(&mut store, &batch))?;
        map_err(res)?
    };
    wake(&app);
    Ok(res)
}

/// "Skip this folder": turned down, not offered again, and a "no" for the rules involved.
#[tauri::command(async)]
pub fn filing_skip(app: AppHandle, state: State<'_, AppState>, batch: String) -> Result<(), IpcError> {
    {
        let mut store = lock_store(&state)?;
        map_err(aegis_core::filing::batches::skip_batch(&mut store, &batch))?;
    }
    wake(&app);
    Ok(())
}

/// Items Aegis isn't sure about, for the owner to place.
#[tauri::command(async)]
pub fn filing_to_sort(state: State<'_, AppState>, scope: Scope) -> Result<Vec<Placement>, IpcError> {
    let store = own_store(&state)?;
    let Some(choice) = map_err(aegis_core::filing::blueprint_of(store.conn(), &scope))? else { return Ok(vec![]) };
    Ok(map_err(plan(&store, &scope, choice))?.into_iter().filter(|p| p.to_sort).take(500).collect())
}

/// Folders worth suggesting for each unsure item, keyed by file id (see `blueprint::destinations`).
#[tauri::command(async)]
pub fn filing_destinations(
    state: State<'_, AppState>,
    scope: Scope,
    file_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, Vec<String>>, IpcError> {
    let store = own_store(&state)?;
    map_err(destinations(&store, &scope, &file_ids))
}

/// The owner chose a folder for one item: moved at once, and remembered as an approval.
#[tauri::command(async)]
pub fn filing_place(
    app: AppHandle,
    state: State<'_, AppState>,
    scope: Scope,
    file_id: String,
    dest_folder: String,
) -> Result<(), IpcError> {
    {
        let mut store = lock_store(&state)?;
        let (path, name, size): (String, String, i64) = map_err(
            store
                .conn()
                .query_row("SELECT path, name, size FROM files WHERE id = ?1", [&file_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(Into::into),
        )?;
        let meta = map_err(load_meta(store.conn(), &file_id))?;
        let placement = Placement {
            target: Target::File { file_id: file_id.clone() },
            from: path,
            to: Some(Path::new(&dest_folder).join(&name).to_string_lossy().to_string()),
            new_name: None,
            confidence: 1.0,
            reason: "You chose this place".into(),
            rule_key: meta.as_ref().map(rule_key),
            to_sort: false,
            bytes: size,
        };
        let batch = map_err(propose_placements(&mut store, &scope, &format!("filing:{}", scope.key()), vec![placement]))?;
        map_err(approve_batch(&mut store, &batch, None))?;
        map_err(execute_batch(&mut store, &batch))?;
        map_err(after_execute(&mut store, &batch))?;
    }
    wake(&app);
    Ok(())
}

/// Filing batches that have run, newest first, so any of them can be undone.
#[tauri::command(async)]
pub fn filing_recent(state: State<'_, AppState>) -> Result<Vec<RecentBatch>, IpcError> {
    let store = lock_store(&state)?;
    map_err(recent_batches(store.conn(), 30))
}

#[tauri::command(async)]
pub fn filing_rules(state: State<'_, AppState>) -> Result<Vec<FilingRule>, IpcError> {
    let store = lock_store(&state)?;
    map_err(list_rules(store.conn()))
}

#[tauri::command(async)]
pub fn filing_rule_stop(state: State<'_, AppState>, id: String) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(stop_automatic(store.conn(), &id))
}

#[tauri::command(async)]
pub fn filing_rule_delete(state: State<'_, AppState>, id: String) -> Result<(), IpcError> {
    let store = lock_store(&state)?;
    map_err(delete_rule(store.conn(), &id))
}
