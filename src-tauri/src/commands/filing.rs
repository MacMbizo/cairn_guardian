//! Auto-filing IPC (spec §14). Planning a whole drive can take a while, so those commands use
//! their own database connection instead of holding the UI's.

use super::{AppState, lock_store, map_err};
use crate::engine_host::wake;
use aegis_core::contracts::ExecuteResult;
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, rollback_batch};
use aegis_core::error::IpcError;
use aegis_core::filing::batches::{after_execute, after_rollback, next_batch, propose_placements};
use aegis_core::filing::blueprint::{BlueprintSummary, Placement, Target, plan, rule_key, scope_roots, summarize};
use aegis_core::filing::rules::{FilingRule, delete_rule, list_rules, stop_automatic};
use aegis_core::filing::{BlueprintChoice, Scope, ScopeStatus, load_meta, scope_statuses, set_blueprint};
use aegis_core::now_ms;
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
    let store = lock_store(&state)?;
    map_err(set_blueprint(store.conn(), &scope, choice, now_ms()))
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

/// Approves the ticked items (all when `selection` is empty), runs them and records the result.
#[tauri::command(async)]
pub fn filing_approve(
    app: AppHandle,
    state: State<'_, AppState>,
    batch: String,
    selection: Option<Vec<String>>,
) -> Result<ExecuteResult, IpcError> {
    let res = {
        let mut store = lock_store(&state)?;
        map_err(approve_batch(&mut store, &batch, selection.as_deref()))?;
        let res = map_err(execute_batch(&mut store, &batch))?;
        map_err(after_execute(&mut store, &batch))?;
        res
    };
    wake(&app);
    Ok(res)
}

#[tauri::command(async)]
pub fn filing_undo(app: AppHandle, state: State<'_, AppState>, batch: String) -> Result<(), IpcError> {
    {
        let mut store = lock_store(&state)?;
        map_err(rollback_batch(&mut store, &batch))?;
        map_err(after_rollback(&mut store, &batch))?;
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

/// Folders worth suggesting for an unsure item: where filing put things most, then the
/// structure's catch-all.
#[tauri::command(async)]
pub fn filing_destinations(state: State<'_, AppState>, scope: Scope) -> Result<Vec<String>, IpcError> {
    let store = own_store(&state)?;
    let roots = map_err(scope_roots(&store, &scope))?;
    let mut stmt = map_err(store.conn().prepare(
        "SELECT new_path FROM file_history h JOIN action_batches b ON b.id = h.batch_id WHERE b.origin IN (?1, ?2)",
    ).map_err(Into::into))?;
    let paths: Vec<String> = stmt
        .query_map([format!("filing:{}", scope.key()), format!("filing-auto:{}", scope.key())], |r| r.get(0))
        .map_err(|e| aegis_core::error::AegisError::from(e).to_ipc())?
        .filter_map(|r| r.ok())
        .collect();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for p in paths {
        if let Some(d) = Path::new(&p).parent() {
            *counts.entry(d.to_string_lossy().to_string()).or_default() += 1;
        }
    }
    let mut top: Vec<(String, usize)> = counts.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut out: Vec<String> = top.into_iter().map(|(d, _)| d).take(3).collect();
    if let Some((root, _)) = roots.first() {
        let catch_all = Path::new(root).join("Documents").join("Other documents").to_string_lossy().to_string();
        if !out.contains(&catch_all) {
            out.push(catch_all);
        }
    }
    Ok(out)
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
