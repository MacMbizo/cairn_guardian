//! Keeps recognition current (spec §13): re-runs the recognition pass when the catalog or the
//! owner's answers changed since the last pass, at most once every ten minutes, while the PC is
//! calm. It raises no reminders of its own.

use crate::db::SqliteStore;
use crate::engine::{Finding, Watch};
use crate::error::Result;

const STAMP: &str = "catalog_stamp";

/// A cheap signature of the catalog and the owner's answers: it changes when files are added,
/// changed or removed, or an answer is given.
pub fn catalog_stamp(store: &SqliteStore) -> Result<String> {
    Ok(store.conn().query_row(
        "SELECT (SELECT COUNT(*) FROM files WHERE deleted_at IS NULL) || ':' ||
                (SELECT COALESCE(MAX(updated_at), 0) FROM files) || ':' ||
                (SELECT COUNT(*) FROM context_decisions) || ':' ||
                (SELECT COALESCE(MAX(at), 0) FROM context_decisions)",
        [],
        |r| r.get(0),
    )?)
}

/// Re-runs [`super::recognise`] when [`catalog_stamp`] moved.
#[derive(Default)]
pub struct ContextWatch;

impl Watch for ContextWatch {
    fn name(&self) -> &'static str {
        "context"
    }
    fn every_ms(&self) -> i64 {
        600_000
    }
    fn needs_idle(&self) -> bool {
        true
    }
    fn kinds(&self) -> &'static [&'static str] {
        &[]
    }
    fn run(&mut self, store: &mut SqliteStore, _now: i64) -> Result<Vec<Finding>> {
        let stamp = catalog_stamp(store)?;
        if store.get_setting(STAMP, "context")?.as_deref() == Some(stamp.as_str()) {
            return Ok(Vec::new());
        }
        super::recognise(store)?;
        store.set_setting(STAMP, "context", &stamp)?;
        Ok(Vec::new())
    }
}
