//! Cross-Project Intelligence engine (`3.3`) (`K1`–`K5`, `R4`, `R5`, `R7`, `R9`, `R11`, `D1`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use rusqlite::{Connection, params};

use crate::db::files::{is_sensitive, path_is_within};
use crate::db::vec::{cosine_similarity, load_file_vectors};
use crate::error::{AegisError, Result};
use crate::librarian::engine::{
    clean_excerpt, dominant_vec_model, extract_distinctive_terms, tokenize_distinctive_set,
};
use crate::now_ms;
use crate::versions::patterns::is_excluded_relative_path;

use super::manifests::{is_manifest_filename, parse_manifest_deps};
use super::{
    CrossProjectBriefCitation, CrossProjectEdge, CrossProjectFileRef, CrossProjectFinding,
    CrossProjectOptions, CrossProjectReport, DependencyKind, FindingKind, FindingSeverity,
    InsightStatus, ProjectNodeSummary,
};

const MAX_DOCS_PER_PROJECT: usize = 30;
const MAX_TOTAL_VECTOR_DOCS: usize = 600;
const MAX_MANIFEST_READ_BYTES: i64 = 262_144;
const DAY_MS: i64 = 86_400_000;

const POSITIVE_MARKERS: &[&str] = &[
    "adopt",
    "adopted",
    "use",
    "using",
    "enable",
    "enabled",
    "require",
    "required",
    "mandate",
    "mandated",
    "standardize",
    "keep",
    "allow",
    "enforce",
    "prefer",
    "choose",
    "chose",
];

const NEGATIVE_MARKERS: &[&str] = &[
    "avoid",
    "ban",
    "banned",
    "deprecate",
    "deprecated",
    "disable",
    "disabled",
    "drop",
    "dropped",
    "forbid",
    "forbidden",
    "never",
    "prohibit",
    "prohibited",
    "reject",
    "rejected",
    "remove",
    "removed",
    "skip",
];

const EXCLUSIVE_TECH_PAIRS: &[(&str, &str)] = &[
    ("postgres", "mysql"),
    ("postgresql", "mysql"),
    ("postgres", "sqlite"),
    ("postgresql", "sqlite"),
    ("sqlite", "mysql"),
    ("local", "cloud"),
    ("offline", "cloud"),
    ("rest", "graphql"),
    ("monolith", "microservices"),
    ("sync", "async"),
    ("aws", "gcp"),
    ("aws", "azure"),
    ("gcp", "azure"),
];

struct CollectionMeta {
    collection_id: String,
    name: String,
    kind: String,
    project_kind: Option<String>,
    root_paths: Vec<String>,
}

struct CollectionLoc {
    collection_id: String,
    volume_id: String,
    root_path: String,
    case_sensitive: bool,
}

struct RawFileRecord {
    file_id: String,
    volume_id: String,
    mount_point: String,
    path: String,
    name: String,
    size: i64,
    mtime: i64,
    blake3: Option<Vec<u8>>,
    cloud_state: String,
    drive_online: bool,
    sensitivity: String,
}

struct DocCandidate {
    file_id: String,
    filename: String,
    path: String,
    collection_id: String,
    collection_name: String,
    mtime: i64,
    blake3: Option<Vec<u8>>,
    excerpt: String,
    combined_text: String,
    tokens: HashSet<String>,
}

struct ManifestEntry {
    raw_version: String,
    major_version: Option<u64>,
    file_ref: CrossProjectFileRef,
}

struct EntityProjectItem {
    entity_id: String,
    entity_type: String,
    name: String,
    status_tag: String,
    project_hint: Option<String>,
    summary: String,
    sample_file: CrossProjectFileRef,
}

struct ProjectAcc {
    meta: CollectionMeta,
    file_count: usize,
    indexed_doc_count: usize,
    embedded_doc_count: usize,
    total_bytes: i64,
    last_activity_at: i64,
    docs: Vec<DocCandidate>,
    manifests: HashMap<String, ManifestEntry>,
    entities: Vec<EntityProjectItem>,
    corpus_text: String,
    top_terms: Vec<String>,
    open_action_count: usize,
    decision_count: usize,
    people_count: usize,
}

struct TriageEntry {
    status: InsightStatus,
    note: String,
}

/// Prepared prompt + `[1]..[N]` citations for on-demand `OnDemandLlm` executive brief synthesis.
#[derive(Debug, Clone)]
pub struct PreparedCrossProjectBrief {
    pub title: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub citations: Vec<CrossProjectBriefCitation>,
}

fn ensure_sensitive_allowed(include_sensitive: bool, reveal_sensitive: bool) -> Result<()> {
    if include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Unlock sensitive previews with Windows authentication before including confidential or secret project files in Cross-Project Intelligence (R7)."
                .into(),
        ));
    }
    Ok(())
}

/// Word-boundary case-insensitive substring check for project names (`len >= 4`).
fn contains_project_mention(haystack_lower: &str, project_name_lower: &str) -> bool {
    if project_name_lower.len() < 4 || haystack_lower.is_empty() {
        return false;
    }
    let bytes = haystack_lower.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = haystack_lower[start..].find(project_name_lower) {
        let idx = start + rel;
        let end = idx + project_name_lower.len();
        let before_ok = idx == 0 || !bytes[idx - 1].is_ascii_alphanumeric();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = idx + project_name_lower.len();
    }
    false
}

fn doc_to_file_ref(doc: &DocCandidate) -> CrossProjectFileRef {
    CrossProjectFileRef {
        file_id: doc.file_id.clone(),
        filename: doc.filename.clone(),
        path: doc.path.clone(),
        collection_id: doc.collection_id.clone(),
        collection_name: doc.collection_name.clone(),
        mtime: doc.mtime,
        excerpt: doc.excerpt.clone(),
    }
}

fn load_triage_state(conn: &Connection) -> Result<HashMap<String, TriageEntry>> {
    let mut out = HashMap::new();
    let mut st = conn.prepare("SELECT insight_key, status, note FROM cross_project_insight_state")?;
    for row in st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (key, status, note) = row?;
        out.insert(
            key,
            TriageEntry {
                status: InsightStatus::from_db(&status),
                note,
            },
        );
    }
    Ok(out)
}

struct ActiveProjectsData {
    projects: Vec<ProjectAcc>,
    id_to_idx: HashMap<String, usize>,
    locations: Vec<CollectionLoc>,
    members: HashMap<String, String>,
}

fn load_active_projects(
    conn: &Connection,
    include_design_sets: bool,
) -> Result<ActiveProjectsData> {
    let kinds_sql = if include_design_sets {
        "kind IN ('project', 'design_set')"
    } else {
        "kind = 'project'"
    };
    let sql = format!(
        "SELECT id, name, kind, project_kind
         FROM collections
         WHERE state NOT IN ('left_as_is', 'not_a_project')
           AND {kinds_sql}
         ORDER BY name ASC"
    );
    let mut projects = Vec::new();
    let mut id_to_idx = HashMap::new();

    {
        let mut st = conn.prepare(&sql)?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })? {
            let (collection_id, name, kind, project_kind) = row?;
            let idx = projects.len();
            id_to_idx.insert(collection_id.clone(), idx);
            projects.push(ProjectAcc {
                meta: CollectionMeta {
                    collection_id,
                    name,
                    kind,
                    project_kind,
                    root_paths: Vec::new(),
                },
                file_count: 0,
                indexed_doc_count: 0,
                embedded_doc_count: 0,
                total_bytes: 0,
                last_activity_at: 0,
                docs: Vec::new(),
                manifests: HashMap::new(),
                entities: Vec::new(),
                corpus_text: String::new(),
                top_terms: Vec::new(),
                open_action_count: 0,
                decision_count: 0,
                people_count: 0,
            });
        }
    }

    let mut locations = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT cl.collection_id, cl.volume_id, cl.root_path, COALESCE(v.case_sensitive, 0)
             FROM collection_locations cl
             LEFT JOIN volumes v ON v.id = cl.volume_id
             ORDER BY cl.root_path ASC",
        )?;
        for row in st.query_map([], |r| {
            Ok(CollectionLoc {
                collection_id: r.get(0)?,
                volume_id: r.get(1)?,
                root_path: r.get(2)?,
                case_sensitive: r.get::<_, i64>(3)? != 0,
            })
        })? {
            let loc = row?;
            if let Some(&idx) = id_to_idx.get(&loc.collection_id) {
                if !projects[idx].meta.root_paths.contains(&loc.root_path) {
                    projects[idx].meta.root_paths.push(loc.root_path.clone());
                }
                locations.push(loc);
            }
        }
    }

    let mut members = HashMap::new();
    {
        let mut st = conn.prepare("SELECT file_id, collection_id FROM collection_members")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (fid, cid) = row?;
            if id_to_idx.contains_key(&cid) {
                members.insert(fid, cid);
            }
        }
    }

    Ok(ActiveProjectsData {
        projects,
        id_to_idx,
        locations,
        members,
    })
}

fn assign_collection_idx(
    f: &RawFileRecord,
    id_to_idx: &HashMap<String, usize>,
    locations: &[CollectionLoc],
    members: &HashMap<String, String>,
) -> Option<usize> {
    if let Some(cid) = members.get(&f.file_id) {
        return id_to_idx.get(cid).copied();
    }
    let mut best: Option<(usize, usize)> = None;
    for loc in locations {
        if loc.volume_id == f.volume_id && path_is_within(&f.path, &loc.root_path, loc.case_sensitive)
        {
            let len = loc.root_path.len();
            if best.is_none_or(|(bl, _)| len > bl)
                && let Some(&idx) = id_to_idx.get(&loc.collection_id)
            {
                best = Some((len, idx));
            }
        }
    }
    best.map(|(_, idx)| idx)
}

fn load_chunks_for_file(conn: &Connection, file_id: &str) -> Result<Vec<String>> {
    let mut st =
        conn.prepare("SELECT text FROM chunks WHERE file_id = ?1 ORDER BY seq ASC LIMIT 8")?;
    let rows = st.query_map(params![file_id], |r| r.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn read_manifest_text_fallback(f: &RawFileRecord, chunks: &[String]) -> Option<String> {
    if !chunks.is_empty() {
        return Some(chunks.join("\n"));
    }
    if f.drive_online
        && f.cloud_state == "local"
        && f.size >= 0
        && f.size <= MAX_MANIFEST_READ_BYTES
    {
        let p = Path::new(&f.path);
        if p.is_file()
            && let Ok(bytes) = std::fs::read(p)
        {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    None
}

fn populate_projects_and_files(
    conn: &Connection,
    projects: &mut [ProjectAcc],
    id_to_idx: &HashMap<String, usize>,
    locations: &[CollectionLoc],
    members: &HashMap<String, String>,
    allow_sensitive: bool,
) -> Result<(usize, HashMap<String, usize>)> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.volume_id, v.mount_point, f.path, f.name,
                f.size, f.mtime, f.blake3, f.cloud_state,
                COALESCE(v.status = 'online', 0), f.sensitivity
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, '') != 'backup'
         ORDER BY f.mtime DESC, f.name ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RawFileRecord {
            file_id: r.get(0)?,
            volume_id: r.get(1)?,
            mount_point: r.get(2)?,
            path: r.get(3)?,
            name: r.get(4)?,
            size: r.get(5)?,
            mtime: r.get(6)?,
            blake3: r.get(7)?,
            cloud_state: r.get(8)?,
            drive_online: r.get::<_, i64>(9)? != 0,
            sensitivity: r.get(10)?,
        })
    })?;

    let mut excluded_sensitive_count = 0usize;
    let mut allowed_file_to_project: HashMap<String, usize> = HashMap::new();
    let mut file_ref_map: HashMap<String, CrossProjectFileRef> = HashMap::new();

    for row_res in rows {
        let f = row_res?;
        if is_excluded_relative_path(&f.path, &f.mount_point) {
            continue;
        }
        let Some(p_idx) = assign_collection_idx(&f, id_to_idx, locations, members) else {
            continue;
        };
        if is_sensitive(&f.sensitivity) && !allow_sensitive {
            excluded_sensitive_count += 1;
            continue;
        }

        let proj = &mut projects[p_idx];
        proj.file_count += 1;
        proj.total_bytes = proj.total_bytes.saturating_add(f.size.max(0));
        if f.mtime > proj.last_activity_at {
            proj.last_activity_at = f.mtime;
        }
        allowed_file_to_project.insert(f.file_id.clone(), p_idx);

        let chunks = load_chunks_for_file(conn, &f.file_id)?;
        let excerpt = chunks
            .first()
            .map(|c| clean_excerpt(c, 220))
            .unwrap_or_else(|| f.name.clone());

        let fref = CrossProjectFileRef {
            file_id: f.file_id.clone(),
            filename: f.name.clone(),
            path: f.path.clone(),
            collection_id: proj.meta.collection_id.clone(),
            collection_name: proj.meta.name.clone(),
            mtime: f.mtime,
            excerpt: excerpt.clone(),
        };
        file_ref_map.insert(f.file_id.clone(), fref.clone());

        let is_manifest = is_manifest_filename(&f.name);
        if is_manifest
            && let Some(m_text) = read_manifest_text_fallback(&f, &chunks)
        {
            for dep in parse_manifest_deps(&f.name, &m_text) {
                proj.manifests.entry(dep.package).or_insert(ManifestEntry {
                    raw_version: dep.raw_version,
                    major_version: dep.major_version,
                    file_ref: fref.clone(),
                });
            }
        }

        if !chunks.is_empty() && !is_manifest {
            proj.indexed_doc_count += 1;
            let body = chunks.join("\n");
            let mut combined = f.name.replace(['_', '-', '.'], " ");
            combined.push('\n');
            combined.push_str(&body);
            if proj.corpus_text.len() < 24_000 {
                proj.corpus_text.push('\n');
                proj.corpus_text.push_str(&combined);
            }
            if proj.docs.len() < MAX_DOCS_PER_PROJECT {
                let tokens = tokenize_distinctive_set(&combined);
                proj.docs.push(DocCandidate {
                    file_id: f.file_id,
                    filename: f.name,
                    path: f.path,
                    collection_id: proj.meta.collection_id.clone(),
                    collection_name: proj.meta.name.clone(),
                    mtime: f.mtime,
                    blake3: f.blake3,
                    excerpt,
                    combined_text: combined,
                    tokens,
                });
            }
        }
    }

    for proj in projects.iter_mut() {
        proj.top_terms = extract_distinctive_terms(&proj.corpus_text, 10);
    }

    // Load approved Knowledge Graph entities scoped exclusively to allowed (`K2`/`K4`/`R7`) files.
    let mut ent_st = conn.prepare(
        "SELECT r.src_id, e.id, e.entity_type, e.name, e.attrs_json
         FROM relationships r
         JOIN entities e ON e.id = r.dst_id
         WHERE r.src_kind = 'file'
           AND r.dst_kind = 'entity'
           AND r.valid_to IS NULL",
    )?;
    let mut seen_proj_ent: HashSet<(usize, String)> = HashSet::new();
    for row in ent_st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })? {
        let (file_id, entity_id, entity_type, name, attrs_json) = row?;
        let Some(&p_idx) = allowed_file_to_project.get(&file_id) else {
            continue;
        };
        if !seen_proj_ent.insert((p_idx, entity_id.clone())) {
            continue;
        }
        let Some(sample_file) = file_ref_map.get(&file_id).cloned() else {
            continue;
        };
        let attrs: serde_json::Value =
            serde_json::from_str(&attrs_json).unwrap_or_else(|_| serde_json::json!({}));
        let status_tag = attrs
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("open")
            .to_ascii_lowercase();
        let project_hint = attrs
            .get("project_hint")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let summary = attrs
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let proj = &mut projects[p_idx];
        match entity_type.as_str() {
            "person" => proj.people_count += 1,
            "decision" => proj.decision_count += 1,
            "action_item" if status_tag != "done" && status_tag != "closed" => {
                proj.open_action_count += 1;
            }
            _ => {}
        }
        proj.entities.push(EntityProjectItem {
            entity_id,
            entity_type,
            name,
            status_tag,
            project_hint,
            summary,
            sample_file,
        });
    }

    Ok((excluded_sensitive_count, allowed_file_to_project))
}

fn detect_duplicate_effort(
    projects: &[ProjectAcc],
    vecs: &HashMap<String, Vec<f32>>,
    min_vec_sim: f64,
    triage: &HashMap<String, TriageEntry>,
    findings: &mut Vec<CrossProjectFinding>,
) {
    for i in 0..projects.len() {
        for j in (i + 1)..projects.len() {
            let pa = &projects[i];
            let pb = &projects[j];
            let mut pair_hits: Vec<CrossProjectFinding> = Vec::new();

            for da in &pa.docs {
                for db in &pb.docs {
                    let exact_hash = da
                        .blake3
                        .as_deref()
                        .filter(|h| !h.is_empty())
                        .is_some_and(|ha| db.blake3.as_deref() == Some(ha));

                    let shared_terms: Vec<String> = {
                        let mut inter: Vec<String> =
                            da.tokens.intersection(&db.tokens).cloned().collect();
                        inter.sort();
                        inter.truncate(8);
                        inter
                    };

                    let both_embedded =
                        vecs.contains_key(&da.file_id) && vecs.contains_key(&db.file_id);
                    let sim = if exact_hash {
                        1.0
                    } else if let (Some(va), Some(vb)) =
                        (vecs.get(&da.file_id), vecs.get(&db.file_id))
                    {
                        cosine_similarity(va, vb).clamp(0.0, 1.0)
                    } else if !da.tokens.is_empty() && !db.tokens.is_empty() {
                        let inter_count = da.tokens.intersection(&db.tokens).count();
                        let union_count = da.tokens.union(&db.tokens).count().max(1);
                        let jaccard = inter_count as f64 / union_count as f64;
                        if inter_count >= 4 && jaccard >= 0.42 {
                            jaccard.clamp(0.0, 0.99)
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                    let passes = exact_hash
                        || (both_embedded && sim >= min_vec_sim)
                        || (!both_embedded && sim >= 0.42);
                    if !passes {
                        continue;
                    }

                    let (older, newer) = if da.mtime <= db.mtime {
                        (da, db)
                    } else {
                        (db, da)
                    };
                    let age_delta_days = ((newer.mtime - older.mtime).abs() / DAY_MS).max(0);
                    let pct = (sim * 100.0).round() as i64;
                    let key = if da.file_id <= db.file_id {
                        format!("dup:{}:{}", da.file_id, db.file_id)
                    } else {
                        format!("dup:{}:{}", db.file_id, da.file_id)
                    };

                    let severity = if exact_hash || sim >= 0.88 {
                        FindingSeverity::High
                    } else {
                        FindingSeverity::Medium
                    };
                    let title = if exact_hash {
                        format!(
                            "Identical file duplicated across {} & {}",
                            older.collection_name, newer.collection_name
                        )
                    } else {
                        format!(
                            "Overlapping work across {} & {}",
                            older.collection_name, newer.collection_name
                        )
                    };
                    let when_phrase = if age_delta_days == 0 {
                        "on the same day".to_string()
                    } else {
                        format!("{age_delta_days} day(s) earlier")
                    };
                    let summary = format!(
                        "\"{}\" in {} closely matches \"{}\" created in {} {} ({}% similarity).",
                        newer.filename,
                        newer.collection_name,
                        older.filename,
                        older.collection_name,
                        when_phrase,
                        pct
                    );

                    let st_entry = triage.get(&key);
                    pair_hits.push(CrossProjectFinding {
                        insight_key: key,
                        kind: FindingKind::DuplicateEffort,
                        severity,
                        status: st_entry.map(|t| t.status).unwrap_or(InsightStatus::Open),
                        note: st_entry.map(|t| t.note.clone()).unwrap_or_default(),
                        title,
                        summary,
                        project_a_id: older.collection_id.clone(),
                        project_a_name: older.collection_name.clone(),
                        project_b_id: newer.collection_id.clone(),
                        project_b_name: newer.collection_name.clone(),
                        similarity: sim,
                        age_delta_days,
                        shared_terms,
                        file_a: Some(doc_to_file_ref(older)),
                        file_b: Some(doc_to_file_ref(newer)),
                    });
                }
            }

            pair_hits.sort_by(|a, b| {
                b.similarity
                    .total_cmp(&a.similarity)
                    .then_with(|| a.insight_key.cmp(&b.insight_key))
            });
            pair_hits.truncate(2);
            findings.extend(pair_hits);
        }
    }
}

fn has_any_marker(text_lower: &str, markers: &[&str]) -> bool {
    text_lower
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .any(|tok| markers.contains(&tok))
}

fn detect_decision_conflict(a: &EntityProjectItem, b: &EntityProjectItem) -> Option<Vec<String>> {
    if a.entity_id == b.entity_id {
        return None;
    }
    let text_a = format!("{} {}", a.name, a.summary);
    let text_b = format!("{} {}", b.name, b.summary);
    let low_a = text_a.to_ascii_lowercase();
    let low_b = text_b.to_ascii_lowercase();

    let stop_markers: HashSet<&str> = POSITIVE_MARKERS
        .iter()
        .chain(NEGATIVE_MARKERS.iter())
        .copied()
        .collect();

    let subj_a: HashSet<String> = tokenize_distinctive_set(&text_a)
        .into_iter()
        .filter(|t| !stop_markers.contains(t.as_str()))
        .collect();
    let subj_b: HashSet<String> = tokenize_distinctive_set(&text_b)
        .into_iter()
        .filter(|t| !stop_markers.contains(t.as_str()))
        .collect();

    let mut shared: Vec<String> = subj_a.intersection(&subj_b).cloned().collect();
    shared.sort();

    let pos_a = has_any_marker(&low_a, POSITIVE_MARKERS);
    let neg_a = has_any_marker(&low_a, NEGATIVE_MARKERS);
    let pos_b = has_any_marker(&low_b, POSITIVE_MARKERS);
    let neg_b = has_any_marker(&low_b, NEGATIVE_MARKERS);
    let polarity_clash = (pos_a && !neg_a && neg_b) || (pos_b && !neg_b && neg_a);

    let mut tech_clash = false;
    for (x, y) in EXCLUSIVE_TECH_PAIRS {
        let a_has_x = contains_project_mention(&low_a, x);
        let a_has_y = contains_project_mention(&low_a, y);
        let b_has_x = contains_project_mention(&low_b, x);
        let b_has_y = contains_project_mention(&low_b, y);
        if (a_has_x && !a_has_y && b_has_y && !b_has_x)
            || (a_has_y && !a_has_x && b_has_x && !b_has_y)
        {
            tech_clash = true;
            if !shared.contains(&x.to_string()) {
                shared.push((*x).to_string());
            }
            if !shared.contains(&y.to_string()) {
                shared.push((*y).to_string());
            }
            break;
        }
    }

    if (polarity_clash && shared.len() >= 2) || (tech_clash && !shared.is_empty()) {
        shared.truncate(6);
        Some(shared)
    } else {
        None
    }
}

fn detect_conflicting_decisions(
    projects: &[ProjectAcc],
    triage: &HashMap<String, TriageEntry>,
    findings: &mut Vec<CrossProjectFinding>,
) {
    let mut seen_keys = HashSet::new();
    for i in 0..projects.len() {
        for j in (i + 1)..projects.len() {
            let pa = &projects[i];
            let pb = &projects[j];
            for da in pa.entities.iter().filter(|e| e.entity_type == "decision") {
                for db in pb.entities.iter().filter(|e| e.entity_type == "decision") {
                    let Some(shared_terms) = detect_decision_conflict(da, db) else {
                        continue;
                    };
                    let key = if da.entity_id <= db.entity_id {
                        format!("conflict:{}:{}", da.entity_id, db.entity_id)
                    } else {
                        format!("conflict:{}:{}", db.entity_id, da.entity_id)
                    };
                    if !seen_keys.insert(key.clone()) {
                        continue;
                    }
                    let st_entry = triage.get(&key);
                    findings.push(CrossProjectFinding {
                        insight_key: key,
                        kind: FindingKind::ConflictingDecision,
                        severity: FindingSeverity::High,
                        status: st_entry.map(|t| t.status).unwrap_or(InsightStatus::Open),
                        note: st_entry.map(|t| t.note.clone()).unwrap_or_default(),
                        title: format!(
                            "Conflicting decisions between {} & {}",
                            pa.meta.name, pb.meta.name
                        ),
                        summary: format!(
                            "{} decided \"{}\" whereas {} recorded \"{}\".",
                            pa.meta.name, da.name, pb.meta.name, db.name
                        ),
                        project_a_id: pa.meta.collection_id.clone(),
                        project_a_name: pa.meta.name.clone(),
                        project_b_id: pb.meta.collection_id.clone(),
                        project_b_name: pb.meta.name.clone(),
                        similarity: 0.90,
                        age_delta_days: ((da.sample_file.mtime - db.sample_file.mtime).abs()
                            / DAY_MS)
                            .max(0),
                        shared_terms,
                        file_a: Some(da.sample_file.clone()),
                        file_b: Some(db.sample_file.clone()),
                    });
                }
            }
        }
    }
}

fn detect_version_divergence(
    projects: &[ProjectAcc],
    triage: &HashMap<String, TriageEntry>,
    findings: &mut Vec<CrossProjectFinding>,
) {
    for i in 0..projects.len() {
        for j in (i + 1)..projects.len() {
            let pa = &projects[i];
            let pb = &projects[j];
            for (pkg, ma) in &pa.manifests {
                let Some(mb) = pb.manifests.get(pkg) else {
                    continue;
                };
                let (Some(maj_a), Some(maj_b)) = (ma.major_version, mb.major_version) else {
                    continue;
                };
                if maj_a == maj_b || maj_a.max(maj_b) == 0 {
                    continue;
                }
                let (min_col, max_col) = if pa.meta.collection_id <= pb.meta.collection_id {
                    (&pa.meta.collection_id, &pb.meta.collection_id)
                } else {
                    (&pb.meta.collection_id, &pa.meta.collection_id)
                };
                let key = format!("ver:{min_col}:{max_col}:{pkg}");
                let st_entry = triage.get(&key);
                let major_gap = maj_a.abs_diff(maj_b);
                let severity = if major_gap >= 2 {
                    FindingSeverity::High
                } else {
                    FindingSeverity::Medium
                };
                findings.push(CrossProjectFinding {
                    insight_key: key,
                    kind: FindingKind::VersionDivergence,
                    severity,
                    status: st_entry.map(|t| t.status).unwrap_or(InsightStatus::Open),
                    note: st_entry.map(|t| t.note.clone()).unwrap_or_default(),
                    title: format!("Dependency \"{pkg}\" major version drift"),
                    summary: format!(
                        "{} targets {} {} while {} targets {} {}.",
                        pa.meta.name,
                        pkg,
                        ma.raw_version,
                        pb.meta.name,
                        pkg,
                        mb.raw_version
                    ),
                    project_a_id: pa.meta.collection_id.clone(),
                    project_a_name: pa.meta.name.clone(),
                    project_b_id: pb.meta.collection_id.clone(),
                    project_b_name: pb.meta.name.clone(),
                    similarity: 0.72,
                    age_delta_days: ((ma.file_ref.mtime - mb.file_ref.mtime).abs() / DAY_MS)
                        .max(0),
                    shared_terms: vec![pkg.clone()],
                    file_a: Some(ma.file_ref.clone()),
                    file_b: Some(mb.file_ref.clone()),
                });
            }
        }
    }
}

fn push_unique_evidence(target: &mut Vec<CrossProjectFileRef>, cand: CrossProjectFileRef) {
    if target.len() < 4 && !target.iter().any(|x| x.file_id == cand.file_id) {
        target.push(cand);
    }
}

fn detect_dependency_edges(projects: &[ProjectAcc]) -> Vec<CrossProjectEdge> {
    let mut edges = Vec::new();

    for i in 0..projects.len() {
        for j in (i + 1)..projects.len() {
            let pa = &projects[i];
            let pb = &projects[j];

            let (min_col, min_name, max_col, max_name) =
                if pa.meta.collection_id <= pb.meta.collection_id {
                    (
                        &pa.meta.collection_id,
                        &pa.meta.name,
                        &pb.meta.collection_id,
                        &pb.meta.name,
                    )
                } else {
                    (
                        &pb.meta.collection_id,
                        &pb.meta.name,
                        &pa.meta.collection_id,
                        &pa.meta.name,
                    )
                };

            // 1. Shared stakeholders (`person` / `organization`).
            let mut stake_labels = BTreeSet::new();
            let mut stake_files = Vec::new();
            for ea in pa
                .entities
                .iter()
                .filter(|e| matches!(e.entity_type.as_str(), "person" | "organization"))
            {
                if let Some(eb) = pb
                    .entities
                    .iter()
                    .find(|eb| eb.entity_id == ea.entity_id)
                {
                    stake_labels.insert(ea.name.clone());
                    push_unique_evidence(&mut stake_files, ea.sample_file.clone());
                    push_unique_evidence(&mut stake_files, eb.sample_file.clone());
                }
            }
            if !stake_labels.is_empty() {
                let labels: Vec<String> = stake_labels.into_iter().take(6).collect();
                edges.push(CrossProjectEdge {
                    edge_key: format!("stake:{min_col}:{max_col}"),
                    kind: DependencyKind::SharedStakeholder,
                    directed: false,
                    from_project_id: min_col.clone(),
                    from_project_name: min_name.clone(),
                    to_project_id: max_col.clone(),
                    to_project_name: max_name.clone(),
                    strength: (labels.len() as f64 / 3.0).clamp(0.45, 1.0),
                    summary: format!(
                        "{} and {} share stakeholder(s): {}",
                        min_name,
                        max_name,
                        labels.join(", ")
                    ),
                    shared_labels: labels,
                    evidence_files: stake_files,
                });
            }

            // 2. Shared approved decisions (`decision`).
            let mut dec_labels = BTreeSet::new();
            let mut dec_files = Vec::new();
            for ea in pa.entities.iter().filter(|e| e.entity_type == "decision") {
                if let Some(eb) = pb
                    .entities
                    .iter()
                    .find(|eb| eb.entity_id == ea.entity_id)
                {
                    dec_labels.insert(ea.name.clone());
                    push_unique_evidence(&mut dec_files, ea.sample_file.clone());
                    push_unique_evidence(&mut dec_files, eb.sample_file.clone());
                }
            }
            if !dec_labels.is_empty() {
                let labels: Vec<String> = dec_labels.into_iter().take(4).collect();
                edges.push(CrossProjectEdge {
                    edge_key: format!("dec:{min_col}:{max_col}"),
                    kind: DependencyKind::SharedDecision,
                    directed: false,
                    from_project_id: min_col.clone(),
                    from_project_name: min_name.clone(),
                    to_project_id: max_col.clone(),
                    to_project_name: max_name.clone(),
                    strength: (labels.len() as f64 / 2.0).clamp(0.60, 1.0),
                    summary: format!(
                        "{} and {} share approved decision(s): {}",
                        min_name,
                        max_name,
                        labels.join("; ")
                    ),
                    shared_labels: labels,
                    evidence_files: dec_files,
                });
            }

            // 3. Shared dependency stack (>= 2 common manifest packages).
            let mut shared_pkgs: Vec<String> = pa
                .manifests
                .keys()
                .filter(|k| pb.manifests.contains_key(*k))
                .cloned()
                .collect();
            shared_pkgs.sort();
            if shared_pkgs.len() >= 2 {
                let mut stack_files = Vec::new();
                for pkg in shared_pkgs.iter().take(2) {
                    if let Some(ma) = pa.manifests.get(pkg) {
                        push_unique_evidence(&mut stack_files, ma.file_ref.clone());
                    }
                    if let Some(mb) = pb.manifests.get(pkg) {
                        push_unique_evidence(&mut stack_files, mb.file_ref.clone());
                    }
                }
                let shown: Vec<String> = shared_pkgs.into_iter().take(8).collect();
                edges.push(CrossProjectEdge {
                    edge_key: format!("stack:{min_col}:{max_col}"),
                    kind: DependencyKind::SharedStack,
                    directed: false,
                    from_project_id: min_col.clone(),
                    from_project_name: min_name.clone(),
                    to_project_id: max_col.clone(),
                    to_project_name: max_name.clone(),
                    strength: (shown.len() as f64 / 5.0).clamp(0.40, 0.95),
                    summary: format!(
                        "{} and {} share {} stack dependencies ({})",
                        min_name,
                        max_name,
                        shown.len(),
                        shown.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
                    ),
                    shared_labels: shown,
                    evidence_files: stack_files,
                });
            }
        }
    }

    // Directed dependencies (`CrossProjectAction` & `DocumentReference`) for every ordered pair (src != dst).
    for src_idx in 0..projects.len() {
        for dst_idx in 0..projects.len() {
            if src_idx == dst_idx {
                continue;
            }
            let src = &projects[src_idx];
            let dst = &projects[dst_idx];
            let dst_low = dst.meta.name.trim().to_ascii_lowercase();
            if dst_low.len() < 4 {
                continue;
            }

            // 4. Cross-project action items in `src` pointing at `dst`.
            let mut action_labels = Vec::new();
            let mut action_files = Vec::new();
            for ent in src
                .entities
                .iter()
                .filter(|e| e.entity_type == "action_item" && e.status_tag != "done")
            {
                let hint_match = ent
                    .project_hint
                    .as_deref()
                    .is_some_and(|h| contains_project_mention(&h.to_ascii_lowercase(), &dst_low));
                let body_low = format!("{} {}", ent.name, ent.summary).to_ascii_lowercase();
                if hint_match || contains_project_mention(&body_low, &dst_low) {
                    if action_labels.len() < 5 {
                        action_labels.push(ent.name.clone());
                    }
                    push_unique_evidence(&mut action_files, ent.sample_file.clone());
                }
            }
            if !action_labels.is_empty() {
                edges.push(CrossProjectEdge {
                    edge_key: format!("act:{}:{}", src.meta.collection_id, dst.meta.collection_id),
                    kind: DependencyKind::CrossProjectAction,
                    directed: true,
                    from_project_id: src.meta.collection_id.clone(),
                    from_project_name: src.meta.name.clone(),
                    to_project_id: dst.meta.collection_id.clone(),
                    to_project_name: dst.meta.name.clone(),
                    strength: (0.65 + 0.15 * action_labels.len() as f64).min(1.0),
                    summary: format!(
                        "{} has {} action item(s) referencing {}",
                        src.meta.name,
                        action_labels.len(),
                        dst.meta.name
                    ),
                    shared_labels: action_labels,
                    evidence_files: action_files,
                });
            }

            // 5. Explicit document mentions of `dst` inside `src` docs.
            let mut doc_files = Vec::new();
            for d in &src.docs {
                if contains_project_mention(&d.combined_text.to_ascii_lowercase(), &dst_low) {
                    push_unique_evidence(&mut doc_files, doc_to_file_ref(d));
                }
            }
            if !doc_files.is_empty() {
                let doc_names: Vec<String> =
                    doc_files.iter().map(|f| f.filename.clone()).collect();
                edges.push(CrossProjectEdge {
                    edge_key: format!(
                        "docref:{}:{}",
                        src.meta.collection_id, dst.meta.collection_id
                    ),
                    kind: DependencyKind::DocumentReference,
                    directed: true,
                    from_project_id: src.meta.collection_id.clone(),
                    from_project_name: src.meta.name.clone(),
                    to_project_id: dst.meta.collection_id.clone(),
                    to_project_name: dst.meta.name.clone(),
                    strength: (0.45 + 0.15 * doc_files.len() as f64).min(0.95),
                    summary: format!(
                        "Documents in {} explicitly reference {} ({})",
                        src.meta.name,
                        dst.meta.name,
                        doc_names.join(", ")
                    ),
                    shared_labels: doc_names,
                    evidence_files: doc_files,
                });
            }
        }
    }

    edges
}

/// Scans active project folders on disk for duplicate effort, conflicting decisions,
/// version drift, and cross-functional dependencies (`K1`–`K5`, `R4`, `R5`, `R7`, `D1`).
pub fn scan_cross_project_intelligence(
    conn: &Connection,
    options: &CrossProjectOptions,
    reveal_sensitive: bool,
) -> Result<CrossProjectReport> {
    let started = Instant::now();
    let include_sensitive = options.include_sensitive.unwrap_or(false);
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let allow_sensitive = include_sensitive && reveal_sensitive;

    let include_design_sets = options.include_design_sets.unwrap_or(true);
    let include_dismissed = options.include_dismissed.unwrap_or(false);
    let limit_findings = options.limit_findings.unwrap_or(80).clamp(1, 300);
    let limit_edges = options.limit_edges.unwrap_or(100).clamp(1, 300);

    let triage = load_triage_state(conn)?;
    let ActiveProjectsData {
        mut projects,
        id_to_idx,
        locations,
        members,
    } = load_active_projects(conn, include_design_sets)?;

    let (excluded_sensitive_count, _allowed_map) = populate_projects_and_files(
        conn,
        &mut projects,
        &id_to_idx,
        &locations,
        &members,
        allow_sensitive,
    )?;

    let dominant = dominant_vec_model(conn)?;
    let mut vec_file_ids = Vec::new();
    for p in &projects {
        for d in &p.docs {
            if vec_file_ids.len() >= MAX_TOTAL_VECTOR_DOCS {
                break;
            }
            vec_file_ids.push(d.file_id.clone());
        }
    }
    let vecs = load_file_vectors(conn, &vec_file_ids)?;
    let used_vectors = !vecs.is_empty();
    for p in projects.iter_mut() {
        p.embedded_doc_count = p
            .docs
            .iter()
            .filter(|d| vecs.contains_key(&d.file_id))
            .count();
    }

    let default_sim = match dominant.as_ref().map(|(m, _)| m.as_str()) {
        Some(m) if m.contains("lexical") => 0.42,
        _ => 0.78,
    };
    let min_vec_sim = options.min_similarity.unwrap_or(default_sim).clamp(0.1, 1.0);

    let mut raw_findings = Vec::new();
    detect_duplicate_effort(&projects, &vecs, min_vec_sim, &triage, &mut raw_findings);
    detect_conflicting_decisions(&projects, &triage, &mut raw_findings);
    detect_version_divergence(&projects, &triage, &mut raw_findings);

    let raw_edges = detect_dependency_edges(&projects);

    // Filter by optional `project_id`, `kind`, and `include_dismissed`.
    let pid_filter = options.project_id.as_deref();
    let mut dismissed_count = 0usize;
    let mut findings = Vec::new();
    for f in raw_findings {
        if let Some(pid) = pid_filter
            && f.project_a_id != pid
            && f.project_b_id != pid
        {
            continue;
        }
        if let Some(k) = options.kind
            && f.kind != k
        {
            continue;
        }
        if f.status == InsightStatus::Dismissed {
            dismissed_count += 1;
            if !include_dismissed {
                continue;
            }
        }
        findings.push(f);
    }

    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| b.similarity.total_cmp(&a.similarity))
            .then_with(|| a.insight_key.cmp(&b.insight_key))
    });

    let mut edges: Vec<CrossProjectEdge> = raw_edges
        .into_iter()
        .filter(|e| {
            pid_filter.is_none_or(|pid| e.from_project_id == pid || e.to_project_id == pid)
        })
        .collect();
    edges.sort_by(|a, b| {
        b.strength
            .total_cmp(&a.strength)
            .then_with(|| a.edge_key.cmp(&b.edge_key))
    });

    // Aggregate per-project cross-link & finding counters prior to truncation.
    let mut related_sets: HashMap<String, HashSet<String>> = HashMap::new();
    let mut finding_counts: HashMap<String, usize> = HashMap::new();
    for f in &findings {
        if f.status != InsightStatus::Dismissed {
            *finding_counts.entry(f.project_a_id.clone()).or_insert(0) += 1;
            *finding_counts.entry(f.project_b_id.clone()).or_insert(0) += 1;
            related_sets
                .entry(f.project_a_id.clone())
                .or_default()
                .insert(f.project_b_id.clone());
            related_sets
                .entry(f.project_b_id.clone())
                .or_default()
                .insert(f.project_a_id.clone());
        }
    }
    for e in &edges {
        related_sets
            .entry(e.from_project_id.clone())
            .or_default()
            .insert(e.to_project_id.clone());
        related_sets
            .entry(e.to_project_id.clone())
            .or_default()
            .insert(e.from_project_id.clone());
    }

    let duplicate_effort_count = findings
        .iter()
        .filter(|f| {
            f.kind == FindingKind::DuplicateEffort && f.status != InsightStatus::Dismissed
        })
        .count();
    let conflict_count = findings
        .iter()
        .filter(|f| {
            matches!(
                f.kind,
                FindingKind::ConflictingDecision | FindingKind::VersionDivergence
            ) && f.status != InsightStatus::Dismissed
        })
        .count();
    let dependency_edge_count = edges.len();

    findings.truncate(limit_findings);
    edges.truncate(limit_edges);

    let mut project_summaries: Vec<ProjectNodeSummary> = projects
        .into_iter()
        .map(|p| {
            let related_project_count = related_sets
                .get(&p.meta.collection_id)
                .map_or(0, HashSet::len);
            let finding_count = finding_counts
                .get(&p.meta.collection_id)
                .copied()
                .unwrap_or(0);
            ProjectNodeSummary {
                collection_id: p.meta.collection_id,
                name: p.meta.name,
                kind: p.meta.kind,
                project_kind: p.meta.project_kind,
                root_paths: p.meta.root_paths,
                file_count: p.file_count,
                indexed_doc_count: p.indexed_doc_count,
                embedded_doc_count: p.embedded_doc_count,
                total_bytes: p.total_bytes,
                last_activity_at: p.last_activity_at,
                open_action_count: p.open_action_count,
                decision_count: p.decision_count,
                people_count: p.people_count,
                related_project_count,
                finding_count,
                top_terms: p.top_terms,
            }
        })
        .collect();
    project_summaries.sort_by(|a, b| {
        b.finding_count
            .cmp(&a.finding_count)
            .then_with(|| b.related_project_count.cmp(&a.related_project_count))
            .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(CrossProjectReport {
        total_projects: project_summaries.len(),
        projects: project_summaries,
        findings,
        edges,
        duplicate_effort_count,
        conflict_count,
        dependency_edge_count,
        dismissed_count,
        excluded_sensitive_count,
        used_vectors,
        took_ms: started.elapsed().as_millis() as u64,
    })
}

/// Persists reversible triage status & optional note for an insight inside SQLCipher (`K1`, `D1`).
pub fn set_insight_status(
    conn: &Connection,
    insight_key: &str,
    status: InsightStatus,
    note: Option<&str>,
) -> Result<bool> {
    let trimmed_key = insight_key.trim();
    if trimmed_key.is_empty() {
        return Err(AegisError::InvalidInput(
            "Insight key must not be empty.".into(),
        ));
    }
    let trimmed_note = note.unwrap_or("").trim();
    if status == InsightStatus::Open && trimmed_note.is_empty() {
        conn.execute(
            "DELETE FROM cross_project_insight_state WHERE insight_key = ?1",
            params![trimmed_key],
        )?;
        return Ok(true);
    }
    conn.execute(
        "INSERT INTO cross_project_insight_state (insight_key, status, note, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(insight_key) DO UPDATE SET
           status = excluded.status,
           note = CASE WHEN excluded.note != '' THEN excluded.note ELSE cross_project_insight_state.note END,
           updated_at = excluded.updated_at",
        params![trimmed_key, status.as_str(), trimmed_note, now_ms()],
    )?;
    Ok(true)
}

/// Prepares a strictly grounded prompt + `[1]..[N]` citations for `OnDemandLlm` synthesis (`K2`, `K4`, `K5`, `R7`).
pub fn prepare_cross_project_brief(
    conn: &Connection,
    project_a_id: Option<&str>,
    project_b_id: Option<&str>,
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<PreparedCrossProjectBrief> {
    ensure_sensitive_allowed(include_sensitive, reveal_sensitive)?;
    let opts = CrossProjectOptions {
        project_id: project_a_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        include_sensitive: Some(include_sensitive),
        ..Default::default()
    };
    let report = scan_cross_project_intelligence(conn, &opts, reveal_sensitive)?;

    let matches_pair = |a: &str, b: &str| -> bool {
        match (
            project_a_id.map(str::trim).filter(|s| !s.is_empty()),
            project_b_id.map(str::trim).filter(|s| !s.is_empty()),
        ) {
            (Some(pa), Some(pb)) => (a == pa && b == pb) || (a == pb && b == pa),
            (Some(pa), None) => a == pa || b == pa,
            _ => true,
        }
    };

    let mut citations = Vec::new();
    let mut seen_fids = HashSet::new();
    let mut highlight_lines = Vec::new();

    for f in report
        .findings
        .iter()
        .filter(|f| matches_pair(&f.project_a_id, &f.project_b_id))
        .take(4)
    {
        highlight_lines.push(format!("- [Finding: {}] {}", f.title, f.summary));
        for opt_ref in [&f.file_a, &f.file_b] {
            if citations.len() >= 6 {
                break;
            }
            if let Some(rf) = opt_ref
                && seen_fids.insert(rf.file_id.clone())
            {
                let ref_id = citations.len() + 1;
                citations.push(CrossProjectBriefCitation {
                    ref_id,
                    file_id: rf.file_id.clone(),
                    filename: rf.filename.clone(),
                    path: rf.path.clone(),
                    project_name: rf.collection_name.clone(),
                    excerpt: rf.excerpt.clone(),
                });
            }
        }
    }

    for e in report
        .edges
        .iter()
        .filter(|e| matches_pair(&e.from_project_id, &e.to_project_id))
        .take(4)
    {
        highlight_lines.push(format!("- [Dependency] {}", e.summary));
        for rf in &e.evidence_files {
            if citations.len() >= 6 {
                break;
            }
            if seen_fids.insert(rf.file_id.clone()) {
                let ref_id = citations.len() + 1;
                citations.push(CrossProjectBriefCitation {
                    ref_id,
                    file_id: rf.file_id.clone(),
                    filename: rf.filename.clone(),
                    path: rf.path.clone(),
                    project_name: rf.collection_name.clone(),
                    excerpt: rf.excerpt.clone(),
                });
            }
        }
    }

    if citations.is_empty() {
        return Err(AegisError::InvalidInput(
            "No non-sensitive cross-project overlaps, conflicts, or dependencies found to synthesize."
                .into(),
        ));
    }

    let title = match (project_a_id, project_b_id) {
        (Some(a), Some(b)) => {
            let na = report
                .projects
                .iter()
                .find(|p| p.collection_id == a)
                .map(|p| p.name.as_str())
                .unwrap_or(a);
            let nb = report
                .projects
                .iter()
                .find(|p| p.collection_id == b)
                .map(|p| p.name.as_str())
                .unwrap_or(b);
            format!("Cross-Project Synthesis — {na} ↔ {nb}")
        }
        (Some(a), None) => {
            let na = report
                .projects
                .iter()
                .find(|p| p.collection_id == a)
                .map(|p| p.name.as_str())
                .unwrap_or(a);
            format!("Cross-Project Synthesis — {na}")
        }
        _ => "Portfolio Cross-Project Intelligence Brief".to_string(),
    };

    let citation_blocks: Vec<String> = citations
        .iter()
        .map(|c| {
            format!(
                "[{}] {} (Project {}): {}",
                c.ref_id, c.filename, c.project_name, c.excerpt
            )
        })
        .collect();

    let system_prompt = "You are Cairn Guardian's offline Cross-Project Intelligence analyst. \
         Synthesize duplicate effort, architectural conflicts, and cross-functional dependencies \
         across the user's local projects strictly from the verified facts and numbered file \
         citations ([1], [2], ...). Never invent file names or facts; cite [N] inline."
        .to_string();

    let user_prompt = format!(
        "Detected cross-project signals:\n{}\n\nSource excerpts:\n{}\n\n\
         Write a crisp 3-5 sentence executive summary identifying duplicated work to consolidate, \
         any decisions/versions to reconcile, and critical cross-project dependencies with [N] citations.",
        highlight_lines.join("\n"),
        citation_blocks.join("\n\n")
    );

    Ok(PreparedCrossProjectBrief {
        title,
        system_prompt,
        user_prompt,
        citations,
    })
}

#[cfg(test)]
mod tests {
    use super::contains_project_mention;

    #[test]
    fn contains_project_mention_respects_word_boundaries() {
        assert!(contains_project_mention(
            "depends on helios storage milestone",
            "helios"
        ));
        assert!(!contains_project_mention(
            "heliostat mirror calibration test",
            "helios"
        ));
        assert!(!contains_project_mention("short", "abc"));
    }
}
