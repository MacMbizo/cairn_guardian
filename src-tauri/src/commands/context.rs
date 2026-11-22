//! Projects IPC (context layer, spec 2026-11-22 §8.4, §12). Recognition reads the whole catalog,
//! so these commands use their own database connection instead of holding the UI's.

use super::{AppState, map_err};
use aegis_core::context::store::list_collections;
use aegis_core::context::{Answer, Collection, CollectionKind, RecogniseReport, answer, recognise};
use aegis_core::db::SqliteStore;
use aegis_core::error::IpcError;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

/// One recognition pass at a time: a second "Look again now" while one runs is refused.
static RECOGNISING: AtomicBool = AtomicBool::new(false);

fn own_store(state: &State<'_, AppState>) -> Result<SqliteStore, IpcError> {
    map_err(SqliteStore::open(&state.db_path))
}

fn busy() -> IpcError {
    IpcError {
        code: "busy".into(),
        message: "Aegis is already looking for projects. It will be done in a moment.".into(),
        retryable: true,
        remediation: None,
    }
}

/// Clears the single-flight flag however the pass ends.
struct Running;

impl Running {
    fn start() -> Result<Self, IpcError> {
        RECOGNISING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map(|_| Running).map_err(|_| busy())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RECOGNISING.store(false, Ordering::Release);
    }
}

/// Every recognised project, those found in more than one place first.
#[tauri::command(async)]
pub fn context_projects(state: State<'_, AppState>) -> Result<Vec<Collection>, IpcError> {
    let store = own_store(&state)?;
    map_err(list_collections(store.conn(), CollectionKind::Project))
}

/// Whether a recognition pass is running now (so a reopened screen shows it).
#[tauri::command]
pub fn context_recognising() -> bool {
    RECOGNISING.load(Ordering::Acquire)
}

/// Looks for projects again now.
#[tauri::command(async)]
pub fn context_recognise_now(state: State<'_, AppState>) -> Result<RecogniseReport, IpcError> {
    let _running = Running::start()?;
    let mut store = own_store(&state)?;
    map_err(recognise(&mut store))
}

/// Records one of the owner's answers about a project card (it re-runs recognition).
#[tauri::command(async)]
pub fn context_answer(
    state: State<'_, AppState>,
    collection_id: String,
    location_id: Option<String>,
    answer_given: Answer,
) -> Result<(), IpcError> {
    let _running = Running::start()?;
    let mut store = own_store(&state)?;
    map_err(answer(&mut store, &collection_id, location_id.as_deref(), answer_given))
}
