//! Projects IPC (context layer, spec 2026-11-22 §8.4, §12). Recognition reads the whole catalog,
//! so these commands use their own database connection instead of holding the UI's.

use super::{AppState, map_err};
use aegis_core::context::plan::{PlanOutcome, Planner, ProjectPlan, propose, run};
use aegis_core::context::reunite::{ReunitePlan, propose_reunite, reunite_plans, run_reunite};
use aegis_core::context::store::list_collections_in;
use aegis_core::context::{Answer, Collection, CollectionKind, RecogniseReport, answer, recognise};
use aegis_core::db::SqliteStore;
use aegis_core::error::IpcError;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

/// One piece of project work at a time (a recognition pass, an answer, or a plan run): a
/// second request while one runs is refused, so a plan never moves folders mid-recognition.
static RECOGNISING: AtomicBool = AtomicBool::new(false);

fn own_store(state: &State<'_, AppState>) -> Result<SqliteStore, IpcError> {
    map_err(state.open_store())
}

fn busy(message: &str) -> IpcError {
    IpcError { code: "busy".into(), message: message.into(), retryable: true, remediation: None }
}

/// Clears the single-flight flag however the work ends.
struct Running;

impl Running {
    fn start() -> Result<Self, IpcError> {
        Self::start_with("Cairn is already looking for projects. It will be done in a moment.")
    }

    fn start_with(message: &str) -> Result<Self, IpcError> {
        RECOGNISING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map(|_| Running).map_err(|_| busy(message))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RECOGNISING.store(false, Ordering::Release);
    }
}

const PROJECT_LIST_KINDS: [CollectionKind; 3] = [CollectionKind::Project, CollectionKind::DesignSet, CollectionKind::OldCopy];

/// Every recognised project, design set, and old copy, those found in more than one place first.
#[tauri::command(async)]
pub fn context_projects(state: State<'_, AppState>) -> Result<Vec<Collection>, IpcError> {
    let store = own_store(&state)?;
    map_err(list_collections_in(store.conn(), &PROJECT_LIST_KINDS))
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

/// Every project's, design set's, and old copy's plans (one per drive it lives on), worked out
/// now from the saved collections.
#[tauri::command(async)]
pub fn context_plans(state: State<'_, AppState>) -> Result<Vec<ProjectPlan>, IpcError> {
    let store = own_store(&state)?;
    let projects = map_err(list_collections_in(store.conn(), &PROJECT_LIST_KINDS))?;
    let planner = map_err(Planner::new(&store))?;
    let mut out = Vec::new();
    for c in &projects {
        // One project's plan that can't be made never hides the others'.
        if let Ok(ps) = planner.plan_for(&store, c) {
            out.extend(ps);
        }
    }
    Ok(out)
}

/// Runs one project's plan on one drive: re-planned from the disk as it is now, verified
/// against the card digest the user reviewed, then every step or none. Undo goes through
/// Recent batches (or the card's Undo) like any batch.
#[tauri::command(async)]
pub fn context_run_plan(
    state: State<'_, AppState>,
    collection_id: String,
    volume_id: String,
    expected_digest: String,
) -> Result<PlanOutcome, IpcError> {
    let _running = Running::start_with("Cairn is busy with your projects. Try again in a moment.")?;
    let mut store = own_store(&state)?;
    let projects = map_err(list_collections_in(store.conn(), &PROJECT_LIST_KINDS))?;
    let stale = || IpcError {
        code: "not_found".into(),
        message: "This project changed since the card was shown. Look again and check its plan.".into(),
        retryable: false,
        remediation: None,
    };
    let c = projects.into_iter().find(|c| c.id == collection_id).ok_or_else(stale)?;
    let plan = map_err(Planner::new(&store).and_then(|p| p.plan_for(&store, &c)))?
        .into_iter()
        .find(|p| p.volume_id == volume_id)
        .ok_or_else(stale)?;
    if plan.digest != expected_digest {
        return Err(stale());
    }
    let batch = map_err(propose(&mut store, &c, &plan))?;
    map_err(run(&mut store, &batch))
}

/// "Put back together" plans: files moved away from a project or set by an earlier filing batch.
#[tauri::command(async)]
pub fn context_reunite_plans(state: State<'_, AppState>) -> Result<Vec<ReunitePlan>, IpcError> {
    let store = own_store(&state)?;
    map_err(reunite_plans(&store))
}

/// Runs one "Put back together" plan on one drive after verifying its digest: every file or none.
#[tauri::command(async)]
pub fn context_run_reunite(
    state: State<'_, AppState>,
    collection_id: String,
    volume_id: String,
    expected_digest: String,
) -> Result<PlanOutcome, IpcError> {
    let _running = Running::start_with("Cairn is busy with your projects. Try again in a moment.")?;
    let mut store = own_store(&state)?;
    let stale = || IpcError {
        code: "not_found".into(),
        message: "This group changed since it was shown. Refresh and check again.".into(),
        retryable: false,
        remediation: None,
    };
    let plan = map_err(reunite_plans(&store))?
        .into_iter()
        .find(|p| p.collection_id == collection_id && p.volume_id == volume_id)
        .ok_or_else(stale)?;
    if plan.digest != expected_digest {
        return Err(stale());
    }
    let batch = map_err(propose_reunite(&mut store, &plan))?;
    map_err(run_reunite(&mut store, &batch))
}
