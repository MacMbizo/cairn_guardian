//! Put back together (spec 2026-11-22 §11): when an earlier filing batch moved files out of a
//! folder that is now recognised as a project or away from their design set / photo subject,
//! proposes one reversible batch per collection and drive to return them.

use super::plan::{
    PlanOutcome, design_projects_home_conn, existing_design_project_dir, inside, join, kind_home, leaf, list_roots_conn, parent,
    plain_reason, safe_name, same_path, unique_file_in_dir,
};
use super::sets::set_fingerprint;
use super::store::{list_collections_in, list_members, record_decision};
use super::{CollectionKind, Decision, DecisionKind, ProjectKind};
use crate::contracts::{ActionType, BatchStatus, ProposedActionInput, RefilePlan, RollbackResult, RootConfig, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::{approve_batch, execute_batch, propose_batch, rollback_batch};
use crate::db::files::path_is_within;
use crate::error::{AegisError, Result};
use crate::{new_id, now_ms};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use ts_rs::TS;

const SEP: char = std::path::MAIN_SEPARATOR;

/// One earlier filing batch whose moves split files belonging to this collection.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[ts(export)]
pub struct SplitBatchRef {
    pub batch_id: String,
    pub origin: String,
    #[ts(type = "number")]
    pub at: i64,
    pub files: usize,
}

/// One file returned to its project or reunited with its set/subject.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[ts(export)]
pub struct ReuniteItem {
    pub file_id: String,
    pub from: String,
    pub to: String,
    pub original_path: String,
    pub split_batch_id: String,
    #[ts(type = "number")]
    pub split_at: i64,
}

/// Proposal to put scattered files back together for one collection on one drive.
#[derive(TS, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[ts(export)]
pub struct ReunitePlan {
    pub collection_id: String,
    pub collection_name: String,
    pub collection_kind: CollectionKind,
    pub volume_id: String,
    pub target_dir: String,
    pub items: Vec<ReuniteItem>,
    pub split_batches: Vec<SplitBatchRef>,
    pub waits_for: Option<String>,
    pub summary: String,
    pub digest: String,
}

struct SplitFile {
    file_id: String,
    old_path: String,
    batch_id: String,
    origin: String,
    changed_at: i64,
    volume_id: String,
    live_path: String,
    mtime: i64,
    exif_taken_at: Option<i64>,
    doc_date: Option<String>,
}

fn subject_year(exif_taken_at: Option<i64>, doc_date: Option<&str>, mtime: i64) -> String {
    if let Some(t) = exif_taken_at {
        return crate::engine::note::date_of(t.div_euclid(86_400_000))[..4].to_string();
    }
    if let Some(d) = doc_date.filter(|d| d.len() >= 4) {
        return d[..4].to_string();
    }
    if mtime > 0 {
        return crate::engine::note::date_of(crate::filing::local_ms(mtime).div_euclid(86_400_000))[..4].to_string();
    }
    "Undated".into()
}

fn relative_after<'a>(root: &str, path: &'a str) -> Option<&'a str> {
    let rc = root.trim_end_matches(SEP).chars().count();
    path.char_indices().nth(rc + 1).map(|(b, _)| &path[b..])
}

/// Traces historical paths a project location moved from (`to_path` <- `from_path`), cycle-guarded.
fn historical_roots(cur_root: &str, rev_moves: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut q: VecDeque<(String, usize)> = VecDeque::new();
    let key = cur_root.trim_end_matches(SEP).to_lowercase();
    seen.insert(key);
    out.push(cur_root.to_string());
    q.push_back((cur_root.to_string(), 0));
    while let Some((cur, depth)) = q.pop_front() {
        if depth >= 8 {
            continue;
        }
        let k = cur.trim_end_matches(SEP).to_lowercase();
        if let Some(prevs) = rev_moves.get(&k) {
            for prev in prevs {
                let pk = prev.trim_end_matches(SEP).to_lowercase();
                if seen.insert(pk) {
                    out.push(prev.clone());
                    q.push_back((prev.clone(), depth + 1));
                }
            }
        }
    }
    out
}

fn pictures_root(roots: &[RootConfig], volume_id: &str, mount: &str, online: bool) -> Option<String> {
    if let Some(r) = roots
        .iter()
        .find(|r| r.enabled && r.volume_id == volume_id && matches!(leaf(&r.path).to_lowercase().as_str(), "pictures" | "my pictures"))
    {
        return Some(r.path.clone());
    }
    if !roots.iter().any(|r| r.enabled && r.volume_id == volume_id) {
        return Some(join(mount, "Photos"));
    }
    let pic = join(mount, "Pictures");
    (online && Path::new(&pic).is_dir()).then_some(pic)
}

fn reunite_digest(collection_id: &str, volume_id: &str, target_dir: &str, items: &[ReuniteItem]) -> String {
    let mut h = blake3::Hasher::new();
    let feed = |h: &mut blake3::Hasher, s: &str| {
        h.update(&(s.len() as u64).to_le_bytes());
        h.update(s.as_bytes());
    };
    feed(&mut h, collection_id);
    feed(&mut h, volume_id);
    feed(&mut h, target_dir);
    let mut sorted: Vec<(&str, &str, &str)> =
        items.iter().map(|i| (i.file_id.as_str(), i.from.as_str(), i.to.as_str())).collect();
    sorted.sort_unstable();
    h.update(&(sorted.len() as u64).to_le_bytes());
    for (fid, from, to) in sorted {
        feed(&mut h, fid);
        feed(&mut h, from);
        feed(&mut h, to);
    }
    h.finalize().to_hex().to_string()
}

/// Discovers "Put back together" plans from `conn` (used by both IPC and weekly-note builder).
pub fn reunite_plans_conn(conn: &Connection) -> Result<Vec<ReunitePlan>> {
    let mut split_files: Vec<SplitFile> = Vec::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT h.file_id, h.old_path, h.batch_id, b.origin, h.changed_at,
                    f.volume_id, f.path, f.mtime, m.exif_taken_at, m.doc_date
             FROM file_history h
             JOIN action_batches b ON b.id = h.batch_id
             JOIN files f ON f.id = h.file_id
             JOIN volumes v ON v.id = f.volume_id
             LEFT JOIN file_meta m ON m.file_id = f.id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND (b.origin LIKE 'filing:%' OR b.origin LIKE 'filing-auto:%')
             ORDER BY h.changed_at ASC, h.rowid ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SplitFile {
                file_id: r.get(0)?,
                old_path: r.get(1)?,
                batch_id: r.get(2)?,
                origin: r.get(3)?,
                changed_at: r.get(4)?,
                volume_id: r.get(5)?,
                live_path: r.get(6)?,
                mtime: r.get(7)?,
                exif_taken_at: r.get(8)?,
                doc_date: r.get(9)?,
            })
        })?;
        for row in rows {
            let sf = row?;
            if seen_files.insert(sf.file_id.clone()) && !same_path(&sf.live_path, &sf.old_path) {
                split_files.push(sf);
            }
        }
    }
    if split_files.is_empty() {
        return Ok(Vec::new());
    }

    let vols: HashMap<String, (String, String, bool)> = {
        let mut stmt =
            conn.prepare("SELECT id, mount_point, COALESCE(NULLIF(label, ''), mount_point), status = 'online' FROM volumes")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
            .collect::<rusqlite::Result<_>>()?
    };
    let enabled_roots: Vec<RootConfig> = list_roots_conn(conn)?.into_iter().filter(|r| r.enabled).collect();

    let mut rev_moves: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT from_path, to_path FROM actions
             WHERE target_kind = 'dir' AND action_type = 'move' AND status = 'executed' AND to_path IS NOT NULL
             ORDER BY executed_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (from_path, to_path) = row?;
            rev_moves.entry(to_path.trim_end_matches(SEP).to_lowercase()).or_default().push(from_path);
        }
    }

    let mut all_cols = list_collections_in(conn, &[CollectionKind::Project, CollectionKind::DesignSet, CollectionKind::PhotoSubject])?;
    all_cols.retain(|c| !matches!(c.state.as_str(), "left_as_is" | "not_a_project"));

    let mut all_proj_locs: Vec<(String, String, String)> = Vec::new();
    let mut project_info: HashMap<String, (String, Option<ProjectKind>, bool)> = HashMap::new();
    for c in all_cols.iter().filter(|c| c.kind == CollectionKind::Project) {
        let is_repo = c.locations.iter().any(|l| l.is_repo);
        project_info.insert(c.id.clone(), (c.name.clone(), c.project_kind, is_repo));
        for l in &c.locations {
            all_proj_locs.push((c.id.clone(), l.volume_id.clone(), l.root_path.clone()));
        }
    }
    let in_any_project_loc = |volume_id: &str, path: &str| {
        all_proj_locs
            .iter()
            .any(|(_, v, r)| v == volume_id && path_is_within(path, r, false))
    };

    let split_by_id: HashMap<&str, &SplitFile> = split_files.iter().map(|s| (s.file_id.as_str(), s)).collect();
    let mut split_by_vol: HashMap<&str, Vec<&SplitFile>> = HashMap::new();
    for s in &split_files {
        split_by_vol.entry(s.volume_id.as_str()).or_default().push(s);
    }

    let mut claimed: HashSet<String> = HashSet::new();
    let mut plans: Vec<ReunitePlan> = Vec::new();

    let push_plan = |plans: &mut Vec<ReunitePlan>,
                     cid: &str,
                     cname: &str,
                     ckind: CollectionKind,
                     volume_id: &str,
                     target_dir: String,
                     mut items: Vec<ReuniteItem>,
                     origins: HashMap<String, String>| {
        if items.is_empty() {
            return;
        }
        items.sort_by(|a, b| a.from.to_lowercase().cmp(&b.from.to_lowercase()).then_with(|| a.file_id.cmp(&b.file_id)));
        let mut by_batch: HashMap<String, (String, i64, usize)> = HashMap::new();
        for it in &items {
            let origin = origins.get(&it.split_batch_id).cloned().unwrap_or_default();
            let e = by_batch.entry(it.split_batch_id.clone()).or_insert((origin, it.split_at, 0));
            e.1 = e.1.max(it.split_at);
            e.2 += 1;
        }
        let mut split_batches: Vec<SplitBatchRef> = by_batch
            .into_iter()
            .map(|(batch_id, (origin, at, files))| SplitBatchRef { batch_id, origin, at, files })
            .collect();
        split_batches.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.batch_id.cmp(&a.batch_id)));
        let waits_for = match vols.get(volume_id) {
            Some((_, label, false)) => Some(format!("{label} is unplugged.")),
            None => Some("That drive isn't known any more.".into()),
            _ => None,
        };
        let n = items.len();
        let summary = format!("Puts {n} {} back into {target_dir}.", if n == 1 { "file" } else { "files" });
        let digest = reunite_digest(cid, volume_id, &target_dir, &items);
        plans.push(ReunitePlan {
            collection_id: cid.to_string(),
            collection_name: cname.to_string(),
            collection_kind: ckind,
            volume_id: volume_id.to_string(),
            target_dir,
            items,
            split_batches,
            waits_for,
            summary,
            digest,
        });
    };

    // Priority 1: Projects
    for c in all_cols.iter().filter(|c| c.kind == CollectionKind::Project) {
        let mut vols_here: Vec<&str> = Vec::new();
        for l in &c.locations {
            if !vols_here.contains(&l.volume_id.as_str()) {
                vols_here.push(&l.volume_id);
            }
        }
        for vol in vols_here {
            let Some((mount, _, _)) = vols.get(vol) else { continue };
            let Some(vol_splits) = split_by_vol.get(vol) else { continue };
            let mut locs: Vec<_> = c.locations.iter().filter(|l| l.volume_id == vol).collect();
            locs.sort_by_key(|l| (l.relationship != super::Relationship::Home, l.root_path.to_lowercase()));
            let Some(primary_target) = locs.first().map(|l| l.root_path.clone()) else { continue };
            let hist_by_loc: Vec<(&str, Vec<String>)> =
                locs.iter().map(|l| (l.root_path.as_str(), historical_roots(&l.root_path, &rev_moves))).collect();

            let mut items = Vec::new();
            let mut origins = HashMap::new();
            let mut dir_counts: HashMap<String, usize> = HashMap::new();
            for sf in vol_splits {
                if claimed.contains(&sf.file_id)
                    || in_any_project_loc(vol, &sf.live_path)
                    || !path_is_within(&sf.live_path, mount, false)
                {
                    continue;
                }
                let matched = hist_by_loc.iter().find_map(|(cur_root, hroots)| {
                    hroots.iter().find_map(|hr| {
                        inside(&sf.old_path, hr)
                            .then(|| relative_after(hr, &sf.old_path))
                            .flatten()
                            .map(|rel| (*cur_root, join(cur_root, rel)))
                    })
                });
                let Some((cur_root, dest)) = matched else { continue };
                if !path_is_within(&dest, mount, false) || same_path(&sf.live_path, &dest) {
                    continue;
                }
                claimed.insert(sf.file_id.clone());
                origins.insert(sf.batch_id.clone(), sf.origin.clone());
                *dir_counts.entry(cur_root.to_string()).or_default() += 1;
                items.push(ReuniteItem {
                    file_id: sf.file_id.clone(),
                    from: sf.live_path.clone(),
                    to: dest,
                    original_path: sf.old_path.clone(),
                    split_batch_id: sf.batch_id.clone(),
                    split_at: sf.changed_at,
                });
            }
            let target_dir = dir_counts
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(d, _)| d)
                .unwrap_or(primary_target);
            push_plan(&mut plans, &c.id, &c.name, c.kind, vol, target_dir, items, origins);
        }
    }

    // Priority 2: Design sets
    for c in all_cols.iter().filter(|c| c.kind == CollectionKind::DesignSet) {
        let members = list_members(conn, &c.id)?;
        let mut by_vol: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for m in members {
            by_vol.entry(m.volume_id).or_default().push(m.file_id);
        }
        for (vol, fids) in by_vol {
            let Some((mount, _, _)) = vols.get(&vol) else { continue };
            let Some(ph) = design_projects_home_conn(conn, &enabled_roots, &vol)? else { continue };
            let shelf = kind_home(&ph, c.project_kind);
            let target_dir = if c.project_kind.unwrap_or(ProjectKind::Design) == ProjectKind::Design {
                existing_design_project_dir(&all_proj_locs, &project_info, &vol, &ph, &shelf, &c.name)
                    .unwrap_or_else(|| join(&shelf, &safe_name(&c.name)))
            } else {
                join(&shelf, &safe_name(&c.name))
            };
            if !path_is_within(&target_dir, mount, false) {
                continue;
            }
            let mut items = Vec::new();
            let mut origins = HashMap::new();
            for fid in fids {
                if claimed.contains(&fid) {
                    continue;
                }
                let Some(sf) = split_by_id.get(fid.as_str()).copied() else { continue };
                if sf.volume_id != vol
                    || inside(&sf.live_path, &target_dir)
                    || in_any_project_loc(&vol, &sf.live_path)
                    || !path_is_within(&sf.live_path, mount, false)
                {
                    continue;
                }
                let dest = join(&target_dir, leaf(&sf.live_path));
                if !path_is_within(&dest, mount, false) || same_path(&sf.live_path, &dest) {
                    continue;
                }
                claimed.insert(fid);
                origins.insert(sf.batch_id.clone(), sf.origin.clone());
                items.push(ReuniteItem {
                    file_id: sf.file_id.clone(),
                    from: sf.live_path.clone(),
                    to: dest,
                    original_path: sf.old_path.clone(),
                    split_batch_id: sf.batch_id.clone(),
                    split_at: sf.changed_at,
                });
            }
            push_plan(&mut plans, &c.id, &c.name, c.kind, &vol, target_dir, items, origins);
        }
    }

    // Priority 3: Photo subjects
    for c in all_cols.iter().filter(|c| c.kind == CollectionKind::PhotoSubject) {
        let members = list_members(conn, &c.id)?;
        let mut by_vol: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for m in members {
            by_vol.entry(m.volume_id).or_default().push(m.file_id);
        }
        let subj = safe_name(&c.name);
        for (vol, fids) in by_vol {
            let Some((mount, _, online)) = vols.get(&vol) else { continue };
            let Some(proot) = pictures_root(&enabled_roots, &vol, mount, *online) else { continue };
            let mut items = Vec::new();
            let mut origins = HashMap::new();
            let mut dir_counts: HashMap<String, usize> = HashMap::new();
            for fid in fids {
                if claimed.contains(&fid) {
                    continue;
                }
                let Some(sf) = split_by_id.get(fid.as_str()).copied() else { continue };
                if sf.volume_id != vol
                    || in_any_project_loc(&vol, &sf.live_path)
                    || !path_is_within(&sf.live_path, mount, false)
                {
                    continue;
                }
                let year = subject_year(sf.exif_taken_at, sf.doc_date.as_deref(), sf.mtime);
                let dest_dir = join(&join(&proot, &year), &subj);
                if !path_is_within(&dest_dir, mount, false) || inside(&sf.live_path, &dest_dir) {
                    continue;
                }
                let dest = join(&dest_dir, leaf(&sf.live_path));
                if !path_is_within(&dest, mount, false) || same_path(&sf.live_path, &dest) {
                    continue;
                }
                claimed.insert(fid);
                origins.insert(sf.batch_id.clone(), sf.origin.clone());
                *dir_counts.entry(dest_dir).or_default() += 1;
                items.push(ReuniteItem {
                    file_id: sf.file_id.clone(),
                    from: sf.live_path.clone(),
                    to: dest,
                    original_path: sf.old_path.clone(),
                    split_batch_id: sf.batch_id.clone(),
                    split_at: sf.changed_at,
                });
            }
            if items.is_empty() {
                continue;
            }
            let target_dir = dir_counts
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(d, _)| d)
                .unwrap_or_else(|| join(&proot, &subj));
            push_plan(&mut plans, &c.id, &c.name, c.kind, &vol, target_dir, items, origins);
        }
    }

    plans.sort_by(|a, b| {
        let rank = |k: CollectionKind| match k {
            CollectionKind::Project => 0u8,
            CollectionKind::DesignSet => 1u8,
            CollectionKind::PhotoSubject => 2u8,
            _ => 3u8,
        };
        a.waits_for
            .is_some()
            .cmp(&b.waits_for.is_some())
            .then_with(|| rank(a.collection_kind).cmp(&rank(b.collection_kind)))
            .then_with(|| a.collection_name.to_lowercase().cmp(&b.collection_name.to_lowercase()))
            .then_with(|| a.volume_id.cmp(&b.volume_id))
    });
    Ok(plans)
}

/// Discovers "Put back together" plans across all active projects, design sets and photo subjects.
pub fn reunite_plans(store: &SqliteStore) -> Result<Vec<ReunitePlan>> {
    reunite_plans_conn(store.conn())
}

fn origin_for_reunite(store: &SqliteStore, p: &ReunitePlan) -> Result<String> {
    let roots = store.list_roots()?;
    let on_pc = p.items.iter().any(|it| roots.iter().any(|r| r.enabled && path_is_within(&it.to, &r.path, false)));
    let key = if on_pc { "pc".to_string() } else { p.volume_id.clone() };
    Ok(format!("reunite:{key}:{}", p.collection_name))
}

/// Proposes `plan` as one reversible file-move batch, refusing offline drives, missing sources,
/// or empty plans, and suffixing destination collisions instead of overwriting.
pub fn propose_reunite(store: &mut SqliteStore, plan: &ReunitePlan) -> Result<String> {
    if let Some(why) = &plan.waits_for {
        return Err(AegisError::InvalidInput(why.clone()));
    }
    if plan.items.is_empty() {
        return Err(AegisError::InvalidInput("Nothing to put back together.".into()));
    }
    let origin = origin_for_reunite(store, plan)?;
    store.conn().execute(
        "UPDATE actions SET status = 'superseded'
         WHERE status IN ('proposed', 'approved')
           AND batch_id IN (SELECT id FROM action_batches WHERE origin = ?1 AND status IN ('draft', 'approved'))",
        [&origin],
    )?;
    store.conn().execute(
        "UPDATE action_batches SET status = 'rejected'
         WHERE origin = ?1 AND status IN ('draft', 'approved')",
        [&origin],
    )?;

    let mut taken: HashSet<String> = HashSet::new();
    let mut actions = Vec::new();
    for (idx, it) in plan.items.iter().enumerate() {
        if !Path::new(&it.from).is_file() {
            return Err(AegisError::NotFound(format!(
                "{} is no longer there. Aegis will check again.",
                it.from
            )));
        }
        let dest = unique_file_in_dir(parent(&it.to), leaf(&it.to), &mut taken);
        let ev = serde_json::json!({
            "step": idx + 1,
            "kind": "reunite",
            "exact_dest": true,
            "collection_id": plan.collection_id,
            "collection_kind": plan.collection_kind,
            "project": plan.collection_name,
            "dest_dir": parent(&dest),
            "split_batch_id": it.split_batch_id,
            "original_path": it.original_path,
        });
        actions.push(ProposedActionInput {
            agent: Some("context.reunite".into()),
            action_type: ActionType::Move,
            target_kind: TargetKind::File,
            file_id: Some(it.file_id.clone()),
            atomic_unit_id: None,
            from_path: it.from.clone(),
            to_path: Some(dest),
            reason: "Split by an earlier filing batch: puts it back where it belongs.".into(),
            confidence: 0.95,
            evidence_json: ev.to_string(),
        });
    }
    propose_batch(store, &RefilePlan { origin, actions })
}

fn remove_empty_dirs_inside_roots(roots: &[RootConfig], dirs: HashSet<String>, also_design_parent: bool) {
    let strictly_inside = |p: &str| roots.iter().any(|r| !same_path(p, &r.path) && path_is_within(p, &r.path, false));
    for d in dirs {
        if strictly_inside(&d) && Path::new(&d).is_dir() {
            let _ = std::fs::remove_dir(&d);
        }
        if also_design_parent {
            let shelf = parent(&d);
            if leaf(shelf).eq_ignore_ascii_case("design") && strictly_inside(shelf) && Path::new(shelf).is_dir() {
                let _ = std::fs::remove_dir(shelf);
            }
        }
    }
}

fn cleanup_after_undo(store: &mut SqliteStore, batch_id: &str) -> Result<()> {
    let rows: Vec<(Option<String>, String)> = {
        let mut stmt = store.conn().prepare(
            "SELECT file_id, evidence_json FROM actions WHERE batch_id = ?1 AND status = 'rolled_back'",
        )?;
        stmt.query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let mut dest_dirs: HashSet<String> = HashSet::new();
    for (fid, ev) in rows {
        if let Some(fid) = fid {
            store.conn().execute(
                "UPDATE collection_members SET path = COALESCE((SELECT path FROM files WHERE id = ?1), path) WHERE file_id = ?1",
                [&fid],
            )?;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev)
            && let Some(d) = v.get("dest_dir").and_then(|x| x.as_str())
        {
            dest_dirs.insert(d.to_string());
        }
    }
    store.conn().execute(
        "DELETE FROM file_history WHERE batch_id = ?1 AND EXISTS (
            SELECT 1 FROM files f WHERE f.id = file_history.file_id AND lower(f.path) = lower(file_history.old_path))",
        [batch_id],
    )?;
    if !dest_dirs.is_empty() {
        let roots = store.list_roots()?;
        remove_empty_dirs_inside_roots(&roots, dest_dirs, true);
    }
    Ok(())
}

/// Runs a proposed reunite batch atomically: every file, or rollback cleanly on first failure.
pub fn run_reunite(store: &mut SqliteStore, batch_id: &str) -> Result<PlanOutcome> {
    approve_batch(store, batch_id, None)?;
    execute_batch(store, batch_id)?;
    let first_bad: Option<(String, String)> = store
        .conn()
        .query_row(
            "SELECT from_path, COALESCE(error_message, status) FROM actions
             WHERE batch_id = ?1 AND status NOT IN ('executed', 'superseded')
             ORDER BY step, created_at, id LIMIT 1",
            [batch_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((path, msg)) = first_bad {
        let why = plain_reason(&msg, &path);
        let reason = match rollback_batch(store, batch_id) {
            Ok(rb) if rb.status == BatchStatus::RolledBack && rb.conflicts.is_empty() => {
                format!("{why} Nothing was changed.")
            }
            Ok(rb) => format!(
                "{why} Automatic undo hit {} conflict(s). Open Recent batches to finish restoring.",
                rb.conflicts.len()
            ),
            Err(re) => format!(
                "{why} Automatic undo could not finish ({}). Open Recent batches to finish restoring.",
                plain_reason(&re.to_string(), &path)
            ),
        };
        let _ = cleanup_after_undo(store, batch_id);
        return Ok(PlanOutcome { batch_id: batch_id.to_string(), done: false, reason: Some(reason), moved_files: 0, freed_bytes: 0 });
    }

    let rows: Vec<(Option<String>, String, Option<String>, String)> = {
        let mut stmt = store.conn().prepare(
            "SELECT file_id, from_path, to_path, evidence_json FROM actions WHERE batch_id = ?1 AND status = 'executed'",
        )?;
        stmt.query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let moved_files = rows.len() as i64;
    let mut from_dirs: HashSet<String> = HashSet::new();
    let mut learned_photos: HashSet<String> = HashSet::new();
    for (fid, from_path, to_path, ev) in rows {
        from_dirs.insert(parent(&from_path).to_string());
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev) else { continue };
        let ckind = v.get("collection_kind").and_then(|x| x.as_str()).unwrap_or("");
        if let Some(fid) = fid {
            if ckind == "project" {
                store.conn().execute("DELETE FROM collection_members WHERE file_id = ?1", [&fid])?;
            } else if let Some(dest) = to_path {
                store.conn().execute("UPDATE collection_members SET path = ?1 WHERE file_id = ?2", params![dest, fid])?;
            }
        }
        if ckind == "photo_subject"
            && let Some(subj) = v.get("project").and_then(|x| x.as_str())
        {
            learned_photos.insert(subj.to_string());
        }
    }
    if !learned_photos.is_empty() {
        let at = now_ms();
        for subj in learned_photos {
            let val = format!("photo:{subj}");
            let exists: bool = store.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM context_decisions WHERE kind = ?1 AND lower(value) = lower(?2))",
                params![DecisionKind::LearnedSubject.as_str(), val],
                |r| r.get(0),
            )?;
            if !exists {
                record_decision(
                    store.conn(),
                    &Decision {
                        id: new_id(),
                        kind: DecisionKind::LearnedSubject,
                        fingerprint_a: set_fingerprint(&subj.to_lowercase()),
                        fingerprint_b: None,
                        value: val,
                        at,
                    },
                )?;
            }
        }
    }
    if !from_dirs.is_empty() {
        let roots = store.list_roots()?;
        remove_empty_dirs_inside_roots(&roots, from_dirs, false);
    }
    Ok(PlanOutcome { batch_id: batch_id.to_string(), done: true, reason: None, moved_files, freed_bytes: 0 })
}

/// Whether `batch_id` is a "Put back together" batch (undone with [`undo_reunite`]).
pub fn is_reunite_batch(conn: &Connection, batch_id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT origin LIKE 'reunite:%' FROM action_batches WHERE id = ?1", [batch_id], |r| r.get(0))
        .optional()?
        .unwrap_or(false))
}

/// Undoes a reunite batch, restores `collection_members.path`, clears its history, and removes
/// newly-emptied destination folders strictly inside configured roots.
pub fn undo_reunite(store: &mut SqliteStore, batch_id: &str) -> Result<RollbackResult> {
    let rb = rollback_batch(store, batch_id)?;
    cleanup_after_undo(store, batch_id)?;
    Ok(rb)
}
