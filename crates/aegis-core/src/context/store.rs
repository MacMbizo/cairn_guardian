//! Saving and loading collections, their locations and the owner's answers.

use super::{Collection, CollectionKind, Decision, DecisionKind, Location, ProjectKind, Relationship};
use crate::error::Result;
use crate::now_ms;
use rusqlite::{Connection, params};

/// Replaces every previously computed collection (and its locations) with `collections`, in
/// one transaction. The owner's answers live elsewhere and are never touched.
pub fn save_collections(conn: &Connection, collections: &[Collection]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM collection_locations", [])?;
    tx.execute("DELETE FROM collections", [])?;
    let at = now_ms();
    {
        let mut c_stmt = tx.prepare(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at, plan_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        let mut l_stmt = tx.prepare(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes,
                                               newest_mtime, shared_files, compared_by, is_repo, fingerprint, keep_here)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(volume_id, root_path) DO NOTHING",
        )?;
        for c in collections {
            c_stmt.execute(params![
                c.id,
                c.kind.as_str(),
                c.name,
                c.project_kind.map(ProjectKind::as_str),
                c.confidence,
                c.state,
                serde_json::to_string(&c.evidence)?,
                serde_json::to_string(&c.sub_projects)?,
                at,
                c.plan_state
            ])?;
            for l in &c.locations {
                l_stmt.execute(params![
                    l.id,
                    c.id,
                    l.volume_id,
                    l.root_path,
                    l.relationship.as_str(),
                    l.files,
                    l.bytes,
                    l.newest_mtime,
                    l.shared_files,
                    l.compared_by,
                    i32::from(l.is_repo),
                    l.fingerprint,
                    i32::from(l.keep_here)
                ])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Every saved collection of `kind`, in the order they were saved, with their locations.
pub fn list_collections(conn: &Connection, kind: CollectionKind) -> Result<Vec<Collection>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_kind, confidence, state, evidence_json, sub_projects_json, plan_state
         FROM collections WHERE kind = ?1 ORDER BY rowid",
    )?;
    let mut out: Vec<Collection> = stmt
        .query_map([kind.as_str()], |r| {
            let evidence: String = r.get(5)?;
            let subs: String = r.get(6)?;
            Ok(Collection {
                id: r.get(0)?,
                kind,
                name: r.get(1)?,
                project_kind: r.get::<_, Option<String>>(2)?.map(|k| ProjectKind::from_db(&k)),
                confidence: r.get(3)?,
                state: r.get(4)?,
                evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                locations: Vec::new(),
                sub_projects: serde_json::from_str(&subs).unwrap_or_default(),
                plan_state: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut l_stmt = conn.prepare(
        "SELECT l.id, l.collection_id, l.volume_id, l.root_path, l.relationship, l.files, l.bytes, l.newest_mtime,
                l.shared_files, l.compared_by, l.is_repo, l.fingerprint, COALESCE(v.status = 'online', 0), l.keep_here
         FROM collection_locations l LEFT JOIN volumes v ON v.id = l.volume_id
         WHERE l.collection_id = ?1 ORDER BY l.rowid",
    )?;
    for c in &mut out {
        c.locations = l_stmt
            .query_map([&c.id], |r| {
                Ok(Location {
                    id: r.get(0)?,
                    collection_id: r.get(1)?,
                    volume_id: r.get(2)?,
                    root_path: r.get(3)?,
                    relationship: Relationship::from_db(&r.get::<_, String>(4)?),
                    files: r.get(5)?,
                    bytes: r.get(6)?,
                    newest_mtime: r.get(7)?,
                    shared_files: r.get(8)?,
                    compared_by: r.get(9)?,
                    is_repo: r.get::<_, i64>(10)? != 0,
                    fingerprint: r.get(11)?,
                    online: r.get::<_, i64>(12)? != 0,
                    keep_here: r.get::<_, i64>(13)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(out)
}

/// Records one of the owner's answers (an answer with the same id is replaced).
pub fn record_decision(conn: &Connection, d: &Decision) -> Result<()> {
    conn.execute(
        "INSERT INTO context_decisions (id, kind, fingerprint_a, fingerprint_b, value, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, fingerprint_a = excluded.fingerprint_a,
            fingerprint_b = excluded.fingerprint_b, value = excluded.value, at = excluded.at",
        params![d.id, d.kind.as_str(), d.fingerprint_a, d.fingerprint_b, d.value, d.at],
    )?;
    Ok(())
}

/// One project plan that was proposed or run: its batch, the drive it acts on, the fingerprints
/// of the locations it touched, and `proposed`, `done` or `undone`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRun {
    pub batch_id: String,
    pub volume_id: String,
    pub fingerprints: Vec<String>,
    pub state: String,
    pub at: i64,
}

/// Records (or updates) the run of a project plan.
pub fn record_plan_run(conn: &Connection, batch_id: &str, volume_id: &str, fingerprints: &[String], state: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO project_plan_runs (batch_id, volume_id, fingerprints_json, state, at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(batch_id) DO UPDATE SET volume_id = excluded.volume_id, fingerprints_json = excluded.fingerprints_json,
            state = excluded.state, at = excluded.at",
        params![batch_id, volume_id, serde_json::to_string(fingerprints)?, state, now_ms()],
    )?;
    Ok(())
}

/// Every recorded plan run, oldest first.
pub fn plan_runs(conn: &Connection) -> Result<Vec<PlanRun>> {
    let mut stmt =
        conn.prepare("SELECT batch_id, volume_id, fingerprints_json, state, at FROM project_plan_runs ORDER BY at, rowid")?;
    let rows = stmt.query_map([], |r| {
        let fps: String = r.get(2)?;
        Ok(PlanRun {
            batch_id: r.get(0)?,
            volume_id: r.get(1)?,
            fingerprints: serde_json::from_str(&fps).unwrap_or_default(),
            state: r.get(3)?,
            at: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Every answer the owner has given, oldest first.
pub fn decisions(conn: &Connection) -> Result<Vec<Decision>> {
    let mut stmt =
        conn.prepare("SELECT id, kind, fingerprint_a, fingerprint_b, value, at FROM context_decisions ORDER BY at, rowid")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, kind, a, b, value, at) = row?;
        if let Some(kind) = DecisionKind::from_db(&kind) {
            out.push(Decision { id, kind, fingerprint_a: a, fingerprint_b: b, value, at });
        }
    }
    Ok(out)
}
