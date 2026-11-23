//! Saving and loading collections, their locations, loose set members and the owner's answers.

use super::sets::{location_fingerprint, location_id};
use super::{Collection, CollectionKind, Decision, DecisionKind, Location, ProjectKind, Relationship, SetMember};
use crate::error::Result;
use crate::now_ms;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use std::path::Path;

/// Replaces every previously computed collection (their folder locations and loose set members)
/// with `collections` and `members` in one transaction. The owner's answers live elsewhere and
/// are never touched.
pub fn save_collections_with_members(
    conn: &Connection,
    collections: &[(Collection, Option<String>)],
    members: &[SetMember],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM collection_members", [])?;
    tx.execute("DELETE FROM collection_locations", [])?;
    tx.execute("DELETE FROM collections", [])?;
    let at = now_ms();
    {
        let mut c_stmt = tx.prepare(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at, plan_state, fingerprint)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        let mut l_stmt = tx.prepare(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes,
                                               newest_mtime, shared_files, compared_by, is_repo, fingerprint, keep_here)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(volume_id, root_path) DO NOTHING",
        )?;
        let mut m_stmt = tx.prepare(
            "INSERT INTO collection_members (collection_id, file_id, volume_id, path, size, mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(collection_id, file_id) DO NOTHING",
        )?;
        for (c, fp) in collections {
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
                c.plan_state,
                fp
            ])?;
            if c.kind.is_folder_collection() {
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
        for m in members {
            m_stmt.execute(params![m.collection_id, m.file_id, m.volume_id, m.path, m.size, m.mtime])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replaces every previously computed collection (and its folder locations) with `collections`.
pub fn save_collections(conn: &Connection, collections: &[Collection]) -> Result<()> {
    let wrapped: Vec<(Collection, Option<String>)> = collections.iter().cloned().map(|c| (c, None)).collect();
    save_collections_with_members(conn, &wrapped, &[])
}

/// Synthesizes folder-level `Location` rows for a loose-file set from `collection_members`,
/// grouped by `(volume_id, parent_dir)`.
pub(crate) fn synthesize_set_locations(
    conn: &Connection,
    collection_id: &str,
    set_fp: &str,
    answers: &[Decision],
) -> Result<Vec<Location>> {
    let mut stmt = conn.prepare(
        "SELECT m.volume_id, m.path, m.size, m.mtime, COALESCE(v.status = 'online', 0)
         FROM collection_members m LEFT JOIN volumes v ON v.id = m.volume_id
         WHERE m.collection_id = ?1",
    )?;
    let rows: Vec<(String, String, i64, i64, bool)> = stmt
        .query_map([collection_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)? != 0)))?
        .collect::<rusqlite::Result<_>>()?;
    #[derive(Default)]
    struct Acc {
        volume_id: String,
        dir: String,
        files: i64,
        bytes: i64,
        newest: i64,
        online: bool,
    }
    let mut by_dir: BTreeMap<(String, String), Acc> = BTreeMap::new();
    for (vol, path, size, mtime, online) in rows {
        let dir = Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let key = (vol.to_lowercase(), dir.to_lowercase());
        let e = by_dir.entry(key).or_default();
        if e.dir.is_empty() {
            e.volume_id = vol;
            e.dir = dir;
        }
        e.files += 1;
        e.bytes += size;
        e.newest = e.newest.max(mtime);
        e.online = e.online || online;
    }
    let mut locs: Vec<Location> = by_dir
        .into_values()
        .map(|a| {
            let loc_fp = location_fingerprint(set_fp, &a.volume_id, &a.dir);
            let keep_here = answers
                .iter()
                .filter(|d| d.kind == DecisionKind::KeepHere && d.fingerprint_b.is_none() && d.fingerprint_a == loc_fp)
                .max_by_key(|d| d.at)
                .is_some_and(|d| d.value == "yes");
            Location {
                id: location_id(collection_id, &a.volume_id, &a.dir),
                collection_id: collection_id.to_string(),
                volume_id: a.volume_id,
                root_path: a.dir,
                relationship: Relationship::SetFiles,
                files: a.files,
                bytes: a.bytes,
                newest_mtime: a.newest,
                shared_files: a.files,
                compared_by: "name".into(),
                is_repo: false,
                online: a.online,
                fingerprint: loc_fp,
                keep_here,
            }
        })
        .collect();
    locs.sort_by_key(|l| (std::cmp::Reverse(l.files), l.root_path.to_lowercase()));
    Ok(locs)
}

/// Every saved collection matching `kinds`, in the order they were saved, with their locations
/// and stored collection fingerprint.
pub fn list_collections_with_fingerprints(conn: &Connection, kinds: &[CollectionKind]) -> Result<Vec<(Collection, Option<String>)>> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", kinds.len()).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, plan_state, fingerprint
         FROM collections WHERE kind IN ({placeholders}) ORDER BY rowid"
    );
    let mut stmt = conn.prepare(&sql)?;
    let kind_strs: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
    let rows = stmt.query_map(rusqlite::params_from_iter(kind_strs), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, f64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, Option<String>>(8)?,
            r.get::<_, Option<String>>(9)?,
        ))
    })?;
    let mut out: Vec<(Collection, Option<String>)> = Vec::new();
    for row in rows {
        let (id, kstr, name, pkind, confidence, state, evidence, subs, plan_state, fp) = row?;
        let Some(kind) = CollectionKind::from_db(&kstr) else { continue };
        out.push((
            Collection {
                id,
                kind,
                name,
                project_kind: pkind.map(|k| ProjectKind::from_db(&k)),
                confidence,
                state,
                evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                locations: Vec::new(),
                sub_projects: serde_json::from_str(&subs).unwrap_or_default(),
                plan_state,
            },
            fp,
        ));
    }
    let need_set_answers = out.iter().any(|(c, _)| c.kind.is_member_collection());
    let answers = if need_set_answers { decisions(conn)? } else { Vec::new() };
    let mut l_stmt = conn.prepare(
        "SELECT l.id, l.collection_id, l.volume_id, l.root_path, l.relationship, l.files, l.bytes, l.newest_mtime,
                l.shared_files, l.compared_by, l.is_repo, l.fingerprint, COALESCE(v.status = 'online', 0), l.keep_here
         FROM collection_locations l LEFT JOIN volumes v ON v.id = l.volume_id
         WHERE l.collection_id = ?1 ORDER BY l.rowid",
    )?;
    for (c, fp) in &mut out {
        if c.kind.is_folder_collection() {
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
        } else {
            let set_fp = fp.clone().unwrap_or_else(|| format!("set:{}", c.name.to_lowercase()));
            c.locations = synthesize_set_locations(conn, &c.id, &set_fp, &answers)?;
        }
    }
    Ok(out)
}

/// Every saved collection matching `kinds`, in the order they were saved, with their locations.
pub fn list_collections_in(conn: &Connection, kinds: &[CollectionKind]) -> Result<Vec<Collection>> {
    Ok(list_collections_with_fingerprints(conn, kinds)?.into_iter().map(|(c, _)| c).collect())
}

/// Every saved collection of `kind`, in the order they were saved, with their locations.
pub fn list_collections(conn: &Connection, kind: CollectionKind) -> Result<Vec<Collection>> {
    list_collections_in(conn, &[kind])
}

/// Stored fingerprint for a collection (used by loose sets).
pub fn collection_fingerprint(conn: &Connection, collection_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT fingerprint FROM collections WHERE id = ?1", [collection_id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

/// Member files belonging to one collection.
pub fn list_members(conn: &Connection, collection_id: &str) -> Result<Vec<SetMember>> {
    let mut stmt = conn.prepare(
        "SELECT collection_id, file_id, volume_id, path, size, mtime FROM collection_members WHERE collection_id = ?1 ORDER BY lower(path)",
    )?;
    let rows = stmt.query_map([collection_id], |r| {
        Ok(SetMember {
            collection_id: r.get(0)?,
            file_id: r.get(1)?,
            volume_id: r.get(2)?,
            path: r.get(3)?,
            size: r.get(4)?,
            mtime: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `(collection_id, name, confidence, state, member)` for all live members of collections of `kind`.
pub type MemberWithCollection = (String, String, f64, String, SetMember);

/// `(collection_id, name, sub_projects, confidence, state, member)` for all live members of
/// collections of `kind`.
pub type MemberWithCollectionMeta = (String, String, Vec<String>, f64, String, SetMember);

/// `(collection_id, name, confidence, state, member)` for all members of collections of `kind`.
pub fn list_members_with_collection(conn: &Connection, kind: CollectionKind) -> Result<Vec<MemberWithCollection>> {
    Ok(list_members_with_collection_meta(conn, kind)?
        .into_iter()
        .map(|(cid, name, _, conf, state, m)| (cid, name, conf, state, m))
        .collect())
}

/// `(collection_id, name, sub_projects, confidence, state, member)` for all live members of
/// collections of `kind`.
pub fn list_members_with_collection_meta(conn: &Connection, kind: CollectionKind) -> Result<Vec<MemberWithCollectionMeta>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name, c.sub_projects_json, c.confidence, c.state, m.file_id, f.volume_id, f.path, f.size, f.mtime
         FROM collection_members m
         JOIN collections c ON c.id = m.collection_id
         JOIN files f ON f.id = m.file_id
         WHERE c.kind = ?1 AND f.deleted_at IS NULL AND f.state NOT IN ('quarantine', 'missing')",
    )?;
    let rows = stmt.query_map([kind.as_str()], |r| {
        let cid: String = r.get(0)?;
        let subs_json: String = r.get(2)?;
        let subs: Vec<String> = serde_json::from_str(&subs_json).unwrap_or_default();
        Ok((
            cid.clone(),
            r.get(1)?,
            subs,
            r.get(3)?,
            r.get(4)?,
            SetMember {
                collection_id: cid,
                file_id: r.get(5)?,
                volume_id: r.get(6)?,
                path: r.get(7)?,
                size: r.get(8)?,
                mtime: r.get(9)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
